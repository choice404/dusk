//! Private name isolation for imported modules.
//!
//! The loader merges every imported module's items into one flat program, which
//! would let a bare call reach another file's private helper and would collide
//! two modules that each define a private `helper`. Before the merge, this pass
//! renames each imported module's non exported top level items with a per file
//! suffix and rewrites the references inside that module to match. The root
//! module is never renamed, exported names are never renamed, and foreign
//! functions are never renamed, since they bind to a C symbol by name.
//!
//! Reference rewriting is scope aware: a local binding, parameter, pattern
//! binding, or generic parameter shadows a renamed global, so uses of the local
//! keep their name.

use std::collections::{HashMap, HashSet};

use crate::parser::ast::*;

/// Renames the private top level items of a parsed module with `suffix` and
/// rewrites every reference inside the module to the new names.
pub fn privatize(m: &mut Module, suffix: &str) {
    let mut map: HashMap<String, String> = HashMap::new();
    for it in &m.items {
        let (exported, name) = match it {
            Item::Func(f) => (f.exported, &f.name),
            Item::Struct(s) => (s.exported, &s.name),
            Item::Enum(e) => (e.exported, &e.name),
            Item::Interface(i) => (i.exported, &i.name),
            Item::Impl(_) | Item::Foreign(_) => continue,
        };
        if !exported {
            map.insert(name.clone(), format!("{name}__{suffix}"));
        }
    }
    if map.is_empty() {
        return;
    }
    let r = Renamer { map };
    for it in &mut m.items {
        r.item(it);
    }
}

/// A filesystem safe suffix for a module, from its file stem and its position in
/// the load order. The index keeps two same named files from different
/// directories distinct.
pub fn suffix_for(path: &str, index: usize) -> String {
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("mod");
    let clean: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{clean}_{index}")
}

struct Renamer {
    map: HashMap<String, String>,
}

impl Renamer {
    fn renamed(&self, n: &str) -> Option<String> {
        self.map.get(n).cloned()
    }

    fn item(&self, it: &mut Item) {
        match it {
            Item::Func(f) => {
                if let Some(nn) = self.renamed(&f.name) {
                    f.name = nn;
                }
                self.func(f);
            }
            Item::Struct(s) => {
                if let Some(nn) = self.renamed(&s.name) {
                    s.name = nn;
                }
                let skip: HashSet<String> = s.generics.iter().cloned().collect();
                for fl in &mut s.fields {
                    self.ty(&mut fl.ty, &skip);
                }
            }
            Item::Enum(e) => {
                if let Some(nn) = self.renamed(&e.name) {
                    e.name = nn;
                }
                let skip: HashSet<String> = e.generics.iter().cloned().collect();
                for v in &mut e.variants {
                    for fl in &mut v.fields {
                        self.ty(&mut fl.ty, &skip);
                    }
                }
            }
            Item::Interface(i) => {
                if let Some(nn) = self.renamed(&i.name) {
                    i.name = nn;
                }
                let skip: HashSet<String> = i.generics.iter().cloned().collect();
                for m in &mut i.methods {
                    for p in &mut m.params {
                        self.ty(&mut p.ty, &skip);
                    }
                    self.ty(&mut m.ret, &skip);
                }
            }
            Item::Impl(im) => {
                if let Some(nn) = self.renamed(&im.ty) {
                    im.ty = nn;
                }
                if let Some(iface) = &im.iface {
                    if let Some(nn) = self.renamed(iface) {
                        im.iface = Some(nn);
                    }
                }
                for m in &mut im.methods {
                    self.func(m);
                }
            }
            // Foreign names bind to C symbols; renaming one would unlink it.
            Item::Foreign(_) => {}
        }
    }

    fn func(&self, f: &mut Func) {
        let skip: HashSet<String> = f.generics.iter().cloned().collect();
        let mut bound: Vec<HashSet<String>> = vec![HashSet::new()];
        for p in &mut f.params {
            self.ty(&mut p.ty, &skip);
            bound.last_mut().unwrap().insert(p.name.clone());
        }
        self.ty(&mut f.ret, &skip);
        self.block(&mut f.body, &skip, &mut bound);
    }

