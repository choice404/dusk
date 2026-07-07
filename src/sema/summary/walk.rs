//! Per-function transfer for the escape-summary walk: the abstract
//! interpretation that replays the intraprocedural escape check, broadened
//! to track parameter aliasing across calls. Split from `super`, which holds
//! the fixpoint driver and the summary shape, to keep each file focused.

use std::collections::{HashMap, HashSet};

use crate::diag::Span;
use crate::parser::ast::*;

use super::place::{close_sinks_over_flows, elem_of, projection_root, reads_through_root};
use super::{builtin_summary, EscapeSummary, ParamSet, Summarizer};

/// The abstract value of a place during the per-function walk: whether it may
/// hold a frame-local view (`frame`, the broadened form of the intraprocedural
/// escape flag) and which parameters' views it may alias (`origins`). `frame`
/// mirrors the existing intraprocedural notion and propagates through calls so
/// the enforcement step can see a frame view laundered through one; the summary
/// output itself is built only from `origins`, `reads`, and the store edges.
/// Every join carries all three components, so no value form drops one.
#[derive(Clone, Copy, Default)]
pub(super) struct AbsVal {
    pub(super) frame: bool,
    pub(super) origins: ParamSet,
    /// The parameters this value was read back through, or that it *is*: a value
    /// equal to pointer parameter `j`, read out of the heap object `*j`
    /// addresses, or projected out of a parameter whose type reaches a managed
    /// pointer, carries `reads = {j}`. The summary's `reads_through` is the
    /// union of the returned values' `reads`.
    pub(super) reads: ParamSet,
}

impl AbsVal {
    pub(super) fn join(self, o: AbsVal) -> AbsVal {
        AbsVal {
            frame: self.frame || o.frame,
            origins: self.origins.union(o.origins),
            reads: self.reads.union(o.reads),
        }
    }
}

impl Summarizer {
    /// Re-derives one function's summary from its body under the current summary
    /// table. Pure in the table: it reads callee summaries and writes only local
    /// state, returning the fresh summary.
    pub(super) fn transfer(&self, f: &Func, summaries: &HashMap<String, EscapeSummary>) -> EscapeSummary {
        let mut st = FnState::new(self, summaries, f);
        st.block(&f.body);
        st.finish()
    }

    /// The lambda variant of `transfer`: the same walk over a lambda body posed as
    /// a synthetic function of its own parameters, returning both the summary and
    /// the capture-flow edges (a parameter's view stored through a free, captured
    /// binding). A real function has no free names, so this is only meaningful for
    /// a synthetic lambda transfer; the edges name the captured binding the checker
    /// raises at the call.
    pub(super) fn transfer_lambda(
        &self,
        f: &Func,
        summaries: &HashMap<String, EscapeSummary>,
    ) -> (EscapeSummary, Vec<(u8, String)>) {
        let mut st = FnState::new(self, summaries, f);
        st.block(&f.body);
        let mut caps = std::mem::take(&mut st.capture_flows);
        caps.sort();
        caps.dedup();
        (st.finish(), caps)
    }

    /// The collection variant of `transfer`: the same walk, keeping the
    /// per-lambda alias table and the frame-store sites instead of the summary.
    /// Run once per function and impl method after the fixpoint settles.
    #[allow(clippy::type_complexity)]
    pub(super) fn transfer_collect(
        &self,
        f: &Func,
        summaries: &HashMap<String, EscapeSummary>,
    ) -> (
        HashMap<Span, ParamSet>,
        HashMap<Span, ParamSet>,
        HashMap<Span, ParamSet>,
        Vec<(Span, u8)>,
        HashMap<Span, Vec<(u8, String)>>,
    ) {
        let mut st = FnState::new(self, summaries, f);
        st.block(&f.body);
        st.frame_stores.sort_unstable_by_key(|&(s, j)| (s.lo, s.hi, j));
        st.frame_stores.dedup();
        (
            st.lambda_returns,
            st.lambda_sinks,
            st.lambda_collect_sinks,
            st.frame_stores,
            st.lambda_capture_flows,
        )
    }
}

