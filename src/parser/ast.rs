//! Abstract syntax tree. M2.

use crate::diag::Span;
use std::collections::HashSet;

/// A parsed source file: its directives and top level items.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub paradigms: Vec<String>,
    pub imports: Vec<String>,
    /// The `monad` blocks this file declared, each with the span of its keyword.
    /// The parser flattens a monad block into plain functions, so the paradigm
    /// gate reads this record instead of the vanished syntax.
    pub monads: Vec<(String, Span)>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Func(Func),
    Struct(Struct),
    Enum(Enum),
    Interface(Interface),
    Impl(Impl),
    Foreign(Foreign),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Func {
    pub exported: bool,
    /// True for an `async func`. The state machine transform lives in codegen,
    /// below sema, so every checker pass sees the intact body and only new node
    /// arms; this flag rides the declaration through loader, sema, and mono.
    pub is_async: bool,
    pub name: String,
    /// The span of the function's name, for diagnostics about the whole function.
    pub span: Span,
    pub generics: Vec<String>,
    pub params: Vec<Param>,
    pub ret: Type,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub using: bool,
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Struct {
    pub exported: bool,
    pub name: String,
    pub generics: Vec<String>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Enum {
    pub exported: bool,
    pub name: String,
    pub generics: Vec<String>,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Interface {
    pub exported: bool,
    pub name: String,
    pub generics: Vec<String>,
    pub methods: Vec<MethodSig>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodSig {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Impl {
    pub iface: Option<String>,
    pub ty: String,
    /// The span of the `impl` keyword, for diagnostics about the whole block.
    pub span: Span,
    pub methods: Vec<Func>,
}

/// A block of external function declarations bound to a calling convention, as in
/// `foreign "C" { func write(fd: int32, buf: *raw int8, count: int) -> int }`. The
/// functions have no body. They resolve to a C symbol of the same name at link,
/// and the boundary trades in raw pointers only, never a managed `*T`.
#[derive(Debug, Clone, PartialEq)]
pub struct Foreign {
    pub abi: String,
    /// The span of the `foreign` keyword, for diagnostics about the block.
    pub span: Span,
    pub funcs: Vec<ForeignFunc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForeignFunc {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Named(String, Vec<Type>),
    Ptr(Box<Type>),
    RawPtr(Box<Type>),
    Slice(Box<Type>),
    Array(Box<Type>, u64),
    Tuple(Vec<Type>),
    Func(Vec<Type>, Box<Type>),
    /// A collected wrapper `collector<T>`. Its runtime rep is a managed `*T`, a
    /// fat `{ptr, gen}` whose block lives on the collected heap the collector
    /// reclaims. Deref and field projection mirror `*T` with the same generation
    /// check; the value is escape neutral and never freed or moved.
    Collector(Box<Type>),
    Unit,
    /// A type hole produced only by desugar, for the open continuation lambdas of
    /// a `do` over a generic monad, and resolved only by mono's per-site
    /// inference. The parser never constructs it, and it is a hard error anywhere
    /// downstream of mono: codegen treats it as unreachable.
    Infer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let(Let),
    Assign(Expr, Expr),
    /// A compound assignment `place op= rhs`, as in `xs[i] += 1`. Kept as its own
    /// statement rather than a clone of `place = place op rhs` so the place is
    /// evaluated exactly once by codegen. The `++` and `--` statements desugar to
    /// this with an `Add`/`Sub` and a literal 1.
    AssignOp(BinOp, Expr, Expr),
    Return(Option<Expr>),
    Defer(Expr),
    If(If),
    While(While),
    For(For),
    Match(Match),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Let {
    pub mutable: bool,
    pub is_ref: bool,
    pub infer: bool,
    pub binds: Vec<Bind>,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bind {
    pub name: String,
    pub ty: Option<Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct If {
    pub cond: Expr,
    pub then: Block,
    pub els: Option<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct While {
    pub cond: Expr,
    pub body: Block,
    pub post_test: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct For {
    pub var: String,
    pub iter: Expr,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub scrut: Box<Expr>,
    pub arms: Vec<Arm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub pat: Pattern,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard,
    Ident(String),
    Variant(String, Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lambda {
    pub params: Vec<Param>,
    pub ret: Type,
    pub body: Block,
}

/// One element of a monadic `do` block: either `name <- expr` (a monadic bind)
/// or a bare expression. The last element is the result lifted with `unit`.
#[derive(Debug, Clone, PartialEq)]
pub struct DoBind {
    pub name: Option<String>,
    pub expr: Expr,
}

/// An expression node carrying its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64, Option<String>),
    Float(f64, Option<String>),
    Str(String),
    Char(char),
    Bool(bool),
    Ident(String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// A range `lo..hi` or, when the flag is set, an inclusive `lo..=hi`. Ranges
    /// exist only inside a slice index. An inclusive range is defined as
    /// `lo..hi+1`, converted before the bounds checks run.
    Range(Box<Expr>, Box<Expr>, bool),
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    StructLit(String, Vec<(String, Expr)>),
    Lambda(Lambda),
    Match(Box<Match>),
    /// A monadic `do` block, with an optional monad name as in `do Maybe { ... }`.
    /// The name selects which `bind` and `unit` the block desugars to, so several
    /// monads can coexist. A bare `do` uses the top level `bind` and `unit`.
    Do(Option<String>, Vec<DoBind>),
    /// `await e`, the statement level suspension inside an async func. The
    /// operand is the awaited future; the optional type is its element type,
    /// left None by the parser and filled by the monomorphizer. Constructed only
    /// as the whole value of a let, the whole operand of a return, or the whole
    /// expression of a bare statement, so the suspension always sits at a
    /// statement boundary the codegen state machine can resume at.
    Await(Box<Expr>, Option<Type>),
    /// `sizeof` of a resolved type. Produced only by the monomorphizer when a
    /// `sizeof(T)` over a type parameter is substituted to its concrete type, so
    /// composite types such as slices and tuples are sized correctly.
    SizeofType(Type),
    /// `collector<T>(value)`, the minting expression: it evaluates `value`,
    /// stores it in a freshly minted collected block, and yields a `collector<T>`
    /// fat value. `ty` is the element type `T` the value checks against and the
    /// block is sized for; `arg` is the value stored.
    Collect { ty: Type, arg: Box<Expr> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Deref,
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Pow,
}

// Free variable collection, shared by closure lowering and the escape check. The
// `used` vector accumulates every identifier read and `bound` the names a scope
// introduces, so a name in `used` that is not in `bound` is captured from an
// enclosing scope.
pub fn collect_block(b: &Block, used: &mut Vec<String>, bound: &mut HashSet<String>) {
    for s in &b.stmts {
        collect_stmt(s, used, bound);
    }
}

pub fn collect_stmt(s: &Stmt, used: &mut Vec<String>, bound: &mut HashSet<String>) {
    match s {
        Stmt::Let(l) => {
            collect_expr(&l.value, used, bound);
            for b in &l.binds {
                bound.insert(b.name.clone());
            }
        }
        Stmt::Assign(a, b) => {
            collect_expr(a, used, bound);
            collect_expr(b, used, bound);
        }
        Stmt::AssignOp(_, a, b) => {
            collect_expr(a, used, bound);
            collect_expr(b, used, bound);
        }
        Stmt::Return(Some(e)) | Stmt::Defer(e) | Stmt::Expr(e) => collect_expr(e, used, bound),
        Stmt::Return(None) => {}
        Stmt::If(i) => {
            collect_expr(&i.cond, used, bound);
            collect_block(&i.then, used, bound);
            if let Some(e) = &i.els {
                collect_block(e, used, bound);
            }
        }
        Stmt::While(w) => {
            collect_expr(&w.cond, used, bound);
            collect_block(&w.body, used, bound);
        }
        Stmt::For(f) => {
            collect_expr(&f.iter, used, bound);
            bound.insert(f.var.clone());
            collect_block(&f.body, used, bound);
        }
        Stmt::Match(m) => collect_match(m, used, bound),
    }
}

pub fn collect_match(m: &Match, used: &mut Vec<String>, bound: &mut HashSet<String>) {
    collect_expr(&m.scrut, used, bound);
    for arm in &m.arms {
        match &arm.pat {
            Pattern::Variant(_, binds) => {
                for b in binds {
                    bound.insert(b.clone());
                }
            }
            Pattern::Ident(n) => {
                bound.insert(n.clone());
            }
            Pattern::Wildcard => {}
        }
        collect_block(&arm.body, used, bound);
    }
}

pub fn collect_expr(e: &Expr, used: &mut Vec<String>, bound: &mut HashSet<String>) {
    match &e.kind {
        ExprKind::Ident(n) => used.push(n.clone()),
        ExprKind::Unary(_, x) => collect_expr(x, used, bound),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b, _) => {
            collect_expr(a, used, bound);
            collect_expr(b, used, bound);
        }
        ExprKind::Call(f, args) => {
            collect_expr(f, used, bound);
            for a in args {
                collect_expr(a, used, bound);
            }
        }
        ExprKind::Field(b, _) => collect_expr(b, used, bound),
        ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
            for x in xs {
                collect_expr(x, used, bound);
            }
        }
        ExprKind::StructLit(_, fs) => {
            for (_, v) in fs {
                collect_expr(v, used, bound);
            }
        }
        ExprKind::Lambda(l) => {
            for p in &l.params {
                bound.insert(p.name.clone());
            }
            collect_block(&l.body, used, bound);
        }
        ExprKind::Match(m) => collect_match(m, used, bound),
        ExprKind::Await(op, _) => collect_expr(op, used, bound),
        ExprKind::Collect { arg, .. } => collect_expr(arg, used, bound),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(name: &str) -> Expr {
        Expr {
            kind: ExprKind::Ident(name.to_string()),
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn compound_assign_captures_both_sides() {
        // A lambda body of only `xs[i] += 1` must capture xs and i, or closure
        // lowering and the escape check miss them, a silent miscompile.
        let lhs = Expr {
            kind: ExprKind::Index(Box::new(ident("xs")), Box::new(ident("i"))),
            span: Span::new(0, 0),
        };
        let stmt = Stmt::AssignOp(BinOp::Add, lhs, ident("one"));
        let mut used = Vec::new();
        let mut bound = HashSet::new();
        collect_stmt(&stmt, &mut used, &mut bound);
        assert!(used.contains(&"xs".to_string()), "xs must be captured");
        assert!(used.contains(&"i".to_string()), "i must be captured");
        assert!(used.contains(&"one".to_string()), "the rhs must be captured");
    }
}
