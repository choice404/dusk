//! Monomorphization. Expands generic functions, structs, and enums into concrete
//! copies, one per distinct set of type arguments. Runs after sema and before
//! codegen, so codegen only ever sees ground types.
//!
//! Type arguments are taken from explicit annotations where present and inferred
//! from value argument types otherwise. Each instantiation is mangled with a `$`
//! separated suffix and added to a worklist; expanding one may request others.

use std::collections::{HashMap, HashSet};

use crate::diag::Span;
use crate::parser::ast::{
    Bind, Block, Enum, Expr, ExprKind, Field, Func, Item, Let, Module, Param, Stmt, Struct, Type,
    Variant,
};

type Subst = HashMap<String, Type>;
type Env = HashMap<String, Type>;

/// Expands all generics in a module into concrete monomorphic items.
pub fn expand(module: &Module) -> Module {
    let mut m = Mono::new(module);
    let items = m.run();
    Module {
        paradigms: module.paradigms.clone(),
        imports: module.imports.clone(),
        items,
    }
}

struct Mono<'a> {
    items: &'a [Item],
    gfuncs: HashMap<String, &'a Func>,
    gstructs: HashMap<String, &'a Struct>,
    genums: HashMap<String, &'a Enum>,
    fn_ret: HashMap<String, Type>,
    requested: HashSet<String>,
    worklist: Vec<(String, Vec<Type>)>,
    out: Vec<Item>,
}

impl<'a> Mono<'a> {
    fn new(module: &'a Module) -> Self {
        let mut gfuncs = HashMap::new();
        let mut gstructs = HashMap::new();
        let mut genums = HashMap::new();
        let mut fn_ret = HashMap::new();
        for item in &module.items {
            match item {
                Item::Func(f) if !f.generics.is_empty() => {
                    gfuncs.insert(f.name.clone(), f);
                }
                Item::Func(f) => {
                    fn_ret.insert(f.name.clone(), f.ret.clone());
                }
                Item::Struct(s) if !s.generics.is_empty() => {
                    gstructs.insert(s.name.clone(), s);
                }
                Item::Enum(e) if !e.generics.is_empty() => {
                    genums.insert(e.name.clone(), e);
                }
                _ => {}
            }
        }
        Mono {
            items: &module.items,
            gfuncs,
            gstructs,
            genums,
            fn_ret,
            requested: HashSet::new(),
            worklist: Vec::new(),
            out: Vec::new(),
        }
    }

    fn run(&mut self) -> Vec<Item> {
        let items = self.items;
        for item in items {
            self.rewrite_item(item);
        }
        while let Some((name, args)) = self.worklist.pop() {
            self.expand_instance(&name, &args);
        }
        std::mem::take(&mut self.out)
    }

    fn is_generic(&self, name: &str) -> bool {
        self.gstructs.contains_key(name) || self.genums.contains_key(name)
    }


    fn enqueue(&mut self, name: &str, args: &[Type]) {
        let m = mangle(name, args);
        // Guard against runaway polymorphic recursion, where each instantiation
        // requests a strictly larger one and the worklist never drains. A real
        // program's mangled names stay short; an unbounded one grows without
        // limit, so a length ceiling stops the divergence cheaply.
        if m.len() > 1024 {
            return;
        }
        if self.requested.insert(m) {
            self.worklist.push((name.to_string(), args.to_vec()));
        }
    }

    /// Requests an instantiation from an expression site and returns its mangled
    /// name. Type arguments are lowered through `emit_ty` first so a nested
    /// generic argument mangles the same way it would from a type annotation,
    /// keeping the construction site and the emitted definition in agreement.
    fn instantiate(&mut self, name: &str, args: &[Type]) -> String {
        let cargs: Vec<Type> = args.iter().map(|a| self.emit_ty(a)).collect();
        self.enqueue(name, &cargs);
        mangle(name, &cargs)
    }

    fn expand_instance(&mut self, name: &str, args: &[Type]) {
        let mangled = mangle(name, args);
        if let Some(f) = self.gfuncs.get(name).copied() {
            let subst = bind(&f.generics, args);
            let mono = self.rw_func(f, &subst, Some(mangled));
            self.out.push(Item::Func(mono));
        } else if let Some(s) = self.gstructs.get(name).copied() {
            let subst = bind(&s.generics, args);
            let fields = s
                .fields
                .iter()
                .map(|fl| Field {
                    name: fl.name.clone(),
                    ty: self.emit_field_ty(&fl.ty, &subst),
                })
                .collect();
            self.out.push(Item::Struct(Struct {
                exported: s.exported,
                name: mangled,
                generics: Vec::new(),
                fields,
            }));
        } else if let Some(e) = self.genums.get(name).copied() {
            let subst = bind(&e.generics, args);
            let variants = e
                .variants
                .iter()
                .map(|v| Variant {
                    name: v.name.clone(),
                    fields: v
                        .fields
                        .iter()
                        .map(|fl| Field {
                            name: fl.name.clone(),
                            ty: self.emit_field_ty(&fl.ty, &subst),
                        })
                        .collect(),
                })
                .collect();
            self.out.push(Item::Enum(Enum {
                exported: e.exported,
                name: mangled,
                generics: Vec::new(),
                variants,
            }));
        }
    }

