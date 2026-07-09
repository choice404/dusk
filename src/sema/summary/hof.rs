//! Lambda bodies and the higher-order builtins, analyzed with the same
//! abstract-value walk that summarizes a function. A lambda literal used to be
//! classified by a syntactic root chain (`return r`, `return r[i]`), which a
//! local alias, a passthrough call, or a tuple wrap defeated; here the body is
//! walked with its parameters seeded, so every laundering the function walk
//! catches is caught inside a lambda too. Split from `walk`, which holds the
//! per-function state and the statement walk, to keep each file focused.

use std::collections::{HashMap, HashSet};

use crate::diag::Span;
use crate::parser::ast::*;

use super::place::elem_of;
use super::walk::{AbsVal, FnState};
use super::ParamSet;

/// The names a function binds exactly once, by a straight `name := g` to a known
/// module function, and never reassigns or rebinds: at a call these resolve to
/// the target's real summary instead of the conservative TOP an opaque callee
/// gets, so `f := id; f(data)` uses id's sink-free relation rather than assuming
/// the call may hand its argument to a channel. A name reassigned, bound more
/// than once, or introduced by a loop or match binder is excluded, which keeps
/// the resolution sound without modeling a branch join: a name that survives is
/// bound to one fixed function over the whole body, so its call is that function.
pub(crate) fn resolvable_fn_binds(
    f: &Func,
    module_fns: &HashSet<String>,
) -> HashMap<String, String> {
    let mut candidate: HashMap<String, String> = HashMap::new();
    let mut poisoned: HashSet<String> = HashSet::new();
    collect_fn_binds_block(&f.body, module_fns, &mut candidate, &mut poisoned);
    candidate.retain(|k, _| !poisoned.contains(k));
    candidate
}

fn poison(name: &str, candidate: &mut HashMap<String, String>, poisoned: &mut HashSet<String>) {
    candidate.remove(name);
    poisoned.insert(name.to_string());
}

fn collect_fn_binds_block(
    b: &Block,
    fns: &HashSet<String>,
    candidate: &mut HashMap<String, String>,
    poisoned: &mut HashSet<String>,
) {
    for s in &b.stmts {
        collect_fn_binds_stmt(s, fns, candidate, poisoned);
    }
}

fn collect_fn_binds_stmt(
    s: &Stmt,
    fns: &HashSet<String>,
    candidate: &mut HashMap<String, String>,
    poisoned: &mut HashSet<String>,
) {
    match s {
        Stmt::Let(l) => {
            collect_fn_binds_expr(&l.value, fns, candidate, poisoned);
            // A single `name := g` to a module function is the only resolvable
            // shape; a second binding of the same name, or a binding to anything
            // else, poisons it so the name falls back to the opaque TOP.
            if l.binds.len() == 1 {
                let name = &l.binds[0].name;
                match &l.value.kind {
                    ExprKind::Ident(g) if fns.contains(g) => {
                        if candidate.contains_key(name) || poisoned.contains(name) {
                            poison(name, candidate, poisoned);
                        } else {
                            candidate.insert(name.clone(), g.clone());
                        }
                    }
                    _ => poison(name, candidate, poisoned),
                }
            } else {
                for b in &l.binds {
                    poison(&b.name, candidate, poisoned);
                }
            }
        }
        Stmt::Assign(lhs, rhs) => {
            collect_fn_binds_expr(lhs, fns, candidate, poisoned);
            collect_fn_binds_expr(rhs, fns, candidate, poisoned);
            if let ExprKind::Ident(n) = &lhs.kind {
                poison(n, candidate, poisoned);
            }
        }
        Stmt::AssignOp(_, lhs, rhs) => {
            collect_fn_binds_expr(lhs, fns, candidate, poisoned);
            collect_fn_binds_expr(rhs, fns, candidate, poisoned);
            if let ExprKind::Ident(n) = &lhs.kind {
                poison(n, candidate, poisoned);
            }
        }
        Stmt::Return(Some(e)) | Stmt::Defer(e) | Stmt::Expr(e) => {
            collect_fn_binds_expr(e, fns, candidate, poisoned)
        }
        Stmt::Return(None) => {}
        Stmt::If(i) => {
            collect_fn_binds_expr(&i.cond, fns, candidate, poisoned);
            collect_fn_binds_block(&i.then, fns, candidate, poisoned);
            if let Some(e) = &i.els {
                collect_fn_binds_block(e, fns, candidate, poisoned);
            }
        }
        Stmt::While(w) => {
            collect_fn_binds_expr(&w.cond, fns, candidate, poisoned);
            collect_fn_binds_block(&w.body, fns, candidate, poisoned);
        }
        Stmt::For(f) => {
            collect_fn_binds_expr(&f.iter, fns, candidate, poisoned);
            poison(&f.var, candidate, poisoned);
            collect_fn_binds_block(&f.body, fns, candidate, poisoned);
        }
        Stmt::Match(m) => collect_fn_binds_match(m, fns, candidate, poisoned),
    }
}

