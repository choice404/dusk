//! Type checking. M4.
//!
//! Integers and floats carry their bit width, with width 0 standing for a bare
//! literal that adapts to the width around it, so `int32 + int64` is rejected
//! while `x + 1` still works at any width. Generics, methods, and imported types
//! become `Unknown`, which is compatible with everything, so advanced code is
//! accepted without false errors while real core type errors are still caught.
//! The unused variable rule and the bare identifier immutability rule live in the
//! resolver pass (M3); the projection form of immutability, the must handle rule,
//! and the ownership rules live here, where types are known.

use std::collections::{HashMap, HashSet};

use crate::diag::{Diagnostic, Span};
use crate::parser::ast::*;
use crate::sema::summary::{builtin_summary, EscapeInfo, EscapeSummary, ParamSet};

/// Type checks a module, returning diagnostics and the storage types of the
/// narrow mutable-tuple class the surface pass reconciled, which mono stamps onto
/// the affected bindings so codegen sizes their slots as slices.
pub fn check(module: &Module, escape: &EscapeInfo) -> (Vec<Diagnostic>, HashMap<Span, Type>) {
    let (diags, muts) = run_pass(module, false, &HashMap::new(), escape);
    (diags, muts)
}

/// Re-runs only the type/width/argument/exhaustiveness class over a
/// mono-expanded (ground) module. A `do` over a generic monad desugars its
/// continuations with `Type::Infer` holes that lower to `Unknown`; the surface
/// pass wildcards them, so a continuation body's widths go unchecked there. Once
/// mono makes those types ground, this pass width-checks them with the real
/// walk. The ownership, escape, and must-handle classes stay suppressed here
/// (`types_only`), since the surface pass already ran them at full fidelity on
/// the un-erased AST and `Unknown` erasure never suppressed them.
///
/// `future_table` maps each mangled `Future$T` name mono minted back to its
/// element type. Mono lowers `Future<T>` to the named struct `Future$T`, but an
/// async call still types as the surface `Ty::Future`, so an annotation, a
/// parameter, or a container element spelled `Future<T>` would look like an
/// unrelated named type here and clash with the call it came from. Restoring the
/// `Ty::Future` shape from the table before comparison recovers exactly the
/// forms the surface pass accepted, so a future passed, annotated, or stored in
/// a container ground-checks the same as one that is named and awaited.
pub fn check_ground(module: &Module, future_table: &HashMap<String, Type>) -> Vec<Diagnostic> {
    run_pass(module, true, future_table, &EscapeInfo::default()).0
}

fn run_pass(
    module: &Module,
    types_only: bool,
    future_table: &HashMap<String, Type>,
    escape: &EscapeInfo,
) -> (Vec<Diagnostic>, HashMap<Span, Type>) {
    let mut tc = TypeChecker::new();
    tc.types_only = types_only;
    tc.summaries = escape.fns.clone();
    tc.method_summaries = escape.method_summaries.clone();
    tc.lambda_returns = escape.lambda_returns.clone();
    tc.lambda_sinks = escape.lambda_sinks.clone();
    tc.lambda_collect_sinks = escape.lambda_collect_sinks.clone();
    tc.lambda_capture_flows = escape.lambda_capture_flows.clone();
    // Every direct store of a frame view into a place reachable through a
    // parameter, found by the summary walk: the view dies with the frame no
    // matter what the caller does, so each site is a diagnostic outright. The
    // walk sees the store through a borrowed pointer, a destructured borrow, or
    // a lambda capture identically, so no per-shape arm exists here to fall out
    // of date. Surface pass only; the ground pass carries an empty table.
    if !types_only {
        for &(span, j) in &escape.frame_stores {
            tc.errors.push(Diagnostic::new(
                format!(
                    "a view of the current frame is stored into a place reachable through parameter {}, which outlives it; put the backing on the heap",
                    j + 1
                ),
                span,
            ));
        }
    }
    // The element types arrive as ground AST types; lower each once so the
    // unmangle walk can splice a ready `Ty` in place of the mangled named type.
    let no_generics = HashSet::new();
    tc.future_elems = future_table
        .iter()
        .map(|(name, elem)| (name.clone(), lower(elem, &no_generics)))
        .collect();
    tc.collect_sigs(module);
    tc.run(module);
    (tc.errors, tc.mut_tuple_types)
}

/// A checked type. `Int(w)` and `Float(w)` carry the bit width; width 0 is a
/// bare numeric literal, compatible with any width of its kind, so a literal
/// adapts to its context and only two differently sized variables clash.
#[derive(Clone, Debug, PartialEq)]
enum Ty {
    Int(u32),
    Float(u32),
    Bool,
    Char,
    Rune,
    Str,
    Unit,
    Error,
    Thread,
    Ptr(Box<Ty>),
    RawPtr(Box<Ty>),
    Slice(Box<Ty>),
    Array(Box<Ty>, u64),
    Tuple(Vec<Ty>),
    Named(String),
    Func(Vec<Ty>, Box<Ty>),
    /// A one-shot completion future carrying an element of the boxed type. An
    /// async call mints one; awaiting it yields the element. Its element type is
    /// tracked here even though the `Future<T>` struct erases it at runtime, so
    /// awaits and the event-loop capture rules can reason about the element.
    Future(Box<Ty>),
    /// A collected wrapper `collector<T>`. Its runtime rep is a managed `*T`, but
    /// it is not owned and not a frame view: freeing, moving, or `ref`-aliasing it
    /// is rejected, and returning it never escapes. Deref and field projection
    /// mirror `*T`, with the same generation check.
    Collector(Box<Ty>),
    Unknown,
}

/// Ownership state of a managed pointer binding, for the single owner rules.
/// A non managed binding has no entry at all.
#[derive(Clone, Copy, PartialEq)]
enum Own {
    Owner,
    Borrow,
    Moved,
}

/// Why a value may not cross an async boundary, so the diagnostic names the
/// reason precisely: a future belongs to the loop thread, while a slice,
/// closure, or interface value views the frame it was built in.
#[derive(Clone, Copy, PartialEq)]
enum CrossFail {
    Ok,
    Future,
    View,
}

/// The escape relation of a call's target at a call site (M5). A known callee
/// carries its computed summary; an opaque one (a closure value, a
/// function-typed parameter, a method, or a foreign symbol) gets the
/// conservative TOP, treated as returning and cross-storing every argument.
enum CalleeEsc {
    Known(EscapeSummary),
    Top,
}

/// A lambda-bound local's recorded sink sets. `sinks` is every parameter the
/// bound lambda hands to an outliving sink, `collect` the subset handed to a
/// `collector<T>` mint rather than a channel. A direct call of the name rejects a
/// polluted argument in any sink position; the subset only selects the wording,
/// so a collect sink names the mint and a channel sink names the send.
#[derive(Clone, Copy, Default)]
struct LambdaSinkBind {
    sinks: ParamSet,
    collect: ParamSet,
}

struct TypeChecker {
    sigs: HashMap<String, (Vec<Ty>, Ty)>,
    // Async function names mapped to their declared return type, the element of
    // the future a call mints. Used to type an await, to check async_run, and to
    // reject an async name used as a value.
    async_fns: HashMap<String, Ty>,
    ifaces: HashSet<String>,
    enums: HashMap<String, Vec<String>>,
    // Each enum variant's declared payload field types, keyed by variant name,
    // which is globally unique. Used by the escape walk to gate a fat payload
    // returned by value the same way a struct field is gated.
    variant_payloads: HashMap<String, Vec<Ty>>,
    // The same payloads keyed by the owning enum and the variant, so a qualified
    // constructor's arity and payload types check against the enum it names even
    // when two enums happen to share a variant name and the by-name map holds only
    // the last. Absent for a nullary variant.
    variant_payloads_by_enum: HashMap<(String, String), Vec<Ty>>,
    // Each struct's generic flag and its declared fields, for struct literal
    // validation. A generic struct checks field names only, since matching a
    // field's type parameter against a concrete value needs inference.
    structs: HashMap<String, (bool, Vec<(String, Ty)>)>,
    // Interface conformance state. `iface_methods` holds each interface's
    // required method names and arities, `impls` the `(interface, type)` pairs an
    // `impl` block declares, and `raw_sigs` the unfixed parameter types, so a call
    // can tell a concrete struct from the interface a parameter expects.
    iface_methods: HashMap<String, Vec<(String, usize)>>,
    impls: HashSet<(String, String)>,
    raw_sigs: HashMap<String, Vec<Ty>>,
    // Unfixed field types of every struct and enum, for the spawn capture walk,
    // which must see a slice or an interface buried in a field where the fixed
    // tables erase them.
    embed_fields: HashMap<String, Vec<Ty>>,
    // Bindings annotated with an interface type, per scope. Their checked type
    // is Unknown after fix, so the spawn capture check reads this record to
    // reject capturing an interface value, whose data pointer may sit in the
    // spawning frame.
    iface_binds: Vec<HashMap<String, String>>,
    // Bindings initialized from an enum constructor, per scope, mapped to the enum
    // they name. A `m := Opt.Some(1)` local types as Unknown (enum locals are not
    // yet ground-typed), so a stray method call `m.foo()` would slip past the
    // enum-receiver reject that keys on a `Ty::Named` enum type. This side table
    // recovers the enum for exactly that reject.
    enum_binds: Vec<HashMap<String, String>>,
    // Bindings annotated with a slice-of-interface type, per scope, mapped to
    // their unfixed slice type. A later assignment of a slice of concrete structs
    // is the covariance error the fixed table cannot see, so the raw type is kept.
    slice_iface_elem: Vec<HashMap<String, Ty>>,
    // Bindings that hold a closure capturing a frame local, per scope. Its
    // environment sits on this frame, so returning such a binding, directly or
    // as a tuple member, escapes and is rejected the same as a returned lambda
    // literal that captures a local.
    esc_closures: Vec<HashMap<String, bool>>,
    // Bindings that hold a slice viewing a frame local array, per scope. The
    // array materializes on this frame, so returning the binding, directly or as
    // a tuple member, escapes. Tracked here because a slice from an array literal
    // loses its array type at the binding, so a return of the bare name cannot be
    // caught by type alone. A scope maps a name to its escape flag, so a shadowing
    // or a reassignment writes a fresh flag that masks any outer or stale one.
    esc_slices: Vec<HashMap<String, bool>>,
    // Symmetric alias adjacency over managed-pointer binding names, per scope. Two
    // bindings share an edge when one is a `ref` or borrow-copy of the other, when
    // a non-pointer aggregate binding embeds a bare pointer member, when a
    // destructure binder takes a pointer member, or when a call hands one argument's
    // pointer back through its result. The escape flag raised on any member of an
    // alias group is propagated to the whole group at `raise_esc`, so a frame view
    // stored through one name is caught when a different name in the group is
    // returned. Scope-stacked so a shadow does not carry a stale edge out; the group
    // closure unions every stacked scope so a chain built across scopes still
    // resolves. Managed bindings only; empty on the ground pass.
    alias_edges: Vec<HashMap<String, HashSet<String>>>,
    scopes: Vec<HashMap<String, Ty>>,
    owns: Vec<HashMap<String, Own>>,
    // Mutable binding names, tracked here so an element or field store can walk
    // to its root binding and reject writing through an immutable one. The bare
    // `x = v` form is the resolver's; this pass owns the projection forms.
    muts: Vec<HashSet<String>>,
    // Error typed bindings that have not been handled yet, reported when their
    // scope pops. Handling is one of exists, check, or ignore on the binding, or
    // returning it to the caller.
    err_binds: Vec<HashMap<String, Span>>,
    // How many conditional or loop bodies enclose the current statement. A defer
    // inside one is rejected, since defers replay lexically at every return and
    // a conditional registration cannot be honored.
    branch_depth: u32,
    // Nonzero while replaying a loop body to raise loop-carried escape flags to a
    // fixpoint. Diagnostics are suppressed on the replay so the final real pass
    // emits each once, with the escape state the fixpoint settled on.
    suppress: u32,
    // True while checking an async func body, so async_run and the awaited
    // control forms can reject what is illegal inside a task.
    in_async: bool,
    // Whether the function being checked is an impl method, so `self` names the
    // receiver value. The self-value-in-pointer check keys on this context, not on
    // the spelling of the name, so a function-local binding a user happens to call
    // `self` gets the ordinary mismatch message, not the receiver-value one.
    in_method: bool,
    cur_generics: HashSet<String>,
    cur_ret: Ty,
    // Each mangled `Future$T` name mono minted, mapped to its element type. Mono
    // lowers the `Future<T>` struct to a named type, so a `Future<T>` written in
    // an annotation, a parameter, or a container element reaches the ground pass
    // as `Ty::Named("Future$T")`, while an async call still types as the surface
    // `Ty::Future`. Restoring the future shape from this table before a
    // comparison lets a future cross the same forms a plain value does. Empty on
    // the surface pass, where nothing is mangled, so the unmangle walk no-ops.
    future_elems: HashMap<String, Ty>,
    // True for the ground re-check over the mono-expanded module. It suppresses
    // the ownership, escape, and must-handle diagnostics, which the surface pass
    // already reported at full fidelity on the un-erased AST, and keeps only the
    // type/width/argument/exhaustiveness class, the one `Unknown` erasure hid.
    types_only: bool,
    // Interprocedural escape summaries (M5), keyed by top-level function name.
    // Read at a call site to decide whether the call returns a view of a frame
    // argument or stores one into another argument. Empty on the ground pass, so
    // the escape class stays surface-only.
    summaries: HashMap<String, EscapeSummary>,
    // Interprocedural escape summaries of impl methods (M5), keyed by the receiver
    // type name and the method name. The receiver is the method's implicit
    // parameter 0, so a method call threads its receiver as argument 0 and reads
    // this table exactly as a named call reads `summaries`: a method that sinks or
    // stores through `self` is caught on a polluted receiver. Empty on the ground
    // pass, so the escape class stays surface-only.
    method_summaries: HashMap<(String, String), EscapeSummary>,
    // The lowered parameter types of each impl method, keyed by the receiver type
    // name and the method name. A method call resolves its callee's parameters
    // here to catch a value `self` handed into a `*T` parameter position, the same
    // precise self-value message a direct call earns; without it the call stays
    // opaque (infer returns Unknown) and the backend faults on the struct where a
    // fat pointer belongs. Populated on both passes from the impl declarations.
    method_params: HashMap<(String, String), Vec<Ty>>,
    // The lowered parameter types of each interface method, keyed by the interface
    // name and the method name. An interface-typed receiver names the interface,
    // not a concrete impl, so its method call misses `method_params`; this table
    // lets the same slice-covariance guard fire on a dynamic-dispatch argument.
    // Stored unfixed, so a slice-of-interface parameter keeps its interface name.
    iface_method_params: HashMap<(String, String), Vec<Ty>>,
    // Each lambda literal's self-alias set, keyed by its expression span: the
    // lambda's own parameter indices whose views or pointees may reach its
    // return value, decided by the summary module's abstract walk. The
    // higher-order gates read it, so an element passthrough laundered through a
    // local alias, a call, or a tuple wrap is classified the same as a bare
    // `return x`. Empty on the ground pass.
    lambda_returns: HashMap<Span, ParamSet>,
    // Each lambda literal's self-sink set, keyed by its expression span: the
    // lambda's own parameter indices whose value or pointee reaches a channel
    // send in its body, decided by the summary module's abstract walk. A lambda
    // bound to a local has no computed summary, so a direct call of the bound
    // name reads this to reject a polluted argument in a sink position, the
    // interprocedural send the leaf-site check cannot see through a closure.
    // Empty on the ground pass.
    lambda_sinks: HashMap<Span, ParamSet>,
    // The subset of each lambda literal's `lambda_sinks` whose sink is a collector
    // mint rather than a channel send, keyed by the same span. Read beside
    // `lambda_sinks` so a minting closure's reject names the mint. Empty on the
    // ground pass.
    lambda_collect_sinks: HashMap<Span, ParamSet>,
    // Local bindings that hold a lambda literal, mapped to that lambda's sink
    // set (empty for a clean lambda), scope-stacked so a shadow masks an outer
    // binding. A direct call of such a name is checked against the recorded set,
    // the closure counterpart of the named-helper sink check; the entry also
    // marks the binding as a known lambda, not an opaque callee the conservative
    // send-reject would refuse. Populated on the surface pass only.
    lambda_sink_binds: Vec<HashMap<String, LambdaSinkBind>>,
    // Each lambda literal's capture-flow edge set, keyed by its expression span:
    // the lambda's own parameter indices paired with the captured binding name
    // each flows into, from the summary module's synthetic lambda walk. A lambda
    // bound to a local has no computed summary, so a direct call of the bound
    // name reads this to raise the captured binding's escape flag when the
    // argument in that position is a frame view, the capture store the argument-
    // to-argument flow model cannot see. Empty on the ground pass.
    lambda_capture_flows: HashMap<Span, Vec<(u8, String)>>,
    // Local bindings that hold a lambda literal, mapped to that lambda's capture-
    // flow edges, scope-stacked so a shadow masks an outer binding. A direct call
    // of such a name raises each captured binding's escape flag when the matching
    // argument is a frame view, so a later return, send, or spawn of the captured
    // binding is caught while a purely local use stays legal. Surface pass only.
    lambda_capture_binds: Vec<HashMap<String, Vec<(u8, String)>>>,
    // The locals of the function being checked that resolve to a module function:
    // a name bound once by a straight `f := g` to a top-level function and never
    // reassigned. A call of such a name is that function's known relation, not an
    // opaque callee, so the leaf-frame send check reads its summary rather than
    // refusing a polluted pointer argument. Recomputed per function on the surface
    // pass; empty on the ground pass, where the send class is off.
    resolvable_fn_binds: HashMap<String, String>,
    // The parameter names of the function currently being checked, so a view
    // laundered into a parameter through a call is known to outlive the frame,
    // while a view laundered into a local waits for the local's own return check.
    cur_params: HashSet<String>,
    // The parameter names of the function currently being checked, mapped to their
    // 0-based index, so a store through a pointer local that aliases a parameter
    // can name the parameter it reaches. Reset per function.
    cur_param_index: HashMap<String, usize>,
    // Pointer locals that alias a parameter pointer (`d := c`, or a `ref` of one),
    // mapped to the aliased parameter's index. Scope-stacked so a shadow does not
    // carry a stale alias out. A frame view stored through such a local reaches the
    // caller's object at once, exactly as a store through the parameter itself.
    ptr_param_borrows: Vec<HashMap<String, usize>>,
    // For a local raised to hold a frame view through a call's store edge, the
    // originating call span and the source/destination argument indices, so a
    // later return of that local names the store precisely. Reset per function.
    flow_prov: HashMap<String, (Span, u8, u8)>,
    // The reconciled storage type of each narrow mutable-tuple binding, keyed by
    // its value span. A binding `mut t := ([1, 2, 3], 5)` infers its array-literal
    // member as a slice, since a later `t = (xs, 9)` may store one, but the
    // initializer alone shapes that member as a fixed array. Codegen sizes an
    // unannotated slot from the initializer, so it would build a fixed-array slot
    // the fat slice cannot store into. Recording the reconciled tuple type here
    // lets mono stamp it onto `Bind.ty` and drive codegen's annotated-let path,
    // which sizes the slot as a slice. Filled only on the surface pass.
    mut_tuple_types: HashMap<Span, Type>,
    errors: Vec<Diagnostic>,
}

impl TypeChecker {
    fn new() -> Self {
        TypeChecker {
            sigs: HashMap::new(),
            async_fns: HashMap::new(),
            ifaces: HashSet::new(),
            enums: HashMap::new(),
            variant_payloads: HashMap::new(),
            variant_payloads_by_enum: HashMap::new(),
            structs: HashMap::new(),
            iface_methods: HashMap::new(),
            impls: HashSet::new(),
            raw_sigs: HashMap::new(),
            embed_fields: HashMap::new(),
            iface_binds: Vec::new(),
            enum_binds: Vec::new(),
            slice_iface_elem: Vec::new(),
            esc_closures: Vec::new(),
            esc_slices: Vec::new(),
            alias_edges: Vec::new(),
            scopes: Vec::new(),
            owns: Vec::new(),
            muts: Vec::new(),
            err_binds: Vec::new(),
            branch_depth: 0,
            suppress: 0,
            in_async: false,
            in_method: false,
            cur_generics: HashSet::new(),
            cur_ret: Ty::Unit,
            future_elems: HashMap::new(),
            types_only: false,
            summaries: HashMap::new(),
            method_summaries: HashMap::new(),
            method_params: HashMap::new(),
            iface_method_params: HashMap::new(),
            lambda_returns: HashMap::new(),
            lambda_sinks: HashMap::new(),
            lambda_collect_sinks: HashMap::new(),
            lambda_sink_binds: Vec::new(),
            lambda_capture_flows: HashMap::new(),
            lambda_capture_binds: Vec::new(),
            resolvable_fn_binds: HashMap::new(),
            cur_params: HashSet::new(),
            cur_param_index: HashMap::new(),
            ptr_param_borrows: Vec::new(),
            flow_prov: HashMap::new(),
            mut_tuple_types: HashMap::new(),
            errors: Vec::new(),
        }
    }