/// The per-function abstract interpretation state. It replays the intraprocedural
/// escape walk, broadened so each place carries both the frame flag and the set
/// of parameters it may alias, and it accumulates the two summary components. All
/// updates are joins (raise, never clear), so branch structure needs no modeling:
/// unioning every path is a sound over-approximation, exactly the raise-only
/// discipline the intraprocedural pass already uses for field stores.
pub(super) struct FnState<'a> {
    pub(super) s: &'a Summarizer,
    pub(super) summaries: &'a HashMap<String, EscapeSummary>,
    pub(super) generics: HashSet<String>,
    /// The declaration-ordered generic names, for a synthetic lambda transfer.
    pub(super) generics_list: Vec<String>,
    pub(super) param_index: HashMap<String, u8>,
    /// Locals bound exactly once to a known module function and never
    /// reassigned, mapped to that function's name. A call of such a local
    /// resolves to the target's real summary instead of the conservative TOP an
    /// opaque callee gets, so `f := id; f(data)` uses id's sink-free relation
    /// rather than assuming the call may hand its argument to a channel. A
    /// function-typed parameter and a lambda-bound local are not here, so they
    /// stay opaque and their calls keep the conservative sink.
    pub(super) fn_binds: HashMap<String, String>,
    /// Parameters that are valid store destinations: a view, or a type through
    /// which a managed pointer is reachable.
    pub(super) dest: HashSet<u8>,
    /// Names introduced as locals (let bindings, loop and match binders), so a
    /// lambda capturing one is a frame capture.
    pub(super) locals: HashSet<String>,
    /// Declared types of parameters and annotated locals, for projection typing.
    pub(super) local_types: HashMap<String, Type>,
    pub(super) env: HashMap<String, AbsVal>,
    pub(super) returns_alias: ParamSet,
    pub(super) flows_into: Vec<(u8, u8)>,
    pub(super) reads_through: ParamSet,
    /// The parameters whose value or pointee is handed to `chan_send`/
    /// `chan_try_send`, directly or through a helper that sinks its argument. A
    /// caller passing a polluted argument in one of these positions crosses a
    /// thread boundary the receiver outlives, so the enforcement rejects it. This
    /// accumulates the directly-observed and propagated sinks; the backward
    /// closure over the store edges (a parameter that flows into a sunk one) is
    /// applied once at `finish`, so the emitted summary is store-edge closed.
    pub(super) sinks: ParamSet,
    /// The subset of `sinks` whose sink is a `collector<T>` mint rather than a
    /// channel send. Accumulated in lockstep with `sinks` and closed over the same
    /// store edges at `finish`, so it stays a subset. It changes nothing about the
    /// escape decision, only which diagnostic wording the checker picks.
    pub(super) collect_sinks: ParamSet,
    /// The return sink stack of the lambda bodies being walked inline. A
    /// `return` inside a lambda is the lambda's value, not the function's, so it
    /// joins the innermost sink instead of `returns_alias`/`reads_through`.
    pub(super) lambda_rets: Vec<AbsVal>,
    /// Each lambda literal's self-alias set, keyed by its expression span: the
    /// lambda's own parameter indices whose views may reach its return value.
    /// Kept by the collection pass for the checker's higher-order gates.
    pub(super) lambda_returns: HashMap<Span, ParamSet>,
    /// Each lambda literal's self-sink set, keyed by its expression span: the
    /// lambda's own parameter indices whose value or pointee reaches a
    /// `chan_send`/`chan_try_send` in its body, closed over the lambda's own
    /// store edges. A lambda bound to a local has no computed summary, so the
    /// checker reads this at a direct call of the bound name to reject a polluted
    /// argument in a sink position, the send the leaf-site check cannot see
    /// through a closure. Kept by the collection pass alongside `lambda_returns`.
    pub(super) lambda_sinks: HashMap<Span, ParamSet>,
    /// The subset of each lambda literal's `lambda_sinks` whose sink is a
    /// `collector<T>` mint rather than a channel send, keyed by the same span. The
    /// checker reads it beside `lambda_sinks` to pick the collect wording for a
    /// minting closure's reject.
    pub(super) lambda_collect_sinks: HashMap<Span, ParamSet>,
    /// Direct stores of a frame-local view into a place reachable through a
    /// parameter: the store site's span and the parameter index. Such a view
    /// outlives the frame unconditionally, so the checker reports each one.
    /// Call-mediated stores are excluded; the checker's own call-flow routing
    /// reports those at the call site.
    pub(super) frame_stores: Vec<(Span, u8)>,
    /// Edges by which a parameter's view is stored through a *captured* binding,
    /// accumulated only while walking a lambda body as a synthetic function of
    /// its own parameters: the parameter index and the captured binding's name.
    /// A captured binding is a name the store roots at that is neither a
    /// parameter nor a local of the walked body, so it is free in the body and
    /// resolves in the enclosing scope. A top-level function has no free names,
    /// so this stays empty for one; it is meaningful only for the synthetic
    /// lambda transfer, whose captures are the enclosing function's bindings.
    pub(super) capture_flows: Vec<(u8, String)>,
    /// Each lambda literal's capture-flow edge set, keyed by its expression span,
    /// kept by the collection pass for the checker's capture-store gate.
    pub(super) lambda_capture_flows: HashMap<Span, Vec<(u8, String)>>,
}