    fn emit_field_ty(&mut self, ty: &Type, subst: &Subst) -> Type {
        let applied = subst_apply(ty, subst);
        self.emit_ty(&applied)
    }


    /// Replaces type parameters with their bindings without mangling.
    // (free function `subst_apply` is used; this stays a thin wrapper site.)

    /// Mangles ground generic references and requests their instantiation.
    fn emit_ty(&mut self, ty: &Type) -> Type {
        match ty {
            Type::Named(n, args) if !args.is_empty() => {
                let cargs: Vec<Type> = args.iter().map(|a| self.emit_ty(a)).collect();
                if self.is_generic(n) {
                    self.enqueue(n, &cargs);
                    Type::Named(mangle(n, &cargs), Vec::new())
                } else {
                    Type::Named(n.clone(), cargs)
                }
            }
            Type::Named(n, _) => Type::Named(n.clone(), Vec::new()),
            Type::Ptr(b) => Type::Ptr(Box::new(self.emit_ty(b))),
            Type::RawPtr(b) => Type::RawPtr(Box::new(self.emit_ty(b))),
            Type::Slice(b) => Type::Slice(Box::new(self.emit_ty(b))),
            Type::Array(b, n) => Type::Array(Box::new(self.emit_ty(b)), *n),
            Type::Tuple(xs) => Type::Tuple(xs.iter().map(|x| self.emit_ty(x)).collect()),
            Type::Func(ps, r) => Type::Func(
                ps.iter().map(|p| self.emit_ty(p)).collect(),
                Box::new(self.emit_ty(r)),
            ),
            Type::Unit => Type::Unit,
        }
    }


    fn rewrite_item(&mut self, item: &Item) {
        match item {
            Item::Func(f) if f.generics.is_empty() => {
                let mono = self.rw_func(f, &Subst::new(), None);
                self.out.push(Item::Func(mono));
            }
            Item::Struct(s) if s.generics.is_empty() => {
                let fields = s
                    .fields
                    .iter()
                    .map(|fl| Field {
                        name: fl.name.clone(),
                        ty: self.emit_field_ty(&fl.ty, &Subst::new()),
                    })
                    .collect();
                self.out.push(Item::Struct(Struct {
                    exported: s.exported,
                    name: s.name.clone(),
                    generics: Vec::new(),
                    fields,
                }));
            }
            Item::Enum(e) if e.generics.is_empty() => {
                let variants = e
                    .variants
                    .iter()
                    .map(|v| Variant {
                        name: v.name.clone(),
                        fields: v
                            .fields
                            .iter()
                            .map(|fl| Field {
                                name: fl.name.clone(),
                                ty: self.emit_field_ty(&fl.ty, &Subst::new()),
                            })
                            .collect(),
                    })
                    .collect();
                self.out.push(Item::Enum(Enum {
                    exported: e.exported,
                    name: e.name.clone(),
                    generics: Vec::new(),
                    variants,
                }));
            }
            Item::Impl(im) if !self.is_generic(&im.ty) => {
                let methods = im
                    .methods
                    .iter()
                    .map(|mth| self.rw_func(mth, &Subst::new(), None))
                    .collect();
                self.out.push(Item::Impl(crate::parser::ast::Impl {
                    iface: im.iface.clone(),
                    ty: im.ty.clone(),
                    methods,
                }));
            }
            Item::Interface(i) => self.out.push(Item::Interface(i.clone())),
            _ => {}
        }
    }

    fn rw_func(&mut self, f: &Func, subst: &Subst, mangled: Option<String>) -> Func {
        let name = mangled.unwrap_or_else(|| f.name.clone());
        let mut env = Env::new();
        let mut params = Vec::with_capacity(f.params.len());
        for p in &f.params {
            let applied = subst_apply(&p.ty, subst);
            env.insert(p.name.clone(), applied.clone());
            params.push(Param {
                using: p.using,
                name: p.name.clone(),
                ty: self.emit_ty(&applied),
            });
        }
        let ret_applied = subst_apply(&f.ret, subst);
        let ret = self.emit_ty(&ret_applied);
        let body = self.rw_block(&f.body, subst, &mut env, &ret_applied);
        Func {
            exported: f.exported,
            name,
            generics: Vec::new(),
            params,
            ret,
            body,
        }
    }

