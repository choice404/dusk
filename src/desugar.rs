//! Desugaring pass. M9.
//!
//! Rewrites monadic `do` notation into nested `bind` and `unit` calls before name
//! resolution and type checking. A do block
//!
//! ```text
//! do { x <- m; y <- n; x + y }
//! ```
//!
//! becomes `bind(m, lambda (x) -> { return bind(n, lambda (y) -> { return unit(x + y) }) })`.
//! The continuation lambda parameter and return types come from the chosen
//! `bind` function's second parameter, a function type `(A) -> B`. A `do Name { }`
//! block desugars to `Name.bind` and `Name.unit`, so several monads coexist; a
//! bare `do` uses the top level `bind` and `unit`.

use crate::diag::Span;
use crate::parser::ast::*;

/// Rewrites a module, expanding all `do` blocks. Returns a new module.
pub fn run(module: &Module) -> Module {
    let d = Desugar { module };
    Module {
        paradigms: module.paradigms.clone(),
        imports: module.imports.clone(),
        monads: module.monads.clone(),
        items: module.items.iter().map(|it| d.item(it)).collect(),
    }
}

/// The parameter and return types of the continuation lambda passed to `bind`,
/// read from the named `bind` function's second parameter `(A) -> B`. Defaults to
/// int64 when the function or its signature is absent.
fn cont_type(module: &Module, bind_name: &str) -> (Type, Type) {
    for item in &module.items {
        if let Item::Func(f) = item {
            if f.name == bind_name {
                if let Some(p) = f.params.get(1) {
                    if let Type::Func(ps, r) = &p.ty {
                        let a = ps.first().cloned().unwrap_or_else(int_ty);
                        return (a, (**r).clone());
                    }
                }
            }
        }
    }
    (int_ty(), int_ty())
}

fn int_ty() -> Type {
    Type::Named("int64".to_string(), Vec::new())
}

struct Desugar<'a> {
    module: &'a Module,
}