impl<'a> FnState<'a> {
    pub(super) fn new(s: &'a Summarizer, summaries: &'a HashMap<String, EscapeSummary>, f: &Func) -> Self {
        let generics: HashSet<String> = f.generics.iter().cloned().collect();
        let mut param_index = HashMap::new();
        let mut dest = HashSet::new();
        let mut local_types = HashMap::new();
        let mut env = HashMap::new();
        for (i, p) in f.params.iter().enumerate() {
            let idx = i as u8;
            param_index.insert(p.name.clone(), idx);
            local_types.insert(p.name.clone(), p.ty.clone());
            let view = s.is_view(&p.ty, &generics);
            // A parameter whose type reaches a managed pointer, bare or buried in
            // a struct, enum, tuple, or array, exposes whatever the caller stored
            // behind it, so it seeds the reads-through relation; a bare-pointer
            // rule would let a wrapper wash the relation out.
            let reaches = s.reaches_managed_ptr(&p.ty);
            if view || reaches {
                dest.insert(idx);
                let mut av = AbsVal::default();
                if view {
                    av.origins = ParamSet::single(idx);
                }
                if reaches {
                    av.reads = ParamSet::single(idx);
                }
                env.insert(p.name.clone(), av);
            }
        }
        FnState {
            s,
            summaries,
            generics,
            generics_list: f.generics.clone(),
            param_index,
            fn_binds: super::hof::resolvable_fn_binds(f, &s.module_fns),
            dest,
            locals: HashSet::new(),
            local_types,
            env,
            returns_alias: ParamSet::default(),
            flows_into: Vec::new(),
            reads_through: ParamSet::default(),
            sinks: ParamSet::default(),
            collect_sinks: ParamSet::default(),
            lambda_rets: Vec::new(),
            lambda_returns: HashMap::new(),
            lambda_sinks: HashMap::new(),
            lambda_collect_sinks: HashMap::new(),
            frame_stores: Vec::new(),
            capture_flows: Vec::new(),
            lambda_capture_flows: HashMap::new(),
        }
    }

    fn finish(mut self) -> EscapeSummary {
        self.flows_into.sort_unstable();
        self.flows_into.dedup();
        // Close the sink set backward over the store edges: a parameter whose
        // view flows into a sunk parameter's place reaches the same thread
        // boundary, so it sinks too. Fuses the store hop and the send hop when
        // one body does both (`evil(ch, c, s) { (*c).rows = s; chan_send(ch,
        // c) }`), which the pre-call sink check cannot otherwise see.
        let sinks = close_sinks_over_flows(self.sinks, &self.flows_into);
        // The collect subset rides the same store-edge closure, so a parameter
        // that flows into a collect sink is itself a collect sink; closing it with
        // the same edges keeps it a subset of the full `sinks`.
        let collect_sinks = close_sinks_over_flows(self.collect_sinks, &self.flows_into);
        EscapeSummary {
            returns_alias: self.returns_alias,
            flows_into: self.flows_into,
            reads_through: self.reads_through,
            sinks,
            collect_sinks,
        }
    }