    fn rw_block(&mut self, b: &Block, subst: &Subst, env: &mut Env, ret: &Type) -> Block {
        let mut stmts = Vec::with_capacity(b.stmts.len());
        for s in &b.stmts {
            stmts.push(self.rw_stmt(s, subst, env, ret));
        }
        Block { stmts }
    }

    fn rw_stmt(&mut self, s: &Stmt, subst: &Subst, env: &mut Env, ret: &Type) -> Stmt {
        match s {
            Stmt::Let(l) => {
                let exp = l
                    .binds
                    .first()
                    .and_then(|b| b.ty.as_ref())
                    .map(|t| subst_apply(t, subst));
                let value = self.rw_expr(&l.value, subst, env, exp.as_ref());
                let vt = exp
                    .clone()
                    .or_else(|| self.static_ty(&l.value, subst, env));
                let mut binds = Vec::with_capacity(l.binds.len());
                for b in &l.binds {
                    let ty = b.ty.as_ref().map(|t| {
                        let a = subst_apply(t, subst);
                        self.emit_ty(&a)
                    });
                    if let Some(t) = &vt {
                        env.insert(b.name.clone(), t.clone());
                    }
                    binds.push(Bind {
                        name: b.name.clone(),
                        ty,
                    });
                }
                Stmt::Let(Let {
                    mutable: l.mutable,
                    infer: l.infer,
                    binds,
                    value,
                })
            }
            Stmt::Assign(lhs, rhs) => Stmt::Assign(
                self.rw_expr(lhs, subst, env, None),
                self.rw_expr(rhs, subst, env, None),
            ),
            Stmt::Return(Some(e)) => Stmt::Return(Some(self.rw_expr(e, subst, env, Some(ret)))),
            Stmt::Return(None) => Stmt::Return(None),
            Stmt::Defer(e) => Stmt::Defer(self.rw_expr(e, subst, env, None)),
            Stmt::If(i) => {
                let cond = self.rw_expr(&i.cond, subst, env, None);
                let then = self.rw_block(&i.then, subst, &mut env.clone(), ret);
                let els = i
                    .els
                    .as_ref()
                    .map(|b| self.rw_block(b, subst, &mut env.clone(), ret));
                Stmt::If(crate::parser::ast::If { cond, then, els })
            }
            Stmt::While(w) => {
                let cond = self.rw_expr(&w.cond, subst, env, None);
                let body = self.rw_block(&w.body, subst, &mut env.clone(), ret);
                Stmt::While(crate::parser::ast::While {
                    cond,
                    body,
                    post_test: w.post_test,
                })
            }
            Stmt::Match(m) => Stmt::Match(self.rw_match(m, subst, env, ret)),
            Stmt::Expr(e) => Stmt::Expr(self.rw_expr(e, subst, env, None)),
            Stmt::For(f) => Stmt::For(crate::parser::ast::For {
                var: f.var.clone(),
                iter: self.rw_expr(&f.iter, subst, env, None),
                body: self.rw_block(&f.body, subst, &mut env.clone(), ret),
            }),
        }
    }

    fn rw_match(
        &mut self,
        m: &crate::parser::ast::Match,
        subst: &Subst,
        env: &Env,
        ret: &Type,
    ) -> crate::parser::ast::Match {
        let scrut = Box::new(self.rw_expr(&m.scrut, subst, env, None));
        let scrut_ty = self.static_ty(&m.scrut, subst, env);
        let arms = m
            .arms
            .iter()
            .map(|arm| {
                let mut e2 = env.clone();
                self.bind_pattern(&arm.pat, scrut_ty.as_ref(), subst, &mut e2);
                let body = self.rw_block(&arm.body, subst, &mut e2, ret);
                crate::parser::ast::Arm {
                    pat: arm.pat.clone(),
                    body,
                }
            })
            .collect();
        crate::parser::ast::Match { scrut, arms }
    }

    /// Types the variables a match arm pattern introduces and inserts them into
    /// the arm's env, so generic inference in the arm body sees real payload
    /// types instead of falling through to the int64 default.
    fn bind_pattern(
        &self,
        pat: &crate::parser::ast::Pattern,
        scrut_ty: Option<&Type>,
        subst: &Subst,
        env: &mut Env,
    ) {
        use crate::parser::ast::Pattern;
        match pat {
            Pattern::Wildcard => {}
            Pattern::Ident(name) => {
                if let Some(t) = scrut_ty {
                    env.insert(name.clone(), subst_apply(t, subst));
                }
            }
            Pattern::Variant(variant, binds) => {
                if let Some(Type::Named(g, eargs)) = scrut_ty {
                    if let Some(ge) = self.genums.get(g.as_str()) {
                        let vsubst = bind(&ge.generics, eargs);
                        if let Some(var) = ge.variants.iter().find(|v| &v.name == variant) {
                            for (b, fld) in binds.iter().zip(&var.fields) {
                                let ft = subst_apply(&subst_apply(&fld.ty, &vsubst), subst);
                                env.insert(b.clone(), ft);
                            }
                        }
                    }
                }
            }
        }
    }

