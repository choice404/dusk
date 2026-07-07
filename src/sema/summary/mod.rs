//! Interprocedural escape function summaries. M5 (0.5.0), compute-only.
//!
//! dusk's escape analysis in `typeck` is flow-sensitive but intraprocedural: it
//! catches a slice into a frame-local array or a closure over a local returned
//! *directly*, but it cannot see a view laundered through a call. A trivial
//! `func passthrough(s: int64[]) -> int64[] { return s }`, invoked on a view of
//! a frame-local array and returned, dangles with no diagnostic. This pass
//! computes, for every function, a summary of how its *view-typed* parameters
//! relate to its return value and to one another, so the enforcement step in
//! `typeck` can propagate escape across call boundaries.
//!
//! This pass is *compute-only*: it produces the summary table and emits no
//! diagnostics. `typeck` reads the table at each call site, so a call that
//! returns one of its frame arguments, or stores one into another argument's
//! place, is caught with the intraprocedural walk left unchanged. The table is
//! also exercised (and cannot panic) over every real program and the whole
//! standard library, since it is computed on every `sema::check`.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::diag::Span;
use crate::parser::ast::*;

mod hof;
mod place;
mod walk;

/// The names a function binds once, by a straight `name := g` to a module
/// function and never reassigns, so a call of the name resolves to that
/// function's known relation. Re-exported for the type checker's leaf-frame
/// send check, which treats such a binding as the resolved function rather than
/// an opaque callee.
pub(crate) use hof::resolvable_fn_binds;

/// A small set of parameter indices, backed by a `u64` bitmask. A function with
/// more than 64 parameters is beyond any real program, so an index past the
/// bound is simply dropped from the set; this is the only place the summary
/// under-approximates, and it is guarded by an absurd bound.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ParamSet(u64);

impl ParamSet {
    pub(crate) fn single(i: u8) -> Self {
        if i < 64 {
            ParamSet(1u64 << i)
        } else {
            // A function with more than 64 parameters is beyond any real program.
            // Rather than drop the index (bottom, which would silently claim the
            // relation does not hold), saturate to the top of the lattice so the
            // over-approximation stays sound: every representable index is set,
            // forcing the conservative reading at every call site.
            ParamSet(!0)
        }
    }

    pub(crate) fn contains(self, i: u8) -> bool {
        i < 64 && self.0 & (1u64 << i) != 0
    }

    pub(crate) fn union(self, o: ParamSet) -> Self {
        ParamSet(self.0 | o.0)
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn iter(self) -> impl Iterator<Item = u8> {
        (0..64u8).filter(move |&i| self.contains(i))
    }

    /// The indices in ascending order, for assertions.
    pub fn to_vec(self) -> Vec<u8> {
        self.iter().collect()
    }
}

/// What one function does to its view-typed parameters. `returns_alias` names the
/// parameters whose view may reach the return value; `flows_into` records that
/// arg `i`'s view may be stored into a place reachable through arg `j` (a view
/// parameter or a managed-pointer parameter); `reads_through` names the managed
/// pointer parameters `j` such that the return value may expose a view reachable
/// through `*j` (the return is that pointer itself, or a value read back out of
/// the heap object it addresses); `sinks` names the parameters `j` whose value or
/// pointee is handed to `chan_send`/`chan_try_send`, directly, through a helper
/// that itself sinks its argument, or through a store edge into a sunk place (a
/// parameter whose view `flows_into` a sunk one is itself a sink). All four are
/// least-fixpoint results: empty is the initial (bottom) summary, and each can
/// only grow.
///
/// `reads_through` is what lets the enforcement follow a view laundered through a
/// heap object: a store edge polluted a caller-visible object through a pointer
/// argument, and reading the object back out (a pointer passthrough, or a
/// `vec_get`-style element read) hands the frame view to the caller. At the call
/// site the caller applies `value_escape` of its own argument `j`, so only a
/// pointer whose object actually holds a frame view trips it; a heap-backed
/// object read back the same way stays clean.
///
/// `sinks` is what lets the enforcement follow a value laundered across a thread
/// boundary through a helper: `relay(ch, c) { chan_send(ch, c) }` sinks parameter
/// 1, so a caller passing a pointer whose heap object a store edge polluted is
/// caught at the `relay` call, the same reject a direct `chan_send` earns. The
/// leaf-site check in `typeck` still catches a send whose value is frame-local in
/// the sending frame itself; the two layers complement, one per hop.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct EscapeSummary {
    pub returns_alias: ParamSet,
    pub flows_into: Vec<(u8, u8)>,
    pub reads_through: ParamSet,
    pub sinks: ParamSet,
    /// The subset of `sinks` whose outliving sink is a `collector<T>` mint rather
    /// than a channel send. Both cross the frame boundary the same way, so the
    /// enforcement rejects a polluted argument in either position; this subset
    /// only selects the diagnostic wording, so a collect sink names the mint and a
    /// channel sink names the send. Kept a subset of `sinks` at every step, so the
    /// pre-existing full-set closure and propagation are unchanged.
    pub collect_sinks: ParamSet,
}