    pub(super) fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
    }

    /// Replays a loop body until the abstract state stops growing. A two-step
    /// alias chain (`out = tmp; tmp = s`) only closes on the second iteration, and
    /// a longer chain needs one pass per link, so a single visit under-reports.
    /// Every update is a join (raise, never clear), so the state climbs a finite
    /// lattice and converges; the cap guards a future non-monotone change.
    fn loop_fixpoint(&mut self, body: &Block) {
        let cap = self.param_index.len().saturating_add(self.locals.len()).saturating_add(4);
        let mut prev = self.loop_state();
        for _ in 0..cap {
            self.block(body);
            let now = self.loop_state();
            if now == prev {
                break;
            }
            prev = now;
        }
    }

    /// A comparable snapshot of every raise-only component the loop body may grow,
    /// so the fixpoint can detect convergence. The innermost lambda return sink
    /// and the frame-store count are included, so a loop inside a lambda body
    /// converges on those too.
    #[allow(clippy::type_complexity)]
    fn loop_state(&self) -> (Vec<(String, (bool, u64, u64))>, u64, usize, u64, (bool, u64, u64), usize, u64, u64) {
        let mut env: Vec<(String, (bool, u64, u64))> = self
            .env
            .iter()
            .map(|(k, v)| (k.clone(), (v.frame, v.origins.0, v.reads.0)))
            .collect();
        env.sort();
        let sink = self
            .lambda_rets
            .last()
            .map(|v| (v.frame, v.origins.0, v.reads.0))
            .unwrap_or((false, 0, 0));
        (
            env,
            self.returns_alias.0,
            self.flows_into.len(),
            self.reads_through.0,
            sink,
            self.frame_stores.len(),
            self.sinks.0,
            self.collect_sinks.0,
        )
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(l) => self.let_stmt(l),
            Stmt::Assign(lhs, rhs) => {
                let rv = self.eval(rhs);
                match &lhs.kind {
                    // A bare reassignment updates the binding; it never stores
                    // into a caller-visible place, so it only raises local state.
                    ExprKind::Ident(n) => self.raise_env(n, rv),
                    // A field or element store may reach the caller through a
                    // parameter; evaluate the place for nested calls, then record.
                    // This is a direct store, so a frame-view source is kept for
                    // the checker's frame-store diagnostics.
                    _ => {
                        self.eval(lhs);
                        self.record_store(lhs, rv, false);
                    }
                }
            }
            Stmt::AssignOp(_, lhs, rhs) => {
                // A compound assign combines the place with an arithmetic rhs, so
                // the stored value carries no view; only evaluate for call effects.
                self.eval(rhs);
                self.eval(lhs);
            }
            Stmt::Return(Some(e)) => {
                if let ExprKind::Await(op, _) = &e.kind {
                    // The awaited element cannot be a view (async signature rule),
                    // so only the operand's call effects matter.
                    self.eval(op);
                    return;
                }
                let v = self.eval(e);
                // A return inside an inline-walked lambda body is the lambda's
                // value, not this function's; it joins the innermost sink.
                if let Some(top) = self.lambda_rets.last_mut() {
                    *top = top.join(v);
                } else {
                    self.returns_alias = self.returns_alias.union(v.origins);
                    self.reads_through = self.reads_through.union(v.reads);
                }
            }
            Stmt::Return(None) => {}
            Stmt::Defer(e) | Stmt::Expr(e) => {
                self.eval(e);
            }
            Stmt::If(i) => {
                self.eval(&i.cond);
                self.block(&i.then);
                if let Some(e) = &i.els {
                    self.block(e);
                }
            }
            Stmt::While(w) => {
                self.eval(&w.cond);
                self.loop_fixpoint(&w.body);
            }
            Stmt::For(f) => {
                let it = self.eval(&f.iter);
                self.locals.insert(f.var.clone());
                // The loop variable views an element of the iterand; carry the
                // iterand's origins conservatively.
                self.env.insert(f.var.clone(), it);
                self.loop_fixpoint(&f.body);
            }
            Stmt::Match(m) => self.stmt_match(m),
        }
    }

    fn let_stmt(&mut self, l: &Let) {
        if let ExprKind::Await(op, _) = &l.value.kind {
            self.eval(op);
            for b in &l.binds {
                self.note_local(b);
            }
            return;
        }
        if l.binds.len() == 1 {
            let v = self.eval(&l.value);
            let b = &l.binds[0];
            self.note_local(b);
            self.raise_env(&b.name, v);
            return;
        }
        // A tuple-literal destructure binds each name to a member; anything else
        // gives every binder the whole value's abstract state, conservatively.
        match &l.value.kind {
            ExprKind::Tuple(members) if members.len() == l.binds.len() => {
                for (b, m) in l.binds.iter().zip(members) {
                    let mv = self.eval(m);
                    self.note_local(b);
                    self.raise_env(&b.name, mv);
                }
            }
            _ => {
                let v = self.eval(&l.value);
                for b in &l.binds {
                    self.note_local(b);
                    self.raise_env(&b.name, v);
                }
            }
        }
    }

    fn note_local(&mut self, b: &Bind) {
        self.locals.insert(b.name.clone());
        if let Some(t) = &b.ty {
            self.local_types.insert(b.name.clone(), t.clone());
        }
    }

    fn stmt_match(&mut self, m: &Match) {
        let scrut = self.eval(&m.scrut);
        for arm in &m.arms {
            self.bind_arm(&arm.pat, scrut);
            self.block(&arm.body);
        }
    }

    fn bind_arm(&mut self, pat: &Pattern, scrut: AbsVal) {
        match pat {
            Pattern::Variant(_, binds) => {
                for b in binds {
                    self.locals.insert(b.clone());
                    self.env.insert(b.clone(), scrut);
                }
            }
            Pattern::Ident(n) => {
                self.locals.insert(n.clone());
                self.env.insert(n.clone(), scrut);
            }
            Pattern::Wildcard => {}
        }
    }

    /// The abstract value of an expression, evaluating every subexpression so a
    /// nested call's store edges are recorded even when the call's own result is
    /// discarded.
    pub(super) fn eval(&mut self, e: &Expr) -> AbsVal {
        match &e.kind {
            ExprKind::Ident(n) => self.env.get(n).copied().unwrap_or_default(),
            ExprKind::Unary(_, x) => {
                self.eval(x);
                AbsVal::default()
            }
            ExprKind::Binary(_, a, b) | ExprKind::Range(a, b, _) => {
                self.eval(a);
                self.eval(b);
                AbsVal::default()
            }
            ExprKind::Array(elems) => {
                let mut origins = ParamSet::default();
                for el in elems {
                    origins = origins.union(self.eval(el).origins);
                }
                // A local array literal materializes on this frame.
                AbsVal { frame: true, origins, reads: ParamSet::default() }
            }
            ExprKind::Tuple(xs) => {
                let mut v = AbsVal::default();
                for x in xs {
                    v = v.join(self.eval(x));
                }
                v
            }
            ExprKind::Index(base, idx) => {
                if let ExprKind::Range(..) = &idx.kind {
                    // A re-slice keeps its base's frame-ness and aliasing: a
                    // re-slice of a parameter still views the parameter, a
                    // re-slice of a local array still views the frame.
                    self.eval(idx);
                    self.eval(base)
                } else {
                    self.eval(idx);
                    self.eval(base);
                    self.projection_val(e)
                }
            }
            ExprKind::Field(base, _) => {
                self.eval(base);
                self.projection_val(e)
            }
            ExprKind::Lambda(l) => self.eval_lambda(l, e.span),
            ExprKind::StructLit(name, fields) => self.eval_struct_lit(name, fields),
            ExprKind::Call(callee, args) => self.eval_call(callee, args),
            ExprKind::Match(m) => self.eval_match(m),
            ExprKind::Await(op, _) => {
                self.eval(op);
                AbsVal::default()
            }
            // A mint hands its value to a collected block that outlives the frame,
            // the same shape a channel send hands its element across a thread
            // boundary. Record the sink so a caller passing a frame-view-polluted
            // argument into a minting helper is caught one hop up, closed over the
            // store edges like every other sink. The block itself is a fresh root,
            // so the minted value aliases nothing.
            //
            // A closure mint copies its captures into the collected environment, so
            // the lambda's abstract value already carries each captured parameter's
            // origins and pointee reads; recording it sinks exactly the managed and
            // view captures, so a caller burying a frame view behind a captured
            // pointer parameter is caught one hop up. A scalar capture carries no
            // provenance and self-limits. A slice mint deep copies its element
            // storage, so a frame-view source does not outlive and records no sink,
            // unless the element reaches a managed pointer whose pointee the copy
            // leaves untouched: a buried view then outlives, so the sink fires for
            // that case. The plain kind always sinks its value.
            ExprKind::Collect { ty, arg } => {
                let v = self.eval(arg);
                let sink = match ty {
                    Type::Slice(elem) => self.s.reaches_managed_ptr(elem),
                    _ => true,
                };
                if sink {
                    self.record_collect_sink(v);
                }
                AbsVal::default()
            }
            ExprKind::Do(_, binds) => {
                for b in binds {
                    self.eval(&b.expr);
                }
                AbsVal::default()
            }
            _ => AbsVal::default(),
        }
    }

    /// The abstract value of a by-value projection: only a member that can carry
    /// a view inherits its root binding's state, exactly as the intraprocedural
    /// `projection_escape` gates on `member_carries_view`.
    fn projection_val(&self, e: &Expr) -> AbsVal {
        let t = self.chain_ty(e);
        if !self.s.is_view(&t, &self.generics) {
            return AbsVal::default();
        }
        // Frame-ness and parameter origins root through fields and indexes but
        // stop at a pointer dereference, whose target is the heap.
        let mut v = match projection_root(e) {
            Some(root) => self.env.get(&root).copied().unwrap_or_default(),
            None => AbsVal::default(),
        };
        // The reads-through relation, in contrast, follows the dereference: reading
        // a view out of `(*p).field` or `(*p)[i]` exposes whatever the caller
        // stored behind `p`, so the projected view reads-through the pointer's
        // parameter.
        if let Some(root) = reads_through_root(e) {
            if let Some(av) = self.env.get(&root) {
                v.reads = v.reads.union(av.reads);
            }
        }
        v
    }

    fn eval_struct_lit(&mut self, name: &str, fields: &[(String, Expr)]) -> AbsVal {
        let decl = self.s.structs.get(name).map(|(_, fs)| fs.clone());
        let mut v = AbsVal::default();
        for (fname, init) in fields {
            let iv = self.eval(init);
            let is_view_field = match &decl {
                Some(fs) => fs
                    .iter()
                    .find(|(n, _)| n == fname)
                    .map(|(_, ft)| self.s.is_view(ft, &self.generics))
                    .unwrap_or(true),
                // Unknown struct: keep every field, conservatively.
                None => true,
            };
            if is_view_field {
                v = v.join(iv);
            }
        }
        v
    }

    fn eval_match(&mut self, m: &Match) -> AbsVal {
        let scrut = self.eval(&m.scrut);
        let mut r = AbsVal::default();
        for arm in &m.arms {
            self.bind_arm(&arm.pat, scrut);
            self.block(&arm.body);
            if let Some(Stmt::Expr(tail)) = arm.body.stmts.last() {
                r = r.join(self.eval(tail));
            }
        }
        r
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Expr]) -> AbsVal {
        let argvals: Vec<AbsVal> = args.iter().map(|a| self.eval(a)).collect();
        match &callee.kind {
            ExprKind::Ident(name) => {
                // A channel send hands its element to a receiver thread that
                // outlives this frame. Recognized by name beside its library
                // summary, so a parameter whose value or pointee is sent enters
                // the sink set even though chan_send also carries a summary.
                if (name == "chan_send" || name == "chan_try_send") && argvals.len() == 2 {
                    self.record_chan_sink(argvals[1]);
                }
                if let Some(sum) = self.summaries.get(name).cloned() {
                    self.apply_summary(&sum, args, &argvals)
                } else if let Some(payloads) = self.s.variant_payloads.get(name).cloned() {
                    self.ctor_val(&payloads, &argvals)
                } else if let Some(sum) = builtin_summary(name) {
                    // A higher-order builtin whose element or accumulator function
                    // is an element passthrough leaks the collection or seed
                    // argument's views; otherwise it returns a fresh heap value.
                    if let Some(v) = self.higher_order_val(name, args, &argvals) {
                        v
                    } else {
                        self.apply_summary(&sum, args, &argvals)
                    }
                } else if let Some(sum) = self
                    .fn_binds
                    .get(name)
                    .and_then(|g| self.summaries.get(g))
                    .cloned()
                {
                    // A local bound once to a known module function: resolve to
                    // its real summary, so a call of it is that function's known
                    // relation, not the opaque TOP that would assume a channel
                    // sink. Only a name proven bound to one fixed function reaches
                    // here; a function-typed parameter and a lambda-bound local
                    // stay opaque below.
                    self.apply_summary(&sum, args, &argvals)
                } else {
                    // A function-typed parameter, an import, a foreign symbol, or
                    // a lambda-bound local: a genuinely opaque callee gets the
                    // conservative TOP summary, which now also assumes it may hand
                    // an argument to a channel.
                    self.apply_top(args, &argvals)
                }
            }
            ExprKind::Field(base, v) => {
                // An enum-qualified constructor `E.V(x)` names its variant.
                if let ExprKind::Ident(en) = &base.kind {
                    if self.s.enums.contains_key(en) {
                        return match self.s.variant_payloads.get(v).cloned() {
                            Some(payloads) => self.ctor_val(&payloads, &argvals),
                            None => self.join_all(&argvals),
                        };
                    }
                }
                // A method call: the receiver and dispatch are opaque, so TOP.
                self.eval(base);
                self.apply_top(args, &argvals)
            }
            _ => {
                self.eval(callee);
                self.apply_top(args, &argvals)
            }
        }
    }

    /// Applies a known callee summary at a call site: route each `flows_into`
    /// edge from arg `i` into arg `j`'s place, and join every returned arg into
    /// the call's result.
    fn apply_summary(&mut self, sum: &EscapeSummary, args: &[Expr], argvals: &[AbsVal]) -> AbsVal {
        for &(i, j) in &sum.flows_into {
            let (iu, ju) = (i as usize, j as usize);
            if iu < argvals.len() && ju < args.len() {
                self.flow_into_place(&args[ju], argvals[iu]);
            }
        }
        let mut r = AbsVal::default();
        for i in sum.returns_alias.iter() {
            if let Some(v) = argvals.get(i as usize) {
                r = r.join(*v);
            }
        }
        // A result that reads through the callee's pointer parameter `j` exposes
        // whatever the caller's argument `j` addresses. Absorb that argument's
        // whole abstract state: its frame-ness (a polluted local), its reads (a
        // pointer that itself reads through one of this function's parameters),
        // so the relation threads across the call chain.
        for j in sum.reads_through.iter() {
            if let Some(v) = argvals.get(j as usize) {
                r = r.join(*v);
            }
        }
        // A callee that sinks its parameter `j` into a channel makes the caller's
        // argument `j` cross the same thread boundary, so that argument's origins
        // and pointee reads propagate into this function's sink set, threading the
        // relation up a chain of relaying helpers (relay2 -> relay -> chan_send).
        for j in sum.sinks.iter() {
            if let Some(v) = argvals.get(j as usize) {
                let ps = v.origins.union(v.reads);
                self.sinks = self.sinks.union(ps);
                // Carry the sink flavor up the chain: a caller relaying an
                // argument into a callee's collect sink sinks it as a collect too,
                // so the reject one hop up still names the mint.
                if sum.collect_sinks.contains(j) {
                    self.collect_sinks = self.collect_sinks.union(ps);
                }
            }
        }
        r
    }

    /// Records that a channel send hands its element across a thread boundary the
    /// receiver outlives: the sent value's parameter origins and pointee reads
    /// enter the sink set, so a caller passing a polluted argument in that
    /// position is caught one hop up. A sent frame-local value is a direct send
    /// within this frame, which the leaf-site check in `typeck` reports, so it
    /// need not enter the summary.
    fn record_chan_sink(&mut self, sent: AbsVal) {
        self.sinks = self.sinks.union(sent.origins.union(sent.reads));
    }

    /// Records that a `collector<T>` mint hands its value to a collected block the
    /// frame outlives, the same outliving escape a channel send is. It enters the
    /// full sink set exactly as a channel sink does, so the enforcement is
    /// identical, and additionally the collect subset, so the checker names the
    /// mint rather than a channel at the reject.
    fn record_collect_sink(&mut self, sent: AbsVal) {
        let ps = sent.origins.union(sent.reads);
        self.sinks = self.sinks.union(ps);
        self.collect_sinks = self.collect_sinks.union(ps);
    }

    /// The TOP summary of an unknown callee: it may return any argument, may
    /// store any argument's view into any other, and may hand any argument to a
    /// channel. Non-view arguments carry empty origins and reads, so each part
    /// self-limits to the arguments that actually carry a frame view or a managed
    /// pointee. The sink part is what closes the last hole in the interprocedural
    /// send check: an opaque callee (a function-value parameter, an import, a
    /// method dispatch) may send its argument across a thread boundary the
    /// receiver outlives, so a managed or view argument threaded into one makes
    /// the enclosing function sink that parameter, exactly as a named `relay(ch,
    /// c)` helper does. The store-edge closure at `finish` then lifts the sink to
    /// any source position that flows into it. This is deliberately conservative,
    /// on the same footing as the store-edge TOP already applied above: it can
    /// reject a harmless opaque call that never sends, but only when the caller
    /// hands it an argument a frame view was actually stored into.
    fn apply_top(&mut self, args: &[Expr], argvals: &[AbsVal]) -> AbsVal {
        for (i, &vi) in argvals.iter().enumerate() {
            for (j, place) in args.iter().enumerate() {
                if i != j {
                    self.flow_into_place(place, vi);
                }
            }
        }
        for v in argvals {
            self.sinks = self.sinks.union(v.origins.union(v.reads));
        }
        self.join_all(argvals)
    }

    /// The abstract value of an enum or variant constructor: a payload argument
    /// whose declared field type carries a view is aliased by the constructed
    /// value; a scalar payload is copied inline.
    fn ctor_val(&self, payloads: &[Type], argvals: &[AbsVal]) -> AbsVal {
        let mut v = AbsVal::default();
        for (idx, av) in argvals.iter().enumerate() {
            let keep = match payloads.get(idx) {
                Some(pty) => self.s.is_view(pty, &self.generics),
                None => true,
            };
            if keep {
                v = v.join(*av);
            }
        }
        v
    }

    fn join_all(&self, argvals: &[AbsVal]) -> AbsVal {
        let mut r = AbsVal::default();
        for v in argvals {
            r = r.join(*v);
        }
        r
    }

    /// A side-effect-free AST type walk for projection chains, mirroring the
    /// intraprocedural `chain_ty`. A range index is a re-slice, so it keeps the
    /// slice shape over the base's element instead of projecting the element
    /// out. An unknown-typed place is `Type::Infer`, which `is_view` treats
    /// conservatively as a view.
    pub(super) fn chain_ty(&self, e: &Expr) -> Type {
        match &e.kind {
            ExprKind::Ident(n) => self.local_types.get(n).cloned().unwrap_or(Type::Infer),
            ExprKind::Field(base, fname) => match self.chain_ty(base) {
                Type::Named(s, _) => self
                    .s
                    .structs
                    .get(&s)
                    .and_then(|(_, fs)| fs.iter().find(|(n, _)| n == fname).map(|(_, t)| t.clone()))
                    .unwrap_or(Type::Infer),
                _ => Type::Infer,
            },
            ExprKind::Index(base, idx) => {
                let bt = self.chain_ty(base);
                if matches!(idx.kind, ExprKind::Range(..)) {
                    Type::Slice(Box::new(elem_of(&bt)))
                } else {
                    elem_of(&bt)
                }
            }
            ExprKind::Unary(UnOp::Deref, p) => match self.chain_ty(p) {
                Type::Ptr(inner) | Type::RawPtr(inner) | Type::Collector(inner) => *inner,
                _ => Type::Infer,
            },
            _ => Type::Infer,
        }
    }
}