    fn rw_expr(&mut self, e: &Expr, subst: &Subst, env: &Env, expected: Option<&Type>) -> Expr {
        let kind = match &e.kind {
            ExprKind::Call(callee, args) => self.rw_call(callee, args, subst, env, expected),
            ExprKind::StructLit(name, fields) => {
                self.rw_struct_lit(name, fields, subst, env, expected)
            }
            ExprKind::Field(base, name) => {
                if let ExprKind::Ident(g) = &base.kind {
                    if self.genums.contains_key(g) && self.enum_has_variant(g, name) {
                        let targs = self.enum_args(g, expected, &[], subst, env, name);
                        let mg = node(ExprKind::Ident(self.instantiate(g, &targs)), base.span);
                        return node(ExprKind::Field(Box::new(mg), name.clone()), e.span);
                    }
                }
                ExprKind::Field(Box::new(self.rw_expr(base, subst, env, None)), name.clone())
            }
            ExprKind::Unary(op, x) => {
                ExprKind::Unary(*op, Box::new(self.rw_expr(x, subst, env, None)))
            }
            ExprKind::Binary(op, a, b) => ExprKind::Binary(
                *op,
                Box::new(self.rw_expr(a, subst, env, None)),
                Box::new(self.rw_expr(b, subst, env, None)),
            ),
            ExprKind::Index(a, b) => ExprKind::Index(
                Box::new(self.rw_expr(a, subst, env, None)),
                Box::new(self.rw_expr(b, subst, env, None)),
            ),
            ExprKind::Range(a, b) => ExprKind::Range(
                Box::new(self.rw_expr(a, subst, env, None)),
                Box::new(self.rw_expr(b, subst, env, None)),
            ),
            ExprKind::Tuple(xs) => {
                ExprKind::Tuple(xs.iter().map(|x| self.rw_expr(x, subst, env, None)).collect())
            }
            ExprKind::Array(xs) => {
                ExprKind::Array(xs.iter().map(|x| self.rw_expr(x, subst, env, None)).collect())
            }
            ExprKind::Lambda(l) => {
                let mut e2 = env.clone();
                for p in &l.params {
                    e2.insert(p.name.clone(), subst_apply(&p.ty, subst));
                }
                let ret = subst_apply(&l.ret, subst);
                let body = self.rw_block(&l.body, subst, &mut e2, &ret);
                ExprKind::Lambda(crate::parser::ast::Lambda {
                    params: l
                        .params
                        .iter()
                        .map(|p| Param {
                            using: p.using,
                            name: p.name.clone(),
                            ty: self.emit_ty(&subst_apply(&p.ty, subst)),
                        })
                        .collect(),
                    ret: self.emit_ty(&ret),
                    body,
                })
            }
            ExprKind::Match(m) => {
                let _ = expected;
                ExprKind::Match(Box::new(self.rw_match(m, subst, env, &Type::Unit)))
            }
            other => other.clone(),
        };
        node(kind, e.span)
    }

