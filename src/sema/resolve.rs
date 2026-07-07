//! Name resolution and scope checks. M3.
//!
//! Resolves value identifiers against lexical scopes, top level items, enum
//! variants, builtins, and imported names. Enforces the unused variable rule for
//! `let` bindings, immutability, and the function scope mutation rule: a closure
//! may read an outer variable but may not mutate it.
//!
//! Type name resolution and the must handle error rule need type information and
//! land in M4. Imported names are accepted leniently until the module system (M10).

use std::collections::{HashMap, HashSet};

use crate::diag::{Diagnostic, Span};
use crate::parser::ast::*;

const BUILTINS: &[&str] = &[
    "alloc", "free", "print", "println", "printerr", "sizeof", "alloc_bytes", "ptr_add", "map",
    "filter", "reduce", "fold", "foreach", "debug_alloc", "debug_free", "debug_leaks",
    "debug_double_frees", "read_file", "write_file", "read_line", "read_all", "parse_float",
    "cstr", "move", "spawn", "join", "submit", "async_run",
];

/// Resolves names and checks scope rules for a module, returning diagnostics.
pub fn check(module: &Module) -> Vec<Diagnostic> {
    let mut r = Resolver::new(module);
    r.run(module);
    r.errors
}

struct Var {
    mutable: bool,
    depth: u32,
    used: bool,
    is_let: bool,
    span: Span,
}

struct Resolver {
    globals: HashSet<String>,
    imports: HashSet<String>,
    scopes: Vec<HashMap<String, Var>>,
    cur_generics: HashSet<String>,
    depth: u32,
    errors: Vec<Diagnostic>,
}

impl Resolver {
    fn new(module: &Module) -> Self {
        let mut globals = HashSet::new();
        for item in &module.items {
            match item {
                Item::Func(f) => {
                    globals.insert(f.name.clone());
                }
                Item::Struct(s) => {
                    globals.insert(s.name.clone());
                }
                Item::Enum(e) => {
                    globals.insert(e.name.clone());
                    for v in &e.variants {
                        globals.insert(v.name.clone());
                    }
                }
                Item::Interface(i) => {
                    globals.insert(i.name.clone());
                }
                Item::Impl(_) => {}
                Item::Foreign(fb) => {
                    for ff in &fb.funcs {
                        globals.insert(ff.name.clone());
                    }
                }
            }
        }
        // Only the namespace root is accepted leniently. Real imported symbols
        // are merged as globals by the loader, so the leaf segment is not added
        // here, which lets a typo'd call surface as a clear undefined name error.
        let mut imports = HashSet::new();
        for imp in &module.imports {
            if let Some(first) = imp.split('.').next() {
                imports.insert(first.to_string());
            }
        }
        Resolver {
            globals,
            imports,
            scopes: Vec::new(),
            cur_generics: HashSet::new(),
            depth: 0,
            errors: Vec::new(),
        }
    }

    fn run(&mut self, module: &Module) {
        let mut seen = HashSet::new();
        for item in &module.items {
            if let Some(name) = item_name(item) {
                if !seen.insert(name.to_string()) {
                    self.errors.push(Diagnostic::new(
                        format!("duplicate definition of '{name}'"),
                        Span::new(0, 0),
                    ));
                }
            }
        }
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
        self.push_scope();
        if is_method {
            self.declare("self".into(), false, false, Span::new(0, 0));
        }
        for p in &f.params {
            self.declare(p.name.clone(), false, false, Span::new(0, 0));
        }
        self.block(&f.body);
        self.pop_scope();
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        let scope = self.scopes.pop().unwrap();
        for (name, var) in scope {
            if var.is_let && !var.used {
                self.errors
                    .push(Diagnostic::new(format!("unused variable '{name}'"), var.span));
            }
        }
    }

    fn declare(&mut self, name: String, mutable: bool, is_let: bool, span: Span) {
        if name.is_empty() {
            return;
        }
        let dup = self.scopes.last().unwrap().contains_key(&name);
        if dup {
            self.errors.push(Diagnostic::new(
                format!("'{name}' is already declared in this scope"),
                span,
            ));
        }
        self.scopes.last_mut().unwrap().insert(
            name,
            Var {
                mutable,
                depth: self.depth,
                used: false,
                is_let,
                span,
            },
        );
    }

