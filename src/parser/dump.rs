//! Canonical AST dump renderer.
//!
//! The parser dump is a byte-for-byte interchange contract. This module mirrors
//! the shape produced by derived pretty `Debug` for the AST while replacing the
//! scalar leaves that are awkward for a non-Rust compiler to reproduce exactly.

use std::fmt::{self, Debug, Formatter};

use crate::diag::Span;

use super::ast::*;

/// Renders a parsed module in the canonical parser dump format.
pub fn render_module(m: &Module) -> String {
    format!("{:#?}", Dump(m))
}

/// Escapes a scalar sequence for a dump literal: printable ASCII passes through,
/// and everything else, controls and every non-ASCII scalar alike, becomes a
/// `\u{hex}` escape with the lowercase, minimal-width code point.
pub fn escape_canonical(chars: impl Iterator<Item = char>) -> String {
    let mut out = String::new();
    for c in chars {
        let cp = c as u32;
        if (0x20..=0x7e).contains(&cp) {
            out.push(c);
        } else {
            out.push_str(&format!("\\u{{{cp:x}}}"));
        }
    }
    out
}

struct Dump<'a, T: ?Sized>(&'a T);

struct List<'a, T>(&'a [T]);

fn list<T>(items: &[T]) -> List<'_, T> {
    List(items)
}

impl<'a, T> Debug for List<'a, T>
where
    Dump<'a, T>: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.iter().map(Dump)).finish()
    }
}

struct Opt<'a, T>(&'a Option<T>);

fn opt<T>(value: &Option<T>) -> Opt<'_, T> {
    Opt(value)
}

impl<'a, T> Debug for Opt<'a, T>
where
    Dump<'a, T>: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => f.debug_tuple("Some").field(&Dump(value)).finish(),
            None => f.write_str("None"),
        }
    }
}

struct HexFloat(f64);

impl Debug for HexFloat {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016X}", self.0.to_bits())
    }
}

struct CanonChar(char);

impl Debug for CanonChar {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "'{}'", escape_canonical(std::iter::once(self.0)))
    }
}

impl Debug for Dump<'_, String> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", escape_canonical(self.0.chars()))
    }
}

impl<'a, A, B> Debug for Dump<'a, (A, B)>
where
    Dump<'a, A>: Debug,
    Dump<'a, B>: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("")
            .field(&Dump(&self.0 .0))
            .field(&Dump(&self.0 .1))
            .finish()
    }
}

impl Debug for Dump<'_, Span> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Span")
            .field("lo", &self.0.lo)
            .field("hi", &self.0.hi)
            .finish()
    }
}

impl Debug for Dump<'_, Module> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Module")
            .field("paradigms", &list(&self.0.paradigms))
            .field("imports", &list(&self.0.imports))
            .field("monads", &list(&self.0.monads))
            .field("items", &list(&self.0.items))
            .finish()
    }
}

impl Debug for Dump<'_, Item> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Item::Func(func) => f.debug_tuple("Func").field(&Dump(func)).finish(),
            Item::Struct(strukt) => f.debug_tuple("Struct").field(&Dump(strukt)).finish(),
            Item::Enum(enm) => f.debug_tuple("Enum").field(&Dump(enm)).finish(),
            Item::Interface(iface) => f.debug_tuple("Interface").field(&Dump(iface)).finish(),
            Item::Impl(imp) => f.debug_tuple("Impl").field(&Dump(imp)).finish(),
            Item::Foreign(foreign) => f.debug_tuple("Foreign").field(&Dump(foreign)).finish(),
        }
    }
}

impl Debug for Dump<'_, Func> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Func")
            .field("exported", &self.0.exported)
            .field("is_async", &self.0.is_async)
            .field("name", &Dump(&self.0.name))
            .field("span", &Dump(&self.0.span))
            .field("generics", &list(&self.0.generics))
            .field("params", &list(&self.0.params))
            .field("ret", &Dump(&self.0.ret))
            .field("body", &Dump(&self.0.body))
            .finish()
    }
}