    fn rw_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        subst: &Subst,
        env: &Env,
        expected: Option<&Type>,
    ) -> ExprKind {
        if let ExprKind::Field(base, v) = &callee.kind {
            if let ExprKind::Ident(g) = &base.kind {
                if self.genums.contains_key(g) && self.enum_has_variant(g, v) {
                    let cargs: Vec<Expr> =
                        args.iter().map(|a| self.rw_expr(a, subst, env, None)).collect();
                    let targs = self.enum_args(g, expected, args, subst, env, v);
                    let mg = node(ExprKind::Ident(self.instantiate(g, &targs)), base.span);
                    let nc = node(ExprKind::Field(Box::new(mg), v.clone()), callee.span);
                    return ExprKind::Call(Box::new(nc), cargs);
                }
            }
        }
        if let ExprKind::Ident(f) = &callee.kind {
            if f == "sizeof" && args.len() == 1 {
                if let ExprKind::Ident(g) = &args[0].kind {
                    // Resolve a type parameter argument to its concrete type and
                    // emit a typed sizeof, so any concrete type, including a slice
                    // or tuple, is sized correctly in generic code.
                    if subst.contains_key(g) {
                        let ty = subst_apply(&Type::Named(g.clone(), Vec::new()), subst);
                        return ExprKind::SizeofType(self.emit_ty(&ty));
                    }
                }
            }
            if let Some(gf) = self.gfuncs.get(f).copied() {
                let cargs: Vec<Expr> =
                    args.iter().map(|a| self.rw_expr(a, subst, env, None)).collect();
                let targs = self.infer_fn_args(gf, args, subst, env, expected);
                let mg = node(ExprKind::Ident(self.instantiate(f, &targs)), callee.span);
                return ExprKind::Call(Box::new(mg), cargs);
            }
            if f == "alloc" && args.len() == 1 {
                let inner = match expected {
                    Some(Type::Ptr(b)) => Some((**b).clone()),
                    _ => None,
                };
                let a = self.rw_expr(&args[0], subst, env, inner.as_ref());
                let c = self.rw_expr(callee, subst, env, None);
                return ExprKind::Call(Box::new(c), vec![a]);
            }
        }
        let c = self.rw_expr(callee, subst, env, None);
        let a = args.iter().map(|x| self.rw_expr(x, subst, env, None)).collect();
        ExprKind::Call(Box::new(c), a)
    }

    fn rw_struct_lit(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
        subst: &Subst,
        env: &Env,
        expected: Option<&Type>,
    ) -> ExprKind {
        let new_fields: Vec<(String, Expr)> = fields
            .iter()
            .map(|(n, v)| (n.clone(), self.rw_expr(v, subst, env, None)))
            .collect();
        if let Some(gs) = self.gstructs.get(name).copied() {
            let targs = self.infer_struct_args(gs, fields, expected, subst, env);
            return ExprKind::StructLit(self.instantiate(name, &targs), new_fields);
        }
        ExprKind::StructLit(name.to_string(), new_fields)
    }


    fn infer_fn_args(
        &self,
        gf: &Func,
        args: &[Expr],
        subst: &Subst,
        env: &Env,
        expected: Option<&Type>,
    ) -> Vec<Type> {
        let params: HashSet<String> = gf.generics.iter().cloned().collect();
        let mut inf = Subst::new();
        for (i, p) in gf.params.iter().enumerate() {
            if let Some(a) = args.get(i) {
                if let Some(at) = self.static_ty(a, subst, env) {
                    unify(&p.ty, &at, &params, &mut inf);
                }
            }
        }
        // Push down the expected (annotation) type so params that appear only in
        // the return position get inferred instead of silently defaulting.
        if let Some(et) = expected {
            unify(&gf.ret, et, &params, &mut inf);
        }
        solve(&gf.generics, &inf)
    }

    fn infer_struct_args(
        &self,
        gs: &Struct,
        fields: &[(String, Expr)],
        expected: Option<&Type>,
        subst: &Subst,
        env: &Env,
    ) -> Vec<Type> {
        if let Some(Type::Named(en, eargs)) = expected {
            if en == &gs.name && !eargs.is_empty() {
                return eargs.iter().map(|t| subst_apply(t, subst)).collect();
            }
        }
        let params: HashSet<String> = gs.generics.iter().cloned().collect();
        let mut inf = Subst::new();
        for (n, v) in fields {
            if let Some(decl) = gs.fields.iter().find(|f| &f.name == n) {
                if let Some(vt) = self.static_ty(v, subst, env) {
                    unify(&decl.ty, &vt, &params, &mut inf);
                }
            }
        }
        solve(&gs.generics, &inf)
    }

    fn enum_args(
        &self,
        g: &str,
        expected: Option<&Type>,
        payload: &[Expr],
        subst: &Subst,
        env: &Env,
        variant: &str,
    ) -> Vec<Type> {
        let ge = self.genums[g];
        if let Some(Type::Named(en, eargs)) = expected {
            if en == g && !eargs.is_empty() {
                return eargs.iter().map(|t| subst_apply(t, subst)).collect();
            }
        }
        let params: HashSet<String> = ge.generics.iter().cloned().collect();
        let mut inf = Subst::new();
        if let Some(var) = ge.variants.iter().find(|v| v.name == variant) {
            for (i, fld) in var.fields.iter().enumerate() {
                if let Some(a) = payload.get(i) {
                    if let Some(at) = self.static_ty(a, subst, env) {
                        unify(&fld.ty, &at, &params, &mut inf);
                    }
                }
            }
        }
        solve(&ge.generics, &inf)
    }

    fn enum_has_variant(&self, g: &str, v: &str) -> bool {
        self.genums
            .get(g)
            .map(|e| e.variants.iter().any(|x| x.name == v))
            .unwrap_or(false)
    }


    fn static_ty(&self, e: &Expr, subst: &Subst, env: &Env) -> Option<Type> {
        match &e.kind {
            ExprKind::Int(_, s) => Some(named(int_lit_ty(s))),
            ExprKind::Float(..) => Some(named("float64")),
            ExprKind::Bool(_) => Some(named("bool")),
            ExprKind::Char(_) => Some(named("char")),
            ExprKind::Str(_) => Some(named("string")),
            ExprKind::Ident(n) => env.get(n).cloned(),
            ExprKind::Unary(op, x) => match op {
                crate::parser::ast::UnOp::Not => Some(named("bool")),
                crate::parser::ast::UnOp::Neg => self.static_ty(x, subst, env),
                crate::parser::ast::UnOp::Deref => match self.static_ty(x, subst, env)? {
                    Type::Ptr(b) => Some(*b),
                    _ => None,
                },
            },
            ExprKind::Binary(op, a, _) => {
                use crate::parser::ast::BinOp::*;
                match op {
                    Eq | Ne | Lt | Le | Gt | Ge | And | Or => Some(named("bool")),
                    _ => self.static_ty(a, subst, env),
                }
            }
            ExprKind::Index(a, _) => match self.static_ty(a, subst, env)? {
                Type::Slice(b) | Type::Array(b, _) => Some(*b),
                _ => None,
            },
            ExprKind::Call(callee, args) => match &callee.kind {
                ExprKind::Ident(f) => {
                    if let Some(gf) = self.gfuncs.get(f) {
                        let params: HashSet<String> = gf.generics.iter().cloned().collect();
                        let mut inf = Subst::new();
                        for (i, p) in gf.params.iter().enumerate() {
                            if let Some(a) = args.get(i) {
                                if let Some(at) = self.static_ty(a, subst, env) {
                                    unify(&p.ty, &at, &params, &mut inf);
                                }
                            }
                        }
                        let r = subst_apply(&gf.ret, &inf);
                        if mentions(&r, &params) {
                            None
                        } else {
                            Some(r)
                        }
                    } else {
                        self.fn_ret.get(f).cloned()
                    }
                }
                ExprKind::Field(base, v) => {
                    if let ExprKind::Ident(g) = &base.kind {
                        if self.genums.contains_key(g) && self.enum_has_variant(g, v) {
                            let targs = self.enum_args(g, None, args, subst, env, v);
                            return Some(Type::Named(g.clone(), targs));
                        }
                    }
                    None
                }
                _ => None,
            },
            ExprKind::Field(base, name) => {
                if let ExprKind::Ident(g) = &base.kind {
                    if self.genums.contains_key(g) && self.enum_has_variant(g, name) {
                        let targs = self.enum_args(g, None, &[], subst, env, name);
                        return Some(Type::Named(g.clone(), targs));
                    }
                }
                if let Type::Named(s, sargs) = self.static_ty(base, subst, env)? {
                    if let Some(gs) = self.gstructs.get(s.as_str()) {
                        let fsubst = bind(&gs.generics, &sargs);
                        let fld = gs.fields.iter().find(|f| &f.name == name)?;
                        return Some(subst_apply(&subst_apply(&fld.ty, &fsubst), subst));
                    }
                }
                None
            }
            ExprKind::StructLit(name, fields) => {
                if let Some(gs) = self.gstructs.get(name).copied() {
                    let targs = self.infer_struct_args(gs, fields, None, subst, env);
                    Some(Type::Named(name.clone(), targs))
                } else {
                    Some(named(name))
                }
            }
            _ => None,
        }
    }
}