    fn ty(&self, t: &mut Type, skip: &HashSet<String>) {
        match t {
            Type::Named(n, args) => {
                if !skip.contains(n) {
                    if let Some(nn) = self.renamed(n) {
                        *n = nn;
                    }
                }
                for a in args {
                    self.ty(a, skip);
                }
            }
            Type::Ptr(b) | Type::RawPtr(b) | Type::Slice(b) | Type::Array(b, _) => {
                self.ty(b, skip)
            }
            Type::Tuple(ts) => {
                for x in ts {
                    self.ty(x, skip);
                }
            }
            Type::Func(ps, r) => {
                for p in ps {
                    self.ty(p, skip);
                }
                self.ty(r, skip);
            }
            Type::Collector(b) => self.ty(b, skip),
            Type::Unit => {}
            Type::Infer => {}
        }
    }

    fn block(&self, b: &mut Block, skip: &HashSet<String>, bound: &mut Vec<HashSet<String>>) {
        bound.push(HashSet::new());
        for s in &mut b.stmts {
            self.stmt(s, skip, bound);
        }
        bound.pop();
    }

    fn stmt(&self, s: &mut Stmt, skip: &HashSet<String>, bound: &mut Vec<HashSet<String>>) {
        match s {
            Stmt::Let(l) => {
                self.expr(&mut l.value, skip, bound);
                for bind in &mut l.binds {
                    if let Some(t) = &mut bind.ty {
                        self.ty(t, skip);
                    }
                    bound.last_mut().unwrap().insert(bind.name.clone());
                }
            }
            Stmt::Assign(a, b) => {
                self.expr(a, skip, bound);
                self.expr(b, skip, bound);
            }
            Stmt::AssignOp(_, a, b) => {
                self.expr(a, skip, bound);
                self.expr(b, skip, bound);
            }
            Stmt::Return(Some(e)) | Stmt::Defer(e) | Stmt::Expr(e) => self.expr(e, skip, bound),
            Stmt::Return(None) => {}
            Stmt::If(i) => {
                self.expr(&mut i.cond, skip, bound);
                self.block(&mut i.then, skip, bound);
                if let Some(e) = &mut i.els {
                    self.block(e, skip, bound);
                }
            }
            Stmt::While(w) => {
                self.expr(&mut w.cond, skip, bound);
                self.block(&mut w.body, skip, bound);
            }
            Stmt::For(f) => {
                self.expr(&mut f.iter, skip, bound);
                bound.push(HashSet::new());
                bound.last_mut().unwrap().insert(f.var.clone());
                self.block(&mut f.body, skip, bound);
                bound.pop();
            }
            Stmt::Match(m) => self.match_(m, skip, bound),
        }
    }

    fn match_(&self, m: &mut Match, skip: &HashSet<String>, bound: &mut Vec<HashSet<String>>) {
        self.expr(&mut m.scrut, skip, bound);
        for arm in &mut m.arms {
            bound.push(HashSet::new());
            match &arm.pat {
                Pattern::Variant(_, binds) => {
                    for b in binds {
                        bound.last_mut().unwrap().insert(b.clone());
                    }
                }
                Pattern::Ident(n) => {
                    bound.last_mut().unwrap().insert(n.clone());
                }
                Pattern::Wildcard => {}
            }
            self.block(&mut arm.body, skip, bound);
            bound.pop();
        }
    }

    fn is_bound(&self, n: &str, bound: &[HashSet<String>]) -> bool {
        bound.iter().any(|s| s.contains(n))
    }