    fn collect_sigs(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Interface(i) => {
                    if i.name == "rune" {
                        self.err("'rune' is a builtin type name", Span::new(0, 0));
                        continue;
                    }
                    self.ifaces.insert(i.name.clone());
                    let methods = i
                        .methods
                        .iter()
                        .map(|m| (m.name.clone(), m.params.len()))
                        .collect();
                    self.iface_methods.insert(i.name.clone(), methods);
                    // Record each interface method's unfixed parameter types so a
                    // dynamic-dispatch call site can run the slice-covariance guard,
                    // the same as a concrete impl call. An interface is non-generic
                    // over its own type parameters here, so lower against the
                    // interface's generic set.
                    let gens: HashSet<String> = i.generics.iter().cloned().collect();
                    for m in &i.methods {
                        let params = m.params.iter().map(|p| lower(&p.ty, &gens)).collect();
                        self.iface_method_params
                            .insert((i.name.clone(), m.name.clone()), params);
                    }
                }
                Item::Enum(e) => {
                    if e.name == "rune" {
                        self.err("'rune' is a builtin type name", Span::new(0, 0));
                        continue;
                    }
                    let variants = e.variants.iter().map(|v| v.name.clone()).collect();
                    self.enums.insert(e.name.clone(), variants);
                    let gens: HashSet<String> = e.generics.iter().cloned().collect();
                    let fields = e
                        .variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(|f| lower(&f.ty, &gens)))
                        .collect();
                    self.embed_fields.insert(e.name.clone(), fields);
                    for v in &e.variants {
                        if !v.fields.is_empty() {
                            // Stored unfixed, so an interface payload element keeps
                            // its name for the slice-covariance check; the escape
                            // walk reads only the top-level shape.
                            let payloads: Vec<Ty> =
                                v.fields.iter().map(|f| lower(&f.ty, &gens)).collect();
                            self.variant_payloads
                                .insert(v.name.clone(), payloads.clone());
                            self.variant_payloads_by_enum
                                .insert((e.name.clone(), v.name.clone()), payloads);
                        }
                    }
                }
                Item::Impl(im) => {
                    if let Some(iface) = &im.iface {
                        if !self.impls.insert((iface.clone(), im.ty.clone())) {
                            self.err(
                                format!(
                                    "duplicate 'impl {iface} for {}'; merge the two blocks",
                                    im.ty
                                ),
                                im.span,
                            );
                        }
                    }
                    // Record each method's lowered parameter types so a method call
                    // can catch a value `self` in a `*T` argument position. Only the
                    // top-level pointer shape is read, so an unfixed interface name
                    // buried in a param is harmless here.
                    for m in &im.methods {
                        let gens: HashSet<String> = m.generics.iter().cloned().collect();
                        let params = m.params.iter().map(|p| lower(&p.ty, &gens)).collect();
                        self.method_params
                            .insert((im.ty.clone(), m.name.clone()), params);
                    }
                }
                _ => {}
            }
        }
        for item in &module.items {
            match item {
                Item::Func(f) => {
                    let gens: HashSet<String> = f.generics.iter().cloned().collect();
                    self.raw_sigs.insert(
                        f.name.clone(),
                        f.params.iter().map(|p| lower(&p.ty, &gens)).collect(),
                    );
                    let params = f
                        .params
                        .iter()
                        .map(|p| self.unmangle(self.fix(lower(&p.ty, &gens))))
                        .collect();
                    let ret = self.unmangle(self.fix(lower(&f.ret, &gens)));
                    if f.is_async {
                        // A call of an async func mints a Future of its declared
                        // return, so the signature table records the wrapped type
                        // and call inference types the site as a future with no
                        // extra code. async_fns keeps the element for await and
                        // async_run.
                        self.async_fns.insert(f.name.clone(), ret.clone());
                        self.sigs
                            .insert(f.name.clone(), (params, Ty::Future(Box::new(ret))));
                    } else {
                        self.sigs.insert(f.name.clone(), (params, ret));
                    }
                }
                Item::Foreign(fb) => {
                    // A foreign function is non generic, so its signature lowers
                    // against an empty generic set. It joins the same table an
                    // ordinary call resolves against, so the call is type checked.
                    let gens = HashSet::new();
                    for ff in &fb.funcs {
                        let params = ff
                            .params
                            .iter()
                            .map(|p| self.fix(lower(&p.ty, &gens)))
                            .collect();
                        let ret = self.fix(lower(&ff.ret, &gens));
                        self.sigs.insert(ff.name.clone(), (params, ret));
                    }
                }
                Item::Struct(s) => {
                    if s.name == "rune" {
                        self.err("'rune' is a builtin type name", Span::new(0, 0));
                        continue;
                    }
                    let gens: HashSet<String> = s.generics.iter().cloned().collect();
                    let is_gen = !gens.is_empty();
                    let fields = s
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), self.unmangle(self.fix(lower(&f.ty, &gens)))))
                        .collect();
                    self.structs.insert(s.name.clone(), (is_gen, fields));
                    let raw = s.fields.iter().map(|f| lower(&f.ty, &gens)).collect();
                    self.embed_fields.insert(s.name.clone(), raw);
                }
                _ => {}
            }
        }
    }

    /// Interface typed slots accept any value (the boxing and impl dispatch
    /// happen in codegen), so they lower to Unknown for compatibility purposes.
    fn fix(&self, t: Ty) -> Ty {
        match t {
            Ty::Named(n) if self.ifaces.contains(&n) => Ty::Unknown,
            Ty::Ptr(b) => Ty::Ptr(Box::new(self.fix(*b))),
            Ty::RawPtr(b) => Ty::RawPtr(Box::new(self.fix(*b))),
            Ty::Slice(b) => Ty::Slice(Box::new(self.fix(*b))),
            Ty::Array(b, n) => Ty::Array(Box::new(self.fix(*b)), n),
            Ty::Tuple(xs) => Ty::Tuple(xs.into_iter().map(|x| self.fix(x)).collect()),
            Ty::Func(ps, r) => Ty::Func(
                ps.into_iter().map(|p| self.fix(p)).collect(),
                Box::new(self.fix(*r)),
            ),
            Ty::Future(b) => Ty::Future(Box::new(self.fix(*b))),
            Ty::Collector(b) => Ty::Collector(Box::new(self.fix(*b))),
            other => other,
        }
    }

    fn run(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Func(f) => {
                    if f.name == "main" {
                        self.check_main_sig(f);
                    }
                    self.func(f, None);
                }
                Item::Impl(im) => {
                    // Codegen dispatches a method call only on a struct receiver; a
                    // method on an enum emits no call and a `match self` in its body
                    // falls to the non-enum path and silently yields a wrong value.
                    // Reject it here, naming the fix, so the broken lowering is never
                    // reached. (When codegen learns enum-receiver dispatch this guard
                    // lifts.)
                    if self.enums.contains_key(&im.ty) {
                        self.err(
                            format!(
                                "methods on the enum '{}' are not supported; methods are supported on struct types only",
                                im.ty
                            ),
                            im.span,
                        );
                        continue;
                    }
                    self.check_impl_complete(im);
                    for m in &im.methods {
                        self.func(m, Some(im.ty.as_str()));
                    }
                }
                Item::Foreign(fb) => self.check_foreign(fb),
                // Struct fields, enum payloads, and interface method signatures
                // carry types too, so the reserved unsigned names are refused here
                // as well as in function signatures and bindings. These AST nodes
                // hold no span, so the diagnostic points at the module the same way
                // the interface paradigm gate does.
                Item::Struct(s) => {
                    for fld in &s.fields {
                        self.reject_reserved_uint(&fld.ty, Span::new(0, 0));
                    }
                }
                Item::Enum(e) => {
                    for v in &e.variants {
                        for fld in &v.fields {
                            self.reject_reserved_uint(&fld.ty, Span::new(0, 0));
                        }
                    }
                }
                Item::Interface(i) => {
                    for m in &i.methods {
                        for p in &m.params {
                            self.reject_reserved_uint(&p.ty, Span::new(0, 0));
                        }
                        self.reject_reserved_uint(&m.ret, Span::new(0, 0));
                    }
                }
            }
        }
    }

    /// The only main signatures the C entry supports: no parameters, or exactly
    /// `(argc: int32, argv: string[])`, which the wrapper bridges. Any other
    /// shape would be emitted as the C `main` with the wrong parameters and read
    /// garbage registers, so it is rejected here.
    fn check_main_sig(&mut self, f: &Func) {
        if f.is_async {
            self.err(
                "main cannot be async; call an async func with async_run instead",
                f.span,
            );
            return;
        }
        if f.params.is_empty() {
            return;
        }
        let ok = f.params.len() == 2
            && !f.params.iter().any(|p| p.using)
            && self.lower(&f.params[0].ty) == Ty::Int(32)
            && matches!(self.lower(&f.params[1].ty), Ty::Slice(ref e) if matches!(**e, Ty::Str));
        if !ok {
            self.err(
                "main takes no parameters or (argc: int32, argv: string[]); the allocator form is not supported yet",
                f.span,
            );
        }
    }

    /// A foreign block ties external C symbols into the program. The abi must be
    /// "C", and every parameter and return type sits on the raw pointer layer, a
    /// scalar, a `*raw T`, or a `*void`. A managed `*T` carries a generation
    /// header C cannot read, so it never crosses, and an aggregate passed by value
    /// is left to a later interop release.
    fn check_foreign(&mut self, fb: &Foreign) {
        if fb.abi != "C" {
            self.err(
                format!(
                    "unsupported foreign abi \"{}\", only \"C\" is supported",
                    fb.abi
                ),
                fb.span,
            );
        }
        let empty = HashSet::new();
        for ff in &fb.funcs {
            for p in &ff.params {
                self.reject_reserved_uint(&p.ty, fb.span);
                let ty = self.fix(lower(&p.ty, &empty));
                self.check_foreign_ty(&ty, &ff.name, fb.span);
            }
            self.reject_reserved_uint(&ff.ret, fb.span);
            let ret = self.fix(lower(&ff.ret, &empty));
            self.check_foreign_ty(&ret, &ff.name, fb.span);
        }
    }

    fn check_foreign_ty(&mut self, ty: &Ty, fname: &str, span: Span) {
        if foreign_ty_ok(ty) {
            return;
        }
        let msg = if is_managed(ty) {
            format!(
                "foreign function '{fname}' uses a managed pointer at the C boundary; \
                 use *raw T or *void, since a managed *T carries a generation C cannot read"
            )
        } else {
            format!(
                "foreign function '{fname}' uses a type the C boundary does not support yet; \
                 a foreign signature takes scalars and raw pointers, *raw T or *void"
            )
        };
        self.err(msg, span);
    }

    /// Validates a struct literal against the struct's declared fields. Every
    /// field value is inferred first, then the set is checked for a duplicate, an
    /// unknown field, and a missing field, with a type check on each field of a
    /// non generic struct. An unknown struct name stays permissive, since it may
    /// be imported or a generic instantiation the checker does not track yet.
    fn check_struct_lit(&mut self, name: &str, fields: &[(String, Expr)], span: Span) {
        let vals: Vec<Ty> = fields.iter().map(|(_, v)| self.infer(v)).collect();
        let Some((is_gen, declared)) = self.structs.get(name).cloned() else {
            return;
        };
        for i in 0..fields.len() {
            for j in (i + 1)..fields.len() {
                if fields[i].0 == fields[j].0 {
                    self.err(
                        format!("field '{}' is set more than once in '{name}'", fields[i].0),
                        span,
                    );
                }
            }
        }
        // The unfixed field types keep an interface slice element's name, which
        // the fixed table above erases, for the slice-covariance check.
        let raw_fields = self.embed_fields.get(name).cloned().unwrap_or_default();
        for ((fname, fexpr), vty) in fields.iter().zip(&vals) {
            match declared.iter().find(|(dn, _)| dn == fname) {
                None => self.err(format!("'{name}' has no field '{fname}'"), fexpr.span),
                Some((_, dty)) => {
                    if !is_gen && !compatible(dty, vty) {
                        self.err(
                            format!(
                                "field '{fname}' of '{name}' is set to a value of the wrong type"
                            ),
                            fexpr.span,
                        );
                    }
                    if !is_gen {
                        self.check_int_fits(fexpr, dty);
                    }
                    if let Some(idx) = declared.iter().position(|(dn, _)| dn == fname) {
                        if let Some(raw) = raw_fields.get(idx) {
                            self.check_slice_covariance(&raw.clone(), vty, fexpr, fexpr.span);
                            // The declared field type keeps its interface name here,
                            // so boxing a collected value into an interface field is
                            // refused with a clean diagnostic rather than reaching
                            // codegen as a fat-rep mismatch.
                            self.reject_collector_iface_box(&raw.clone(), vty, fexpr.span);
                        }
                    }
                }
            }
        }
        for (dn, _) in &declared {
            if !fields.iter().any(|(fname, _)| fname == dn) {
                self.err(
                    format!("struct literal for '{name}' is missing field '{dn}'"),
                    span,
                );
            }
        }
    }

    fn func(&mut self, f: &Func, self_ty: Option<&str>) {
        self.cur_generics = f.generics.iter().cloned().collect();
        self.cur_ret = self.unmangle(lower(&f.ret, &self.cur_generics));
        self.branch_depth = 0;
        self.in_async = f.is_async;
        self.in_method = self_ty.is_some();
        self.cur_params = f.params.iter().map(|p| p.name.clone()).collect();
        self.cur_param_index = f
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), i))
            .collect();
        self.flow_prov = HashMap::new();
        // The locals this function binds once to a module function, so the
        // leaf-frame send check resolves a call of such a name to that function's
        // known relation instead of refusing it as an opaque callee. The summary
        // keys are exactly the module functions; the ground pass has none, so the
        // map is empty there and the send class stays off.
        let module_fns: HashSet<String> = self.summaries.keys().cloned().collect();
        self.resolvable_fn_binds = crate::sema::summary::resolvable_fn_binds(f, &module_fns);
        // The signature is the first place a generic instantiation over an
        // interface can be spelled, `func take(b: Box<Speaker>)` or a return of
        // one; reject it here so the monomorphizer never receives a request with
        // no ground shape to expand.
        for p in &f.params {
            self.reject_iface_targ(&p.ty, f.span);
            self.reject_reserved_uint(&p.ty, f.span);
            self.reject_unknown_param_type(&p.ty, f.span);
        }
        self.reject_iface_targ(&f.ret, f.span);
        self.reject_reserved_uint(&f.ret, f.span);
        if f.is_async {
            self.check_async_sig(f);
        } else if matches!(&self.cur_ret, Ty::Named(n) if self.ifaces.contains(n)) {
            // Returning an interface value by value is not supported: codegen
            // boxes the payload into a frame slot that dangles once the function
            // returns, so it is refused here rather than miscompiled. An async
            // func already rejects it through the frame-view signature walk.
            self.err(
                "returning an interface value is not supported; return the concrete type or a pointer to it",
                f.span,
            );
        }
        self.push_scope();
        if let Some(t) = self_ty {
            // `self` is the receiver value, of the concrete struct type the impl
            // names. Codegen passes the receiver by pointer and loads the value
            // for a bare `self`, so a whole-value use of `self` types as the
            // struct, not a pointer to it: `return self` against a `*T` return, or
            // `chan_send(ch, self)` into a `Channel<*T>`, is a value where a
            // pointer is required, caught by the ordinary return and argument
            // checks instead of miscompiling in the backend. Naming the receiver
            // type also lets `self.field` and `self.method()` resolve precisely.
            self.declare("self", named_ty(t));
            // A method takes its receiver by pointer, so writing through
            // `self.field` mutates the caller's value by design.
            self.declare_mut("self");
        }
        for p in &f.params {
            let ty = self.lower(&p.ty);
            // A pointer parameter is a borrow with no keyword; the callee never
            // owns or frees it.
            if is_managed(&ty) {
                self.declare_own(&p.name, Own::Borrow);
            }
            let raw = lower(&p.ty, &self.cur_generics);
            if let Ty::Named(n) = &raw {
                if self.ifaces.contains(n) {
                    self.declare_iface_bind(&p.name, n);
                }
            }
            // An error parameter carries the same must-handle obligation a
            // let-bound error does: the callee that receives it must inspect it
            // with exists, handle it with check, discard it with ignore, return it,
            // or hand it off again. Without this a call-site hand-off into an error
            // parameter would discharge the caller's obligation while the callee
            // silently dropped the error, ending the chain with no one accountable.
            if matches!(ty, Ty::Error) {
                self.declare_err(&p.name, f.span);
            }
            self.declare(&p.name, ty);
        }
        for s in &f.body.stmts {
            self.stmt(s);
        }
        self.pop_scope();
        // A non void function must return on every path; falling off the end
        // would otherwise hand back a zeroed value with no diagnostic.
        if !matches!(self.cur_ret, Ty::Unit) && !block_returns(&f.body) {
            self.err(
                format!("not all paths in '{}' return a value", f.name),
                f.span,
            );
        }
        self.in_async = false;
    }

    /// The extra signature rules an async func obeys. Its frame outlives the
    /// call, so a value that views the caller's frame, a slice, a closure, an
    /// interface value, or a future the loop owns, may not cross as a parameter
    /// or a return. It also takes no type parameters, since the task frame and
    /// its future are laid out at the single declared shape.
    fn check_async_sig(&mut self, f: &Func) {
        if !f.generics.is_empty() {
            self.err("an async func cannot take type parameters", f.span);
        }
        for p in &f.params {
            // The raw lowering keeps interface and future names intact, which the
            // fixed form would erase to Unknown. A burial behind a generic type
            // argument is beyond this erased walk and is caught in mono.
            let raw = lower(&p.ty, &self.cur_generics);
            let mut seen = HashSet::new();
            match self.async_cross_reason(&raw, &mut seen) {
                CrossFail::Future => self.err(
                    format!("an async func cannot take '{}': a future belongs to the event loop thread; await it in the caller instead", p.name),
                    f.span,
                ),
                CrossFail::View => self.err(
                    format!("an async func cannot take '{}': a slice, closure, or interface value may view the caller's frame, which the task outlives", p.name),
                    f.span,
                ),
                CrossFail::Ok => {}
            }
        }
        let raw_ret = lower(&f.ret, &self.cur_generics);
        let mut seen = HashSet::new();
        match self.async_cross_reason(&raw_ret, &mut seen) {
            CrossFail::Future => self.err(
                "an async func cannot return a future; a future belongs to the event loop thread, so await it in the caller instead",
                f.span,
            ),
            CrossFail::View => self.err(
                "an async func cannot return a slice, closure, or interface value; the value would outlive the task frame it views",
                f.span,
            ),
            CrossFail::Ok => {}
        }
    }

    /// Why a value may not cross an async boundary, so a diagnostic can name the
    /// reason. A future is loop-thread property; a slice, closure, or interface
    /// value views the frame. Walks the same shape as `spawn_capturable` but over
    /// the raw `Ty`, so it sees a future or interface a struct field buries; a
    /// burial behind a generic type argument is erased here and caught in mono.
    fn async_cross_reason(&self, t: &Ty, seen: &mut HashSet<String>) -> CrossFail {
        match t {
            Ty::Future(_) => CrossFail::Future,
            Ty::Slice(_) | Ty::Func(..) => CrossFail::View,
            Ty::Array(e, _) => self.async_cross_reason(e, seen),
            Ty::Tuple(ts) => ts
                .iter()
                .map(|x| self.async_cross_reason(x, seen))
                .find(|r| !matches!(r, CrossFail::Ok))
                .unwrap_or(CrossFail::Ok),
            Ty::Named(n) => {
                if self.ifaces.contains(n) {
                    return CrossFail::View;
                }
                if !seen.insert(n.clone()) {
                    return CrossFail::Ok;
                }
                match self.embed_fields.get(n).cloned() {
                    Some(fs) => fs
                        .iter()
                        .map(|f| self.async_cross_reason(f, seen))
                        .find(|r| !matches!(r, CrossFail::Ok))
                        .unwrap_or(CrossFail::Ok),
                    None => CrossFail::Ok,
                }
            }
            _ => CrossFail::Ok,
        }
    }

    fn lower(&self, t: &Type) -> Ty {
        self.unmangle(self.fix(lower(t, &self.cur_generics)))
    }

    /// Restores the surface `Ty::Future` shape wherever mono left a mangled
    /// `Future$T` named type, walking every carrier so a future buried in a
    /// slice, array, tuple, pointer, or function type is restored too. Empty
    /// `future_elems` on the surface pass makes this a pass-through, so it costs
    /// nothing there and only rewrites on the ground re-check.
    fn unmangle(&self, t: Ty) -> Ty {
        if self.future_elems.is_empty() {
            return t;
        }
        match t {
            Ty::Named(n) => match self.future_elems.get(&n) {
                Some(elem) => Ty::Future(Box::new(self.unmangle(elem.clone()))),
                None => Ty::Named(n),
            },
            Ty::Ptr(b) => Ty::Ptr(Box::new(self.unmangle(*b))),
            Ty::RawPtr(b) => Ty::RawPtr(Box::new(self.unmangle(*b))),
            Ty::Slice(b) => Ty::Slice(Box::new(self.unmangle(*b))),
            Ty::Array(b, n) => Ty::Array(Box::new(self.unmangle(*b)), n),
            Ty::Tuple(xs) => Ty::Tuple(xs.into_iter().map(|x| self.unmangle(x)).collect()),
            Ty::Func(ps, r) => Ty::Func(
                ps.into_iter().map(|p| self.unmangle(p)).collect(),
                Box::new(self.unmangle(*r)),
            ),
            Ty::Future(b) => Ty::Future(Box::new(self.unmangle(*b))),
            Ty::Collector(b) => Ty::Collector(Box::new(self.unmangle(*b))),
            other => other,
        }
    }

    /// Rejects an interface named as a generic type argument, anywhere it is
    /// buried in an annotation: `Box<Speaker>`, `Box<Pair<Speaker>>`, or
    /// `Box<Speaker>[]`. A generic is monomorphized into one concrete copy per
    /// distinct type argument, and an interface has no single concrete layout to
    /// instantiate against, so it cannot stand in for a type parameter. Left
    /// unchecked the request reaches the monomorphizer, which has no ground shape
    /// to expand and used to diverge; catching it here reports at the source
    /// annotation with the fix named. A bare `T` in `self.cur_generics` is a type
    /// parameter, not an interface, and is skipped.
    fn reject_iface_targ(&mut self, t: &Type, span: Span) {
        match t {
            Type::Named(_, args) => {
                for a in args {
                    if let Type::Named(n, _) = a {
                        // A type parameter in scope shadows any interface of the
                        // same name, so `func wrap<Speaker>(...) -> Box<Speaker>`
                        // names the parameter, not the interface, and must not be
                        // rejected. Only a genuine interface name, one not bound as
                        // a current type parameter, is refused.
                        if self.ifaces.contains(n) && !self.cur_generics.contains(n) {
                            self.err(
                                "an interface cannot be a generic type argument; generics are monomorphized over concrete types",
                                span,
                            );
                        }
                    }
                    self.reject_iface_targ(a, span);
                }
            }
            Type::Ptr(b) | Type::RawPtr(b) | Type::Slice(b) | Type::Array(b, _) => {
                self.reject_iface_targ(b, span)
            }
            Type::Tuple(ts) => {
                for x in ts {
                    self.reject_iface_targ(x, span);
                }
            }
            Type::Func(ps, r) => {
                for p in ps {
                    self.reject_iface_targ(p, span);
                }
                self.reject_iface_targ(r, span);
            }
            Type::Collector(b) => self.reject_iface_targ(b, span),
            Type::Unit | Type::Infer => {}
        }
    }

    /// Rejects a use of a reserved unsigned integer name anywhere in a type
    /// annotation. The `uint8`..`uint64` names are parsed and would lower to the
    /// same width as their signed twin, so every operation on one, printing, a
    /// comparison, a divide, runs the signed path and yields a wrong value with no
    /// diagnostic. They are reserved until real unsigned support lands, so a type
    /// that names one is refused up front. Recurses through pointers, slices,
    /// arrays, tuples, functions, the collector wrapper, and generic arguments so
    /// a buried `Box<uint8>` or `uint8[]` is caught the same as a bare annotation.
    fn reject_reserved_uint(&mut self, t: &Type, span: Span) {
        match t {
            Type::Named(n, args) => {
                if matches!(n.as_str(), "uint8" | "uint16" | "uint32" | "uint64") {
                    self.err(
                        "unsigned integers are reserved; use the signed widths",
                        span,
                    );
                }
                for a in args {
                    self.reject_reserved_uint(a, span);
                }
            }
            Type::Ptr(b) | Type::RawPtr(b) | Type::Slice(b) | Type::Array(b, _) => {
                self.reject_reserved_uint(b, span)
            }
            Type::Tuple(ts) => {
                for x in ts {
                    self.reject_reserved_uint(x, span);
                }
            }
            Type::Func(ps, r) => {
                for p in ps {
                    self.reject_reserved_uint(p, span);
                }
                self.reject_reserved_uint(r, span);
            }
            Type::Collector(b) => self.reject_reserved_uint(b, span),
            Type::Unit | Type::Infer => {}
        }
    }

    /// Rejects a parameter type that names a type the module does not define. A
    /// parameter is materialized in codegen, a `using` parameter allocates a frame
    /// slot for it, so a phantom named type like `Collector` (never declared)
    /// lowers to an unsized slot and clang aborts with `Cannot allocate unsized
    /// type`. Every other type position either has a value to check the annotation
    /// against or never reaches the backend, so this walk is scoped to parameters,
    /// where an unused, undeclared type otherwise slips straight through. A
    /// generic parameter in scope, a primitive, `Future`, and any declared struct,
    /// enum, or interface are all known; anything else is refused. Surface pass
    /// only: mono mangles names, so the ground pass would misread a valid slot.
    fn reject_unknown_param_type(&mut self, t: &Type, span: Span) {
        if self.types_only {
            return;
        }
        match t {
            Type::Named(n, args) => {
                if !self.is_known_type_name(n) {
                    self.err(
                        format!("unknown type '{n}'; no type of that name is declared or imported"),
                        span,
                    );
                }
                for a in args {
                    self.reject_unknown_param_type(a, span);
                }
            }
            Type::Ptr(b) | Type::RawPtr(b) | Type::Slice(b) | Type::Array(b, _) => {
                self.reject_unknown_param_type(b, span)
            }
            Type::Tuple(ts) => {
                for x in ts {
                    self.reject_unknown_param_type(x, span);
                }
            }
            Type::Func(ps, r) => {
                for p in ps {
                    self.reject_unknown_param_type(p, span);
                }
                self.reject_unknown_param_type(r, span);
            }
            Type::Collector(b) => self.reject_unknown_param_type(b, span),
            Type::Unit | Type::Infer => {}
        }
    }

    /// Whether a type name is one the checker knows: a primitive (which lowers to
    /// a non-`Named` type), the one-shot `Future`, a generic parameter in scope, or
    /// a declared struct, enum, or interface. The reserved unsigned names read as
    /// known here so they earn the precise reserved diagnostic from
    /// `reject_reserved_uint` rather than a bare unknown-type message.
    fn is_known_type_name(&self, n: &str) -> bool {
        n == "Future"
            || !matches!(named_ty(n), Ty::Named(_))
            || self.cur_generics.contains(n)
            || self.structs.contains_key(n)
            || self.enums.contains_key(n)
            || self.ifaces.contains(n)
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.owns.push(HashMap::new());
        self.muts.push(HashSet::new());
        self.err_binds.push(HashMap::new());
        self.iface_binds.push(HashMap::new());
        self.enum_binds.push(HashMap::new());
        self.slice_iface_elem.push(HashMap::new());
        self.esc_closures.push(HashMap::new());
        self.esc_slices.push(HashMap::new());
        self.alias_edges.push(HashMap::new());
        self.ptr_param_borrows.push(HashMap::new());
        self.lambda_sink_binds.push(HashMap::new());
        self.lambda_capture_binds.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.owns.pop();
        self.muts.pop();
        self.iface_binds.pop();
        self.enum_binds.pop();
        self.slice_iface_elem.pop();
        self.esc_closures.pop();
        self.esc_slices.pop();
        self.alias_edges.pop();
        self.ptr_param_borrows.pop();
        self.lambda_sink_binds.pop();
        self.lambda_capture_binds.pop();
        // Every error bound in this scope must have been handled by now. The
        // handled ones were removed at their handling site; the rest report. The
        // stack is popped either way to stay balanced, but the ground re-check
        // suppresses the report, which the surface pass already made.
        if let Some(pending) = self.err_binds.pop() {
            if !self.types_only {
                let mut pending: Vec<(String, Span)> = pending.into_iter().collect();
                pending
                    .sort_by(|(an, aspan), (bn, bspan)| aspan.lo.cmp(&bspan.lo).then(an.cmp(bn)));
                for (name, span) in pending {
                    self.err(
                        format!(
                            "the error '{name}' is never handled; inspect it with exists, handle it with check, or discard it with ignore"
                        ),
                        span,
                    );
                }
            }
        }
    }

    /// Marks a binding as mutable in the current scope.
    fn declare_mut(&mut self, name: &str) {
        if let Some(scope) = self.muts.last_mut() {
            scope.insert(name.to_string());
        }
    }

    fn is_mutable(&self, name: &str) -> bool {
        self.muts.iter().rev().any(|s| s.contains(name))
    }

    /// Records a binding annotated with an interface type in the current scope.
    fn declare_iface_bind(&mut self, name: &str, iface: &str) {
        if let Some(scope) = self.iface_binds.last_mut() {
            scope.insert(name.to_string(), iface.to_string());
        }
    }

    fn is_iface_bind(&self, name: &str) -> bool {
        self.iface_binds.iter().rev().any(|s| s.contains_key(name))
    }

    fn declare_enum_bind(&mut self, name: &str, en: &str) {
        if let Some(scope) = self.enum_binds.last_mut() {
            scope.insert(name.to_string(), en.to_string());
        }
    }

    /// The enum a binding was initialized from through a constructor, when it
    /// names one. An enum local types as Unknown, so this recovers the enum for
    /// the enum-receiver method reject.
    fn enum_bind_name(&self, name: &str) -> Option<String> {
        self.enum_binds
            .iter()
            .rev()
            .find_map(|s| s.get(name).cloned())
    }

    /// The interface a binding was annotated with, when it names one. The receiver
    /// of a method call is fixed to Unknown in scope (interfaces erase for the
    /// value passes), so this recovers the interface name a dynamic-dispatch call
    /// needs to look up its method's parameter types for the covariance guard.
    fn iface_bind_name(&self, name: &str) -> Option<String> {
        self.iface_binds
            .iter()
            .rev()
            .find_map(|s| s.get(name).cloned())
    }

    fn is_esc_closure(&self, name: &str) -> bool {
        Self::esc_flag(&self.esc_closures, name)
    }

    fn is_esc_slice(&self, name: &str) -> bool {
        Self::esc_flag(&self.esc_slices, name)
    }

    /// The escape flag of a name from the innermost scope that binds it. Stopping
    /// at the first scope that has the key lets an inner binding, a shadow or a
    /// reassignment, write `false` and mask an outer or stale `true`.
    fn esc_flag(scopes: &[HashMap<String, bool>], name: &str) -> bool {
        for scope in scopes.iter().rev() {
            if let Some(&flag) = scope.get(name) {
                return flag;
            }
        }
        false
    }

    /// Records, in the current scope, whether a binding holds an escaping slice or
    /// closure. The flag is written unconditionally, not removed, so a reassign to
    /// a clean value writes `false` and masks a stale `true` in the same scope,
    /// and a shadowing bind writes its own flag into the inner scope.
    fn set_esc(&mut self, name: &str, slice: bool, closure: bool) {
        if let Some(scope) = self.esc_slices.last_mut() {
            scope.insert(name.to_string(), slice);
        }
        if let Some(scope) = self.esc_closures.last_mut() {
            scope.insert(name.to_string(), closure);
        }
        // A fresh binding of this name starts with no store-edge provenance. The
        // flag stacks are scope-nested, so a shadow masks an outer binding's flag,
        // but flow_prov is flat, so a stale entry from an outer binding of the same
        // name would misdirect the diagnostic to the outer store site. Clearing it
        // here keeps the provenance in step with the binding the flag describes; a
        // later store edge re-records it for this binding.
        self.flow_prov.remove(name);
    }

    /// Updates the escape flags of a reassigned binding in the scope that owns it,
    /// not the current branch scope which a pop would discard. Inside a
    /// conditional or loop body the update is a may-join: it can only raise a
    /// flag, since the branch may or may not run, so `if c { t = escaping }`
    /// leaves t able to escape and `if c { t = clean }` cannot prove it clean. At
    /// straight line the last assignment wins, so an unconditional reassign to a
    /// clean value clears a prior escape.
    fn assign_esc(&mut self, name: &str, slice: bool, closure: bool) {
        let conditional = self.branch_depth > 0;
        Self::update_owner(&mut self.esc_slices, name, slice, conditional);
        Self::update_owner(&mut self.esc_closures, name, closure, conditional);
    }

    fn update_owner(
        scopes: &mut [HashMap<String, bool>],
        name: &str,
        flag: bool,
        conditional: bool,
    ) {
        for scope in scopes.iter_mut().rev() {
            if let Some(entry) = scope.get_mut(name) {
                *entry = if conditional { *entry || flag } else { flag };
                return;
            }
        }
        if let Some(scope) = scopes.last_mut() {
            scope.insert(name.to_string(), flag);
        }
    }

    /// Raises the escape flags of a binding, never clearing them. A field store,
    /// `s.rows = ...`, can only add a frame-local view to a struct binding, since
    /// another field may still hold one, so it joins into the owning scope like a
    /// conditional assignment regardless of branch depth.
    fn raise_esc(&mut self, name: &str, slice: bool, closure: bool) {
        // A frame view stored through one name pollutes the heap object every name
        // in its alias group reaches, so raising the flag here carries it to the
        // whole group: a store through `q` (a `ref` of `c`) taints `c`, a store
        // through a struct binding that embeds `c` taints `c`, and a later return
        // of any group member is caught. The join is unconditional, like the raise
        // itself, since a store can only add a view. The single sink covers the
        // direct store path, the call-flow paths, and the capture-flow path, all of
        // which raise through here. The propagation is deliberately absent from
        // set_esc and assign_esc: a struct bound with an unrelated frame-view
        // sibling field raises the struct there and must not taint the embedded
        // pointer.
        for m in self.alias_group(name) {
            Self::update_owner(&mut self.esc_slices, &m, slice, true);
            Self::update_owner(&mut self.esc_closures, &m, closure, true);
        }
    }

    /// Records a symmetric alias edge between two managed-pointer binding names in
    /// the current scope. A self-edge is a no-op. The edge lets a frame view raised
    /// on one name reach the other, so a store through an alias taints the name a
    /// later return escapes.
    fn alias_link(&mut self, a: &str, b: &str) {
        if a == b {
            return;
        }
        if let Some(scope) = self.alias_edges.last_mut() {
            scope
                .entry(a.to_string())
                .or_default()
                .insert(b.to_string());
            scope
                .entry(b.to_string())
                .or_default()
                .insert(a.to_string());
        }
    }

    /// The transitive alias closure of a name: every binding reachable from it over
    /// the union of every stacked scope's adjacency, by breadth-first search, so a
    /// chain (`ref c := p; ref q := c`) and a cross-scope link both resolve. Always
    /// includes the name itself, so a name with no edges returns the singleton set.
    fn alias_group(&self, name: &str) -> HashSet<String> {
        let mut seen = HashSet::new();
        seen.insert(name.to_string());
        let mut queue = vec![name.to_string()];
        while let Some(cur) = queue.pop() {
            for scope in &self.alias_edges {
                if let Some(neighbors) = scope.get(&cur) {
                    for n in neighbors {
                        if seen.insert(n.clone()) {
                            queue.push(n.clone());
                        }
                    }
                }
            }
        }
        seen
    }

    /// Drops every alias edge touching a name, from the innermost scope that binds
    /// it, mirroring drop_lambda_sink. A binding reassigned to a value dusk cannot
    /// alias (a non-pointer, a projection, a call result) no longer aliases its old
    /// group, so its edges are removed unconditionally; the drop can only narrow the
    /// group, which stays sound. The symmetric back-edges from its neighbors are
    /// removed too, so no stale edge points back at the dropped name.
    fn alias_drop(&mut self, name: &str) {
        for scope in self.alias_edges.iter_mut().rev() {
            if let Some(neighbors) = scope.remove(name) {
                for n in neighbors {
                    if let Some(back) = scope.get_mut(&n) {
                        back.remove(name);
                    }
                }
                break;
            }
        }
    }

    /// Classifies a bound value as holding a frame-local slice and/or closure, so
    /// returning the binding later is caught even when the return names the bare
    /// binding. A slice into a local array (an array literal or a range slice of a
    /// local array), a lambda that captures a local, an alias of a binding that
    /// already holds either, or a tuple whose member does, all propagate. A slice
    /// from a parameter or a heap array, and a closure capturing only parameters
    /// or globals, do not, so a legal return is not over-rejected.
    fn value_escape(&self, e: &Expr) -> (bool, bool) {
        match &e.kind {
            ExprKind::Array(_) => (true, false),
            ExprKind::Index(base, idx) if matches!(idx.kind, ExprKind::Range(..)) => {
                // A range slice escapes when its base is a local array, or when the
                // base is a binding already known to hold a frame-local slice; an
                // unannotated array literal binding infers to a slice, so the type
                // check alone would miss re-slicing it.
                let base_local = matches!(self.chain_ty(base), Ty::Array(..))
                    || matches!(&base.kind, ExprKind::Ident(n) if self.is_esc_slice(n))
                    || self.value_escape(base).0;
                (base_local, false)
            }
            ExprKind::Lambda(l) => (false, self.lambda_captures_local(l)),
            ExprKind::Ident(n) => (self.is_esc_slice(n), self.is_esc_closure(n)),
            // Reading a value out through a bare dereference yields whatever a
            // store edge (or the caller) put behind the pointer. A fat pointee
            // that roots to a binding flagged as holding a frame view carries
            // that view out, so `return *p` after `p := alloc(local[0..n])`
            // dangles the same as returning the slice directly. A scalar pointee
            // or a clean pointer stays clean. Field and index chains that cross
            // a deref are handled by projection_escape's deref root instead.
            ExprKind::Unary(UnOp::Deref, _) => {
                if !self.member_carries_view(&self.chain_ty(e)) {
                    return (false, false);
                }
                match store_root(e) {
                    Some(root) => (self.is_esc_slice(&root), self.is_esc_closure(&root)),
                    None => (false, false),
                }
            }
            // A projection extracts a fat sub-value out of an aggregate. When the
            // aggregate roots to a local binding holding a frame-local view, the
            // projected slice, closure, tuple, struct, or enum views it too. A
            // scalar member, or a projection rooted in a param or heap aggregate,
            // does not escape. The range-index re-slice is handled above.
            ExprKind::Field(..) | ExprKind::Index(..) => self.projection_escape(e),
            ExprKind::Tuple(members) => {
                let mut slice = false;
                let mut closure = false;
                for m in members {
                    let (s, c) = self.value_escape(m);
                    slice |= s;
                    closure |= c;
                }
                (slice, closure)
            }
            // A struct returned by value carries its fat fields, so a field
            // holding a frame-local view escapes through it. Only a reference
            // shaped field, a slice, closure, tuple, or nested struct, can view a
            // frame; a fixed array or scalar field holds its value inline.
            ExprKind::StructLit(name, fields) => {
                let mut slice = false;
                let mut closure = false;
                if let Some((_, decl)) = self.structs.get(name).cloned() {
                    for (fname, init) in fields {
                        if let Some((_, fty)) = decl.iter().find(|(n, _)| n == fname) {
                            let (s, c) = self.field_kind_escape(init, fty);
                            slice |= s;
                            closure |= c;
                        }
                    }
                }
                (slice, closure)
            }
            // An enum constructor returned by value carries its payload, so a fat
            // payload viewing a frame local escapes. The payload is gated by the
            // variant's declared field types, the same discipline as a struct
            // field. Both the bare `V(x)` and the qualified `E.V(x)` forms resolve
            // to the variant by its globally unique name. Any other call is a
            // plain function or closure call, whose result views a frame when the
            // callee returns one of its frame-view arguments (M5, interprocedural).
            ExprKind::Call(callee, args) => {
                if let Some(vname) = self.variant_name(callee) {
                    let payloads = self
                        .variant_payloads
                        .get(vname)
                        .cloned()
                        .unwrap_or_default();
                    let mut slice = false;
                    let mut closure = false;
                    for (arg, pty) in args.iter().zip(&payloads) {
                        let (s, c) = self.field_kind_escape(arg, pty);
                        slice |= s;
                        closure |= c;
                    }
                    return (slice, closure);
                }
                self.call_result_escape(callee, args)
            }
            // A match used as a return value builds its result from the arm tails.
            // A tail that names one of the arm's payload bindings projects the
            // scrutinee's payload, so it inherits the scrutinee's escape; any other
            // tail is classified on its own.
            ExprKind::Match(m) => {
                let (ss, sc) = self.value_escape(&m.scrut);
                let mut slice = false;
                let mut closure = false;
                for arm in &m.arms {
                    if let Some(Stmt::Expr(tail)) = arm.body.stmts.last() {
                        let projects_payload = matches!(
                            (&tail.kind, &arm.pat),
                            (ExprKind::Ident(n), Pattern::Variant(_, binds)) if binds.contains(n)
                        );
                        let (s, c) = if projects_payload {
                            (ss, sc)
                        } else {
                            self.value_escape(tail)
                        };
                        slice |= s;
                        closure |= c;
                    }
                }
                (slice, closure)
            }
            _ => (false, false),
        }
    }

    /// The escape of a projection, a field access or a non-range index. Only a fat
    /// projected member can carry a view; the projection escapes when its base
    /// roots to a binding recorded as holding a frame-local slice or closure, or
    /// when the chain reads through a pointer binding a call polluted with one,
    /// so `(*c).rows` after a store edge tainted `c` is a frame view here too.
    fn projection_escape(&self, e: &Expr) -> (bool, bool) {
        if !self.member_carries_view(&self.chain_ty(e)) {
            return (false, false);
        }
        let mut slice = false;
        let mut closure = false;
        if let Some(root) = self.projection_root(e) {
            slice |= self.is_esc_slice(&root);
            closure |= self.is_esc_closure(&root);
        }
        if let Some(root) = deref_root(e) {
            slice |= self.is_esc_slice(&root);
            closure |= self.is_esc_closure(&root);
        }
        (slice, closure)
    }

    /// The base binding of a projection chain, rooting through every field access
    /// and index, whether the indexed base is an array or a slice. Unlike
    /// value_chain_root, which stops the immutability walk at a slice indirection,
    /// this reaches the binding whose escape flag governs the projected view, since
    /// indexing a slice that views a frame local still yields a frame-local view.
    /// A pointer dereference roots to nothing: its target is heap.
    fn projection_root(&self, e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            ExprKind::Field(base, _) | ExprKind::Index(base, _) => self.projection_root(base),
            _ => None,
        }
    }

    /// The single binding-alias choke every binding-introduction site funnels
    /// through, so the rule for "which managed pointers does this new binding
    /// alias" lives in one function instead of being re-derived at each binding
    /// form. Given a freshly bound (or reassigned) name and its initializer, it
    /// links the name into the alias group of every managed pointer the
    /// initializer hands it, computed by `binding_alias_targets`: a bare Ident
    /// whose type reaches a managed pointer (a `ref`/borrow chain, or a whole
    /// aggregate copied by name), a by-value projection (field, index, deref)
    /// that reads a managed value out of an aggregate and joins the projection
    /// root's group, and each bare pointer embedded in an aggregate literal (a
    /// struct, tuple, array, or variant constructor) or in an `alloc` of one, at
    /// any nesting depth. A slice, scalar, or any type that reaches no managed
    /// pointer hands no target, so precision holds: a frame-slice sibling never
    /// taints the embedded pointer. `reassign` selects the mut-reassign join: a
    /// fresh binding has no prior edges and passes `false`; an Assign passes
    /// `true`, where at straight line the name leaves its old group first
    /// (replace) and inside a branch the old edges stay while the new targets
    /// union on top (a may-join, since the name may still hold its prior
    /// pointer). A reassign to a value that hands no target drops the old edges
    /// unconditionally, the name no longer holding its old pointer through that
    /// value; the drop can only narrow the group, which stays sound.
    fn link_binding_aliases(&mut self, name: &str, init: &Expr, reassign: bool) {
        let targets = self.binding_alias_targets(init);
        if reassign && (self.branch_depth == 0 || targets.is_empty()) {
            self.alias_drop(name);
        }
        for t in &targets {
            self.alias_link(name, t);
        }
    }

    /// The managed-pointer alias targets an initializer hands its binding: the
    /// bare-Ident chain, the projection root, or every pointer an aggregate
    /// literal (or an `alloc` of one) embeds at any depth. Pure over the checker
    /// state; the caller decides whether to link (a fresh binding) or reassign
    /// (an Assign). This is the one place the binding forms agree on the rule, so
    /// a new binding site need only route its initializer here.
    fn binding_alias_targets(&self, init: &Expr) -> Vec<String> {
        let mut out = Vec::new();
        match &init.kind {
            // A bare binding hands its whole alias group whenever its type can
            // reach a managed pointer, not only when it is a bare pointer: a
            // `ref`/borrow chain, or an aggregate copied by name that buries a
            // pointer, both carry it. A plain copy of an owner is rejected at
            // binding_own; a `move` reads as a call, not an Ident, and hands no
            // target here.
            ExprKind::Ident(src) if self.links_as_alias(&self.lookup(src)) => {
                out.push(src.clone());
            }
            // A by-value projection reads a managed value out of an aggregate and
            // joins the projection root's group: `x := st.c`, `x := arr[0]`, and
            // `x := (*b).c` each read a pointer (or a pointer-bearing aggregate)
            // out of a larger object, so a store through `x` taints what a return
            // of the root, or a group member, escapes. A rootless projection (of
            // a call result) has no root and hands no target.
            ExprKind::Field(..) | ExprKind::Index(..) | ExprKind::Unary(UnOp::Deref, _)
                if self.chain_projection_manages(init) =>
            {
                if let Some(root) = store_root(init) {
                    out.push(root);
                }
            }
            // An aggregate literal, or an `alloc` of one, buries the bare
            // pointers it wraps at any nesting depth, so the binding aliases each
            // one: `outer := Outer { inner: inner }` reaches the pointer `inner`
            // buries, and `b := alloc(Box { c: c })` aliases `c` a projection
            // reads back out. The walk descends only aggregate literals; a `move`
            // or a fresh `alloc` member reads as a call and hands no target.
            ExprKind::Tuple(members) | ExprKind::Array(members) => {
                for m in members {
                    self.collect_embedded(m, &mut out);
                }
            }
            ExprKind::StructLit(_, fields) => {
                for (_, f) in fields {
                    self.collect_embedded(f, &mut out);
                }
            }
            ExprKind::Call(callee, args) => {
                // A variant constructor buries its payload the same way a struct
                // literal buries a field: `o := Some(c)` embeds `c`, so a store
                // through a projection of `o` (or a match binder off it) taints
                // `c` and a later return of it is caught. The compiler builds the
                // variant here, so its payload is transparent, unlike an opaque
                // function call whose buried alias is the summary domain's job.
                if self.variant_name(callee).is_some() {
                    for a in args {
                        self.collect_embedded(a, &mut out);
                    }
                } else if args.len() == 1
                    && matches!(&callee.kind, ExprKind::Ident(n) if n == "alloc" && !self.sigs.contains_key(n))
                {
                    // `b := alloc(Box { c: c })` allocates a heap object that
                    // embeds the bare pointers its initializer names; a projection
                    // reads one back out (`x := (*b).c`), so the allocation
                    // binding aliases each embedded pointer the same as a stack
                    // aggregate does. A nested `move` or `alloc` member reads as a
                    // call and is skipped at every layer.
                    self.collect_embedded(&args[0], &mut out);
                }
            }
            _ => {}
        }
        out
    }

    /// Collects every bare managed-pointer binding an aggregate literal embeds,
    /// at any nesting depth, into `out`. A member named by a bare binding whose
    /// type reaches a managed pointer is a target; a nested struct, tuple, or
    /// array member is descended. A member that is a `move`, a fresh `alloc`, or
    /// any non-literal reads as a call, not a nested literal, so the walk steps
    /// past it and collects nothing there, at every layer. A slice, array, or
    /// scalar member reaches no managed pointer and is collected only when it in
    /// turn nests a pointer, so a frame-slice sibling stays off the pointer.
    fn collect_embedded(&self, e: &Expr, out: &mut Vec<String>) {
        match &e.kind {
            ExprKind::Ident(mn) if self.links_as_alias(&self.lookup(mn)) => {
                out.push(mn.clone());
            }
            ExprKind::Tuple(members) | ExprKind::Array(members) => {
                for m in members {
                    self.collect_embedded(m, out);
                }
            }
            ExprKind::StructLit(_, fields) => {
                for (_, init) in fields {
                    self.collect_embedded(init, out);
                }
            }
            _ => {}
        }
    }

    /// Whether a type can carry a frame-local view when projected out by value: a
    /// slice, closure, tuple, struct, enum, interface, a managed pointer (whose
    /// heap object a store edge may have polluted with a view), or a fat array
    /// of any of them. A scalar or a scalar array is copied and cannot. `*void`
    /// is the raw allocator currency and never a view carrier.
    ///
    /// A `*raw T` is deliberately excluded: the raw pointer layer is the FFI
    /// boundary, honor-system by design, so the escape walk does not trace a view
    /// stored through it and the managed-pointer generation backstop does not
    /// cover it. A frame view stashed through a `*raw T` is the caller's
    /// responsibility, the same contract a foreign pointer carries.
    fn member_carries_view(&self, t: &Ty) -> bool {
        match t {
            Ty::Slice(_) | Ty::Func(..) | Ty::Tuple(_) => true,
            Ty::Ptr(inner) => !matches!(**inner, Ty::Unit),
            // The raw pointer layer is honor-system; a view stored through it is
            // outside the escape walk by design, so this arm is `false` on
            // purpose rather than falling through the wildcard.
            Ty::RawPtr(_) => false,
            Ty::Named(n) => {
                self.structs.contains_key(n)
                    || self.enums.contains_key(n)
                    || self.ifaces.contains(n)
            }
            Ty::Array(e, _) => self.member_carries_view(e),
            _ => false,
        }
    }

    /// The escape check a `collector<T>(value)` mint runs. A mint is an outliving
    /// sink: the collected block escapes the frame just as a return does, so a
    /// frame view carried by the minted value, or stored behind a minted pointer's
    /// pointee, dangles once the frame is gone. It reuses the same `value_escape`
    /// and flow-provenance walk the `Ty::Ptr` return arm uses, so a bare polluted
    /// pointer, a struct embedding one, and a laundered call result all classify
    /// the same way. Surface only, like every escape check; the type-shape guard
    /// `ty_reaches_view` and the ground re-check cover the mislowering and
    /// laundering directions. `elem` is the element type, so a pointer element
    /// earns the pointer message and a struct or slice element the view message.
    fn check_collect_escape(&mut self, arg: &Expr, elem: &Ty, span: Span) {
        if self.types_only {
            return;
        }
        let (esc_s, esc_c) = self.value_escape(arg);
        if !esc_s && !esc_c {
            return;
        }
        if let Some((s, i, j)) = self.value_flow_prov(arg) {
            self.emit_flow_escape(s, i, j);
        } else if matches!(elem, Ty::Ptr(_)) {
            let s = self
                .call_frame_origin(arg)
                .map(|(sp, _)| sp)
                .unwrap_or(span);
            self.emit_escape(
                "this collects a pointer to an object that stores a view of the current frame; the collected block outlives the frame".to_string(),
                s,
            );
        } else if let Some((s, _)) = self.call_frame_origin(arg) {
            // A collected call result that returns a view of a frame argument is
            // an outliving collect, not a return; the shared slice/closure
            // reporters would name a return here, so the mint site words it as a
            // collect directly, matching the pointer message above.
            self.emit_escape(
                "this collects a value that views the current frame; the collected block outlives the frame".to_string(),
                s,
            );
        } else if esc_s {
            self.report_slice_escape(arg);
        } else {
            self.report_closure_escape(arg);
        }
    }

    /// The capture rule a `collector<F>` mint runs. The minted environment lives
    /// on the collected heap and outlives the frame, so a capture that views a
    /// frame would dangle once the frame is gone. A capture is rejected on either
    /// of two grounds. First, its type must be immortal safe: a scalar, a managed
    /// pointer the registry roots, a string, a nested `collector<..>`, or an
    /// aggregate of those. A view-reaching type that is not itself collector typed,
    /// a slice, closure, or interface, fails, the negation of `ty_reaches_view`,
    /// so a collector-typed capture whose element the wrapper reads as safe passes.
    /// Second, a managed pointer capture whose pointee stores a frame view escapes
    /// through the environment just as the plain mint's escape walk catches, so the
    /// same per-binding view flags gate it: `p := alloc(Inner { s: local[0..n] })`
    /// captured here dangles the buried slice exactly as returning `p` would. Both
    /// grounds name the binding so the fix is clear: collect it first, or capture
    /// heap owned data. A generic-typed capture reads `Unknown` and passes on the
    /// surface, and the ground re-check catches it once mono grounds the type; the
    /// flow ground is surface only, like every escape check. A capture-free lambda
    /// has nothing to check.
    fn check_collect_captures(&mut self, arg: &Expr, span: Span) {
        let ExprKind::Lambda(l) = &arg.kind else {
            self.err(
                "a collected closure must be a lambda literal written at the mint",
                arg.span,
            );
            return;
        };
        for n in self.spawn_lambda_captures(l) {
            let t = self.lookup(&n);
            let mut seen = HashSet::new();
            let type_views = self.ty_reaches_view(&t, &mut seen);
            // A bare Ident's escape is exactly its two per-binding view flags, so a
            // managed pointer whose pointee was tainted with a frame view is caught
            // here without walking the value again. Surface only.
            let flow_views = !self.types_only && (self.is_esc_slice(&n) || self.is_esc_closure(&n));
            if type_views || flow_views {
                self.err(
                    format!(
                        "cannot collect a closure that captures '{n}': it may view a frame; collect '{n}' first or capture heap owned data"
                    ),
                    span,
                );
            }
        }
    }

    /// Whether a slice-collector source reaches a managed pointer whose pointee
    /// stores a frame view. The deep copy immortalizes the element storage but not
    /// what each managed pointer points at, so a tainted pointee still dangles.
    /// The outer slice or array being a frame view is not the concern, the copy
    /// fixes that, so this looks only at the buried pointers: a binding or a
    /// projection of one hands its alias group, the pointers the binding choke
    /// linked in, and a literal source is walked for the pointers it embeds. A
    /// group member that is a managed pointer and carries a view flag is a tainted
    /// pointee. Surface only, like every escape check; the alias group and the view
    /// flags are not tracked on the ground pass.
    fn slice_source_buries_view(&self, arg: &Expr) -> bool {
        if self.types_only {
            return false;
        }
        let mut ptrs: Vec<String> = Vec::new();
        if let Some(root) = store_root(arg) {
            ptrs.extend(self.alias_group(&root));
        } else {
            self.collect_embedded(arg, &mut ptrs);
        }
        ptrs.iter().any(|m| {
            is_managed(&self.lookup(m)) && (self.is_esc_slice(m) || self.is_esc_closure(m))
        })
    }

    /// Whether a value of this type can carry a frame-local view, so minting it
    /// into a collected block would copy a fat pointer whose backing dies with the
    /// frame. A slice or a closure is a view directly; an interface value carries
    /// a data pointer that may sit in the frame; a struct, enum, tuple, or array
    /// reaches one through a field, payload, member, or element. A managed `*T`
    /// does not: its block is a root the collector scans, not a frame view, so it
    /// is allowed, and a `*raw T` is the honor-system FFI layer and never counts.
    /// An unresolved generic hole reads as `false` here: the surface pass leaves a
    /// `collector<T>` element `Unknown`, and the ground re-check sees the concrete
    /// element and rejects then, exactly as the free/move/ref laundering closure
    /// does. The struct and enum walk reads the unfixed `embed_fields`, so an
    /// interface field is seen where the fixed table would erase it to `Unknown`.
    /// `seen` breaks recursive type cycles.
    fn ty_reaches_view(&self, t: &Ty, seen: &mut HashSet<String>) -> bool {
        match t {
            Ty::Slice(_) | Ty::Func(..) => true,
            Ty::Ptr(_) | Ty::RawPtr(_) => false,
            Ty::Array(e, _) => self.ty_reaches_view(e, seen),
            Ty::Tuple(ts) => ts.iter().any(|x| self.ty_reaches_view(x, seen)),
            Ty::Named(n) => {
                if self.ifaces.contains(n) {
                    return true;
                }
                if !seen.insert(n.clone()) {
                    return false;
                }
                match self.embed_fields.get(n).cloned() {
                    Some(fs) => fs.iter().any(|f| self.ty_reaches_view(f, seen)),
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// The escape of a struct field or enum payload initializer, gated by the
    /// declared field type. A reference shaped field, a slice, closure, tuple,
    /// struct, enum, or fat fixed array, views whatever its initializer does; a
    /// fixed array of scalars or a scalar field copies its initializer inline, so
    /// a local array literal in it does not escape. This covers the SAME carrier
    /// set as the top-level escape walk, so a carrier nested one level down, a
    /// field whose type is an enum or fat array, is caught at any depth.
    fn field_kind_escape(&self, init: &Expr, fty: &Ty) -> (bool, bool) {
        match fty {
            // A fat fixed array mirrors the top-level array walk: a literal is
            // checked per element by the element type, a binding by its recorded
            // flags. Routing straight to value_escape would flag a param-backed
            // array literal, so the element gate is kept here; the carrier set is
            // `member_carries_view`, the same one every gate uses.
            Ty::Array(elem, _) if self.member_carries_view(elem) => {
                if let ExprKind::Array(elems) = &init.kind {
                    let mut slice = false;
                    let mut closure = false;
                    for el in elems {
                        let (s, c) = self.field_kind_escape(el, elem);
                        slice |= s;
                        closure |= c;
                    }
                    (slice, closure)
                } else {
                    self.value_escape(init)
                }
            }
            // Any other carrier, a slice, closure, tuple, struct, enum,
            // interface, or managed pointer whose heap object a store edge may
            // have polluted, views whatever its initializer does. The single
            // `member_carries_view` gate replaces the old per-constructor list,
            // so this layer and the projection/return gates cannot drift apart.
            _ if self.member_carries_view(fty) => self.value_escape(init),
            // A generic type parameter, or an interface field, erases to Unknown,
            // so the declared type cannot say the field is fat. Fall back to the
            // initializer's own dataflow: a frame-local view buried behind a type
            // parameter is caught, while a param or heap init is still accepted.
            Ty::Unknown => self.value_escape(init),
            _ => (false, false),
        }
    }

    /// The variant name a call constructs, if the callee is a bare or
    /// enum-qualified constructor, else None. A nullary variant carries no
    /// payload and is never a store or view origin, so it is not named here.
    fn variant_name<'a>(&self, callee: &'a Expr) -> Option<&'a str> {
        match &callee.kind {
            ExprKind::Ident(v) if self.variant_payloads.contains_key(v) => Some(v),
            ExprKind::Field(base, v)
                if matches!(&base.kind, ExprKind::Ident(en) if self.enums.contains_key(en))
                    && self.variant_payloads.contains_key(v) =>
            {
                Some(v)
            }
            _ => None,
        }
    }

    /// The enum that declares a variant of the given name, if any. Variant names
    /// are unique across the module, so at most one enum owns each. Used to reject
    /// an unqualified `Some(x)` constructor and to name the qualified form
    /// `Opt.Some(x)` in the fix. A nullary variant carries no payload entry, so
    /// this scans the full variant list, not `variant_payloads`.
    fn variant_owner(&self, vname: &str) -> Option<&str> {
        self.enums
            .iter()
            .find(|(_, vs)| vs.iter().any(|v| v == vname))
            .map(|(en, _)| en.as_str())
    }

    /// Checks a qualified enum constructor's argument count and payload types
    /// against the variant's declaration. A wrong arity (`Opt.Some()`,
    /// `Opt.Some(1, 2)`) or a mistyped scalar payload (`Opt.Some(true)` where the
    /// payload is int64) is rejected at the constructor site rather than slipping
    /// through as the Unknown the constructor otherwise infers as and an
    /// annotation would paper over. An interface or slice-of-interface payload is
    /// left to the conformance and covariance guards, which allow a boxed concrete
    /// value; a generic payload lowers to Unknown here and wildcards against any
    /// argument. Runs in both passes: a generic enum's payload reads as Unknown at
    /// the surface and wildcards, so a mistyped payload buried behind the element
    /// parameter is caught only once mono grounds it. The ground pass keys by the
    /// mangled enum name (`Opt$int32`), which is unique per instantiation, so a
    /// variant name shared across instantiations still checks against the right
    /// declaration. The display name is demangled so the message reads `Opt.Some`.
    fn check_variant_ctor_args(
        &mut self,
        en: &str,
        v: &str,
        args: &[Expr],
        arg_tys: &[Ty],
        span: Span,
    ) {
        // Only a real variant of this enum is a constructor; a mistyped member
        // name is not one and is not shaped here.
        if !self
            .enums
            .get(en)
            .is_some_and(|vs| vs.iter().any(|x| x == v))
        {
            return;
        }
        // Keyed by the enum this constructor names, so a variant name two enums
        // share is checked against the right declaration, not the by-name map's
        // last writer. Absent means a nullary variant of this enum.
        let payloads = self
            .variant_payloads_by_enum
            .get(&(en.to_string(), v.to_string()))
            .cloned()
            .unwrap_or_default();
        // Strip the `$...` monomorphization suffix so a ground instantiation names
        // its source enum in the message, not the mangled internal name.
        let disp = en.split('$').next().unwrap_or(en);
        if payloads.len() != args.len() {
            self.err(
                format!(
                    "'{disp}.{v}' takes {} argument(s), but {} were given",
                    payloads.len(),
                    args.len()
                ),
                span,
            );
            return;
        }
        for (i, (p, a)) in payloads.iter().zip(arg_tys).enumerate() {
            // An interface payload boxes a concrete implementer, and a slice-of-
            // interface payload is governed by the covariance guard, so neither is
            // a plain type match; skip both and let those checks own the case.
            if self.payload_iface_exempt(p) {
                continue;
            }
            // A literal payload ranges against the variant's declared width the
            // same way an annotated binding does, so `E.Has(4294967297)` at an
            // int32 payload is rejected here rather than silently truncating in
            // codegen. Reaches the generic-pin case too: once mono grounds a
            // `Opt<int32>.Some(lit)`, the second type pass runs this with the
            // pinned width.
            if let Some(arg) = args.get(i) {
                self.check_int_fits(arg, p);
            }
            if !compatible(p, a) {
                let s = args.get(i).map(|x| x.span).unwrap_or(span);
                self.err(
                    format!("argument {} to '{disp}.{v}' has the wrong type", i + 1),
                    s,
                );
            }
        }
    }

    /// Whether a variant payload type is an interface or a slice of one, so the
    /// plain type match is skipped in favor of the conformance and covariance
    /// guards that allow a boxed concrete value.
    fn payload_iface_exempt(&self, p: &Ty) -> bool {
        match p {
            Ty::Named(n) => self.ifaces.contains(n),
            Ty::Slice(e) => matches!(&**e, Ty::Named(n) if self.ifaces.contains(n)),
            _ => false,
        }
    }

    /// The escape relation of a call's target: the known summary of a top-level
    /// function or a builtin, or the conservative TOP of an opaque callee (a
    /// closure value, a function-typed parameter, a method, or a foreign symbol).
    /// A local of function type shadows any module function of the same name, so
    /// its call is opaque, not the shadowed summary.
    fn callee_esc(&self, callee: &Expr) -> CalleeEsc {
        if let ExprKind::Ident(name) = &callee.kind {
            if !self.is_local(name) {
                if let Some(s) = self.summaries.get(name) {
                    return CalleeEsc::Known(s.clone());
                }
                if let Some(s) = builtin_summary(name) {
                    return CalleeEsc::Known(s);
                }
            } else if let Some(g) = self.resolvable_fn_binds.get(name) {
                // A local proven bound once to one module function resolves to
                // that function's relation, not the opaque TOP, so its flow path
                // matches its sink path and no spurious cross-flow is invented on
                // top of the precise sink diagnostic.
                if let Some(s) = self.summaries.get(g) {
                    return CalleeEsc::Known(s.clone());
                }
            }
        }
        CalleeEsc::Top
    }

    /// The argument indices whose frame-view kind feeds a call's result: the
    /// parameters the callee returns (`returns_alias`) and those it reads a view
    /// back through (`reads_through`, a pointer passthrough or a heap read-back).
    /// Both apply the same way at the call site, checking `value_escape` of the
    /// caller's argument, so a polluted pointer argument taints the result exactly
    /// as a frame-view argument does. An opaque callee may expose any argument.
    fn summary_alias_indices(&self, callee: &Expr, args_len: usize) -> Vec<usize> {
        match self.callee_esc(callee) {
            CalleeEsc::Known(sum) => {
                let mut v: Vec<usize> = sum
                    .returns_alias
                    .to_vec()
                    .iter()
                    .map(|&i| i as usize)
                    .collect();
                for j in sum.reads_through.to_vec() {
                    v.push(j as usize);
                }
                v.sort_unstable();
                v.dedup();
                v
            }
            CalleeEsc::Top => (0..args_len.min(64)).collect(),
        }
    }

    /// The frame-view kind of a plain call's result: it views a frame when the
    /// callee returns one of its arguments (per its summary), reads a view back
    /// through a pointer argument, and that argument is itself a frame-local view
    /// or a polluted pointer. An opaque callee may return any argument, so every
    /// frame-view argument feeds the result. The higher-order builtins are
    /// element-aware: `map`/`filter` leak the collection's frame views only when
    /// the mapping function hands its element back, and `fold`/`reduce` leak the
    /// accumulator's when the folding function returns it.
    fn call_result_escape(&self, callee: &Expr, args: &[Expr]) -> (bool, bool) {
        if self.types_only {
            return (false, false);
        }
        if let Some((s, c, _)) = self.higher_order_taint(callee, args) {
            return (s, c);
        }
        let mut slice = false;
        let mut closure = false;
        for i in self.summary_alias_indices(callee, args.len()) {
            if let Some(a) = args.get(i) {
                let (s, c) = self.value_escape(a);
                slice |= s;
                closure |= c;
            }
        }
        (slice, closure)
    }

    /// If `e` is a plain call whose result views a frame through a returned or
    /// read-back argument, the call span and the 0-based index of the first such
    /// argument, for the interprocedural return diagnostic. None for a
    /// constructor or a call that returns no frame view.
    fn call_frame_origin(&self, e: &Expr) -> Option<(Span, usize)> {
        let ExprKind::Call(callee, args) = &e.kind else {
            return None;
        };
        if self.variant_name(callee).is_some() {
            return None;
        }
        if let Some((s, c, idx)) = self.higher_order_taint(callee, args) {
            return if s || c { Some((e.span, idx)) } else { None };
        }
        for i in self.summary_alias_indices(callee, args.len()) {
            if let Some(a) = args.get(i) {
                let (s, c) = self.value_escape(a);
                if s || c {
                    return Some((e.span, i));
                }
            }
        }
        None
    }

    /// The frame-view taint of a higher-order builtin result, with the argument
    /// index the diagnostic should name, or None when the callee is not one (or is
    /// shadowed by a local of the same name). Each builtin is a set-side model:
    /// `map` leaks the collection when its function's result may alias the element
    /// parameter; `filter`'s result is a subset of the collection's elements no
    /// matter what the predicate does, so a view-carrying element type leaks the
    /// collection outright; `fold` and `reduce` leak the seed through the
    /// accumulator parameter and the collection through the element parameter. A
    /// function that mints a fresh value, or a scalar element type, stays clean,
    /// so a map over a frame-local array of scalars (the common case) is not
    /// over-rejected.
    fn higher_order_taint(&self, callee: &Expr, args: &[Expr]) -> Option<(bool, bool, usize)> {
        let ExprKind::Ident(name) = &callee.kind else {
            return None;
        };
        // A local of the same name, or a top-level function that shadows the
        // builtin, is not the higher-order builtin, so defer to its own summary.
        if self.is_local(name) || self.summaries.contains_key(name) {
            return None;
        }
        match name.as_str() {
            "map" if args.len() == 2 => {
                let (s, c) = if self.mapper_aliases(&args[1], 0) {
                    self.value_escape(&args[0])
                } else {
                    (false, false)
                };
                Some((s, c, 0))
            }
            "filter" if args.len() == 2 => {
                let elem = elem_of(&self.chain_ty(&args[0]));
                let (s, c) = if self.member_carries_view(&elem) || matches!(elem, Ty::Unknown) {
                    self.value_escape(&args[0])
                } else {
                    (false, false)
                };
                Some((s, c, 0))
            }
            "fold" if args.len() == 3 => {
                if self.mapper_aliases(&args[2], 0) {
                    let (s, c) = self.value_escape(&args[1]);
                    if s || c {
                        return Some((s, c, 1));
                    }
                }
                let (s, c) = if self.mapper_aliases(&args[2], 1) {
                    self.value_escape(&args[0])
                } else {
                    (false, false)
                };
                Some((s, c, 0))
            }
            "reduce" if args.len() == 2 => {
                // reduce's seed is the collection's first element, so both the
                // accumulator and the element parameter lead back to argument 0.
                let (s, c) = if self.mapper_aliases(&args[1], 0) || self.mapper_aliases(&args[1], 1)
                {
                    self.value_escape(&args[0])
                } else {
                    (false, false)
                };
                Some((s, c, 0))
            }
            _ => None,
        }
    }

    /// Whether a higher-order builtin's function may hand parameter `i` (or its
    /// pointee) back through its result. A lambda literal is answered by the
    /// summary module's abstract walk of its body, recorded per span, so an
    /// alias chain, a passthrough call, or a tuple wrap counts the same as a
    /// bare `return x`; a named module function is answered by its computed
    /// summary; an opaque function value by its declared result type. A missing
    /// table entry stays conservative.
    fn mapper_aliases(&self, f: &Expr, i: u8) -> bool {
        match &f.kind {
            ExprKind::Lambda(_) => self
                .lambda_returns
                .get(&f.span)
                .map(|ps| ps.contains(i))
                .unwrap_or(true),
            ExprKind::Ident(n) if !self.is_local(n) => match self.summaries.get(n) {
                Some(sum) => sum.returns_alias.contains(i) || sum.reads_through.contains(i),
                None => matches!(self.chain_ty(f), Ty::Func(_, r) if self.member_carries_view(&r)),
            },
            _ => matches!(self.chain_ty(f), Ty::Func(_, r) if self.member_carries_view(&r)),
        }
    }

    /// The store-edge provenance of a returned local, if a call stored a frame
    /// view into it. Only a bare name carries the record; a projection off it is
    /// caught by its own root instead.
    fn returned_flow_prov(&self, e: &Expr) -> Option<(Span, u8, u8)> {
        match &e.kind {
            ExprKind::Ident(n) => self.flow_prov.get(n).copied(),
            _ => None,
        }
    }

    /// The store-edge provenance behind a returned pointer value: the record on
    /// a bare name, or on the root binding of a projection that carries the
    /// flagged pointer out, so the diagnostic names the store that polluted it.
    fn value_flow_prov(&self, e: &Expr) -> Option<(Span, u8, u8)> {
        match &e.kind {
            ExprKind::Ident(n) => self.flow_prov.get(n).copied(),
            ExprKind::Field(..) | ExprKind::Index(..) => {
                let root = self.projection_root(e).or_else(|| deref_root(e))?;
                self.flow_prov.get(&root).copied()
            }
            _ => None,
        }
    }

    /// The store-edge provenance of a struct literal's field, if one wraps a
    /// pointer or slice binding a call polluted with a frame view. A struct that
    /// carries such a field walks the dangling view out, so the field's provenance
    /// names the store precisely rather than falling to the generic message.
    fn struct_lit_flow_prov(&self, e: &Expr) -> Option<(Span, u8, u8)> {
        let ExprKind::StructLit(_, fields) = &e.kind else {
            return None;
        };
        for (_, init) in fields {
            if let ExprKind::Ident(n) = &init.kind {
                if let Some(pv) = self.flow_prov.get(n).copied() {
                    return Some(pv);
                }
            }
        }
        None
    }

    /// Applies a call's store edges to the caller. For each edge (i, j) where
    /// argument i is a frame-local view, the view flows into argument j's place:
    /// a place rooted at a parameter escapes this frame at once (a parameter
    /// outlives the call), and a place rooted at a local raises that local's
    /// escape flag so its own return check catches the laundered view. Runs on
    /// the surface pass only; the ground pass carries no summaries.
    fn apply_call_flows(&mut self, callee: &Expr, args: &[Expr], call_span: Span) {
        if self.types_only || self.variant_name(callee).is_some() {
            return;
        }
        let edges: Vec<(u8, u8)> = match self.callee_esc(callee) {
            CalleeEsc::Known(sum) => sum.flows_into.clone(),
            CalleeEsc::Top => top_flow_edges(args.len()),
        };
        self.apply_flow_edges(&edges, args, call_span);
    }

    /// The shared body of the call-flow application: route each store edge `(i, j)`
    /// from a frame-view argument `i` into argument `j`'s place. The named-call
    /// path (`apply_call_flows`) and the method-call path
    /// (`check_method_call_escape`, whose effective argument 0 is the receiver)
    /// both route through here, so a method that stores a frame view through `self`
    /// pollutes the receiver's binding exactly as a `stash(s, c)` helper does.
    fn apply_flow_edges(&mut self, edges: &[(u8, u8)], args: &[Expr], call_span: Span) {
        // The frame-view kind of every argument before any edge raises a flag, so
        // an edge reads the pre-call state.
        let src: Vec<(bool, bool)> = args.iter().map(|a| self.value_escape(a)).collect();
        for &(i, j) in edges {
            let (iu, ju) = (i as usize, j as usize);
            if iu >= src.len() || ju >= args.len() {
                continue;
            }
            let (fs, fc) = src[iu];
            if !fs && !fc {
                continue;
            }
            let Some(root) = store_root(&args[ju]) else {
                continue;
            };
            if self.cur_params.contains(&root) {
                self.emit_flow_escape(call_span, i, j);
            } else if let Some(pidx) = self.param_alias_of(&root) {
                // The destination is a pointer local that aliases a parameter, so
                // the store reaches the caller's object at once. `d := c` routes
                // the edge into a local that is never returned, so raising the
                // local would lose it; naming the borrowed parameter reports it.
                self.emit_escape(
                    format!(
                        "a frame view is stored through a pointer that borrows argument {} and may outlive this frame",
                        pidx + 1
                    ),
                    call_span,
                );
            } else {
                self.raise_esc(&root, fs, fc);
                self.flow_prov.insert(root, (call_span, i, j));
            }
        }
    }

    /// The parameter a pointer name aliases, if it is a parameter itself or a
    /// pointer local recorded as borrowing one. None for a fresh local allocation,
    /// whose polluted object stays frame-local unless the pointer is returned.
    fn param_alias_of(&self, name: &str) -> Option<usize> {
        if let Some(&i) = self.cur_param_index.get(name) {
            return Some(i);
        }
        for scope in self.ptr_param_borrows.iter().rev() {
            if let Some(&i) = scope.get(name) {
                return Some(i);
            }
        }
        None
    }

    /// Emits the frame-escape diagnostic for a returned slice: the interprocedural
    /// message when the value comes from a call that returns a frame argument, the
    /// store-edge message when a call stored a frame view into the returned local,
    /// else the intraprocedural message existing tests depend on.
    fn report_slice_escape(&mut self, e: &Expr) {
        if let Some((span, n)) = self.call_frame_origin(e) {
            self.emit_escape(
                format!(
                    "this call may return a view of argument {}, which views the current frame",
                    n + 1
                ),
                span,
            );
        } else if let Some((span, i, j)) = self.returned_flow_prov(e) {
            self.emit_flow_escape(span, i, j);
        } else if let Some((span, i, j)) = self.struct_lit_flow_prov(e) {
            self.emit_flow_escape(span, i, j);
        } else {
            self.err(
                "a slice into a local array escapes its frame; put the backing on the heap",
                e.span,
            );
        }
    }

    /// The closure counterpart of `report_slice_escape`.
    fn report_closure_escape(&mut self, e: &Expr) {
        if let Some((span, n)) = self.call_frame_origin(e) {
            self.emit_escape(
                format!(
                    "this call may return a view of argument {}, which views the current frame",
                    n + 1
                ),
                span,
            );
        } else if let Some((span, i, j)) = self.returned_flow_prov(e) {
            self.emit_flow_escape(span, i, j);
        } else {
            self.err(
                "a closure that captures a local escapes its frame; it cannot be returned",
                e.span,
            );
        }
    }

    fn emit_flow_escape(&mut self, span: Span, i: u8, j: u8) {
        self.emit_escape(
            format!(
                "argument {}'s view is stored into argument {} and may outlive this frame",
                i + 1,
                j + 1
            ),
            span,
        );
    }

    /// The single choke point for the interprocedural escape diagnostics, so the
    /// two M5 messages are constructed and emitted in one place.
    fn emit_escape(&mut self, msg: String, span: Span) {
        self.err(msg, span);
    }

    /// Registers an error typed binding as pending until a handling site clears it.
    fn declare_err(&mut self, name: &str, span: Span) {
        if let Some(scope) = self.err_binds.last_mut() {
            scope.insert(name.to_string(), span);
        }
    }

    /// Clears an error binding's pending state, from innermost scope outward.
    fn mark_err_handled(&mut self, name: &str) {
        for scope in self.err_binds.iter_mut().rev() {
            if scope.remove(name).is_some() {
                return;
            }
        }
    }

    /// Marks every error binding an expression mentions as handled. Used for
    /// `return e` and `return (v, e)`, which hand the error to the caller.
    fn mark_errs_in(&mut self, e: &Expr) {
        let mut used = Vec::new();
        let mut bound = HashSet::new();
        collect_expr(e, &mut used, &mut bound);
        for name in used {
            self.mark_err_handled(&name);
        }
    }

    fn declare(&mut self, name: &str, ty: Ty) {
        if name.is_empty() {
            return;
        }
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }

    fn lookup(&self, name: &str) -> Ty {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.get(name) {
                return t.clone();
            }
        }
        if let Some((params, ret)) = self.sigs.get(name) {
            return Ty::Func(params.clone(), Box::new(ret.clone()));
        }
        Ty::Unknown
    }

    /// Whether a name is a local binding in some enclosing scope, so a global
    /// like an async function name it shadows is not consulted.
    fn is_local(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains_key(name))
    }

    /// A whole-value use of `self` where a managed pointer is required. `self` is
    /// the receiver value, of the concrete struct type the impl names; codegen
    /// passes the receiver by pointer and loads the value for a bare `self`, so
    /// `return self` against a `*T` return, or `chan_send(ch, self)` into a
    /// `Channel<*T>`, hands the struct where the fat pointer belongs and the
    /// backend faults on the type. Reject it here, naming the mismatch, rather
    /// than leaving a stray backend error. Returns true when it fired, so the
    /// caller suppresses the generic mismatch that would otherwise double it.
    fn self_value_in_ptr_position(&mut self, e: &Expr, expected: &Ty) -> bool {
        // Gate on the impl-method context, not the spelling: a function-local
        // binding named `self` is an ordinary value, and a mismatch on it earns
        // the plain message, not the misleading receiver-value one.
        let is_self = self.in_method && matches!(&e.kind, ExprKind::Ident(n) if n == "self");
        if is_self && matches!(expected, Ty::Ptr(_)) && matches!(self.lookup("self"), Ty::Named(_))
        {
            self.err(
                "cannot use 'self' where a pointer is required; 'self' is the receiver value, not a pointer to it",
                e.span,
            );
            return true;
        }
        false
    }

    /// The unfixed slice-of-interface type a binding was annotated with, if any,
    /// so an assignment into it can be checked for slice covariance.
    fn slice_iface_of(&self, name: &str) -> Option<Ty> {
        for scope in self.slice_iface_elem.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    /// Records the ownership state of a managed pointer binding in the current
    /// scope. Only `*T` bindings get an entry; raw and non pointer bindings do
    /// not participate in the single owner rules.
    fn declare_own(&mut self, name: &str, own: Own) {
        if let Some(scope) = self.owns.last_mut() {
            scope.insert(name.to_string(), own);
        }
    }

    fn own_of(&self, name: &str) -> Option<Own> {
        for scope in self.owns.iter().rev() {
            if let Some(o) = scope.get(name) {
                return Some(*o);
            }
        }
        None
    }

    fn set_moved(&mut self, name: &str) {
        for scope in self.owns.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), Own::Moved);
                return;
            }
        }
    }

    fn err(&mut self, msg: impl Into<String>, span: Span) {
        // A loop-body replay only raises escape flags to a fixpoint; the real pass
        // that follows emits each diagnostic once, so the replay stays silent.
        if self.suppress > 0 {
            return;
        }
        self.errors.push(Diagnostic::new(msg, span));
    }

    /// Replays a loop body until its escape flags stop rising, then walks it once
    /// for real. A loop-carried alias chain (`out = tmp; tmp = s`) closes only on a
    /// later iteration, so a single visit under-reports; the escape state is
    /// raise-only, so the replay climbs a finite lattice and converges. The replay
    /// is silent (see `err`) and the final pass emits diagnostics against the
    /// settled state, so nothing is doubled and nothing is missed.
    fn loop_body_fixpoint(&mut self, body: &Block) {
        let cap = 64usize;
        self.suppress += 1;
        let mut prev = self.esc_snapshot();
        for _ in 0..cap {
            self.branch_block(body);
            let now = self.esc_snapshot();
            if now == prev {
                break;
            }
            prev = now;
        }
        self.suppress -= 1;
        self.branch_block(body);
    }

    /// A comparable snapshot of the raise-only escape-flag stacks, so the loop
    /// fixpoint can detect convergence. Ownership and scope structure are restored
    /// by `block` on every pass, so only the escape flags need comparing.
    #[allow(clippy::type_complexity)]
    fn esc_snapshot(&self) -> (Vec<Vec<(String, bool)>>, Vec<Vec<(String, bool)>>) {
        let snap = |stack: &[HashMap<String, bool>]| -> Vec<Vec<(String, bool)>> {
            stack
                .iter()
                .map(|m| {
                    let mut v: Vec<(String, bool)> =
                        m.iter().map(|(k, &b)| (k.clone(), b)).collect();
                    v.sort();
                    v
                })
                .collect()
        };
        (snap(&self.esc_slices), snap(&self.esc_closures))
    }

    fn block(&mut self, b: &Block) {
        // Snapshot ownership so a move inside a conditional or loop branch does
        // not leak out to the straight line code after it. The static pass is
        // single block with no cross branch data flow; the runtime generation
        // check backstops a move that a branch actually took.
        let saved = self.owns.clone();
        self.push_scope();
        for s in &b.stmts {
            self.stmt(s);
        }
        self.pop_scope();
        self.owns = saved;
    }

    /// A block that hangs off a conditional or loop, tracked so a `defer` inside
    /// one can be rejected.
    fn branch_block(&mut self, b: &Block) {
        self.branch_depth += 1;
        self.block(b);
        self.branch_depth -= 1;
    }

    /// Enforces immutability through projections: an element or field store whose
    /// chain stays in value territory, arrays and struct fields, must root in a
    /// mutable binding. A store through a pointer, a slice, or a string writes a
    /// buffer the binding merely views, which the resolver's function scope rules
    /// govern instead. The bare `x = v` form is the resolver's; this pass adds
    /// the `xs[i] = v` and `s.f = v` forms it cannot see.
    fn check_assign_target(&mut self, lhs: &Expr) {
        if matches!(&lhs.kind, ExprKind::Ident(_)) {
            return;
        }
        // A field store on a pointer never reaches memory today; the language
        // requires an explicit dereference, so say so instead of dropping it.
        if let ExprKind::Field(base, _) = &lhs.kind {
            if matches!(self.chain_ty(base), Ty::Ptr(_) | Ty::RawPtr(_)) {
                self.err(
                    "a field store through a pointer needs an explicit dereference; write (*p).field",
                    lhs.span,
                );
                return;
            }
            // An error is a single message pointer with no writable place, so
            // `e.message = ...` has no store to lower; codegen would silently drop
            // it. Reading the message is fine, but the field is read only.
            if matches!(self.chain_ty(base), Ty::Error) {
                self.err(
                    "an error's message is read only; build a new error instead",
                    lhs.span,
                );
                return;
            }
        }
        // A string is an immutable byte view, so `s[0] = 'H'` has no writable
        // element; codegen would store into read-only memory and fault. Reject
        // the element store and name the builder as the fix. Reading `s[0]` is
        // untouched, since only an assignment target reaches here.
        if let ExprKind::Index(base, idx) = &lhs.kind {
            if !matches!(idx.kind, ExprKind::Range(..)) && matches!(self.chain_ty(base), Ty::Str) {
                self.err(
                    "a string is immutable; build a new one with a StringBuilder",
                    lhs.span,
                );
                return;
            }
        }
        if let Some((root, span)) = self.value_chain_root(lhs) {
            if root != "self" && !self.is_mutable(&root) {
                self.err(
                    format!("cannot assign through immutable '{root}'; declare it 'mut'"),
                    span,
                );
            }
        }
    }

    /// The root binding of an assignment chain that stays in value territory, or
    /// None once the chain crosses a pointer, slice, or string indirection.
    fn value_chain_root(&self, e: &Expr) -> Option<(String, Span)> {
        match &e.kind {
            ExprKind::Ident(n) => Some((n.clone(), e.span)),
            ExprKind::Field(base, _) => self.value_chain_root(base),
            ExprKind::Index(base, _) => match self.chain_ty(base) {
                Ty::Array(..) => self.value_chain_root(base),
                _ => None,
            },
            _ => None,
        }
    }

    /// A side effect free type walk for assignment and projection chains. Unlike
    /// `infer`, it never emits diagnostics, so walking the same expression twice
    /// is safe. A range index is a re-slice, so it keeps the slice shape over
    /// the base's element instead of projecting the element out, matching what
    /// `infer` computes for the same expression.
    fn chain_ty(&self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Ident(n) => self.lookup(n),
            ExprKind::Field(base, fname) => match self.chain_ty(base) {
                Ty::Named(s) => self
                    .structs
                    .get(&s)
                    .and_then(|(_, fs)| fs.iter().find(|(n, _)| n == fname).map(|(_, t)| t.clone()))
                    .unwrap_or(Ty::Unknown),
                // The one field an error carries; matches `infer`, so a projection
                // chain reads `e.message` as the string it is.
                Ty::Error if fname == "message" => Ty::Str,
                _ => Ty::Unknown,
            },
            ExprKind::Index(base, idx) => {
                let bt = self.chain_ty(base);
                if matches!(idx.kind, ExprKind::Range(..)) {
                    Ty::Slice(Box::new(elem_of(&bt)))
                } else {
                    elem_of(&bt)
                }
            }
            ExprKind::Unary(UnOp::Deref, p) => match self.chain_ty(p) {
                Ty::Ptr(inner) | Ty::RawPtr(inner) | Ty::Collector(inner) => *inner,
                _ => Ty::Unknown,
            },
            _ => Ty::Unknown,
        }
    }

    /// Whether a field, index, or dereference projection reads out a value that
    /// links its binding into the base's alias group. A concrete `*T` field
    /// answers yes directly; so does a projection whose type can reach a managed
    /// pointer through a struct, tuple, array, or enum layer (`y := outer.inner`,
    /// where `Inner` buries a `*Cell`), so a store through the projected
    /// aggregate's own pointer taints the root's group. A generic field of type
    /// `T` erases to `Unknown` (its width resolves in codegen), so `chain_ty`
    /// cannot tell a `Box<*Cell>.c` from a `Box<int>.c`; the maybe joins the
    /// group on the erased case too, the coarse over-approximate direction, safe
    /// for an escape reject. A scalar field, or an aggregate that reaches no
    /// managed pointer, answers no. The link alone rejects nothing, so a
    /// projection consumed only in frame stays accepted; only a frame view stored
    /// through the binding and a later return of a group member then trips.
    fn chain_projection_manages(&self, e: &Expr) -> bool {
        self.links_as_alias(&self.chain_ty(e))
    }

    /// Whether a member or projected value of this type links its binding into an
    /// alias group. A managed pointer links directly; any type that can reach a
    /// managed pointer through a struct field, enum payload, tuple element, or
    /// array element links too, and an erased generic member (`Unknown`, whose
    /// width resolves in codegen) reads as a maybe and links, so a frame view
    /// stored through the buried pointer and a later return of a group member is
    /// caught. This one predicate is the single rule the embed walk and the
    /// projection gate share: whenever a type can reach a managed pointer, chain
    /// it. A slice, array, or scalar that reaches no managed pointer does not
    /// link, so a frame-slice sibling (`Store { c: c, d: local[0..4] }`) never
    /// taints the embedded pointer; that escape rides the binding's own view flag,
    /// a separate path the alias mechanism must not double-count.
    fn links_as_alias(&self, t: &Ty) -> bool {
        is_managed(t) || self.ty_reaches_managed(t)
    }

    /// The `Ty`-domain twin of the summary walk's `reaches_managed_ptr`: whether a
    /// type can reach a managed pointer, however deeply a struct field, enum
    /// payload, tuple element, or array/slice element buries it. Recursion guards
    /// against a self-referential type by name. An `Unknown` reads as a maybe (a
    /// managed pointer may hide behind an erased generic), the coarse
    /// over-approximate direction; a `*raw T` is the honor-system FFI layer and
    /// never counts, matching `member_carries_view`; a closure and a bare scalar
    /// reach nothing.
    fn ty_reaches_managed(&self, t: &Ty) -> bool {
        let mut seen = HashSet::new();
        self.ty_reaches_managed_rec(t, &mut seen)
    }

    fn ty_reaches_managed_rec(&self, t: &Ty, seen: &mut HashSet<String>) -> bool {
        match t {
            Ty::Ptr(inner) => !matches!(**inner, Ty::Unit),
            Ty::RawPtr(_) => false,
            Ty::Unknown => true,
            Ty::Slice(e) | Ty::Array(e, _) => self.ty_reaches_managed_rec(e, seen),
            Ty::Tuple(ts) => ts.iter().any(|x| self.ty_reaches_managed_rec(x, seen)),
            Ty::Named(n) => {
                if !seen.insert(n.clone()) {
                    return false;
                }
                if let Some((_, fields)) = self.structs.get(n) {
                    return fields
                        .iter()
                        .any(|(_, ft)| self.ty_reaches_managed_rec(ft, seen));
                }
                if let Some(variants) = self.enums.get(n) {
                    return variants.iter().any(|v| {
                        self.variant_payloads.get(v).is_some_and(|ps| {
                            ps.iter().any(|pt| self.ty_reaches_managed_rec(pt, seen))
                        })
                    });
                }
                false
            }
            _ => false,
        }
    }

    /// Rejects an integer literal that cannot fit the width it is bound to, as in
    /// `x: int8 = 300`. Only a direct literal (or its negation) is checked; a
    /// computed value is the programmer's to range.
    fn check_int_fits(&mut self, e: &Expr, ty: &Ty) {
        let Ty::Int(w @ (8 | 16 | 32)) = ty else {
            return;
        };
        let (neg, v) = match &e.kind {
            ExprKind::Int(v, None) => (false, *v),
            ExprKind::Unary(UnOp::Neg, inner) => match &inner.kind {
                ExprKind::Int(v, None) => (true, *v),
                _ => return,
            },
            _ => return,
        };
        let val = if neg { -(v as i128) } else { v as i128 };
        // The signed widths are the only integer widths on the surface: the
        // unsigned names are reserved (see reject_reserved_uint), so a literal
        // ranges against the signed bounds for its width, `int8` accepting
        // -128..127 and rejecting 128, which would silently wrap to -128.
        let lo = -(1i128 << (w - 1));
        let hi = (1i128 << (w - 1)) - 1;
        if val < lo || val > hi {
            self.err(format!("literal {val} does not fit in {w} bits"), e.span);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(l) => self.let_stmt(l),
            Stmt::Assign(lhs, rhs) => {
                let lt = self.infer(lhs);
                let rt = self.infer(rhs);
                if !compatible(&lt, &rt) {
                    self.err("assignment type mismatch", lhs.span);
                }
                self.check_int_fits(rhs, &lt);
                self.check_assign_target(lhs);
                if is_managed(&lt) && !self.types_only {
                    // `q = p` aliases two owners exactly like the let form copy, so
                    // flag it the same. This takes priority, naming the precise
                    // mistake when both sides are owners.
                    let copy_of_owner = match (&lhs.kind, &rhs.kind) {
                        (ExprKind::Ident(_), ExprKind::Ident(src)) => {
                            matches!(self.own_of(src), Some(Own::Owner))
                        }
                        _ => false,
                    };
                    if copy_of_owner {
                        self.err(
                            "cannot copy an owning pointer; bind a `ref` alias or `move` it",
                            rhs.span,
                        );
                    } else if let ExprKind::Ident(dst) = &lhs.kind {
                        // Reassigning an owning pointer drops the allocation it
                        // holds without freeing it, a leak. A borrowing cursor, as
                        // in a list walk, is not an owner and may advance.
                        if matches!(self.own_of(dst), Some(Own::Owner)) {
                            self.err(
                                "cannot reassign an owning pointer; it leaks the allocation it holds, free it first or bind a new pointer",
                                lhs.span,
                            );
                        }
                    }
                }
                // Reassigning a bare binding updates its escape flag in the scope
                // that owns it, so a rebind inside a branch is not lost when the
                // branch scope pops. A projection or dereference store,
                // `s.rows = ...`, `xs[i] = ...`, `(*p).f = ...`, cannot rebind
                // the whole binding but can put a frame-local view into a place
                // the root reaches, so it raises the root binding's flag through
                // the unified store root (which follows every index and
                // dereference), never clears it. A place rooted at a parameter
                // or a borrow of one is skipped here: the summary walk's
                // frame-store table already rejected the store outright, and a
                // raised flag on a parameter would misclassify its later uses.
                if let ExprKind::Ident(dst) = &lhs.kind {
                    let (esc_s, esc_c) = self.value_escape(rhs);
                    self.assign_esc(dst, esc_s, esc_c);
                    // A reassign to a lambda literal re-records the new lambda's
                    // sink set, so a binding reversed to a sinking lambda is caught
                    // at the call and one reversed to a clean lambda stays precisely
                    // accepted, both keeping the name a known lambda. A reassign to
                    // any other value drops the record so the binding falls to the
                    // opaque conservative send-reject, since dusk can no longer see
                    // what the name now holds.
                    if let ExprKind::Lambda(_) = &rhs.kind {
                        let bind = self.lambda_sink_bind_of(rhs.span);
                        self.rebind_lambda_sink(dst, bind);
                        let cf = self
                            .lambda_capture_flows
                            .get(&rhs.span)
                            .cloned()
                            .unwrap_or_default();
                        self.rebind_lambda_capture(dst, cf);
                    } else {
                        self.drop_lambda_sink(dst);
                        self.drop_lambda_capture(dst);
                    }
                    // A reassign updates the binding's alias membership through
                    // the same choke every binding site funnels through, with the
                    // reassign join: `q = d` leaves q's old group and joins d's at
                    // straight line, unions d's group on top inside a branch (a
                    // may-join, since q may still hold its prior pointer), and a
                    // reassign to a value that hands no alias drops q's edges, so a
                    // stale alias never carries a raised flag onward. The aggregate
                    // form `outer = Outer { inner: inner }` reaches the pointers the
                    // literal embeds, the same as the let-embed does, so the Assign
                    // site no longer walks past a re-bound aggregate. Gated on
                    // whether either the place type or the value can reach a managed
                    // pointer: a field read infers to Unknown here (its width
                    // resolves in codegen), so `chain_ty` re-derives the projected
                    // type, and an aggregate place carries its embedded pointers
                    // through `links_as_alias`.
                    if self.links_as_alias(&lt) || self.links_as_alias(&self.chain_ty(rhs)) {
                        self.link_binding_aliases(dst, rhs, true);
                    }
                    // Assigning a slice of concrete structs to a slice-of-interface
                    // binding is the covariance error the fixed binding type hides.
                    if let Some(raw) = self.slice_iface_of(dst) {
                        self.check_slice_covariance(&raw, &rt, rhs, rhs.span);
                    }
                } else if let Some(root) = store_root(lhs) {
                    if self.param_alias_of(&root).is_none() {
                        let (esc_s, esc_c) = self.value_escape(rhs);
                        if esc_s || esc_c {
                            self.raise_esc(&root, esc_s, esc_c);
                        }
                    }
                }
            }
            Stmt::AssignOp(op, lhs, rhs) => {
                // The place type governs the operation. The result must be
                // compatible with it, and the mut rules are the plain assignment's.
                let lt = self.infer(lhs);
                let rt = self.infer(rhs);
                let result = self.check_binary(*op, &lt, &rt, lhs.span);
                match op {
                    BinOp::Shl | BinOp::Shr => self.check_shift_amount(rhs, &lt, lhs.span),
                    BinOp::Pow => self.check_pow_exponent(&lt, &rt, rhs, lhs.span),
                    _ => {}
                }
                if !compatible(&result, &lt) {
                    self.err("assignment type mismatch", lhs.span);
                }
                self.check_int_fits(rhs, &lt);
                self.check_assign_target(lhs);
            }
            Stmt::Return(Some(e)) => {
                // `return await f` returns the awaited element; its type must
                // match the async func's declared return, and the operand's
                // error bindings are handed to the caller like any returned tuple.
                if let ExprKind::Await(op, _) = &e.kind {
                    let el = self.infer_await(op, e.span);
                    let ret = self.cur_ret.clone();
                    if !compatible(&ret, &el) {
                        self.err(
                            "return type does not match the function's return type",
                            e.span,
                        );
                    }
                    self.mark_errs_in(op);
                    return;
                }
                let t = self.infer(e);
                let ret = self.cur_ret.clone();
                // Returning a concrete struct where an interface is declared is
                // the boxing site; it needs an impl, checked precisely here, and
                // the plain mismatch error would misfire on the valid case.
                let iface_ret =
                    matches!((&ret, &t), (Ty::Named(i), Ty::Named(_)) if self.ifaces.contains(i));
                if iface_ret {
                    self.check_conformance(&ret, &t, e.span);
                } else if self.tuple_iface_mismatch(&ret, &t, e.span) {
                    // A precise interface-in-tuple error already fired; the generic
                    // mismatch would otherwise double it, since the unfixed return
                    // tuple never `compatible`s against the concrete member.
                } else if self.self_value_in_ptr_position(e, &ret) {
                    // A `return self` against a `*T` return already earned the
                    // precise self-value message; the generic mismatch would double
                    // it.
                } else if !compatible(&ret, &t) {
                    self.err(
                        "return type does not match the function's return type",
                        e.span,
                    );
                }
                self.check_int_fits(e, &ret);
                self.check_escape(e, &t);
                // A returned error is the caller's to handle.
                self.mark_errs_in(e);
            }
            Stmt::Return(None) => {
                if !compatible(&self.cur_ret.clone(), &Ty::Unit) {
                    self.err("missing return value", Span::new(0, 0));
                }
            }
            Stmt::Defer(e) => {
                if self.branch_depth > 0 {
                    self.err(
                        "a defer inside a conditional or loop is not supported; register it at the top level of the function",
                        e.span,
                    );
                }
                self.infer(e);
            }
            Stmt::If(i) => {
                let c = self.infer(&i.cond);
                if !compatible(&Ty::Bool, &c) {
                    self.err("if condition must be a bool", i.cond.span);
                }
                self.branch_block(&i.then);
                if let Some(els) = &i.els {
                    self.branch_block(els);
                }
            }
            Stmt::While(w) => {
                let c = self.infer(&w.cond);
                if !compatible(&Ty::Bool, &c) {
                    self.err("while condition must be a bool", w.cond.span);
                }
                self.loop_body_fixpoint(&w.body);
            }
            Stmt::For(f) => {
                let it_ty = self.infer(&f.iter);
                self.push_scope();
                self.declare(&f.var, Ty::Unknown);
                // The loop variable views an element of the iterand. When the
                // iterand is a frame-tainted view and its element type can itself
                // carry a view, each element views the frame too, so returning the
                // loop variable dangles once the frame is reclaimed. A scalar
                // element is a value copy and never escapes.
                if self.member_carries_view(&elem_of(&it_ty)) {
                    let (esc_s, esc_c) = self.value_escape(&f.iter);
                    self.set_esc(&f.var, esc_s, esc_c);
                }
                // The loop variable also aliases the iterand's group when the
                // element type can reach a managed pointer: `for p in arr[0..1]`
                // where `arr` embeds a `*Cell` links `p` to `arr` (to `c`), so a
                // frame view stored through `(*p).rows` taints `c` and a later
                // return of it is caught. Routed through the same binding-alias
                // choke a `p := <iter>[i]` element read uses, gated on the element
                // type so a scalar loop variable links nothing.
                if self.links_as_alias(&elem_of(&it_ty)) {
                    self.link_binding_aliases(&f.var, &f.iter, false);
                }
                self.loop_body_fixpoint(&f.body);
                self.pop_scope();
            }
            Stmt::Match(m) => self.walk_match(m),
            Stmt::Expr(e) => {
                // The void await form discards its value; it is legal only when
                // the awaited element is void, else the value must be bound.
                if let ExprKind::Await(op, _) = &e.kind {
                    let el = self.infer_await(op, e.span);
                    if !compatible(&Ty::Unit, &el) {
                        self.err(
                            "'await f' discards a value; bind it, as in v, e := await f",
                            e.span,
                        );
                    }
                    return;
                }
                // A bare call of an async func mints a future that is dropped
                // before it can be awaited or released.
                if let ExprKind::Call(callee, _) = &e.kind {
                    if let ExprKind::Ident(g) = &callee.kind {
                        if self.async_fns.contains_key(g) && !self.is_local(g) {
                            self.err(
                                format!("the future from '{g}' is never awaited; bind it so it can be awaited or released"),
                                e.span,
                            );
                        }
                    }
                }
                let t = self.infer(e);
                // A fallible expression used as a bare statement drops its error.
                // The spec requires every error to be handled, so bind the result
                // and handle the error with exists, check, or ignore. A blessed
                // handler returns bool or unit, so it is not flagged here.
                if ty_has_error(&t) {
                    self.err(
                        "this expression's error result is ignored; bind it and handle the error with exists, check, or ignore",
                        e.span,
                    );
                }
            }
        }
    }

    fn let_stmt(&mut self, l: &Let) {
        // An awaited value binds by the await result shape rule, not the ordinary
        // value path, so intercept it before inference reaches the backstop.
        if let ExprKind::Await(op, _) = &l.value.kind {
            self.let_await(l, op);
            return;
        }
        // `p: *T = alloc()` is the uninitialized allocation; its size comes from
        // the annotation, so the form is only valid with a pointer annotation on
        // a single binding. Intercepted before infer, whose alloc arm rejects
        // every other zero argument alloc site.
        if is_zero_arg_alloc(&l.value, &self.sigs) && l.binds.len() == 1 {
            let b = &l.binds[0];
            let ty = match &b.ty {
                Some(t) => {
                    let lt = self.lower(t);
                    if !matches!(lt, Ty::Ptr(_)) {
                        self.err(
                            "alloc() with no value takes its size from a pointer annotation; write p: *T = alloc()",
                            l.value.span,
                        );
                    }
                    lt
                }
                None => {
                    self.err(
                        "alloc() with no value needs a pointer type annotation to size the allocation; write p: *T = alloc()",
                        l.value.span,
                    );
                    Ty::Unknown
                }
            };
            self.declare(&b.name, ty.clone());
            if l.mutable {
                self.declare_mut(&b.name);
            }
            if is_managed(&ty) {
                self.declare_own(&b.name, Own::Owner);
            }
            return;
        }
        let vt = self.infer(&l.value);
        // A collected value is not owned, so a `ref` alias of one is meaningless:
        // copy it directly. Fires on both passes so a `collector<T>` laundered
        // through a generic is still caught once mono makes it concrete.
        if l.is_ref && matches!(vt, Ty::Collector(_)) {
            self.err(
                "a collected value is not borrowed with ref; copy it directly",
                l.value.span,
            );
        }
        if l.binds.len() == 1 {
            // A fallible function returns a `(value, error)` tuple, and the error
            // carries the must-handle obligation only when it is bound to its own
            // name. Binding the whole tuple to one name buries the error inside a
            // value that is never an `error`, so the obligation is silently lost:
            // `p := fail()` then handles nothing. Require the destructure that
            // binds the value and the error separately. Narrowed to a direct call
            // result: a tuple literal the program builds by hand keeps each error
            // member's own binding obligation, so it is not swept up here. Surface
            // pass only, like the rest of the must-handle rule.
            if !self.types_only
                && matches!(&l.value.kind, ExprKind::Call(callee, _) if self.variant_name(callee).is_none())
                && matches!(&vt, Ty::Tuple(ts) if ts.iter().any(|t| matches!(t, Ty::Error)))
            {
                self.err(
                    "a fallible result must be destructured; bind the value and the error, as in `v, e := f()`",
                    l.value.span,
                );
            }
            let b = &l.binds[0];
            let ty = match &b.ty {
                Some(t) => {
                    let lt = self.lower(t);
                    // A generic instantiation over an interface, `b: Box<Speaker>`,
                    // is refused at the annotation before its request reaches the
                    // monomorphizer.
                    self.reject_iface_targ(t, l.value.span);
                    self.reject_reserved_uint(t, l.value.span);
                    // The annotation with the interface name intact, so binding a
                    // struct to an interface checks its impl instead of emitting
                    // a reference to a vtable that does not exist.
                    let raw = lower(t, &self.cur_generics);
                    self.check_conformance(&raw, &vt, l.value.span);
                    self.check_covariance_deep(&raw, &vt, &l.value, l.value.span);
                    if let Ty::Named(n) = &raw {
                        if self.ifaces.contains(n) {
                            self.declare_iface_bind(&b.name, n);
                        }
                    }
                    // Remember a slice-of-interface binding so a later assignment
                    // of a slice of concrete structs is caught as covariance.
                    if matches!(&raw, Ty::Slice(el) if matches!(&**el, Ty::Named(n) if self.ifaces.contains(n)))
                    {
                        if let Some(scope) = self.slice_iface_elem.last_mut() {
                            scope.insert(b.name.clone(), raw.clone());
                        }
                    }
                    if !compatible(&lt, &vt) {
                        self.err(
                            format!(
                                "'{}' has a type annotation that does not match its value",
                                b.name
                            ),
                            l.value.span,
                        );
                    }
                    self.check_int_fits(&l.value, &lt);
                    lt
                }
                // An unannotated binding of a bare literal takes the default
                // width, int64 or float64, so the binding is concretely typed
                // and cannot launder a width mismatch later.
                None => harden(vt.clone()),
            };
            self.declare(&b.name, ty.clone());
            // A binding whose initializer is a qualified enum constructor names an
            // enum value even though its checked type is Unknown. Record the enum
            // so a stray method call on the binding is rejected.
            if let ExprKind::Call(callee, _) = &l.value.kind {
                if let ExprKind::Field(cbase, _) = &callee.kind {
                    if let ExprKind::Ident(en) = &cbase.kind {
                        if self.enums.contains_key(en) && self.variant_name(callee).is_some() {
                            self.declare_enum_bind(&b.name, en);
                        }
                    }
                }
            }
            if l.mutable {
                self.declare_mut(&b.name);
            }
            if matches!(ty, Ty::Error) {
                self.declare_err(&b.name, l.value.span);
            }
            if is_managed(&ty) {
                let own = self.binding_own(l, &vt);
                self.declare_own(&b.name, own);
                // A pointer that reaches a parameter pointer's object, directly
                // or transitively, lets a frame view stored through it escape.
                // Record the aliased parameter's index for the store-edge check:
                // a plain copy or `ref` alias roots at its source name, and a
                // call result roots at any pointer argument the callee's summary
                // says the result aliases or reads through (`d := same(c)`,
                // `d := move(c)`), so a borrow laundered through a call routes
                // the same way the bare copy does.
                if let ExprKind::Ident(src) = &l.value.kind {
                    if let Some(idx) = self.param_alias_of(src) {
                        if let Some(scope) = self.ptr_param_borrows.last_mut() {
                            scope.insert(b.name.clone(), idx);
                        }
                    }
                }
                if let ExprKind::Call(callee, cargs) = &l.value.kind {
                    if self.variant_name(callee).is_none() {
                        for i in self.summary_alias_indices(callee, cargs.len()) {
                            if let Some(ExprKind::Ident(src)) = cargs.get(i).map(|a| &a.kind) {
                                // The callee hands this argument's pointer back
                                // through its result, so the result binding aliases
                                // the argument: a frame view later stored through the
                                // result taints the argument a return escapes
                                // (`d := same(c)`). Managed pointers only.
                                if is_managed(&self.lookup(src)) {
                                    self.alias_link(&b.name, src);
                                }
                                if let Some(idx) = self.param_alias_of(src) {
                                    if let Some(scope) = self.ptr_param_borrows.last_mut() {
                                        scope.insert(b.name.clone(), idx);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            // A tuple literal that packs a parameter-borrowing pointer keeps the
            // borrow on the tuple binding, so a later destructure hands it back
            // out and a frame view stored through the rebound pointer still
            // names the borrowed parameter.
            if let ExprKind::Tuple(members) = &l.value.kind {
                for mexpr in members {
                    if let ExprKind::Ident(src) = &mexpr.kind {
                        if let Some(idx) = self.param_alias_of(src) {
                            if let Some(scope) = self.ptr_param_borrows.last_mut() {
                                scope.insert(b.name.clone(), idx);
                            }
                            break;
                        }
                    }
                }
            }
            // Every intra-function alias the initializer forms is generated by
            // the single binding-alias choke: a `ref`/borrow chain or a whole
            // aggregate copied by name, a by-value projection that reads a managed
            // value out of an aggregate, and each bare pointer an aggregate
            // literal (or an `alloc` of one) embeds at any depth. A frame view
            // stored through any group member then taints the pointer a later
            // return escapes. The param-borrow and call-summary edges above are a
            // separate, interprocedural mechanism (the callee's summary, not the
            // initializer's value shape), so they stay outside the choke.
            self.link_binding_aliases(&b.name, &l.value, false);
            // Record whether the binding holds a frame-local slice or closure, so
            // returning the bare name, or an alias of it, is caught as an escape
            // even though the return expression is not the escaping literal.
            let (esc_s, esc_c) = self.value_escape(&l.value);
            self.set_esc(&b.name, esc_s, esc_c);
            // A binding of a lambda literal keeps that lambda's sink set, empty for
            // a clean lambda, so a later direct call of the name is checked exactly
            // against what the lambda does: a sinking lambda rejects a polluted
            // argument the same as a named relaying helper, a clean lambda accepts
            // even a polluted pointer, and either way the binding is a known lambda
            // the leaf-frame send check never treats as an opaque callee. Keyed by
            // the lambda expression's span, the summary walk's handle on the literal.
            if let ExprKind::Lambda(_) = &l.value.kind {
                let bind = self.lambda_sink_bind_of(l.value.span);
                if let Some(scope) = self.lambda_sink_binds.last_mut() {
                    scope.insert(b.name.clone(), bind);
                }
                // The lambda's capture-flow edges ride the binding the same way,
                // so a direct call of the name raises each captured binding's flag
                // when the matching argument is a frame view: the capture store the
                // argument-to-argument flow model cannot see through a closure.
                let cf = self
                    .lambda_capture_flows
                    .get(&l.value.span)
                    .cloned()
                    .unwrap_or_default();
                if let Some(scope) = self.lambda_capture_binds.last_mut() {
                    scope.insert(b.name.clone(), cf);
                }
            }
            self.record_mut_tuple(l, b, &ty);
            return;
        }
        let parts = match &vt {
            Ty::Tuple(ts) if ts.len() == l.binds.len() => ts.clone(),
            Ty::Unknown => vec![Ty::Unknown; l.binds.len()],
            _ => {
                self.err(
                    "destructuring binding expects a tuple of matching arity",
                    l.value.span,
                );
                vec![Ty::Unknown; l.binds.len()]
            }
        };
        for (b, pt) in l.binds.iter().zip(parts.iter()) {
            let ty = match &b.ty {
                Some(t) => {
                    self.reject_iface_targ(t, l.value.span);
                    self.reject_reserved_uint(t, l.value.span);
                    self.lower(t)
                }
                None => harden(pt.clone()),
            };
            // A destructured managed pointer is conservatively an owner, so it is
            // tracked and freeing it is not wrongly rejected. The static copy
            // check for the destructure itself falls to the runtime backstop.
            if is_managed(&ty) {
                self.declare_own(&b.name, Own::Owner);
            }
            if l.mutable {
                self.declare_mut(&b.name);
            }
            if matches!(ty, Ty::Error) {
                self.declare_err(&b.name, l.value.span);
            }
            self.declare(&b.name, ty);
        }
        // Destructuring a tuple binds each name to a member. A tuple literal
        // exposes its members directly, so each binder inherits its own member's
        // escape. Any other tuple value, a call result, an alias, or a match, has
        // no per-position expression, so each view-carrying binder conservatively
        // inherits the whole value's frame-view flag; a scalar member cannot carry
        // a view and stays clean. The flag alone rejects nothing, so a destructure
        // consumed within the owning frame is still accepted; only a later return
        // or store of a flagged binder is caught.
        match &l.value.kind {
            ExprKind::Tuple(members) if members.len() == l.binds.len() => {
                for (b, m) in l.binds.iter().zip(members) {
                    let (esc_s, esc_c) = self.value_escape(m);
                    self.set_esc(&b.name, esc_s, esc_c);
                    // Each binder aliases the member it takes, through the single
                    // binding-alias choke: an Ident member (`a, n := (inner, 1)`
                    // where `inner` wraps a `*Cell` links `a` to `inner` and
                    // transitively to the pointer), a projection member, or a
                    // nested aggregate member all route here, so a frame view
                    // stored through `a.c` taints the buried pointer and a later
                    // return is caught. A slice or scalar member reaches nothing
                    // and links nothing.
                    self.link_binding_aliases(&b.name, m, false);
                    // A pointer member that aliases a parameter pointer keeps its
                    // borrow through the destructure, so a frame view stored
                    // through the binder still names the borrowed parameter.
                    if let ExprKind::Ident(src) = &m.kind {
                        if let Some(idx) = self.param_alias_of(src) {
                            if let Some(scope) = self.ptr_param_borrows.last_mut() {
                                scope.insert(b.name.clone(), idx);
                            }
                        }
                    }
                }
            }
            _ => {
                let (esc_s, esc_c) = self.value_escape(&l.value);
                if esc_s || esc_c {
                    for (b, pt) in l.binds.iter().zip(parts.iter()) {
                        if self.member_carries_view(pt) {
                            self.set_esc(&b.name, esc_s, esc_c);
                        }
                    }
                }
                // Destructuring a tuple binding that carries a parameter borrow
                // (a `t := (c, 1)` round-trip) hands the borrow to every managed
                // pointer binder, so a frame view stored through one still names
                // the borrowed parameter. The whole-tuple record cannot say
                // which member held the pointer, so each pointer binder inherits
                // it, conservatively.
                if let ExprKind::Ident(src) = &l.value.kind {
                    if let Some(idx) = self.param_alias_of(src) {
                        for (b, pt) in l.binds.iter().zip(parts.iter()) {
                            if is_managed(pt) {
                                if let Some(scope) = self.ptr_param_borrows.last_mut() {
                                    scope.insert(b.name.clone(), idx);
                                }
                            }
                        }
                    }
                }
                // Destructuring a non-literal tuple hands each view-reaching binder
                // the alias group of the whole tuple binding, through the same
                // binding-alias choke the literal arm uses: `t := (st, 7); a, n := t`
                // links `a` to `t` and transitively to the pointer `st` buries, so a
                // frame view stored through `a.c` taints that pointer and a later
                // return of it is caught. The whole-value form cannot name the member
                // each binder took, so every binder whose part type can reach a
                // managed pointer, not only a bare pointer binder but an aggregate
                // binder like a `Store` that buries one, conservatively joins the
                // whole group; a scalar member reaches nothing and links nothing. A
                // call or other rootless value hands no target here and links nothing.
                for (b, pt) in l.binds.iter().zip(parts.iter()) {
                    if self.links_as_alias(pt) {
                        self.link_binding_aliases(&b.name, &l.value, false);
                    }
                }
            }
        }
    }

    /// Records the storage type of the narrow mutable-tuple class, keyed by the
    /// binding's value span, so mono can stamp it onto `Bind.ty`. The class is a
    /// single unannotated mutable binding whose value is a tuple with at least one
    /// array-literal member: the checker infers such a member as a slice, since a
    /// later reassignment may store one, but the initializer alone shapes it as a
    /// fixed array, so an unannotated codegen slot would be too narrow for the fat
    /// slice a reassignment writes. Only fires on the surface pass, and only when
    /// the whole tuple converts to a spellable AST type, so a member the printer
    /// cannot name (a generic, an interface, a struct) leaves the binding untouched
    /// and its already-loud build failure in place rather than risking a wrong slot.
    fn record_mut_tuple(&mut self, l: &Let, b: &Bind, ty: &Ty) {
        if self.types_only || !l.mutable || b.ty.is_some() {
            return;
        }
        let ExprKind::Tuple(members) = &l.value.kind else {
            return;
        };
        if !members.iter().any(|m| matches!(m.kind, ExprKind::Array(_))) {
            return;
        }
        if let Some(at) = ty_to_ast(ty) {
            self.mut_tuple_types.insert(l.value.span, at);
        }
    }

    /// The ownership of a single managed pointer binding. A `ref` binding is a
    /// non owning alias; alloc, move, or a pointer returning call produce an
    /// owner; a plain copy of a borrow is a borrow; and a plain copy of an owner
    /// is the flagged single owner violation.
    fn binding_own(&mut self, l: &Let, vt: &Ty) -> Own {
        if l.is_ref {
            return Own::Borrow;
        }
        match &l.value.kind {
            // A call owns its managed `*T` result, since a pointer return moves
            // ownership out. A call that returns `*void` or a raw pointer, like
            // an arena bump, hands back a view into something else, not a fresh
            // owner, so the binding borrows.
            ExprKind::Call(..) => {
                if is_managed(vt) {
                    Own::Owner
                } else {
                    Own::Borrow
                }
            }
            ExprKind::Ident(src) => match self.own_of(src) {
                Some(Own::Owner) => {
                    if !self.types_only {
                        self.err(
                            "cannot copy an owning pointer; bind a `ref` alias or `move` it",
                            l.value.span,
                        );
                    }
                    Own::Owner
                }
                _ => Own::Borrow,
            },
            // A projection that detaches a managed pointer, a deref, field, or
            // index, is treated as an owner so freeing it is not rejected; the
            // runtime generation check backstops any aliasing this does not see.
            _ => Own::Owner,
        }
    }

    /// Infers the element type of an awaited operand. The operand must be a
    /// future; its element is what the await yields. A non-future operand is
    /// rejected, and an Unknown operand stays permissive.
    fn infer_await(&mut self, op: &Expr, span: Span) -> Ty {
        match self.infer(op) {
            Ty::Future(el) => *el,
            Ty::Unknown => Ty::Unknown,
            // On the ground AST mono may leave a future as its mangled `Future$T`
            // named type where the operand's type was read from a table the
            // unmangle walk does not pass through, a struct field among them. The
            // future table names its element, so an await still yields the right
            // type instead of falling to the permissive backstop.
            Ty::Named(n) if self.future_elems.contains_key(&n) => {
                self.future_elems.get(&n).cloned().unwrap_or(Ty::Unknown)
            }
            _ => {
                // The surface pass already validated every await operand, so the
                // ground re-check stays permissive here rather than false-firing
                // on a shape the erasure or a mangle left it unable to name.
                if !self.types_only {
                    self.err("the operand of await is not a future", span);
                }
                Ty::Unknown
            }
        }
    }

    /// Types the bindings of `binds := await op` by the await result shape rule.
    /// A tuple element destructures member-wise, so the must-handle rule fires on
    /// an error member; a non-tuple element binds its value and, with a second
    /// name, the completer's error word; a single name discards that error word.
    fn let_await(&mut self, l: &Let, op: &Expr) {
        let el = self.infer_await(op, l.value.span);
        let n = l.binds.len();
        match &el {
            Ty::Tuple(ts) if n > 1 => {
                if ts.len() != n {
                    self.err(
                        format!(
                            "await destructures this future into {} values, but {n} names are bound",
                            ts.len()
                        ),
                        l.value.span,
                    );
                }
                for (b, pt) in l.binds.iter().zip(ts) {
                    self.declare_await_bind(l, b, pt.clone());
                }
                // Bind any surplus names to Unknown so later references resolve.
                for b in l.binds.iter().skip(ts.len()) {
                    self.declare(&b.name, Ty::Unknown);
                }
            }
            _ if n == 2 => {
                // element, then the completer's error word.
                self.declare_await_bind(l, &l.binds[0], el.clone());
                self.declare_await_bind(l, &l.binds[1], Ty::Error);
            }
            _ if n == 1 => {
                self.declare_await_bind(l, &l.binds[0], el.clone());
            }
            _ => {
                let values = match &el {
                    Ty::Tuple(ts) => ts.len(),
                    _ => 2,
                };
                self.err(
                    format!("await destructures this future into {values} values, but {n} names are bound"),
                    l.value.span,
                );
                for b in &l.binds {
                    self.declare(&b.name, Ty::Unknown);
                }
            }
        }
    }

    /// Declares one binding produced by an await, honoring a type annotation,
    /// hardening a bare width, and registering mutability, ownership, and a
    /// pending error the same way an ordinary let binding does.
    fn declare_await_bind(&mut self, l: &Let, b: &Bind, pt: Ty) {
        let ty = match &b.ty {
            Some(t) => self.lower(t),
            None => harden(pt),
        };
        if l.mutable {
            self.declare_mut(&b.name);
        }
        if is_managed(&ty) {
            self.declare_own(&b.name, Own::Owner);
        }
        if matches!(ty, Ty::Error) {
            self.declare_err(&b.name, l.value.span);
        }
        self.declare(&b.name, ty);
    }

    /// Rejects the clear cases of a value escaping its frame through a return: a
    /// slice that views a frame local fixed array, and a closure that captures a
    /// local. A managed pointer escape is covered by the generation check, not
    /// here, since dusk has no address of operator and so every pointer is heap.
    /// The walk is driven by the declared return shape so an escaping fat value
    /// buried in a returned tuple is caught, not only a whole-return fat value.
    fn check_escape(&mut self, e: &Expr, t: &Ty) {
        // The whole escape class is a surface-only concern: it ran at full
        // fidelity on the un-erased AST, and `Unknown` erasure never suppressed
        // it, so the ground re-check skips it to avoid a foreign re-fire.
        if self.types_only {
            return;
        }
        let declared = self.cur_ret.clone();
        self.escape_walk(e, &declared, t);
    }

    /// Walks a returned value against its declared type, rejecting a fat member
    /// whose backing lives on this frame. A tuple recurses per position when the
    /// return expression is a tuple literal, so `return ([1,2,3], 42)` and
    /// `return (add, 7)` are checked member by member.
    fn escape_walk(&mut self, e: &Expr, declared: &Ty, inferred: &Ty) {
        match declared {
            // A slice that views a frame local array dangles once the function
            // returns and the stack array is reclaimed. A heap backed slice, like
            // a map result, or a slice parameter, whose backing the caller owns,
            // is fine.
            Ty::Slice(_) if self.slice_escapes(e, inferred) => {
                self.report_slice_escape(e);
            }
            // A closure that captures a frame local keeps its environment on this
            // frame, so returning it, as a lambda literal or as a binding that
            // holds one, dangles. A closure with no captures is a plain function
            // pointer and may be returned.
            Ty::Func(..) if self.expr_is_local_closure(e) => {
                self.report_closure_escape(e);
            }
            // A managed pointer to a heap object polluted with a frame view: a
            // view stored through the pointer via a call lands in the caller-
            // visible object, so the returned pointer carries the dangling view
            // out. The generation check guards the allocation itself, not a view
            // laundered inside it, so it is caught here (M5). The check is the
            // general value walk, not a per-constructor list, so a bare name, a
            // call (`return move(c)`, `return same(c)`), a projection
            // (`return h.c`), a destructured binder, and a match payload all
            // classify the same way.
            Ty::Ptr(_) => {
                let (esc_s, esc_c) = self.value_escape(e);
                if esc_s || esc_c {
                    if let Some((span, i, j)) = self.value_flow_prov(e) {
                        self.emit_flow_escape(span, i, j);
                    } else {
                        let span = self.call_frame_origin(e).map(|(s, _)| s).unwrap_or(e.span);
                        self.emit_escape(
                            "this returns a pointer to an object that stores a view of the current frame".to_string(),
                            span,
                        );
                    }
                }
            }
            Ty::Tuple(ds) => {
                if let ExprKind::Tuple(es) = &e.kind {
                    // A tuple literal exposes its members; recurse per position.
                    if es.len() == ds.len() {
                        let members = match inferred {
                            Ty::Tuple(ts) if ts.len() == ds.len() => Some(ts),
                            _ => None,
                        };
                        for (i, (ei, di)) in es.iter().zip(ds).enumerate() {
                            let ii = members.and_then(|ts| ts.get(i)).unwrap_or(&Ty::Unknown);
                            self.escape_walk(ei, di, ii);
                        }
                    }
                } else {
                    // A tuple returned by a bare name, an alias, or a match carries
                    // no per-position expression, so the recorded escape flags of
                    // the returned value decide it. Codegen would still build the
                    // fat members on this frame, so an escaping one must be caught.
                    let (esc_s, esc_c) = self.value_escape(e);
                    if esc_s {
                        self.report_slice_escape(e);
                    } else if esc_c {
                        self.report_closure_escape(e);
                    }
                }
            }
            // A struct or enum returned by value carries its fat fields or payload.
            // A literal escapes when a field or payload initializer views a frame
            // local; a binding, alias, or match reflects the flags recorded on it.
            // Both are decided by value_escape, one shape over from the tuple case.
            Ty::Named(sname)
                if self.structs.contains_key(sname) || self.enums.contains_key(sname) =>
            {
                let (esc_s, esc_c) = self.value_escape(e);
                if esc_s {
                    self.report_slice_escape(e);
                } else if esc_c {
                    self.report_closure_escape(e);
                }
            }
            // A fixed array returned by value copies its elements, so it escapes
            // only when the element type can carry a view (the same carrier set
            // every other gate uses) and the elements view a frame local. A
            // scalar array is copied whole and never escapes. A literal recurses
            // per element; a binding or alias reflects its recorded flags.
            Ty::Array(elem, _) if self.member_carries_view(elem) => {
                if let ExprKind::Array(elems) = &e.kind {
                    for el in elems {
                        self.escape_walk(el, elem, &Ty::Unknown);
                    }
                } else {
                    let (esc_s, esc_c) = self.value_escape(e);
                    if esc_s {
                        self.report_slice_escape(e);
                    } else if esc_c {
                        self.report_closure_escape(e);
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether returning a slice-typed expression would dangle: its backing is a
    /// frame local array. The sliced base or the returned array literal must have
    /// array type, or the returned name is a binding recorded as holding a local
    /// array slice; a slice parameter or a heap backed slice does not escape.
    fn slice_escapes(&mut self, e: &Expr, inferred: &Ty) -> bool {
        match &e.kind {
            ExprKind::Index(base, idx) if matches!(idx.kind, ExprKind::Range(..)) => {
                // A local array base, or a binding already known to hold a
                // frame-local slice, since re-slicing a frame-local slice stays
                // frame-local and its unannotated array literal infers to a slice.
                // A call result base is re-sliced through its own escape: `id(
                // local[0..4])[0..2]` re-slices a laundered frame view.
                matches!(self.infer(base), Ty::Array(..))
                    || matches!(&base.kind, ExprKind::Ident(n) if self.is_esc_slice(n))
                    || self.value_escape(base).0
            }
            // An array literal materializes in this frame, so returning it as a
            // slice views a dead frame no matter what its type says.
            ExprKind::Array(_) => true,
            ExprKind::Ident(n) if self.is_esc_slice(n) => true,
            // A local-array return, or any other shape the leaf predicate catches:
            // a projection out of a local aggregate, or a match arm that projects
            // an escaping payload, all reach the bare slice return through here.
            _ => matches!(inferred, Ty::Array(..)) || self.value_escape(e).0,
        }
    }

    /// Whether a returned expression is a closure whose environment sits on this
    /// frame: a lambda literal that captures a local, a binding recorded as
    /// holding one, or a projection or match that yields such a closure.
    fn expr_is_local_closure(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Lambda(l) => self.lambda_captures_local(l),
            ExprKind::Ident(n) => self.is_esc_closure(n),
            _ => self.value_escape(e).1,
        }
    }

    /// The argument rule `spawn` and `submit` share: exactly one argument, a
    /// lambda literal, nullary and void, because only the literal site knows
    /// the environment layout that codegen heap copies for the task. Captures
    /// cross as heap copies, so a capture that may view this frame, a slice, a
    /// closure, or an interface value, even buried in a struct or enum field,
    /// is rejected. A captured managed pointer becomes a borrow inside the
    /// task, so the body can read through it but never free or move it, and a
    /// moved away pointer keeps its moved state so capturing it stays the
    /// error a plain lambda gets.
    fn check_task_lambda(&mut self, name: &str, args: &[Expr], span: Span) {
        if args.len() != 1 {
            self.err(
                format!("{name} takes one argument, a lambda literal of type () -> void"),
                span,
            );
            for a in args {
                self.infer(a);
            }
            return;
        }
        let ExprKind::Lambda(l) = &args[0].kind else {
            self.err(
                format!(
                    "{name} takes a lambda literal written at the call site; wrap a closure variable as lambda () -> void {{ g() }}"
                ),
                args[0].span,
            );
            self.infer(&args[0]);
            return;
        };
        if !l.params.is_empty() || !matches!(self.lower(&l.ret), Ty::Unit) {
            self.err(
                format!(
                    "a lambda passed to {name} takes no parameters and returns void; pass data by capture and results through a channel"
                ),
                args[0].span,
            );
        }
        let caps = self.spawn_lambda_captures(l);
        let mut borrowed = Vec::new();
        for c in &caps {
            let t = self.lookup(c);
            let mut seen = HashSet::new();
            let mut fseen = HashSet::new();
            let mut cseen = HashSet::new();
            let reaches_collector = self.ptr_reaches_collector(&t, &mut cseen);
            if self.ptr_reaches_future(&t, &mut fseen) {
                // A future is loop-thread property; a completer thread carries
                // the raw handle words, not the typed future. The reach walk
                // sees a future behind a pointer too, so a `*Future<T>` smuggled
                // into the frame names this same reason, not the slice rule.
                self.err(
                    format!(
                        "{name} cannot capture '{c}': a future belongs to the event loop thread"
                    ),
                    args[0].span,
                );
            } else if reaches_collector {
                // A collected value stays on the main thread; a spawned frame
                // lives on another thread, so the capture is refused with its own
                // reason rather than the frame-view one. The reach walk sees a
                // collector behind a pointer too, so a `*collector<T>` or a pointer
                // to a struct that holds one names this same reason.
                self.err(
                    format!("{name} cannot capture '{c}': a collected value stays on the main thread; it cannot cross to another thread"),
                    args[0].span,
                );
            } else if !self.spawn_capturable(&t, &mut seen) || self.is_iface_bind(c) {
                self.err(
                    format!(
                        "{name} cannot capture '{c}'; a slice, closure, or interface value may view the spawning frame, so move the data to the heap or send it through a channel"
                    ),
                    args[0].span,
                );
            } else if self.is_esc_slice(c) || self.is_esc_closure(c) {
                // The type says the capture crosses, but the flow says its value
                // holds (or its heap object stores) a view of this frame: the
                // task may outlive the frame and read it dead. The store edge
                // that polluted the binding names the site when one is recorded.
                if let Some((pspan, i, j)) = self.flow_prov.get(c).copied() {
                    self.err(
                        format!(
                            "{name} cannot capture '{c}': argument {}'s view was stored into argument {} here, leaving '{c}' viewing the spawning frame; move the backing to the heap",
                            i + 1,
                            j + 1
                        ),
                        pspan,
                    );
                } else {
                    self.err(
                        format!(
                            "{name} cannot capture '{c}': it holds a view of the spawning frame, which the task outlives; move the backing to the heap or send it through a channel"
                        ),
                        args[0].span,
                    );
                }
            }
            if is_managed(&t) && !matches!(self.own_of(c), Some(Own::Moved)) {
                borrowed.push(c.clone());
            }
        }
        self.infer_lambda(l, args[0].span, &borrowed);
    }

    /// The binding a channel send hands over, seen through a `move` wrapper and a
    /// projection chain: `chan_send(ch, move(c))`, `chan_send(ch, c)`, and
    /// `chan_send(ch, d)` after `d := c` all root at the pointer whose heap
    /// object a store edge may have polluted. None for a value with no binding
    /// root, a fresh literal or a heap constructor, which carries no frame view.
    fn send_value_root(&self, e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Call(callee, cargs)
                if cargs.len() == 1
                    && matches!(&callee.kind, ExprKind::Ident(n) if n == "move") =>
            {
                self.send_value_root(&cargs[0])
            }
            ExprKind::Ident(n) => Some(n.clone()),
            ExprKind::Field(..) | ExprKind::Index(..) => {
                self.projection_root(e).or_else(|| deref_root(e))
            }
            _ => None,
        }
    }

    /// A channel send crosses a thread boundary that outlives the sending frame,
    /// so a sent value whose binding holds, or whose heap object a store edge
    /// polluted with, a view of this frame would dangle in the receiver. The
    /// monomorphizer's element-type ban catches a slice, closure, or interface
    /// element at the minting site; this catches the value channel the type ban
    /// clears, a managed pointer whose object stores a frame view, the same flow
    /// a spawn or submit capture is refused for. The store edge that polluted the
    /// binding names the site when one is recorded, else the send site is blamed.
    fn check_chan_send_flow(&mut self, name: &str, args: &[Expr]) {
        if self.types_only || args.len() != 2 {
            return;
        }
        let Some(root) = self.send_value_root(&args[1]) else {
            return;
        };
        if !(self.is_esc_slice(&root) || self.is_esc_closure(&root)) {
            return;
        }
        if let Some((pspan, i, j)) = self.flow_prov.get(&root).copied() {
            self.err(
                format!(
                    "{name} cannot send '{root}': argument {}'s view was stored into argument {} here, leaving '{root}' viewing the sending frame; move the backing to the heap",
                    i + 1,
                    j + 1
                ),
                pspan,
            );
        } else {
            self.err(
                format!(
                    "{name} cannot send '{root}': it holds a view of the sending frame, which the receiver outlives; move the backing to the heap or send heap owned data"
                ),
                args[1].span,
            );
        }
    }

    /// The interprocedural counterpart of `check_chan_send_flow`: a call whose
    /// callee sinks one of its parameters into a channel (per the escape summary's
    /// `sinks`) hands that argument's value across a thread boundary the receiver
    /// outlives, so a polluted argument in a sink position dangles in the receiver
    /// exactly as a direct `chan_send` of it would. The leaf-site check catches
    /// the send in the frame that owns the value; this catches the send one or
    /// more hops away, through a `relay(ch, c)` helper the leaf check cannot see.
    /// The store edge that polluted the argument names the site when one is
    /// recorded, mirroring the leaf message; else the call site is blamed.
    fn check_call_sinks(&mut self, callee: &Expr, args: &[Expr], call_span: Span) {
        if self.types_only {
            return;
        }
        let ExprKind::Ident(cname) = &callee.kind else {
            return;
        };
        // The channel-send builtins carry a stdlib summary whose body sinks the
        // element through the foreign send intrinsic, so their `sinks` now name
        // the element parameter. The leaf-site `check_chan_send_flow` already owns
        // a direct send by name, on the identical polluted-argument condition, so
        // deferring to it here keeps the canonical message and avoids a double
        // diagnostic. Every hop away from the frame that owns the value is still
        // caught: a helper that calls `chan_send` sinks its own parameter, and a
        // call of that helper is a plain summarized call this check does fire on.
        if cname == "chan_send" || cname == "chan_try_send" {
            return;
        }
        // A local of function type is opaque, not the summarized module function
        // of the same name, so its call carries no known sink relation, unless it
        // is a local proven bound once to one module function: then its call is
        // that function's known relation, checked here so a polluted argument to
        // `f := relay; f(ch, c)` is refused exactly as a direct `relay(ch, c)`
        // would be, closing the resolvable-fn-bind leaf-frame send path. Any other
        // local (a function-typed parameter, a reassigned or tuple-sourced binding)
        // falls to the opaque conservative reject instead.
        let target = if self.is_local(cname) {
            match self.resolvable_fn_binds.get(cname) {
                Some(g) => g.clone(),
                None => return,
            }
        } else {
            cname.clone()
        };
        let Some(sum) = self.summaries.get(&target).cloned() else {
            return;
        };
        self.check_summary_sinks(cname, &sum, args, call_span);
    }

    /// The shared body of the interprocedural sink check: for each sink parameter
    /// `j` of a callee's summary, reject a polluted argument in that position. The
    /// named-call path (`check_call_sinks`) and the method-call path
    /// (`check_method_call_escape`, whose effective argument 0 is the receiver)
    /// both route through here, so a self-sink method is caught on a polluted
    /// receiver with the identical message a `relay(ch, c)` helper earns.
    fn check_summary_sinks(
        &mut self,
        cname: &str,
        sum: &EscapeSummary,
        args: &[Expr],
        call_span: Span,
    ) {
        for j in sum.sinks.to_vec() {
            let Some(arg) = args.get(j as usize) else {
                continue;
            };
            let Some(root) = self.send_value_root(arg) else {
                continue;
            };
            if !(self.is_esc_slice(&root) || self.is_esc_closure(&root)) {
                continue;
            }
            let prov = self.flow_prov.get(&root).copied();
            self.emit_sink_reject(cname, &root, sum.collect_sinks.contains(j), prov, call_span);
        }
    }

    /// Emits the interprocedural sink reject for a polluted argument `root` handed
    /// to callee `cname` in a sink position. `collect` selects the wording: a
    /// `collector<T>` mint names the collect and the block the value outlives, a
    /// channel send names the send and the receiver, since either outliving sink
    /// dangles the frame view the same way. The store edge that polluted the
    /// argument names its site when one is recorded, else the call site is blamed.
    /// The escape decision is made before this; only the message differs.
    fn emit_sink_reject(
        &mut self,
        cname: &str,
        root: &str,
        collect: bool,
        prov: Option<(Span, u8, u8)>,
        call_span: Span,
    ) {
        let (msg, span) = match (collect, prov) {
            (false, Some((pspan, i, k))) => (
                format!(
                    "'{cname}' sends '{root}' across a channel, but argument {}'s view was stored into argument {} here, leaving '{root}' viewing the sending frame; move the backing to the heap",
                    i + 1,
                    k + 1
                ),
                pspan,
            ),
            (false, None) => (
                format!(
                    "'{cname}' sends '{root}' across a channel, but it holds a view of the sending frame, which the receiver outlives; move the backing to the heap or send heap owned data"
                ),
                call_span,
            ),
            (true, Some((pspan, i, k))) => (
                format!(
                    "'{cname}' collects '{root}', but argument {}'s view was stored into argument {} here, leaving '{root}' viewing the frame; move the backing to the heap",
                    i + 1,
                    k + 1
                ),
                pspan,
            ),
            (true, None) => (
                format!(
                    "'{cname}' collects '{root}', but it holds a view of the frame, which the collected value outlives; move the backing to the heap"
                ),
                call_span,
            ),
        };
        self.emit_escape(msg, span);
    }

    /// The concrete receiver type of a method call, peeling any managed or raw
    /// pointer off the base's type: `c.m()` on `c: *Cell` and `(*c).m()` both name
    /// `Cell`. None when the base is not a concrete named type (an interface value,
    /// a generic, or an inference hole), where no method summary is keyed and the
    /// call stays opaque to this check. Side-effect free: it walks the AST types
    /// through `chain_ty` and never re-runs inference.
    fn receiver_type_name(&self, base: &Expr) -> Option<String> {
        let mut t = self.chain_ty(base);
        loop {
            match t {
                Ty::Ptr(inner) | Ty::RawPtr(inner) => t = *inner,
                Ty::Named(n) => return Some(n),
                _ => return None,
            }
        }
    }

    /// The resolved method name and escape summary of a method call `base.m(..)`,
    /// when the base's type names a concrete type with a computed method summary.
    /// None for an error-method call, a struct-field callee, or an interface or
    /// generic receiver, none of which key a method summary. Both the escape check
    /// and the opaque-callee gate read this, so a method with a precise summary is
    /// checked once, precisely, rather than falling to the conservative opaque
    /// reject a field callee gets.
    fn resolved_method_summary(&self, callee: &Expr) -> Option<(String, EscapeSummary)> {
        let ExprKind::Field(base, mname) = &callee.kind else {
            return None;
        };
        let tname = self.receiver_type_name(base)?;
        let sum = self.method_summaries.get(&(tname, mname.clone()))?.clone();
        Some((mname.clone(), sum))
    }

    /// The interprocedural escape check for a method call. A method is a named
    /// function whose hidden first parameter is the by-pointer receiver, so the
    /// leaf and helper send checks that read a callee's summary miss it: the
    /// callee is a field expression, not a bare name, and the receiver never sits
    /// in the argument list. This threads the receiver as effective argument 0,
    /// looks up the method's summary (self as parameter 0), and applies the same
    /// sink and store-edge checks a named call gets, so `c.ship(ch)` with a
    /// polluted `c` whose `ship` sends `self` is rejected exactly as a direct
    /// `chan_send(ch, c)` is, and a method that stores a frame view through `self`
    /// pollutes the receiver's binding for its own later egress check. A method
    /// with a summary is not opaque, so the conservative opaque-callee reject
    /// steps aside (see `callee_is_opaque`) and this precise check owns the call.
    fn check_method_call_escape(&mut self, callee: &Expr, args: &[Expr], call_span: Span) {
        if self.types_only {
            return;
        }
        let ExprKind::Field(base, _) = &callee.kind else {
            return;
        };
        let Some((mname, sum)) = self.resolved_method_summary(callee) else {
            return;
        };
        // The receiver is the method's implicit parameter 0; the declared
        // arguments follow it. A method takes `self` by pointer, so the pointer
        // that becomes `self` is the base itself when the base is already a
        // pointer (`c.ship()`), or the dereferenced binding when the base
        // re-addresses one (`(*c).ship()` re-takes the address of `c`); both name
        // the same pointer whose object a store edge may have polluted. Every
        // summary index is stated against that frame, so the effective argument
        // list carries the receiver at 0 and the call's arguments at 1, 2, ....
        let recv = match &base.kind {
            ExprKind::Unary(UnOp::Deref, inner) => (**inner).clone(),
            _ => (**base).clone(),
        };
        let eff: Vec<Expr> = std::iter::once(recv).chain(args.iter().cloned()).collect();
        self.check_summary_sinks(&mname, &sum, &eff, call_span);
        self.apply_flow_edges(&sum.flows_into, &eff, call_span);
    }

    /// The recorded sink sets of a local bound to a sinking lambda, from the
    /// innermost scope that binds the name, so a shadow masks an outer binding.
    fn lambda_sink_bind(&self, name: &str) -> Option<LambdaSinkBind> {
        for scope in self.lambda_sink_binds.iter().rev() {
            if let Some(b) = scope.get(name) {
                return Some(*b);
            }
        }
        None
    }

    /// The sink sets a lambda literal at `span` carries, the full set and its
    /// collect subset, read together so the binding remembers which sink flavor to
    /// name at a later direct call. A clean lambda yields an empty pair, which
    /// still marks the binding a known lambda.
    fn lambda_sink_bind_of(&self, span: Span) -> LambdaSinkBind {
        LambdaSinkBind {
            sinks: self.lambda_sinks.get(&span).copied().unwrap_or_default(),
            collect: self
                .lambda_collect_sinks
                .get(&span)
                .copied()
                .unwrap_or_default(),
        }
    }

    /// The closure counterpart of `check_call_sinks`: a direct call of a local
    /// bound to a lambda whose body sinks one of its parameters into a channel
    /// hands that argument across a thread boundary the receiver outlives. A
    /// lambda carries no computed summary, so `check_call_sinks` skips it as a
    /// local; this reads the sink set recorded on the binding instead, rejecting
    /// a polluted argument in a sink position exactly as a direct `chan_send` of
    /// it, or a named `relay(ch, c)` helper call, would. The store edge that
    /// polluted the argument names the site when one is recorded, mirroring the
    /// leaf message; else the call site is blamed.
    fn check_lambda_call_sinks(&mut self, callee: &Expr, args: &[Expr], call_span: Span) {
        if self.types_only {
            return;
        }
        let ExprKind::Ident(cname) = &callee.kind else {
            return;
        };
        let Some(bind) = self.lambda_sink_bind(cname) else {
            return;
        };
        for j in bind.sinks.to_vec() {
            let Some(arg) = args.get(j as usize) else {
                continue;
            };
            let Some(root) = self.send_value_root(arg) else {
                continue;
            };
            if !(self.is_esc_slice(&root) || self.is_esc_closure(&root)) {
                continue;
            }
            let prov = self.flow_prov.get(&root).copied();
            self.emit_sink_reject(cname, &root, bind.collect.contains(j), prov, call_span);
        }
    }

    /// Records the sink set of a lambda literal reassigned to an existing name, in
    /// the scope that owns the binding. At straight line the new set replaces the
    /// old, so a binding reversed from a clean lambda to a sinking one (or the
    /// reverse) is checked against exactly what it now holds. Inside a branch the
    /// update is a may-join, the union of the two sink sets, since the binding may
    /// still be its prior lambda: a conditional reversal to a sinking lambda then
    /// sinks, and a conditional reversal to a clean one cannot prove the prior
    /// lambda gone. A name with no prior record starts one in the current scope.
    fn rebind_lambda_sink(&mut self, name: &str, bind: LambdaSinkBind) {
        let conditional = self.branch_depth > 0;
        for scope in self.lambda_sink_binds.iter_mut().rev() {
            if let Some(existing) = scope.get_mut(name) {
                *existing = if conditional {
                    LambdaSinkBind {
                        sinks: existing.sinks.union(bind.sinks),
                        collect: existing.collect.union(bind.collect),
                    }
                } else {
                    bind
                };
                return;
            }
        }
        if let Some(scope) = self.lambda_sink_binds.last_mut() {
            scope.insert(name.to_string(), bind);
        }
    }

    /// Drops a name's lambda-sink record, from the innermost scope that binds it,
    /// so a binding reassigned to a non-lambda value is no longer a known lambda
    /// and falls to the opaque conservative send-reject. The drop is
    /// unconditional even inside a branch: it can only widen the reject, which
    /// stays sound, and a binding that might now hold an opaque value cannot be
    /// proven clean.
    fn drop_lambda_sink(&mut self, name: &str) {
        for scope in self.lambda_sink_binds.iter_mut().rev() {
            if scope.remove(name).is_some() {
                break;
            }
        }
    }

    /// The recorded capture-flow edges of a local bound to a lambda, from the
    /// innermost scope that binds the name, so a shadow masks an outer binding.
    fn lambda_capture_bind(&self, name: &str) -> Option<Vec<(u8, String)>> {
        for scope in self.lambda_capture_binds.iter().rev() {
            if let Some(cf) = scope.get(name) {
                return Some(cf.clone());
            }
        }
        None
    }

    /// Records the capture-flow edges of a lambda literal reassigned to an
    /// existing name, mirroring `rebind_lambda_sink`. At straight line the new
    /// edges replace the old; inside a branch the update is a may-join (the union
    /// of both edge sets), since the binding may still be its prior lambda.
    fn rebind_lambda_capture(&mut self, name: &str, cf: Vec<(u8, String)>) {
        let conditional = self.branch_depth > 0;
        for scope in self.lambda_capture_binds.iter_mut().rev() {
            if let Some(existing) = scope.get_mut(name) {
                if conditional {
                    existing.extend(cf.iter().cloned());
                    existing.sort();
                    existing.dedup();
                } else {
                    *existing = cf;
                }
                return;
            }
        }
        if let Some(scope) = self.lambda_capture_binds.last_mut() {
            scope.insert(name.to_string(), cf);
        }
    }

    /// Drops a name's capture-flow record, mirroring `drop_lambda_sink`: a binding
    /// reassigned to a non-lambda value is no longer a known lambda, so its call
    /// falls to the opaque conservative store-reject instead.
    fn drop_lambda_capture(&mut self, name: &str) {
        for scope in self.lambda_capture_binds.iter_mut().rev() {
            if scope.remove(name).is_some() {
                break;
            }
        }
    }

    /// Applies the capture-flow edges of a direct call of a lambda-bound local: for
    /// each edge (i, B) where the lambda stores its parameter i's view through the
    /// captured binding B, a frame-view argument in position i is stored through B
    /// beyond this frame. When B is a parameter of the enclosing function, or a
    /// pointer that borrows one, the store lands in the caller's own object and
    /// escapes at once, so it is reported here. When B is a plain local, the store
    /// only raises B's escape flag: the view dies with this frame, so a purely local
    /// use stays legal and a later return, send, or spawn of B is what the existing
    /// egress checks reject, exactly as a named helper's store edge is handled.
    fn apply_lambda_capture_flows(&mut self, callee: &Expr, args: &[Expr], call_span: Span) {
        if self.types_only {
            return;
        }
        let ExprKind::Ident(cname) = &callee.kind else {
            return;
        };
        let Some(edges) = self.lambda_capture_bind(cname) else {
            return;
        };
        for (i, b) in edges {
            let Some(arg) = args.get(i as usize) else {
                continue;
            };
            let (fs, fc) = self.value_escape(arg);
            if !fs && !fc {
                continue;
            }
            if self.cur_params.contains(&b) || self.param_alias_of(&b).is_some() {
                self.emit_escape(
                    "this call stores a view of the current frame into a place that outlives it; put the backing on the heap".to_string(),
                    call_span,
                );
            } else {
                self.raise_esc(&b, fs, fc);
            }
        }
    }

    /// Whether a call's callee is opaque to the send analysis: dusk cannot see
    /// which arguments it may hand to a channel the receiver outlives. Four callee
    /// shapes are not opaque, each carrying a precise sink relation a dedicated
    /// check already reads: a summarized module function, a builtin, a local
    /// resolvably bound once to a module function, and a local bound to a lambda
    /// literal whose sink set is recorded. A variant constructor allocates and
    /// never sends, so it is not opaque either. Everything else is opaque: a field
    /// or method callee, a reassigned or `mut` binding, a tuple- or
    /// destructure-sourced binding, a call-result callee, or any other expression.
    /// Whether a call's callee is a synchronous error handler that invokes its
    /// function argument in place and never stores it: `e.check(h)` calls
    /// `h(self)` immediately and returns, and `e.ignore()` discharges the error
    /// with no argument at all. The receiver's type is `error`, so neither keys
    /// a method summary, and both would otherwise fall to the conservative
    /// opaque reject in `callee_is_opaque`, refusing a frame-capturing handler
    /// that outlives nothing. Their escape relation is empty in every dimension,
    /// so they are treated as not opaque and the frame-closure argument passes.
    fn callee_is_sync_handler(&self, callee: &Expr) -> bool {
        let ExprKind::Field(base, mname) = &callee.kind else {
            return false;
        };
        matches!(self.chain_ty(base), Ty::Error) && matches!(mname.as_str(), "check" | "ignore")
    }

    fn callee_is_opaque(&self, callee: &Expr) -> bool {
        if self.variant_name(callee).is_some() {
            return false;
        }
        // A method call whose receiver type names a concrete method summary is not
        // opaque: `check_method_call_escape` threads the receiver as argument 0 and
        // reads the method's precise sink and store-edge relations, which subsume
        // the conservative opaque reject (a sink argument is caught by the sink
        // check, a stored argument by the flow check, and an argument the method
        // neither sends nor stores is provably safe). Letting the opaque face fire
        // too would only double the diagnostic on the same store. A struct-field
        // lambda callee keys no method summary, so it stays opaque here.
        if self.resolved_method_summary(callee).is_some() {
            return false;
        }
        // A synchronous error handler (`e.check(h)`, `e.ignore()`) invokes its
        // function argument in the current frame and never stores it, so it is
        // not opaque: its escape summary is empty in every dimension (no sink,
        // no capture flow, no return). The receiver type is `error`, not a
        // struct, so it keys no method summary and would otherwise fall through
        // to the opaque reject below, refusing a frame-capturing handler that is
        // only ever invoked in place (the `e.check(lambda ...)` idiom). Stepping
        // aside here lets the precise checks pass a clean synchronous handler
        // while the closure face still fires on a genuinely opaque callee (a
        // struct-field, tuple, or reassigned lambda) that may stash it.
        if self.callee_is_sync_handler(callee) {
            return false;
        }
        match &callee.kind {
            ExprKind::Ident(name) => {
                if !self.is_local(name) {
                    // A module function carries a computed summary; a builtin a
                    // library one. Both name their sinks precisely.
                    return !(self.summaries.contains_key(name) || builtin_summary(name).is_some());
                }
                // A local proven bound to one module function, or bound to a lambda
                // literal, is a known callee the precise checks handle.
                !(self.resolvable_fn_binds.contains_key(name)
                    || self.lambda_sink_bind(name).is_some())
            }
            _ => true,
        }
    }

    /// The leaf-frame counterpart of the summary and lambda sink checks: a call
    /// whose callee is opaque may hand any argument to a channel the receiver
    /// outlives, or store it through one of its own captures, and dusk cannot see
    /// through it to know it does neither. Two argument shapes are refused
    /// conservatively. A managed pointer whose heap object a store edge polluted
    /// with a frame view: an opaque callee may send it across a channel, exactly as
    /// a direct `chan_send` of it, or a named `relay(ch, c)` helper call, would be,
    /// and the store edge that polluted it names the site. A bare frame slice: an
    /// opaque callee (a struct-field or reassigned lambda whose captured
    /// destinations the checker cannot see) may store it through a captured pointer
    /// that outlives the frame, the capture store the argument-to-argument flow
    /// model washes past; a slice cannot cross a channel, but it can be stashed
    /// beyond the frame this way, so it is refused here. A frame-capturing closure
    /// is refused for the same reason: an opaque callee may store it into one of
    /// its own captures that outlives the frame, the `box.f(bad)` shape, so a
    /// closure that captures a frame local is stopped here too. The synchronous
    /// handler idiom that invokes its argument in place, `e.check(lambda ...)`, is
    /// not opaque (see `callee_is_sync_handler`), so it never reaches this face
    /// and the common higher-order call is not over-rejected. A named function and
    /// a recorded lambda are not opaque either, so a frame slice or closure handed
    /// to one of those, or to a builtin higher-order, is checked precisely and
    /// unaffected by this face.
    fn check_opaque_call_send(&mut self, callee: &Expr, args: &[Expr], call_span: Span) {
        if self.types_only || !self.callee_is_opaque(callee) {
            return;
        }
        for arg in args {
            // A managed pointer whose object was polluted with a frame view: refused
            // as a possible channel send, blamed at the polluting store edge.
            if let Some(root) = self.send_value_root(arg) {
                if is_managed(&self.lookup(&root))
                    && (self.is_esc_slice(&root) || self.is_esc_closure(&root))
                {
                    if let Some((pspan, i, k)) = self.flow_prov.get(&root).copied() {
                        self.emit_escape(
                            format!(
                                "this call may send '{root}' across a channel, but argument {}'s view was stored into argument {} here, leaving '{root}' viewing the current frame; move the backing to the heap",
                                i + 1,
                                k + 1
                            ),
                            pspan,
                        );
                    } else {
                        self.emit_escape(
                            format!(
                                "this call may send '{root}' across a channel; '{root}' holds a view of the current frame, which the receiver outlives; move the backing to the heap or send heap owned data"
                            ),
                            call_span,
                        );
                    }
                    continue;
                }
            }
            // A bare frame slice or a frame-capturing closure: an opaque callee
            // may store it through a captured place that outlives this frame (the
            // capture store the argument-to-argument flow model washes past), so
            // it is refused. A clean or heap-backed slice, and a closure that
            // captures only parameters or globals, carry no frame view and are
            // untouched. The synchronous higher-order idiom that invokes its
            // handler in place, `e.check(lambda ...)`, is not opaque (see
            // `callee_is_sync_handler`), so it never reaches this face; a
            // genuinely opaque callee (a struct-field, tuple, or reassigned
            // lambda) may stash the closure into a capture the frame outlives,
            // exactly the `box.f(bad)` shape, and is caught here.
            let (fs, fc) = self.value_escape(arg);
            if fs {
                self.emit_escape(
                    "this call may store a view of the current frame beyond it; put the backing on the heap".to_string(),
                    call_span,
                );
            } else if fc {
                self.emit_escape(
                    "this call may store a closure that captures the current frame beyond it; put the captured backing on the heap".to_string(),
                    call_span,
                );
            }
        }
    }

    /// Whether a value of this type may cross a `spawn` as a heap copied
    /// capture. A slice, a closure, or an interface value may carry a pointer
    /// into the spawning frame that the copy would dangle, so they are rejected
    /// wherever they sit, including inside a struct or enum field. A pointer
    /// field is fine: every pointer targets the heap, where the generation
    /// check covers it. `seen` breaks recursive type cycles.
    fn spawn_capturable(&self, t: &Ty, seen: &mut HashSet<String>) -> bool {
        match t {
            // A future belongs to the event loop thread; a completer carries the
            // raw handle words instead, so a typed future is not a capturable
            // value.
            Ty::Future(_) => false,
            // A collected value stays on the main thread; a spawn or submit would
            // carry it into another thread's frame, so it is not capturable.
            Ty::Collector(_) => false,
            Ty::Slice(_) | Ty::Func(..) => false,
            // A pointer targets the heap, which the generation check covers, so a
            // slice or interface value behind it is fine to copy into a spawned
            // frame. Two payloads are the exception. A future is event-loop-thread
            // property, so one smuggled behind a pointer still faults when the
            // completer thread awaits it. A collected value stays on the main
            // thread: its block is reachable only from anchor-side roots the scan
            // covers, never the worker stack, so a collected ref carried behind a
            // pointer into the task is swept while the task still holds it. Hunt
            // the pointee for either and refuse only that, leaving every other
            // pointer capturable.
            Ty::Ptr(b) | Ty::RawPtr(b) => {
                let mut fseen = HashSet::new();
                let mut cseen = HashSet::new();
                !self.ptr_reaches_future(b, &mut fseen)
                    && !self.ptr_reaches_collector(b, &mut cseen)
            }
            Ty::Array(e, _) => self.spawn_capturable(e, seen),
            Ty::Tuple(ts) => ts.iter().all(|x| self.spawn_capturable(x, seen)),
            Ty::Named(n) => {
                if self.ifaces.contains(n) {
                    return false;
                }
                if !seen.insert(n.clone()) {
                    return true;
                }
                match self.embed_fields.get(n).cloned() {
                    Some(fs) => fs.iter().all(|f| self.spawn_capturable(f, seen)),
                    None => true,
                }
            }
            _ => true,
        }
    }

    /// Whether a future is reachable through this type once a pointer is crossed.
    /// A future belongs to the event loop thread, so smuggling one behind a managed
    /// or raw pointer into a spawned frame still faults when the completer awaits
    /// it. A slice or interface value behind a pointer is fine, so only a future
    /// counts here; the walk follows nested pointers, arrays, tuples, and struct
    /// fields. `seen` breaks recursive type cycles.
    fn ptr_reaches_future(&self, t: &Ty, seen: &mut HashSet<String>) -> bool {
        match t {
            Ty::Future(_) => true,
            Ty::Ptr(b) | Ty::RawPtr(b) | Ty::Slice(b) | Ty::Array(b, _) => {
                self.ptr_reaches_future(b, seen)
            }
            Ty::Tuple(ts) => ts.iter().any(|x| self.ptr_reaches_future(x, seen)),
            Ty::Named(n) => {
                if !seen.insert(n.clone()) {
                    return false;
                }
                match self.embed_fields.get(n).cloned() {
                    Some(fs) => fs.iter().any(|f| self.ptr_reaches_future(f, seen)),
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// Whether a collector is reachable through this type once a pointer is crossed.
    /// A collected value stays on the main thread, so smuggling one behind a managed
    /// or raw pointer into a spawned frame leaves its block reachable only from
    /// anchor-side roots the scan covers, never the worker stack, so it is swept
    /// while the task holds the pointer. A collector value in hand is caught by the
    /// direct arm of `spawn_capturable`; this walk finds one behind a pointer,
    /// through nested pointers, arrays, tuples, and struct fields. `seen` breaks
    /// recursive type cycles. A generic burial the erased walk cannot see is caught
    /// again at mono, the same second layer the future reach walk takes.
    fn ptr_reaches_collector(&self, t: &Ty, seen: &mut HashSet<String>) -> bool {
        match t {
            Ty::Collector(_) => true,
            Ty::Ptr(b) | Ty::RawPtr(b) | Ty::Slice(b) | Ty::Array(b, _) => {
                self.ptr_reaches_collector(b, seen)
            }
            Ty::Tuple(ts) => ts.iter().any(|x| self.ptr_reaches_collector(x, seen)),
            Ty::Named(n) => {
                if !seen.insert(n.clone()) {
                    return false;
                }
                match self.embed_fields.get(n).cloned() {
                    Some(fs) => fs.iter().any(|f| self.ptr_reaches_collector(f, seen)),
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// The names a lambda captures from enclosing scopes, in first use order,
    /// for the spawn capture checks.
    fn spawn_lambda_captures(&self, l: &Lambda) -> Vec<String> {
        let mut used = Vec::new();
        let mut bound: HashSet<String> = l.params.iter().map(|p| p.name.clone()).collect();
        crate::parser::ast::collect_block(&l.body, &mut used, &mut bound);
        let mut seen = HashSet::new();
        used.into_iter()
            .filter(|n| {
                !bound.contains(n)
                    && self.scopes.iter().any(|s| s.contains_key(n))
                    && seen.insert(n.clone())
            })
            .collect()
    }

    /// Whether a lambda reads any variable bound in an enclosing scope, which
    /// makes its environment a frame local that must not escape.
    fn lambda_captures_local(&self, l: &Lambda) -> bool {
        let mut used = Vec::new();
        let mut bound: HashSet<String> = l.params.iter().map(|p| p.name.clone()).collect();
        crate::parser::ast::collect_block(&l.body, &mut used, &mut bound);
        used.iter()
            .any(|n| !bound.contains(n) && self.scopes.iter().any(|s| s.contains_key(n)))
    }

    fn infer(&mut self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Int(v, suffix) => {
                let w = int_suffix_width(suffix);
                if w != 0 {
                    self.check_int_fits_suffixed(*v, w, e.span);
                }
                Ty::Int(w)
            }
            ExprKind::Float(_, suffix) => Ty::Float(float_suffix_width(suffix)),
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Char(_) => Ty::Char,
            ExprKind::Rune(_) => Ty::Rune,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Ident(name) => {
                if !self.types_only && matches!(self.own_of(name), Some(Own::Moved)) {
                    self.err("use of a moved pointer", e.span);
                }
                // An async function's name in value position cannot be stored or
                // passed, only called. A direct call resolves its callee without
                // this arm, so a plain call never trips it; a local of the same
                // name shadows the function.
                if self.async_fns.contains_key(name) && !self.is_local(name) {
                    self.err(
                        format!("'{name}' is async; call it with await or start it with async_run"),
                        e.span,
                    );
                }
                // A bare variant name in value position is not a constructor
                // either: a nullary `None` must be written `Opt.None`, so it cannot
                // slip through as the Unknown it otherwise looks up as and reach a
                // codegen path that loads a non-existent binding. A function or an
                // in-scope local of the same name shadows the variant and is the
                // real value, so only a name that is a variant and nothing else is
                // refused.
                if !self.is_local(name) && !self.sigs.contains_key(name) {
                    if let Some(en) = self.variant_owner(name).map(str::to_string) {
                        self.err(
                            format!(
                                "use the qualified form '{en}.{name}' to name an enum value; the unqualified variant name is not one"
                            ),
                            e.span,
                        );
                    }
                }
                self.lookup(name)
            }
            ExprKind::Unary(op, x) => {
                let t = self.infer(x);
                self.check_unary(*op, &t, e.span)
            }
            ExprKind::Binary(op, a, b) => {
                let ta = self.infer(a);
                let tb = self.infer(b);
                let result = self.check_binary(*op, &ta, &tb, e.span);
                match op {
                    BinOp::Shl | BinOp::Shr => self.check_shift_amount(b, &result, e.span),
                    BinOp::Pow => self.check_pow_exponent(&ta, &tb, b, e.span),
                    _ => {}
                }
                result
            }
            ExprKind::Call(f, args) => {
                // A helper that sinks a parameter into a channel crosses a thread
                // boundary the receiver outlives, so a polluted argument in that
                // position is rejected at the call, read before any edge raises a
                // flag (M5, interprocedural chan-send). The named-helper form
                // reads the callee's summary; the closure form reads the sink set
                // recorded on a local bound to a sinking lambda.
                self.check_call_sinks(f, args, e.span);
                self.check_lambda_call_sinks(f, args, e.span);
                // The leaf-frame default-deny: a call whose callee is opaque to the
                // send analysis (a field or method callee, a reassigned or tuple-
                // sourced binding, any callee outside the four known buckets) may
                // hand an argument to a channel, so a polluted managed pointer in
                // any argument position is refused, the case the value-flow through
                // a laundered lambda binding would otherwise wash past.
                self.check_opaque_call_send(f, args, e.span);
                // A direct call of a lambda bound to a local stores its argument
                // through the lambda's captured bindings (per the escape pass's
                // capture-flow edges), a store the argument-to-argument flow model
                // cannot see; raise each captured binding's flag so its own egress
                // check catches a laundered frame view (M5, capture store).
                self.apply_lambda_capture_flows(f, args, e.span);
                // Apply the callee's store edges to the caller before typing the
                // call, so a frame view laundered into a parameter or a local is
                // flagged at its true source (M5, interprocedural escape).
                self.apply_call_flows(f, args, e.span);
                // A method call hides its receiver from every check above: the
                // callee is a field expression, not a bare name, and the receiver
                // is not in the argument list. Thread it as effective argument 0
                // and read the method's summary (self as parameter 0), so a
                // self-sink method on a polluted receiver, or a frame view stored
                // through self, is caught (M5, method receiver sink).
                self.check_method_call_escape(f, args, e.span);
                self.infer_call(f, args)
            }
            ExprKind::Field(x, name) => {
                // A fixed array exposes only `.len`, the int64 count a slice's
                // `.len` also yields. Any other field on an array is rejected here
                // so it is a clear error, not a silent zero from codegen. Every
                // other base stays permissive and resolves its field in codegen.
                let tx = self.infer(x);
                if let Ty::Array(..) = tx {
                    if name == "len" {
                        return Ty::Int(64);
                    }
                    self.err(
                        format!("a fixed array has no field '{name}'; only '.len' is available"),
                        e.span,
                    );
                }
                // An error carries exactly one field, `message`, the string the
                // spec names. Reading it is not handling, so it never discharges
                // the must-handle obligation; the type is Str either way. Any
                // other field name is a clear error rather than a silent zero.
                if let Ty::Error = tx {
                    if name == "message" {
                        return Ty::Str;
                    }
                    self.err(
                        format!("error has no field '{name}'; it carries only 'message'"),
                        e.span,
                    );
                    return Ty::Str;
                }
                Ty::Unknown
            }
            ExprKind::Index(x, i) => {
                let tx = self.infer(x);
                self.infer(i);
                if matches!(i.kind, ExprKind::Range(..)) {
                    Ty::Slice(Box::new(elem_of(&tx)))
                } else {
                    elem_of(&tx)
                }
            }
            ExprKind::Range(a, b, _) => {
                self.infer(a);
                self.infer(b);
                Ty::Unknown
            }
            ExprKind::Tuple(xs) => Ty::Tuple(xs.iter().map(|x| self.infer(x)).collect()),
            ExprKind::Array(xs) => {
                let mut elem = Ty::Unknown;
                for x in xs {
                    elem = self.infer(x);
                }
                Ty::Slice(Box::new(elem))
            }
            ExprKind::StructLit(name, fields) => {
                self.check_struct_lit(name, fields, e.span);
                if name == "Future" {
                    // The future struct's words do not carry its element type, so
                    // a literal infers as a future of an unknown element, which
                    // the compatibility rule accepts against any Future<T>.
                    Ty::Future(Box::new(Ty::Unknown))
                } else {
                    // On the ground AST mono has mangled a `Future<T>` literal to
                    // `Future$T`, so unmangle restores the future shape the async
                    // call and the annotation both carry, keeping the three forms
                    // mutually compatible. A plain struct name is left untouched.
                    self.unmangle(named_ty(name))
                }
            }
            ExprKind::Lambda(l) => self.infer_lambda(l, e.span, &[]),
            ExprKind::Match(m) => {
                self.walk_match(m);
                Ty::Unknown
            }
            ExprKind::Do(_, binds) => {
                for b in binds {
                    self.infer(&b.expr);
                }
                Ty::Unknown
            }
            // An await reached through plain inference is buried mid-expression;
            // the sanctioned statement positions intercept it before this arm, so
            // anything landing here is a misuse. This is the backstop for the
            // parser's own rejection, catching an await a rewrite might smuggle in.
            ExprKind::Await(op, _) => {
                self.infer(op);
                self.err(
                    "'await' cannot appear mid-expression; give the awaited value a name, as in v, e := await f",
                    e.span,
                );
                Ty::Unknown
            }
            ExprKind::SizeofType(_) => Ty::Int(64),
            // `collector<T>(e)` mints a collected block. The value checks against
            // the element type T, and the result is a `collector<T>`. Deref and
            // field projection then mirror `*T`.
            ExprKind::Collect { ty, arg } => {
                let inner = self.lower(ty);
                let at = self.infer(arg);
                if !compatible(&inner, &at) {
                    self.err(
                        "the collected value's type does not match the collector element type",
                        arg.span,
                    );
                }
                self.check_int_fits(arg, &inner);
                // The kind of element picks the soundness check. A slice element
                // is deep copied onto the collected heap, so a frame-view source
                // is legal; only its own element must be immortal safe, since a
                // one-level copy does not immortalize a nested fat pointer. A
                // function element mints a fresh collected environment, so every
                // capture must be immortal safe. Any other element is the plain
                // kind: it copies its value into the collected block, so a view it
                // reaches by shape is rejected and a pointer whose pointee views a
                // frame is caught by the escape walk. The unfixed element is walked
                // so an interface survives, and every check runs on both passes so
                // a view laundered through a generic is caught once mono grounds it.
                let unfixed = lower(ty, &self.cur_generics);
                match &unfixed {
                    Ty::Slice(u) => {
                        let mut seen = HashSet::new();
                        if self.ty_reaches_view(u, &mut seen) {
                            self.err(
                                "a collected slice's element cannot itself hold a slice, function, or interface; collect the inner view first",
                                e.span,
                            );
                        } else if self.ty_reaches_managed(u) && self.slice_source_buries_view(arg) {
                            // The deep copy immortalizes the element storage but not
                            // each pointee, so an element reaching a managed pointer
                            // whose pointee stores a frame view still dangles once the
                            // frame is gone. The outer slice being a frame view is
                            // fine, the copy fixes that, so this looks past it at the
                            // buried pointers the source reaches and only rejects a
                            // tainted one. A scalar or scalar-struct element reaches no
                            // managed pointer, so the common slice kind skips this and
                            // its frame-view source stays legal.
                            self.err(
                                "a collected slice element holds a pointer to an object that stores a view of the current frame; the collected block outlives the frame, so heap own the pointee or collect it first",
                                e.span,
                            );
                        }
                    }
                    Ty::Func(..) => self.check_collect_captures(arg, e.span),
                    _ => {
                        let mut seen = HashSet::new();
                        if self.ty_reaches_view(&unfixed, &mut seen) {
                            self.err(
                                "a collected value cannot hold a slice, function, or interface; collect a scalar, a pointer, or a struct of those",
                                e.span,
                            );
                        }
                        self.check_collect_escape(arg, &inner, e.span);
                    }
                }
                Ty::Collector(Box::new(inner))
            }
        }
    }

    /// Rejects a suffixed literal whose value cannot fit its own suffix, like
    /// `300i8`, which would silently truncate in codegen.
    fn check_int_fits_suffixed(&mut self, v: i64, w: u32, span: Span) {
        if w >= 64 {
            return;
        }
        let val = v as i128;
        // Signed bounds for the width; the unsigned suffixes are reserved and
        // rejected in the lexer, so only the signed widths reach here.
        let lo = -(1i128 << (w - 1));
        let hi = (1i128 << (w - 1)) - 1;
        if val < lo || val > hi {
            self.err(format!("literal {val} does not fit in {w} bits"), span);
        }
    }

    fn infer_call(&mut self, f: &Expr, args: &[Expr]) -> Ty {
        // Method call syntax. The builtin `error` methods have known types; every
        // other method call stays permissive and returns Unknown for now.
        if let ExprKind::Field(base, mname) = &f.kind {
            let base_ty = self.infer(base);
            let is_error = matches!(base_ty, Ty::Error);
            let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer(a)).collect();
            // A method call on an enum value, `m.unwrap()`, has no dispatch:
            // methods on an enum are rejected at the impl (see the Item::Impl
            // guard), so the receiver names an enum with no methods at all. Without
            // this the call falls through to the permissive Unknown return and
            // codegen emits a zero, printing a wrong value. A qualified constructor
            // `Opt.Some(..)` names the enum *type*, not a value, so it is exempted
            // by the base-is-enum-name guard and handled by the constructor arm.
            let base_is_enum_name =
                matches!(&base.kind, ExprKind::Ident(n) if self.enums.contains_key(n));
            // The receiver's enum either shows in its checked type (a parameter or
            // field typed as the enum) or, for an Unknown-typed enum local, in the
            // enum-bind side table populated at its constructor.
            let recv_enum = match &base_ty {
                Ty::Named(en) if self.enums.contains_key(en) => Some(en.clone()),
                _ => match &base.kind {
                    ExprKind::Ident(n) => self.enum_bind_name(n),
                    _ => None,
                },
            };
            if let Some(en) = recv_enum {
                if !base_is_enum_name {
                    let disp = en.split('$').next().unwrap_or(&en);
                    self.err(
                        format!("'{mname}' is not defined; methods on the enum '{disp}' are not supported, match on it instead"),
                        f.span,
                    );
                    return Ty::Unknown;
                }
            }
            // A qualified enum constructor, `Opt.Some(args)`. Check its argument
            // count and payload types against the variant's declaration, and run
            // the slice-of-interface covariance guard on each payload, so a wrong
            // arity or a mistyped payload is caught at the constructor rather than
            // papered over as the Unknown this branch otherwise returns.
            if let ExprKind::Ident(en) = &base.kind {
                if self.enums.contains_key(en) {
                    self.check_variant_ctor_args(en, mname, args, &arg_tys, f.span);
                    if let Some(payloads) = self.variant_payloads.get(mname).cloned() {
                        for (i, (pty, aty)) in payloads.iter().zip(&arg_tys).enumerate() {
                            if let Some(arg) = args.get(i) {
                                self.check_slice_covariance(pty, aty, arg, arg.span);
                            }
                        }
                    }
                }
            }
            // A method call is otherwise opaque (its return types as Unknown), but
            // the callee's parameters are known from the impl. Passing a value
            // `self` into a `*T` parameter hands the struct where a fat pointer
            // belongs; catch it here with the same precise self-value message a
            // direct call earns, rather than leaving a stray backend fault. A bare
            // lambda at a closure-collector parameter is rejected here too: mono
            // rewrites a lambda into a mint only at a direct top-level call, never a
            // method argument, so a bare lambda would lower to a stack environment
            // typed as a collector and dangle. Point at the explicit mint.
            // A concrete receiver names its struct through `chain_ty`; an
            // interface-typed receiver is fixed to Unknown in scope, so recover its
            // interface name from the annotated-binding record instead.
            let recv_name = self.receiver_type_name(base).or_else(|| match &base.kind {
                ExprKind::Ident(n) => self.iface_bind_name(n),
                _ => None,
            });
            if let Some(tname) = recv_name {
                // A concrete receiver keys `method_params`; an interface receiver
                // keys `iface_method_params`. Either table stores unfixed parameter
                // types, so a slice-of-interface parameter still carries its
                // interface name for the covariance guard below.
                let params = self
                    .method_params
                    .get(&(tname.clone(), mname.clone()))
                    .or_else(|| self.iface_method_params.get(&(tname, mname.clone())))
                    .cloned();
                if let Some(params) = params {
                    for (i, p) in params.iter().enumerate() {
                        if let Some(arg) = args.get(i) {
                            self.self_value_in_ptr_position(arg, p);
                            // method_params keeps the unfixed parameter type, so a
                            // slice-of-interface param still names the interface. A
                            // slice of concrete structs passed there cannot be
                            // reinterpreted whole; reject it here, mirroring the
                            // direct-call site, so it never reaches the codegen
                            // backstop panic.
                            if let Some(aty) = arg_tys.get(i) {
                                self.check_slice_covariance(p, aty, arg, arg.span);
                            }
                            if matches!(p, Ty::Collector(inner) if matches!(&**inner, Ty::Func(..)))
                                && matches!(&arg.kind, ExprKind::Lambda(_))
                            {
                                self.err(
                                    "a bare lambda cannot become a closure collector at a method argument; write the mint explicitly: collector<F>(lambda ...)",
                                    arg.span,
                                );
                            }
                            // The same direct error hand-off a bare call discharges:
                            // an error binding handed to a method parameter declared
                            // `error` is that method's to inspect. Only a bare binding
                            // at the argument counts, so a laundered error stays
                            // pending.
                            if matches!(p, Ty::Error) && matches!(arg_tys.get(i), Some(Ty::Error)) {
                                if let ExprKind::Ident(n) = &arg.kind {
                                    self.mark_err_handled(n);
                                }
                            }
                        }
                    }
                }
            }
            if is_error {
                // exists, check, and ignore are the three ways to handle an
                // error, so a bound error the call is invoked on is discharged.
                if matches!(mname.as_str(), "exists" | "check" | "ignore") {
                    if let ExprKind::Ident(bname) = &base.kind {
                        self.mark_err_handled(bname);
                    }
                }
                return match mname.as_str() {
                    "exists" => Ty::Bool,
                    "toString" => Ty::Str,
                    "check" | "ignore" => Ty::Unit,
                    _ => Ty::Unknown,
                };
            }
            return Ty::Unknown;
        }
        // A builtin with a known return type, unless a user function of the same
        // name shadows it, in which case the normal signature path wins.
        if let ExprKind::Ident(name) = &f.kind {
            // A channel send hands its element to a receiver thread that outlives
            // the sending frame, so a value whose binding a store edge polluted
            // with a frame view would dangle there. Checked by name beside the
            // element-type ban the monomorphizer applies, since chan_send is a
            // library generic, not a builtin, and reaches this signature path.
            if name == "chan_send" || name == "chan_try_send" {
                self.check_chan_send_flow(name, args);
            }
            // The three loop-pumping stdlib functions crank the reactor until a
            // future resolves. An async func already runs on the event loop
            // thread, so calling one from inside re-enters the loop and deadlocks.
            // The suspension keyword `await` is the sanctioned form there and
            // parses to a distinct node, so only these bare stdlib calls reach
            // here. Mirrors the async_run-in-async reject; surface direct calls
            // only, an indirect sync helper that pumps is left to doctrine.
            if self.in_async
                && !self.is_local(name)
                && matches!(name.as_str(), "await" | "await_timeout" | "try_poll")
            {
                self.err(
                    format!("'{name}' pumps the event loop and cannot be called inside an async func; use the await statement"),
                    f.span,
                );
            }
            if !self.sigs.contains_key(name) {
                // An unqualified variant name is not a constructor. `Some(x)` must
                // be written `Opt.Some(x)`; rejecting the bare form here, before
                // codegen, forecloses a lowering that would resolve it by the
                // variant's global name and collide with a like-named function, a
                // stale local now out of scope, or an ambiguous generic instance. A
                // function or an in-scope local of the same name shadows the variant
                // and dispatches normally, so only a name that is a variant and
                // nothing else is refused.
                if !self.is_local(name) {
                    if let Some(en) = self.variant_owner(name).map(str::to_string) {
                        self.err(
                            format!(
                                "use the qualified form '{en}.{name}' to construct an enum value; the unqualified variant name is not a constructor"
                            ),
                            f.span,
                        );
                        for a in args {
                            self.infer(a);
                        }
                        return Ty::Unknown;
                    }
                }
                // The functional builtins take a fixed shape: a collection and a
                // function, plus a seed for fold. Codegen reads exactly those
                // operands and ignores any surplus, so a stray extra argument
                // sails past inference and lowers to invalid IR. Range the arity
                // here so the miscount is a diagnostic, not a backend fault.
                if matches!(
                    name.as_str(),
                    "map" | "filter" | "reduce" | "fold" | "foreach"
                ) {
                    let want = if name == "fold" { 3 } else { 2 };
                    if args.len() != want {
                        self.err(
                            format!(
                                "{name} takes {want} argument(s), but {} were given",
                                args.len()
                            ),
                            f.span,
                        );
                    }
                    for a in args {
                        self.infer(a);
                    }
                    return Ty::Unknown;
                }
                if let Some(ty) = builtin_ret(name) {
                    for a in args {
                        self.infer(a);
                    }
                    return ty;
                }
                // print, println, and printerr take an optional format string. A
                // string literal first argument is always a format string, so a
                // stray hole or a doubled brace behaves the same at any arity,
                // and every printed value must have a printable type.
                if name == "print" || name == "println" || name == "printerr" {
                    let tys: Vec<Ty> = args.iter().map(|a| self.infer(a)).collect();
                    let fmt_lit = matches!(args.first().map(|a| &a.kind), Some(ExprKind::Str(_)));
                    if fmt_lit {
                        self.check_format(args);
                    }
                    let value_start = if fmt_lit { 1 } else { 0 };
                    for (a, t) in args.iter().zip(&tys).skip(value_start) {
                        self.check_printable(t, a.span);
                    }
                    return Ty::Unit;
                }
                if name == "spawn" {
                    self.check_task_lambda("spawn", args, f.span);
                    return Ty::Tuple(vec![Ty::Thread, Ty::Error]);
                }
                if name == "submit" {
                    // A pool task is a thread body that runs later, so submit
                    // shares spawn's whole argument rule. It returns only an
                    // error: the pool owns the task and results flow through a
                    // channel, never a handle.
                    self.check_task_lambda("submit", args, f.span);
                    return Ty::Error;
                }
                if name == "join" {
                    let t = args.first().map(|a| self.infer(a)).unwrap_or(Ty::Unknown);
                    if args.len() != 1 || !compatible(&Ty::Thread, &t) {
                        self.err("join takes one thread handle", f.span);
                    }
                    return Ty::Error;
                }
                if name == "async_run" {
                    return self.infer_async_run(args, f.span);
                }
                if name == "alloc" {
                    // alloc(v) yields a managed *T owner of the value's type, so
                    // the single owner pass engages even for the inferred form.
                    // The zero argument form sizes from a pointer annotation and
                    // is only valid where let_stmt intercepted it.
                    if args.is_empty() {
                        self.err(
                            "alloc() with no value is only valid as p: *T = alloc(), where the annotation sizes the allocation",
                            f.span,
                        );
                        return Ty::Ptr(Box::new(Ty::Unknown));
                    }
                    let inner = args.first().map(|a| self.infer(a)).unwrap_or(Ty::Unknown);
                    return Ty::Ptr(Box::new(inner));
                }
                if name == "ptr_add" {
                    // ptr_add is raw byte arithmetic over the raw pointer layer.
                    // A managed *T is a fat value, not a raw operand, so reject it
                    // and point at *raw T. A *void, a pointer to Unit, stays raw.
                    let pt = args.first().map(|a| self.infer(a)).unwrap_or(Ty::Unknown);
                    if matches!(&pt, Ty::Ptr(inner) if !matches!(&**inner, Ty::Unit)) {
                        self.err(
                            "ptr_add takes a raw pointer; use *raw T or *void, not a managed *T",
                            f.span,
                        );
                    }
                    for a in args.iter().skip(1) {
                        self.infer(a);
                    }
                    return pt;
                }
                if name == "move" {
                    // move(x) transfers ownership; its value and type are x's, and
                    // the source binding is invalidated so a later use is rejected.
                    let t = args.first().map(|a| self.infer(a)).unwrap_or(Ty::Unknown);
                    // A collected value is not owned, so there is nothing to move.
                    // Fires on both passes so a `collector<T>` laundered through a
                    // generic is still caught once mono makes it concrete.
                    if matches!(t, Ty::Collector(_)) {
                        if let Some(a) = args.first() {
                            self.err("a collected value is not owned; copy it directly", a.span);
                        }
                        return t;
                    }
                    if let Some(a) = args.first() {
                        if let ExprKind::Ident(src) = &a.kind {
                            if !self.types_only && matches!(self.own_of(src), Some(Own::Borrow)) {
                                self.err(
                                    "cannot move a borrowed pointer; only its owner can be moved",
                                    a.span,
                                );
                            }
                            self.set_moved(src);
                        }
                    }
                    return t;
                }
                if name == "free" {
                    // Only an owner frees. Freeing a borrow, a ref alias or a
                    // pointer parameter, is rejected; its owner frees it instead.
                    if let Some(a) = args.first() {
                        let t = self.infer(a);
                        // A collected value is reclaimed by the collector, never
                        // freed. Fires on both passes so a `collector<T>` laundered
                        // through a generic is caught once mono makes it concrete.
                        if matches!(t, Ty::Collector(_)) {
                            self.err(
                                "a collected value is not freed; the collector reclaims it",
                                a.span,
                            );
                        } else if is_managed(&t) {
                            if let ExprKind::Ident(p) = &a.kind {
                                if !self.types_only && matches!(self.own_of(p), Some(Own::Borrow)) {
                                    self.err(
                                        "cannot free a borrowed pointer; only its owner frees it",
                                        a.span,
                                    );
                                }
                            }
                        }
                    }
                    return Ty::Unit;
                }
            }
        }
        // A bare function name is resolved from the signature table directly, not
        // through the value-position inference of its ident, so an async function
        // called at the call site types as its future instead of tripping the
        // guard that forbids using its name as a value.
        let direct_toplevel = matches!(
            &f.kind,
            ExprKind::Ident(name) if self.sigs.contains_key(name) && !self.is_local(name)
        );
        let callee = if direct_toplevel {
            match &f.kind {
                ExprKind::Ident(name) => self.lookup(name),
                _ => unreachable!(),
            }
        } else {
            self.infer(f)
        };
        // A closure collector is called through the function it wraps: unwrap it
        // so `collector<F>` dispatches exactly as an `F` value does. Codegen sees
        // the same closure rep, so no unwrap is needed there.
        let callee = match callee {
            Ty::Collector(inner) if matches!(&*inner, Ty::Func(..)) => *inner,
            other => other,
        };
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer(a)).collect();
        if let Ty::Func(params, ret) = callee {
            if params.len() != arg_tys.len() {
                self.err(
                    format!(
                        "expected {} argument(s), found {}",
                        params.len(),
                        arg_tys.len()
                    ),
                    f.span,
                );
            } else {
                for (i, (p, a)) in params.iter().zip(&arg_tys).enumerate() {
                    // An error binding handed directly to a parameter declared
                    // `error` is that callee's to inspect, mirroring a returned
                    // error, so mark that one binding discharged. Only a bare error
                    // binding at the argument counts: an error buried inside a
                    // larger expression (`sink(fst(e, e2))`, `sink(wrap(e))`) is not
                    // a hand-off of that error, so it stays pending and is still
                    // rejected. A value that is not an error binding marks nothing.
                    if matches!(p, Ty::Error) && matches!(a, Ty::Error) {
                        if let ExprKind::Ident(n) = &args[i].kind {
                            self.mark_err_handled(n);
                        }
                    }
                    // A lambda literal where a closure collector is expected is
                    // minted by the mono rewrite into an explicit
                    // collector<F>(lambda) node, so accept it here and enforce the
                    // capture rule now, exactly as the explicit mint would. Only a
                    // lambda literal widens: a non-lambda value stays a plain
                    // mismatch, so nothing can skip the mint that immortalizes the
                    // environment. The lambda's own function type is checked against
                    // the collector's wrapped type so a wrong signature still fails.
                    // The widening is only accepted where mono can mint, a direct
                    // top-level function call; an indirect callee (a function value,
                    // a parameter, a field, or a call result) is not rewritten, so a
                    // bare lambda there would lower to a stack environment typed as a
                    // collector and dangle. Reject it and point at the explicit mint.
                    if let Ty::Collector(inner) = p {
                        if matches!(&**inner, Ty::Func(..))
                            && matches!(&args[i].kind, ExprKind::Lambda(_))
                        {
                            if !direct_toplevel {
                                self.err(
                                    "a bare lambda cannot become a closure collector through an indirect call; write the mint explicitly: collector<F>(lambda ...)",
                                    args[i].span,
                                );
                                continue;
                            }
                            if !compatible(inner, a) {
                                self.err(
                                    format!("argument {} has the wrong type", i + 1),
                                    args[i].span,
                                );
                            }
                            self.check_collect_captures(&args[i], args[i].span);
                            continue;
                        }
                    }
                    // A `chan_send(ch, self)` into a `Channel<*T>` earns the precise
                    // self-value message; only when that does not apply does the
                    // generic argument mismatch fire.
                    if !compatible(p, a) && !self.self_value_in_ptr_position(&args[i], p) {
                        self.err(
                            format!("argument {} has the wrong type", i + 1),
                            args[i].span,
                        );
                    }
                    // A function-value call is indirect, so the direct-call
                    // interface-preserving `raw_sigs` check below never runs for it.
                    // When the callee's parameter type still names an interface
                    // element, run the covariance guard here so a slice of concrete
                    // structs at a function value's slice-of-interface parameter is
                    // caught rather than reaching the codegen backstop.
                    self.check_slice_covariance(p, a, &args[i], args[i].span);
                }
                // The fixed parameter types lower an interface to Unknown, so they
                // accept any value. The unfixed signature keeps the interface name,
                // so a concrete struct passed for an interface is checked here.
                if let ExprKind::Ident(name) = &f.kind {
                    if let Some(raw) = self.raw_sigs.get(name).cloned() {
                        for (i, (rp, a)) in raw.iter().zip(&arg_tys).enumerate() {
                            self.check_conformance(rp, a, f.span);
                            // Reject a concrete struct where a tuple member is an
                            // interface, the same as the return position, since
                            // boxing inside a tuple is not supported.
                            let span = args.get(i).map(|x| x.span).unwrap_or(f.span);
                            self.tuple_iface_mismatch(rp, a, span);
                            // Reject reinterpreting a slice of concrete structs as a
                            // slice of interfaces.
                            if let Some(arg) = args.get(i) {
                                self.check_slice_covariance(rp, a, arg, span);
                            }
                        }
                    }
                }
            }
            return *ret;
        }
        Ty::Unknown
    }

    /// Checks `async_run(g(args))`, the only sync-to-async bridge. It cranks the
    /// event loop until a directly-called async func's future completes, then
    /// yields its result, so it is illegal inside an async body and its argument
    /// must be a literal call of an async func, never a stored future.
    fn infer_async_run(&mut self, args: &[Expr], span: Span) -> Ty {
        if self.in_async {
            self.err(
                "async_run cannot be called inside an async func; await the call instead",
                span,
            );
        }
        // Infer every argument so nested errors surface and inner types check.
        for a in args {
            self.infer(a);
        }
        let g = match args.first().map(|a| &a.kind) {
            Some(ExprKind::Call(callee, _)) => match &callee.kind {
                ExprKind::Ident(g) if self.async_fns.contains_key(g) && !self.is_local(g) => {
                    Some(g.clone())
                }
                _ => None,
            },
            _ => None,
        };
        match g {
            Some(g) if args.len() == 1 => self.async_fns.get(&g).cloned().unwrap_or(Ty::Unknown),
            _ => {
                self.err(
                    "async_run takes a direct call of an async func, written at the call site",
                    span,
                );
                Ty::Unknown
            }
        }
    }

    /// Rejects passing a concrete struct where an interface is expected unless the
    /// struct implements it. Only fires when both sides are known by name, an
    /// interface expected and a concrete struct given, so an Unknown or a generic
    /// stays permissive.
    /// Rejects boxing a collected value into an interface, wherever the coercion
    /// happens: an argument, a binding, a return, a struct field, or an array,
    /// slice, or tuple element. An interface value is a { data, vtable } fat pointer
    /// that can ride a channel or a spawn capture off the main thread, and a
    /// collected value stays on the main thread, so the boxing is refused, which
    /// also closes the shape mismatch between the two fat reps before it reaches
    /// codegen. `expected` must be the unfixed type, so the interface name survives
    /// the fix that erases it to Unknown. The walk descends aggregates element by
    /// element, matching how each is boxed one element at a time.
    fn reject_collector_iface_box(&mut self, expected: &Ty, actual: &Ty, span: Span) {
        match (expected, actual) {
            (Ty::Named(iface), _)
                if self.ifaces.contains(iface) && matches!(actual, Ty::Collector(_)) =>
            {
                self.err(
                    "a collected value cannot be boxed into an interface; it stays on the main thread",
                    span,
                );
            }
            (Ty::Array(ee, _) | Ty::Slice(ee), Ty::Array(ae, _) | Ty::Slice(ae)) => {
                self.reject_collector_iface_box(ee, ae, span);
            }
            (Ty::Tuple(es), Ty::Tuple(as_)) if es.len() == as_.len() => {
                for (e, a) in es.iter().zip(as_) {
                    self.reject_collector_iface_box(e, a, span);
                }
            }
            _ => {}
        }
    }

    fn check_conformance(&mut self, expected: &Ty, actual: &Ty, span: Span) {
        self.reject_collector_iface_box(expected, actual, span);
        if matches!(expected, Ty::Named(i) if self.ifaces.contains(i))
            && matches!(actual, Ty::Collector(_))
        {
            return;
        }
        if let (Ty::Named(iface), Ty::Named(concrete)) = (expected, actual) {
            if self.ifaces.contains(iface)
                && !self.ifaces.contains(concrete)
                && self.structs.contains_key(concrete)
                && !self.impls.contains(&(iface.clone(), concrete.clone()))
            {
                self.err(
                    format!("'{concrete}' does not implement interface '{iface}'; add an 'impl {iface} for {concrete}'"),
                    span,
                );
            }
        }
    }

    /// Rejects a concrete struct value where a tuple member is an interface type.
    /// Boxing a struct to an interface value works at a whole value position but
    /// not inside a tuple, so it is refused consistently in both return and
    /// argument positions rather than accepted at one and miscompiled. Returns
    /// whether any such member was found, so a caller can skip the generic
    /// mismatch error it would otherwise also emit. `expected` must be the unfixed
    /// type, so the interface name survives; the fixed table erases it to Unknown.
    fn tuple_iface_mismatch(&mut self, expected: &Ty, actual: &Ty, span: Span) -> bool {
        let (Ty::Tuple(es), Ty::Tuple(as_)) = (expected, actual) else {
            return false;
        };
        if es.len() != as_.len() {
            return false;
        }
        let mut found = false;
        for (ex, ac) in es.iter().zip(as_) {
            if matches!(ex, Ty::Tuple(_)) {
                found |= self.tuple_iface_mismatch(ex, ac, span);
            } else if let Ty::Named(iface) = ex {
                if self.ifaces.contains(iface)
                    && matches!(ac, Ty::Named(c) if self.structs.contains_key(c))
                {
                    self.err(
                        "an interface value inside a tuple is not supported; return or pass the concrete type, or box it outside the tuple",
                        span,
                    );
                    found = true;
                }
            }
        }
        found
    }

    /// Rejects passing an existing slice of concrete structs where a slice of an
    /// interface is expected, a covariant reinterpretation that shares the same
    /// LLVM shape but reads each element as a boxed interface, silently corrupting
    /// memory. An array literal of structs is exempt: it is coerced element by
    /// element, boxing each, not reinterpreted. `expected` must be the unfixed
    /// type, so the interface element name survives the fix that erases it.
    fn check_slice_covariance(&mut self, expected: &Ty, actual: &Ty, value: &Expr, span: Span) {
        let (Ty::Slice(exp_elem), Ty::Slice(act_elem)) = (expected, actual) else {
            return;
        };
        let Ty::Named(iface) = &**exp_elem else {
            return;
        };
        if !self.ifaces.contains(iface) {
            return;
        }
        // An array literal boxes each element as it coerces, so only a slice value
        // reinterpreted whole is unsound.
        if matches!(&value.kind, ExprKind::Array(_)) {
            return;
        }
        if matches!(&**act_elem, Ty::Named(c) if self.structs.contains_key(c) && !self.ifaces.contains(c))
        {
            let concrete = match &**act_elem {
                Ty::Named(c) => c.as_str(),
                _ => "",
            };
            self.err(
                format!("cannot pass a slice of '{concrete}' as a slice of interface '{iface}'; a slice of concrete values cannot be reinterpreted as a slice of interfaces"),
                span,
            );
        }
    }

    /// The covariance guard, descended through a tuple or array/slice literal so a
    /// slice-of-concrete buried one level down, a tuple member (`(v, 1)` at
    /// `(Shape[], int64)`) or an array-of-slices element (`[v]` at `Shape[][1]`),
    /// is caught at the same annotated binding the top-level check covers. The
    /// literal's own element expressions carry the sub-values, so each pairs with
    /// its annotated element type; the outer array-literal exemption still holds
    /// because the recursion looks at the buried slice element, not the whole
    /// literal. A non-literal value falls straight through to the top-level check.
    fn check_covariance_deep(&mut self, expected: &Ty, actual: &Ty, value: &Expr, span: Span) {
        match (expected, actual) {
            (Ty::Tuple(ets), Ty::Tuple(ats)) => {
                if let ExprKind::Tuple(vals) = &value.kind {
                    for (i, (et, at)) in ets.iter().zip(ats).enumerate() {
                        if let Some(sub) = vals.get(i) {
                            self.check_covariance_deep(et, at, sub, sub.span);
                        }
                    }
                    return;
                }
                self.check_slice_covariance(expected, actual, value, span);
            }
            (Ty::Array(et, _) | Ty::Slice(et), Ty::Array(_, _) | Ty::Slice(_)) => {
                if let ExprKind::Array(vals) = &value.kind {
                    for sub in vals {
                        let at = self.chain_ty(sub);
                        self.check_covariance_deep(et, &at, sub, sub.span);
                    }
                    return;
                }
                self.check_slice_covariance(expected, actual, value, span);
            }
            _ => self.check_slice_covariance(expected, actual, value, span),
        }
    }

    /// Checks that an `impl I for T` provides every method the interface requires,
    /// each with the same parameter count, so an incomplete impl is rejected before
    /// codegen emits an undefined vtable.
    fn check_impl_complete(&mut self, im: &Impl) {
        let Some(iface) = im.iface.clone() else {
            return;
        };
        let Some(required) = self.iface_methods.get(&iface).cloned() else {
            return;
        };
        for (mname, arity) in &required {
            match im.methods.iter().find(|m| &m.name == mname) {
                None => self.err(
                    format!("impl {iface} for {} is missing method '{mname}'", im.ty),
                    im.span,
                ),
                Some(m) if m.params.len() != *arity => self.err(
                    format!(
                        "method '{mname}' in impl {iface} for {} has {} parameter(s), the interface requires {arity}",
                        im.ty,
                        m.params.len()
                    ),
                    m.span,
                ),
                Some(_) => {}
            }
        }
    }

    /// Checks a formatted `print` or `println`. The first argument must be a
    /// string literal, and its hole count must match the number of value
    /// arguments, so codegen can expand the call with no runtime format parser.
    fn check_format(&mut self, args: &[Expr]) {
        let ExprKind::Str(fmt) = &args[0].kind else {
            self.err(
                "a formatted print needs a string literal as its first argument",
                args[0].span,
            );
            return;
        };
        match crate::fmt::parse(fmt) {
            Ok(segs) => {
                let holes = crate::fmt::holes(&segs);
                let given = args.len() - 1;
                if holes != given {
                    self.err(
                        format!(
                            "format string has {holes} hole(s) but {given} argument(s) were given"
                        ),
                        args[0].span,
                    );
                }
            }
            Err(msg) => self.err(msg, args[0].span),
        }
    }

    /// Whether a value of this type can be printed. Scalars, strings, and errors
    /// have printers; a struct prints through its Display impl's toString; and
    /// everything else is rejected, since a silently empty print is the worst of
    /// the options.
    fn check_printable(&mut self, t: &Ty, span: Span) {
        match t {
            Ty::Int(_)
            | Ty::Float(_)
            | Ty::Bool
            | Ty::Char
            | Ty::Rune
            | Ty::Str
            | Ty::Error
            | Ty::Unknown => {}
            Ty::Named(n) => {
                if self.structs.contains_key(n) {
                    if !self.impls.contains(&("Display".to_string(), n.clone())) {
                        self.err(
                            format!("'{n}' has no Display impl; add 'impl Display for {n}' with 'toString() -> string' to print it"),
                            span,
                        );
                    }
                } else if self.enums.contains_key(n) {
                    self.err(
                        format!("cannot print the enum '{n}'; match on it and print its parts"),
                        span,
                    );
                }
                // An unknown named type, a generic or an import the checker does
                // not track, stays permissive.
            }
            Ty::Unit => self.err("cannot print a void value", span),
            Ty::Thread => self.err("cannot print a thread handle; join it instead", span),
            Ty::Ptr(_) | Ty::RawPtr(_) => self.err(
                "cannot print a pointer; dereference it or print its fields",
                span,
            ),
            _ => self.err(
                format!("cannot print {}; print its elements instead", ty_str(t)),
                span,
            ),
        }
    }

    /// Checks a lambda body. `borrowed` marks captured managed pointers as
    /// borrows inside the body, used by `spawn` so a thread cannot free or move
    /// a pointer its spawner still owns.
    fn infer_lambda(&mut self, l: &Lambda, span: Span, borrowed: &[String]) -> Ty {
        let saved_ret = self.cur_ret.clone();
        let saved_depth = self.branch_depth;
        self.cur_ret = self.lower(&l.ret);
        self.branch_depth = 0;
        let params: Vec<Ty> = l.params.iter().map(|p| self.lower(&p.ty)).collect();
        self.push_scope();
        for (p, ty) in l.params.iter().zip(&params) {
            self.declare(&p.name, ty.clone());
        }
        for name in borrowed {
            self.declare_own(name, Own::Borrow);
        }
        for s in &l.body.stmts {
            self.stmt(s);
        }
        self.pop_scope();
        let ret = self.cur_ret.clone();
        if !matches!(ret, Ty::Unit) && !block_returns(&l.body) {
            self.err("not all paths in this lambda return a value", span);
        }
        self.cur_ret = saved_ret;
        self.branch_depth = saved_depth;
        Ty::Func(params, Box::new(ret))
    }

    fn walk_match(&mut self, m: &Match) {
        let st = self.infer(&m.scrut);
        // An arm binder projects the scrutinee's payload, so it inherits the
        // scrutinee's escape flags the same way a destructured tuple binder
        // does: only a binder whose payload type can carry a view is flagged,
        // and a clean scrutinee flags nothing. Without this, a flagged value
        // washed through `match x { V(d) => return d }` drops its flag.
        let (scrut_s, scrut_c) = if self.types_only {
            (false, false)
        } else {
            self.value_escape(&m.scrut)
        };
        self.branch_depth += 1;
        for arm in &m.arms {
            self.push_scope();
            match &arm.pat {
                Pattern::Variant(vname, binds) => {
                    let payloads = self
                        .variant_payloads
                        .get(vname)
                        .cloned()
                        .unwrap_or_default();
                    for (i, b) in binds.iter().enumerate() {
                        self.declare(b, Ty::Unknown);
                        if scrut_s || scrut_c {
                            let carries = payloads
                                .get(i)
                                .map(|pt| self.member_carries_view(pt))
                                .unwrap_or(true);
                            if carries {
                                self.set_esc(b, scrut_s, scrut_c);
                            }
                        }
                        // A payload binder projects the scrutinee's payload, so it
                        // aliases the scrutinee's group when the payload type can
                        // reach a managed pointer: `match o { Some(p) => ... }`
                        // where `o` carries a `*Cell` links `p` to `o` (to `c`), so
                        // a frame view stored through `(*p).rows` taints `c` and a
                        // later return of it is caught. Routed through the same
                        // binding-alias choke a `p := <scrut>` projection uses,
                        // gated on the payload type so a scalar binder links
                        // nothing. An erased generic payload reads as a maybe and
                        // links, the coarse over-approximate direction.
                        let links = payloads
                            .get(i)
                            .map(|pt| self.links_as_alias(pt))
                            .unwrap_or(true);
                        if links {
                            self.link_binding_aliases(b, &m.scrut, false);
                        }
                    }
                }
                Pattern::Ident(name) => {
                    self.declare(name, Ty::Unknown);
                    if scrut_s || scrut_c {
                        self.set_esc(name, scrut_s, scrut_c);
                    }
                    // A catch-all binds the whole scrutinee to `name`, a named
                    // binding chain, so it aliases the scrutinee's group when the
                    // scrutinee reaches a managed pointer: a later re-match of
                    // `name` links its own binder back through here to the
                    // scrutinee. Routed through the same choke, which links nothing
                    // when the scrutinee holds no pointer.
                    self.link_binding_aliases(name, &m.scrut, false);
                }
                Pattern::Wildcard => {}
            }
            for s in &arm.body.stmts {
                self.stmt(s);
            }
            self.pop_scope();
        }
        self.branch_depth -= 1;
        // match is defined over enum values. A known enum checks coverage; any
        // other known type is rejected, since codegen has no dispatch for it. An
        // Unknown, a generic or an unresolved import, stays permissive.
        match &st {
            Ty::Named(ename) => {
                if let Some(variants) = self.enums.get(ename).cloned() {
                    self.check_coverage(&variants, &m.arms, m.scrut.span);
                } else if self.structs.contains_key(ename) || self.ifaces.contains(ename) {
                    self.err(
                        format!(
                            "match needs an enum value, and {} is not an enum",
                            ty_str(&st)
                        ),
                        m.scrut.span,
                    );
                }
            }
            Ty::Unknown => {}
            _ => self.err(
                format!("match needs an enum value, not {}", ty_str(&st)),
                m.scrut.span,
            ),
        }
    }

    /// Checks a match over an enum is exhaustive and free of unreachable arms. A
    /// wildcard or a plain identifier that is not a variant is a catch all.
    fn check_coverage(&mut self, variants: &[String], arms: &[Arm], span: Span) {
        let mut covered: HashSet<String> = HashSet::new();
        let mut catch_all = false;
        for arm in arms {
            if catch_all {
                self.err("unreachable match arm after a catch all pattern", span);
                continue;
            }
            match &arm.pat {
                Pattern::Wildcard => catch_all = true,
                Pattern::Ident(n) if !variants.contains(n) => catch_all = true,
                Pattern::Ident(n) | Pattern::Variant(n, _) => {
                    if !covered.insert(n.clone()) {
                        self.err(
                            format!("unreachable match arm, '{n}' is already covered"),
                            span,
                        );
                    }
                }
            }
        }
        if !catch_all {
            for v in variants {
                if !covered.contains(v) {
                    self.err(format!("non exhaustive match, missing variant '{v}'"), span);
                }
            }
        }
    }

    fn check_unary(&mut self, op: UnOp, t: &Ty, span: Span) -> Ty {
        match op {
            UnOp::Deref => match t {
                Ty::Ptr(inner) | Ty::RawPtr(inner) => (**inner).clone(),
                // A plain collector derefs like the managed `*T` it wraps, reading
                // the collected block through the same generation check. A closure
                // or slice collector is not a pointer: it is called or indexed, so
                // dereferencing it is an error rather than a mislowered load.
                Ty::Collector(inner) if !matches!(&**inner, Ty::Func(..) | Ty::Slice(_)) => {
                    (**inner).clone()
                }
                Ty::Unknown => Ty::Unknown,
                _ => {
                    self.err("cannot dereference a non pointer value", span);
                    Ty::Unknown
                }
            },
            UnOp::Neg => {
                if matches!(t, Ty::Int(_) | Ty::Float(_) | Ty::Unknown) {
                    t.clone()
                } else {
                    self.err("unary minus needs a numeric operand", span);
                    Ty::Unknown
                }
            }
            UnOp::Not => {
                if matches!(t, Ty::Bool | Ty::Unknown) {
                    Ty::Bool
                } else {
                    self.err("'!' needs a bool operand", span);
                    Ty::Bool
                }
            }
            UnOp::BitNot => {
                if matches!(t, Ty::Int(_) | Ty::Unknown) {
                    t.clone()
                } else {
                    self.err("'~' needs an integer operand", span);
                    Ty::Unknown
                }
            }
        }
    }

    fn check_binary(&mut self, op: BinOp, a: &Ty, b: &Ty, span: Span) -> Ty {
        use BinOp::*;
        let unknown = matches!(a, Ty::Unknown) || matches!(b, Ty::Unknown);
        match op {
            Add | Sub | Mul | Div | Mod => {
                if unknown {
                    return if matches!(a, Ty::Unknown) {
                        b.clone()
                    } else {
                        a.clone()
                    };
                }
                match (a, b) {
                    // Same kind: the widths must agree, with a bare literal
                    // (width 0) adapting to the other side. Mixing widths would
                    // silently truncate in codegen, so it is an error here.
                    (Ty::Int(x), Ty::Int(y)) | (Ty::Float(x), Ty::Float(y)) => {
                        if x == y || *x == 0 || *y == 0 {
                            let w = (*x).max(*y);
                            if matches!(a, Ty::Int(_)) {
                                Ty::Int(w)
                            } else {
                                Ty::Float(w)
                            }
                        } else {
                            self.err(
                                format!(
                                    "arithmetic mixes {} and {}; match the widths",
                                    ty_str(a),
                                    ty_str(b)
                                ),
                                span,
                            );
                            Ty::Unknown
                        }
                    }
                    _ => {
                        self.err(
                            "arithmetic needs two operands of the same numeric type",
                            span,
                        );
                        Ty::Unknown
                    }
                }
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                if !compatible(a, b) {
                    self.err("comparison needs two operands of the same type", span);
                }
                Ty::Bool
            }
            And | Or => {
                if !(unknown || matches!(a, Ty::Bool) && matches!(b, Ty::Bool)) {
                    self.err("logical operators need bool operands", span);
                }
                Ty::Bool
            }
            BitAnd | BitOr | BitXor | Shl | Shr => {
                if unknown {
                    return if matches!(a, Ty::Unknown) {
                        b.clone()
                    } else {
                        a.clone()
                    };
                }
                match (a, b) {
                    // Bitwise and shift operators take integers only, both the
                    // same width with a width 0 literal adapting, exactly the
                    // arithmetic rule. Mixing widths would silently truncate.
                    (Ty::Int(x), Ty::Int(y)) => {
                        if x == y || *x == 0 || *y == 0 {
                            Ty::Int((*x).max(*y))
                        } else {
                            self.err(
                                format!(
                                    "'{}' mixes {} and {}; match the widths",
                                    binop_spelling(op),
                                    ty_str(a),
                                    ty_str(b)
                                ),
                                span,
                            );
                            Ty::Unknown
                        }
                    }
                    _ => {
                        let msg = if matches!(op, Shl | Shr) {
                            "shift operators need integer operands"
                        } else {
                            "bitwise operators need integer operands"
                        };
                        self.err(msg, span);
                        Ty::Unknown
                    }
                }
            }
            Pow => {
                if unknown {
                    return if matches!(a, Ty::Unknown) {
                        b.clone()
                    } else {
                        a.clone()
                    };
                }
                match (a, b) {
                    // Both integer, same width (a literal adapting), wraps like the
                    // bare `mul`; or both float, same width, the `llvm.pow` path.
                    (Ty::Int(x), Ty::Int(y)) | (Ty::Float(x), Ty::Float(y)) => {
                        if x == y || *x == 0 || *y == 0 {
                            let w = (*x).max(*y);
                            if matches!(a, Ty::Int(_)) {
                                Ty::Int(w)
                            } else {
                                Ty::Float(w)
                            }
                        } else {
                            self.err(
                                format!(
                                    "'**' mixes {} and {}; match the widths",
                                    ty_str(a),
                                    ty_str(b)
                                ),
                                span,
                            );
                            Ty::Unknown
                        }
                    }
                    _ => {
                        self.err("'**' needs two operands of the same numeric type", span);
                        Ty::Unknown
                    }
                }
            }
        }
    }

    /// Rejects a constant negative exponent on integer `**`. A dynamic negative
    /// exponent faults at runtime; a float exponent may be negative.
    fn check_pow_exponent(&mut self, base: &Ty, exp: &Ty, exp_expr: &Expr, span: Span) {
        if !matches!(base, Ty::Int(_)) || !matches!(exp, Ty::Int(_)) {
            return;
        }
        if let Some(v) = const_int(exp_expr) {
            if v < 0 {
                self.err("'**' on integers needs a nonnegative exponent", span);
            }
        }
    }

    /// Rejects a constant shift amount outside `[0, width)`. A width 0 adaptable
    /// literal result counts as int64. A dynamic amount is guarded at runtime.
    fn check_shift_amount(&mut self, amount: &Expr, result: &Ty, span: Span) {
        let Some(v) = const_int(amount) else {
            return;
        };
        if v < 0 {
            self.err("shift amount is negative", span);
            return;
        }
        let w = match result {
            Ty::Int(0) => 64,
            Ty::Int(w) => *w as i128,
            _ => return,
        };
        if v >= w {
            self.err(format!("shift amount {v} is out of range for int{w}"), span);
        }
    }
}

/// The source spelling of a binary operator, for diagnostics that name the fix.
fn binop_spelling(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::Pow => "**",
    }
}

/// The signed value of a constant integer literal, including a negated literal,
/// or None for any computed expression. Used by the shift range check.
fn const_int(e: &Expr) -> Option<i128> {
    match &e.kind {
        ExprKind::Int(v, _) => Some(*v as i128),
        ExprKind::Unary(UnOp::Neg, inner) => match &inner.kind {
            ExprKind::Int(v, _) => Some(-(*v as i128)),
            _ => None,
        },
        _ => None,
    }
}

fn elem_of(t: &Ty) -> Ty {
    match t {
        Ty::Slice(e) | Ty::Array(e, _) => (**e).clone(),
        Ty::Str => Ty::Char,
        // A slice collector indexes and ranges through the slice it wraps, so its
        // element is that slice's element.
        Ty::Collector(inner) => elem_of(inner),
        _ => Ty::Unknown,
    }
}

/// The root binding of a store place, rooting through field accesses, indexes,
/// and pointer dereferences: a store through a parameter pointer reaches the
/// caller's object, so the dereference is followed here. Mirrors the escape
/// summary's `dest_root`, so the caller-side flow routing agrees with the
/// callee-side edge that produced it.
fn store_root(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field(base, _) | ExprKind::Index(base, _) => store_root(base),
        ExprKind::Unary(UnOp::Deref, base) => store_root(base),
        _ => None,
    }
}

/// The pointer binding a by-value projection reads back through, when the chain
/// crosses a dereference: reading `(*p).f` or `(*p)[i]` reads whatever lives
/// behind `p`, so a `p` flagged by a store edge makes the projected view a
/// frame view. None when the chain has no dereference; the plain-root walk
/// (`projection_root`) covers that side. Mirrors the escape summary's
/// `reads_through_root`, so the two layers classify the same chains.
fn deref_root(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Field(base, _) | ExprKind::Index(base, _) => deref_root(base),
        ExprKind::Unary(UnOp::Deref, base) => store_root(base),
        _ => None,
    }
}

/// The TOP store edges of an opaque callee: any argument's view may be stored
/// into any other. Non-view arguments carry no frame view, so an edge from one
/// self-limits at the routing site; the cap guards the absurd arity bound.
fn top_flow_edges(n: usize) -> Vec<(u8, u8)> {
    let n = n.min(64);
    let mut edges = Vec::new();
    for i in 0..n {
        for j in 0..n {
            if i != j {
                edges.push((i as u8, j as u8));
            }
        }
    }
    edges
}

/// Whether an expression is a call to the `alloc` builtin with no arguments,
/// the uninitialized allocation form. A user function named alloc shadows the
/// builtin, so the signature table is consulted.
fn is_zero_arg_alloc(e: &Expr, sigs: &HashMap<String, (Vec<Ty>, Ty)>) -> bool {
    match &e.kind {
        ExprKind::Call(f, args) if args.is_empty() => {
            matches!(&f.kind, ExprKind::Ident(n) if n == "alloc" && !sigs.contains_key(n))
        }
        _ => false,
    }
}

/// Whether every path through a block reaches a `return`. A block returns when
/// any statement in it does: a plain return, an if whose branches both return,
/// or a match whose arms all return (exhaustiveness is checked separately).
/// Loops never count, since their bodies may run zero times.
fn block_returns(b: &Block) -> bool {
    b.stmts.iter().any(stmt_returns)
}

fn stmt_returns(s: &Stmt) -> bool {
    match s {
        Stmt::Return(_) => true,
        Stmt::If(i) => match &i.els {
            Some(els) => block_returns(&i.then) && block_returns(els),
            None => false,
        },
        Stmt::Match(m) => !m.arms.is_empty() && m.arms.iter().all(|a| block_returns(&a.body)),
        _ => false,
    }
}

/// Whether a type is a managed pointer subject to the single owner rules. A
/// `*void`, which is `Ty::Ptr(Unit)`, is the raw allocator currency and is
/// exempt, as is every `*raw T`.
fn is_managed(ty: &Ty) -> bool {
    matches!(ty, Ty::Ptr(inner) if !matches!(&**inner, Ty::Unit))
}

/// The types a foreign C signature may use. The boundary is the raw pointer
/// layer, so a scalar, a `*raw T`, or a `*void` crosses, and `void` is a valid
/// return. A managed `*T` and an aggregate passed by value do not cross.
fn foreign_ty_ok(ty: &Ty) -> bool {
    match ty {
        Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char | Ty::Rune | Ty::Unit => true,
        Ty::RawPtr(_) => true,
        Ty::Ptr(inner) => matches!(**inner, Ty::Unit),
        _ => false,
    }
}

/// Whether a type carries an error, directly or as a component of a tuple, so a
/// bare statement of this type would drop an error that must be handled.
fn ty_has_error(t: &Ty) -> bool {
    match t {
        Ty::Error => true,
        Ty::Tuple(ts) => ts.iter().any(ty_has_error),
        _ => false,
    }
}

fn compatible(a: &Ty, b: &Ty) -> bool {
    if a == b || matches!(a, Ty::Unknown) || matches!(b, Ty::Unknown) {
        return true;
    }
    match (a, b) {
        // A width of 0 is a bare literal, which adapts to any width of its kind.
        // Two concrete widths must match, so an int32 never silently truncates
        // an int64 or widens into one.
        (Ty::Int(x), Ty::Int(y)) | (Ty::Float(x), Ty::Float(y)) => *x == 0 || *y == 0 || x == y,
        (Ty::Array(x, _), Ty::Slice(y)) | (Ty::Slice(x), Ty::Array(y, _)) => compatible(x, y),
        (Ty::Slice(x), Ty::Slice(y)) => compatible(x, y),
        (Ty::Array(x, n), Ty::Array(y, m)) if n == m => compatible(x, y),
        (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => {
            xs.iter().zip(ys).all(|(x, y)| compatible(x, y))
        }
        (Ty::Func(xp, xr), Ty::Func(yp, yr)) if xp.len() == yp.len() => {
            xp.iter().zip(yp).all(|(x, y)| compatible(x, y)) && compatible(xr, yr)
        }
        (Ty::RawPtr(x), Ty::RawPtr(y)) => compatible(x, y),
        // Two futures agree when their elements agree; an Unknown element, from
        // a generic Future<T> or the element-erased struct literal, wildcards.
        (Ty::Future(x), Ty::Future(y)) => {
            matches!(**x, Ty::Unknown) || matches!(**y, Ty::Unknown) || compatible(x, y)
        }
        (Ty::Ptr(x), Ty::Ptr(y)) => {
            matches!(**x, Ty::Unit) || matches!(**y, Ty::Unit) || compatible(x, y)
        }
        // Two collectors agree when their elements agree; the wrapper never
        // widens into a bare pointer, so a `collector<T>` and a `*T` stay distinct.
        (Ty::Collector(x), Ty::Collector(y)) => compatible(x, y),
        // A closure or slice collector may be used where the value it wraps is
        // expected: the collector's rep is that value and its backing outlives the
        // frame, so a `collector<F>` passes for an `F` and a `collector<U[]>` for a
        // `U[]`. This is a one-way widening: the collector sits in the actual
        // position (the second argument, by the checker's expected-then-actual
        // convention). The reverse, a bare value where a collector is expected,
        // stays rejected here so it cannot skip the mint that immortalizes the
        // backing; a lambda literal in that position is minted by the mono rewrite
        // instead. The plain kind never widens: its inner is not a func or slice,
        // so the guard fails and it falls through to a mismatch.
        (Ty::Func(..), Ty::Collector(y)) | (Ty::Slice(_), Ty::Collector(y))
            if matches!(&**y, Ty::Func(..) | Ty::Slice(_)) =>
        {
            compatible(a, y)
        }
        // *void is the erased currency of the allocator and FFI layer, a thin
        // untracked pointer, so any *raw T passes where *void is expected;
        // codegen already lowers *void to the same bare word. The reverse
        // direction stays an error: a *void that could become a typed *raw
        // would let a managed pointer launder through *void into a
        // dereferenceable alias the generation check cannot see.
        (Ty::Ptr(u), Ty::RawPtr(_)) => matches!(**u, Ty::Unit),
        // A char is an ASCII byte, freely compared with and assigned to ints.
        (Ty::Char, Ty::Int(_)) | (Ty::Int(_), Ty::Char) => true,
        (Ty::Rune, Ty::Int(_)) | (Ty::Int(_), Ty::Rune) => true,
        _ => false,
    }
}

fn lower(t: &Type, generics: &HashSet<String>) -> Ty {
    match t {
        // The one-shot future carries its element type through the checker even
        // though the runtime struct erases it; a generic element becomes Unknown,
        // which the compatibility rule wildcards.
        Type::Named(n, args) if n == "Future" && args.len() == 1 => {
            Ty::Future(Box::new(lower(&args[0], generics)))
        }
        Type::Named(n, _) => {
            if generics.contains(n) {
                Ty::Unknown
            } else {
                named_ty(n)
            }
        }
        Type::Ptr(b) => Ty::Ptr(Box::new(lower(b, generics))),
        Type::RawPtr(b) => Ty::RawPtr(Box::new(lower(b, generics))),
        Type::Slice(b) => Ty::Slice(Box::new(lower(b, generics))),
        Type::Array(b, n) => Ty::Array(Box::new(lower(b, generics)), *n),
        Type::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| lower(t, generics)).collect()),
        Type::Func(ps, r) => Ty::Func(
            ps.iter().map(|t| lower(t, generics)).collect(),
            Box::new(lower(r, generics)),
        ),
        Type::Collector(b) => Ty::Collector(Box::new(lower(b, generics))),
        Type::Unit => Ty::Unit,
        // A do-continuation hole is open to any monad element; Unknown lets the
        // compatibility rule wildcard it, so typeck stays permissive until mono
        // pins the concrete type per site.
        Type::Infer => Ty::Unknown,
    }
}

/// Spells a checked type back as an AST type for the narrow mutable-tuple record,
/// covering only the shapes that class carries: scalars, slices, arrays, tuples,
/// and pointers. A width of 0, a bare literal, spells its default width, matching
/// how codegen sizes an unannotated slot. Signedness is not spelled, since codegen
/// tracks none, so `int64` stands for both the signed and unsigned word and the
/// slot lowers identically either way. A shape the printer cannot name faithfully,
/// a named struct or interface, a generic hole, a future, or a function, returns
/// `None`, so the caller declines to record and leaves the binding untouched.
fn ty_to_ast(t: &Ty) -> Option<Type> {
    let r = match t {
        Ty::Int(w) => Type::Named(int_width_name(*w)?, Vec::new()),
        Ty::Float(w) => Type::Named(float_width_name(*w)?, Vec::new()),
        Ty::Bool => Type::Named("bool".to_string(), Vec::new()),
        Ty::Char => Type::Named("char".to_string(), Vec::new()),
        Ty::Rune => Type::Named("rune".to_string(), Vec::new()),
        Ty::Str => Type::Named("string".to_string(), Vec::new()),
        Ty::Unit => Type::Unit,
        Ty::Ptr(b) => Type::Ptr(Box::new(ty_to_ast(b)?)),
        Ty::RawPtr(b) => Type::RawPtr(Box::new(ty_to_ast(b)?)),
        Ty::Slice(b) => Type::Slice(Box::new(ty_to_ast(b)?)),
        Ty::Array(b, n) => Type::Array(Box::new(ty_to_ast(b)?), *n),
        Ty::Tuple(ts) => Type::Tuple(ts.iter().map(ty_to_ast).collect::<Option<Vec<_>>>()?),
        _ => return None,
    };
    Some(r)
}

/// The AST name for a checked integer width, defaulting a bare literal's width 0
/// to the int64 codegen picks for it. An unrecognized width returns `None`.
fn int_width_name(w: u32) -> Option<String> {
    let n = match w {
        0 | 64 => "int64",
        32 => "int32",
        16 => "int16",
        8 => "int8",
        _ => return None,
    };
    Some(n.to_string())
}

/// The AST name for a checked float width, defaulting a bare literal's width 0 to
/// float64. An unrecognized width returns `None`.
fn float_width_name(w: u32) -> Option<String> {
    let n = match w {
        0 | 64 => "float64",
        32 => "float32",
        _ => return None,
    };
    Some(n.to_string())
}

/// Pins a bare literal's type to its default width, int64 or float64, when it
/// lands in an unannotated binding, matching how codegen sizes the slot.
fn harden(t: Ty) -> Ty {
    match t {
        Ty::Int(0) => Ty::Int(64),
        Ty::Float(0) => Ty::Float(64),
        Ty::Tuple(ts) => Ty::Tuple(ts.into_iter().map(harden).collect()),
        other => other,
    }
}

/// The width an integer literal suffix pins, or 0 for a bare adaptable literal.
fn int_suffix_width(suffix: &Option<String>) -> u32 {
    match suffix.as_deref() {
        Some("i8") | Some("u8") => 8,
        Some("i16") | Some("u16") => 16,
        Some("i32") | Some("u32") => 32,
        Some("i64") | Some("u64") => 64,
        _ => 0,
    }
}

/// The width a float literal suffix pins, or 0 for a bare adaptable literal.
fn float_suffix_width(suffix: &Option<String>) -> u32 {
    match suffix.as_deref() {
        Some("f32") => 32,
        Some("f64") => 64,
        _ => 0,
    }
}

fn named_ty(n: &str) -> Ty {
    match n {
        "int8" | "uint8" => Ty::Int(8),
        "int16" | "uint16" => Ty::Int(16),
        "int32" | "uint32" => Ty::Int(32),
        "int64" | "uint64" => Ty::Int(64),
        "float32" => Ty::Float(32),
        "float64" => Ty::Float(64),
        "bool" => Ty::Bool,
        "char" => Ty::Char,
        "rune" => Ty::Rune,
        "string" => Ty::Str,
        "error" => Ty::Error,
        "thread" => Ty::Thread,
        "void" => Ty::Unit,
        _ => Ty::Named(n.to_string()),
    }
}

/// The return type of a builtin that carries one, so a destructure or an error
/// method on its result types precisely. Builtins without an entry stay
/// permissive and infer to Unknown.
fn builtin_ret(name: &str) -> Option<Ty> {
    match name {
        "read_file" | "read_line" | "read_all" => Some(Ty::Tuple(vec![Ty::Str, Ty::Error])),
        "parse_float" => Some(Ty::Tuple(vec![Ty::Float(64), Ty::Error])),
        "write_file" => Some(Ty::Error),
        "cstr" => Some(Ty::Str),
        _ => None,
    }
}

/// A short human name for a type in diagnostics.
fn ty_str(t: &Ty) -> String {
    match t {
        Ty::Int(0) => "an integer literal".to_string(),
        Ty::Int(w) => format!("int{w}"),
        Ty::Float(0) => "a float literal".to_string(),
        Ty::Float(w) => format!("float{w}"),
        Ty::Bool => "bool".to_string(),
        Ty::Char => "char".to_string(),
        Ty::Rune => "rune".to_string(),
        Ty::Str => "string".to_string(),
        Ty::Unit => "void".to_string(),
        Ty::Error => "error".to_string(),
        Ty::Thread => "a thread handle".to_string(),
        Ty::Ptr(_) => "a managed pointer".to_string(),
        Ty::RawPtr(_) => "a raw pointer".to_string(),
        Ty::Slice(_) => "a slice".to_string(),
        Ty::Array(..) => "an array".to_string(),
        Ty::Tuple(_) => "a tuple".to_string(),
        Ty::Named(n) => format!("'{n}'"),
        Ty::Func(..) => "a function".to_string(),
        Ty::Future(_) => "a future".to_string(),
        Ty::Collector(_) => "a collector".to_string(),
        Ty::Unknown => "an unknown type".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn errs(src: &str) -> Vec<Diagnostic> {
        let (t, le) = lex(src);
        assert!(le.is_empty(), "lex errors: {le:?}");
        let (m, pe) = parse(t);
        assert!(pe.is_empty(), "parse errors: {pe:?}");
        let escape = crate::sema::summary::compute(&m);
        check(&m, &escape).0
    }

    /// The whole sema pipeline: surface typeck, monomorphization, and the ground
    /// re-check the mono-expanded module goes through. A bare async-call future
    /// pinning a generic type parameter is only mistyped once mono runs, and the
    /// mismatch it caused surfaced in the ground re-check, so that class of bug is
    /// invisible to the surface-only `errs` and needs the full run.
    fn full(src: &str) -> Vec<Diagnostic> {
        let (t, le) = lex(src);
        assert!(le.is_empty(), "lex errors: {le:?}");
        let (m, pe) = parse(t);
        assert!(pe.is_empty(), "parse errors: {pe:?}");
        crate::sema::check(&m).0
    }

    #[test]
    fn arithmetic_mismatch() {
        let e = errs("func f() -> int64 { return 1 + true }");
        assert!(!e.is_empty());
    }

    #[test]
    fn bitwise_ops_typecheck() {
        let e = errs("func f() -> int64 { return (12 & 10) | (3 ^ 1) }");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn bitwise_width_mix_rejected() {
        let e = errs("func f() -> int64 {\n  x: int32 = 5\n  y: int64 = 7\n  return x & y\n}");
        assert!(
            e.iter()
                .any(|d| d.msg == "'&' mixes int32 and int64; match the widths"),
            "{e:?}"
        );
    }

    #[test]
    fn bitwise_on_bool_rejected() {
        let e = errs("func f() -> bool { return true & false }");
        assert!(
            e.iter()
                .any(|d| d.msg == "bitwise operators need integer operands"),
            "{e:?}"
        );
    }

    #[test]
    fn shift_on_float_rejected() {
        let e = errs("func f() -> int64 {\n  x: float64 = 1.0\n  return x << 2\n}");
        assert!(
            e.iter()
                .any(|d| d.msg == "shift operators need integer operands"),
            "{e:?}"
        );
    }

    #[test]
    fn bitnot_on_float_rejected() {
        let e = errs("func f() -> float64 { return ~1.5 }");
        assert!(
            e.iter().any(|d| d.msg == "'~' needs an integer operand"),
            "{e:?}"
        );
    }

    #[test]
    fn negative_constant_shift_rejected() {
        let e = errs("func f() -> int64 { return 1 << -1 }");
        assert!(
            e.iter().any(|d| d.msg == "shift amount is negative"),
            "{e:?}"
        );
    }

    #[test]
    fn oversize_constant_shift_rejected() {
        let e = errs("func f() -> int32 {\n  x: int32 = 1\n  return x << 32\n}");
        assert!(
            e.iter()
                .any(|d| d.msg == "shift amount 32 is out of range for int32"),
            "{e:?}"
        );
    }

    #[test]
    fn shift_by_width_of_wide_int_ok() {
        // `1 << 32` on two adaptable literals defaults to int64, where 32 is in
        // range, so no error fires.
        let e = errs("func f() -> int64 { return 1 << 32 }");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn pow_int_and_float_typecheck() {
        let e = errs(
            "func f() -> int64 { return 2 ** 10 }\n\
             func g() -> float64 { return 2.0 ** 3.0 }",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn pow_mixing_int_and_float_rejected() {
        let e = errs("func f() -> int64 {\n  x: int64 = 2\n  return x ** 2.0\n}");
        assert!(
            e.iter()
                .any(|d| d.msg == "'**' needs two operands of the same numeric type"),
            "{e:?}"
        );
    }

    #[test]
    fn pow_negative_constant_exponent_rejected() {
        let e = errs("func f() -> int64 { return 2 ** -1 }");
        assert!(
            e.iter()
                .any(|d| d.msg == "'**' on integers needs a nonnegative exponent"),
            "{e:?}"
        );
    }

    #[test]
    fn pow_float_negative_exponent_ok() {
        // A float exponent may be negative; only integer `**` requires it be
        // nonnegative.
        let e = errs("func f() -> float64 { return 2.0 ** -1.0 }");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn compound_assignment_width_mix_rejected() {
        let e =
            errs("func f() -> void {\n  mut x: int32 = 1\n  y: int64 = 2\n  x += y\n  return\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("mixes int32 and int64")),
            "{e:?}"
        );
    }

    #[test]
    fn compound_assignment_ok() {
        let e = errs(
            "func f() -> void {\n  mut x: int64 = 1\n  x += 2\n  x <<= 3\n  x &= 7\n  return\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn compound_oversize_shift_rejected() {
        let e = errs("func f() -> void {\n  mut x: int32 = 1\n  x <<= 40\n  return\n}");
        assert!(
            e.iter()
                .any(|d| d.msg == "shift amount 40 is out of range for int32"),
            "{e:?}"
        );
    }

    #[test]
    fn foreign_managed_pointer_rejected() {
        let e = errs(
            "foreign \"C\" { func bad(p: *int64) -> int32 }\nfunc main() -> int32 { return 0 }",
        );
        assert!(
            e.iter()
                .any(|d| d.msg.contains("managed pointer at the C boundary")),
            "{e:?}"
        );
    }

    #[test]
    fn foreign_bad_abi_rejected() {
        let e = errs(
            "foreign \"Rust\" { func abs(n: int32) -> int32 }\nfunc main() -> int32 { return 0 }",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("only \"C\" is supported")),
            "{e:?}"
        );
    }

    #[test]
    fn foreign_raw_pointer_ok() {
        let e = errs(
            "foreign \"C\" { func memset(dst: *raw int8, c: int32, n: int64) -> *void }\n\
             func main() -> int32 { return 0 }",
        );
        assert!(
            e.is_empty(),
            "raw pointer boundary should be accepted: {e:?}"
        );
    }

    #[test]
    fn struct_literal_unknown_field_rejected() {
        let e =
            errs("struct S { x: int64 }\nfunc main() -> int32 {\n  s := S { y: 5 }\n  return 0\n}");
        assert!(e.iter().any(|d| d.msg.contains("no field 'y'")), "{e:?}");
    }

    #[test]
    fn struct_literal_missing_field_rejected() {
        let e = errs("struct S { x: int64, y: int64 }\nfunc main() -> int32 {\n  s := S { x: 1 }\n  return 0\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("missing field 'y'")),
            "{e:?}"
        );
    }

    #[test]
    fn struct_literal_complete_ok() {
        let e = errs("struct S { x: int64, y: int64 }\nfunc main() -> int32 {\n  s := S { x: 1, y: 2 }\n  return 0\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn passing_struct_without_impl_is_rejected() {
        let e = errs("interface I { get() -> int64 }\nstruct S { x: int64 }\nfunc take(i: I) -> int64 { return i.get() }\nfunc f() -> void {\n  s := S { x: 1 }\n  take(s)\n}");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("does not implement interface 'I'")),
            "{e:?}"
        );
    }

    #[test]
    fn incomplete_impl_is_rejected() {
        let e = errs("interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { }");
        assert!(
            e.iter().any(|d| d.msg.contains("missing method 'get'")),
            "{e:?}"
        );
    }

    #[test]
    fn struct_with_impl_satisfies_interface() {
        let e = errs("interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return self.x } }\nfunc take(i: I) -> int64 { return i.get() }\nfunc f() -> void {\n  s := S { x: 1 }\n  take(s)\n}");
        assert!(
            !e.iter().any(|d| d.msg.contains("implement interface")),
            "{e:?}"
        );
    }

    #[test]
    fn discarding_a_fallible_call_is_rejected() {
        let e = errs("func fail() -> error { return error { message: \"x\" } }\nfunc f() -> void {\n  fail()\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("error result is ignored")),
            "{e:?}"
        );
    }

    #[test]
    fn handling_a_fallible_result_is_ok() {
        let e = errs("func fail() -> error { return error { message: \"x\" } }\nfunc f() -> void {\n  e := fail()\n  e.ignore()\n}");
        assert!(
            !e.iter().any(|d| d.msg.contains("error result is ignored")),
            "{e:?}"
        );
    }

    #[test]
    fn condition_must_be_bool() {
        let e = errs("func f() -> void {\n  if 5 {\n    return\n  }\n  return\n}");
        assert!(e.iter().any(|d| d.msg.contains("bool")));
    }

    #[test]
    fn return_type_mismatch() {
        let e = errs("func f() -> bool { return 5 }");
        assert!(!e.is_empty());
    }

    #[test]
    fn arg_count_mismatch() {
        let e =
            errs("func g(a: int64) -> int64 { return a }\nfunc f() -> int64 { return g(1, 2) }");
        assert!(e.iter().any(|d| d.msg.contains("argument")));
    }

    #[test]
    fn deref_non_pointer() {
        let e = errs("func f() -> int64 {\n  x := 5\n  return *x\n}");
        assert!(e.iter().any(|d| d.msg.contains("dereference")));
    }

    #[test]
    fn copying_an_owner_is_rejected() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc(5)\n  q: *int64 = p\n  free(q)\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("copy an owning pointer")),
            "{e:?}"
        );
    }

    #[test]
    fn use_after_move_is_rejected() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc(5)\n  q: *int64 = move(p)\n  free(p)\n  free(q)\n}");
        assert!(e.iter().any(|d| d.msg.contains("moved pointer")), "{e:?}");
    }

    #[test]
    fn freeing_a_borrow_is_rejected() {
        let e = errs("func sink(p: *int64) -> void {\n  free(p)\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("borrowed pointer")),
            "{e:?}"
        );
    }

    #[test]
    fn ref_alias_and_move_are_allowed() {
        // A ref aliases without copying ownership, and move transfers it; neither
        // is an ownership error, so the only diagnostics here are unrelated.
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(5)\n  ref r: *int64 = p\n  println(*r)\n  q: *int64 = move(p)\n  free(q)\n}",
        );
        assert!(
            !e.iter().any(|d| d.msg.contains("owning")
                || d.msg.contains("moved pointer")
                || d.msg.contains("borrowed pointer")),
            "{e:?}"
        );
    }

    #[test]
    fn assignment_copy_of_owner_is_rejected() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc(5)\n  mut q: *int64 = alloc(9)\n  q = p\n  free(p)\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("copy an owning pointer")),
            "{e:?}"
        );
    }

    #[test]
    fn reassigning_an_owner_is_rejected() {
        let e =
            errs("func f() -> void {\n  mut p: *int64 = alloc(5)\n  p = alloc(9)\n  free(p)\n}");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("reassign an owning pointer")),
            "{e:?}"
        );
    }

    #[test]
    fn reassigning_a_borrow_cursor_is_allowed() {
        // A borrowing cursor, like a list walk variable, is not an owner, so it
        // may advance without tripping the reassignment or copy rules.
        let e = errs("func walk(head: *int64) -> void {\n  mut cur: *int64 = head\n  cur = head\n  println(*cur)\n}");
        assert!(
            !e.iter()
                .any(|d| d.msg.contains("reassign") || d.msg.contains("owning")),
            "{e:?}"
        );
    }

    #[test]
    fn moving_a_borrow_is_rejected() {
        let e = errs("func sink(p: *int64) -> void {\n  q: *int64 = move(p)\n  free(q)\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("move a borrowed pointer")),
            "{e:?}"
        );
    }

    #[test]
    fn inferred_alloc_binding_is_an_owner() {
        // alloc infers to a managed pointer, so the inferred `:=` form is tracked
        // and a copy of it is the single owner violation.
        let e = errs("func f() -> void {\n  x := alloc(5)\n  y := x\n  free(x)\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("copy an owning pointer")),
            "{e:?}"
        );
    }

    #[test]
    fn a_conditional_move_does_not_leak_past_the_branch() {
        // The static pass is single block: a move inside an if does not invalidate
        // the binding after the branch; the runtime generation check backstops it.
        let e = errs("func f() -> void {\n  p: *int64 = alloc(5)\n  if true {\n    q: *int64 = move(p)\n    free(q)\n    return\n  }\n  free(p)\n}");
        assert!(!e.iter().any(|d| d.msg.contains("moved pointer")), "{e:?}");
    }

    #[test]
    fn slice_into_local_array_cannot_escape() {
        let e = errs("func f() -> int64[] {\n  xs: int64[3] = [1, 2, 3]\n  return xs[0..3]\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("escapes its frame")),
            "{e:?}"
        );
    }

    #[test]
    fn capturing_closure_cannot_escape() {
        let e = errs(
            "func f() -> (int64) -> int64 {\n  x: int64 = 10\n  return lambda (n: int64) -> int64 { return n + x }\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("escapes its frame")),
            "{e:?}"
        );
    }

    #[test]
    fn returning_a_slice_of_a_slice_parameter_is_allowed() {
        let e = errs("func f(xs: int64[]) -> int64[] {\n  return xs[0..2]\n}");
        assert!(!e.iter().any(|d| d.msg.contains("escapes")), "{e:?}");
    }

    // The method-receiver sink source, shared by the reject and the two accept
    // twins below: a Cell method `ship` sends its by-pointer receiver over a
    // channel. The receiver is the method's implicit parameter 0, so a polluted
    // receiver crosses a thread boundary the receiver thread outlives.
    const METHOD_SINK_SRC: &str = "struct Cell { rows: int64[] }\n\
         impl Cell { func ship(ch: Channel<*Cell>) -> void {\n\
           e := chan_send(ch, self)\n\
           e.ignore()\n\
         } }\n\
         func heap4() -> int64[] {\n\
           seed: int64[4] = [1, 2, 3, 4]\n\
           return map(seed[0..4], lambda (x: int64) -> int64 { return x })\n\
         }\n\
         func stash(s: int64[], c: *Cell) -> void { (*c).rows = s }\n";

    #[test]
    fn a_method_that_sends_a_polluted_receiver_over_a_channel_is_rejected() {
        // The method-receiver twin of escchan_helper: the send lives inside a
        // method whose hidden receiver is the sent value. A store edge polluted the
        // receiver's heap object with a frame view, and the call c.ship(ch) threads
        // that receiver as effective argument 0, so the self-sink method is rejected
        // on the polluted receiver exactly as a direct chan_send would be.
        let src = format!(
            "{METHOD_SINK_SRC}\
             func send_one(ch: Channel<*Cell>) -> void {{\n\
               c: *Cell = alloc(Cell {{ rows: heap4() }})\n\
               local: int64[4] = [111, 222, 333, 444]\n\
               stash(local[0..4], c)\n\
               c.ship(ch)\n\
             }}"
        );
        let e = errs(&src);
        assert!(
            e.iter()
                .any(|d| d.msg.contains("'ship' sends 'c' across a channel")),
            "{e:?}"
        );
    }

    #[test]
    fn a_method_that_sends_a_clean_receiver_over_a_channel_is_allowed() {
        // The accept twin: nothing on the sending frame is stashed into the cell, so
        // the receiver carries no frame view and the self-sink method call is legal.
        // The receiver-sink check finds the receiver clean and passes it.
        let src = format!(
            "{METHOD_SINK_SRC}\
             func send_one(ch: Channel<*Cell>) -> void {{\n\
               c: *Cell = alloc(Cell {{ rows: heap4() }})\n\
               c.ship(ch)\n\
             }}"
        );
        let e = errs(&src);
        assert!(
            !e.iter().any(|d| d.msg.contains("across a channel")),
            "{e:?}"
        );
    }

    #[test]
    fn a_frame_view_stored_through_a_method_receiver_pollutes_it_and_a_later_send_is_rejected() {
        // The method-flow twin: a method `fill(s) { (*self).rows = s }` stores its
        // slice parameter through the receiver. Called with a frame-local slice, the
        // store edge (parameter 1 into self, parameter 0) pollutes the receiver's
        // binding, so a later send of that receiver is caught. Proves the method
        // call routes its receiver through the store-edge flow, not only the sink.
        let src = "struct Cell { rows: int64[] }\n\
             impl Cell { func fill(s: int64[]) -> void { (*self).rows = s } }\n\
             func heap4() -> int64[] {\n\
               seed: int64[4] = [1, 2, 3, 4]\n\
               return map(seed[0..4], lambda (x: int64) -> int64 { return x })\n\
             }\n\
             func send_one(ch: Channel<*Cell>) -> void {\n\
               c: *Cell = alloc(Cell { rows: heap4() })\n\
               local: int64[4] = [111, 222, 333, 444]\n\
               c.fill(local[0..4])\n\
               e := chan_send(ch, c)\n\
               e.ignore()\n\
             }";
        let e = errs(src);
        assert!(
            e.iter()
                .any(|d| d.msg.contains("chan_send cannot send 'c'")),
            "{e:?}"
        );
    }

    #[test]
    fn returning_a_non_capturing_closure_is_allowed() {
        let e = errs(
            "func f() -> (int64) -> int64 {\n  return lambda (n: int64) -> int64 { return n + 1 }\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("escapes")), "{e:?}");
    }

    #[test]
    fn exhaustive_match_clean() {
        let e = errs(
            "enum E { A, B, C }\n\
             func f(e: E) -> int64 {\n  match e {\n    A => return 1,\n    B => return 2,\n    C => return 3,\n  }\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn non_exhaustive_match_errors() {
        let e = errs(
            "enum E { A, B, C }\n\
             func f(e: E) -> int64 {\n  match e {\n    A => return 1,\n    B => return 2,\n  }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("non exhaustive")), "{e:?}");
    }

    #[test]
    fn wildcard_makes_match_exhaustive() {
        let e = errs(
            "enum E { A, B, C }\n\
             func f(e: E) -> int64 {\n  match e {\n    A => return 1,\n    _ => return 0,\n  }\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn unreachable_arm_after_catch_all_errors() {
        let e = errs(
            "enum E { A, B }\n\
             func f(e: E) -> int64 {\n  match e {\n    _ => return 0,\n    A => return 1,\n  }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("unreachable")), "{e:?}");
    }

    #[test]
    fn good_program_clean() {
        let e = errs(
            "func add(a: int64, b: int64) -> int64 { return a + b }\n\
             func f() -> int64 {\n  x := add(1, 2)\n  if x == 3 {\n    return x\n  }\n  return 0\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn mixed_integer_widths_are_rejected() {
        let e = errs(
            "func f() -> void {\n  a: int32 = 5\n  b: int64 = 9\n  c := a + b\n  println(c)\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("mixes int32 and int64")),
            "{e:?}"
        );
    }

    #[test]
    fn literal_adapts_to_any_width() {
        let e = errs("func f() -> int32 {\n  a: int32 = 5\n  b := a + 1\n  return b\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn cross_width_assignment_is_rejected() {
        let e = errs("func f(w: int64) -> void {\n  x: int8 = w\n  println(x)\n}");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("annotation that does not match")),
            "{e:?}"
        );
    }

    #[test]
    fn literal_too_wide_for_annotation_is_rejected() {
        let e = errs("func f() -> void {\n  x: int8 = 300\n  println(x)\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("does not fit in 8 bits")),
            "{e:?}"
        );
    }

    #[test]
    fn suffixed_literal_out_of_range_is_rejected() {
        let e = errs("func f() -> void {\n  x := 300i8\n  println(x)\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("does not fit in 8 bits")),
            "{e:?}"
        );
    }

    #[test]
    fn signed_int8_bound_rejects_128_accepts_127() {
        let bad = errs("func f() -> void {\n  x: int8 = 128\n  println(x)\n}");
        assert!(
            bad.iter().any(|d| d.msg.contains("does not fit in 8 bits")),
            "{bad:?}"
        );
        let good = errs("func f() -> void {\n  x: int8 = 127\n  println(x)\n}");
        assert!(good.is_empty(), "{good:?}");
    }

    #[test]
    fn enum_constructor_payload_ranges_against_its_width() {
        let e = errs(
            "enum E { Has(v: int32), Nope }\nfunc f() -> void {\n  e := E.Has(4294967297)\n  match e { Has(v) => println(v), Nope => println(0) }\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("does not fit in 32 bits")),
            "{e:?}"
        );
    }

    #[test]
    fn a_parameter_of_an_undeclared_type_is_rejected() {
        let e = errs("func work(c: Collector) -> int64 { return 0 }");
        assert!(
            e.iter().any(|d| d.msg.contains("unknown type 'Collector'")),
            "{e:?}"
        );
    }

    #[test]
    fn a_reserved_unsigned_type_in_a_parameter_is_rejected() {
        let e = errs("func work(c: uint8) -> int64 { return 0 }");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("unsigned integers are reserved")),
            "{e:?}"
        );
    }

    #[test]
    fn binding_a_whole_fallible_tuple_is_rejected() {
        let bad = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\nfunc f() -> void {\n  p := fail()\n  sink(p)\n}\nfunc sink(t: (int64, error)) -> void { }",
        );
        assert!(
            bad.iter().any(|d| d.msg.contains("must be destructured")),
            "{bad:?}"
        );
        let good = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\nfunc f() -> void {\n  v, e := fail()\n  e.ignore()\n  println(v)\n}",
        );
        assert!(good.is_empty(), "{good:?}");
    }

    #[test]
    fn a_method_on_an_enum_local_is_rejected() {
        let e = errs(
            "enum Maybe { Just(v: int64), Nothing }\nfunc f() -> void {\n  m := Maybe.Just(5)\n  println(m.unwrap())\n}",
        );
        assert!(
            e.iter().any(|d| d
                .msg
                .contains("methods on the enum 'Maybe' are not supported")),
            "{e:?}"
        );
    }

    #[test]
    fn a_functional_builtin_arity_is_ranged() {
        let e = errs(
            "func f() -> void {\n  xs: int64[3] = [1, 2, 3]\n  r := fold(xs[0..3], 0, lambda (a: int64, x: int64) -> int64 { return a + x }, 9)\n  println(r)\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("fold takes 3 argument(s)")),
            "{e:?}"
        );
    }

    #[test]
    fn a_loop_pumping_call_in_an_async_func_is_rejected() {
        let e = errs(
            "async func w() -> int64 {\n  f: Future<int64> = future_new()\n  v, e := try_poll(f)\n  e.ignore()\n  return v\n}\nfunc main() -> int32 { return 0 }",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("pumps the event loop")),
            "{e:?}"
        );
    }

    #[test]
    fn returning_an_array_literal_as_a_slice_is_rejected() {
        let e = errs("func make() -> int64[] { return [1, 2, 3] }");
        assert!(
            e.iter().any(|d| d.msg.contains("escapes its frame")),
            "{e:?}"
        );
    }

    #[test]
    fn unsized_alloc_needs_a_pointer_annotation() {
        let e = errs("func f() -> void {\n  x := alloc()\n  free(x)\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("pointer type annotation")),
            "{e:?}"
        );
    }

    #[test]
    fn unsized_alloc_with_pointer_annotation_is_ok() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc()\n  *p = 5\n  free(p)\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn element_store_into_immutable_array_is_rejected() {
        let e = errs(
            "func f() -> void {\n  xs: int64[3] = [1, 2, 3]\n  xs[0] = 99\n  println(xs[0])\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg.contains("assign through immutable 'xs'")),
            "{e:?}"
        );
    }

    #[test]
    fn field_store_into_immutable_struct_is_rejected() {
        let e = errs(
            "struct P { x: int64 }\nfunc f() -> void {\n  p := P { x: 1 }\n  p.x = 42\n  println(p.x)\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg.contains("assign through immutable 'p'")),
            "{e:?}"
        );
    }

    #[test]
    fn element_store_into_mut_array_is_ok() {
        let e = errs(
            "func f() -> void {\n  mut xs: int64[3] = [1, 2, 3]\n  xs[0] = 99\n  println(xs[0])\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn store_through_a_pointer_deref_is_ok() {
        // Mutation through a pointer writes the pointee, not the binding, so the
        // binding's immutability does not apply.
        let e = errs("struct P { x: int64 }\nfunc f(p: *P) -> void {\n  (*p).x = 42\n}");
        assert!(!e.iter().any(|d| d.msg.contains("immutable")), "{e:?}");
    }

    #[test]
    fn field_store_through_pointer_needs_explicit_deref() {
        let e = errs("struct P { x: int64 }\nfunc f(p: *P) -> void {\n  p.x = 42\n}");
        assert!(
            e.iter().any(|d| d.msg.contains("explicit dereference")),
            "{e:?}"
        );
    }

    #[test]
    fn binding_a_struct_to_an_interface_needs_an_impl() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nfunc f() -> void {\n  s := S { x: 7 }\n  i: I = s\n  println(i.get())\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg.contains("does not implement interface 'I'")),
            "{e:?}"
        );
    }

    #[test]
    fn binding_a_struct_to_an_interface_with_impl_is_ok() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return self.x } }\nfunc f() -> void {\n  s := S { x: 7 }\n  i: I = s\n  println(i.get())\n}",
        );
        assert!(
            !e.iter().any(|d| d.msg.contains("implement interface")),
            "{e:?}"
        );
    }

    #[test]
    fn returning_a_struct_as_an_interface_needs_an_impl() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nfunc mk() -> I {\n  return S { x: 7 }\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg.contains("does not implement interface 'I'")),
            "{e:?}"
        );
    }

    #[test]
    fn match_over_a_non_enum_is_rejected() {
        let e = errs(
            "func f() -> void {\n  x := 5\n  match x {\n    a => println(1),\n    b => println(2),\n  }\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg.contains("match needs an enum value")),
            "{e:?}"
        );
    }

    #[test]
    fn defer_inside_a_conditional_is_rejected() {
        let e = errs("func f() -> void {\n  if true {\n    defer println(1)\n  }\n  println(2)\n}");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("defer inside a conditional or loop")),
            "{e:?}"
        );
    }

    #[test]
    fn defer_at_function_top_level_is_ok() {
        let e = errs("func f() -> void {\n  defer println(1)\n  println(2)\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn printing_a_bound_error_does_not_handle_it() {
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func f() -> void {\n  v, e := fail()\n  println(v)\n  println(e)\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("'e' is never handled")),
            "{e:?}"
        );
    }

    #[test]
    fn reading_the_message_field_does_not_handle_the_error() {
        // `e.message` reads the string out of the error but does not inspect,
        // check, or discard it, so the must-handle obligation stays pending.
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func f() -> void {\n  v, e := fail()\n  println(v)\n  s := e.message\n  println(s)\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("'e' is never handled")),
            "{e:?}"
        );
    }

    #[test]
    fn reading_the_message_field_typechecks_as_a_string() {
        // With the error separately discharged, `e.message` binds to a string.
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func f() -> void {\n  v, e := fail()\n  println(v)\n  s: string = e.message\n  println(s)\n  e.ignore()\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn an_unknown_field_on_an_error_is_rejected() {
        // An error carries only `message`; any other field name is a clear
        // error rather than a silent zero from codegen.
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func f() -> void {\n  v, e := fail()\n  println(v)\n  n := e.code\n  println(n)\n  e.ignore()\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg == "error has no field 'code'; it carries only 'message'"),
            "{e:?}"
        );
    }

    #[test]
    fn assigning_to_an_error_message_is_rejected() {
        // An error's message has no writable place; the store would be silently
        // dropped in codegen, so the write is refused. FIX-B.
        let e = errs(
            "func f() -> void {\n  mut e := error { message: \"x\" }\n  e.message = \"y\"\n  e.ignore()\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg == "an error's message is read only; build a new error instead"),
            "{e:?}"
        );
    }

    #[test]
    fn a_slice_of_structs_at_an_interface_receiver_method_is_rejected() {
        // FIX-A: an interface-typed receiver is fixed to Unknown in scope, but its
        // interface name is recovered from the binding record to run the covariance
        // guard, so a slice of concrete structs at the method's slice-of-interface
        // parameter is caught at check.
        let e = errs(
            "interface Shape { area() -> int64 }\n\
             interface Summer { total(shapes: Shape[]) -> int64 }\n\
             struct Sq { s: int64 }\n\
             impl Shape for Sq { func area() -> int64 { return self.s } }\n\
             struct Calc { b: int64 }\n\
             impl Summer for Calc { func total(shapes: Shape[]) -> int64 { return shapes[0].area() } }\n\
             func go(s: Summer, arr: Sq[]) -> int64 { return s.total(arr) }",
        );
        assert!(
            e.iter().any(|d| d
                .msg
                .contains("cannot pass a slice of 'Sq' as a slice of interface 'Shape'")),
            "{e:?}"
        );
    }

    #[test]
    fn exists_check_ignore_and_return_all_handle_an_error() {
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func a() -> void {\n  v, e := fail()\n  println(v)\n  if e.exists() {\n    return\n  }\n}\n\
             func b() -> void {\n  v, e := fail()\n  println(v)\n  e.ignore()\n}\n\
             func c() -> (int64, error) {\n  v, e := fail()\n  return (v, e)\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("never handled")), "{e:?}");
    }

    #[test]
    fn passing_an_error_to_an_error_parameter_handles_it() {
        // Handing a bound error to a parameter declared `error` discharges the
        // must-handle obligation, mirroring a returned error.
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func sink(err: error) -> void { err.ignore() }\n\
             func f() -> void {\n  v, e := fail()\n  println(v)\n  sink(e)\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("never handled")), "{e:?}");
    }

    #[test]
    fn an_error_passed_to_a_non_error_parameter_is_still_unhandled() {
        // The reject twin: passing the error's sibling value to a plain parameter
        // does not discharge the error, so it is still rejected.
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func take(n: int64) -> void { println(n) }\n\
             func f() -> void {\n  v, e := fail()\n  take(v)\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("'e' is never handled")),
            "{e:?}"
        );
    }

    #[test]
    fn an_error_laundered_through_a_generic_call_is_still_unhandled() {
        // FIX-C: the discharge is narrowed to a bare error binding at the argument.
        // `sink(fst(e, e2))` hands sink the passthrough result but never hands off
        // either error, so both stay pending. Without the narrowing the whole-arg
        // walk would clear e and e2 and let the laundered errors escape.
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func fst<T, U>(a: T, b: U) -> T { return a }\n\
             func sink(err: error) -> void { err.ignore() }\n\
             func f() -> void {\n  v, e := fail()\n  w, e2 := fail()\n  println(v)\n  println(w)\n  sink(fst(e, e2))\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("'e' is never handled")),
            "{e:?}"
        );
        assert!(
            e.iter().any(|d| d.msg.contains("'e2' is never handled")),
            "{e:?}"
        );
    }

    #[test]
    fn an_error_parameter_dropped_by_the_callee_is_rejected() {
        // FIX-1: an error parameter carries the same must-handle obligation a
        // let-bound error does, so a callee that never inspects it is rejected.
        let e = errs("func swallow(err: error) -> void { }");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("the error 'err' is never handled")),
            "{e:?}"
        );
    }

    #[test]
    fn an_error_parameter_inspected_by_the_callee_is_clean() {
        // FIX-1 accept twin: inspecting the error parameter discharges its
        // obligation, so no report fires.
        let e =
            errs("func check_it(err: error) -> int64 { if err.exists() { return 1 }\n  return 0 }");
        assert!(!e.iter().any(|d| d.msg.contains("never handled")), "{e:?}");
    }

    #[test]
    fn an_error_parameter_handed_off_again_is_clean() {
        // FIX-1: re-handing the error parameter to another error parameter is a
        // valid discharge, mirroring the return hand-off.
        let e = errs(
            "func sink(err: error) -> void { err.ignore() }\n\
             func relay(err: error) -> void { sink(err) }",
        );
        assert!(!e.iter().any(|d| d.msg.contains("never handled")), "{e:?}");
    }

    #[test]
    fn passing_an_error_to_an_error_method_parameter_handles_it() {
        // FIX-D: an error binding handed to a method parameter declared `error`
        // discharges the obligation, the same as a bare or indirect call.
        let e = errs(
            "@paradigm oop\n@paradigm procedural\n\
             struct Log { n: int64 }\n\
             impl Log {\n  func note(err: error) -> void { err.ignore() }\n}\n\
             func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func f() -> void {\n  l := Log { n: 1 }\n  v, e := fail()\n  println(v)\n  l.note(e)\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("never handled")), "{e:?}");
    }

    #[test]
    fn printing_a_struct_without_display_is_rejected() {
        let e =
            errs("struct P { x: int64 }\nfunc f() -> void {\n  p := P { x: 1 }\n  println(p)\n}");
        assert!(e.iter().any(|d| d.msg.contains("no Display impl")), "{e:?}");
    }

    #[test]
    fn printing_a_struct_with_display_is_ok() {
        let e = errs(
            "@paradigm oop\n@paradigm procedural\ninterface Display { toString() -> string }\nstruct P { x: int64 }\nimpl Display for P { func toString() -> string { return \"P\" } }\nfunc f() -> void {\n  p := P { x: 1 }\n  println(p)\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("Display")), "{e:?}");
    }

    #[test]
    fn single_argument_format_string_is_checked() {
        let e = errs("func f() -> void {\n  println(\"{}\")\n}");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("1 hole(s) but 0 argument(s)")),
            "{e:?}"
        );
    }

    #[test]
    fn missing_return_on_a_path_is_rejected() {
        let e = errs("func f() -> int64 {\n  println(1)\n}");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("not all paths in 'f' return")),
            "{e:?}"
        );
    }

    #[test]
    fn returns_in_both_branches_satisfy_missing_return() {
        let e = errs(
            "func f(c: bool) -> int64 {\n  if c {\n    return 1\n  } else {\n    return 2\n  }\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn duplicate_impl_for_same_pair_is_rejected() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return 1 } }\nimpl I for S { func get() -> int64 { return 2 } }",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("duplicate 'impl I for S'")),
            "{e:?}"
        );
    }

    #[test]
    fn allocator_main_form_is_rejected_for_now() {
        let e =
            errs("func main(argc: int32, argv: string[], using a: int64) -> int32 { return 0 }");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("allocator form is not supported yet")),
            "{e:?}"
        );
    }

    #[test]
    fn argc_argv_main_is_accepted() {
        let e = errs("func main(argc: int32, argv: string[]) -> int32 { return argc }");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn spawn_and_join_type_check_clean() {
        let e = errs(
            "func f() -> void {\n  t, e := spawn(lambda () -> void {\n    println(1)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn spawn_needs_a_lambda_literal() {
        let e = errs(
            "func f() -> void {\n  g := lambda () -> void { println(1) }\n  t, e := spawn(g)\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("lambda literal")), "{e:?}");
    }

    #[test]
    fn spawn_lambda_must_be_nullary_void() {
        let e = errs(
            "func f() -> void {\n  t, e := spawn(lambda (n: int64) -> void { println(n) })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg.contains("no parameters and returns void")),
            "{e:?}"
        );
    }

    #[test]
    fn spawn_rejects_a_slice_capture() {
        let e = errs(
            "func f(xs: int64[]) -> void {\n  t, e := spawn(lambda () -> void {\n    println(xs[0])\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("cannot capture 'xs'")),
            "{e:?}"
        );
    }

    #[test]
    fn spawned_thread_cannot_free_a_captured_pointer() {
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(5)\n  t, e := spawn(lambda () -> void {\n    free(p)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n  free(p)\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("borrowed pointer")),
            "{e:?}"
        );
    }

    #[test]
    fn a_dropped_join_error_is_rejected() {
        let e = errs(
            "func f() -> void {\n  t, e := spawn(lambda () -> void { println(1) })\n  e.ignore()\n  join(t)\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("error result is ignored")),
            "{e:?}"
        );
    }

    #[test]
    fn spawn_rejects_a_slice_smuggled_in_a_struct_field() {
        let e = errs(
            "struct Wrap { s: int64[] }\nfunc f(xs: int64[]) -> void {\n  w := Wrap { s: xs }\n  t, e := spawn(lambda () -> void {\n    println(w.s[0])\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("cannot capture 'w'")),
            "{e:?}"
        );
    }

    #[test]
    fn spawn_rejects_an_interface_value_capture() {
        let e = errs(
            "@paradigm oop\ninterface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return self.x } }\nfunc f() -> void {\n  s := S { x: 1 }\n  i: I = s\n  t, e := spawn(lambda () -> void {\n    println(i.get())\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("cannot capture 'i'")),
            "{e:?}"
        );
    }

    #[test]
    fn spawn_capturing_a_moved_pointer_is_rejected() {
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(5)\n  q := move(p)\n  free(q)\n  t, e := spawn(lambda () -> void {\n    println(*p)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("moved pointer")), "{e:?}");
    }

    #[test]
    fn spawn_capturing_a_plain_struct_is_ok() {
        let e = errs(
            "struct P { x: int64, y: int64 }\nfunc f() -> void {\n  p := P { x: 1, y: 2 }\n  t, e := spawn(lambda () -> void {\n    println(p.x)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn printing_a_thread_handle_is_rejected() {
        let e = errs(
            "func f() -> void {\n  t, e := spawn(lambda () -> void { println(1) })\n  e.ignore()\n  println(t)\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("thread handle")), "{e:?}");
    }

    #[test]
    fn submit_of_a_closure_variable_is_rejected() {
        let e = errs(
            "func f() -> void {\n  g := lambda () -> void { println(1) }\n  e := submit(g)\n  e.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("lambda literal")), "{e:?}");
    }

    #[test]
    fn submit_with_a_slice_capture_is_rejected() {
        let e = errs(
            "func f() -> void {\n  xs: int64[3] = [1, 2, 3]\n  s: int64[] = xs[0..2]\n  e := submit(lambda () -> void {\n    println(s[0])\n  })\n  e.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("cannot capture")), "{e:?}");
    }

    #[test]
    fn submit_of_a_plain_task_is_ok() {
        let e = errs(
            "func f() -> void {\n  n := 5\n  e := submit(lambda () -> void {\n    println(n)\n  })\n  e.ignore()\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn submit_freeing_a_captured_pointer_is_rejected() {
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(5)\n  e := submit(lambda () -> void {\n    free(p)\n  })\n  e.ignore()\n  free(p)\n}",
        );
        assert!(!e.is_empty(), "a task must not free its borrow");
    }

    #[test]
    fn raw_pointer_passes_where_void_pointer_is_expected() {
        let e = errs(
            "func take(p: *void) -> void { }\nfunc f() -> void {\n  b: *raw int8 = alloc_bytes(8)\n  take(b)\n  free(b)\n}",
        );
        assert!(e.is_empty(), "*raw int8 should pass as *void: {e:?}");
    }

    #[test]
    fn void_pointer_is_rejected_where_raw_pointer_is_expected() {
        // The reverse direction would let a managed pointer launder through
        // *void into a typed raw alias the generation check cannot see, so a
        // *void value binds a *void annotation only.
        let e = errs(
            "foreign \"C\" { func memset(dst: *raw int8, c: int32, n: int64) -> *void }\nfunc take(p: *raw int8) -> void { }\nfunc f() -> void {\n  b: *raw int8 = alloc_bytes(8)\n  v := memset(b, 0, 8)\n  take(v)\n  free(b)\n}",
        );
        assert!(!e.is_empty(), "*void must not pass as *raw int8");
    }

    #[test]
    fn managed_pointer_cannot_launder_to_raw_through_void() {
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(41)\n  v: *void = p\n  r: *raw int64 = v\n  free(p)\n}",
        );
        assert!(!e.is_empty(), "*void must not become a typed raw alias");
    }

    #[test]
    fn raw_pointer_still_rejected_for_a_managed_pointer() {
        let e = errs(
            "func take(p: *int64) -> void { }\nfunc f() -> void {\n  b: *raw int64 = alloc_bytes(8)\n  take(b)\n  free(b)\n}",
        );
        assert!(!e.is_empty(), "*raw int64 must not pass as managed *int64");
    }

    // A legal async program is the baseline every rejection test contrasts with.
    const ASYNC_PRELUDE: &str = "async func leaf() -> (int64, error) { return (1, error {}) }\n\
         async func val(x: int64) -> int64 { return x }\n";

    #[test]
    fn legal_async_program_checks_clean() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}\
             async func amain() -> int64 {{\n  v, e := await leaf()\n  e.ignore()\n  a := await val(v)\n  return a\n}}\n\
             func main() -> int32 {{\n  r := async_run(amain())\n  println(r)\n  return 0\n}}"
        ));
        assert!(
            e.is_empty(),
            "a legal async program must check clean: {e:?}"
        );
    }

    #[test]
    fn main_cannot_be_async() {
        let e = errs("async func main() -> int32 { return 0 }");
        assert!(e.iter().any(|d| d.msg == "main cannot be async; call an async func with async_run instead"), "{e:?}");
    }

    #[test]
    fn async_func_cannot_take_type_parameters() {
        let e = errs("async func g<T>(x: T) -> T { return x }");
        assert!(
            e.iter()
                .any(|d| d.msg == "an async func cannot take type parameters"),
            "{e:?}"
        );
    }

    #[test]
    fn async_func_cannot_take_a_slice_param() {
        let e = errs("async func g(xs: int64[]) -> void { return }");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("an async func cannot take 'xs'")),
            "{e:?}"
        );
    }

    #[test]
    fn async_func_cannot_take_a_future_param() {
        let e = errs("async func g(f: Future<int64>) -> void { return }");
        assert!(
            e.iter().any(|d| d.msg.contains(
                "a future belongs to the event loop thread; await it in the caller instead"
            )),
            "{e:?}"
        );
    }

    #[test]
    fn async_param_with_a_directly_buried_future_reports_the_future_message() {
        // A future buried in a non-generic struct field reports the future reason,
        // not the generic slice/closure/interface message.
        let e =
            errs("struct FBox { x: Future<int64> }\nasync func g(b: FBox) -> int64 { return 1 }");
        assert!(
            e.iter()
                .any(|d| d.msg.contains("an async func cannot take 'b'")
                    && d.msg.contains("a future belongs to the event loop thread")),
            "{e:?}"
        );
    }

    #[test]
    fn async_func_cannot_return_a_slice() {
        let e = errs("async func g() -> int64[] { return [] }");
        assert!(
            e.iter().any(|d| d.msg == "an async func cannot return a slice, closure, or interface value; the value would outlive the task frame it views"),
            "{e:?}"
        );
    }

    #[test]
    fn async_name_in_value_position_is_rejected() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}func main() -> int32 {{\n  h := val\n  return 0\n}}"
        ));
        assert!(
            e.iter()
                .any(|d| d.msg == "'val' is async; call it with await or start it with async_run"),
            "{e:?}"
        );
    }

    #[test]
    fn bare_async_call_is_never_awaited() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  val(3)\n  return 0\n}}"
        ));
        assert!(
            e.iter().any(|d| d.msg == "the future from 'val' is never awaited; bind it so it can be awaited or released"),
            "{e:?}"
        );
    }

    #[test]
    fn void_await_that_discards_a_value_is_rejected() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  await val(3)\n  return 0\n}}"
        ));
        assert!(
            e.iter()
                .any(|d| d.msg == "'await f' discards a value; bind it, as in v, e := await f"),
            "{e:?}"
        );
    }

    #[test]
    fn awaited_error_must_be_handled() {
        // The err word of a two-bind await is a pending error like any other.
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  a, e := await val(3)\n  return a\n}}"
        ));
        assert!(
            e.iter()
                .any(|d| d.msg.contains("the error 'e' is never handled")),
            "{e:?}"
        );
    }

    #[test]
    fn single_bind_await_discards_the_error_word() {
        // O1: a single-bind await is allowed and drops the completer's err word.
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  a := await val(3)\n  return a\n}}"
        ));
        assert!(e.is_empty(), "single-bind await must check clean: {e:?}");
    }

    #[test]
    fn async_run_inside_an_async_func_is_rejected() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  r := async_run(val(3))\n  return r\n}}"
        ));
        assert!(
            e.iter().any(|d| d.msg
                == "async_run cannot be called inside an async func; await the call instead"),
            "{e:?}"
        );
    }

    #[test]
    fn async_run_of_a_bound_future_is_rejected() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}func main() -> int32 {{\n  fa: Future<int64> = fnew()\n  r := async_run(fa)\n  println(r)\n  return 0\n}}"
        ));
        assert!(
            e.iter().any(|d| d.msg
                == "async_run takes a direct call of an async func, written at the call site"),
            "{e:?}"
        );
    }

    #[test]
    fn spawn_capturing_a_future_is_rejected() {
        let e = errs(
            "func main() -> int32 {\n  f: Future<int64> = fnew()\n  t, s := spawn(lambda () -> void {\n    use(f)\n  })\n  s.ignore()\n  return 0\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg
                    == "spawn cannot capture 'f': a future belongs to the event loop thread"),
            "{e:?}"
        );
    }

    #[test]
    fn submit_capturing_a_future_is_rejected() {
        let e = errs(
            "func main() -> int32 {\n  f: Future<int64> = fnew()\n  s := submit(lambda () -> void {\n    use(f)\n  })\n  s.ignore()\n  return 0\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg
                    == "submit cannot capture 'f': a future belongs to the event loop thread"),
            "{e:?}"
        );
    }

    #[test]
    fn spawn_capturing_a_future_behind_a_pointer_is_rejected() {
        // A typed future smuggled to a worker behind a managed pointer still lets
        // the worker await it off the loop thread, which faults at runtime. The
        // capture walk sees the future through the pointer and names the real
        // reason, not the slice rule.
        let e = errs(
            "func main() -> int32 {\n  f: Future<int64> = fnew()\n  p: *Future<int64> = alloc(f)\n  t, s := spawn(lambda () -> void {\n    use(p)\n  })\n  s.ignore()\n  return 0\n}",
        );
        assert!(
            e.iter()
                .any(|d| d.msg
                    == "spawn cannot capture 'p': a future belongs to the event loop thread"),
            "{e:?}"
        );
    }

    #[test]
    fn spawn_capturing_a_slice_behind_a_pointer_is_ok() {
        // A pointer to heap data whose fields hold a heap-backed slice is fine to
        // capture: the pointer targets the heap, which the generation check
        // covers, and nothing behind it views the spawning frame, so the
        // future-only pointer ban must not widen to reject it.
        let e = errs(
            "struct Holder { s: int64[] }\n\
             func main() -> int32 {\n  seed: int64[2] = [1, 2]\n  hs := map(seed[0..2], lambda (x: int64) -> int64 { return x })\n  h: Holder = Holder { s: hs }\n  p: *Holder = alloc(h)\n  t, s := spawn(lambda () -> void {\n    use(p)\n  })\n  s.ignore()\n  return 0\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("cannot capture")), "{e:?}");
    }

    #[test]
    fn spawn_capturing_a_pointer_whose_object_views_the_frame_is_rejected() {
        // The reject twin: the heap copy of the struct carries a slice that still
        // views the spawning frame's array, so the task can read the frame dead.
        // The capture check consults the binding's flow flags, not only its type.
        let e = errs(
            "struct Holder { s: int64[] }\n\
             func main() -> int32 {\n  h: Holder = Holder { s: [1, 2] }\n  p: *Holder = alloc(h)\n  t, s := spawn(lambda () -> void {\n    use(p)\n  })\n  s.ignore()\n  return 0\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("cannot capture 'p'")),
            "{e:?}"
        );
    }

    #[test]
    fn a_bare_future_pins_a_generic_type_parameter_as_a_future() {
        // A bare async-call future passed to a generic pins the parameter to
        // Future<ret>, not the unwrapped return, so the instantiation and the
        // ground re-check agree on the value's real shape. This was a false reject
        // reporting "argument 1 has the wrong type".
        let e = full(
            "struct Future<T> { h: *void, gen: int64 }\n\
             async func one() -> int64 { return 1 }\n\
             func hold<T>(x: T) -> T { return x }\n\
             async func amain() -> int32 {\n  f := one()\n  g := hold(f)\n  v := await g\n  println(v)\n  return 0\n}\n\
             func main() -> int32 {\n  rc := async_run(amain())\n  return rc\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("wrong type")), "{e:?}");
        assert!(
            e.is_empty(),
            "the bare-future generic pin must check clean: {e:?}"
        );
    }

    #[test]
    fn a_bare_future_passed_to_a_multi_argument_generic_checks_clean() {
        // Two future arguments pin the same parameter; first-wins unification and
        // the ground re-check must both see Future<int64> in each position.
        let e = full(
            "struct Future<T> { h: *void, gen: int64 }\n\
             async func one() -> int64 { return 1 }\n\
             func push2<T>(x: T, y: T) -> T { return x }\n\
             async func amain() -> int32 {\n  f := one()\n  g := one()\n  r := push2(f, g)\n  v := await r\n  println(v)\n  return 0\n}\n\
             func main() -> int32 {\n  rc := async_run(amain())\n  return rc\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("wrong type")), "{e:?}");
        assert!(
            e.is_empty(),
            "a future arg in a multi-parameter generic must check clean: {e:?}"
        );
    }

    #[test]
    fn a_direct_await_of_an_async_call_still_checks_clean() {
        // The direct-await path stays correct: static typing now reports the async
        // call as Future<ret>, which the await unwraps to the element.
        let e = full(
            "struct Future<T> { h: *void, gen: int64 }\n\
             async func one() -> int64 { return 1 }\n\
             async func amain() -> int32 {\n  v := await one()\n  println(v)\n  return 0\n}\n\
             func main() -> int32 {\n  rc := async_run(amain())\n  return rc\n}",
        );
        assert!(e.is_empty(), "a direct await must check clean: {e:?}");
    }

    const IFACE_TARG_MSG: &str =
        "an interface cannot be a generic type argument; generics are monomorphized over concrete types";

    #[test]
    fn interface_generic_arg_in_param_rejected() {
        let e = errs(
            "interface Speaker { speak() -> int64 }\n\
             struct Dog { name: int64 }\n\
             impl Speaker for Dog { func speak() -> int64 { return self.name } }\n\
             struct Box<T> { v: T }\n\
             func take(b: Box<Speaker>) -> int64 { return 0 }\n\
             func main() -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg == IFACE_TARG_MSG), "{e:?}");
    }

    #[test]
    fn interface_generic_arg_in_let_rejected() {
        let e = errs(
            "interface Speaker { speak() -> int64 }\n\
             struct Dog { name: int64 }\n\
             impl Speaker for Dog { func speak() -> int64 { return self.name } }\n\
             struct Box<T> { v: T }\n\
             func main() -> int32 {\n\
               b: Box<Speaker> = Box { v: Dog { name: 3 } }\n\
               println(b.v.speak())\n\
               return 0\n\
             }",
        );
        assert!(e.iter().any(|d| d.msg == IFACE_TARG_MSG), "{e:?}");
    }

    #[test]
    fn interface_generic_arg_buried_rejected() {
        // A burial one level deep, Box<Pair<Speaker, int64>>, is still an
        // interface standing in for a type parameter and is rejected.
        let e = errs(
            "interface Speaker { speak() -> int64 }\n\
             struct Pair<A, B> { first: A, second: B }\n\
             struct Box<T> { v: T }\n\
             func take(b: Box<Pair<Speaker, int64>>) -> int64 { return 0 }\n\
             func main() -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg == IFACE_TARG_MSG), "{e:?}");
    }

    #[test]
    fn concrete_generic_arg_accepted() {
        // A concrete type argument monomorphizes cleanly and must not be caught
        // by the interface rule.
        let e = errs(
            "struct Box<T> { v: T }\n\
             func main() -> int32 {\n\
               b: Box<int64> = Box { v: 5 }\n\
               println(b.v)\n\
               return 0\n\
             }",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn generic_type_parameter_is_not_an_interface() {
        // A bare type parameter T in a generic function's own signature is not an
        // interface and must pass, even though it shares the position an interface
        // argument would occupy.
        let e = errs(
            "struct Box<T> { v: T }\n\
             func wrap<T>(x: T) -> Box<T> { return Box { v: x } }\n\
             func main() -> int32 {\n\
               b := wrap(5)\n\
               println(b.v)\n\
               return 0\n\
             }",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn type_parameter_shadowing_an_interface_name_is_accepted() {
        // A generic function may name its type parameter after an interface;
        // inside the function `Speaker` is the parameter, not the interface, so
        // `Box<Speaker>` in the signature is a parameter argument, not an interface
        // argument, and must not be rejected.
        let e = errs(
            "interface Speaker { speak() -> int64 }\n\
             struct Dog { name: int64 }\n\
             impl Speaker for Dog { func speak() -> int64 { return self.name } }\n\
             struct Box<T> { v: T }\n\
             func wrap<Speaker>(x: Speaker) -> Box<Speaker> { return Box { v: x } }\n\
             func main() -> int32 {\n  b := wrap(5)\n  println(b.v)\n  return 0\n}",
        );
        assert!(!e.iter().any(|d| d.msg == IFACE_TARG_MSG), "{e:?}");
    }

    #[test]
    fn a_real_interface_argument_is_still_rejected_when_not_shadowed() {
        // The shadow exemption is narrow: an interface name that is not bound as a
        // type parameter in scope is still refused as a generic argument.
        let e = errs(
            "interface Speaker { speak() -> int64 }\n\
             struct Dog { name: int64 }\n\
             impl Speaker for Dog { func speak() -> int64 { return self.name } }\n\
             struct Box<T> { v: T }\n\
             func wrap<T>(x: T) -> Box<Speaker> { return Box { v: x } }\n\
             func main() -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg == IFACE_TARG_MSG), "{e:?}");
    }

    #[test]
    fn interface_as_plain_field_still_allowed() {
        // An interface as an ordinary, non-generic field or binding is legal; only
        // its use as a generic type argument is refused.
        let e = errs(
            "interface Speaker { speak() -> int64 }\n\
             struct Dog { name: int64 }\n\
             impl Speaker for Dog { func speak() -> int64 { return self.name } }\n\
             struct Wrap { s: Speaker }\n\
             func main() -> int32 {\n\
               w: Wrap = Wrap { s: Dog { name: 7 } }\n\
               println(w.s.speak())\n\
               return 0\n\
             }",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn a_function_local_named_self_gets_the_plain_mismatch_not_the_receiver_message() {
        // The self-value-in-pointer check keys on the impl-method context, not on
        // the spelling of the name. A function-local binding a user happens to call
        // `self` is an ordinary value, so passing it where a pointer is required is
        // a plain argument mismatch, not a receiver-value use. The tailored message
        // must not misfire here.
        let e = errs(
            "struct Cell { n: int64 }\n\
             func take(p: *Cell) -> int64 { return (*p).n }\n\
             func main() -> int32 {\n\
               self := Cell { n: 3 }\n\
               println(take(self))\n\
               return 0\n\
             }",
        );
        assert!(
            e.iter().any(|d| d.msg == "argument 1 has the wrong type"),
            "{e:?}"
        );
        assert!(
            !e.iter().any(|d| d.msg.contains("receiver value")),
            "the receiver-value message must not fire on a plain local: {e:?}"
        );
    }

    #[test]
    fn an_unqualified_variant_constructor_is_rejected() {
        // `Some(7)` without its enum prefix is not a constructor; sema names the
        // qualified fix and fires exactly once.
        let e = errs(
            "enum Opt { Some(v: int64), None }\n\
             func main() -> int32 { o := Some(7) match o { Some(v) => println(v), None => println(-1) } return 0 }",
        );
        assert!(
            e.iter().any(|d| d.msg
                == "use the qualified form 'Opt.Some' to construct an enum value; the unqualified variant name is not a constructor"),
            "{e:?}"
        );
        assert_eq!(
            e.iter()
                .filter(|d| d.msg.contains("is not a constructor"))
                .count(),
            1,
            "single diagnostic: {e:?}"
        );
    }

    #[test]
    fn a_function_that_shares_a_variant_name_wins_over_the_variant() {
        // A top-level function named like a variant is dispatched as the function;
        // the unqualified name resolves to it, not to a constructor, so no rejection
        // fires and the call type-checks.
        let e = errs(
            "enum E { Flag(v: int64), Other }\n\
             func Flag(x: int64) -> int64 { return x * 2 }\n\
             func main() -> int32 { y := Flag(7) println(y) return 0 }",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn a_local_shadowing_a_variant_name_is_not_a_constructor_reject() {
        // An in-scope local named like a variant is the value; calling it is not a
        // bare-constructor attempt, so the constructor rejection stays silent.
        let e = errs(
            "enum Opt { Some(v: int64), None }\n\
             func main() -> int32 { Some := 5 println(Some) return 0 }",
        );
        assert!(
            !e.iter().any(|d| d.msg.contains("is not a constructor")),
            "a local shadow must not trip the constructor rejection: {e:?}"
        );
    }

    #[test]
    fn a_qualified_constructor_with_the_wrong_arity_is_rejected() {
        let e = errs(
            "enum Opt { Some(v: int64), None }\n\
             func main() -> int32 { o := Opt.Some() match o { Some(v) => println(v), None => println(-1) } return 0 }",
        );
        assert!(
            e.iter()
                .any(|d| d.msg == "'Opt.Some' takes 1 argument(s), but 0 were given"),
            "{e:?}"
        );
    }

    #[test]
    fn a_qualified_constructor_with_a_mistyped_payload_is_rejected() {
        let e = errs(
            "enum Opt { Some(v: int64), None }\n\
             func main() -> int32 { o := Opt.Some(true) match o { Some(v) => println(v), None => println(-1) } return 0 }",
        );
        assert!(
            e.iter()
                .any(|d| d.msg == "argument 1 to 'Opt.Some' has the wrong type"),
            "{e:?}"
        );
    }

    #[test]
    fn a_variant_name_two_enums_share_checks_arity_against_the_named_enum() {
        // With two enums sharing a variant name, a qualified constructor's arity is
        // checked against the enum it names, not the by-name payload map's last
        // writer, so the one-payload `A.Hit` accepts one argument even though `B`
        // also declares a two-payload `Hit`.
        let e = errs(
            "enum A { Hit(v: int64), MissA }\n\
             enum B { Hit(a: int64, b: int64), MissB }\n\
             func takeA(x: A) -> int64 { match x { Hit(v) => return v, MissA => return -1 } }\n\
             func main() -> int32 { println(takeA(A.Hit(3))) return 0 }",
        );
        assert!(
            !e.iter().any(|d| d.msg.contains("takes")),
            "the named enum's arity must be used: {e:?}"
        );
    }
}