fn collect_fn_binds_match(
    m: &Match,
    fns: &HashSet<String>,
    candidate: &mut HashMap<String, String>,
    poisoned: &mut HashSet<String>,
) {
    collect_fn_binds_expr(&m.scrut, fns, candidate, poisoned);
    for arm in &m.arms {
        match &arm.pat {
            Pattern::Variant(_, binds) => {
                for b in binds {
                    poison(b, candidate, poisoned);
                }
            }
            Pattern::Ident(n) => poison(n, candidate, poisoned),
            Pattern::Wildcard => {}
        }
        collect_fn_binds_block(&arm.body, fns, candidate, poisoned);
    }
}

fn collect_fn_binds_expr(
    e: &Expr,
    fns: &HashSet<String>,
    candidate: &mut HashMap<String, String>,
    poisoned: &mut HashSet<String>,
) {
    match &e.kind {
        ExprKind::Call(callee, args) => {
            collect_fn_binds_expr(callee, fns, candidate, poisoned);
            for a in args {
                collect_fn_binds_expr(a, fns, candidate, poisoned);
            }
        }
        ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Await(x, _) => {
            collect_fn_binds_expr(x, fns, candidate, poisoned)
        }
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b, _) => {
            collect_fn_binds_expr(a, fns, candidate, poisoned);
            collect_fn_binds_expr(b, fns, candidate, poisoned);
        }
        ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
            for x in xs {
                collect_fn_binds_expr(x, fns, candidate, poisoned);
            }
        }
        ExprKind::StructLit(_, fields) => {
            for (_, v) in fields {
                collect_fn_binds_expr(v, fns, candidate, poisoned);
            }
        }
        // A lambda body carries its own bindings; poisoning a same-named capture
        // is only conservative, never unsound, so the nested walk is safe.
        ExprKind::Lambda(l) => collect_fn_binds_block(&l.body, fns, candidate, poisoned),
        ExprKind::Match(m) => collect_fn_binds_match(m, fns, candidate, poisoned),
        ExprKind::Do(_, binds) => {
            for b in binds {
                collect_fn_binds_expr(&b.expr, fns, candidate, poisoned);
            }
        }
        _ => {}
    }
}