    fn use_name(&mut self, name: &str, span: Span) {
        if name.is_empty() {
            return;
        }
        for scope in self.scopes.iter_mut().rev() {
            if let Some(var) = scope.get_mut(name) {
                var.used = true;
                return;
            }
        }
        if self.globals.contains(name) || self.imports.contains(name) || BUILTINS.contains(&name) {
            return;
        }
        self.errors
            .push(Diagnostic::new(format!("undefined name '{name}'"), span));
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
            Stmt::Let(l) => {
                self.expr(&l.value);
                for bind in &l.binds {
                    self.declare(bind.name.clone(), l.mutable, true, l.value.span);
                }
            }
            Stmt::Assign(lhs, rhs) => {
                self.expr(rhs);
                self.assign_target(lhs);
            }
            Stmt::AssignOp(_, lhs, rhs) => {
                self.expr(rhs);
                self.assign_target(lhs);
            }
            Stmt::Return(Some(e)) => self.expr(e),
            Stmt::Return(None) => {}
            Stmt::Defer(e) => self.expr(e),
            Stmt::If(i) => {
                self.expr(&i.cond);
                self.block(&i.then);
                if let Some(els) = &i.els {
                    self.block(els);
                }
            }
            Stmt::While(w) => {
                self.expr(&w.cond);
                self.block(&w.body);
            }
            Stmt::For(f) => {
                self.expr(&f.iter);
                self.push_scope();
                self.declare(f.var.clone(), false, false, f.iter.span);
                self.block(&f.body);
                self.pop_scope();
            }
            Stmt::Match(m) => self.match_(m),
            Stmt::Expr(e) => self.expr(e),
        }
    }

    fn assign_target(&mut self, lhs: &Expr) {
        let ExprKind::Ident(name) = &lhs.kind else {
            self.expr(lhs);
            return;
        };
        let mut found = None;
        for scope in self.scopes.iter().rev() {
            if let Some(var) = scope.get(name) {
                found = Some((var.mutable, var.depth));
                break;
            }
        }
        match found {
            Some((mutable, depth)) => {
                if depth < self.depth {
                    self.errors.push(Diagnostic::new(
                        format!("cannot mutate '{name}' from an inner scope"),
                        lhs.span,
                    ));
                } else if !mutable {
                    self.errors.push(Diagnostic::new(
                        format!("cannot assign to immutable '{name}'"),
                        lhs.span,
                    ));
                }
            }
            None => self.errors.push(Diagnostic::new(
                format!("cannot assign to undefined name '{name}'"),
                lhs.span,
            )),
        }
    }

    fn match_(&mut self, m: &Match) {
        self.expr(&m.scrut);
        for arm in &m.arms {
            self.push_scope();
            match &arm.pat {
                Pattern::Variant(_, binds) => {
                    for b in binds {
                        self.declare(b.clone(), false, false, Span::new(0, 0));
                    }
                }
                Pattern::Ident(name) if !self.globals.contains(name) => {
                    self.declare(name.clone(), false, false, Span::new(0, 0));
                }
                _ => {}
            }
            self.block(&arm.body);
            self.pop_scope();
        }
    }

    fn lambda(&mut self, l: &Lambda) {
        self.depth += 1;
        self.push_scope();
        for p in &l.params {
            self.declare(p.name.clone(), false, false, Span::new(0, 0));
        }
        self.block(&l.body);
        self.pop_scope();
        self.depth -= 1;
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Ident(name) => self.use_name(name, e.span),
            ExprKind::Int(..)
            | ExprKind::Float(..)
            | ExprKind::Str(_)
            | ExprKind::Char(_)
            | ExprKind::Rune(_)
            | ExprKind::Bool(_) => {}
            ExprKind::Unary(_, x) => self.expr(x),
            ExprKind::Binary(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Call(f, args) => {
                self.expr(f);
                let is_sizeof = matches!(&f.kind, ExprKind::Ident(n) if n == "sizeof");
                for a in args {
                    if is_sizeof {
                        if let ExprKind::Ident(n) = &a.kind {
                            // The reserved unsigned names are refused even as a
                            // sizeof argument, with the same diagnostic a type
                            // annotation earns, rather than falling through to an
                            // opaque undefined-name message.
                            if is_reserved_uint(n) {
                                self.errors.push(Diagnostic::new(
                                    "unsigned integers are reserved; use the signed widths",
                                    a.span,
                                ));
                                continue;
                            }
                            if is_type_name(n)
                                || self.globals.contains(n)
                                || self.cur_generics.contains(n)
                            {
                                continue;
                            }
                        }
                    }
                    self.expr(a);
                }
            }
            ExprKind::Field(x, _) => self.expr(x),
            ExprKind::Index(x, i) => {
                self.expr(x);
                self.expr(i);
            }
            ExprKind::Range(a, b, _) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
                for x in xs {
                    self.expr(x);
                }
            }
            ExprKind::StructLit(_, fields) => {
                for (_, v) in fields {
                    self.expr(v);
                }
            }
            ExprKind::Lambda(l) => self.lambda(l),
            ExprKind::Match(m) => self.match_(m),
            // The awaited operand is a use of its future binding, so an
            // unawaited future would still trip the unused variable rule.
            ExprKind::Await(op, _) => self.expr(op),
            // The minted value is a use of its subexpression; the element type is
            // validated by the type pass, so resolve only walks the value.
            ExprKind::Collect { arg, .. } => self.expr(arg),
            ExprKind::Do(_, binds) => {
                for b in binds {
                    self.expr(&b.expr);
                }
            }
            ExprKind::SizeofType(_) => {}
        }
    }
}