impl Desugar<'_> {
    fn item(&self, item: &Item) -> Item {
        match item {
            Item::Func(f) => Item::Func(self.func(f)),
            Item::Impl(im) => Item::Impl(Impl {
                iface: im.iface.clone(),
                ty: im.ty.clone(),
                span: im.span,
                methods: im.methods.iter().map(|m| self.func(m)).collect(),
            }),
            other => other.clone(),
        }
    }

    fn func(&self, f: &Func) -> Func {
        Func {
            exported: f.exported,
            name: f.name.clone(),
            span: f.span,
            generics: f.generics.clone(),
            params: f.params.clone(),
            ret: f.ret.clone(),
            body: self.block(&f.body),
        }
    }

    fn block(&self, b: &Block) -> Block {
        Block {
            stmts: b.stmts.iter().map(|s| self.stmt(s)).collect(),
        }
    }

    fn stmt(&self, s: &Stmt) -> Stmt {
        match s {
            Stmt::Let(l) => Stmt::Let(Let {
                mutable: l.mutable,
                is_ref: l.is_ref,
                infer: l.infer,
                binds: l.binds.clone(),
                value: self.expr(&l.value),
            }),
            Stmt::Assign(a, b) => Stmt::Assign(self.expr(a), self.expr(b)),
            Stmt::Return(Some(e)) => Stmt::Return(Some(self.expr(e))),
            Stmt::Return(None) => Stmt::Return(None),
            Stmt::Defer(e) => Stmt::Defer(self.expr(e)),
            Stmt::If(i) => Stmt::If(If {
                cond: self.expr(&i.cond),
                then: self.block(&i.then),
                els: i.els.as_ref().map(|e| self.block(e)),
            }),
            Stmt::While(w) => Stmt::While(While {
                cond: self.expr(&w.cond),
                body: self.block(&w.body),
                post_test: w.post_test,
            }),
            Stmt::For(f) => Stmt::For(For {
                var: f.var.clone(),
                iter: self.expr(&f.iter),
                body: self.block(&f.body),
            }),
            Stmt::Match(m) => Stmt::Match(self.match_(m)),
            Stmt::Expr(e) => Stmt::Expr(self.expr(e)),
        }
    }

    fn match_(&self, m: &Match) -> Match {
        Match {
            scrut: Box::new(self.expr(&m.scrut)),
            arms: m
                .arms
                .iter()
                .map(|a| Arm {
                    pat: a.pat.clone(),
                    body: self.block(&a.body),
                })
                .collect(),
        }
    }

    fn expr(&self, e: &Expr) -> Expr {
        let kind = match &e.kind {
            ExprKind::Unary(op, x) => ExprKind::Unary(*op, Box::new(self.expr(x))),
            ExprKind::Binary(op, a, b) => {
                ExprKind::Binary(*op, Box::new(self.expr(a)), Box::new(self.expr(b)))
            }
            ExprKind::Call(f, args) => ExprKind::Call(
                Box::new(self.expr(f)),
                args.iter().map(|a| self.expr(a)).collect(),
            ),
            ExprKind::Field(b, n) => ExprKind::Field(Box::new(self.expr(b)), n.clone()),
            ExprKind::Index(a, b) => {
                ExprKind::Index(Box::new(self.expr(a)), Box::new(self.expr(b)))
            }
            ExprKind::Range(a, b) => {
                ExprKind::Range(Box::new(self.expr(a)), Box::new(self.expr(b)))
            }
            ExprKind::Tuple(xs) => ExprKind::Tuple(xs.iter().map(|x| self.expr(x)).collect()),
            ExprKind::Array(xs) => ExprKind::Array(xs.iter().map(|x| self.expr(x)).collect()),
            ExprKind::StructLit(n, fs) => ExprKind::StructLit(
                n.clone(),
                fs.iter().map(|(k, v)| (k.clone(), self.expr(v))).collect(),
            ),
            ExprKind::Lambda(l) => ExprKind::Lambda(Lambda {
                params: l.params.clone(),
                ret: l.ret.clone(),
                body: self.block(&l.body),
            }),
            ExprKind::Match(m) => ExprKind::Match(Box::new(self.match_(m))),
            ExprKind::Do(monad, binds) => {
                return self.do_to_calls(monad.as_deref(), binds, e.span)
            }
            other => other.clone(),
        };
        Expr { kind, span: e.span }
    }

    /// Folds do binds into nested `bind` calls, lifting the final expression with
    /// `unit`. Earlier binds wrap later ones, so evaluation runs top to bottom.
    fn do_to_calls(&self, monad: Option<&str>, binds: &[DoBind], span: Span) -> Expr {
        // A named monad uses its namespaced pair, `Name.bind` and `Name.unit`, so
        // several monads coexist. A bare do uses the top level `bind` and `unit`.
        let (bind_name, unit_name) = match monad {
            Some(m) => (format!("{m}.bind"), format!("{m}.unit")),
            None => ("bind".to_string(), "unit".to_string()),
        };
        let cont = cont_type(self.module, &bind_name);
        if binds.is_empty() {
            return call(&unit_name, vec![unit_lit(span)], span);
        }
        let last = self.expr(&binds[binds.len() - 1].expr);
        let mut acc = call(&unit_name, vec![last], span);
        for (i, b) in binds[..binds.len() - 1].iter().enumerate().rev() {
            let arg = self.expr(&b.expr);
            // The discard name carries a '$', which the lexer cannot produce, so
            // it can never collide with a user written identifier.
            let pname = b.name.clone().unwrap_or_else(|| format!("$do{i}"));
            let lam = Lambda {
                params: vec![Param {
                    using: false,
                    name: pname,
                    ty: cont.0.clone(),
                }],
                ret: cont.1.clone(),
                body: Block {
                    stmts: vec![Stmt::Return(Some(acc))],
                },
            };
            let lam_expr = Expr {
                kind: ExprKind::Lambda(lam),
                span,
            };
            acc = call(&bind_name, vec![arg, lam_expr], span);
        }
        acc
    }
}

