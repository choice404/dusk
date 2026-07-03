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

/// Type checks a module, returning diagnostics.
pub fn check(module: &Module) -> Vec<Diagnostic> {
    let mut tc = TypeChecker::new();
    tc.collect_sigs(module);
    tc.run(module);
    tc.errors
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

struct TypeChecker {
    sigs: HashMap<String, (Vec<Ty>, Ty)>,
    ifaces: HashSet<String>,
    enums: HashMap<String, Vec<String>>,
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
    iface_binds: Vec<HashSet<String>>,
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
    cur_generics: HashSet<String>,
    cur_ret: Ty,
    errors: Vec<Diagnostic>,
}

impl TypeChecker {
    fn new() -> Self {
        TypeChecker {
            sigs: HashMap::new(),
            ifaces: HashSet::new(),
            enums: HashMap::new(),
            structs: HashMap::new(),
            iface_methods: HashMap::new(),
            impls: HashSet::new(),
            raw_sigs: HashMap::new(),
            embed_fields: HashMap::new(),
            iface_binds: Vec::new(),
            scopes: Vec::new(),
            owns: Vec::new(),
            muts: Vec::new(),
            err_binds: Vec::new(),
            branch_depth: 0,
            cur_generics: HashSet::new(),
            cur_ret: Ty::Unit,
            errors: Vec::new(),
        }
    }

    fn collect_sigs(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Interface(i) => {
                    self.ifaces.insert(i.name.clone());
                    let methods = i.methods.iter().map(|m| (m.name.clone(), m.params.len())).collect();
                    self.iface_methods.insert(i.name.clone(), methods);
                }
                Item::Enum(e) => {
                    let variants = e.variants.iter().map(|v| v.name.clone()).collect();
                    self.enums.insert(e.name.clone(), variants);
                    let gens: HashSet<String> = e.generics.iter().cloned().collect();
                    let fields = e
                        .variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(|f| lower(&f.ty, &gens)))
                        .collect();
                    self.embed_fields.insert(e.name.clone(), fields);
                }
                Item::Impl(im) => {
                    if let Some(iface) = &im.iface {
                        if !self.impls.insert((iface.clone(), im.ty.clone())) {
                            self.err(
                                format!("duplicate 'impl {iface} for {}'; merge the two blocks", im.ty),
                                im.span,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        for item in &module.items {
            match item {
                Item::Func(f) => {
                    let gens: HashSet<String> = f.generics.iter().cloned().collect();
                    self.raw_sigs
                        .insert(f.name.clone(), f.params.iter().map(|p| lower(&p.ty, &gens)).collect());
                    let params = f.params.iter().map(|p| self.fix(lower(&p.ty, &gens))).collect();
                    let ret = self.fix(lower(&f.ret, &gens));
                    self.sigs.insert(f.name.clone(), (params, ret));
                }
                Item::Foreign(fb) => {
                    // A foreign function is non generic, so its signature lowers
                    // against an empty generic set. It joins the same table an
                    // ordinary call resolves against, so the call is type checked.
                    let gens = HashSet::new();
                    for ff in &fb.funcs {
                        let params =
                            ff.params.iter().map(|p| self.fix(lower(&p.ty, &gens))).collect();
                        let ret = self.fix(lower(&ff.ret, &gens));
                        self.sigs.insert(ff.name.clone(), (params, ret));
                    }
                }
                Item::Struct(s) => {
                    let gens: HashSet<String> = s.generics.iter().cloned().collect();
                    let is_gen = !gens.is_empty();
                    let fields = s
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), self.fix(lower(&f.ty, &gens))))
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
                    self.func(f, false);
                }
                Item::Impl(im) => {
                    self.check_impl_complete(im);
                    for m in &im.methods {
                        self.func(m, true);
                    }
                }
                Item::Foreign(fb) => self.check_foreign(fb),
                _ => {}
            }
        }
    }

    /// The only main signatures the C entry supports: no parameters, or exactly
    /// `(argc: int32, argv: string[])`, which the wrapper bridges. Any other
    /// shape would be emitted as the C `main` with the wrong parameters and read
    /// garbage registers, so it is rejected here.
    fn check_main_sig(&mut self, f: &Func) {
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
                format!("unsupported foreign abi \"{}\", only \"C\" is supported", fb.abi),
                fb.span,
            );
        }
        let empty = HashSet::new();
        for ff in &fb.funcs {
            for p in &ff.params {
                let ty = self.fix(lower(&p.ty, &empty));
                self.check_foreign_ty(&ty, &ff.name, fb.span);
            }
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
                    self.err(format!("field '{}' is set more than once in '{name}'", fields[i].0), span);
                }
            }
        }
        for ((fname, fexpr), vty) in fields.iter().zip(&vals) {
            match declared.iter().find(|(dn, _)| dn == fname) {
                None => self.err(format!("'{name}' has no field '{fname}'"), fexpr.span),
                Some((_, dty)) => {
                    if !is_gen && !compatible(dty, vty) {
                        self.err(
                            format!("field '{fname}' of '{name}' is set to a value of the wrong type"),
                            fexpr.span,
                        );
                    }
                }
            }
        }
        for (dn, _) in &declared {
            if !fields.iter().any(|(fname, _)| fname == dn) {
                self.err(format!("struct literal for '{name}' is missing field '{dn}'"), span);
            }
        }
    }

    fn func(&mut self, f: &Func, is_method: bool) {
        self.cur_generics = f.generics.iter().cloned().collect();
        self.cur_ret = lower(&f.ret, &self.cur_generics);
        self.branch_depth = 0;
        self.push_scope();
        if is_method {
            self.declare("self", Ty::Unknown);
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
            if matches!(&raw, Ty::Named(n) if self.ifaces.contains(n)) {
                self.declare_iface_bind(&p.name);
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
    }

    fn lower(&self, t: &Type) -> Ty {
        self.fix(lower(t, &self.cur_generics))
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.owns.push(HashMap::new());
        self.muts.push(HashSet::new());
        self.err_binds.push(HashMap::new());
        self.iface_binds.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.owns.pop();
        self.muts.pop();
        self.iface_binds.pop();
        // Every error bound in this scope must have been handled by now. The
        // handled ones were removed at their handling site; the rest report.
        if let Some(pending) = self.err_binds.pop() {
            let mut pending: Vec<(String, Span)> = pending.into_iter().collect();
            pending.sort_by_key(|(_, s)| s.lo);
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
    fn declare_iface_bind(&mut self, name: &str) {
        if let Some(scope) = self.iface_binds.last_mut() {
            scope.insert(name.to_string());
        }
    }

    fn is_iface_bind(&self, name: &str) -> bool {
        self.iface_binds.iter().rev().any(|s| s.contains(name))
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
        self.errors.push(Diagnostic::new(msg, span));
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

    /// A side effect free type walk for assignment chains. Unlike `infer`, it
    /// never emits diagnostics, so walking the same expression twice is safe.
    fn chain_ty(&self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Ident(n) => self.lookup(n),
            ExprKind::Field(base, fname) => match self.chain_ty(base) {
                Ty::Named(s) => self
                    .structs
                    .get(&s)
                    .and_then(|(_, fs)| fs.iter().find(|(n, _)| n == fname).map(|(_, t)| t.clone()))
                    .unwrap_or(Ty::Unknown),
                _ => Ty::Unknown,
            },
            ExprKind::Index(base, _) => elem_of(&self.chain_ty(base)),
            ExprKind::Unary(UnOp::Deref, p) => match self.chain_ty(p) {
                Ty::Ptr(inner) | Ty::RawPtr(inner) => *inner,
                _ => Ty::Unknown,
            },
            _ => Ty::Unknown,
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
        // Signedness is not tracked yet, so accept the union of the signed and
        // unsigned ranges for the width; a value outside both cannot be right.
        let lo = -(1i128 << (w - 1));
        let hi = (1i128 << w) - 1;
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
                if is_managed(&lt) {
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
            }
            Stmt::Return(Some(e)) => {
                let t = self.infer(e);
                let ret = self.cur_ret.clone();
                // Returning a concrete struct where an interface is declared is
                // the boxing site; it needs an impl, checked precisely here, and
                // the plain mismatch error would misfire on the valid case.
                let iface_ret = matches!((&ret, &t), (Ty::Named(i), Ty::Named(_)) if self.ifaces.contains(i));
                if iface_ret {
                    self.check_conformance(&ret, &t, e.span);
                } else if !compatible(&ret, &t) {
                    self.err("return type does not match the function's return type", e.span);
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
                self.branch_block(&w.body);
            }
            Stmt::For(f) => {
                self.infer(&f.iter);
                self.push_scope();
                self.declare(&f.var, Ty::Unknown);
                self.branch_block(&f.body);
                self.pop_scope();
            }
            Stmt::Match(m) => self.walk_match(m),
            Stmt::Expr(e) => {
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
        if l.binds.len() == 1 {
            let b = &l.binds[0];
            let ty = match &b.ty {
                Some(t) => {
                    let lt = self.lower(t);
                    // The annotation with the interface name intact, so binding a
                    // struct to an interface checks its impl instead of emitting
                    // a reference to a vtable that does not exist.
                    let raw = lower(t, &self.cur_generics);
                    self.check_conformance(&raw, &vt, l.value.span);
                    if matches!(&raw, Ty::Named(n) if self.ifaces.contains(n)) {
                        self.declare_iface_bind(&b.name);
                    }
                    if !compatible(&lt, &vt) {
                        self.err(format!("'{}' has a type annotation that does not match its value", b.name), l.value.span);
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
            if l.mutable {
                self.declare_mut(&b.name);
            }
            if matches!(ty, Ty::Error) {
                self.declare_err(&b.name, l.value.span);
            }
            if is_managed(&ty) {
                let own = self.binding_own(l, &vt);
                self.declare_own(&b.name, own);
            }
            return;
        }
        let parts = match &vt {
            Ty::Tuple(ts) if ts.len() == l.binds.len() => ts.clone(),
            Ty::Unknown => vec![Ty::Unknown; l.binds.len()],
            _ => {
                self.err("destructuring binding expects a tuple of matching arity", l.value.span);
                vec![Ty::Unknown; l.binds.len()]
            }
        };
        for (b, pt) in l.binds.iter().zip(parts) {
            let ty = match &b.ty {
                Some(t) => self.lower(t),
                None => harden(pt),
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
                    self.err(
                        "cannot copy an owning pointer; bind a `ref` alias or `move` it",
                        l.value.span,
                    );
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

    /// Rejects the clear cases of a value escaping its frame through a return: a
    /// slice that views a frame local fixed array, and a closure that captures a
    /// local. A managed pointer escape is covered by the generation check, not
    /// here, since dusk has no address of operator and so every pointer is heap.
    fn check_escape(&mut self, e: &Expr, t: &Ty) {
        // A slice that views a frame local array dangles once the function
        // returns and the stack array is reclaimed. A heap backed slice, like a
        // map result, or a slice parameter, whose backing the caller owns, is
        // fine. The backing is a local array exactly when the sliced base, or the
        // returned value itself for the array to slice coercion, has array type.
        if matches!(self.cur_ret, Ty::Slice(_)) {
            let backs_local_array = match &e.kind {
                ExprKind::Index(base, idx) if matches!(idx.kind, ExprKind::Range(..)) => {
                    matches!(self.infer(base), Ty::Array(..))
                }
                // An array literal materializes in this frame, so returning it
                // as a slice views a dead frame no matter what its type says.
                ExprKind::Array(_) => true,
                _ => matches!(t, Ty::Array(..)),
            };
            if backs_local_array {
                self.err(
                    "a slice into a local array escapes its frame; put the backing on the heap",
                    e.span,
                );
            }
        }
        // A closure that captures a frame local has its environment on the stack,
        // reclaimed at return, so the escaped closure dangles. A closure with no
        // captures is a plain function pointer and may be returned.
        if let ExprKind::Lambda(l) = &e.kind {
            if self.lambda_captures_local(l) {
                self.err(
                    "a closure that captures a local escapes its frame; it cannot be returned",
                    e.span,
                );
            }
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
            if !self.spawn_capturable(&t, &mut seen) || self.is_iface_bind(c) {
                self.err(
                    format!(
                        "{name} cannot capture '{c}'; a slice, closure, or interface value may view the spawning frame, so move the data to the heap or send it through a channel"
                    ),
                    args[0].span,
                );
            }
            if is_managed(&t) && !matches!(self.own_of(c), Some(Own::Moved)) {
                borrowed.push(c.clone());
            }
        }
        self.infer_lambda(l, args[0].span, &borrowed);
    }

    /// Whether a value of this type may cross a `spawn` as a heap copied
    /// capture. A slice, a closure, or an interface value may carry a pointer
    /// into the spawning frame that the copy would dangle, so they are rejected
    /// wherever they sit, including inside a struct or enum field. A pointer
    /// field is fine: every pointer targets the heap, where the generation
    /// check covers it. `seen` breaks recursive type cycles.
    fn spawn_capturable(&self, t: &Ty, seen: &mut HashSet<String>) -> bool {
        match t {
            Ty::Slice(_) | Ty::Func(..) => false,
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
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Ident(name) => {
                if matches!(self.own_of(name), Some(Own::Moved)) {
                    self.err("use of a moved pointer", e.span);
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
                self.check_binary(*op, &ta, &tb, e.span)
            }
            ExprKind::Call(f, args) => self.infer_call(f, args),
            ExprKind::Field(x, _) => {
                self.infer(x);
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
            ExprKind::Range(a, b) => {
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
                named_ty(name)
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
            ExprKind::SizeofType(_) => Ty::Int(64),
        }
    }

    /// Rejects a suffixed literal whose value cannot fit its own suffix, like
    /// `300i8`, which would silently truncate in codegen.
    fn check_int_fits_suffixed(&mut self, v: i64, w: u32, span: Span) {
        if w >= 64 {
            return;
        }
        let val = v as i128;
        let lo = -(1i128 << (w - 1));
        let hi = (1i128 << w) - 1;
        if val < lo || val > hi {
            self.err(format!("literal {val} does not fit in {w} bits"), span);
        }
    }

    fn infer_call(&mut self, f: &Expr, args: &[Expr]) -> Ty {
        // Method call syntax. The builtin `error` methods have known types; every
        // other method call stays permissive and returns Unknown for now.
        if let ExprKind::Field(base, mname) = &f.kind {
            let is_error = matches!(self.infer(base), Ty::Error);
            for a in args {
                self.infer(a);
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
            if !self.sigs.contains_key(name) {
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
                    if let Some(a) = args.first() {
                        if let ExprKind::Ident(src) = &a.kind {
                            if matches!(self.own_of(src), Some(Own::Borrow)) {
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
                        if is_managed(&t) {
                            if let ExprKind::Ident(p) = &a.kind {
                                if matches!(self.own_of(p), Some(Own::Borrow)) {
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
        let callee = self.infer(f);
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer(a)).collect();
        if let Ty::Func(params, ret) = callee {
            if params.len() != arg_tys.len() {
                self.err(
                    format!("expected {} argument(s), found {}", params.len(), arg_tys.len()),
                    f.span,
                );
            } else {
                for (i, (p, a)) in params.iter().zip(&arg_tys).enumerate() {
                    if !compatible(p, a) {
                        self.err(format!("argument {} has the wrong type", i + 1), args[i].span);
                    }
                }
                // The fixed parameter types lower an interface to Unknown, so they
                // accept any value. The unfixed signature keeps the interface name,
                // so a concrete struct passed for an interface is checked here.
                if let ExprKind::Ident(name) = &f.kind {
                    if let Some(raw) = self.raw_sigs.get(name).cloned() {
                        for (rp, a) in raw.iter().zip(&arg_tys) {
                            self.check_conformance(rp, a, f.span);
                        }
                    }
                }
            }
            return *ret;
        }
        Ty::Unknown
    }

    /// Rejects passing a concrete struct where an interface is expected unless the
    /// struct implements it. Only fires when both sides are known by name, an
    /// interface expected and a concrete struct given, so an Unknown or a generic
    /// stays permissive.
    fn check_conformance(&mut self, expected: &Ty, actual: &Ty, span: Span) {
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
                        format!("format string has {holes} hole(s) but {given} argument(s) were given"),
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
            Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char | Ty::Str | Ty::Error | Ty::Unknown => {}
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
            Ty::Ptr(_) | Ty::RawPtr(_) => {
                self.err("cannot print a pointer; dereference it or print its fields", span)
            }
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
        self.branch_depth += 1;
        for arm in &m.arms {
            self.push_scope();
            match &arm.pat {
                Pattern::Variant(_, binds) => {
                    for b in binds {
                        self.declare(b, Ty::Unknown);
                    }
                }
                Pattern::Ident(name) => self.declare(name, Ty::Unknown),
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
                        format!("match needs an enum value, and {} is not an enum", ty_str(&st)),
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
                        self.err(format!("unreachable match arm, '{n}' is already covered"), span);
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
        }
    }

    fn check_binary(&mut self, op: BinOp, a: &Ty, b: &Ty, span: Span) -> Ty {
        use BinOp::*;
        let unknown = matches!(a, Ty::Unknown) || matches!(b, Ty::Unknown);
        match op {
            Add | Sub | Mul | Div | Mod => {
                if unknown {
                    return if matches!(a, Ty::Unknown) { b.clone() } else { a.clone() };
                }
                match (a, b) {
                    // Same kind: the widths must agree, with a bare literal
                    // (width 0) adapting to the other side. Mixing widths would
                    // silently truncate in codegen, so it is an error here.
                    (Ty::Int(x), Ty::Int(y)) | (Ty::Float(x), Ty::Float(y)) => {
                        if x == y || *x == 0 || *y == 0 {
                            let w = (*x).max(*y);
                            if matches!(a, Ty::Int(_)) { Ty::Int(w) } else { Ty::Float(w) }
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
                        self.err("arithmetic needs two operands of the same numeric type", span);
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
        }
    }
}

fn elem_of(t: &Ty) -> Ty {
    match t {
        Ty::Slice(e) | Ty::Array(e, _) => (**e).clone(),
        Ty::Str => Ty::Char,
        _ => Ty::Unknown,
    }
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
        Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char | Ty::Unit => true,
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
        (Ty::Int(x), Ty::Int(y)) | (Ty::Float(x), Ty::Float(y)) => {
            *x == 0 || *y == 0 || x == y
        }
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
        (Ty::Ptr(x), Ty::Ptr(y)) => {
            matches!(**x, Ty::Unit) || matches!(**y, Ty::Unit) || compatible(x, y)
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
        _ => false,
    }
}

fn lower(t: &Type, generics: &HashSet<String>) -> Ty {
    match t {
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
        Type::Unit => Ty::Unit,
    }
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
        check(&m)
    }

    #[test]
    fn arithmetic_mismatch() {
        let e = errs("func f() -> int64 { return 1 + true }");
        assert!(!e.is_empty());
    }

    #[test]
    fn foreign_managed_pointer_rejected() {
        let e = errs(
            "foreign \"C\" { func bad(p: *int64) -> int32 }\nfunc main() -> int32 { return 0 }",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("managed pointer at the C boundary")),
            "{e:?}"
        );
    }

    #[test]
    fn foreign_bad_abi_rejected() {
        let e = errs(
            "foreign \"Rust\" { func abs(n: int32) -> int32 }\nfunc main() -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg.contains("only \"C\" is supported")), "{e:?}");
    }

    #[test]
    fn foreign_raw_pointer_ok() {
        let e = errs(
            "foreign \"C\" { func memset(dst: *raw int8, c: int32, n: int64) -> *void }\n\
             func main() -> int32 { return 0 }",
        );
        assert!(e.is_empty(), "raw pointer boundary should be accepted: {e:?}");
    }

    #[test]
    fn struct_literal_unknown_field_rejected() {
        let e = errs("struct S { x: int64 }\nfunc main() -> int32 {\n  s := S { y: 5 }\n  return 0\n}");
        assert!(e.iter().any(|d| d.msg.contains("no field 'y'")), "{e:?}");
    }

    #[test]
    fn struct_literal_missing_field_rejected() {
        let e = errs("struct S { x: int64, y: int64 }\nfunc main() -> int32 {\n  s := S { x: 1 }\n  return 0\n}");
        assert!(e.iter().any(|d| d.msg.contains("missing field 'y'")), "{e:?}");
    }

    #[test]
    fn struct_literal_complete_ok() {
        let e = errs("struct S { x: int64, y: int64 }\nfunc main() -> int32 {\n  s := S { x: 1, y: 2 }\n  return 0\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn passing_struct_without_impl_is_rejected() {
        let e = errs("interface I { get() -> int64 }\nstruct S { x: int64 }\nfunc take(i: I) -> int64 { return i.get() }\nfunc f() -> void {\n  s := S { x: 1 }\n  take(s)\n}");
        assert!(e.iter().any(|d| d.msg.contains("does not implement interface 'I'")), "{e:?}");
    }

    #[test]
    fn incomplete_impl_is_rejected() {
        let e = errs("interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { }");
        assert!(e.iter().any(|d| d.msg.contains("missing method 'get'")), "{e:?}");
    }

    #[test]
    fn struct_with_impl_satisfies_interface() {
        let e = errs("interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return self.x } }\nfunc take(i: I) -> int64 { return i.get() }\nfunc f() -> void {\n  s := S { x: 1 }\n  take(s)\n}");
        assert!(!e.iter().any(|d| d.msg.contains("implement interface")), "{e:?}");
    }

    #[test]
    fn discarding_a_fallible_call_is_rejected() {
        let e = errs("func fail() -> error { return error { message: \"x\" } }\nfunc f() -> void {\n  fail()\n}");
        assert!(e.iter().any(|d| d.msg.contains("error result is ignored")), "{e:?}");
    }

    #[test]
    fn handling_a_fallible_result_is_ok() {
        let e = errs("func fail() -> error { return error { message: \"x\" } }\nfunc f() -> void {\n  e := fail()\n  e.ignore()\n}");
        assert!(!e.iter().any(|d| d.msg.contains("error result is ignored")), "{e:?}");
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
        let e = errs("func g(a: int64) -> int64 { return a }\nfunc f() -> int64 { return g(1, 2) }");
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
        assert!(e.iter().any(|d| d.msg.contains("copy an owning pointer")), "{e:?}");
    }

    #[test]
    fn use_after_move_is_rejected() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc(5)\n  q: *int64 = move(p)\n  free(p)\n  free(q)\n}");
        assert!(e.iter().any(|d| d.msg.contains("moved pointer")), "{e:?}");
    }

    #[test]
    fn freeing_a_borrow_is_rejected() {
        let e = errs("func sink(p: *int64) -> void {\n  free(p)\n}");
        assert!(e.iter().any(|d| d.msg.contains("borrowed pointer")), "{e:?}");
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
        assert!(e.iter().any(|d| d.msg.contains("copy an owning pointer")), "{e:?}");
    }

    #[test]
    fn reassigning_an_owner_is_rejected() {
        let e = errs("func f() -> void {\n  mut p: *int64 = alloc(5)\n  p = alloc(9)\n  free(p)\n}");
        assert!(e.iter().any(|d| d.msg.contains("reassign an owning pointer")), "{e:?}");
    }

    #[test]
    fn reassigning_a_borrow_cursor_is_allowed() {
        // A borrowing cursor, like a list walk variable, is not an owner, so it
        // may advance without tripping the reassignment or copy rules.
        let e = errs("func walk(head: *int64) -> void {\n  mut cur: *int64 = head\n  cur = head\n  println(*cur)\n}");
        assert!(
            !e.iter().any(|d| d.msg.contains("reassign") || d.msg.contains("owning")),
            "{e:?}"
        );
    }

    #[test]
    fn moving_a_borrow_is_rejected() {
        let e = errs("func sink(p: *int64) -> void {\n  q: *int64 = move(p)\n  free(q)\n}");
        assert!(e.iter().any(|d| d.msg.contains("move a borrowed pointer")), "{e:?}");
    }

    #[test]
    fn inferred_alloc_binding_is_an_owner() {
        // alloc infers to a managed pointer, so the inferred `:=` form is tracked
        // and a copy of it is the single owner violation.
        let e = errs("func f() -> void {\n  x := alloc(5)\n  y := x\n  free(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("copy an owning pointer")), "{e:?}");
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
        assert!(e.iter().any(|d| d.msg.contains("escapes its frame")), "{e:?}");
    }

    #[test]
    fn capturing_closure_cannot_escape() {
        let e = errs(
            "func f() -> (int64) -> int64 {\n  x: int64 = 10\n  return lambda (n: int64) -> int64 { return n + x }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("escapes its frame")), "{e:?}");
    }

    #[test]
    fn returning_a_slice_of_a_slice_parameter_is_allowed() {
        let e = errs("func f(xs: int64[]) -> int64[] {\n  return xs[0..2]\n}");
        assert!(!e.iter().any(|d| d.msg.contains("escapes")), "{e:?}");
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
        assert!(e.iter().any(|d| d.msg.contains("mixes int32 and int64")), "{e:?}");
    }

    #[test]
    fn literal_adapts_to_any_width() {
        let e = errs(
            "func f() -> int32 {\n  a: int32 = 5\n  b := a + 1\n  return b\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn cross_width_assignment_is_rejected() {
        let e = errs("func f(w: int64) -> void {\n  x: int8 = w\n  println(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("annotation that does not match")), "{e:?}");
    }

    #[test]
    fn literal_too_wide_for_annotation_is_rejected() {
        let e = errs("func f() -> void {\n  x: int8 = 300\n  println(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("does not fit in 8 bits")), "{e:?}");
    }

    #[test]
    fn suffixed_literal_out_of_range_is_rejected() {
        let e = errs("func f() -> void {\n  x := 300i8\n  println(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("does not fit in 8 bits")), "{e:?}");
    }

    #[test]
    fn returning_an_array_literal_as_a_slice_is_rejected() {
        let e = errs("func make() -> int64[] { return [1, 2, 3] }");
        assert!(e.iter().any(|d| d.msg.contains("escapes its frame")), "{e:?}");
    }

    #[test]
    fn unsized_alloc_needs_a_pointer_annotation() {
        let e = errs("func f() -> void {\n  x := alloc()\n  free(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("pointer type annotation")), "{e:?}");
    }

    #[test]
    fn unsized_alloc_with_pointer_annotation_is_ok() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc()\n  *p = 5\n  free(p)\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn element_store_into_immutable_array_is_rejected() {
        let e = errs("func f() -> void {\n  xs: int64[3] = [1, 2, 3]\n  xs[0] = 99\n  println(xs[0])\n}");
        assert!(e.iter().any(|d| d.msg.contains("assign through immutable 'xs'")), "{e:?}");
    }

    #[test]
    fn field_store_into_immutable_struct_is_rejected() {
        let e = errs(
            "struct P { x: int64 }\nfunc f() -> void {\n  p := P { x: 1 }\n  p.x = 42\n  println(p.x)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("assign through immutable 'p'")), "{e:?}");
    }

    #[test]
    fn element_store_into_mut_array_is_ok() {
        let e = errs("func f() -> void {\n  mut xs: int64[3] = [1, 2, 3]\n  xs[0] = 99\n  println(xs[0])\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn store_through_a_pointer_deref_is_ok() {
        // Mutation through a pointer writes the pointee, not the binding, so the
        // binding's immutability does not apply.
        let e = errs(
            "struct P { x: int64 }\nfunc f(p: *P) -> void {\n  (*p).x = 42\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("immutable")), "{e:?}");
    }

    #[test]
    fn field_store_through_pointer_needs_explicit_deref() {
        let e = errs(
            "struct P { x: int64 }\nfunc f(p: *P) -> void {\n  p.x = 42\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("explicit dereference")), "{e:?}");
    }

    #[test]
    fn binding_a_struct_to_an_interface_needs_an_impl() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nfunc f() -> void {\n  s := S { x: 7 }\n  i: I = s\n  println(i.get())\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("does not implement interface 'I'")), "{e:?}");
    }

    #[test]
    fn binding_a_struct_to_an_interface_with_impl_is_ok() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return self.x } }\nfunc f() -> void {\n  s := S { x: 7 }\n  i: I = s\n  println(i.get())\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("implement interface")), "{e:?}");
    }

    #[test]
    fn returning_a_struct_as_an_interface_needs_an_impl() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nfunc mk() -> I {\n  return S { x: 7 }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("does not implement interface 'I'")), "{e:?}");
    }

    #[test]
    fn match_over_a_non_enum_is_rejected() {
        let e = errs(
            "func f() -> void {\n  x := 5\n  match x {\n    a => println(1),\n    b => println(2),\n  }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("match needs an enum value")), "{e:?}");
    }

    #[test]
    fn defer_inside_a_conditional_is_rejected() {
        let e = errs(
            "func f() -> void {\n  if true {\n    defer println(1)\n  }\n  println(2)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("defer inside a conditional or loop")), "{e:?}");
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
        assert!(e.iter().any(|d| d.msg.contains("'e' is never handled")), "{e:?}");
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
    fn printing_a_struct_without_display_is_rejected() {
        let e = errs(
            "struct P { x: int64 }\nfunc f() -> void {\n  p := P { x: 1 }\n  println(p)\n}",
        );
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
        assert!(e.iter().any(|d| d.msg.contains("1 hole(s) but 0 argument(s)")), "{e:?}");
    }

    #[test]
    fn missing_return_on_a_path_is_rejected() {
        let e = errs("func f() -> int64 {\n  println(1)\n}");
        assert!(e.iter().any(|d| d.msg.contains("not all paths in 'f' return")), "{e:?}");
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
        assert!(e.iter().any(|d| d.msg.contains("duplicate 'impl I for S'")), "{e:?}");
    }

    #[test]
    fn allocator_main_form_is_rejected_for_now() {
        let e = errs(
            "func main(argc: int32, argv: string[], using a: int64) -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg.contains("allocator form is not supported yet")), "{e:?}");
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
        assert!(e.iter().any(|d| d.msg.contains("no parameters and returns void")), "{e:?}");
    }

    #[test]
    fn spawn_rejects_a_slice_capture() {
        let e = errs(
            "func f(xs: int64[]) -> void {\n  t, e := spawn(lambda () -> void {\n    println(xs[0])\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("cannot capture 'xs'")), "{e:?}");
    }

    #[test]
    fn spawned_thread_cannot_free_a_captured_pointer() {
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(5)\n  t, e := spawn(lambda () -> void {\n    free(p)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n  free(p)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("borrowed pointer")), "{e:?}");
    }

    #[test]
    fn a_dropped_join_error_is_rejected() {
        let e = errs(
            "func f() -> void {\n  t, e := spawn(lambda () -> void { println(1) })\n  e.ignore()\n  join(t)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("error result is ignored")), "{e:?}");
    }

    #[test]
    fn spawn_rejects_a_slice_smuggled_in_a_struct_field() {
        let e = errs(
            "struct Wrap { s: int64[] }\nfunc f(xs: int64[]) -> void {\n  w := Wrap { s: xs }\n  t, e := spawn(lambda () -> void {\n    println(w.s[0])\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("cannot capture 'w'")), "{e:?}");
    }

    #[test]
    fn spawn_rejects_an_interface_value_capture() {
        let e = errs(
            "@paradigm oop\ninterface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return self.x } }\nfunc f() -> void {\n  s := S { x: 1 }\n  i: I = s\n  t, e := spawn(lambda () -> void {\n    println(i.get())\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("cannot capture 'i'")), "{e:?}");
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
}