/// Whether a name is a builtin primitive type, accepted as a `sizeof` argument.
/// The unsigned widths are absent on purpose: they are reserved, caught by
/// `is_reserved_uint` before this runs.
fn is_type_name(n: &str) -> bool {
    matches!(
        n,
        "int8"
            | "int16"
            | "int32"
            | "int64"
            | "float32"
            | "float64"
            | "bool"
            | "char"
            | "rune"
            | "string"
            | "void"
            | "thread"
    )
}

/// The reserved unsigned integer type names, refused wherever a type name is
/// accepted until real unsigned support lands.
fn is_reserved_uint(n: &str) -> bool {
    matches!(n, "uint8" | "uint16" | "uint32" | "uint64")
}

fn item_name(item: &Item) -> Option<&str> {
    match item {
        Item::Func(f) => Some(&f.name),
        Item::Struct(s) => Some(&s.name),
        Item::Enum(e) => Some(&e.name),
        Item::Interface(i) => Some(&i.name),
        Item::Impl(_) => None,
        // A foreign block declares several names, so it has no single item name.
        // The names still register as globals above so calls resolve.
        Item::Foreign(_) => None,
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
    fn undefined_name() {
        assert_eq!(errs("func f() -> int64 { return x }").len(), 1);
    }

    #[test]
    fn unused_let() {
        let e = errs("func f() -> int64 {\n  y := 5\n  return 0\n}");
        assert!(e.iter().any(|d| d.msg.contains("unused")));
    }

    #[test]
    fn immutable_assign() {
        let e = errs("func f() -> int64 {\n  x := 5\n  x = 6\n  return x\n}");
        assert!(e.iter().any(|d| d.msg.contains("immutable")));
    }

    #[test]
    fn mut_ok() {
        let e = errs("func f() -> int64 {\n  mut x: int64 = 5\n  x = 6\n  return x\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn compound_assign_to_immutable_rejected() {
        let e = errs("func f() -> int64 {\n  x := 5\n  x += 1\n  return x\n}");
        assert!(e.iter().any(|d| d.msg.contains("immutable")), "{e:?}");
    }

    #[test]
    fn compound_assign_to_mut_ok() {
        let e = errs("func f() -> int64 {\n  mut x: int64 = 5\n  x += 1\n  return x\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn lambda_cannot_mutate_outer() {
        let e = errs(
            "@paradigm functional\n\
             func f() -> int64 {\n\
               mut x: int64 = 0\n\
               g := lambda (n: int64) -> int64 { x = n\n return x }\n\
               return g(1)\n\
             }",
        );
        assert!(e.iter().any(|d| d.msg.contains("inner scope")));
    }

    #[test]
    fn enum_variant_resolves() {
        let e = errs("enum Sh { Circle(r: float64), Empty }\nfunc f() -> Sh { return Circle(1.0) }");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn duplicate_def() {
        let e = errs("func f() -> int64 { return 0 }\nfunc f() -> int64 { return 1 }");
        assert!(e.iter().any(|d| d.msg.contains("duplicate")));
    }
}