    fn expr(&self, e: &mut Expr, skip: &HashSet<String>, bound: &mut Vec<HashSet<String>>) {
        match &mut e.kind {
            ExprKind::Ident(n) => {
                if !self.is_bound(n, bound) {
                    if let Some(nn) = self.renamed(n) {
                        *n = nn;
                    }
                }
            }
            ExprKind::Unary(_, x) => self.expr(x, skip, bound),
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b, _) => {
                self.expr(a, skip, bound);
                self.expr(b, skip, bound);
            }
            ExprKind::Call(f, args) => {
                self.expr(f, skip, bound);
                for a in args {
                    self.expr(a, skip, bound);
                }
            }
            ExprKind::Field(x, _) => self.expr(x, skip, bound),
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
                for x in xs {
                    self.expr(x, skip, bound);
                }
            }
            ExprKind::StructLit(name, fields) => {
                if let Some(nn) = self.renamed(name) {
                    *name = nn;
                }
                for (_, v) in fields {
                    self.expr(v, skip, bound);
                }
            }
            ExprKind::Lambda(l) => {
                bound.push(HashSet::new());
                for p in &mut l.params {
                    self.ty(&mut p.ty, skip);
                    bound.last_mut().unwrap().insert(p.name.clone());
                }
                self.ty(&mut l.ret, skip);
                self.block(&mut l.body, skip, bound);
                bound.pop();
            }
            ExprKind::Match(m) => self.match_(m, skip, bound),
            ExprKind::Await(op, ty) => {
                self.expr(op, skip, bound);
                if let Some(t) = ty {
                    self.ty(t, skip);
                }
            }
            ExprKind::Do(_, binds) => {
                // A do bind name scopes over the rest of the block; the coarse
                // approximation binds them all up front, which only over shadows.
                let names: Vec<String> = binds.iter().filter_map(|b| b.name.clone()).collect();
                bound.push(names.into_iter().collect());
                for b in binds {
                    self.expr(&mut b.expr, skip, bound);
                }
                bound.pop();
            }
            ExprKind::SizeofType(t) => self.ty(t, skip),
            ExprKind::Collect { ty, arg } => {
                self.ty(ty, skip);
                self.expr(arg, skip, bound);
            }
            ExprKind::Int(..)
            | ExprKind::Float(..)
            | ExprKind::Str(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn module(src: &str) -> Module {
        let (t, le) = lex(src);
        assert!(le.is_empty(), "lex errors: {le:?}");
        let (m, pe) = parse(t);
        assert!(pe.is_empty(), "parse errors: {pe:?}");
        m
    }

    fn names(m: &Module) -> Vec<String> {
        m.items
            .iter()
            .filter_map(|it| match it {
                Item::Func(f) => Some(f.name.clone()),
                Item::Struct(s) => Some(s.name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn private_func_renamed_and_call_follows() {
        let mut m = module(
            "func helper() -> int64 { return 1 }\n\
             export func visible() -> int64 { return helper() }",
        );
        privatize(&mut m, "lib_1");
        let n = names(&m);
        assert!(n.contains(&"helper__lib_1".to_string()), "{n:?}");
        assert!(n.contains(&"visible".to_string()), "exported name must not change: {n:?}");
        let Item::Func(v) = &m.items[1] else { panic!() };
        let Stmt::Return(Some(e)) = &v.body.stmts[0] else { panic!() };
        let ExprKind::Call(f, _) = &e.kind else { panic!() };
        assert!(
            matches!(&f.kind, ExprKind::Ident(n) if n == "helper__lib_1"),
            "call must follow the rename: {:?}",
            f.kind
        );
    }

    #[test]
    fn local_shadow_is_not_renamed() {
        let mut m = module(
            "func helper() -> int64 { return 1 }\n\
             export func f() -> int64 {\n  helper := 5\n  return helper\n}",
        );
        privatize(&mut m, "x_1");
        let Item::Func(f) = &m.items[1] else { panic!() };
        let Stmt::Return(Some(e)) = &f.body.stmts[1] else { panic!() };
        assert!(
            matches!(&e.kind, ExprKind::Ident(n) if n == "helper"),
            "the local shadows the renamed global: {:?}",
            e.kind
        );
    }

    #[test]
    fn private_struct_rename_covers_types_literals_and_impls() {
        let mut m = module(
            "struct P { x: int64 }\n\
             export func mk() -> P {\n  return P { x: 1 }\n}",
        );
        privatize(&mut m, "s_2");
        let Item::Struct(s) = &m.items[0] else { panic!() };
        assert_eq!(s.name, "P__s_2");
        let Item::Func(f) = &m.items[1] else { panic!() };
        assert!(matches!(&f.ret, Type::Named(n, _) if n == "P__s_2"), "{:?}", f.ret);
        let Stmt::Return(Some(e)) = &f.body.stmts[0] else { panic!() };
        assert!(matches!(&e.kind, ExprKind::StructLit(n, _) if n == "P__s_2"));
    }

    #[test]
    fn foreign_names_are_never_renamed() {
        let mut m = module(
            "foreign \"C\" { func abs(n: int32) -> int32 }\n\
             export func f() -> int32 { return abs(-1) }",
        );
        privatize(&mut m, "ffi_1");
        let Item::Foreign(fb) = &m.items[0] else { panic!() };
        assert_eq!(fb.funcs[0].name, "abs");
        let Item::Func(f) = &m.items[1] else { panic!() };
        let Stmt::Return(Some(e)) = &f.body.stmts[0] else { panic!() };
        let ExprKind::Call(callee, _) = &e.kind else { panic!() };
        assert!(matches!(&callee.kind, ExprKind::Ident(n) if n == "abs"));
    }
}