impl Debug for Dump<'_, Param> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Param")
            .field("using", &self.0.using)
            .field("name", &Dump(&self.0.name))
            .field("ty", &Dump(&self.0.ty))
            .finish()
    }
}

impl Debug for Dump<'_, Struct> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Struct")
            .field("exported", &self.0.exported)
            .field("name", &Dump(&self.0.name))
            .field("generics", &list(&self.0.generics))
            .field("fields", &list(&self.0.fields))
            .finish()
    }
}

impl Debug for Dump<'_, Field> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Field")
            .field("name", &Dump(&self.0.name))
            .field("ty", &Dump(&self.0.ty))
            .finish()
    }
}

impl Debug for Dump<'_, Enum> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Enum")
            .field("exported", &self.0.exported)
            .field("name", &Dump(&self.0.name))
            .field("generics", &list(&self.0.generics))
            .field("variants", &list(&self.0.variants))
            .finish()
    }
}

impl Debug for Dump<'_, Variant> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Variant")
            .field("name", &Dump(&self.0.name))
            .field("fields", &list(&self.0.fields))
            .finish()
    }
}

impl Debug for Dump<'_, Interface> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Interface")
            .field("exported", &self.0.exported)
            .field("name", &Dump(&self.0.name))
            .field("generics", &list(&self.0.generics))
            .field("methods", &list(&self.0.methods))
            .finish()
    }
}

impl Debug for Dump<'_, MethodSig> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MethodSig")
            .field("name", &Dump(&self.0.name))
            .field("params", &list(&self.0.params))
            .field("ret", &Dump(&self.0.ret))
            .finish()
    }
}

impl Debug for Dump<'_, Impl> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Impl")
            .field("iface", &opt(&self.0.iface))
            .field("ty", &Dump(&self.0.ty))
            .field("span", &Dump(&self.0.span))
            .field("methods", &list(&self.0.methods))
            .finish()
    }
}

impl Debug for Dump<'_, Foreign> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Foreign")
            .field("abi", &Dump(&self.0.abi))
            .field("span", &Dump(&self.0.span))
            .field("funcs", &list(&self.0.funcs))
            .finish()
    }
}

impl Debug for Dump<'_, ForeignFunc> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ForeignFunc")
            .field("name", &Dump(&self.0.name))
            .field("params", &list(&self.0.params))
            .field("ret", &Dump(&self.0.ret))
            .finish()
    }
}

impl Debug for Dump<'_, Type> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Type::Named(name, args) => f
                .debug_tuple("Named")
                .field(&Dump(name))
                .field(&list(args))
                .finish(),
            Type::Ptr(ty) => f.debug_tuple("Ptr").field(&Dump(&**ty)).finish(),
            Type::RawPtr(ty) => f.debug_tuple("RawPtr").field(&Dump(&**ty)).finish(),
            Type::Slice(ty) => f.debug_tuple("Slice").field(&Dump(&**ty)).finish(),
            Type::Array(ty, len) => f
                .debug_tuple("Array")
                .field(&Dump(&**ty))
                .field(len)
                .finish(),
            Type::Tuple(items) => f.debug_tuple("Tuple").field(&list(items)).finish(),
            Type::Func(params, ret) => f
                .debug_tuple("Func")
                .field(&list(params))
                .field(&Dump(&**ret))
                .finish(),
            Type::Collector(ty) => f.debug_tuple("Collector").field(&Dump(&**ty)).finish(),
            Type::Unit => f.write_str("Unit"),
            Type::Infer => f.write_str("Infer"),
        }
    }
}

impl Debug for Dump<'_, Block> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Block")
            .field("stmts", &list(&self.0.stmts))
            .finish()
    }
}