fn node(kind: ExprKind, span: Span) -> Expr {
    Expr { kind, span }
}

fn named(n: &str) -> Type {
    Type::Named(n.to_string(), Vec::new())
}

fn bind(generics: &[String], args: &[Type]) -> Subst {
    generics.iter().cloned().zip(args.iter().cloned()).collect()
}

/// Whether a type still mentions any of the given type parameter names. Used to
/// reject a not fully inferred type before it leaks into outer inference.
fn mentions(ty: &Type, names: &HashSet<String>) -> bool {
    match ty {
        Type::Named(n, args) if args.is_empty() => names.contains(n),
        Type::Named(_, args) => args.iter().any(|a| mentions(a, names)),
        Type::Ptr(b) | Type::RawPtr(b) | Type::Slice(b) | Type::Array(b, _) => mentions(b, names),
        Type::Tuple(xs) => xs.iter().any(|x| mentions(x, names)),
        Type::Func(ps, r) => ps.iter().any(|p| mentions(p, names)) || mentions(r, names),
        Type::Unit => false,
    }
}

fn solve(generics: &[String], inf: &Subst) -> Vec<Type> {
    generics
        .iter()
        .map(|g| inf.get(g).cloned().unwrap_or_else(|| named("int64")))
        .collect()
}

fn subst_apply(ty: &Type, subst: &Subst) -> Type {
    match ty {
        Type::Named(n, args) if args.is_empty() => {
            subst.get(n).cloned().unwrap_or_else(|| named(n))
        }
        Type::Named(n, args) => {
            Type::Named(n.clone(), args.iter().map(|a| subst_apply(a, subst)).collect())
        }
        Type::Ptr(b) => Type::Ptr(Box::new(subst_apply(b, subst))),
        Type::RawPtr(b) => Type::RawPtr(Box::new(subst_apply(b, subst))),
        Type::Slice(b) => Type::Slice(Box::new(subst_apply(b, subst))),
        Type::Array(b, n) => Type::Array(Box::new(subst_apply(b, subst)), *n),
        Type::Tuple(xs) => Type::Tuple(xs.iter().map(|x| subst_apply(x, subst)).collect()),
        Type::Func(ps, r) => Type::Func(
            ps.iter().map(|p| subst_apply(p, subst)).collect(),
            Box::new(subst_apply(r, subst)),
        ),
        Type::Unit => Type::Unit,
    }
}