/// Everything the escape pass computes for the enforcement step. `fns` is the
/// per-function summary fixpoint. `lambda_returns` maps each lambda literal's
/// expression span to the set of its own parameter indices whose views may
/// reach its return value, decided by the same abstract walk that summarizes a
/// function, so an element passthrough laundered through a local alias, a call,
/// or a tuple wrap is classified the same as a bare `return x`. `lambda_sinks`
/// maps each lambda literal's expression span to the set of its own parameter
/// indices whose value or pointee is handed to `chan_send`/`chan_try_send`
/// inside its body (directly, through a relaying helper, or through a store edge
/// into a sunk place), decided by the same walk closed over the lambda's own
/// flows. A named module function carries this relation in its summary's `sinks`,
/// but a lambda bound to a local has no summary, so the checker reads this table
/// at a direct call of the bound name to reject a polluted argument in a sink
/// position, the interprocedural send the leaf-site check cannot see through a
/// closure. `frame_stores` lists every direct store of a frame-local view into a
/// place reachable through a parameter of the enclosing function (including a
/// store inside a lambda body whose destination is a captured parameter): the
/// store site's span and the parameter index, which the checker turns into
/// diagnostics, since such a view outlives the frame no matter what the caller
/// does. `lambda_capture_flows` maps each lambda literal's expression span to the
/// edges by which one of its own parameters flows into a place reachable through
/// a *captured* binding: the parameter index and the captured binding's name. A
/// lambda `setter := lambda (s) { (*c).rows = s }` records `(0, "c")`, since
/// parameter 0's view is stored through the captured pointer `c`. The captured
/// place is not one of the lambda's arguments, so the ordinary argument-to-
/// argument store model cannot see it; the checker reads this table at a direct
/// call of the bound name and raises the captured binding's escape flag when the
/// argument in that position is a frame view, so a later return, send, or spawn
/// of the captured binding is caught while a purely local use stays legal.
/// `method_summaries` is the same relation for each impl method, keyed by the
/// receiver type name and the method name, computed with the by-pointer receiver
/// `self` spelled as parameter 0 and the declared parameters shifted up by one. A
/// method is a named function whose hidden first parameter is the receiver, so
/// the checker threads a method call's receiver as argument 0 and reads this
/// table exactly as it reads `fns` for a named call: a method that sinks `self`
/// into a channel, or stores a frame view through `self`, is caught on a polluted
/// receiver the same reject a direct `chan_send` or a `relay(ch, c)` helper earns.
#[derive(Default)]
pub struct EscapeInfo {
    pub fns: HashMap<String, EscapeSummary>,
    pub lambda_returns: HashMap<Span, ParamSet>,
    pub lambda_sinks: HashMap<Span, ParamSet>,
    /// The subset of each lambda literal's `lambda_sinks` whose sink is a
    /// `collector<T>` mint rather than a channel send, keyed by the same span. The
    /// checker reads this beside `lambda_sinks` at a direct call of a lambda-bound
    /// local so a collect-minting closure's reject names the mint, not a channel.
    pub lambda_collect_sinks: HashMap<Span, ParamSet>,
    pub frame_stores: Vec<(Span, u8)>,
    pub lambda_capture_flows: HashMap<Span, Vec<(u8, String)>>,
    pub method_summaries: HashMap<(String, String), EscapeSummary>,
}