impl FnState<'_> {
    /// The abstract value of a lambda literal, and its side tables. The value is
    /// capture-based: the closure aliases whatever it captures, and its
    /// environment sits on this frame when it captures any in-scope binding,
    /// matching the intraprocedural `lambda_captures_local`. The body is also
    /// walked inline with unseeded parameters, so a store through a captured
    /// place is recorded even when no higher-order builtin runs the lambda, and
    /// the lambda's self-alias summary is recorded for the checker's gates.
    pub(super) fn eval_lambda(&mut self, l: &Lambda, span: Span) -> AbsVal {
        self.record_lambda_summary(l, span);
        let mut used = Vec::new();
        let mut bound: HashSet<String> = l.params.iter().map(|p| p.name.clone()).collect();
        crate::parser::ast::collect_block(&l.body, &mut used, &mut bound);
        let mut origins = ParamSet::default();
        let mut reads = ParamSet::default();
        let mut frame = false;
        for n in &used {
            if bound.contains(n) {
                continue;
            }
            if let Some(v) = self.env.get(n) {
                origins = origins.union(v.origins);
                reads = reads.union(v.reads);
            }
            if self.locals.contains(n) || self.param_index.contains_key(n) {
                frame = true;
            }
        }
        let _ = self.walk_lambda_inline(l, &[]);
        AbsVal {
            frame,
            origins,
            reads,
        }
    }

    /// Runs the ordinary function transfer over the lambda body as if it were a
    /// function of its own parameters, and records which of them may reach the
    /// return value (as a view alias or a pointer read-back) and which of them
    /// sink into a channel. Both are keyed by the lambda's expression span, the
    /// checker's handle on the literal. Captures resolve to nothing in the
    /// synthetic walk, which is exactly right for a parameter-indexed answer: the
    /// captured channel of `lambda (x) { chan_send(ch, x) }` contributes no
    /// origin, so the sink set names only the parameter `x` handed across.
    fn record_lambda_summary(&mut self, l: &Lambda, span: Span) {
        if self.lambda_returns.contains_key(&span) {
            return;
        }
        let syn = Func {
            exported: false,
            is_async: false,
            name: String::new(),
            span,
            generics: self.generics_list.clone(),
            params: l.params.clone(),
            ret: l.ret.clone(),
            body: l.body.clone(),
        };
        let (sum, caps) = self.s.transfer_lambda(&syn, self.summaries);
        self.lambda_returns
            .insert(span, sum.returns_alias.union(sum.reads_through));
        self.lambda_sinks.insert(span, sum.sinks);
        // The collect subset rides beside the full sink set, so a direct call of a
        // local bound to a minting lambda names the mint, not a channel.
        self.lambda_collect_sinks.insert(span, sum.collect_sinks);
        // A parameter's view stored through a captured binding is an escape edge
        // the argument-to-argument store model cannot see, since the captured
        // place is not one of the lambda's arguments. Record it against the
        // lambda's span so the checker raises the captured binding's flag at a
        // direct call whose argument in that position is a frame view.
        if !caps.is_empty() {
            self.lambda_capture_flows.insert(span, caps);
        }
    }

    /// Walks a lambda body inline in this function's state, with the lambda's
    /// parameters bound to the given seeds. Stores through captures route to the
    /// enclosing parameters exactly as a function-body store does, and the
    /// lambda's `return`s join a sink instead of the function's return relation.
    /// The enclosing bindings the parameters shadow are restored afterwards;
    /// other raises persist, matching the flat raise-only env the function walk
    /// itself uses.
    pub(super) fn walk_lambda_inline(&mut self, l: &Lambda, seeds: &[AbsVal]) -> AbsVal {
        let saved: Vec<(String, Option<AbsVal>, Option<Type>)> = l
            .params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    self.env.get(&p.name).copied(),
                    self.local_types.get(&p.name).cloned(),
                )
            })
            .collect();
        for (i, p) in l.params.iter().enumerate() {
            self.locals.insert(p.name.clone());
            self.local_types.insert(p.name.clone(), p.ty.clone());
            // The parameter stands for an element or accumulator the builtin
            // threads through; a scalar one is a value copy and inherits none of
            // the seed's frame-ness or aliasing, so a bare `return x` over it
            // cannot hand a frame view back. Only a view-carrying or managed-
            // pointer-reaching parameter propagates the seed, exactly the gate
            // the synthetic transfer (`FnState::new`) applies to a function's own
            // parameters. The two must match, since the checker's `mapper_aliases`
            // reads that synthetic transfer while the frame-store walk reads this
            // inline one; seeding a scalar parameter here over-approximated an
            // identity map into a false frame-store the return gate never saw.
            let seed = if self.s.is_view(&p.ty, &self.generics) || self.s.reaches_managed_ptr(&p.ty)
            {
                seeds.get(i).copied().unwrap_or_default()
            } else {
                AbsVal::default()
            };
            self.env.insert(p.name.clone(), seed);
        }
        self.lambda_rets.push(AbsVal::default());
        self.block(&l.body);
        let ret = self.lambda_rets.pop().unwrap_or_default();
        for (name, env0, ty0) in saved {
            match env0 {
                Some(v) => {
                    self.env.insert(name.clone(), v);
                }
                None => {
                    self.env.remove(&name);
                }
            }
            match ty0 {
                Some(t) => {
                    self.local_types.insert(name, t);
                }
                None => {
                    self.local_types.remove(&name);
                }
            }
        }
        ret
    }

    /// The abstract value of a higher-order builtin call. Each is a set-side
    /// model, not a lambda-shape test: `map`'s result is whatever its function
    /// returns over the collection's elements; `filter`'s result is a subset of
    /// the collection's elements no matter what the predicate does; `fold` and
    /// `reduce` thread both the seed and the elements through the function; and
    /// `foreach` returns nothing but still runs its function over the elements,
    /// so its body's stores are recorded. None for any other name, so the
    /// builtin falls back to its audited summary.
    pub(super) fn higher_order_val(
        &mut self,
        name: &str,
        args: &[Expr],
        argvals: &[AbsVal],
    ) -> Option<AbsVal> {
        match name {
            "map" if args.len() == 2 => Some(self.mapper_result(&args[1], &[argvals[0]])),
            "filter" if args.len() == 2 => {
                let _ = self.mapper_result(&args[1], &[argvals[0]]);
                let elem = elem_of(&self.chain_ty(&args[0]));
                if self.s.is_view(&elem, &self.generics) {
                    Some(argvals[0])
                } else {
                    Some(AbsVal::default())
                }
            }
            "fold" if args.len() == 3 => {
                Some(self.mapper_result(&args[2], &[argvals[1], argvals[0]]))
            }
            "reduce" if args.len() == 2 => {
                Some(self.mapper_result(&args[1], &[argvals[0], argvals[0]]))
            }
            "foreach" if args.len() == 2 => {
                let _ = self.mapper_result(&args[1], &[argvals[0]]);
                Some(AbsVal::default())
            }
            _ => None,
        }
    }

    /// The abstract value a higher-order builtin's function produces over the
    /// given per-parameter seeds. A literal lambda is walked inline with the
    /// seeds bound, so an alias chain, a passthrough call, or a tuple wrap in
    /// its body is followed; a named module function applies its computed
    /// summary over the seeds; anything else is an opaque function value, and
    /// its declared result type decides whether the seeds may thread through.
    fn mapper_result(&mut self, fexpr: &Expr, seeds: &[AbsVal]) -> AbsVal {
        match &fexpr.kind {
            ExprKind::Lambda(l) => self.walk_lambda_inline(l, seeds),
            ExprKind::Ident(n) if !self.locals.contains(n) && !self.param_index.contains_key(n) => {
                match self.summaries.get(n).cloned() {
                    Some(sum) => {
                        let mut r = AbsVal::default();
                        for i in sum.returns_alias.union(sum.reads_through).iter() {
                            if let Some(v) = seeds.get(i as usize) {
                                r = r.join(*v);
                            }
                        }
                        // A store edge inside the mapper moves one element into
                        // another's place; both live in the seed space here, so
                        // the source joins the result conservatively.
                        for &(i, _) in &sum.flows_into {
                            if let Some(v) = seeds.get(i as usize) {
                                r = r.join(*v);
                            }
                        }
                        r
                    }
                    None => self.opaque_mapper(fexpr, seeds),
                }
            }
            _ => self.opaque_mapper(fexpr, seeds),
        }
    }

    fn opaque_mapper(&mut self, fexpr: &Expr, seeds: &[AbsVal]) -> AbsVal {
        let viewy = match self.chain_ty(fexpr) {
            Type::Func(_, r) => self.s.is_view(&r, &self.generics),
            _ => true,
        };
        if !viewy {
            return AbsVal::default();
        }
        let mut r = AbsVal::default();
        for v in seeds {
            r = r.join(*v);
        }
        r
    }
}
