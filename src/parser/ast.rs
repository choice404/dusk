//! Abstract syntax tree. M2.

use crate::diag::Span;

/// A parsed source file: its directives and top level items.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub paradigms: Vec<String>,
    pub imports: Vec<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Func(Func),
    Struct(Struct),
    Enum(Enum),
    Interface(Interface),
    Impl(Impl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Func {
    pub exported: bool,
    pub name: String,
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
    pub methods: Vec<Func>,
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
    Unit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let(Let),
    Assign(Expr, Expr),
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
    Range(Box<Expr>, Box<Expr>),
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    StructLit(String, Vec<(String, Expr)>),
    Lambda(Lambda),
    Match(Box<Match>),
    /// A monadic `do` block, with an optional monad name as in `do Maybe { ... }`.
    /// The name selects which `bind` and `unit` the block desugars to, so several
    /// monads can coexist. A bare `do` uses the top level `bind` and `unit`.
    Do(Option<String>, Vec<DoBind>),
    /// `sizeof` of a resolved type. Produced only by the monomorphizer when a
    /// `sizeof(T)` over a type parameter is substituted to its concrete type, so
    /// composite types such as slices and tuples are sized correctly.
    SizeofType(Type),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Deref,
    Neg,
    Not,
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
}