fn call(name: &str, args: Vec<Expr>, span: Span) -> Expr {
    let callee = Expr {
        kind: ExprKind::Ident(name.to_string()),
        span,
    };
    Expr {
        kind: ExprKind::Call(Box::new(callee), args),
        span,
    }
}

fn unit_lit(span: Span) -> Expr {
    Expr {
        kind: ExprKind::Int(0, None),
        span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn desugared(src: &str) -> Module {
        let (t, le) = lex(src);
        assert!(le.is_empty(), "lex errors: {le:?}");
        let (m, pe) = parse(t);
        assert!(pe.is_empty(), "parse errors: {pe:?}");
        run(&m)
    }

    fn main_first_stmt(m: &Module) -> Stmt {
        let Item::Func(f) = m.items.iter().find(|it| matches!(it, Item::Func(f) if f.name == "main")).unwrap() else {
            panic!()
        };
        f.body.stmts[0].clone()
    }

    #[test]
    fn do_block_lowers_to_bind_and_unit() {
        let m = desugared(
            "func bind(x: int64, f: (int64) -> int64) -> int64 { return f(x) }\n\
             func unit(x: int64) -> int64 { return x }\n\
             func main() -> int32 {\n  r := do {\n    a <- 10\n    b <- 20\n    a + b\n  }\n  return 0\n}",
        );
        let Stmt::Let(l) = main_first_stmt(&m) else {
            panic!("expected let")
        };
        // Outer call is bind(10, lambda ...)
        let ExprKind::Call(f, args) = &l.value.kind else {
            panic!("expected call, got {:?}", l.value.kind)
        };
        assert!(matches!(&f.kind, ExprKind::Ident(n) if n == "bind"));
        assert_eq!(args.len(), 2);
        assert!(matches!(args[1].kind, ExprKind::Lambda(_)));
    }

    #[test]
    fn named_monad_lowers_to_namespaced_pair() {
        // `do Name { ... }` desugars to that monad's `Name.bind`, so several
        // monads can coexist without their bind and unit colliding.
        let m = desugared(
            "monad Identity {\n  func bind(x: int64, f: (int64) -> int64) -> int64 { return f(x) }\n  func unit(x: int64) -> int64 { return x }\n}\n\
             func main() -> int32 {\n  r := do Identity {\n    a <- 10\n    b <- 20\n    a + b\n  }\n  return 0\n}",
        );
        let Stmt::Let(l) = main_first_stmt(&m) else {
            panic!("expected let")
        };
        let ExprKind::Call(f, _) = &l.value.kind else {
            panic!("expected call, got {:?}", l.value.kind)
        };
        assert!(
            matches!(&f.kind, ExprKind::Ident(n) if n == "Identity.bind"),
            "outer call should be Identity.bind, got {:?}",
            f.kind
        );
    }

    #[test]
    fn no_do_blocks_remain() {
        let m = desugared(
            "func bind(x: int64, f: (int64) -> int64) -> int64 { return f(x) }\n\
             func unit(x: int64) -> int64 { return x }\n\
             func main() -> int32 {\n  r := do {\n    a <- 1\n    a\n  }\n  return 0\n}",
        );
        // Walk the whole tree; assert no Do node survives.
        fn check(e: &Expr) {
            assert!(!matches!(e.kind, ExprKind::Do(..)), "stray do node");
            match &e.kind {
                ExprKind::Call(f, args) => {
                    check(f);
                    args.iter().for_each(check);
                }
                ExprKind::Lambda(l) => l.body.stmts.iter().for_each(check_stmt),
                ExprKind::Binary(_, a, b) => {
                    check(a);
                    check(b);
                }
                _ => {}
            }
        }
        fn check_stmt(s: &Stmt) {
            match s {
                Stmt::Let(l) => check(&l.value),
                Stmt::Return(Some(e)) | Stmt::Expr(e) => check(e),
                _ => {}
            }
        }
        for it in &m.items {
            if let Item::Func(f) = it {
                f.body.stmts.iter().for_each(check_stmt);
            }
        }
    }
}