fn unify(pat: &Type, concrete: &Type, params: &HashSet<String>, out: &mut Subst) {
    match pat {
        Type::Named(n, args) if args.is_empty() && params.contains(n) => {
            out.entry(n.clone()).or_insert_with(|| concrete.clone());
        }
        Type::Named(_, pargs) => {
            if let Type::Named(_, cargs) = concrete {
                for (p, c) in pargs.iter().zip(cargs) {
                    unify(p, c, params, out);
                }
            }
        }
        Type::Ptr(pb) => {
            if let Type::Ptr(cb) = concrete {
                unify(pb, cb, params, out);
            }
        }
        Type::Slice(pb) => match concrete {
            Type::Slice(cb) | Type::Array(cb, _) => unify(pb, cb, params, out),
            _ => {}
        },
        Type::Array(pb, _) => {
            if let Type::Array(cb, _) = concrete {
                unify(pb, cb, params, out);
            }
        }
        _ => {}
    }
}

fn int_lit_ty(suffix: &Option<String>) -> &'static str {
    match suffix.as_deref() {
        Some("i8") => "int8",
        Some("u8") => "uint8",
        Some("i16") => "int16",
        Some("u16") => "uint16",
        Some("i32") => "int32",
        Some("u32") => "uint32",
        Some("u64") => "uint64",
        _ => "int64",
    }
}

fn mangle(name: &str, args: &[Type]) -> String {
    if args.is_empty() {
        return name.to_string();
    }
    let parts: Vec<String> = args.iter().map(flat).collect();
    format!("{name}${}", parts.join("$"))
}

