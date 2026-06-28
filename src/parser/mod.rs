//! Parser: one base parser for the full grammar to a complete AST. M2.
//!
//! Recursive descent for items and statements, Pratt for expression precedence.
//! Paradigm gating is not done here; the parser accepts the whole language and
//! `sema` validates paradigm usage per file. Newlines separate statements, so a
//! line starting with `*` is a dereference, not a multiply.

pub mod ast;

use crate::diag::{Diagnostic, Span};
use crate::lexer::token::{Keyword, Token, TokenKind};
use ast::*;

/// Parses a token stream into a Module, collecting errors. The stream must end
/// with a `TokenKind::Eof` sentinel, as produced by `lexer::lex`.
pub fn parse(tokens: Vec<Token>) -> (Module, Vec<Diagnostic>) {
    Parser::new(tokens).module()
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    errors: Vec<Diagnostic>,
    no_struct: bool,
}

impl Parser {
    fn new(toks: Vec<Token>) -> Self {
        Parser {
            toks,
            pos: 0,
            errors: Vec::new(),
            no_struct: false,
        }
    }

    fn peek(&self) -> &TokenKind {
        &self.toks[self.pos].kind
    }

    fn peek2(&self) -> &TokenKind {
        self.toks
            .get(self.pos + 1)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn nl_here(&self) -> bool {
        self.toks[self.pos].nl_before
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn prev_hi(&self) -> u32 {
        if self.pos > 0 {
            self.toks[self.pos - 1].span.hi
        } else {
            0
        }
    }

    fn at(&self, k: &TokenKind) -> bool {
        self.peek() == k
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn bump(&mut self) {
        if !self.at_eof() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, k: &TokenKind) -> bool {
        if self.at(k) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, k: &TokenKind) -> bool {
        if self.eat(k) {
            true
        } else {
            self.error(format!("expected {:?}, found {:?}", k, self.peek()));
            false
        }
    }

    fn error(&mut self, msg: impl Into<String>) {
        let span = self.span();
        self.errors.push(Diagnostic::new(msg, span));
    }

    fn ident(&mut self) -> String {
        if matches!(self.peek(), TokenKind::Ident(_)) {
            match self.take_kind() {
                TokenKind::Ident(s) => s,
                _ => unreachable!(),
            }
        } else {
            self.error(format!("expected identifier, found {:?}", self.peek()));
            String::new()
        }
    }

    /// Moves the current token's kind out (replacing it with `Eof`) and advances.
    /// Avoids cloning the inner String of identifiers and literals.
    fn take_kind(&mut self) -> TokenKind {
        let k = std::mem::replace(&mut self.toks[self.pos].kind, TokenKind::Eof);
        self.pos += 1;
        k
    }

    /// Runs `f` with struct literals re-enabled, restoring the prior state. Used
    /// inside brackets so that `if f(P{..}) {` parses the struct, while the top
    /// level condition still forbids a bare `P { .. }`.
    fn with_struct<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = self.no_struct;
        self.no_struct = false;
        let r = f(self);
        self.no_struct = saved;
        r
    }

    fn module(mut self) -> (Module, Vec<Diagnostic>) {
        let mut paradigms = Vec::new();
        let mut imports = Vec::new();
        while self.at(&TokenKind::At) {
            self.bump();
            let dir = self.ident();
            match dir.as_str() {
                "paradigm" => paradigms.push(self.ident()),
                "import" => imports.push(self.import_path()),
                _ => self.error(format!("unknown directive '@{dir}'")),
            }
        }
        let mut items = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            if self.at(&TokenKind::Kw(Keyword::Monad)) {
                for f in self.monad_block() {
                    items.push(Item::Func(f));
                }
            } else if let Some(it) = self.item() {
                items.push(it);
            }
            if self.pos == before {
                self.error(format!("unexpected token {:?}", self.peek()));
                self.bump();
            }
        }
        (
            Module {
                paradigms,
                imports,
                items,
            },
            self.errors,
        )
    }

    fn dotted_path(&mut self) -> String {
        let mut path = self.ident();
        while self.eat(&TokenKind::Dot) {
            path.push('.');
            path.push_str(&self.ident());
        }
        path
    }

    /// An import path: a quoted string for a dawn git package like
    /// `"github.com/user/repo/mod"`, or a bare dotted path like `std.io`.
    fn import_path(&mut self) -> String {
        if matches!(self.peek(), TokenKind::Str(_)) {
            match self.take_kind() {
                TokenKind::Str(s) => s,
                _ => unreachable!(),
            }
        } else {
            self.dotted_path()
        }
    }

    fn item(&mut self) -> Option<Item> {
        let exported = self.eat(&TokenKind::Kw(Keyword::Export));
        match self.peek() {
            TokenKind::Kw(Keyword::Func) => Some(Item::Func(self.func(exported))),
            TokenKind::Kw(Keyword::Struct) => Some(Item::Struct(self.struct_(exported))),
            TokenKind::Kw(Keyword::Enum) => Some(Item::Enum(self.enum_(exported))),
            TokenKind::Kw(Keyword::Interface) => Some(Item::Interface(self.interface(exported))),
            TokenKind::Kw(Keyword::Impl) => Some(Item::Impl(self.impl_())),
            _ => {
                if exported {
                    self.error("expected an item after 'export'");
                }
                None
            }
        }
    }

    fn func(&mut self, exported: bool) -> Func {
        self.bump();
        let name = self.ident();
        let generics = self.generics();
        let params = self.params();
        self.expect(&TokenKind::Arrow);
        let ret = self.type_();
        let body = self.block();
        Func {
            exported,
            name,
            generics,
            params,
            ret,
            body,
        }
    }

    fn generics(&mut self) -> Vec<String> {
        let mut g = Vec::new();
        if self.eat(&TokenKind::Lt) {
            if !self.at(&TokenKind::Gt) {
                g.push(self.ident());
                while self.eat(&TokenKind::Comma) {
                    g.push(self.ident());
                }
            }
            self.expect(&TokenKind::Gt);
        }
        g
    }

    fn params(&mut self) -> Vec<Param> {
        let mut ps = Vec::new();
        self.expect(&TokenKind::LParen);
        if !self.at(&TokenKind::RParen) {
            loop {
                let using = self.eat(&TokenKind::Kw(Keyword::Using));
                let name = self.ident();
                self.expect(&TokenKind::Colon);
                let ty = self.type_();
                ps.push(Param { using, name, ty });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen);
        ps
    }

    fn struct_(&mut self, exported: bool) -> Struct {
        self.bump();
        let name = self.ident();
        let generics = self.generics();
        let fields = self.field_block();
        Struct {
            exported,
            name,
            generics,
            fields,
        }
    }

    fn field_block(&mut self) -> Vec<Field> {
        let mut fs = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let name = self.ident();
            self.expect(&TokenKind::Colon);
            let ty = self.type_();
            fs.push(Field { name, ty });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace);
        fs
    }

    fn enum_(&mut self, exported: bool) -> Enum {
        self.bump();
        let name = self.ident();
        let generics = self.generics();
        let mut variants = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let vname = self.ident();
            let mut fields = Vec::new();
            if self.eat(&TokenKind::LParen) {
                if !self.at(&TokenKind::RParen) {
                    loop {
                        let fname = self.ident();
                        self.expect(&TokenKind::Colon);
                        let ty = self.type_();
                        fields.push(Field { name: fname, ty });
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RParen);
            }
            variants.push(Variant { name: vname, fields });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace);
        Enum {
            exported,
            name,
            generics,
            variants,
        }
    }

    fn interface(&mut self, exported: bool) -> Interface {
        self.bump();
        let name = self.ident();
        let generics = self.generics();
        let mut methods = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let mname = self.ident();
            let params = self.params();
            self.expect(&TokenKind::Arrow);
            let ret = self.type_();
            self.eat(&TokenKind::Semi);
            methods.push(MethodSig {
                name: mname,
                params,
                ret,
            });
        }
        self.expect(&TokenKind::RBrace);
        Interface {
            exported,
            name,
            generics,
            methods,
        }
    }