/// Each struct's generics and declared fields, keyed by struct name.
type StructTable = HashMap<String, (HashSet<String>, Vec<(String, Type)>)>;
/// Each enum's generics and the union of its variants' payload field types.
type EnumTable = HashMap<String, (HashSet<String>, Vec<Type>)>;

/// The immutable type/name tables and call graph the fixpoint reads.
struct Summarizer {
    ifaces: HashSet<String>,
    /// For view classification and projection typing.
    structs: StructTable,
    /// For view classification of enum-typed places.
    enums: EnumTable,
    /// Each variant's payload field types, keyed by the globally unique variant
    /// name, for gating a constructor's arguments the same way a struct field is.
    variant_payloads: HashMap<String, Vec<Type>>,
    /// Top-level function names, the ones that carry a computed summary.
    module_fns: HashSet<String>,
    /// The declaration order of the module functions, for a deterministic seed.
    order: Vec<String>,
    /// For each summarized callee, the summarized functions that call it, so a
    /// grown summary re-enqueues exactly its callers.
    callers: HashMap<String, HashSet<String>>,
}

/// Computes the escape summary of every top-level function in a merged, desugared
/// surface module, plus the per-lambda alias table and the frame-store sites the
/// checker enforces. The summaries are a fixpoint; the lambda and frame-store
/// tables come from one collection pass over every function and impl method under
/// the settled table. The pass names no diagnostics and mutates nothing. Generics
/// are un-monomorphized here, so a type parameter is treated as a view
/// (conservative).
pub fn compute(module: &Module) -> EscapeInfo {
    let s = Summarizer::new(module);
    let fns = s.solve(module);
    let (lambda_returns, lambda_sinks, lambda_collect_sinks, frame_stores, lambda_capture_flows) =
        s.collect(module, &fns);
    let method_summaries = s.method_summaries(module, &fns);
    EscapeInfo {
        fns,
        lambda_returns,
        lambda_sinks,
        lambda_collect_sinks,
        frame_stores,
        lambda_capture_flows,
        method_summaries,
    }
}

/// A method posed as a plain function: the by-pointer receiver `self` prepended
/// as a leading parameter of the impl's type, so the summary walk indexes it as
/// parameter 0 and the declared parameters follow at 1, 2, .... Codegen lowers a
/// method's receiver by pointer, so `*T` is the receiver's true parameter type;
/// `reaches_managed_ptr` then seeds it into the store-destination and
/// reads-through relations exactly as an explicit `*T` parameter would, and a
/// `chan_send(ch, self)` in the body marks parameter 0 a sink.
fn method_as_fn(ty: &str, m: &Func) -> Func {
    let self_param = Param {
        using: false,
        name: "self".to_string(),
        ty: Type::Ptr(Box::new(Type::Named(ty.to_string(), Vec::new()))),
    };
    let mut params = Vec::with_capacity(m.params.len() + 1);
    params.push(self_param);
    params.extend(m.params.iter().cloned());
    Func { params, ..m.clone() }
}

impl Summarizer {
    fn new(module: &Module) -> Self {
        let mut ifaces = HashSet::new();
        let mut structs = HashMap::new();
        let mut enums = HashMap::new();
        let mut variant_payloads = HashMap::new();
        let mut module_fns = HashSet::new();
        let mut order = Vec::new();
        for item in &module.items {
            match item {
                Item::Interface(i) => {
                    ifaces.insert(i.name.clone());
                }
                Item::Struct(st) => {
                    let gens: HashSet<String> = st.generics.iter().cloned().collect();
                    let fields = st.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
                    structs.insert(st.name.clone(), (gens, fields));
                }
                Item::Enum(en) => {
                    let gens: HashSet<String> = en.generics.iter().cloned().collect();
                    let all: Vec<Type> = en
                        .variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                        .collect();
                    enums.insert(en.name.clone(), (gens, all));
                    for v in &en.variants {
                        if !v.fields.is_empty() {
                            variant_payloads
                                .insert(v.name.clone(), v.fields.iter().map(|f| f.ty.clone()).collect());
                        }
                    }
                }
                Item::Func(f) => {
                    module_fns.insert(f.name.clone());
                    order.push(f.name.clone());
                }
                _ => {}
            }
        }
        // Names are unique in a valid module; drop any stray duplicate so the
        // fixpoint seed stays deterministic and each function is seeded once.
        let mut seen = HashSet::new();
        order.retain(|n| seen.insert(n.clone()));
        let callers = build_callers(module, &module_fns);
        Summarizer {
            ifaces,
            structs,
            enums,
            variant_payloads,
            module_fns,
            order,
            callers,
        }
    }

