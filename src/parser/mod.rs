//! Parser: one base parser for the full grammar to a complete AST. M2.
//!
//! Recursive descent for items and statements, Pratt for expression precedence.
//! Paradigm gating is not done here; the parser accepts the whole language and
//! `sema` validates paradigm usage per file. Newlines separate statements, so a
//! line starting with `*` is a dereference, not a multiply.

pub mod ast;
pub mod dump;

pub use dump::{escape_canonical, render_module};

use crate::diag::{Diagnostic, Span};
use crate::lexer::token::{Keyword, Token, TokenKind};
use ast::*;

/// Parses a token stream into a Module, collecting errors. The stream must end
/// with a `TokenKind::Eof` sentinel, as produced by `lexer::lex`.
pub fn parse(tokens: Vec<Token>) -> (Module, Vec<Diagnostic>) {
    Parser::new(tokens).module()
}

/// The recursion-depth ceiling shared by the expression, type, and block parsers.
/// It sits far above any nesting a human writes, so a well-formed program never
/// meets it; it exists only so a pathological input such as `((((...`, `Vec<Vec<...`,
/// or deeply nested `if` blocks unwinds with a diagnostic instead of overflowing the
/// call stack. The existing `deep_nested_generic_parses` test threads 400 layers and
/// must stay clean, so the limit sits comfortably above that.
const MAX_NESTING: u32 = 500;

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    errors: Vec<Diagnostic>,
    no_struct: bool,
    // True while parsing an `async func` body. `await` is recognized as the
    // suspension keyword only here; everywhere else it stays an ordinary
    // identifier, so shipped sync callers of the stdlib `await(f)` keep parsing.
    in_async_fn: bool,
    // How many lambda bodies enclose the current position. Only the enclosing
    // async func can suspend, so `await` at depth greater than zero is rejected.
    lambda_depth: u32,
    // Live nesting depth of the expression, type, and block recursions, kept in
    // step by `enter_nesting` and its paired decrement so a pathological input
    // cannot overflow the stack. See `MAX_NESTING`.
    depth: u32,
    // Set once the nesting ceiling is crossed, so the depth diagnostic is emitted
    // a single time rather than once per bailed-out level.
    hit_depth_limit: bool,
}

impl Parser {
    fn new(toks: Vec<Token>) -> Self {
        Parser {
            toks,
            pos: 0,
            errors: Vec::new(),
            no_struct: false,
            in_async_fn: false,
            lambda_depth: 0,
            depth: 0,
            hit_depth_limit: false,
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

    /// The parser's one progress invariant, shared by every recovery loop. A
    /// recovery loop is a `while !at(CLOSER) && !at_eof()` loop that keeps going
    /// on any token rather than breaking on a missing separator; if its body ever
    /// consumes no token the same token would be re-read forever. Each such loop
    /// captures `self.pos` at its head and calls this at its tail: on a stall it
    /// emits a diagnostic naming the context and bumps one token, forcing
    /// progress, and returns true so a caller may also break. Routing every
    /// recovery loop through here keeps the invariant in one place instead of a
    /// hand-copied check per loop. List loops that break on a missing comma
    /// terminate through that break and must not call this, since a stall there
    /// can be the separator itself (`Foo<,>`), which the comma path recovers.
    fn guard_progress(&mut self, before: usize, ctx: &str) -> bool {
        if self.pos == before {
            self.error(format!("unexpected token in {ctx}: {:?}", self.peek()));
            self.bump();
            true
        } else {
            false
        }
    }

    /// Counts one level of expression, type, or block recursion, returning false
    /// once the nesting ceiling has been crossed. The expression, type, and block
    /// parsers wrap their recursive bodies in this: a caller that gets false returns
    /// a placeholder without recursing further, so a deeply nested input unwinds
    /// cleanly rather than overflowing the stack. Every call always increments and
    /// is balanced by a single `self.depth -= 1` on the way out, on both the
    /// accepted and the too-deep path. The diagnostic is emitted only on the first
    /// crossing, naming the construct (`what` is "expression", "type", or "block")
    /// and the fix (`fix` is the same noun for expressions and types, and "function"
    /// for a block, whose nesting is simplified at the enclosing function), so one
    /// clear error stands in for the flood of bailed-out levels.
    fn enter_nesting(&mut self, what: &str, fix: &str) -> bool {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            if !self.hit_depth_limit {
                self.hit_depth_limit = true;
                self.error(format!("{what} nesting is too deep; simplify the {fix}"));
            }
            false
        } else {
            true
        }
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
        let mut monads = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            if self.at(&TokenKind::Kw(Keyword::Monad)) {
                let (name, span, funcs) = self.monad_block();
                monads.push((name, span));
                for f in funcs {
                    items.push(Item::Func(f));
                }
            } else if let Some(it) = self.item() {
                items.push(it);
            }
            self.guard_progress(before, "module body");
        }
        (
            Module {
                paradigms,
                imports,
                monads,
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
            TokenKind::Kw(Keyword::Func) => Some(Item::Func(self.func(exported, false))),
            TokenKind::Kw(Keyword::Struct) => Some(Item::Struct(self.struct_(exported))),
            TokenKind::Kw(Keyword::Enum) => Some(Item::Enum(self.enum_(exported))),
            TokenKind::Kw(Keyword::Interface) => Some(Item::Interface(self.interface(exported))),
            TokenKind::Kw(Keyword::Impl) => Some(Item::Impl(self.impl_())),
            TokenKind::Kw(Keyword::Foreign) => Some(Item::Foreign(self.foreign())),
            // `async` is contextual: it leads an item only when `func` follows,
            // so `std.async.time` and any other use of the bare name is left
            // untouched. It composes with `export`, already consumed above.
            TokenKind::Ident(s) if s == "async" => {
                self.bump();
                if self.at(&TokenKind::Kw(Keyword::Func)) {
                    Some(Item::Func(self.func(exported, true)))
                } else {
                    self.error("expected 'func' after 'async'");
                    None
                }
            }
            _ => {
                if exported {
                    self.error("expected an item after 'export'");
                }
                None
            }
        }
    }

    fn func(&mut self, exported: bool, is_async: bool) -> Func {
        self.bump();
        let span = self.span();
        let name = self.ident();
        let generics = self.generics();
        let params = self.params();
        self.expect(&TokenKind::Arrow);
        let ret = self.type_();
        let saved = self.in_async_fn;
        self.in_async_fn = is_async;
        let body = self.block();
        self.in_async_fn = saved;
        Func {
            exported,
            is_async,
            name,
            span,
            generics,
            params,
            ret,
            body,
        }
    }

    fn generics(&mut self) -> Vec<String> {
        let mut g = Vec::new();
        if self.eat(&TokenKind::Lt) {
            if !self.at_type_close() {
                g.push(self.ident());
                while self.eat(&TokenKind::Comma) {
                    g.push(self.ident());
                }
            }
            self.expect_gt();
        }
        g
    }

    /// True when the current token can close a generic argument list. The lexer
    /// joins `>>`, `>>=`, and `>=` greedily, so a nested close such as the
    /// `>>` in `Wrap<Box<int64>>` arrives as one token; `expect_gt` splits the
    /// leading `>` back off.
    fn at_type_close(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Gt | TokenKind::Shr | TokenKind::ShrEq | TokenKind::Ge
        )
    }