    fn impl_(&mut self) -> Impl {
        self.bump();
        let first = self.ident();
        let (iface, ty) = if self.eat(&TokenKind::Kw(Keyword::For)) {
            (Some(first), self.ident())
        } else {
            (None, first)
        };
        let mut methods = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            if self.at(&TokenKind::Kw(Keyword::Func)) {
                methods.push(self.func(false));
            } else {
                self.error("expected a method (func) in impl block");
                self.bump();
            }
        }
        self.expect(&TokenKind::RBrace);
        Impl { iface, ty, methods }
    }

    /// Parses a `monad Name { funcs }` block, flattening its methods (typically
    /// `bind` and `unit`) into top level functions named `Name.method`. The
    /// namespace lets several monads each define `bind` and `unit`, and a
    /// `do Name { ... }` block desugars to that monad's pair.
    fn monad_block(&mut self) -> Vec<Func> {
        self.bump();
        let name = self.ident();
        let mut funcs = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let exported = self.eat(&TokenKind::Kw(Keyword::Export));
            if self.at(&TokenKind::Kw(Keyword::Func)) {
                let mut f = self.func(exported);
                f.name = format!("{name}.{}", f.name);
                funcs.push(f);
            } else {
                self.error("expected a method (func) in monad block");
                self.bump();
            }
        }
        self.expect(&TokenKind::RBrace);
        funcs
    }

    fn type_(&mut self) -> Type {
        if self.eat(&TokenKind::Star) {
            // `*raw T` is a thin pointer with no generation, the low level buffer
            // and allocator layer. A bare `*T` is the managed generational
            // pointer. `raw` is contextual: it only modifies the pointer when a
            // type follows, so a type named `raw` still works as a pointee.
            if matches!(self.peek(), TokenKind::Ident(s) if s == "raw")
                && matches!(self.peek2(), TokenKind::Ident(_) | TokenKind::Star)
            {
                self.bump();
                return Type::RawPtr(Box::new(self.type_()));
            }
            return Type::Ptr(Box::new(self.type_()));
        }
        let mut ty = self.base_type();
        while self.eat(&TokenKind::LBracket) {
            if self.eat(&TokenKind::RBracket) {
                ty = Type::Slice(Box::new(ty));
            } else {
                let n = self.array_len();
                self.expect(&TokenKind::RBracket);
                ty = Type::Array(Box::new(ty), n);
            }
        }
        ty
    }

    fn base_type(&mut self) -> Type {
        if self.eat(&TokenKind::LParen) {
            let mut elems = Vec::new();
            while !self.at(&TokenKind::RParen) && !self.at_eof() {
                elems.push(self.type_());
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen);
            if self.eat(&TokenKind::Arrow) {
                let ret = self.type_();
                return Type::Func(elems, Box::new(ret));
            }
            return match elems.len() {
                0 => Type::Unit,
                1 => elems.into_iter().next().unwrap(),
                _ => Type::Tuple(elems),
            };
        }
        let name = self.ident();
        if name == "void" {
            return Type::Unit;
        }
        let args = if self.at(&TokenKind::Lt) {
            self.type_args()
        } else {
            Vec::new()
        };
        Type::Named(name, args)
    }

    fn type_args(&mut self) -> Vec<Type> {
        let mut args = Vec::new();
        self.expect(&TokenKind::Lt);
        while !self.at(&TokenKind::Gt) && !self.at_eof() {
            args.push(self.type_());
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::Gt);
        args
    }

    fn array_len(&mut self) -> u64 {
        let val = if let TokenKind::Int { val, .. } = self.peek() {
            *val
        } else {
            self.error("expected an array length");
            return 0;
        };
        self.pos += 1;
        val as u64
    }

    fn block(&mut self) -> Block {
        let mut stmts = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            if let Some(s) = self.stmt() {
                stmts.push(s);
            }
            if self.pos == before {
                self.error(format!("unexpected token in block: {:?}", self.peek()));
                self.bump();
            }
        }
        self.expect(&TokenKind::RBrace);
        Block { stmts }
    }

    fn stmt(&mut self) -> Option<Stmt> {
        match self.peek() {
            TokenKind::Kw(Keyword::Return) => {
                self.bump();
                let e = if self.at(&TokenKind::RBrace) || self.nl_here() {
                    None
                } else {
                    Some(self.expr())
                };
                Some(Stmt::Return(e))
            }
            TokenKind::Kw(Keyword::Defer) => {
                self.bump();
                Some(Stmt::Defer(self.expr()))
            }
            TokenKind::Kw(Keyword::If) => Some(Stmt::If(self.if_())),
            TokenKind::Kw(Keyword::While) => Some(Stmt::While(self.while_())),
            TokenKind::Kw(Keyword::For) => Some(Stmt::For(self.for_())),
            TokenKind::Kw(Keyword::Match) => Some(Stmt::Match(self.match_())),
            TokenKind::Kw(Keyword::Do) => Some(self.do_stmt()),
            TokenKind::Kw(Keyword::Mut) => {
                self.bump();
                Some(Stmt::Let(self.let_rest(true, false)))
            }
            // `ref y = x` binds a non owning alias. `ref` is contextual: it only
            // leads a binding when an identifier follows, so `ref` stays usable
            // as a name elsewhere.
            TokenKind::Ident(s) if s == "ref" && matches!(self.peek2(), TokenKind::Ident(_)) => {
                self.bump();
                Some(Stmt::Let(self.let_rest(false, true)))
            }
            _ if self.is_binding_start() => Some(Stmt::Let(self.let_rest(false, false))),
            _ => {
                let e = self.expr();
                if self.eat(&TokenKind::Assign) {
                    let rhs = self.expr();
                    Some(Stmt::Assign(e, rhs))
                } else {
                    Some(Stmt::Expr(e))
                }
            }
        }
    }

    fn is_binding_start(&self) -> bool {
        matches!(self.peek(), TokenKind::Ident(_))
            && matches!(
                self.peek2(),
                TokenKind::Colon | TokenKind::ColonEq | TokenKind::Comma
            )
    }

    fn let_rest(&mut self, mutable: bool, is_ref: bool) -> Let {
        let mut binds = Vec::new();
        loop {
            let name = self.ident();
            let ty = if self.eat(&TokenKind::Colon) {
                Some(self.type_())
            } else {
                None
            };
            binds.push(Bind { name, ty });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let infer = if self.eat(&TokenKind::ColonEq) {
            true
        } else {
            self.expect(&TokenKind::Assign);
            false
        };
        if !infer && binds.iter().any(|b| b.ty.is_none()) {
            self.error("destructuring binding needs ':=' or a type on each name");
        }
        let value = self.expr();
        Let {
            mutable,
            is_ref,
            infer,
            binds,
            value,
        }
    }

    fn if_(&mut self) -> If {
        self.bump();
        let cond = self.expr_no_struct();
        let then = self.block();
        let els = if self.eat(&TokenKind::Kw(Keyword::Else)) {
            if self.at(&TokenKind::Kw(Keyword::If)) {
                Some(Block {
                    stmts: vec![Stmt::If(self.if_())],
                })
            } else {
                Some(self.block())
            }
        } else {
            None
        };
        If { cond, then, els }
    }

    fn while_(&mut self) -> While {
        self.bump();
        let cond = self.expr_no_struct();
        let body = self.block();
        While {
            cond,
            body,
            post_test: false,
        }
    }

    /// Parses a `do { ... }` construct in statement position. Followed by `while`
    /// it is a do while loop; otherwise it is a monadic do expression statement.
    fn do_stmt(&mut self) -> Stmt {
        let lo = self.span().lo;
        let (monad, elems) = self.do_block_elems();
        if self.at(&TokenKind::Kw(Keyword::While)) && !self.nl_here() {
            self.bump();
            let cond = self.expr_no_struct();
            if monad.is_some() {
                self.error("a 'do while' loop takes no monad name");
            }
            let mut stmts = Vec::new();
            for el in elems {
                match el {
                    DoElem::Plain(s) => stmts.push(s),
                    DoElem::Bind(..) => {
                        self.error("monadic bind '<-' is not allowed in a do while loop")
                    }
                }
            }
            Stmt::While(While {
                cond,
                body: Block { stmts },
                post_test: true,
            })
        } else {
            Stmt::Expr(self.do_to_expr(monad, elems, lo))
        }
    }

    /// Parses a monadic `do { ... }` or `do Name { ... }` in expression position.
    fn do_expr(&mut self) -> Expr {
        let lo = self.span().lo;
        let (monad, elems) = self.do_block_elems();
        self.do_to_expr(monad, elems, lo)
    }

    /// Parses the body of a `do` block: a sequence of `name <- expr` binds and
    /// ordinary statements, leaving the trailing `while`, if any, for the caller.
    fn do_block_elems(&mut self) -> (Option<String>, Vec<DoElem>) {
        self.bump();
        // An optional monad name precedes the brace, as in `do Maybe { ... }`.
        let monad = if matches!(self.peek(), TokenKind::Ident(_))
            && matches!(self.peek2(), TokenKind::LBrace)
        {
            Some(self.ident())
        } else {
            None
        };
        self.expect(&TokenKind::LBrace);
        let mut elems = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            if matches!(self.peek(), TokenKind::Ident(_))
                && matches!(self.peek2(), TokenKind::LArrow)
            {
                let name = self.ident();
                self.bump();
                let e = self.expr();
                elems.push(DoElem::Bind(name, e));
            } else if let Some(s) = self.stmt() {
                elems.push(DoElem::Plain(s));
            }
            if self.pos == before {
                self.error(format!("unexpected token in do block: {:?}", self.peek()));
                self.bump();
            }
        }
        self.expect(&TokenKind::RBrace);
        (monad, elems)
    }

    /// Builds a `Do` expression node from parsed do elements. Binds keep their
    /// name; bare expressions become anonymous sequencing steps; the final
    /// element is the result. The monad name, if any, selects the desugar target.
    fn do_to_expr(&mut self, monad: Option<String>, elems: Vec<DoElem>, lo: u32) -> Expr {
        let mut binds = Vec::new();
        for el in elems {
            match el {
                DoElem::Bind(name, expr) => binds.push(DoBind {
                    name: Some(name),
                    expr,
                }),
                DoElem::Plain(Stmt::Expr(expr)) => binds.push(DoBind { name: None, expr }),
                DoElem::Plain(_) => {
                    self.error("do notation allows only 'x <- e' binds and a final expression")
                }
            }
        }
        if matches!(binds.last(), Some(DoBind { name: Some(_), .. })) {
            self.error("a do block must end in an expression, not a 'x <- e' bind");
        }
        let span = Span::new(lo, self.prev_hi());
        node(ExprKind::Do(monad, binds), span)
    }

    fn for_(&mut self) -> For {
        self.bump();
        let var = self.ident();
        self.expect(&TokenKind::Kw(Keyword::In));
        let iter = self.expr_no_struct();
        let body = self.block();
        For { var, iter, body }
    }

    fn match_(&mut self) -> Match {
        self.bump();
        let scrut = Box::new(self.expr_no_struct());
        let mut arms = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let pat = self.pattern();
            self.expect(&TokenKind::FatArrow);
            let block_body = self.at(&TokenKind::LBrace);
            let body = if block_body {
                self.block()
            } else if self.at(&TokenKind::Kw(Keyword::Return)) {
                let before = self.pos;
                match self.stmt() {
                    Some(s) => Block { stmts: vec![s] },
                    None => {
                        if self.pos == before {
                            self.bump();
                        }
                        Block { stmts: Vec::new() }
                    }
                }
            } else {
                Block {
                    stmts: vec![Stmt::Expr(self.expr())],
                }
            };
            arms.push(Arm { pat, body });
            if block_body {
                self.eat(&TokenKind::Comma);
            } else if !self.at(&TokenKind::RBrace) {
                self.expect(&TokenKind::Comma);
            }
        }
        self.expect(&TokenKind::RBrace);
        Match { scrut, arms }
    }

    fn pattern(&mut self) -> Pattern {
        let name = self.ident();
        if name == "_" {
            return Pattern::Wildcard;
        }
        if self.eat(&TokenKind::LParen) {
            let mut binds = Vec::new();
            if !self.at(&TokenKind::RParen) {
                loop {
                    binds.push(self.ident());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RParen);
            Pattern::Variant(name, binds)
        } else {
            Pattern::Ident(name)
        }
    }

    fn expr(&mut self) -> Expr {
        self.range()
    }

    fn expr_no_struct(&mut self) -> Expr {
        let saved = self.no_struct;
        self.no_struct = true;
        let e = self.range();
        self.no_struct = saved;
        e
    }

    fn range(&mut self) -> Expr {
        let lhs = self.bin(0);
        if self.at(&TokenKind::DotDot) && !self.nl_here() {
            self.bump();
            let rhs = self.bin(0);
            let span = Span::new(lhs.span.lo, rhs.span.hi);
            return Expr {
                kind: ExprKind::Range(Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        lhs
    }

    fn bin(&mut self, min_bp: u8) -> Expr {
        let mut lhs = self.unary();
        loop {
            if self.nl_here() {
                break;
            }
            let Some((op, lbp, rbp)) = self.peek_binop() else {
                break;
            };
            if lbp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.bin(rbp);
            let span = Span::new(lhs.span.lo, rhs.span.hi);
            lhs = Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
            if is_cmp(op) && self.peek_binop().is_some_and(|(o, _, _)| is_cmp(o)) {
                self.error("comparison operators cannot be chained");
                break;
            }
        }
        lhs
    }

    fn peek_binop(&self) -> Option<(BinOp, u8, u8)> {
        let op = match self.peek() {
            TokenKind::OrOr => BinOp::Or,
            TokenKind::AndAnd => BinOp::And,
            TokenKind::EqEq => BinOp::Eq,
            TokenKind::Ne => BinOp::Ne,
            TokenKind::Lt => BinOp::Lt,
            TokenKind::Le => BinOp::Le,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::Ge => BinOp::Ge,
            TokenKind::Plus => BinOp::Add,
            TokenKind::Minus => BinOp::Sub,
            TokenKind::Star => BinOp::Mul,
            TokenKind::Slash => BinOp::Div,
            TokenKind::Percent => BinOp::Mod,
            _ => return None,
        };
        let lvl = match op {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 3,
            BinOp::Add | BinOp::Sub => 4,
            BinOp::Mul | BinOp::Div | BinOp::Mod => 5,
        };
        Some((op, lvl * 2, lvl * 2 + 1))
    }

    fn unary(&mut self) -> Expr {
        let lo = self.span().lo;
        let op = match self.peek() {
            TokenKind::Star => Some(UnOp::Deref),
            TokenKind::Minus => Some(UnOp::Neg),
            TokenKind::Bang => Some(UnOp::Not),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let e = self.unary();
            let span = Span::new(lo, e.span.hi);
            Expr {
                kind: ExprKind::Unary(op, Box::new(e)),
                span,
            }
        } else {
            self.postfix()
        }
    }

    fn postfix(&mut self) -> Expr {
        let mut e = self.primary();
        loop {
            if self.nl_here() {
                break;
            }
            match self.peek() {
                TokenKind::LParen => {
                    self.bump();
                    let args = self.with_struct(|p| p.expr_list(&TokenKind::RParen));
                    self.expect(&TokenKind::RParen);
                    let span = Span::new(e.span.lo, self.prev_hi());
                    e = Expr {
                        kind: ExprKind::Call(Box::new(e), args),
                        span,
                    };
                }
                TokenKind::LBracket => {
                    self.bump();
                    let idx = self.with_struct(|p| p.expr());
                    self.expect(&TokenKind::RBracket);
                    let span = Span::new(e.span.lo, self.prev_hi());
                    e = Expr {
                        kind: ExprKind::Index(Box::new(e), Box::new(idx)),
                        span,
                    };
                }
                TokenKind::Dot => {
                    self.bump();
                    let name = self.ident();
                    let span = Span::new(e.span.lo, self.prev_hi());
                    e = Expr {
                        kind: ExprKind::Field(Box::new(e), name),
                        span,
                    };
                }
                _ => break,
            }
        }
        e
    }

    fn expr_list(&mut self, end: &TokenKind) -> Vec<Expr> {
        let mut v = Vec::new();
        if !self.at(end) {
            loop {
                v.push(self.expr());
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        v
    }

    fn primary(&mut self) -> Expr {
        let span = self.toks[self.pos].span;
        let kind = self.peek().clone();
        match kind {
            TokenKind::Int { val, suffix } => {
                self.pos += 1;
                node(ExprKind::Int(val, suffix), span)
            }
            TokenKind::Float { val, suffix } => {
                self.pos += 1;
                node(ExprKind::Float(val, suffix), span)
            }
            TokenKind::Str(s) => {
                self.pos += 1;
                node(ExprKind::Str(s), span)
            }
            TokenKind::Char(c) => {
                self.pos += 1;
                node(ExprKind::Char(c), span)
            }
            TokenKind::Bool(b) => {
                self.pos += 1;
                node(ExprKind::Bool(b), span)
            }
            TokenKind::LBracket => {
                self.bump();
                let elems = self.with_struct(|p| p.expr_list(&TokenKind::RBracket));
                self.expect(&TokenKind::RBracket);
                let sp = Span::new(span.lo, self.prev_hi());
                node(ExprKind::Array(elems), sp)
            }
            TokenKind::Kw(Keyword::Lambda) => self.lambda_expr(),
            TokenKind::Kw(Keyword::Match) => {
                let m = self.match_();
                let sp = Span::new(span.lo, self.prev_hi());
                node(ExprKind::Match(Box::new(m)), sp)
            }
            TokenKind::Kw(Keyword::Do) => self.do_expr(),
            TokenKind::LParen => {
                self.bump();
                if self.at(&TokenKind::RParen) {
                    self.bump();
                    let sp = Span::new(span.lo, self.prev_hi());
                    return node(ExprKind::Tuple(Vec::new()), sp);
                }
                self.with_struct(|p| {
                    let first = p.expr();
                    if p.at(&TokenKind::Comma) {
                        let mut elems = vec![first];
                        while p.eat(&TokenKind::Comma) {
                            if p.at(&TokenKind::RParen) {
                                break;
                            }
                            elems.push(p.expr());
                        }
                        p.expect(&TokenKind::RParen);
                        let sp = Span::new(span.lo, p.prev_hi());
                        node(ExprKind::Tuple(elems), sp)
                    } else {
                        p.expect(&TokenKind::RParen);
                        let sp = Span::new(span.lo, p.prev_hi());
                        Expr {
                            kind: first.kind,
                            span: sp,
                        }
                    }
                })
            }
            TokenKind::Ident(name) => {
                self.pos += 1;
                if !self.no_struct && self.at(&TokenKind::LBrace) {
                    self.struct_lit(name, span)
                } else {
                    node(ExprKind::Ident(name), span)
                }
            }
            _ => {
                self.error(format!("expected an expression, found {:?}", self.peek()));
                self.bump();
                node(ExprKind::Ident(String::new()), span)
            }
        }
    }

    fn struct_lit(&mut self, name: String, start: Span) -> Expr {
        self.expect(&TokenKind::LBrace);
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let fname = self.ident();
            self.expect(&TokenKind::Colon);
            let val = self.with_struct(|p| p.expr());
            fields.push((fname, val));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace);
        let span = Span::new(start.lo, self.prev_hi());
        node(ExprKind::StructLit(name, fields), span)
    }

    fn lambda_expr(&mut self) -> Expr {
        let lo = self.span().lo;
        self.bump();
        let params = self.params();
        self.expect(&TokenKind::Arrow);
        let ret = self.type_();
        let body = self.block();
        let span = Span::new(lo, self.prev_hi());
        node(ExprKind::Lambda(Lambda { params, ret, body }), span)
    }
}

/// A parsed element of a `do` block, before it is classified as a monadic do
/// expression or a do while loop body.
enum DoElem {
    Bind(String, Expr),
    Plain(Stmt),
}

fn node(kind: ExprKind, span: Span) -> Expr {
    Expr { kind, span }
}

fn is_cmp(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn parse_ok(src: &str) -> Module {
        let (toks, lerr) = lex(src);
        assert!(lerr.is_empty(), "lex errors: {lerr:?}");
        let (m, perr) = parse(toks);
        assert!(perr.is_empty(), "parse errors: {perr:?}");
        m
    }

    #[test]
    fn directives_and_empty_func() {
        let m = parse_ok("@paradigm functional\n@import std.io\nfunc main() -> int32 { return 0 }");
        assert_eq!(m.paradigms, vec!["functional".to_string()]);
        assert_eq!(m.imports, vec!["std.io".to_string()]);
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn git_string_import() {
        let m = parse_ok(
            "@import \"github.com/user/repo/mod\"\n@import std.io\nfunc main() -> int32 { return 0 }",
        );
        assert_eq!(
            m.imports,
            vec!["github.com/user/repo/mod".to_string(), "std.io".to_string()]
        );
    }

    #[test]
    fn struct_enum_interface_impl() {
        let m = parse_ok(
            "struct Point { x: float64, y: float64 }\n\
             enum Shape { Circle(r: float64), Empty }\n\
             interface Display { toString() -> string; }\n\
             impl Display for Point { func toString() -> string { return \"p\" } }",
        );
        assert_eq!(m.items.len(), 4);
    }

    #[test]
    fn precedence_mul_over_add() {
        let m = parse_ok("func f() -> int64 { return 1 + 2 * 3 }");
        let Item::Func(f) = &m.items[0] else {
            panic!()
        };
        let Stmt::Return(Some(e)) = &f.body.stmts[0] else {
            panic!()
        };
        match &e.kind {
            ExprKind::Binary(BinOp::Add, _, rhs) => {
                assert!(matches!(rhs.kind, ExprKind::Binary(BinOp::Mul, _, _)));
            }
            other => panic!("expected add at root, got {other:?}"),
        }
    }

    #[test]
    fn deref_at_line_start_not_multiply() {
        let m = parse_ok("func f(p: *int64) -> void {\n  x := 5\n  *p = x\n}");
        let Item::Func(f) = &m.items[0] else {
            panic!()
        };
        assert_eq!(f.body.stmts.len(), 2);
        assert!(matches!(f.body.stmts[1], Stmt::Assign(_, _)));
    }

    #[test]
    fn tuple_destructure_and_call() {
        let m = parse_ok("func f() -> int32 {\n  y, e := g()\n  return 0\n}");
        let Item::Func(f) = &m.items[0] else {
            panic!()
        };
        let Stmt::Let(l) = &f.body.stmts[0] else {
            panic!()
        };
        assert_eq!(l.binds.len(), 2);
        assert!(l.infer);
    }

    #[test]
    fn match_and_lambda() {
        let m = parse_ok(
            "@paradigm functional\n\
             func f(s: Shape) -> float64 {\n\
               m := lambda (n: int64) -> int64 { return n }\n\
               match s {\n\
                 Circle(r) => return r,\n\
                 Empty => return 0.0,\n\
               }\n\
             }",
        );
        let Item::Func(f) = &m.items[0] else {
            panic!()
        };
        assert!(matches!(f.body.stmts[1], Stmt::Match(_)));
    }

    #[test]
    fn match_expr_arm_with_ident_body() {
        let m = parse_ok(
            "func f(c: C) -> int64 {\n\
               x := match c {\n\
                 A => 1,\n\
                 B(n) => n,\n\
                 _ => 0,\n\
               }\n\
               return x\n\
             }",
        );
        let Item::Func(f) = &m.items[0] else {
            panic!()
        };
        assert!(matches!(f.body.stmts[0], Stmt::Let(_)));
    }

    #[test]
    fn pointer_and_slice_types() {
        let m = parse_ok("func f(a: *Point, b: int32[], c: int32[4]) -> void { return }");
        let Item::Func(f) = &m.items[0] else {
            panic!()
        };
        assert!(matches!(f.params[0].ty, Type::Ptr(_)));
        assert!(matches!(f.params[1].ty, Type::Slice(_)));
        assert!(matches!(f.params[2].ty, Type::Array(_, 4)));
    }

    #[test]
    fn raw_pointer_type_is_distinct_from_managed() {
        let m = parse_ok("func f(a: *int64, b: *raw int64, c: *raw *int64) -> void { return }");
        let Item::Func(f) = &m.items[0] else {
            panic!()
        };
        // A bare `*T` is the managed pointer, `*raw T` the thin one, and `raw`
        // nests, so `*raw *int64` is a raw pointer to a managed pointer.
        assert!(matches!(f.params[0].ty, Type::Ptr(_)));
        assert!(matches!(f.params[1].ty, Type::RawPtr(_)));
        assert!(matches!(&f.params[2].ty, Type::RawPtr(inner) if matches!(&**inner, Type::Ptr(_))));

        // `raw` only modifies when a type follows, so `*raw` alone stays a
        // managed pointer to a type named raw, keeping raw usable as a name.
        let m2 = parse_ok("func g(x: *raw) -> void { return }");
        let Item::Func(g) = &m2.items[0] else {
            panic!()
        };
        assert!(
            matches!(&g.params[0].ty, Type::Ptr(inner) if matches!(&**inner, Type::Named(n, _) if n == "raw"))
        );
    }

    #[test]
    fn do_ending_in_bind_errors() {
        let (toks, _) = lex(
            "func f() -> int64 {\n  r := do {\n    a <- 1\n    b <- 2\n  }\n  return 0\n}",
        );
        let (_m, errs) = parse(toks);
        assert!(errs.iter().any(|d| d.msg.contains("must end in an expression")), "{errs:?}");
    }

    #[test]
    fn do_stmt_then_separate_while_parses() {
        let m = parse_ok(
            "func bind(x: int64, f: (int64) -> int64) -> int64 { return f(x) }\n\
             func unit(x: int64) -> int64 { return x }\n\
             func f() -> void {\n  mut i: int64 = 0\n  do {\n    a <- 1\n    a\n  }\n  while i < 3 {\n    i = i + 1\n  }\n}",
        );
        let Item::Func(f) = m.items.iter().find(|it| matches!(it, Item::Func(f) if f.name == "f")).unwrap() else {
            panic!()
        };
        // The do statement and the while loop are two separate statements.
        assert!(matches!(f.body.stmts.last(), Some(Stmt::While(_))));
    }

    #[test]
    fn do_while_parses() {
        let m = parse_ok(
            "func f() -> void {\n  mut x: int32 = 0\n  do {\n    x = x + 1\n  } while x < 3\n}",
        );
        let Item::Func(f) = &m.items[0] else {
            panic!()
        };
        match &f.body.stmts[1] {
            Stmt::While(w) => assert!(w.post_test),
            other => panic!("expected do while, got {other:?}"),
        }
    }

    #[test]
    fn comparison_chain_errors() {
        let (toks, _) = lex("func f() -> bool { return 1 < 2 < 3 }");
        let (_m, errs) = parse(toks);
        assert!(!errs.is_empty());
    }

    #[test]
    fn untyped_multi_assign_errors() {
        let (toks, _) = lex("func f() -> void {\n  a, b = g()\n}");
        let (_m, errs) = parse(toks);
        assert!(!errs.is_empty());
    }

    #[test]
    fn type_trailing_comma_ok() {
        let m = parse_ok("func f() -> (int32, int64,) { return (0, 0) }");
        let Item::Func(f) = &m.items[0] else {
            panic!()
        };
        assert!(matches!(f.ret, Type::Tuple(_)));
    }
}