    /// The worklist fixpoint. Every function seeds the queue with a bottom
    /// summary; re-analyzing a function that reads a callee's summary can only
    /// grow, so a growth re-enqueues the function's callers and the monotone,
    /// finite lattice converges. Recursion and mutual recursion are ordinary
    /// cycles that climb from bottom.
    fn solve(&self, module: &Module) -> HashMap<String, EscapeSummary> {
        let funcs: HashMap<&str, &Func> = module
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Func(f) => Some((f.name.as_str(), f)),
                _ => None,
            })
            .collect();
        let mut summaries: HashMap<String, EscapeSummary> =
            self.order.iter().map(|n| (n.clone(), EscapeSummary::default())).collect();
        let mut queue: VecDeque<String> = self.order.iter().cloned().collect();
        let mut queued: HashSet<String> = self.order.iter().cloned().collect();
        // Termination is guaranteed by monotonicity; the cap only guards against a
        // future bug turning the walk non-monotone, and is far above the true
        // bound of |funcs| * (64 + 64*64) re-enqueues.
        let cap = self.order.len().saturating_mul(4200).saturating_add(10_000);
        let mut steps = 0usize;
        while let Some(name) = queue.pop_front() {
            queued.remove(&name);
            steps += 1;
            if steps > cap {
                break;
            }
            let Some(f) = funcs.get(name.as_str()) else {
                continue;
            };
            let next = self.transfer(f, &summaries);
            if summaries.get(&name) != Some(&next) {
                summaries.insert(name.clone(), next);
                if let Some(cs) = self.callers.get(&name) {
                    for c in cs {
                        if queued.insert(c.clone()) {
                            queue.push_back(c.clone());
                        }
                    }
                }
            }
        }
        summaries
    }

    /// The lambda and frame-store collection pass: one walk of every function and
    /// impl method under the settled summary table. It repeats the transfer walk,
    /// which records each lambda literal's self-alias set and each direct store of
    /// a frame view into a parameter-reachable place; only this final pass keeps
    /// them, so the tables reflect the fixpoint.
    #[allow(clippy::type_complexity)]
    fn collect(
        &self,
        module: &Module,
        summaries: &HashMap<String, EscapeSummary>,
    ) -> (
        HashMap<Span, ParamSet>,
        HashMap<Span, ParamSet>,
        HashMap<Span, ParamSet>,
        Vec<(Span, u8)>,
        HashMap<Span, Vec<(u8, String)>>,
    ) {
        let mut lambda_returns = HashMap::new();
        let mut lambda_sinks = HashMap::new();
        let mut lambda_collect_sinks = HashMap::new();
        let mut frame_stores: Vec<(Span, u8)> = Vec::new();
        let mut lambda_capture_flows = HashMap::new();
        for item in &module.items {
            match item {
                Item::Func(f) => self.collect_func(
                    f,
                    summaries,
                    &mut lambda_returns,
                    &mut lambda_sinks,
                    &mut lambda_collect_sinks,
                    &mut frame_stores,
                    &mut lambda_capture_flows,
                ),
                Item::Impl(im) => {
                    for m in &im.methods {
                        self.collect_func(
                            m,
                            summaries,
                            &mut lambda_returns,
                            &mut lambda_sinks,
                            &mut lambda_collect_sinks,
                            &mut frame_stores,
                            &mut lambda_capture_flows,
                        );
                    }
                }
                _ => {}
            }
        }
        frame_stores.sort_unstable_by_key(|&(s, j)| (s.lo, s.hi, j));
        frame_stores.dedup();
        (lambda_returns, lambda_sinks, lambda_collect_sinks, frame_stores, lambda_capture_flows)
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_func(
        &self,
        f: &Func,
        summaries: &HashMap<String, EscapeSummary>,
        lambda_returns: &mut HashMap<Span, ParamSet>,
        lambda_sinks: &mut HashMap<Span, ParamSet>,
        lambda_collect_sinks: &mut HashMap<Span, ParamSet>,
        frame_stores: &mut Vec<(Span, u8)>,
        lambda_capture_flows: &mut HashMap<Span, Vec<(u8, String)>>,
    ) {
        let (lr, ls, lcs, fs, cf) = self.transfer_collect(f, summaries);
        lambda_returns.extend(lr);
        lambda_sinks.extend(ls);
        lambda_collect_sinks.extend(lcs);
        frame_stores.extend(fs);
        lambda_capture_flows.extend(cf);
    }

    /// The escape summary of every impl method, keyed by the receiver type name
    /// and the method name, computed once under the settled function table with
    /// the receiver `self` spelled as parameter 0 (see `method_as_fn`). A method
    /// body that calls a top-level function reads that function's converged
    /// summary from `fns`; a method-to-method call stays opaque (TOP) inside the
    /// walk, the same conservative reading a method dispatch already gets, so no
    /// fixpoint over methods is required and this one-shot pass is sound.
    fn method_summaries(
        &self,
        module: &Module,
        fns: &HashMap<String, EscapeSummary>,
    ) -> HashMap<(String, String), EscapeSummary> {
        let mut out = HashMap::new();
        for item in &module.items {
            if let Item::Impl(im) = item {
                for m in &im.methods {
                    let synth = method_as_fn(&im.ty, m);
                    out.insert((im.ty.clone(), m.name.clone()), self.transfer(&synth, fns));
                }
            }
        }
        out
    }

    /// Whether a type may carry a frame pointer when held or projected by value:
    /// a slice, closure, interface, or an aggregate containing one, and a generic
    /// or an inference hole (conservative). A pointer, a future, and every scalar
    /// or heap-backed primitive do not.
    fn is_view(&self, t: &Type, generics: &HashSet<String>) -> bool {
        let mut seen = HashSet::new();
        self.is_view_rec(t, generics, &mut seen)
    }

    fn is_view_rec(&self, t: &Type, generics: &HashSet<String>, seen: &mut HashSet<String>) -> bool {
        match t {
            Type::Slice(_) | Type::Func(..) => true,
            Type::Array(e, _) => self.is_view_rec(e, generics, seen),
            Type::Tuple(ts) => ts.iter().any(|x| self.is_view_rec(x, generics, seen)),
            // A pointer is never a frame view: dusk has no address-of, so a
            // managed `*T` or a `*raw T` points at the heap or the FFI layer, and
            // the managed-pointer escape is covered by the generation backstop.
            Type::Ptr(_) | Type::RawPtr(_) => false,
            // A collector's block lives on the collected heap, which outlives the
            // frame, so a collector value never views it: escape neutral.
            Type::Collector(_) => false,
            Type::Named(n, _) => {
                if n == "Future" {
                    return false;
                }
                if generics.contains(n) {
                    return true;
                }
                if self.ifaces.contains(n) {
                    return true;
                }
                if let Some((sgen, fields)) = self.structs.get(n) {
                    if !seen.insert(n.clone()) {
                        return false;
                    }
                    return fields.iter().any(|(_, ft)| self.is_view_rec(ft, sgen, seen));
                }
                if let Some((egen, fields)) = self.enums.get(n) {
                    if !seen.insert(n.clone()) {
                        return false;
                    }
                    return fields.iter().any(|ft| self.is_view_rec(ft, egen, seen));
                }
                // A primitive (int, float, bool, char, string, error, ...) or an
                // otherwise-unknown name is not a frame view.
                false
            }
            Type::Unit => false,
            // A do-continuation hole erases to Unknown; treat it as a view.
            Type::Infer => true,
        }
    }

    /// Whether a managed pointer is reachable through a value of this type,
    /// however deeply a struct, enum, tuple, or array buries it. A parameter
    /// whose type reaches one exposes whatever the caller stored behind the
    /// pointer, so it seeds the reads-through relation the same way a bare `*T`
    /// parameter does; a bare-pointer-only rule would let a `Holder { c: *Cell }`
    /// wrapper wash the relation out. `*void` is the raw allocator currency and
    /// never a view carrier, matching `typeck`. A generic type parameter
    /// does not count: it already seeds `origins` as a view, which covers the
    /// value flow conservatively.
    fn reaches_managed_ptr(&self, t: &Type) -> bool {
        let mut seen = HashSet::new();
        self.reaches_managed_ptr_rec(t, &mut seen)
    }

    fn reaches_managed_ptr_rec(&self, t: &Type, seen: &mut HashSet<String>) -> bool {
        match t {
            Type::Ptr(inner) => !matches!(**inner, Type::Unit),
            // A `*raw T` is deliberately excluded: the raw pointer layer is the
            // FFI boundary, honor-system by design, so a caller object reached
            // through it is not tracked and a view stored through it is the
            // caller's responsibility, matching `typeck`'s `member_carries_view`.
            Type::RawPtr(_) => false,
            Type::Slice(e) | Type::Array(e, _) => self.reaches_managed_ptr_rec(e, seen),
            Type::Tuple(ts) => ts.iter().any(|x| self.reaches_managed_ptr_rec(x, seen)),
            Type::Named(n, args) => {
                if args.iter().any(|a| self.reaches_managed_ptr_rec(a, seen)) {
                    return true;
                }
                if !seen.insert(n.clone()) {
                    return false;
                }
                if let Some((_, fields)) = self.structs.get(n) {
                    return fields.iter().any(|(_, ft)| self.reaches_managed_ptr_rec(ft, seen));
                }
                if let Some((_, fields)) = self.enums.get(n) {
                    return fields.iter().any(|ft| self.reaches_managed_ptr_rec(ft, seen));
                }
                false
            }
            // A collector aliases nothing: its backing is a fresh collected block,
            // not a managed pointer the alias-group rules track.
            Type::Collector(_) => false,
            Type::Func(..) | Type::Unit | Type::Infer => false,
        }
    }
}

