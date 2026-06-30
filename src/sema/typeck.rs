//! Type checking. M4.
//!
//! Coarse types: integers collapse to one `Int` and floats to one `Float` for now;
//! width checking lands later. Structs, enums, generics, methods, and imported types
//! become `Unknown`, which is compatible with everything, so advanced code is accepted
//! without false errors while real core type errors are still caught. The unused
//! variable and must handle error rules are enforced by the resolver pass (M3).

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

#[derive(Clone, Debug, PartialEq)]
enum Ty {
    Int,
    Float,
    Bool,
    Char,
    Str,
    Unit,
    Error,
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
    scopes: Vec<HashMap<String, Ty>>,
    owns: Vec<HashMap<String, Own>>,
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
            scopes: Vec::new(),
            owns: Vec::new(),
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
                }
                Item::Enum(e) => {
                    let variants = e.variants.iter().map(|v| v.name.clone()).collect();
                    self.enums.insert(e.name.clone(), variants);
                }
                _ => {}
            }
        }
        for item in &module.items {
            match item {
                Item::Func(f) => {
                    let gens: HashSet<String> = f.generics.iter().cloned().collect();
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
                Item::Func(f) => self.func(f, false),
                Item::Impl(im) => {
                    for m in &im.methods {
                        self.func(m, true);
                    }
                }
                Item::Foreign(fb) => self.check_foreign(fb),
                _ => {}
            }
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
                Span::new(0, 0),
            );
        }
        let empty = HashSet::new();
        for ff in &fb.funcs {
            for p in &ff.params {
                let ty = self.fix(lower(&p.ty, &empty));
                self.check_foreign_ty(&ty, &ff.name);
            }
            let ret = self.fix(lower(&ff.ret, &empty));
            self.check_foreign_ty(&ret, &ff.name);
        }
    }

    fn check_foreign_ty(&mut self, ty: &Ty, fname: &str) {
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
        self.err(msg, Span::new(0, 0));
    }

    fn func(&mut self, f: &Func, is_method: bool) {
        self.cur_generics = f.generics.iter().cloned().collect();
        self.cur_ret = lower(&f.ret, &self.cur_generics);
        self.push_scope();
        if is_method {
            self.declare("self", Ty::Unknown);
        }
        for p in &f.params {
            let ty = self.lower(&p.ty);
            // A pointer parameter is a borrow with no keyword; the callee never
            // owns or frees it.
            if is_managed(&ty) {
                self.declare_own(&p.name, Own::Borrow);
            }
            self.declare(&p.name, ty);
        }
        for s in &f.body.stmts {
            self.stmt(s);
        }
        self.pop_scope();
    }

    fn lower(&self, t: &Type) -> Ty {
        self.fix(lower(t, &self.cur_generics))
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.owns.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.owns.pop();
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

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(l) => self.let_stmt(l),
            Stmt::Assign(lhs, rhs) => {
                let lt = self.infer(lhs);
                let rt = self.infer(rhs);
                if !compatible(&lt, &rt) {
                    self.err("assignment type mismatch", lhs.span);
                }
                // `q = p` aliases two owners exactly like the let form copy, so
                // flag it the same rather than leaving it to the runtime.
                if is_managed(&lt) {
                    if let (ExprKind::Ident(_), ExprKind::Ident(src)) = (&lhs.kind, &rhs.kind) {
                        if matches!(self.own_of(src), Some(Own::Owner)) {
                            self.err(
                                "cannot copy an owning pointer; bind a `ref` alias or `move` it",
                                rhs.span,
                            );
                        }
                    }
                }
            }
            Stmt::Return(Some(e)) => {
                let t = self.infer(e);
                if !compatible(&self.cur_ret.clone(), &t) {
                    self.err("return type does not match the function's return type", e.span);
                }
                self.check_escape(e, &t);
            }
            Stmt::Return(None) => {
                if !compatible(&self.cur_ret.clone(), &Ty::Unit) {
                    self.err("missing return value", Span::new(0, 0));
                }
            }
            Stmt::Defer(e) => {
                self.infer(e);
            }
            Stmt::If(i) => {
                let c = self.infer(&i.cond);
                if !compatible(&Ty::Bool, &c) {
                    self.err("if condition must be a bool", i.cond.span);
                }
                self.block(&i.then);
                if let Some(els) = &i.els {
                    self.block(els);
                }
            }
            Stmt::While(w) => {
                let c = self.infer(&w.cond);
                if !compatible(&Ty::Bool, &c) {
                    self.err("while condition must be a bool", w.cond.span);
                }
                self.block(&w.body);
            }
            Stmt::For(f) => {
                self.infer(&f.iter);
                self.push_scope();
                self.declare(&f.var, Ty::Unknown);
                self.block(&f.body);
                self.pop_scope();
            }
            Stmt::Match(m) => self.walk_match(m),
            Stmt::Expr(e) => {
                self.infer(e);
            }
        }
    }

    fn let_stmt(&mut self, l: &Let) {
        let vt = self.infer(&l.value);
        if l.binds.len() == 1 {
            let b = &l.binds[0];
            let ty = match &b.ty {
                Some(t) => {
                    let lt = self.lower(t);
                    if !compatible(&lt, &vt) {
                        self.err(format!("'{}' has a type annotation that does not match its value", b.name), l.value.span);
                    }
                    lt
                }
                None => vt.clone(),
            };
            self.declare(&b.name, ty.clone());
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
                None => pt,
            };
            // A destructured managed pointer is conservatively an owner, so it is
            // tracked and freeing it is not wrongly rejected. The static copy
            // check for the destructure itself falls to the runtime backstop.
            if is_managed(&ty) {
                self.declare_own(&b.name, Own::Owner);
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
            ExprKind::Int(..) => Ty::Int,
            ExprKind::Float(..) => Ty::Float,
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
                for (_, v) in fields {
                    self.infer(v);
                }
                named_ty(name)
            }
            ExprKind::Lambda(l) => self.infer_lambda(l),
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
            ExprKind::SizeofType(_) => Ty::Int,
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
                // print and println take an optional format string. With a value
                // argument the first argument is a literal whose holes the rest
                // fill, checked here so codegen can expand it directly.
                if (name == "print" || name == "println") && !args.is_empty() {
                    for a in args {
                        self.infer(a);
                    }
                    if args.len() >= 2 {
                        self.check_format(args);
                    }
                    return Ty::Unit;
                }
                if name == "alloc" {
                    // alloc(v) yields a managed *T owner of the value's type, so
                    // the single owner pass engages even for the inferred form.
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
            }
            return *ret;
        }
        Ty::Unknown
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

    fn infer_lambda(&mut self, l: &Lambda) -> Ty {
        let saved_ret = self.cur_ret.clone();
        self.cur_ret = self.lower(&l.ret);
        let params: Vec<Ty> = l.params.iter().map(|p| self.lower(&p.ty)).collect();
        self.push_scope();
        for (p, ty) in l.params.iter().zip(&params) {
            self.declare(&p.name, ty.clone());
        }
        for s in &l.body.stmts {
            self.stmt(s);
        }
        self.pop_scope();
        let ret = self.cur_ret.clone();
        self.cur_ret = saved_ret;
        Ty::Func(params, Box::new(ret))
    }

    fn walk_match(&mut self, m: &Match) {
        let st = self.infer(&m.scrut);
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
        if let Ty::Named(ename) = &st {
            if let Some(variants) = self.enums.get(ename).cloned() {
                self.check_coverage(&variants, &m.arms, m.scrut.span);
            }
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
                if matches!(t, Ty::Int | Ty::Float | Ty::Unknown) {
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
                if a == b && matches!(a, Ty::Int | Ty::Float) {
                    a.clone()
                } else {
                    self.err("arithmetic needs two operands of the same numeric type", span);
                    Ty::Unknown
                }
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                if !compatible(a, b) {
                    self.err("comparison needs two operands of the same type", span);
                }
                Ty::Bool
            }
            And | Or => {
                if !unknown && !(matches!(a, Ty::Bool) && matches!(b, Ty::Bool)) {
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
        Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::Unit => true,
        Ty::RawPtr(_) => true,
        Ty::Ptr(inner) => matches!(**inner, Ty::Unit),
        _ => false,
    }
}

fn compatible(a: &Ty, b: &Ty) -> bool {
    if a == b || matches!(a, Ty::Unknown) || matches!(b, Ty::Unknown) {
        return true;
    }
    match (a, b) {
        (Ty::Array(x, _), Ty::Slice(y)) | (Ty::Slice(x), Ty::Array(y, _)) => compatible(x, y),
        (Ty::Slice(x), Ty::Slice(y)) => compatible(x, y),
        (Ty::Array(x, n), Ty::Array(y, m)) if n == m => compatible(x, y),
        (Ty::Ptr(x), Ty::Ptr(y)) => {
            matches!(**x, Ty::Unit) || matches!(**y, Ty::Unit) || compatible(x, y)
        }
        // A char is an ASCII byte, freely compared with and assigned to ints.
        (Ty::Char, Ty::Int) | (Ty::Int, Ty::Char) => true,
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

fn named_ty(n: &str) -> Ty {
    match n {
        "int8" | "int16" | "int32" | "int64" | "uint8" | "uint16" | "uint32" | "uint64" => Ty::Int,
        "float32" | "float64" => Ty::Float,
        "bool" => Ty::Bool,
        "char" => Ty::Char,
        "string" => Ty::Str,
        "error" => Ty::Error,
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
        "parse_float" => Some(Ty::Tuple(vec![Ty::Float, Ty::Error])),
        "write_file" => Some(Ty::Error),
        "cstr" => Some(Ty::Str),
        _ => None,
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
}