    /// Consumes a single `>` that closes a generic list. A greedily joined
    /// `>>`, `>>=`, or `>=` is split in place: the leading `>` is dropped and the
    /// remainder stays as `Gt`, `Ge`, or `Assign` for the following parse, its
    /// span advanced past the consumed `>`. This also closes the pre-existing
    /// `Vec<T>= v` gap for free.
    fn expect_gt(&mut self) {
        let span = self.toks[self.pos].span;
        let rest = Span::new(span.lo + 1, span.hi);
        match self.peek().clone() {
            TokenKind::Gt => self.bump(),
            TokenKind::Shr => {
                self.toks[self.pos].kind = TokenKind::Gt;
                self.toks[self.pos].span = rest;
            }
            TokenKind::ShrEq => {
                self.toks[self.pos].kind = TokenKind::Ge;
                self.toks[self.pos].span = rest;
            }
            TokenKind::Ge => {
                self.toks[self.pos].kind = TokenKind::Assign;
                self.toks[self.pos].span = rest;
            }
            other => self.error(format!("expected '>', found {other:?}")),
        }
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
            variants.push(Variant {
                name: vname,
                fields,
            });
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
            let before = self.pos;
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
            self.guard_progress(before, "interface body");
        }
        self.expect(&TokenKind::RBrace);
        Interface {
            exported,
            name,
            generics,
            methods,
        }
    }

    /// `foreign "C" { func name(params) -> ret ... }`. Each entry is a function
    /// signature with no body, prefixed by `func` like an ordinary declaration.
    /// The abi string and the raw pointer boundary are validated in the checker.
    fn foreign(&mut self) -> Foreign {
        let span = self.span();
        self.bump();
        let abi = if matches!(self.peek(), TokenKind::Str(_)) {
            match self.take_kind() {
                TokenKind::Str(s) => s,
                _ => unreachable!(),
            }
        } else {
            self.error("expected a calling convention string after 'foreign', such as \"C\"");
            String::new()
        };
        let mut funcs = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            self.expect(&TokenKind::Kw(Keyword::Func));
            let name = self.ident();
            let params = self.params();
            self.expect(&TokenKind::Arrow);
            let ret = self.type_();
            self.eat(&TokenKind::Semi);
            funcs.push(ForeignFunc { name, params, ret });
            self.guard_progress(before, "foreign block");
        }
        self.expect(&TokenKind::RBrace);
        Foreign { abi, span, funcs }
    }

    fn impl_(&mut self) -> Impl {
        let span = self.span();
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
            let before = self.pos;
            if self.at(&TokenKind::Kw(Keyword::Func)) {
                methods.push(self.func(false, false));
            } else if matches!(self.peek(), TokenKind::Ident(s) if s == "async") {
                // A method cannot suspend, so an async method is rejected. The
                // `func` that follows is still parsed as an ordinary method so
                // its body does not spill a cascade of stray token errors.
                self.error("a method cannot be async");
                self.bump();
                if self.at(&TokenKind::Kw(Keyword::Func)) {
                    methods.push(self.func(false, false));
                }
            } else {
                self.error("expected a method (func) in impl block");
                self.bump();
            }
            self.guard_progress(before, "impl block");
        }
        self.expect(&TokenKind::RBrace);
        Impl {
            iface,
            ty,
            span,
            methods,
        }
    }

    /// Parses a `monad Name { funcs }` block, flattening its methods (typically
    /// `bind` and `unit`) into top level functions named `Name.method`. The
    /// namespace lets several monads each define `bind` and `unit`, and a
    /// `do Name { ... }` block desugars to that monad's pair. Returns the monad's
    /// name and keyword span too, recorded on the module for the paradigm gate.
    fn monad_block(&mut self) -> (String, Span, Vec<Func>) {
        let span = self.span();
        self.bump();
        let name = self.ident();
        let mut funcs = Vec::new();
        let mut methods = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            let exported = self.eat(&TokenKind::Kw(Keyword::Export));
            if self.at(&TokenKind::Kw(Keyword::Func)) {
                let mut f = self.func(exported, false);
                methods.push(f.name.clone());
                f.name = format!("{name}.{}", f.name);
                funcs.push(f);
            } else {
                self.error("expected a method (func) in monad block");
                self.bump();
            }
            self.guard_progress(before, "monad block");
        }
        self.expect(&TokenKind::RBrace);
        // A monad is the (unit, bind) pair, and a `do Name { ... }` block desugars
        // to calls of both. A block that defines only one of them would leave the
        // missing operation to resolve to nothing at the desugar site, so require
        // both here, at the monad's keyword, where the whole block is in view.
        let has = |m: &str| methods.iter().any(|n| n == m);
        if !has("bind") || !has("unit") {
            self.errors.push(Diagnostic::new(
                "a monad block must define both 'bind' and 'unit'",
                span,
            ));
        }
        (name, span, funcs)
    }

    /// Depth-guarded entry to the recursive type grammar. Every recursion into a
    /// nested type re-enters here, so the shared counter bounds the stack. On the
    /// too-deep path it yields `Type::Unit` as an inert placeholder, never the
    /// `Infer` hole, which only desugar may construct.
    fn type_(&mut self) -> Type {
        let ty = if self.enter_nesting("type", "type") {
            self.type_inner()
        } else {
            Type::Unit
        };
        self.depth -= 1;
        ty
    }

    fn type_inner(&mut self) -> Type {
        // `**T` is a pointer to a pointer. The lexer joins the stars into one
        // `**` token; split the leading `*` off as the outer pointer and let the
        // recursive parse consume the remaining `*`.
        if matches!(self.peek(), TokenKind::StarStar) {
            let span = self.toks[self.pos].span;
            self.toks[self.pos].kind = TokenKind::Star;
            self.toks[self.pos].span = Span::new(span.lo + 1, span.hi);
            return Type::Ptr(Box::new(self.type_()));
        }
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
        // `collector<T>` is a wrapper type, not an ordinary generic. It is
        // contextual: only `collector` immediately followed by `<` names the
        // wrapper, so a type named `collector` used bare stays an ordinary name.
        if name == "collector" && self.at(&TokenKind::Lt) {
            return Type::Collector(Box::new(self.collector_type_arg()));
        }
        let args = if self.at(&TokenKind::Lt) {
            self.type_args()
        } else {
            Vec::new()
        };
        Type::Named(name, args)
    }

    /// Parses the `< T >` element of a `collector<T>`, taking exactly one type. A
    /// missing or surplus argument is a diagnostic naming the one type form.
    fn collector_type_arg(&mut self) -> Type {
        let args = self.type_args();
        match args.len() {
            1 => args.into_iter().next().unwrap(),
            _ => {
                self.error("collector takes one element type: collector<T>");
                args.into_iter().next().unwrap_or(Type::Unit)
            }
        }
    }

    fn type_args(&mut self) -> Vec<Type> {
        let mut args = Vec::new();
        self.expect(&TokenKind::Lt);
        while !self.at_type_close() && !self.at_eof() {
            args.push(self.type_());
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect_gt();
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

    /// Depth-guarded entry to the statement grammar. Every control-flow body
    /// (`if`, `while`, `for`, `match` arm, `do while`) recurses back through here
    /// as `if_ -> block -> stmt -> if_`, so the shared counter bounds that stack
    /// alongside the expression and type recursions. On the too-deep path it
    /// consumes nothing and yields an empty block, exactly as the expression and
    /// type guards do: the enclosing recovery loop mops up the leftover tokens and
    /// the stack unwinds cleanly rather than overflowing.
    fn block(&mut self) -> Block {
        if !self.enter_nesting("block", "function") {
            self.depth -= 1;
            return Block { stmts: Vec::new() };
        }
        let mut stmts = Vec::new();
        self.expect(&TokenKind::LBrace);
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            if let Some(s) = self.stmt() {
                stmts.push(s);
            }
            self.guard_progress(before, "block");
        }
        self.expect(&TokenKind::RBrace);
        self.depth -= 1;
        Block { stmts }
    }

    fn stmt(&mut self) -> Option<Stmt> {
        match self.peek() {
            TokenKind::Kw(Keyword::Return) => {
                self.bump();
                let e = if self.at(&TokenKind::RBrace) || self.nl_here() {
                    None
                } else {
                    Some(self.await_or_expr())
                };
                Some(Stmt::Return(e))
            }
            TokenKind::Kw(Keyword::Defer) => {
                self.bump();
                // A defer runs at completion and cannot suspend. Inside an async
                // func a leading `await` is rejected here; in a sync func the
                // stdlib `defer await(f)` call is untouched.
                if self.in_async_fn && matches!(self.peek(), TokenKind::Ident(s) if s == "await") {
                    self.error(
                        "'await' cannot appear under defer; a defer runs at completion and cannot suspend",
                    );
                    self.bump();
                }
                Some(Stmt::Defer(self.expr()))
            }
            TokenKind::Kw(Keyword::If) => Some(Stmt::If(self.if_())),
            TokenKind::Kw(Keyword::While) => Some(Stmt::While(self.while_())),
            TokenKind::Kw(Keyword::For) => Some(Stmt::For(self.for_())),
            TokenKind::Kw(Keyword::Match) => Some(Stmt::Match(self.match_())),
            TokenKind::Kw(Keyword::Do) => Some(self.do_stmt()),
            TokenKind::Kw(Keyword::Break) => {
                let sp = self.span();
                self.bump();
                Some(Stmt::Break(sp))
            }
            TokenKind::Kw(Keyword::Continue) => {
                let sp = self.span();
                self.bump();
                Some(Stmt::Continue(sp))
            }
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
                let e = self.await_or_expr();
                if self.eat(&TokenKind::Assign) {
                    let rhs = self.expr();
                    Some(Stmt::Assign(e, rhs))
                } else if let Some(op) = compound_op(self.peek()) {
                    self.bump();
                    let rhs = self.expr();
                    Some(Stmt::AssignOp(op, e, rhs))
                } else if matches!(self.peek(), TokenKind::PlusPlus | TokenKind::MinusMinus)
                    && !self.nl_here()
                {
                    // `++`/`--` are statement only and postfix only, with no value.
                    // They desugar to a compound add or subtract of a literal 1, so
                    // the mut rules and single evaluation come from `AssignOp` for
                    // free. The newline guard keeps `x` and a following line that
                    // begins with `++` from joining.
                    let op = if matches!(self.peek(), TokenKind::PlusPlus) {
                        BinOp::Add
                    } else {
                        BinOp::Sub
                    };
                    let sp = self.span();
                    self.bump();
                    Some(Stmt::AssignOp(op, e, node(ExprKind::Int(1, None), sp)))
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
        let value = self.await_or_expr();
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
                // An `else if` recurses straight into `if_`, bypassing both block
                // guards, so a long chain adds a real stack frame per link while the
                // shared depth stays flat: each `then` block increments and returns
                // to zero before the next `else if`, so nothing accumulates and the
                // chain once overflowed the stack. Count the descent here so the
                // chain unwinds with a diagnostic like any other over-deep nesting.
                // The `then` block is already charged by `block`, so guarding only
                // this arm keeps the ceiling from being double-counted for ordinary
                // nested `if` blocks; the accumulation held across the recursive call
                // is what bounds the chain. Within each link the condition and `then`
                // block are parsed before this descent, so at the boundary the inner
                // condition crosses the ceiling first and the surfaced diagnostic
                // names the expression, exactly as the deep if-block path does; this
                // guard is what accumulates the depth to that point. It is charged
                // like a block, so it reuses the block wording. On the too-deep path
                // nothing is consumed and an empty else is returned, exactly as the
                // other guards do: the enclosing block loop mops up the leftover `if`.
                if !self.enter_nesting("block", "function") {
                    self.depth -= 1;
                    Some(Block { stmts: Vec::new() })
                } else {
                    let chained = Block {
                        stmts: vec![Stmt::If(self.if_())],
                    };
                    self.depth -= 1;
                    Some(chained)
                }
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
        // A `do { ... }` body recurses `do_stmt -> do_block_elems -> stmt ->
        // do_stmt` without passing through `block`, so it needs the same depth
        // guard: on the too-deep path it consumes nothing and yields an empty
        // body, letting the enclosing recovery loop mop up the leftover tokens.
        if !self.enter_nesting("block", "function") {
            self.depth -= 1;
            return (None, Vec::new());
        }
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
            self.guard_progress(before, "do block");
        }
        self.expect(&TokenKind::RBrace);
        self.depth -= 1;
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
            let before = self.pos;
            let pat = self.pattern();
            self.expect(&TokenKind::FatArrow);
            let block_body = self.at(&TokenKind::LBrace);
            let body = if block_body {
                self.block()
            } else if self.at(&TokenKind::Kw(Keyword::Return)) {
                let ret_start = self.pos;
                match self.stmt() {
                    Some(s) => Block { stmts: vec![s] },
                    None => {
                        if self.pos == ret_start {
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
            self.guard_progress(before, "match arm");
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

    /// Parses a statement-position value that may be an `await`. Inside an async
    /// func body, outside any lambda, a leading `await` is the suspension: it is
    /// consumed and the remainder parses as the awaited operand, wrapped in an
    /// `Await` node. Outside an async context a leading `await` not written as a
    /// call `await(f)` is the misplaced keyword and is rejected, then the ordinary
    /// expression parse runs for recovery. Everywhere else the parse is unchanged,
    /// so `await(f)` stays a plain call for sync callers of the stdlib function.
    fn await_or_expr(&mut self) -> Expr {
        if matches!(self.peek(), TokenKind::Ident(s) if s == "await") {
            if self.in_async_fn && self.lambda_depth == 0 {
                let lo = self.span().lo;
                self.bump();
                let op = self.expr();
                let span = Span::new(lo, op.span.hi);
                return node(ExprKind::Await(Box::new(op), None), span);
            }
            if !self.in_async_fn && !matches!(self.peek2(), TokenKind::LParen) {
                self.error("'await' is only legal inside an async func");
            }
        }
        self.expr()
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
        let lhs = self.pipe();
        let inclusive = match self.peek() {
            TokenKind::DotDot => false,
            TokenKind::DotDotEq => true,
            _ => return lhs,
        };
        if self.nl_here() {
            return lhs;
        }
        self.bump();
        let rhs = self.pipe();
        let span = Span::new(lhs.span.lo, rhs.span.hi);
        Expr {
            kind: ExprKind::Range(Box::new(lhs), Box::new(rhs), inclusive),
            span,
        }
    }

    /// The pipe operator `x |> f(a)`, the loosest binary form. It is a parse time
    /// rewrite to a plain call with the left side prepended as the first argument,
    /// so `x |> f(a)` is `f(x, a)`. It adds no capability, only a call spelling, so
    /// it is ungated; a piped functional builtin still faces the paradigm gate,
    /// which sees the rewritten call. Left associative: the inner `bin(0)` stops at
    /// the next `|>`.
    fn pipe(&mut self) -> Expr {
        let mut lhs = self.bin(0);
        while matches!(self.peek(), TokenKind::PipeGt) && !self.nl_here() {
            self.bump();
            let rhs = self.bin(0);
            lhs = self.make_pipe(lhs, rhs);
        }
        lhs
    }

    /// Rewrites `lhs |> rhs` into a call. When `rhs` is already a call, `lhs` is
    /// prepended to its arguments; when `rhs` is a bare function name or a field
    /// access, it becomes the sole argument of a fresh call; anything else is a
    /// parse error, recovering with `lhs`.
    fn make_pipe(&mut self, lhs: Expr, rhs: Expr) -> Expr {
        let span = Span::new(lhs.span.lo, rhs.span.hi);
        let rspan = rhs.span;
        match rhs.kind {
            ExprKind::Call(callee, mut args) => {
                args.insert(0, lhs);
                node(ExprKind::Call(callee, args), span)
            }
            kind @ (ExprKind::Ident(_) | ExprKind::Field(..)) => {
                let callee = Expr { kind, span: rspan };
                node(ExprKind::Call(Box::new(callee), vec![lhs]), span)
            }
            _ => {
                self.errors.push(Diagnostic::new(
                    "the right side of '|>' must be a function name or call",
                    rspan,
                ));
                lhs
            }
        }
    }

    /// Depth-guarded entry to the binary-operator layer. Right operands recurse
    /// through here (`self.bin(rbp)`), so a right-associative chain such as
    /// `2 ** 2 ** ...` is bounded alongside the paren and prefix recursions. On
    /// the too-deep path it yields an empty-identifier placeholder, the same inert
    /// node `primary` uses when it cannot form an expression.
    fn bin(&mut self, min_bp: u8) -> Expr {
        let e = if self.enter_nesting("expression", "expression") {
            self.bin_inner(min_bp)
        } else {
            node(ExprKind::Ident(String::new()), self.span())
        };
        self.depth -= 1;
        e
    }

    fn bin_inner(&mut self, min_bp: u8) -> Expr {
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
            TokenKind::Pipe => BinOp::BitOr,
            TokenKind::Caret => BinOp::BitXor,
            TokenKind::Amp => BinOp::BitAnd,
            TokenKind::Shl => BinOp::Shl,
            TokenKind::Shr => BinOp::Shr,
            TokenKind::Plus => BinOp::Add,
            TokenKind::Minus => BinOp::Sub,
            TokenKind::Star => BinOp::Mul,
            TokenKind::Slash => BinOp::Div,
            TokenKind::Percent => BinOp::Mod,
            TokenKind::StarStar => BinOp::Pow,
            _ => return None,
        };
        // The exponent operator is right associative: equal binding powers let
        // `bin(20)` re-accept a following `**`, so `2 ** 3 ** 2` is `2 ** (3 ** 2)`.
        if matches!(op, BinOp::Pow) {
            return Some((op, 20, 20));
        }
        // Bitwise operators sit between comparison and the shifts, and the shifts
        // between `&` and `+`, following Rust's ladder. Level 10 is `**`, above
        // the multiplicatives.
        let lvl = match op {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 3,
            BinOp::BitOr => 4,
            BinOp::BitXor => 5,
            BinOp::BitAnd => 6,
            BinOp::Shl | BinOp::Shr => 7,
            BinOp::Add | BinOp::Sub => 8,
            BinOp::Mul | BinOp::Div | BinOp::Mod => 9,
            BinOp::Pow => 10,
        };
        Some((op, lvl * 2, lvl * 2 + 1))
    }

    /// Depth-guarded entry to the prefix-operator layer. A prefix chain such as
    /// `----x` or `****x` recurses through here, and every paren, index, and call
    /// re-enters the expression grammar below it, so counting here bounds those
    /// stacks too. On the too-deep path it yields an empty-identifier placeholder.
    fn unary(&mut self) -> Expr {
        let e = if self.enter_nesting("expression", "expression") {
            self.unary_inner()
        } else {
            node(ExprKind::Ident(String::new()), self.span())
        };
        self.depth -= 1;
        e
    }

    fn unary_inner(&mut self) -> Expr {
        let lo = self.span().lo;
        // `**pp` at a prefix position is deref of deref, not the exponent
        // operator. Split the joined `**` into two `*`: consume one as a deref
        // here and leave the other, its span advanced, for the recursive parse.
        if matches!(self.peek(), TokenKind::StarStar) {
            let span = self.toks[self.pos].span;
            self.toks[self.pos].kind = TokenKind::Star;
            self.toks[self.pos].span = Span::new(span.lo + 1, span.hi);
            let e = self.unary();
            let sp = Span::new(lo, e.span.hi);
            return Expr {
                kind: ExprKind::Unary(UnOp::Deref, Box::new(e)),
                span: sp,
            };
        }
        let op = match self.peek() {
            TokenKind::Star => Some(UnOp::Deref),
            TokenKind::Minus => Some(UnOp::Neg),
            TokenKind::Bang => Some(UnOp::Not),
            TokenKind::Tilde => Some(UnOp::BitNot),
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
            TokenKind::Rune(c) => {
                self.pos += 1;
                node(ExprKind::Rune(c), span)
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
                // A statement-position await is consumed before it reaches here,
                // so an `await` that arrives at primary inside an async func is a
                // misuse: either buried mid-expression or trapped inside a lambda.
                if self.in_async_fn && name == "await" {
                    if self.lambda_depth == 0 {
                        self.error(
                            "'await' cannot appear mid-expression; give the awaited value a name, as in v, e := await f",
                        );
                    } else {
                        self.error(
                            "a lambda cannot await; only the enclosing async func can suspend",
                        );
                    }
                }
                // `collector<T>(value)` is the minting expression. It is
                // contextual: `collector` mints only when a balanced `<T>`
                // immediately followed by `(` proves the mint shape, so a value
                // named `collector` compared with `<`, as in `collector < n`,
                // stays an ordinary identifier. The newline guard keeps a
                // `collector` at a line end from joining a `<` on the next line.
                if name == "collector"
                    && self.at(&TokenKind::Lt)
                    && !self.nl_here()
                    && self.collector_mint_ahead()
                {
                    return self.collect_mint(span);
                }
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

    /// Non-destructive lookahead deciding whether `collector <` opens a mint,
    /// `collector<T>(value)`, or a comparison, `collector < expr`. A mint has a
    /// balanced angle-bracket type immediately followed by `(`; a comparison
    /// reaches an operator, a brace, a literal, or a statement boundary before the
    /// angles close. `self.pos` sits at the `<`. It only peeks, never consuming or
    /// rewriting a token, so a misjudged comparison re-parses cleanly, unlike a
    /// speculative parse that would split a joined `>>` in place.
    fn collector_mint_ahead(&self) -> bool {
        let mut i = self.pos;
        let mut depth: i32 = 0;
        let end = (self.pos + 128).min(self.toks.len());
        while i < end {
            match &self.toks[i].kind {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(
                            self.toks.get(i + 1).map(|t| &t.kind),
                            Some(TokenKind::LParen)
                        );
                    }
                }
                // A joined `>>` closes two nested levels at once, as in
                // `collector<Vec<int>>(x)`. It closes the mint only when it lands
                // exactly on depth zero and a `(` follows.
                TokenKind::Shr => {
                    depth -= 2;
                    if depth <= 0 {
                        return depth == 0
                            && matches!(
                                self.toks.get(i + 1).map(|t| &t.kind),
                                Some(TokenKind::LParen)
                            );
                    }
                }
                // The tokens a type argument list can contain: names, nested
                // angles handled above, commas, pointer stars, array brackets,
                // function-type parens and arrows, and an array length.
                TokenKind::Ident(_)
                | TokenKind::Comma
                | TokenKind::Star
                | TokenKind::StarStar
                | TokenKind::LBracket
                | TokenKind::RBracket
                | TokenKind::LParen
                | TokenKind::RParen
                | TokenKind::Arrow
                | TokenKind::Int { .. } => {}
                // Anything else, an operator, a brace, a literal, or a keyword,
                // ends the would-be type, so this is a comparison, not a mint.
                _ => return false,
            }
            i += 1;
        }
        false
    }

    /// Parses `collector<T>(value)` in expression position, the minting form. The
    /// element type is parsed like the wrapper type, then exactly one value
    /// argument follows in parentheses. A missing paren or a wrong argument count
    /// is a diagnostic naming the one value form.
    fn collect_mint(&mut self, start: Span) -> Expr {
        let ty = self.collector_type_arg();
        let arg = if self.eat(&TokenKind::LParen) {
            let args = self.with_struct(|p| p.expr_list(&TokenKind::RParen));
            self.expect(&TokenKind::RParen);
            if args.len() != 1 {
                self.error("collector takes one value: collector<T>(value)");
            }
            args.into_iter()
                .next()
                .unwrap_or_else(|| node(ExprKind::Ident(String::new()), start))
        } else {
            self.error("collector takes one value: collector<T>(value)");
            node(ExprKind::Ident(String::new()), start)
        };
        let span = Span::new(start.lo, self.prev_hi());
        node(
            ExprKind::Collect {
                ty,
                arg: Box::new(arg),
            },
            span,
        )
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
        // Only the enclosing async func can suspend, so an await inside a lambda
        // body is rejected; the depth counter marks the boundary.
        self.lambda_depth += 1;
        let body = self.block();
        self.lambda_depth -= 1;
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

/// Maps a compound assignment token to its binary operator, or None when the
/// token does not begin a compound assignment.
fn compound_op(k: &TokenKind) -> Option<BinOp> {
    Some(match k {
        TokenKind::PlusEq => BinOp::Add,
        TokenKind::MinusEq => BinOp::Sub,
        TokenKind::StarEq => BinOp::Mul,
        TokenKind::SlashEq => BinOp::Div,
        TokenKind::PercentEq => BinOp::Mod,
        TokenKind::AmpEq => BinOp::BitAnd,
        TokenKind::PipeEq => BinOp::BitOr,
        TokenKind::CaretEq => BinOp::BitXor,
        TokenKind::ShlEq => BinOp::Shl,
        TokenKind::ShrEq => BinOp::Shr,
        _ => return None,
    })
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
    use std::thread;

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
    fn foreign_block_parses() {
        let m = parse_ok(
            "foreign \"C\" {\n\
             func abs(n: int32) -> int32\n\
             func labs(n: int64) -> int64\n\
             }",
        );
        assert_eq!(m.items.len(), 1);
        let Item::Foreign(fb) = &m.items[0] else {
            panic!("expected a foreign block")
        };
        assert_eq!(fb.abi, "C");
        assert_eq!(fb.funcs.len(), 2);
        assert_eq!(fb.funcs[0].name, "abs");
        assert_eq!(fb.funcs[0].params.len(), 1);
        assert_eq!(fb.funcs[1].name, "labs");
    }

    #[test]
    fn precedence_mul_over_add() {
        let m = parse_ok("func f() -> int64 { return 1 + 2 * 3 }");
        let Item::Func(f) = &m.items[0] else { panic!() };
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
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert_eq!(f.body.stmts.len(), 2);
        assert!(matches!(f.body.stmts[1], Stmt::Assign(_, _)));
    }

    #[test]
    fn tuple_destructure_and_call() {
        let m = parse_ok("func f() -> int32 {\n  y, e := g()\n  return 0\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
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
        let Item::Func(f) = &m.items[0] else { panic!() };
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
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert!(matches!(f.body.stmts[0], Stmt::Let(_)));
    }

    #[test]
    fn pointer_and_slice_types() {
        let m = parse_ok("func f(a: *Point, b: int32[], c: int32[4]) -> void { return }");
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert!(matches!(f.params[0].ty, Type::Ptr(_)));
        assert!(matches!(f.params[1].ty, Type::Slice(_)));
        assert!(matches!(f.params[2].ty, Type::Array(_, 4)));
    }

    #[test]
    fn raw_pointer_type_is_distinct_from_managed() {
        let m = parse_ok("func f(a: *int64, b: *raw int64, c: *raw *int64) -> void { return }");
        let Item::Func(f) = &m.items[0] else { panic!() };
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
        let (toks, _) =
            lex("func f() -> int64 {\n  r := do {\n    a <- 1\n    b <- 2\n  }\n  return 0\n}");
        let (_m, errs) = parse(toks);
        assert!(
            errs.iter()
                .any(|d| d.msg.contains("must end in an expression")),
            "{errs:?}"
        );
    }

    #[test]
    fn do_stmt_then_separate_while_parses() {
        let m = parse_ok(
            "func bind(x: int64, f: (int64) -> int64) -> int64 { return f(x) }\n\
             func unit(x: int64) -> int64 { return x }\n\
             func f() -> void {\n  mut i: int64 = 0\n  do {\n    a <- 1\n    a\n  }\n  while i < 3 {\n    i = i + 1\n  }\n}",
        );
        let Item::Func(f) = m
            .items
            .iter()
            .find(|it| matches!(it, Item::Func(f) if f.name == "f"))
            .unwrap()
        else {
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
        let Item::Func(f) = &m.items[0] else { panic!() };
        match &f.body.stmts[1] {
            Stmt::While(w) => assert!(w.post_test),
            other => panic!("expected do while, got {other:?}"),
        }
    }

    #[test]
    fn bitwise_below_comparison_shift_between_and_add() {
        // `1 & 3 == 1` groups as `(1 & 3) == 1`, so `&` binds tighter than `==`.
        let m = parse_ok("func f() -> bool { return 1 & 3 == 1 }");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Return(Some(e)) = &f.body.stmts[0] else {
            panic!()
        };
        match &e.kind {
            ExprKind::Binary(BinOp::Eq, lhs, _) => {
                assert!(matches!(lhs.kind, ExprKind::Binary(BinOp::BitAnd, _, _)));
            }
            other => panic!("expected == at root over a & subtree, got {other:?}"),
        }
    }

    #[test]
    fn shift_binds_looser_than_add() {
        // `1 + 2 << 3` groups as `(1 + 2) << 3`, so `+` binds tighter than `<<`.
        let m = parse_ok("func f() -> int64 { return 1 + 2 << 3 }");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Return(Some(e)) = &f.body.stmts[0] else {
            panic!()
        };
        match &e.kind {
            ExprKind::Binary(BinOp::Shl, lhs, _) => {
                assert!(matches!(lhs.kind, ExprKind::Binary(BinOp::Add, _, _)));
            }
            other => panic!("expected << at root over an add subtree, got {other:?}"),
        }
    }

    #[test]
    fn exponent_is_right_associative() {
        // `2 ** 3 ** 2` groups as `2 ** (3 ** 2)`, so the right child is a Pow.
        let m = parse_ok("func f() -> int64 { return 2 ** 3 ** 2 }");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Return(Some(e)) = &f.body.stmts[0] else {
            panic!()
        };
        match &e.kind {
            ExprKind::Binary(BinOp::Pow, _, rhs) => {
                assert!(matches!(rhs.kind, ExprKind::Binary(BinOp::Pow, _, _)));
            }
            other => panic!("expected right associative Pow, got {other:?}"),
        }
    }

    #[test]
    fn unary_minus_binds_tighter_than_exponent() {
        // `-2 ** 2` is `(-2) ** 2`, so the left child of Pow is a negation.
        let m = parse_ok("func f() -> int64 { return -2 ** 2 }");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Return(Some(e)) = &f.body.stmts[0] else {
            panic!()
        };
        match &e.kind {
            ExprKind::Binary(BinOp::Pow, lhs, _) => {
                assert!(matches!(lhs.kind, ExprKind::Unary(UnOp::Neg, _)));
            }
            other => panic!("expected Pow with a negated base, got {other:?}"),
        }
    }

    #[test]
    fn compound_assignment_produces_assign_op() {
        let m = parse_ok("func f() -> void {\n  mut x: int64 = 0\n  x += 3\n  return\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        match &f.body.stmts[1] {
            Stmt::AssignOp(BinOp::Add, lhs, rhs) => {
                assert!(matches!(lhs.kind, ExprKind::Ident(_)));
                assert!(matches!(rhs.kind, ExprKind::Int(3, _)));
            }
            other => panic!("expected AssignOp(Add, ..), got {other:?}"),
        }
    }

    #[test]
    fn all_compound_forms_map_to_their_operator() {
        for (src, want) in [
            ("x -= 1", BinOp::Sub),
            ("x *= 1", BinOp::Mul),
            ("x /= 1", BinOp::Div),
            ("x %= 1", BinOp::Mod),
            ("x &= 1", BinOp::BitAnd),
            ("x |= 1", BinOp::BitOr),
            ("x ^= 1", BinOp::BitXor),
            ("x <<= 1", BinOp::Shl),
            ("x >>= 1", BinOp::Shr),
        ] {
            let prog = format!("func f() -> void {{\n  mut x: int64 = 0\n  {src}\n  return\n}}");
            let m = parse_ok(&prog);
            let Item::Func(f) = &m.items[0] else { panic!() };
            match &f.body.stmts[1] {
                Stmt::AssignOp(op, ..) => assert_eq!(*op, want, "for {src}"),
                other => panic!("expected AssignOp for {src}, got {other:?}"),
            }
        }
    }

    #[test]
    fn increment_desugars_to_compound_add_of_one() {
        let m = parse_ok("func f() -> void {\n  mut i: int64 = 0\n  i++\n  return\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        match &f.body.stmts[1] {
            Stmt::AssignOp(BinOp::Add, lhs, rhs) => {
                assert!(matches!(lhs.kind, ExprKind::Ident(_)));
                assert!(matches!(rhs.kind, ExprKind::Int(1, None)));
            }
            other => panic!("expected AssignOp(Add, i, 1), got {other:?}"),
        }
    }

    #[test]
    fn decrement_desugars_to_compound_sub_of_one() {
        let m = parse_ok("func f() -> void {\n  mut i: int64 = 0\n  i--\n  return\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        match &f.body.stmts[1] {
            Stmt::AssignOp(BinOp::Sub, _, rhs) => {
                assert!(matches!(rhs.kind, ExprKind::Int(1, None)));
            }
            other => panic!("expected AssignOp(Sub, i, 1), got {other:?}"),
        }
    }

    #[test]
    fn increment_on_a_place_expression() {
        // `++`/`--` work on any place, so `xs[i]++` and `s.f++` are increments of
        // the element or field.
        let m =
            parse_ok("func f() -> void {\n  mut xs: int64[3] = [0, 0, 0]\n  xs[1]++\n  return\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert!(matches!(
            &f.body.stmts[1],
            Stmt::AssignOp(BinOp::Add, lhs, _) if matches!(lhs.kind, ExprKind::Index(..))
        ));
    }

    #[test]
    fn exclusive_and_inclusive_range_flags() {
        let m = parse_ok(
            "func f(xs: int64[]) -> void {\n  a := xs[1..3]\n  b := xs[1..=3]\n  println(a[0])\n  println(b[0])\n  return\n}",
        );
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Let(la) = &f.body.stmts[0] else {
            panic!()
        };
        let Stmt::Let(lb) = &f.body.stmts[1] else {
            panic!()
        };
        let ExprKind::Index(_, ai) = &la.value.kind else {
            panic!("expected an index")
        };
        let ExprKind::Index(_, bi) = &lb.value.kind else {
            panic!("expected an index")
        };
        assert!(
            matches!(ai.kind, ExprKind::Range(_, _, false)),
            "exclusive flag"
        );
        assert!(
            matches!(bi.kind, ExprKind::Range(_, _, true)),
            "inclusive flag"
        );
    }

    #[test]
    fn pipe_into_a_call_prepends_the_left_side() {
        // `x |> f(a)` rewrites to `f(x, a)`: x is prepended as the first argument.
        let m = parse_ok("func f() -> int64 {\n  x := 1\n  return x |> g(2)\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Return(Some(e)) = &f.body.stmts[1] else {
            panic!()
        };
        let ExprKind::Call(callee, args) = &e.kind else {
            panic!("expected a call, got {:?}", e.kind)
        };
        assert!(matches!(&callee.kind, ExprKind::Ident(n) if n == "g"));
        assert_eq!(args.len(), 2);
        assert!(matches!(&args[0].kind, ExprKind::Ident(n) if n == "x"));
        assert!(matches!(args[1].kind, ExprKind::Int(2, _)));
    }

    #[test]
    fn pipe_into_a_bare_name_makes_a_call() {
        // `x |> f` rewrites to `f(x)`.
        let m = parse_ok("func f() -> int64 {\n  x := 1\n  return x |> g\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Return(Some(e)) = &f.body.stmts[1] else {
            panic!()
        };
        let ExprKind::Call(callee, args) = &e.kind else {
            panic!("expected a call, got {:?}", e.kind)
        };
        assert!(matches!(&callee.kind, ExprKind::Ident(n) if n == "g"));
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn pipe_is_left_associative_and_chains() {
        // `x |> f |> g` is `g(f(x))`: the outer call is g over the inner f call.
        let m = parse_ok("func f() -> int64 {\n  x := 1\n  return x |> f |> g\n}");
        let Item::Func(func) = &m.items[0] else {
            panic!()
        };
        let Stmt::Return(Some(e)) = &func.body.stmts[1] else {
            panic!()
        };
        let ExprKind::Call(callee, args) = &e.kind else {
            panic!()
        };
        assert!(matches!(&callee.kind, ExprKind::Ident(n) if n == "g"));
        assert!(
            matches!(&args[0].kind, ExprKind::Call(inner, _) if matches!(&inner.kind, ExprKind::Ident(n) if n == "f"))
        );
    }

    #[test]
    fn pipe_into_a_non_callable_errors() {
        let (toks, _) = lex("func f() -> int64 {\n  x := 1\n  return x |> 3\n}");
        let (_m, errs) = parse(toks);
        assert!(
            errs.iter()
                .any(|d| d.msg == "the right side of '|>' must be a function name or call"),
            "{errs:?}"
        );
    }

    #[test]
    fn pipe_is_looser_than_arithmetic() {
        // `1 + 2 |> f` is `f(1 + 2)`, so the left side keeps its `+`.
        let m = parse_ok("func f() -> int64 { return 1 + 2 |> g }");
        let Item::Func(func) = &m.items[0] else {
            panic!()
        };
        let Stmt::Return(Some(e)) = &func.body.stmts[0] else {
            panic!()
        };
        let ExprKind::Call(_, args) = &e.kind else {
            panic!("expected a call, got {:?}", e.kind)
        };
        assert!(matches!(&args[0].kind, ExprKind::Binary(BinOp::Add, _, _)));
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
    fn nested_generic_close_splits_joined_shr() {
        // The lexer joins the two closing `>` into one `>>`; the parser must
        // split it so `Wrap<Box<int64>>` types both layers.
        let m = parse_ok("func f(w: Wrap<Box<int64>>) -> void { return }");
        let Item::Func(f) = &m.items[0] else { panic!() };
        match &f.params[0].ty {
            Type::Named(outer, args) => {
                assert_eq!(outer, "Wrap");
                assert_eq!(args.len(), 1);
                assert!(matches!(&args[0], Type::Named(inner, inner_args)
                    if inner == "Box" && matches!(&inner_args[0], Type::Named(n, _) if n == "int64")));
            }
            other => panic!("expected Wrap<Box<int64>>, got {other:?}"),
        }
    }

    #[test]
    fn generic_close_before_assign_splits_ge() {
        // `x: Vec<int64>= v` closes the generic and leaves `=` as the assignment.
        let m = parse_ok("func f() -> void {\n  x: Vec<int64>= v\n  return\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Let(l) = &f.body.stmts[0] else {
            panic!("expected a let binding")
        };
        assert_eq!(l.binds.len(), 1);
        assert!(matches!(&l.binds[0].ty, Some(Type::Named(n, _)) if n == "Vec"));
    }

    #[test]
    fn double_star_at_prefix_is_deref_of_deref() {
        // `**pp = 5` stores through a pointer to a pointer, not an exponent.
        let m = parse_ok("func f(pp: **int64) -> void {\n  **pp = 5\n  return\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Assign(lhs, _) = &f.body.stmts[0] else {
            panic!("expected an assignment")
        };
        assert!(matches!(&lhs.kind, ExprKind::Unary(UnOp::Deref, inner)
            if matches!(&inner.kind, ExprKind::Unary(UnOp::Deref, _))));
    }

    #[test]
    fn type_trailing_comma_ok() {
        let m = parse_ok("func f() -> (int32, int64,) { return (0, 0) }");
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert!(matches!(f.ret, Type::Tuple(_)));
    }

    fn parse_errs(src: &str) -> Vec<Diagnostic> {
        let (toks, _) = lex(src);
        let (_m, errs) = parse(toks);
        errs
    }

    #[test]
    fn async_func_sets_the_flag() {
        let m = parse_ok("async func g() -> int64 {\n  return 1\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert!(f.is_async, "async func must set is_async");
        assert_eq!(f.name, "g");
    }

    #[test]
    fn monad_block_missing_bind_is_rejected() {
        let e = parse_errs(
            "monad M {\n  func unit(x: int64) -> int64 { return x }\n}\nfunc main() -> int32 { return 0 }",
        );
        assert!(
            e.iter()
                .any(|d| d.msg.contains("must define both 'bind' and 'unit'")),
            "{e:?}"
        );
    }

    #[test]
    fn complete_monad_block_parses() {
        let e = parse_errs(
            "monad M {\n  func bind(x: int64, f: (int64) -> int64) -> int64 { return f(x) }\n  func unit(x: int64) -> int64 { return x }\n}\nfunc main() -> int32 { return 0 }",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn export_async_func_composes() {
        let m = parse_ok("export async func g() -> int64 {\n  return 1\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert!(f.is_async && f.exported, "export and async compose");
    }

    #[test]
    fn plain_func_is_not_async() {
        let m = parse_ok("func g() -> int64 {\n  return 1\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert!(!f.is_async);
    }

    #[test]
    fn async_as_a_bare_name_is_untouched() {
        // `async` only leads an item when `func` follows; the qualified name in a
        // call keeps parsing as a field path.
        let m = parse_ok("func f() -> void {\n  std.async.time(1)\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert!(matches!(f.body.stmts[0], Stmt::Expr(_)));
    }

    #[test]
    fn async_without_func_errors() {
        let errs = parse_errs("async g() -> int64 { return 1 }");
        assert!(
            errs.iter()
                .any(|d| d.msg == "expected 'func' after 'async'"),
            "{errs:?}"
        );
    }

    #[test]
    fn await_let_destructure_in_async() {
        let m = parse_ok(
            "async func g() -> int64 {\n  v, e := await leaf()\n  e.ignore()\n  return v\n}",
        );
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Let(l) = &f.body.stmts[0] else {
            panic!("expected a let")
        };
        assert!(
            matches!(l.value.kind, ExprKind::Await(..)),
            "value is an await: {:?}",
            l.value.kind
        );
    }

    #[test]
    fn await_single_bind_in_async() {
        let m = parse_ok("async func g() -> int64 {\n  v := await leaf()\n  return v\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Let(l) = &f.body.stmts[0] else {
            panic!()
        };
        assert!(matches!(l.value.kind, ExprKind::Await(..)));
    }

    #[test]
    fn return_await_in_async() {
        let m = parse_ok("async func g() -> int64 {\n  return await leaf()\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Return(Some(e)) = &f.body.stmts[0] else {
            panic!()
        };
        assert!(matches!(e.kind, ExprKind::Await(..)));
    }

    #[test]
    fn void_await_form_in_async() {
        let m = parse_ok("async func g() -> void {\n  await tick()\n  return\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Expr(e) = &f.body.stmts[0] else {
            panic!("expected an expr stmt")
        };
        assert!(matches!(e.kind, ExprKind::Await(..)));
    }

    #[test]
    fn await_call_absorbs_the_name_in_async() {
        // Inside an async body the keyword absorbs the name, so `await(f)` is a
        // suspension of the parenthesized operand, not a call to a function.
        let m = parse_ok("async func g() -> int64 {\n  v := await(leaf())\n  return v\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Let(l) = &f.body.stmts[0] else {
            panic!()
        };
        let ExprKind::Await(op, _) = &l.value.kind else {
            panic!("expected an await")
        };
        assert!(
            matches!(op.kind, ExprKind::Call(..)),
            "operand is the inner call"
        );
    }

    #[test]
    fn await_call_in_sync_stays_a_call() {
        // Outside an async func `await(f)` remains an ordinary call, so shipped
        // sync callers of the stdlib await keep compiling.
        let m =
            parse_ok("func f() -> int64 {\n  v, e := await(leaf())\n  e.ignore()\n  return v\n}");
        let Item::Func(f) = &m.items[0] else { panic!() };
        let Stmt::Let(l) = &f.body.stmts[0] else {
            panic!()
        };
        assert!(
            matches!(l.value.kind, ExprKind::Call(..)),
            "value is a call: {:?}",
            l.value.kind
        );
    }

    #[test]
    fn await_in_sync_func_is_rejected() {
        let errs =
            parse_errs("func f() -> int64 {\n  v, e := await leaf()\n  e.ignore()\n  return v\n}");
        assert!(
            errs.iter()
                .any(|d| d.msg == "'await' is only legal inside an async func"),
            "{errs:?}"
        );
    }

    #[test]
    fn await_mid_expression_is_rejected() {
        let errs = parse_errs("async func g() -> int64 {\n  x := 1 + await leaf()\n  return x\n}");
        assert!(
            errs.iter()
                .any(|d| d.msg.contains("'await' cannot appear mid-expression")),
            "{errs:?}"
        );
    }

    #[test]
    fn await_inside_a_lambda_is_rejected() {
        let errs = parse_errs(
            "async func g() -> int64 {\n  h := lambda () -> int64 {\n    v := await leaf()\n    return v\n  }\n  return h()\n}",
        );
        assert!(
            errs.iter().any(|d| d.msg.contains("a lambda cannot await")),
            "{errs:?}"
        );
    }

    #[test]
    fn await_under_defer_is_rejected() {
        let errs = parse_errs("async func g() -> int64 {\n  defer await tick()\n  return 1\n}");
        assert!(
            errs.iter()
                .any(|d| d.msg.contains("'await' cannot appear under defer")),
            "{errs:?}"
        );
    }

    #[test]
    fn async_method_is_rejected() {
        let errs = parse_errs("impl T {\n  async func m() -> int64 { return 1 }\n}");
        assert!(
            errs.iter().any(|d| d.msg == "a method cannot be async"),
            "{errs:?}"
        );
    }

    // Progress invariant. Every recovery loop routes its no-progress case through
    // `guard_progress`, so a malformed body cannot spin the parser. Each snippet
    // below lands in a different recovery loop; the call to `parse` returning at
    // all is the termination proof, and a malformed program must be rejected, not
    // silently accepted.
    #[test]
    fn recovery_loops_terminate_with_diagnostics() {
        let cases = [
            "struct S { , , , }",
            "enum E { , , , }",
            "interface I { + + + }",
            "foreign \"C\" { + + + }",
            "impl T { + + + }",
            "monad M { + + + }",
            "func f() -> void { @ @ @ }",
            "func f() -> void { match x { @ @ @ } }",
            "func f() -> void { do { @ @ @ } }",
            "@ @ @ @",
            "func f() -> Foo<,,,> { return }",
            "func f(,,,) -> void { return }",
            "func f() -> void { g(,,,) }",
            "func f() -> void { x := [,,,] }",
            "func f() -> (,,,) { return }",
        ];
        for src in cases {
            let errs = parse_errs(src);
            assert!(
                !errs.is_empty(),
                "malformed input silently accepted: {src:?}"
            );
        }
    }

    #[test]
    fn stalled_interface_body_names_its_context() {
        let errs = parse_errs("interface I { + }");
        assert!(
            errs.iter()
                .any(|d| d.msg.contains("unexpected token in interface body")),
            "{errs:?}"
        );
    }

    #[test]
    fn stalled_foreign_body_names_its_context() {
        let errs = parse_errs("foreign \"C\" { + }");
        assert!(
            errs.iter()
                .any(|d| d.msg.contains("unexpected token in foreign block")),
            "{errs:?}"
        );
    }

    #[test]
    fn stalled_module_body_names_its_context() {
        let errs = parse_errs("123 456");
        assert!(
            errs.iter()
                .any(|d| d.msg.contains("unexpected token in module body")),
            "{errs:?}"
        );
    }

    // A deeply nested generic is a valid, terminating parse: the invariant does
    // not touch it, and it must stay linear (no per-level rescans). This is the
    // regression twin for the recovery-loop guards.
    #[test]
    fn deep_nested_generic_parses() {
        let n = 400;
        let src = format!(
            "func f() -> {}int{} {{ return }}",
            "Vec<".repeat(n),
            ">".repeat(n)
        );
        let (toks, _) = lex(&src);
        let (_m, errs) = parse(toks);
        assert!(errs.is_empty(), "valid nested generic rejected: {errs:?}");
    }

    // Recursion-depth ceiling. A pathological deeply nested input once overflowed
    // the stack (SIGABRT); `enter_nesting` now unwinds it into a diagnostic. The
    // depth cases below intentionally drive the recursion to the ceiling, which
    // needs more stack than the small default test-harness thread offers, so they
    // run on an 8 MiB worker matching the process main thread. The ceiling keeps
    // the depth bounded, so that worker never overflows.
    fn parse_big_stack(src: &str) -> Vec<Diagnostic> {
        let owned = src.to_string();
        thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let (toks, _lex) = lex(&owned);
                let (_m, errs) = parse(toks);
                errs
            })
            .expect("spawn parser worker")
            .join()
            .expect("parser worker overflowed or panicked")
    }

    #[test]
    fn deep_paren_expression_is_rejected_not_crashed() {
        // Twenty thousand open parens used to overflow the stack; the depth guard
        // now unwinds them into a single diagnostic that names the fix.
        let src = format!("func f() -> void {{ x := {}1 }}", "(".repeat(20_000));
        let errs = parse_big_stack(&src);
        assert!(
            errs.iter()
                .any(|d| d.msg == "expression nesting is too deep; simplify the expression"),
            "deep parens not reported as too deep: {errs:?}"
        );
    }

    #[test]
    fn deep_type_is_rejected_not_crashed() {
        // Deeply nested generics recurse through `type_`; the ceiling stops them.
        let src = format!("func f() -> {}int {{ return }}", "Vec<".repeat(5_000));
        let errs = parse_big_stack(&src);
        assert!(
            errs.iter()
                .any(|d| d.msg == "type nesting is too deep; simplify the type"),
            "deep type not reported as too deep: {errs:?}"
        );
    }

    #[test]
    fn deep_right_assoc_operator_chain_is_rejected() {
        // A long right-associative `**` run recurses through `bin`, not the prefix
        // or paren paths, so this proves the binary layer is depth-guarded too.
        let src = format!("func f() -> int64 {{ return {}2 }}", "2 ** ".repeat(20_000));
        let errs = parse_big_stack(&src);
        assert!(
            errs.iter()
                .any(|d| d.msg == "expression nesting is too deep; simplify the expression"),
            "deep operator chain not reported as too deep: {errs:?}"
        );
    }

    #[test]
    fn shallow_nesting_is_still_accepted() {
        // Ten levels of parens, far under the ceiling, parse clean: the guard does
        // not perturb ordinary nested code.
        let src = format!(
            "func f() -> int64 {{ return {}1{} }}",
            "(".repeat(10),
            ")".repeat(10)
        );
        let (toks, _) = lex(&src);
        let (_m, errs) = parse(toks);
        assert!(
            errs.is_empty(),
            "shallow nesting spuriously rejected: {errs:?}"
        );
    }
}