/// The hand-audited escape behavior of each builtin, so a call to one does not
/// fall through to the conservative TOP an unknown callee gets. Return value:
/// `Some(summary)` for a recognized builtin, `None` for a name that is not a
/// builtin (and is therefore an unknown callee).
///
/// Audit of the whole builtin surface (the resolver's `BUILTINS` list plus the
/// forms `typeck` special-cases). Every one either returns a fresh heap value, a
/// scalar, a managed pointer, unit, or an error, and stores nothing from one
/// argument into another. The sole exception is `move`, which hands its argument
/// straight back, so a view argument is aliased by the result. `cstr` returns a
/// freshly heap-allocated C string, not a view of its argument, so it is clean;
/// `map`/`filter`/`reduce`/`fold` return fresh heap results; `read_*`/`parse_*`
/// return `(string|float, error)` with a heap string; `alloc`/`ptr_add` return a
/// pointer (never a frame view); the `debug_*`, `print*`, `spawn`, `join`,
/// `submit`, and `async_run` forms return scalars, handles, unit, or errors.
///
/// Shared with `typeck`, which resolves the same summary at a call site so a
/// builtin does not fall through to the conservative TOP an unknown callee gets.
pub(crate) fn builtin_summary(name: &str) -> Option<EscapeSummary> {
    match name {
        // move(x) is identity: a view argument leaves through the result.
        "move" => Some(EscapeSummary {
            returns_alias: ParamSet::single(0),
            flows_into: Vec::new(),
            reads_through: ParamSet::default(),
            sinks: ParamSet::default(),
            collect_sinks: ParamSet::default(),
        }),
        // alloc(v) heap-copies its initializer and hands back a pointer to it. The
        // pointer aliases the initializer, so a struct literal whose fat field
        // views the current frame walks out through the returned pointer; the
        // enforcement checks `value_escape` of the argument, so `alloc(Cell {
        // rows: heap_slice })` stays clean while `alloc(Cell { rows: local[0..n]
        // })` is caught. A scalar or heap initializer contributes nothing.
        "alloc" => Some(EscapeSummary {
            returns_alias: ParamSet::single(0),
            flows_into: Vec::new(),
            reads_through: ParamSet::default(),
            sinks: ParamSet::default(),
            collect_sinks: ParamSet::default(),
        }),
        "free" | "print" | "println" | "printerr" | "sizeof" | "alloc_bytes"
        | "ptr_add" | "map" | "filter" | "reduce" | "fold" | "foreach" | "debug_alloc"
        | "debug_free" | "debug_leaks" | "debug_double_frees" | "read_file" | "write_file"
        | "read_line" | "read_all" | "parse_float" | "cstr" | "spawn" | "join" | "submit"
        | "async_run" => Some(EscapeSummary::default()),
        _ => None,
    }
}

