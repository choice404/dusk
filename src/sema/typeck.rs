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

struct TypeChecker {
    sigs: HashMap<String, (Vec<Ty>, Ty)>,
    ifaces: HashSet<String>,
    enums: HashMap<String, Vec<String>>,
    scopes: Vec<HashMap<String, Ty>>,
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
            if let Item::Func(f) = item {
                let gens: HashSet<String> = f.generics.iter().cloned().collect();
                let params = f.params.iter().map(|p| self.fix(lower(&p.ty, &gens))).collect();
                let ret = self.fix(lower(&f.ret, &gens));
                self.sigs.insert(f.name.clone(), (params, ret));
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
                _ => {}
            }
        }
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
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
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

    fn err(&mut self, msg: impl Into<String>, span: Span) {
        self.errors.push(Diagnostic::new(msg, span));
    }

    fn block(&mut self, b: &Block) {
        self.push_scope();
        for s in &b.stmts {
            self.stmt(s);
        }
        self.pop_scope();
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
            }
            Stmt::Return(Some(e)) => {
                let t = self.infer(e);
                if !compatible(&self.cur_ret.clone(), &t) {
                    self.err("return type does not match the function's return type", e.span);
                }
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
                None => vt,
            };
            self.declare(&b.name, ty);
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
            self.declare(&b.name, ty);
        }
    }

    fn infer(&mut self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Int(..) => Ty::Int,
            ExprKind::Float(..) => Ty::Float,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Char(_) => Ty::Char,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Ident(name) => self.lookup(name),
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
