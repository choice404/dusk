//! Paradigm gating. M9.
//!
//! User defined names stay paradigm agnostic. Builtins and paradigm specific
//! syntax are gated against the file's `@paradigm` directives. The functional
//! builtins (map, filter, reduce, fold, foreach) and do notation require
//! `functional`. The procedural keywords (while, for, mut) require `procedural`.
//! An `interface` declaration requires `oop`. A file with no directive defaults
//! to procedural, so a gated construct in a file that never declares its
//! paradigm is rejected with a clear error.

use std::collections::HashSet;

use crate::diag::{Diagnostic, Span};
use crate::parser::ast::*;

const FUNCTIONAL_BUILTINS: &[&str] = &["map", "filter", "reduce", "fold", "foreach"];

/// Checks builtin usage against the file's declared paradigms.
pub fn check(module: &Module) -> Vec<Diagnostic> {
    let declares = |name: &str| module.paradigms.iter().any(|p| p == name);
    let functional = declares("functional");
    let oop = declares("oop");
    // A file with no paradigm directive defaults to procedural.
    let procedural = declares("procedural") || module.paradigms.is_empty();
    let user_fns: HashSet<String> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Func(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect();
    let mut g = Gate {
        functional,
        procedural,
        oop,
        user_fns,
        errors: Vec::new(),
    };
    // The parser flattens a `monad` block into plain functions, so the gate reads
    // the module's record of them rather than the vanished syntax.
    for (name, span) in &module.monads {
        g.need_functional(&format!("the '{name}' monad block"), *span);
    }
    for item in &module.items {
        match item {
            Item::Func(f) => g.block(&f.body),
            Item::Impl(im) => {
                for m in &im.methods {
                    g.block(&m.body);
                }
            }
            Item::Interface(i) => {
                g.need_oop(&format!("the '{}' interface", i.name), Span::new(0, 0))
            }
            _ => {}
        }
    }
    g.errors
}

struct Gate {
    functional: bool,
    procedural: bool,
    oop: bool,
    user_fns: HashSet<String>,
    errors: Vec<Diagnostic>,
}

impl Gate {
    fn need_functional(&mut self, what: &str, span: Span) {
        if !self.functional {
            self.errors.push(Diagnostic::new(
                format!("{what} requires the functional paradigm; add '@paradigm functional'"),
                span,
            ));
        }
    }

    fn need_procedural(&mut self, what: &str, span: Span) {
        if !self.procedural {
            self.errors.push(Diagnostic::new(
                format!("{what} requires the procedural paradigm; add '@paradigm procedural'"),
                span,
            ));
        }
    }

    fn need_oop(&mut self, what: &str, span: Span) {
        if !self.oop {
            self.errors.push(Diagnostic::new(
                format!("{what} requires the oop paradigm; add '@paradigm oop'"),
                span,
            ));
        }
    }

    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(l) => {
                if l.mutable {
                    self.need_procedural("the 'mut' keyword", l.value.span);
                }
                self.expr(&l.value);
            }
            Stmt::Assign(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Stmt::AssignOp(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Stmt::Return(Some(e)) | Stmt::Defer(e) | Stmt::Expr(e) => self.expr(e),
            Stmt::Return(None) => {}
            Stmt::If(i) => {
                self.expr(&i.cond);
                self.block(&i.then);
                if let Some(e) = &i.els {
                    self.block(e);
                }
            }
            Stmt::While(w) => {
                let what = if w.post_test {
                    "the 'do while' loop"
                } else {
                    "the 'while' loop"
                };
                self.need_procedural(what, w.cond.span);
                self.expr(&w.cond);
                self.block(&w.body);
            }
            Stmt::For(f) => {
                self.need_procedural("the 'for' loop", f.iter.span);
                self.expr(&f.iter);
                self.block(&f.body);
            }
            Stmt::Match(m) => self.match_(m),
        }
    }

    fn match_(&mut self, m: &Match) {
        self.expr(&m.scrut);
        for arm in &m.arms {
            self.block(&arm.body);
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Call(f, args) => {
                if let ExprKind::Ident(name) = &f.kind {
                    if FUNCTIONAL_BUILTINS.contains(&name.as_str())
                        && !self.user_fns.contains(name)
                    {
                        self.need_functional(&format!("the '{name}' builtin"), e.span);
                    }
                }
                self.expr(f);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Do(_, binds) => {
                self.need_functional("do notation", e.span);
                for b in binds {
                    self.expr(&b.expr);
                }
            }
            ExprKind::Unary(_, x) => self.expr(x),
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b, _) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Field(b, _) => self.expr(b),
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
                for x in xs {
                    self.expr(x);
                }
            }
            ExprKind::StructLit(_, fs) => {
                for (_, v) in fs {
                    self.expr(v);
                }
            }
            ExprKind::Lambda(l) => self.block(&l.body),
            ExprKind::Match(m) => self.match_(m),
            // async is paradigm agnostic; the gate only walks into the operand.
            ExprKind::Await(op, _) => self.expr(op),
            // The collector mint is paradigm agnostic; only the value is walked.
            ExprKind::Collect { arg, .. } => self.expr(arg),
            ExprKind::Int(..)
            | ExprKind::Float(..)
            | ExprKind::Str(_)
            | ExprKind::Char(_)
            | ExprKind::Rune(_)
            | ExprKind::Bool(_)
            | ExprKind::Ident(_)
            | ExprKind::SizeofType(_) => {}
        }
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
    fn procedural_map_rejected() {
        let e = errs(
            "@paradigm procedural\n\
             func main() -> int32 {\n  xs := map(ys, f)\n  return 0\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("functional paradigm")), "{e:?}");
    }

    #[test]
    fn default_paradigm_map_rejected() {
        let e = errs("func main() -> int32 {\n  xs := map(ys, f)\n  return 0\n}");
        assert!(!e.is_empty());
    }

    #[test]
    fn functional_map_ok() {
        let e = errs(
            "@paradigm functional\n\
             func main() -> int32 {\n  xs := map(ys, f)\n  return 0\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn user_defined_map_not_gated() {
        let e = errs(
            "@paradigm procedural\n\
             func map(x: int64) -> int64 { return x }\n\
             func main() -> int32 {\n  z := map(5)\n  return 0\n}",
        );
        assert!(e.is_empty(), "user function named map must stay agnostic: {e:?}");
    }

    #[test]
    fn procedural_do_rejected() {
        let e = errs(
            "func main() -> int32 {\n  r := do {\n    a <- 1\n    a\n  }\n  return 0\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("do notation")), "{e:?}");
    }

    #[test]
    fn functional_while_rejected() {
        let e = errs(
            "@paradigm functional\n\
             func main() -> int32 {\n  mut i: int64 = 0\n  while i < 3 { i = i + 1 }\n  return 0\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("procedural paradigm")), "{e:?}");
    }

    #[test]
    fn functional_mut_rejected() {
        let e = errs(
            "@paradigm functional\nfunc main() -> int32 {\n  mut i: int64 = 0\n  return 0\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("procedural paradigm")), "{e:?}");
    }

    #[test]
    fn procedural_while_ok() {
        let e = errs(
            "@paradigm procedural\n\
             func main() -> int32 {\n  mut i: int64 = 0\n  while i < 3 { i = i + 1 }\n  return 0\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn default_paradigm_while_ok() {
        // No directive defaults to procedural, so loops and mut are allowed.
        let e = errs(
            "func main() -> int32 {\n  mut i: int64 = 0\n  while i < 3 { i = i + 1 }\n  return 0\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn stacked_functional_procedural_while_ok() {
        let e = errs(
            "@paradigm functional\n@paradigm procedural\n\
             func main() -> int32 {\n  mut i: int64 = 0\n  while i < 3 { i = i + 1 }\n  return 0\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn monad_block_requires_functional() {
        let e = errs(
            "monad M {\n  func bind(x: int64, f: (int64) -> int64) -> int64 { return f(x) }\n  func unit(x: int64) -> int64 { return x }\n}\nfunc main() -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg.contains("monad block requires the functional paradigm")), "{e:?}");
    }

    #[test]
    fn monad_block_under_functional_is_ok() {
        let e = errs(
            "@paradigm functional\nmonad M {\n  func bind(x: int64, f: (int64) -> int64) -> int64 { return f(x) }\n  func unit(x: int64) -> int64 { return x }\n}\nfunc main() -> int32 { return 0 }",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn non_oop_interface_rejected() {
        let e = errs(
            "@paradigm procedural\ninterface Foo {\n  bar() -> void\n}\nfunc main() -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg.contains("oop paradigm")), "{e:?}");
    }

    #[test]
    fn oop_interface_ok() {
        let e = errs(
            "@paradigm oop\ninterface Foo {\n  bar() -> void\n}\nfunc main() -> int32 { return 0 }",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn default_paradigm_interface_rejected() {
        // Default is procedural, which does not unlock interfaces.
        let e = errs(
            "interface Foo {\n  bar() -> void\n}\nfunc main() -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg.contains("oop paradigm")), "{e:?}");
    }
}