/// The direct callees of each summarized function, inverted so a grown callee
/// summary re-enqueues its callers.
fn build_callers(module: &Module, module_fns: &HashSet<String>) -> HashMap<String, HashSet<String>> {
    let mut callers: HashMap<String, HashSet<String>> = HashMap::new();
    for item in &module.items {
        if let Item::Func(f) = item {
            if !module_fns.contains(&f.name) {
                continue;
            }
            let mut callees = HashSet::new();
            collect_callees_block(&f.body, module_fns, &mut callees);
            for c in callees {
                callers.entry(c).or_default().insert(f.name.clone());
            }
        }
    }
    callers
}

fn collect_callees_block(b: &Block, fns: &HashSet<String>, out: &mut HashSet<String>) {
    for s in &b.stmts {
        collect_callees_stmt(s, fns, out);
    }
}

fn collect_callees_stmt(s: &Stmt, fns: &HashSet<String>, out: &mut HashSet<String>) {
    match s {
        Stmt::Let(l) => collect_callees_expr(&l.value, fns, out),
        Stmt::Assign(a, b) => {
            collect_callees_expr(a, fns, out);
            collect_callees_expr(b, fns, out);
        }
        Stmt::AssignOp(_, a, b) => {
            collect_callees_expr(a, fns, out);
            collect_callees_expr(b, fns, out);
        }
        Stmt::Return(Some(e)) | Stmt::Defer(e) | Stmt::Expr(e) => collect_callees_expr(e, fns, out),
        Stmt::Return(None) => {}
        Stmt::If(i) => {
            collect_callees_expr(&i.cond, fns, out);
            collect_callees_block(&i.then, fns, out);
            if let Some(e) = &i.els {
                collect_callees_block(e, fns, out);
            }
        }
        Stmt::While(w) => {
            collect_callees_expr(&w.cond, fns, out);
            collect_callees_block(&w.body, fns, out);
        }
        Stmt::For(f) => {
            collect_callees_expr(&f.iter, fns, out);
            collect_callees_block(&f.body, fns, out);
        }
        Stmt::Match(m) => collect_callees_match(m, fns, out),
    }
}