impl Debug for Dump<'_, Stmt> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Stmt::Let(let_) => f.debug_tuple("Let").field(&Dump(let_)).finish(),
            Stmt::Assign(lhs, rhs) => f
                .debug_tuple("Assign")
                .field(&Dump(lhs))
                .field(&Dump(rhs))
                .finish(),
            Stmt::AssignOp(op, lhs, rhs) => f
                .debug_tuple("AssignOp")
                .field(&Dump(op))
                .field(&Dump(lhs))
                .field(&Dump(rhs))
                .finish(),
            Stmt::Return(value) => f.debug_tuple("Return").field(&opt(value)).finish(),
            Stmt::Defer(expr) => f.debug_tuple("Defer").field(&Dump(expr)).finish(),
            Stmt::If(if_) => f.debug_tuple("If").field(&Dump(if_)).finish(),
            Stmt::While(while_) => f.debug_tuple("While").field(&Dump(while_)).finish(),
            Stmt::For(for_) => f.debug_tuple("For").field(&Dump(for_)).finish(),
            Stmt::Match(match_) => f.debug_tuple("Match").field(&Dump(match_)).finish(),
            Stmt::Break(_) => f.write_str("Break"),
            Stmt::Continue(_) => f.write_str("Continue"),
            Stmt::Expr(expr) => f.debug_tuple("Expr").field(&Dump(expr)).finish(),
        }
    }
}

impl Debug for Dump<'_, Let> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Let")
            .field("mutable", &self.0.mutable)
            .field("is_ref", &self.0.is_ref)
            .field("infer", &self.0.infer)
            .field("binds", &list(&self.0.binds))
            .field("value", &Dump(&self.0.value))
            .finish()
    }
}

impl Debug for Dump<'_, Bind> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bind")
            .field("name", &Dump(&self.0.name))
            .field("ty", &opt(&self.0.ty))
            .finish()
    }
}

impl Debug for Dump<'_, If> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("If")
            .field("cond", &Dump(&self.0.cond))
            .field("then", &Dump(&self.0.then))
            .field("els", &opt(&self.0.els))
            .finish()
    }
}

impl Debug for Dump<'_, While> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("While")
            .field("cond", &Dump(&self.0.cond))
            .field("body", &Dump(&self.0.body))
            .field("post_test", &self.0.post_test)
            .finish()
    }
}

impl Debug for Dump<'_, For> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("For")
            .field("var", &Dump(&self.0.var))
            .field("iter", &Dump(&self.0.iter))
            .field("body", &Dump(&self.0.body))
            .finish()
    }
}

impl Debug for Dump<'_, Match> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Match")
            .field("scrut", &Dump(&*self.0.scrut))
            .field("arms", &list(&self.0.arms))
            .finish()
    }
}

impl Debug for Dump<'_, Arm> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Arm")
            .field("pat", &Dump(&self.0.pat))
            .field("body", &Dump(&self.0.body))
            .finish()
    }
}

impl Debug for Dump<'_, Pattern> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Pattern::Wildcard => f.write_str("Wildcard"),
            Pattern::Ident(name) => f.debug_tuple("Ident").field(&Dump(name)).finish(),
            Pattern::Variant(name, binds) => f
                .debug_tuple("Variant")
                .field(&Dump(name))
                .field(&list(binds))
                .finish(),
        }
    }
}

impl Debug for Dump<'_, Lambda> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lambda")
            .field("params", &list(&self.0.params))
            .field("ret", &Dump(&self.0.ret))
            .field("body", &Dump(&self.0.body))
            .finish()
    }
}

impl Debug for Dump<'_, DoBind> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("DoBind")
            .field("name", &opt(&self.0.name))
            .field("expr", &Dump(&self.0.expr))
            .finish()
    }
}

impl Debug for Dump<'_, Expr> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Expr")
            .field("kind", &Dump(&self.0.kind))
            .field("span", &Dump(&self.0.span))
            .finish()
    }
}