/// Flattens a type to an injective token-safe string. Nested generic
/// references carry an arity prefix so siblings and nesting never alias
/// (`A$B$1$C$D` and `A$B$C$1$D` stay distinct), and non nominal constructors
/// use a leading `$` marker that no source identifier can begin with.
fn flat(ty: &Type) -> String {
    match ty {
        Type::Named(n, args) if args.is_empty() => n.clone(),
        Type::Named(n, args) => {
            let parts: Vec<String> = args.iter().map(flat).collect();
            format!("{n}${}${}", args.len(), parts.join("$"))
        }
        Type::Ptr(b) => format!("$p${}", flat(b)),
        Type::RawPtr(b) => format!("$rp${}", flat(b)),
        Type::Slice(b) => format!("$s${}", flat(b)),
        Type::Array(b, n) => format!("$a{n}${}", flat(b)),
        Type::Tuple(xs) => {
            let parts: Vec<String> = xs.iter().map(flat).collect();
            format!("$t{}${}", xs.len(), parts.join("$"))
        }
        Type::Func(ps, r) => {
            let parts: Vec<String> = ps.iter().map(flat).collect();
            format!("$f{}${}${}", ps.len(), parts.join("$"), flat(r))
        }
        Type::Unit => "$void".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn names(src: &str) -> Vec<String> {
        let (t, _) = lex(src);
        let (m, e) = parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        let out = expand(&m);
        out.items
            .iter()
            .map(|i| match i {
                Item::Func(f) => f.name.clone(),
                Item::Struct(s) => s.name.clone(),
                Item::Enum(en) => en.name.clone(),
                Item::Impl(im) => format!("impl {}", im.ty),
                Item::Interface(it) => it.name.clone(),
            })
            .collect()
    }

    #[test]
    fn generic_fn_instantiated_per_type() {
        let n = names(
            "func id<T>(x: T) -> T { return x }\n\
             func main() -> int32 {\n  a := id(1)\n  b := id(2.0)\n  return 0\n}",
        );
        assert!(n.contains(&"id$int64".to_string()), "{n:?}");
        assert!(n.contains(&"id$float64".to_string()), "{n:?}");
        assert!(!n.contains(&"id".to_string()), "generic def must not survive: {n:?}");
    }

    #[test]
    fn generic_struct_and_enum_instantiated() {
        let n = names(
            "struct Box<T> { v: T }\n\
             enum Opt<T> { Some(v: T), None }\n\
             func main() -> int32 {\n\
               b: Box<int32> = Box { v: 1 }\n\
               o: Opt<int64> = Opt.Some(2)\n\
               return 0\n\
             }",
        );
        assert!(n.contains(&"Box$int32".to_string()), "{n:?}");
        assert!(n.contains(&"Opt$int64".to_string()), "{n:?}");
        assert!(!n.iter().any(|x| x == "Box" || x == "Opt"), "{n:?}");
    }

    #[test]
    fn return_only_param_uses_annotation() {
        // A type param appearing only in the return position must be taken from the
        // let annotation, not silently defaulted to int64.
        let n = names(
            "func cast<From, To>(x: From) -> To {\n  return x\n}\n\
             func main() -> int32 {\n  y: float64 = cast(7)\n  return 0\n}",
        );
        assert!(n.contains(&"cast$int64$float64".to_string()), "{n:?}");
        assert!(!n.contains(&"cast$int64$int64".to_string()), "{n:?}");
    }

    #[test]
    fn no_generic_items_remain() {
        let n = names(
            "struct Pair<A, B> { first: A, second: B }\n\
             func mk<T>(x: T) -> Pair<T, T> { return Pair { first: x, second: x } }\n\
             func main() -> int32 {\n  p := mk(3)\n  return 0\n}",
        );
        assert!(n.contains(&"mk$int64".to_string()), "{n:?}");
        assert!(n.contains(&"Pair$int64$int64".to_string()), "{n:?}");
    }

    #[test]
    fn tmp_poly_recursion_repro() {
        let n = names(
            "struct Box<T> { v: T }\n\
             func f<T>(x: T) -> int32 {\n  b := Box { v: x }\n  return f(b)\n}\n\
             func main() -> int32 { return f(0) }",
        );
        eprintln!("POLY NAMES = {n:?}");
        assert!(n.iter().any(|x| x.starts_with("f$")), "{n:?}");
    }

    #[test]
    fn zz_leak_typeparam_check() {
        let src = "func pick<T>() -> T { return pick() }\n\
                   func first<A>(a: A, b: int64) -> A { return a }\n\
                   func main() -> int32 {\n\
                     x := first(pick(), 3)\n\
                     return 0\n\
                   }";
        let (t, _) = lex(src);
        let (m, e) = parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        let out = expand(&m);
        for it in &out.items {
            if let Item::Func(f) = it {
                eprintln!("FUNC {} params={:?} ret={:?}", f.name, f.params, f.ret);
            }
        }
        // Assert NO emitted function signature mentions a bare type parameter.
        for it in &out.items {
            if let Item::Func(f) = it {
                for p in &f.params {
                    if let Type::Named(n, a) = &p.ty {
                        assert!(
                            a.is_empty() == false || n != "T" && n != "A",
                            "LEAK: func {} param {} typed bare param {}",
                            f.name, p.name, n
                        );
                    }
                }
                if let Type::Named(n, _) = &f.ret {
                    assert!(
                        n != "T" && n != "A",
                        "LEAK: func {} returns bare param {}",
                        f.name, n
                    );
                }
            }
        }
    }

    #[test]
    fn repro_structlit_inference_gap() {
        let n = names(
            "struct Pair<A, B> { x: A, y: B }\n\
             func identity<T>(x: T) -> T { return x }\n\
             func main() -> int32 {\n\
               v := Pair { x: 1, y: 2 }\n\
               w := identity(v)\n\
               return 0\n\
             }",
        );
        eprintln!("EMITTED NAMES: {n:?}");
        assert!(n.contains(&"Pair$int64$int64".to_string()), "missing Pair$int64$int64: {n:?}");
        assert!(
            n.contains(&"identity$Pair$int64$int64".to_string()),
            "MISSING correct identity monomorph: {n:?}"
        );
        assert!(
            !n.iter().any(|x| x.contains("void")),
            "BOGUS void monomorph present: {n:?}"
        );
    }

    #[test]
    fn repro_match_payload_binding_inference() {
        let n = names(
            "enum Box<T> { Has(value: T), Empty }\n\
             func id<T>(x: T) -> T { return x }\n\
             func f(b: Box<float64>) -> float64 {\n\
               match b {\n\
                 Has(v) => return id(v),\n\
                 Empty => return 0.0,\n\
               }\n\
             }\n\
             func main() -> int32 { return 0 }",
        );
        eprintln!("MATCH PAYLOAD NAMES = {n:?}");
        assert!(n.contains(&"id$float64".to_string()), "expected id$float64, got {n:?}");
        assert!(!n.contains(&"id$int64".to_string()), "wrong int64 monomorph present: {n:?}");
    }

}