fn collect_callees_match(m: &Match, fns: &HashSet<String>, out: &mut HashSet<String>) {
    collect_callees_expr(&m.scrut, fns, out);
    for arm in &m.arms {
        collect_callees_block(&arm.body, fns, out);
    }
}

fn collect_callees_expr(e: &Expr, fns: &HashSet<String>, out: &mut HashSet<String>) {
    match &e.kind {
        ExprKind::Call(callee, args) => {
            if let ExprKind::Ident(n) = &callee.kind {
                if fns.contains(n) {
                    out.insert(n.clone());
                }
            }
            collect_callees_expr(callee, fns, out);
            for a in args {
                collect_callees_expr(a, fns, out);
            }
        }
        ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Await(x, _) => {
            collect_callees_expr(x, fns, out)
        }
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b, _) => {
            collect_callees_expr(a, fns, out);
            collect_callees_expr(b, fns, out);
        }
        ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
            for x in xs {
                collect_callees_expr(x, fns, out);
            }
        }
        ExprKind::StructLit(_, fields) => {
            for (_, v) in fields {
                collect_callees_expr(v, fns, out);
            }
        }
        ExprKind::Lambda(l) => collect_callees_block(&l.body, fns, out),
        ExprKind::Match(m) => collect_callees_match(m, fns, out),
        ExprKind::Do(_, binds) => {
            for b in binds {
                collect_callees_expr(&b.expr, fns, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