impl Debug for Dump<'_, ExprKind> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            ExprKind::Int(value, suffix) => f
                .debug_tuple("Int")
                .field(value)
                .field(&opt(suffix))
                .finish(),
            ExprKind::Float(value, suffix) => f
                .debug_tuple("Float")
                .field(&HexFloat(*value))
                .field(&opt(suffix))
                .finish(),
            ExprKind::Str(value) => f.debug_tuple("Str").field(&Dump(value)).finish(),
            ExprKind::Char(value) => f.debug_tuple("Char").field(&CanonChar(*value)).finish(),
            ExprKind::Rune(value) => f.debug_tuple("Rune").field(&CanonChar(*value)).finish(),
            ExprKind::Bool(value) => f.debug_tuple("Bool").field(value).finish(),
            ExprKind::Ident(name) => f.debug_tuple("Ident").field(&Dump(name)).finish(),
            ExprKind::Unary(op, expr) => f
                .debug_tuple("Unary")
                .field(&Dump(op))
                .field(&Dump(&**expr))
                .finish(),
            ExprKind::Binary(op, lhs, rhs) => f
                .debug_tuple("Binary")
                .field(&Dump(op))
                .field(&Dump(&**lhs))
                .field(&Dump(&**rhs))
                .finish(),
            ExprKind::Call(callee, args) => f
                .debug_tuple("Call")
                .field(&Dump(&**callee))
                .field(&list(args))
                .finish(),
            ExprKind::Field(base, name) => f
                .debug_tuple("Field")
                .field(&Dump(&**base))
                .field(&Dump(name))
                .finish(),
            ExprKind::Index(base, index) => f
                .debug_tuple("Index")
                .field(&Dump(&**base))
                .field(&Dump(&**index))
                .finish(),
            ExprKind::Range(lo, hi, inclusive) => f
                .debug_tuple("Range")
                .field(&Dump(&**lo))
                .field(&Dump(&**hi))
                .field(inclusive)
                .finish(),
            ExprKind::Tuple(items) => f.debug_tuple("Tuple").field(&list(items)).finish(),
            ExprKind::Array(items) => f.debug_tuple("Array").field(&list(items)).finish(),
            ExprKind::StructLit(name, fields) => f
                .debug_tuple("StructLit")
                .field(&Dump(name))
                .field(&list(fields))
                .finish(),
            ExprKind::Lambda(lambda) => f.debug_tuple("Lambda").field(&Dump(lambda)).finish(),
            ExprKind::Match(match_) => f.debug_tuple("Match").field(&Dump(&**match_)).finish(),
            ExprKind::Do(monad, binds) => f
                .debug_tuple("Do")
                .field(&opt(monad))
                .field(&list(binds))
                .finish(),
            ExprKind::Await(expr, ty) => f
                .debug_tuple("Await")
                .field(&Dump(&**expr))
                .field(&opt(ty))
                .finish(),
            ExprKind::SizeofType(ty) => f.debug_tuple("SizeofType").field(&Dump(ty)).finish(),
            ExprKind::Collect { ty, arg } => f
                .debug_struct("Collect")
                .field("ty", &Dump(ty))
                .field("arg", &Dump(&**arg))
                .finish(),
        }
    }
}

impl Debug for Dump<'_, UnOp> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            UnOp::Deref => f.write_str("Deref"),
            UnOp::Neg => f.write_str("Neg"),
            UnOp::Not => f.write_str("Not"),
            UnOp::BitNot => f.write_str("BitNot"),
        }
    }
}

impl Debug for Dump<'_, BinOp> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            BinOp::Add => f.write_str("Add"),
            BinOp::Sub => f.write_str("Sub"),
            BinOp::Mul => f.write_str("Mul"),
            BinOp::Div => f.write_str("Div"),
            BinOp::Mod => f.write_str("Mod"),
            BinOp::Eq => f.write_str("Eq"),
            BinOp::Ne => f.write_str("Ne"),
            BinOp::Lt => f.write_str("Lt"),
            BinOp::Le => f.write_str("Le"),
            BinOp::Gt => f.write_str("Gt"),
            BinOp::Ge => f.write_str("Ge"),
            BinOp::And => f.write_str("And"),
            BinOp::Or => f.write_str("Or"),
            BinOp::BitAnd => f.write_str("BitAnd"),
            BinOp::BitOr => f.write_str("BitOr"),
            BinOp::BitXor => f.write_str("BitXor"),
            BinOp::Shl => f.write_str("Shl"),
            BinOp::Shr => f.write_str("Shr"),
            BinOp::Pow => f.write_str("Pow"),
        }
    }
}
