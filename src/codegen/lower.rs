//! Lowering: AST to textual LLVM IR. M5 (scalar core) plus M6 (structs, pointers,
//! methods, defer).
//!
//! Locals are alloca slots; values load and store through them and LLVM's mem2reg
//! builds SSA. Structs are first class aggregates built with insertvalue and read with
//! GEP or extractvalue. Pointers come from `alloc` (a malloc plus a store) and are read
//! with explicit `*`. Methods dispatch statically by receiver type. `defer`red calls
//! run in reverse at each return. Slices, arrays, sum types, generics, closures, and
//! the allocator interface land in later milestones and lower to placeholders here.

use std::collections::{HashMap, HashSet};

use crate::codegen::llvm::Module;
use crate::codegen::DEFAULT_TRIPLE;
use crate::parser::ast::{
    self, Arm, BinOp, Block, Expr, ExprKind, Func, If, Item, Lambda, Let, Pattern, Stmt, Type, UnOp,
    While,
};

/// Compiles a module to LLVM IR text. Generics are monomorphized first.
pub fn compile(module: &ast::Module) -> String {
    let expanded = crate::mono::expand(module);
    let module = &expanded;
    let ctx = Ctx::new(module);
    let mut m = Module::new("dusk", DEFAULT_TRIPLE);
    m.declare("void @cool_print_i64(i64)");
    m.declare("void @cool_print_f64(double)");
    m.declare("void @cool_println_cstr(ptr)");
    m.declare("ptr @cool_alloc(i64)");
    m.declare("void @cool_free(ptr)");
    for (name, fields) in &ctx.structs {
        let body = fields
            .iter()
            .map(|(_, ty)| ty.ll())
            .collect::<Vec<_>>()
            .join(", ");
        m.define_type(name, &format!("{{ {body} }}"));
    }
    for def in &ctx.enums {
        let lanes = ctx.blob_lanes(def);
        let body = format!("{{ i{}, [{lanes} x i64] }}", def.tag_bits);
        m.define_type(&def.name, &body);
    }
    for item in &module.items {
        match item {
            Item::Func(f) => {
                let def = gen_func(&mut m, &ctx, f, None);
                m.push_function(def);
            }
            Item::Impl(im) => {
                for method in &im.methods {
                    let def = gen_func(&mut m, &ctx, method, Some(&im.ty));
                    m.push_function(def);
                }
            }
            _ => {}
        }
    }
    emit_vtables(&mut m, &ctx);
    m.render()
}

/// Emits one vtable constant per `impl Iface for Type`, plus a thunk per slot
/// that loads the receiver from the data pointer and forwards to the by value
/// method, bridging dynamic dispatch to the static method calling convention.
fn emit_vtables(m: &mut Module, ctx: &Ctx) {
    for im in &ctx.impls {
        let Some(idef) = ctx.iface(&im.iface) else {
            continue;
        };
        let mut slots = Vec::new();
        for meth in &idef.methods {
            let thunk = format!("@thunk.{}.{}.{}", im.iface, im.ty, meth.name);
            emit_thunk(m, &im.ty, meth, &thunk);
            slots.push(format!("ptr {thunk}"));
        }
        let n = idef.methods.len();
        m.global(format!(
            "@vtable.{}.{} = constant [{n} x ptr] [{}]",
            im.iface,
            im.ty,
            slots.join(", ")
        ));
    }
}

fn emit_thunk(m: &mut Module, ty: &str, meth: &IMethod, name: &str) {
    let mut sig = vec!["ptr %d".to_string()];
    let mut call_args = vec![format!("%{ty} %self")];
    for (i, p) in meth.params.iter().enumerate() {
        sig.push(format!("{} %a{i}", p.ll()));
        call_args.push(format!("{} %a{i}", p.ll()));
    }
    let ca = call_args.join(", ");
    let mut body = String::from("entry:\n");
    body.push_str(&format!("  %self = load %{ty}, ptr %d\n"));
    if matches!(meth.ret, CTy::Void) {
        body.push_str(&format!("  call void @{ty}.{}({ca})\n", meth.name));
        body.push_str("  ret void\n");
    } else {
        body.push_str(&format!(
            "  %r = call {} @{ty}.{}({ca})\n",
            meth.ret.ll(),
            meth.name
        ));
        body.push_str(&format!("  ret {} %r\n", meth.ret.ll()));
    }
    m.push_function(format!(
        "define {} {name}({}) {{\n{body}}}",
        meth.ret.ll(),
        sig.join(", ")
    ));
}

#[derive(Clone, PartialEq)]
enum CTy {
    Int(u32),
    F64,
    F32,
    Bool,
    Void,
    Ptr(Box<CTy>),
    Slice(Box<CTy>),
    Array(Box<CTy>, u64),
    Struct(String),
    Enum(String),
    Iface(String),
    Closure(Vec<CTy>, Box<CTy>),
    Unknown,
}

impl CTy {
    fn ll(&self) -> String {
        match self {
            CTy::Int(n) => format!("i{n}"),
            CTy::F64 => "double".to_string(),
            CTy::F32 => "float".to_string(),
            CTy::Bool => "i1".to_string(),
            CTy::Void => "void".to_string(),
            CTy::Ptr(_) => "ptr".to_string(),
            CTy::Slice(_) => "{ ptr, i64 }".to_string(),
            CTy::Array(e, n) => format!("[{n} x {}]", e.ll()),
            CTy::Struct(n) | CTy::Enum(n) => format!("%{n}"),
            CTy::Iface(_) | CTy::Closure(..) => "{ ptr, ptr }".to_string(),
            CTy::Unknown => "i64".to_string(),
        }
    }

    fn is_aggregate(&self) -> bool {
        matches!(
            self,
            CTy::Struct(_)
                | CTy::Enum(_)
                | CTy::Iface(_)
                | CTy::Closure(..)
                | CTy::Slice(_)
                | CTy::Array(..)
        )
    }

    fn is_float(&self) -> bool {
        matches!(self, CTy::F64 | CTy::F32)
    }

    fn int_bits(&self) -> Option<u32> {
        match self {
            CTy::Int(n) => Some(*n),
            CTy::Bool => Some(1),
            _ => None,
        }
    }
}

struct Val {
    ty: CTy,
    op: String,
}

impl Val {
    fn new(ty: CTy, op: impl Into<String>) -> Self {
        Val { ty, op: op.into() }
    }

    fn i0() -> Self {
        Val::new(CTy::Int(64), "0")
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Nom {
    Struct,
    Enum,
    Iface,
    None,
}

/// An interface method slot: its name and lowered signature (return plus the
/// parameters after the receiver).
struct IMethod {
    name: String,
    ret: CTy,
    params: Vec<CTy>,
}

struct IfaceDef {
    name: String,
    methods: Vec<IMethod>,
}

/// An `impl Iface for Type` record: the concrete type and interface it links.
struct ImplInfo {
    iface: String,
    ty: String,
}

struct VariantDef {
    name: String,
    tag: u64,
    fields: Vec<(String, CTy)>,
}

struct EnumDef {
    name: String,
    tag_bits: u32,
    variants: Vec<VariantDef>,
}

struct Ctx {
    structs: Vec<(String, Vec<(String, CTy)>)>,
    enums: Vec<EnumDef>,
    ifaces: Vec<IfaceDef>,
    impls: Vec<ImplInfo>,
    fns: HashMap<String, (CTy, Vec<CTy>)>,
    methods: HashMap<String, (CTy, Vec<CTy>)>,
}

impl Ctx {
    fn new(module: &ast::Module) -> Self {
        let mut struct_names = Vec::new();
        let mut enum_names = Vec::new();
        let mut iface_names = Vec::new();
        for item in &module.items {
            match item {
                Item::Struct(s) => struct_names.push(s.name.clone()),
                Item::Enum(e) => enum_names.push(e.name.clone()),
                Item::Interface(i) => iface_names.push(i.name.clone()),
                _ => {}
            }
        }
        let nom = |n: &str| {
            if struct_names.iter().any(|s| s == n) {
                Nom::Struct
            } else if enum_names.iter().any(|s| s == n) {
                Nom::Enum
            } else if iface_names.iter().any(|s| s == n) {
                Nom::Iface
            } else {
                Nom::None
            }
        };
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    let fields = s
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), lower_ty(&f.ty, &nom)))
                        .collect();
                    structs.push((s.name.clone(), fields));
                }
                Item::Enum(e) => {
                    let tag_bits = tag_bits(e.variants.len());
                    let variants = e
                        .variants
                        .iter()
                        .enumerate()
                        .map(|(i, v)| VariantDef {
                            name: v.name.clone(),
                            tag: i as u64,
                            fields: v
                                .fields
                                .iter()
                                .map(|f| (f.name.clone(), lower_ty(&f.ty, &nom)))
                                .collect(),
                        })
                        .collect();
                    enums.push(EnumDef {
                        name: e.name.clone(),
                        tag_bits,
                        variants,
                    });
                }
                _ => {}
            }
        }
        let mut ifaces = Vec::new();
        let mut impls = Vec::new();
        for item in &module.items {
            match item {
                Item::Interface(i) => {
                    let methods = i
                        .methods
                        .iter()
                        .map(|m| IMethod {
                            name: m.name.clone(),
                            ret: lower_ty(&m.ret, &nom),
                            params: m.params.iter().map(|p| lower_ty(&p.ty, &nom)).collect(),
                        })
                        .collect();
                    ifaces.push(IfaceDef {
                        name: i.name.clone(),
                        methods,
                    });
                }
                Item::Impl(im) => {
                    if let Some(iface) = &im.iface {
                        impls.push(ImplInfo {
                            iface: iface.clone(),
                            ty: im.ty.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        let mut fns = HashMap::new();
        let mut methods = HashMap::new();
        for item in &module.items {
            match item {
                Item::Func(f) => {
                    let ret = lower_ty(&f.ret, &nom);
                    let params = f.params.iter().map(|p| lower_ty(&p.ty, &nom)).collect();
                    fns.insert(f.name.clone(), (ret, params));
                }
                Item::Impl(im) => {
                    for method in &im.methods {
                        let ret = lower_ty(&method.ret, &nom);
                        let mut params = vec![CTy::Struct(im.ty.clone())];
                        params.extend(method.params.iter().map(|p| lower_ty(&p.ty, &nom)));
                        methods.insert(format!("{}.{}", im.ty, method.name), (ret, params));
                    }
                }
                _ => {}
            }
        }
        Ctx {
            structs,
            enums,
            ifaces,
            impls,
            fns,
            methods,
        }
    }

    fn nom(&self, name: &str) -> Nom {
        if self.structs.iter().any(|(n, _)| n == name) {
            Nom::Struct
        } else if self.enums.iter().any(|e| e.name == name) {
            Nom::Enum
        } else if self.ifaces.iter().any(|i| i.name == name) {
            Nom::Iface
        } else {
            Nom::None
        }
    }

    fn iface(&self, name: &str) -> Option<&IfaceDef> {
        self.ifaces.iter().find(|i| i.name == name)
    }

    /// Index of an interface method by name, for vtable slot lookup.
    fn iface_method(&self, iface: &str, method: &str) -> Option<(usize, &IMethod)> {
        self.iface(iface)?
            .methods
            .iter()
            .enumerate()
            .find(|(_, m)| m.name == method)
    }

    fn fields(&self, name: &str) -> Option<&Vec<(String, CTy)>> {
        self.structs.iter().find(|(n, _)| n == name).map(|(_, f)| f)
    }

    fn field(&self, name: &str, field: &str) -> Option<(u32, CTy)> {
        let fields = self.fields(name)?;
        fields
            .iter()
            .position(|(n, _)| n == field)
            .map(|i| (i as u32, fields[i].1.clone()))
    }

    fn enum_def(&self, name: &str) -> Option<&EnumDef> {
        self.enums.iter().find(|e| e.name == name)
    }

    fn variant(&self, ename: &str, vname: &str) -> Option<&VariantDef> {
        self.enum_def(ename)?.variants.iter().find(|v| v.name == vname)
    }

    /// Size and alignment in bytes of a lowered type.
    fn size_align(&self, ty: &CTy) -> (u64, u64) {
        match ty {
            CTy::Bool => (1, 1),
            CTy::Int(n) => {
                let b = (*n as u64 / 8).max(1);
                (b, b)
            }
            CTy::F32 => (4, 4),
            CTy::F64 => (8, 8),
            CTy::Ptr(_) => (8, 8),
            CTy::Slice(_) => (16, 8),
            CTy::Array(e, n) => {
                let (es, ea) = self.size_align(e);
                (align_up(es, ea) * n, ea)
            }
            CTy::Struct(name) => self.fields(name).map(|f| self.layout(f)).unwrap_or((8, 8)),
            CTy::Enum(name) => self.enum_size_align(name),
            CTy::Iface(_) | CTy::Closure(..) => (16, 8),
            CTy::Void => (0, 1),
            CTy::Unknown => (8, 8),
        }
    }

    /// Sequential field layout: returns total size and alignment.
    fn layout(&self, fields: &[(String, CTy)]) -> (u64, u64) {
        let mut off = 0u64;
        let mut align = 1u64;
        for (_, ty) in fields {
            let (s, a) = self.size_align(ty);
            off = align_up(off, a) + s;
            align = align.max(a);
        }
        align = align.max(1);
        (align_up(off, align), align)
    }

    /// Byte offset of each payload field within a variant.
    fn offsets(&self, v: &VariantDef) -> Vec<u64> {
        let mut off = 0u64;
        let mut out = Vec::new();
        for (_, ty) in &v.fields {
            let (s, a) = self.size_align(ty);
            off = align_up(off, a);
            out.push(off);
            off += s;
        }
        out
    }

    /// Number of i64 lanes the payload blob needs to cover the largest variant.
    fn blob_lanes(&self, def: &EnumDef) -> u64 {
        let max = def
            .variants
            .iter()
            .map(|v| self.layout(&v.fields).0)
            .max()
            .unwrap_or(0);
        max.div_ceil(8)
    }

    fn enum_size_align(&self, name: &str) -> (u64, u64) {
        let Some(def) = self.enum_def(name) else {
            return (8, 8);
        };
        let tag_bytes = (def.tag_bits as u64) / 8;
        let size = align_up(tag_bytes, 8) + self.blob_lanes(def) * 8;
        (size.max(8), 8)
    }
}

fn tag_bits(count: usize) -> u32 {
    if count <= 256 {
        8
    } else if count <= 65536 {
        16
    } else {
        32
    }
}

fn align_up(n: u64, align: u64) -> u64 {
    if align <= 1 {
        n
    } else {
        (n + align - 1) / align * align
    }
}

fn lower_ty(t: &Type, nom: &impl Fn(&str) -> Nom) -> CTy {
    match t {
        Type::Named(n, _) => match n.as_str() {
            "int8" | "uint8" => CTy::Int(8),
            "int16" | "uint16" => CTy::Int(16),
            "int32" | "uint32" => CTy::Int(32),
            "int64" | "uint64" => CTy::Int(64),
            "bool" => CTy::Bool,
            "char" => CTy::Int(8),
            "float64" => CTy::F64,
            "float32" => CTy::F32,
            "string" => CTy::Ptr(Box::new(CTy::Int(8))),
            _ => match nom(n) {
                Nom::Struct => CTy::Struct(n.clone()),
                Nom::Enum => CTy::Enum(n.clone()),
                Nom::Iface => CTy::Iface(n.clone()),
                Nom::None => CTy::Unknown,
            },
        },
        Type::Ptr(b) => CTy::Ptr(Box::new(lower_ty(b, nom))),
        Type::Slice(b) => CTy::Slice(Box::new(lower_ty(b, nom))),
        Type::Array(b, n) => CTy::Array(Box::new(lower_ty(b, nom)), *n),
        Type::Func(ps, r) => CTy::Closure(
            ps.iter().map(|p| lower_ty(p, nom)).collect(),
            Box::new(lower_ty(r, nom)),
        ),
        Type::Unit => CTy::Void,
        _ => CTy::Unknown,
    }
}

fn gen_func(m: &mut Module, ctx: &Ctx, f: &Func, self_ty: Option<&str>) -> String {
    let nom = |n: &str| ctx.nom(n);
    let mut params: Vec<(String, CTy)> = Vec::new();
    if let Some(t) = self_ty {
        params.push(("self".to_string(), CTy::Struct(t.to_string())));
    }
    for p in &f.params {
        params.push((p.name.clone(), lower_ty(&p.ty, &nom)));
    }
    let ret = lower_ty(&f.ret, &nom);
    let name = match self_ty {
        Some(t) => format!("{t}.{}", f.name),
        None => f.name.clone(),
    };
    let sig = params
        .iter()
        .enumerate()
        .map(|(i, (_, ty))| format!("{} %a{i}", ty.ll()))
        .collect::<Vec<_>>()
        .join(", ");
    let header = format!("define {} @{name}({sig}) {{", ret.ll());
    let mut fb = Fb::new(m, ctx, ret);
    fb.body.push_str("entry:\n");
    for (i, (pname, ty)) in params.iter().enumerate() {
        let ptr = fb.alloca(ty);
        fb.line(&format!("store {} %a{i}, ptr {ptr}", ty.ll()));
        fb.locals.insert(pname.clone(), (ty.clone(), ptr));
    }
    for p in &f.params {
        if p.using {
            if let Some((ty, _)) = fb.locals.get(&p.name) {
                fb.allocator = Some((p.name.clone(), ty.clone()));
            }
        }
    }
    fb.gen_block(&f.body.stmts);
    if !fb.terminated {
        fb.emit_defers();
        fb.default_ret();
    }
    format!("{header}\n{}}}", fb.body)
}

struct Fb<'a> {
    m: &'a mut Module,
    ctx: &'a Ctx,
    ret: CTy,
    body: String,
    tmp: u32,
    label: u32,
    locals: HashMap<String, (CTy, String)>,
    defers: Vec<Expr>,
    terminated: bool,
    allocator: Option<(String, CTy)>,
}

impl<'a> Fb<'a> {
    fn new(m: &'a mut Module, ctx: &'a Ctx, ret: CTy) -> Self {
        Fb {
            m,
            ctx,
            ret,
            body: String::new(),
            tmp: 0,
            label: 0,
            locals: HashMap::new(),
            defers: Vec::new(),
            terminated: false,
            allocator: None,
        }
    }

    fn fresh(&mut self) -> String {
        let t = format!("%t{}", self.tmp);
        self.tmp += 1;
        t
    }

    fn new_label(&mut self) -> String {
        let l = format!("L{}", self.label);
        self.label += 1;
        l
    }

    fn line(&mut self, s: &str) {
        self.body.push_str("  ");
        self.body.push_str(s);
        self.body.push('\n');
    }

    fn place_label(&mut self, l: &str) {
        self.body.push_str(l);
        self.body.push_str(":\n");
        self.terminated = false;
    }

    fn alloca(&mut self, ty: &CTy) -> String {
        let d = self.fresh();
        self.line(&format!("{d} = alloca {}", ty.ll()));
        d
    }

    fn load(&mut self, ty: &CTy, ptr: &str) -> String {
        let d = self.fresh();
        self.line(&format!("{d} = load {}, ptr {ptr}", ty.ll()));
        d
    }

    fn op2(&mut self, opcode: &str, ty: &str, a: &str, b: &str) -> String {
        let d = self.fresh();
        self.line(&format!("{d} = {opcode} {ty} {a}, {b}"));
        d
    }

    fn br(&mut self, l: &str) {
        self.line(&format!("br label %{l}"));
        self.terminated = true;
    }

    fn cond_br(&mut self, c: &str, t: &str, e: &str) {
        self.line(&format!("br i1 {c}, label %{t}, label %{e}"));
        self.terminated = true;
    }

    fn default_ret(&mut self) {
        let r = self.ret.clone();
        match &r {
            CTy::Void => self.line("ret void"),
            CTy::F64 | CTy::F32 => self.line(&format!("ret {} 0.0", r.ll())),
            CTy::Ptr(_) => self.line("ret ptr null"),
            _ if r.is_aggregate() => self.line(&format!("ret {} zeroinitializer", r.ll())),
            _ => self.line(&format!("ret {} 0", r.ll())),
        }
        self.terminated = true;
    }

    fn coerce(&mut self, from: &CTy, op: &str, to: &CTy) -> String {
        if from == to {
            return op.to_string();
        }
        if let (Some(fb), Some(tb)) = (from.int_bits(), to.int_bits()) {
            if fb == tb {
                return op.to_string();
            }
            // A bool is an unsigned 1 bit value: zero extend it so true widens to
            // 1, not the all ones -1 that sext would give.
            let cast = if tb < fb {
                "trunc"
            } else if matches!(from, CTy::Bool) {
                "zext"
            } else {
                "sext"
            };
            let d = self.fresh();
            self.line(&format!("{d} = {cast} {} {op} to {}", from.ll(), to.ll()));
            return d;
        }
        if from.int_bits().is_some() && to.is_float() {
            let d = self.fresh();
            self.line(&format!("{d} = sitofp {} {op} to {}", from.ll(), to.ll()));
            return d;
        }
        if from.is_float() && to.int_bits().is_some() {
            let d = self.fresh();
            self.line(&format!("{d} = fptosi {} {op} to {}", from.ll(), to.ll()));
            return d;
        }
        op.to_string()
    }

    /// Coerces a value to a target type, boxing a concrete struct into an
    /// interface fat pointer when the target is an interface.
    fn adapt(&mut self, v: Val, target: &CTy) -> Val {
        if let (CTy::Iface(i), CTy::Struct(t)) = (target, &v.ty) {
            let (iface, ty) = (i.clone(), t.clone());
            return self.box_iface(&v, &iface, &ty);
        }
        if let (CTy::Slice(_), CTy::Array(_, n)) = (target, &v.ty) {
            let n = *n as usize;
            return self.slice_from_array(v, n);
        }
        let op = self.coerce(&v.ty, &v.op, target);
        Val::new(target.clone(), op)
    }

    /// Boxes a struct value as an interface fat pointer `{ data, vtable }`.
    fn box_iface(&mut self, v: &Val, iface: &str, ty: &str) -> Val {
        let slot = self.alloca(&v.ty);
        self.line(&format!("store {} {}, ptr {slot}", v.ty.ll(), v.op));
        let a = self.fresh();
        self.line(&format!("{a} = insertvalue {{ ptr, ptr }} undef, ptr {slot}, 0"));
        let b = self.fresh();
        self.line(&format!(
            "{b} = insertvalue {{ ptr, ptr }} {a}, ptr @vtable.{iface}.{ty}, 1"
        ));
        Val::new(CTy::Iface(iface.to_string()), b)
    }

    fn emit_defers(&mut self) {
        let ds = self.defers.clone();
        for e in ds.iter().rev() {
            self.gen_expr(e);
        }
    }

    fn gen_block(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            if self.terminated {
                break;
            }
            self.gen_stmt(s);
        }
    }

    fn gen_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(l) => self.gen_let(l),
            Stmt::Assign(lhs, rhs) => {
                if let Some((ty, ptr)) = self.gen_place(lhs) {
                    let v = self.gen_expr(rhs);
                    let op = self.coerce(&v.ty, &v.op, &ty);
                    self.line(&format!("store {} {op}, ptr {ptr}", ty.ll()));
                } else {
                    self.gen_expr(rhs);
                }
            }
            Stmt::Return(Some(e)) => {
                let v = self.gen_expr(e);
                let ret = self.ret.clone();
                let av = self.adapt(v, &ret);
                self.emit_defers();
                if matches!(ret, CTy::Void) {
                    self.line("ret void");
                } else {
                    self.line(&format!("ret {} {}", ret.ll(), av.op));
                }
                self.terminated = true;
            }
            Stmt::Return(None) => {
                self.emit_defers();
                self.default_ret();
            }
            Stmt::Defer(e) => self.defers.push(e.clone()),
            Stmt::If(i) => self.gen_if(i),
            Stmt::While(w) => self.gen_while(w),
            Stmt::Expr(e) => {
                self.gen_expr(e);
            }
            Stmt::Match(m) => self.gen_match(&m.scrut, &m.arms, None),
            Stmt::For(_) => {}
        }
    }

    fn gen_let(&mut self, l: &Let) {
        if l.binds.len() != 1 {
            return;
        }
        let bind = &l.binds[0];
        let declared = bind
            .ty
            .as_ref()
            .map(|t| lower_ty(t, &|n| self.ctx.nom(n)));
        let v = match (&l.value.kind, &declared) {
            (ExprKind::Array(elems), Some(CTy::Array(elem, _))) => {
                let hint = (**elem).clone();
                self.gen_array_lit(elems, Some(hint))
            }
            (ExprKind::Array(elems), Some(CTy::Slice(elem))) => {
                let hint = (**elem).clone();
                self.array_to_slice(elems, hint)
            }
            _ => self.gen_expr(&l.value),
        };
        let ty = declared.unwrap_or_else(|| v.ty.clone());
        let av = self.adapt(v, &ty);
        let ptr = self.alloca(&ty);
        self.line(&format!("store {} {}, ptr {ptr}", ty.ll(), av.op));
        self.locals.insert(bind.name.clone(), (ty, ptr));
    }

    fn gen_if(&mut self, i: &If) {
        let c = self.gen_expr(&i.cond);
        let cond = self.coerce(&c.ty, &c.op, &CTy::Bool);
        let then_l = self.new_label();
        let end_l = self.new_label();
        let else_l = if i.els.is_some() {
            self.new_label()
        } else {
            end_l.clone()
        };
        self.cond_br(&cond, &then_l, &else_l);
        self.place_label(&then_l);
        self.gen_block(&i.then.stmts);
        if !self.terminated {
            self.br(&end_l);
        }
        if let Some(els) = &i.els {
            self.place_label(&else_l);
            self.gen_block(&els.stmts);
            if !self.terminated {
                self.br(&end_l);
            }
        }
        self.place_label(&end_l);
    }

    fn gen_while(&mut self, w: &While) {
        let cond_l = self.new_label();
        let body_l = self.new_label();
        let end_l = self.new_label();
        if w.post_test {
            self.br(&body_l);
            self.place_label(&body_l);
            self.gen_block(&w.body.stmts);
            if !self.terminated {
                self.br(&cond_l);
            }
            self.place_label(&cond_l);
            let c = self.gen_expr(&w.cond);
            let cond = self.coerce(&c.ty, &c.op, &CTy::Bool);
            self.cond_br(&cond, &body_l, &end_l);
        } else {
            self.br(&cond_l);
            self.place_label(&cond_l);
            let c = self.gen_expr(&w.cond);
            let cond = self.coerce(&c.ty, &c.op, &CTy::Bool);
            self.cond_br(&cond, &body_l, &end_l);
            self.place_label(&body_l);
            self.gen_block(&w.body.stmts);
            if !self.terminated {
                self.br(&cond_l);
            }
        }
        self.place_label(&end_l);
    }

    /// Returns the address and type of an lvalue, or None for an rvalue.
    fn gen_place(&mut self, e: &Expr) -> Option<(CTy, String)> {
        match &e.kind {
            ExprKind::Ident(name) => self.locals.get(name).cloned(),
            ExprKind::Field(base, field) => {
                let (bty, bptr) = self.gen_place(base)?;
                let CTy::Struct(t) = &bty else {
                    return None;
                };
                let (idx, fty) = self.ctx.field(t, field)?;
                let fp = self.fresh();
                self.line(&format!(
                    "{fp} = getelementptr %{t}, ptr {bptr}, i32 0, i32 {idx}"
                ));
                Some((fty, fp))
            }
            ExprKind::Unary(UnOp::Deref, p) => {
                let pv = self.gen_expr(p);
                if let CTy::Ptr(inner) = pv.ty {
                    Some((*inner, pv.op))
                } else {
                    None
                }
            }
            ExprKind::Index(base, idx) => {
                if matches!(idx.kind, ExprKind::Range(..)) {
                    return None;
                }
                let iv = self.gen_expr(idx);
                let i = self.coerce(&iv.ty, &iv.op, &CTy::Int(64));
                self.elem_addr(base, &i)
            }
            _ => None,
        }
    }

    /// Address of element `i` of an indexable base (array, slice, or pointer).
    fn elem_addr(&mut self, base: &Expr, i: &str) -> Option<(CTy, String)> {
        if let Some((bty, bptr)) = self.gen_place(base) {
            match &bty {
                CTy::Array(elem, _) => {
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = getelementptr {}, ptr {bptr}, i64 0, i64 {i}",
                        bty.ll()
                    ));
                    Some(((**elem).clone(), p))
                }
                CTy::Slice(elem) => {
                    let data = self.load(&CTy::Ptr(elem.clone()), &bptr);
                    let p = self.fresh();
                    self.line(&format!("{p} = getelementptr {}, ptr {data}, i64 {i}", elem.ll()));
                    Some(((**elem).clone(), p))
                }
                CTy::Ptr(elem) => {
                    let pv = self.load(&CTy::Ptr(elem.clone()), &bptr);
                    let p = self.fresh();
                    self.line(&format!("{p} = getelementptr {}, ptr {pv}, i64 {i}", elem.ll()));
                    Some(((**elem).clone(), p))
                }
                _ => None,
            }
        } else {
            let bv = self.gen_expr(base);
            match &bv.ty {
                CTy::Slice(elem) => {
                    let data = self.fresh();
                    self.line(&format!("{data} = extractvalue {{ ptr, i64 }} {}, 0", bv.op));
                    let p = self.fresh();
                    self.line(&format!("{p} = getelementptr {}, ptr {data}, i64 {i}", elem.ll()));
                    Some(((**elem).clone(), p))
                }
                CTy::Ptr(elem) => {
                    let p = self.fresh();
                    self.line(&format!("{p} = getelementptr {}, ptr {}, i64 {i}", elem.ll(), bv.op));
                    Some(((**elem).clone(), p))
                }
                _ => None,
            }
        }
    }

    fn gen_expr(&mut self, e: &Expr) -> Val {
        if let Some((en, vn)) = self.as_enum_variant(e) {
            return self.gen_enum_ctor(&en, &vn, &[]);
        }
        match &e.kind {
            ExprKind::Int(v, suffix) => Val::new(int_ty(suffix), v.to_string()),
            ExprKind::Float(v, _) => Val::new(CTy::F64, format!("0x{:016X}", v.to_bits())),
            ExprKind::Bool(b) => Val::new(CTy::Bool, if *b { "1" } else { "0" }),
            ExprKind::Char(c) => Val::new(CTy::Int(8), (*c as u8).to_string()),
            ExprKind::Str(s) => Val::new(CTy::Ptr(Box::new(CTy::Int(8))), self.m.cstring(s)),
            ExprKind::Ident(_) | ExprKind::Field(..) | ExprKind::Unary(UnOp::Deref, _) => {
                self.gen_load(e)
            }
            ExprKind::Index(base, idx) => {
                if let ExprKind::Range(lo, hi) = &idx.kind {
                    self.gen_slice(base, lo, hi)
                } else {
                    self.gen_load(e)
                }
            }
            ExprKind::Array(elems) => self.gen_array_lit(elems, None),
            ExprKind::Unary(op, x) => self.gen_unary(*op, x),
            ExprKind::Binary(op, a, b) => self.gen_binary(*op, a, b),
            ExprKind::Call(f, args) => self.gen_call(f, args),
            ExprKind::StructLit(name, fields) => self.gen_struct_lit(name, fields),
            ExprKind::Lambda(l) => self.gen_lambda(l),
            ExprKind::Match(m) => {
                let rty = self.match_result_ty(&m.arms);
                let slot = self.alloca(&rty);
                self.gen_match(&m.scrut, &m.arms, Some((rty.clone(), slot.clone())));
                let v = self.load(&rty, &slot);
                Val::new(rty, v)
            }
            ExprKind::SizeofType(t) => {
                let cty = lower_ty(t, &|n| self.ctx.nom(n));
                let sz = self.elem_size(&cty);
                Val::new(CTy::Int(64), sz)
            }
            _ => Val::i0(),
        }
    }

    fn gen_load(&mut self, e: &Expr) -> Val {
        if let Some((ty, ptr)) = self.gen_place(e) {
            let v = self.load(&ty, &ptr);
            return Val::new(ty, v);
        }
        if let ExprKind::Field(base, field) = &e.kind {
            let bv = self.gen_expr(base);
            match &bv.ty {
                CTy::Struct(t) => {
                    if let Some((idx, fty)) = self.ctx.field(t, field) {
                        let d = self.fresh();
                        self.line(&format!("{d} = extractvalue {} {}, {idx}", bv.ty.ll(), bv.op));
                        return Val::new(fty, d);
                    }
                }
                CTy::Slice(elem) => {
                    let (idx, fty) = match field.as_str() {
                        "ptr" => (0, CTy::Ptr(elem.clone())),
                        "len" => (1, CTy::Int(64)),
                        _ => return Val::i0(),
                    };
                    let d = self.fresh();
                    self.line(&format!("{d} = extractvalue {{ ptr, i64 }} {}, {idx}", bv.op));
                    return Val::new(fty, d);
                }
                _ => {}
            }
        }
        Val::i0()
    }

    fn gen_unary(&mut self, op: UnOp, x: &Expr) -> Val {
        let v = self.gen_expr(x);
        match op {
            UnOp::Neg => {
                if v.ty.is_float() {
                    let d = self.fresh();
                    self.line(&format!("{d} = fneg {} {}", v.ty.ll(), v.op));
                    Val::new(v.ty, d)
                } else {
                    let r = self.op2("sub", &v.ty.ll(), "0", &v.op);
                    Val::new(v.ty, r)
                }
            }
            UnOp::Not => {
                let r = self.op2("xor", "i1", &v.op, "1");
                Val::new(CTy::Bool, r)
            }
            UnOp::Deref => v,
        }
    }

    fn gen_binary(&mut self, op: BinOp, a: &Expr, b: &Expr) -> Val {
        let av = self.gen_expr(a);
        let bv = self.gen_expr(b);
        let bo = self.coerce(&bv.ty, &bv.op, &av.ty);
        let is_float = av.ty.is_float();
        let ty = av.ty.clone();
        use BinOp::*;
        match op {
            Add | Sub | Mul | Div | Mod => {
                let opc = arith_opcode(op, is_float);
                let r = self.op2(opc, &ty.ll(), &av.op, &bo);
                Val::new(ty, r)
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                let (instr, cond) = cmp_opcode(op, is_float);
                let d = self.fresh();
                self.line(&format!("{d} = {instr} {cond} {} {}, {bo}", ty.ll(), av.op));
                Val::new(CTy::Bool, d)
            }
            And => {
                let r = self.op2("and", "i1", &av.op, &bo);
                Val::new(CTy::Bool, r)
            }
            Or => {
                let r = self.op2("or", "i1", &av.op, &bo);
                Val::new(CTy::Bool, r)
            }
        }
    }

    fn gen_struct_lit(&mut self, name: &str, fields: &[(String, Expr)]) -> Val {
        let ty = CTy::Struct(name.to_string());
        let mut agg = "undef".to_string();
        for (fname, fexpr) in fields {
            let Some((idx, fty)) = self.ctx.field(name, fname) else {
                continue;
            };
            let fv = self.gen_expr(fexpr);
            let op = self.coerce(&fv.ty, &fv.op, &fty);
            let d = self.fresh();
            self.line(&format!(
                "{d} = insertvalue {} {agg}, {} {op}, {idx}",
                ty.ll(),
                fty.ll()
            ));
            agg = d;
        }
        Val::new(ty, agg)
    }

    fn gen_array_lit(&mut self, elems: &[Expr], hint: Option<CTy>) -> Val {
        let vals: Vec<Val> = elems.iter().map(|e| self.gen_expr(e)).collect();
        let elem_ty = hint
            .or_else(|| vals.first().map(|v| v.ty.clone()))
            .unwrap_or(CTy::Int(64));
        let aty = CTy::Array(Box::new(elem_ty.clone()), vals.len() as u64);
        let mut agg = "undef".to_string();
        for (i, v) in vals.into_iter().enumerate() {
            let op = self.coerce(&v.ty, &v.op, &elem_ty);
            let d = self.fresh();
            self.line(&format!(
                "{d} = insertvalue {} {agg}, {} {op}, {i}",
                aty.ll(),
                elem_ty.ll()
            ));
            agg = d;
        }
        Val::new(aty, agg)
    }

    /// Builds a slice `{ ptr, len }` viewing `base[lo..hi]`.
    fn gen_slice(&mut self, base: &Expr, lo: &Expr, hi: &Expr) -> Val {
        let lov = self.gen_expr(lo);
        let lo_i = self.coerce(&lov.ty, &lov.op, &CTy::Int(64));
        let hiv = self.gen_expr(hi);
        let hi_i = self.coerce(&hiv.ty, &hiv.op, &CTy::Int(64));
        let len = self.op2("sub", "i64", &hi_i, &lo_i);
        let Some((elem, data)) = self.elem_addr(base, &lo_i) else {
            return Val::i0();
        };
        let sty = CTy::Slice(Box::new(elem));
        let a = self.fresh();
        self.line(&format!("{a} = insertvalue {{ ptr, i64 }} undef, ptr {data}, 0"));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {{ ptr, i64 }} {a}, i64 {len}, 1"));
        Val::new(sty, b)
    }

    /// Recognizes `Enum.Variant` as an enum constructor reference.
    fn as_enum_variant(&self, e: &Expr) -> Option<(String, String)> {
        let ExprKind::Field(base, v) = &e.kind else {
            return None;
        };
        let ExprKind::Ident(en) = &base.kind else {
            return None;
        };
        if self.ctx.variant(en, v).is_some() {
            Some((en.clone(), v.clone()))
        } else {
            None
        }
    }

    fn gen_enum_ctor(&mut self, ename: &str, vname: &str, args: &[Expr]) -> Val {
        let ety = CTy::Enum(ename.to_string());
        let tag_bits = self.ctx.enum_def(ename).map(|d| d.tag_bits).unwrap_or(8);
        let (tag, fields, offsets) = {
            let Some(v) = self.ctx.variant(ename, vname) else {
                return Val::i0();
            };
            let fields: Vec<CTy> = v.fields.iter().map(|(_, t)| t.clone()).collect();
            (v.tag, fields, self.ctx.offsets(v))
        };
        let slot = self.alloca(&ety);
        let tp = self.fresh();
        self.line(&format!("{tp} = getelementptr {}, ptr {slot}, i32 0, i32 0", ety.ll()));
        self.line(&format!("store i{tag_bits} {tag}, ptr {tp}"));
        if !args.is_empty() {
            let pp = self.fresh();
            self.line(&format!("{pp} = getelementptr {}, ptr {slot}, i32 0, i32 1", ety.ll()));
            for (i, a) in args.iter().enumerate() {
                let Some(fty) = fields.get(i).cloned() else {
                    self.gen_expr(a);
                    continue;
                };
                let av = self.gen_expr(a);
                let op = self.coerce(&av.ty, &av.op, &fty);
                let fp = self.field_ptr(&pp, offsets.get(i).copied().unwrap_or(0));
                self.line(&format!("store {} {op}, ptr {fp}", fty.ll()));
            }
        }
        let val = self.load(&ety, &slot);
        Val::new(ety, val)
    }

    /// Address of a payload field at `off` bytes from the blob base `pp`.
    fn field_ptr(&mut self, pp: &str, off: u64) -> String {
        if off == 0 {
            return pp.to_string();
        }
        let d = self.fresh();
        self.line(&format!("{d} = getelementptr i8, ptr {pp}, i64 {off}"));
        d
    }

    fn gen_match(&mut self, scrut: &Expr, arms: &[Arm], result: Option<(CTy, String)>) {
        let (ety, addr) = match self.gen_place(scrut) {
            Some(p) => p,
            None => {
                let v = self.gen_expr(scrut);
                let slot = self.alloca(&v.ty);
                self.line(&format!("store {} {}, ptr {slot}", v.ty.ll(), v.op));
                (v.ty, slot)
            }
        };
        let CTy::Enum(ename) = ety.clone() else {
            for arm in arms {
                if self.terminated {
                    break;
                }
                self.gen_arm_body(&arm.body, &result);
            }
            return;
        };
        let tag_bits = self.ctx.enum_def(&ename).map(|d| d.tag_bits).unwrap_or(8);
        let tp = self.fresh();
        self.line(&format!("{tp} = getelementptr {}, ptr {addr}, i32 0, i32 0", ety.ll()));
        let tag = self.fresh();
        self.line(&format!("{tag} = load i{tag_bits}, ptr {tp}"));

        let end = self.new_label();
        let mut labels = Vec::new();
        let mut cases: Vec<(u64, String)> = Vec::new();
        let mut default = end.clone();
        for arm in arms {
            let l = self.new_label();
            labels.push(l.clone());
            let tag = match &arm.pat {
                Pattern::Variant(vn, _) => self.ctx.variant(&ename, vn).map(|v| v.tag),
                Pattern::Ident(vn) => self.ctx.variant(&ename, vn).map(|v| v.tag),
                Pattern::Wildcard => None,
            };
            match tag {
                Some(t) if !cases.iter().any(|(seen, _)| *seen == t) => cases.push((t, l)),
                Some(_) => {}
                None => default = l,
            }
        }
        let cases_str = cases
            .iter()
            .map(|(t, l)| format!("i{tag_bits} {t}, label %{l}"))
            .collect::<Vec<_>>()
            .join(" ");
        self.line(&format!("switch i{tag_bits} {tag}, label %{default} [ {cases_str} ]"));
        self.terminated = true;

        for (arm, l) in arms.iter().zip(&labels) {
            self.place_label(l);
            match &arm.pat {
                Pattern::Variant(vn, binds) => self.bind_payload(&ename, vn, binds, &addr, &ety),
                Pattern::Ident(vn) if self.ctx.variant(&ename, vn).is_none() => {
                    self.locals.insert(vn.clone(), (ety.clone(), addr.clone()));
                }
                _ => {}
            }
            self.gen_arm_body(&arm.body, &result);
            if !self.terminated {
                self.br(&end);
            }
        }
        self.place_label(&end);
    }

    fn bind_payload(&mut self, ename: &str, vname: &str, binds: &[String], addr: &str, ety: &CTy) {
        if binds.is_empty() {
            return;
        }
        let (fields, offsets) = {
            let Some(v) = self.ctx.variant(ename, vname) else {
                return;
            };
            let fields: Vec<CTy> = v.fields.iter().map(|(_, t)| t.clone()).collect();
            (fields, self.ctx.offsets(v))
        };
        let pp = self.fresh();
        self.line(&format!("{pp} = getelementptr {}, ptr {addr}, i32 0, i32 1", ety.ll()));
        for (i, b) in binds.iter().enumerate() {
            let Some(fty) = fields.get(i).cloned() else {
                break;
            };
            let fp = self.field_ptr(&pp, offsets.get(i).copied().unwrap_or(0));
            self.locals.insert(b.clone(), (fty, fp));
        }
    }

    fn gen_arm_body(&mut self, body: &Block, result: &Option<(CTy, String)>) {
        let Some((rty, slot)) = result else {
            self.gen_block(&body.stmts);
            return;
        };
        let n = body.stmts.len();
        for (i, s) in body.stmts.iter().enumerate() {
            if self.terminated {
                break;
            }
            if i + 1 == n {
                if let Stmt::Expr(e) = s {
                    let v = self.gen_expr(e);
                    let op = self.coerce(&v.ty, &v.op, rty);
                    self.line(&format!("store {} {op}, ptr {slot}", rty.ll()));
                    continue;
                }
            }
            self.gen_stmt(s);
        }
    }

    /// Best effort result type for an expression form match: the first arm tail
    /// expression with a known static type.
    fn match_result_ty(&self, arms: &[Arm]) -> CTy {
        for arm in arms {
            if let Some(Stmt::Expr(e)) = arm.body.stmts.last() {
                let t = self.static_ty(e);
                if t != CTy::Unknown {
                    return t;
                }
            }
        }
        CTy::Int(64)
    }

    /// Static type of an expression without emitting IR. Covers scalars, locals,
    /// and call returns; falls back to Unknown.
    fn static_ty(&self, e: &Expr) -> CTy {
        match &e.kind {
            ExprKind::Int(_, s) => int_ty(s),
            ExprKind::Float(..) => CTy::F64,
            ExprKind::Bool(_) => CTy::Bool,
            ExprKind::Char(_) => CTy::Int(8),
            ExprKind::Str(_) => CTy::Ptr(Box::new(CTy::Int(8))),
            ExprKind::Ident(n) => self.locals.get(n).map(|(t, _)| t.clone()).unwrap_or(CTy::Unknown),
            ExprKind::Binary(op, a, _) => {
                use BinOp::*;
                match op {
                    Eq | Ne | Lt | Le | Gt | Ge | And | Or => CTy::Bool,
                    _ => self.static_ty(a),
                }
            }
            ExprKind::Unary(UnOp::Not, _) => CTy::Bool,
            ExprKind::Unary(UnOp::Neg, x) => self.static_ty(x),
            ExprKind::Call(f, _) => match &f.kind {
                ExprKind::Ident(n) => self.ctx.fns.get(n).map(|(r, _)| r.clone()).unwrap_or(CTy::Unknown),
                _ => CTy::Unknown,
            },
            ExprKind::Field(base, name) => match self.static_ty(base) {
                CTy::Struct(t) => self.ctx.field(&t, name).map(|(_, ty)| ty).unwrap_or(CTy::Unknown),
                CTy::Slice(elem) => match name.as_str() {
                    "ptr" => CTy::Ptr(elem),
                    "len" => CTy::Int(64),
                    _ => CTy::Unknown,
                },
                _ => CTy::Unknown,
            },
            ExprKind::Index(base, _) => match self.static_ty(base) {
                CTy::Array(e, _) | CTy::Slice(e) | CTy::Ptr(e) => *e,
                _ => CTy::Unknown,
            },
            ExprKind::StructLit(name, _) => CTy::Struct(name.clone()),
            _ => CTy::Unknown,
        }
    }

    fn gen_call(&mut self, f: &Expr, args: &[Expr]) -> Val {
        if let Some((en, vn)) = self.as_enum_variant(f) {
            return self.gen_enum_ctor(&en, &vn, args);
        }
        if let ExprKind::Field(base, mname) = &f.kind {
            if let Some(v) = self.gen_method_call(base, mname, args) {
                return v;
            }
        }
        if let ExprKind::Ident(name) = &f.kind {
            let local_closure = matches!(
                self.locals.get(name).map(|(t, _)| t),
                Some(CTy::Closure(..))
            );
            if local_closure {
                let cv = self.gen_load(f);
                return self.gen_closure_call(&cv, args);
            }
            // A user defined function of the same name wins over a builtin, so
            // builtin names stay paradigm agnostic and never shadow user code.
            if self.ctx.fns.contains_key(name) {
                return self.gen_user_call(name, args);
            }
            return match name.as_str() {
                "print" | "println" => self.gen_print(args),
                "alloc" => self.gen_alloc(args),
                "free" => {
                    if let Some(a) = args.first() {
                        let v = self.gen_expr(a);
                        self.gen_free_call(&v.op);
                    }
                    Val::new(CTy::Void, "")
                }
                "map" => self.gen_map(args),
                "filter" => self.gen_filter(args),
                "reduce" => self.gen_reduce(args),
                "fold" => self.gen_fold(args),
                "foreach" => self.gen_foreach(args),
                "sizeof" => self.gen_sizeof(args),
                "alloc_bytes" => self.gen_alloc_bytes(args),
                "ptr_add" => self.gen_ptr_add(args),
                _ => self.gen_user_call(name, args),
            };
        }
        let cv = self.gen_expr(f);
        if matches!(cv.ty, CTy::Closure(..)) {
            return self.gen_closure_call(&cv, args);
        }
        for a in args {
            self.gen_expr(a);
        }
        Val::i0()
    }

    fn gen_method_call(&mut self, base: &Expr, mname: &str, args: &[Expr]) -> Option<Val> {
        let bv = self.gen_expr(base);
        if let CTy::Iface(iface) = &bv.ty {
            let iface = iface.clone();
            return self.gen_dyn_dispatch(&bv, &iface, mname, args);
        }
        let (tyname, selfval) = match &bv.ty {
            CTy::Struct(t) => (t.clone(), bv.op.clone()),
            CTy::Ptr(inner) => {
                if let CTy::Struct(t) = inner.as_ref() {
                    let loaded = self.load(inner, &bv.op);
                    (t.clone(), loaded)
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        let key = format!("{tyname}.{mname}");
        let (ret, params) = self.ctx.methods.get(&key).cloned()?;
        let mut parts = vec![format!("%{tyname} {selfval}")];
        for (i, a) in args.iter().enumerate() {
            let v = self.gen_expr(a);
            let target = params.get(i + 1).cloned().unwrap_or(v.ty.clone());
            let op = self.coerce(&v.ty, &v.op, &target);
            parts.push(format!("{} {op}", target.ll()));
        }
        let argstr = parts.join(", ");
        if matches!(ret, CTy::Void) {
            self.line(&format!("call void @{tyname}.{mname}({argstr})"));
            return Some(Val::new(CTy::Void, ""));
        }
        let d = self.fresh();
        self.line(&format!("{d} = call {} @{tyname}.{mname}({argstr})", ret.ll()));
        Some(Val::new(ret, d))
    }

    /// Dynamic dispatch through an interface fat pointer: load the method slot
    /// from the vtable and call it indirectly with the data pointer as receiver.
    fn gen_dyn_dispatch(
        &mut self,
        iv: &Val,
        iface: &str,
        mname: &str,
        args: &[Expr],
    ) -> Option<Val> {
        let (idx, ret, params, n) = {
            let (idx, m) = self.ctx.iface_method(iface, mname)?;
            let n = self.ctx.iface(iface).map(|i| i.methods.len()).unwrap_or(0);
            (idx, m.ret.clone(), m.params.clone(), n)
        };
        let data = self.fresh();
        self.line(&format!("{data} = extractvalue {{ ptr, ptr }} {}, 0", iv.op));
        let vt = self.fresh();
        self.line(&format!("{vt} = extractvalue {{ ptr, ptr }} {}, 1", iv.op));
        let slot = self.fresh();
        self.line(&format!(
            "{slot} = getelementptr [{n} x ptr], ptr {vt}, i64 0, i64 {idx}"
        ));
        let fp = self.fresh();
        self.line(&format!("{fp} = load ptr, ptr {slot}"));
        let mut parts = vec![format!("ptr {data}")];
        for (i, a) in args.iter().enumerate() {
            let v = self.gen_expr(a);
            let target = params.get(i).cloned().unwrap_or(v.ty.clone());
            let av = self.adapt(v, &target);
            parts.push(format!("{} {}", target.ll(), av.op));
        }
        let argstr = parts.join(", ");
        if matches!(ret, CTy::Void) {
            self.line(&format!("call void {fp}({argstr})"));
            return Some(Val::new(CTy::Void, ""));
        }
        let d = self.fresh();
        self.line(&format!("{d} = call {} {fp}({argstr})", ret.ll()));
        Some(Val::new(ret, d))
    }

    fn gen_user_call(&mut self, name: &str, args: &[Expr]) -> Val {
        let Some((ret, params)) = self.ctx.fns.get(name).cloned() else {
            for a in args {
                self.gen_expr(a);
            }
            return Val::i0();
        };
        let mut parts = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let v = self.gen_expr(a);
            let target = params.get(i).cloned().unwrap_or(v.ty.clone());
            let av = self.adapt(v, &target);
            parts.push(format!("{} {}", target.ll(), av.op));
        }
        let argstr = parts.join(", ");
        if matches!(ret, CTy::Void) {
            self.line(&format!("call void @{name}({argstr})"));
            return Val::new(CTy::Void, "");
        }
        let d = self.fresh();
        self.line(&format!("{d} = call {} @{name}({argstr})", ret.ll()));
        Val::new(ret, d)
    }

    fn alloca_raw(&mut self, llty: &str) -> String {
        let d = self.fresh();
        self.line(&format!("{d} = alloca {llty}"));
        d
    }

    /// Free variables of a lambda that are bound in the enclosing function, with
    /// their lowered types. These become the captured environment.
    fn lambda_captures(&self, l: &Lambda) -> Vec<(String, CTy)> {
        let mut used = Vec::new();
        let mut bound: HashSet<String> = l.params.iter().map(|p| p.name.clone()).collect();
        collect_block(&l.body, &mut used, &mut bound);
        let mut caps = Vec::new();
        let mut seen = HashSet::new();
        for n in used {
            if bound.contains(&n) || !seen.insert(n.clone()) {
                continue;
            }
            if let Some((ty, _)) = self.locals.get(&n) {
                caps.push((n, ty.clone()));
            }
        }
        caps
    }

    fn gen_lambda(&mut self, l: &Lambda) -> Val {
        let caps = self.lambda_captures(l);
        let params: Vec<(String, CTy)> = l
            .params
            .iter()
            .map(|p| (p.name.clone(), lower_ty(&p.ty, &|n| self.ctx.nom(n))))
            .collect();
        let ret = lower_ty(&l.ret, &|n| self.ctx.nom(n));
        let id = self.m.fresh_lambda();
        let fname = format!("@lambda.{id}");
        let env_ty = format!(
            "{{ {} }}",
            caps.iter().map(|(_, t)| t.ll()).collect::<Vec<_>>().join(", ")
        );
        let env = if caps.is_empty() {
            "null".to_string()
        } else {
            let e = self.alloca_raw(&env_ty);
            for (i, (cname, cty)) in caps.iter().enumerate() {
                let (lty, lptr) = self.locals.get(cname).cloned().unwrap();
                let v = self.load(&lty, &lptr);
                let slot = self.fresh();
                self.line(&format!("{slot} = getelementptr {env_ty}, ptr {e}, i32 0, i32 {i}"));
                self.line(&format!("store {} {v}, ptr {slot}", cty.ll()));
            }
            e
        };
        self.emit_lambda_fn(&fname, &env_ty, &caps, &params, &ret, &l.body);
        let a = self.fresh();
        self.line(&format!("{a} = insertvalue {{ ptr, ptr }} undef, ptr {env}, 0"));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {{ ptr, ptr }} {a}, ptr {fname}, 1"));
        let pt = params.iter().map(|(_, t)| t.clone()).collect();
        Val::new(CTy::Closure(pt, Box::new(ret)), b)
    }

    fn emit_lambda_fn(
        &mut self,
        fname: &str,
        env_ty: &str,
        caps: &[(String, CTy)],
        params: &[(String, CTy)],
        ret: &CTy,
        body: &Block,
    ) {
        let sig = std::iter::once("ptr %env".to_string())
            .chain(params.iter().enumerate().map(|(i, (_, t))| format!("{} %a{i}", t.ll())))
            .collect::<Vec<_>>()
            .join(", ");
        let header = format!("define {} {fname}({sig}) {{", ret.ll());
        let mut fb = Fb::new(&mut *self.m, self.ctx, ret.clone());
        fb.body.push_str("entry:\n");
        for (i, (cname, cty)) in caps.iter().enumerate() {
            let slot = fb.fresh();
            fb.line(&format!("{slot} = getelementptr {env_ty}, ptr %env, i32 0, i32 {i}"));
            let v = fb.load(cty, &slot);
            let p = fb.alloca(cty);
            fb.line(&format!("store {} {v}, ptr {p}", cty.ll()));
            fb.locals.insert(cname.clone(), (cty.clone(), p));
        }
        for (i, (pname, ty)) in params.iter().enumerate() {
            let p = fb.alloca(ty);
            fb.line(&format!("store {} %a{i}, ptr {p}", ty.ll()));
            fb.locals.insert(pname.clone(), (ty.clone(), p));
        }
        fb.gen_block(&body.stmts);
        if !fb.terminated {
            fb.emit_defers();
            fb.default_ret();
        }
        let def = format!("{header}\n{}}}", fb.body);
        self.m.push_function(def);
    }

    fn gen_closure_call(&mut self, cv: &Val, args: &[Expr]) -> Val {
        let vals: Vec<Val> = args.iter().map(|a| self.gen_expr(a)).collect();
        self.invoke_closure(cv, vals)
    }

    /// Calls a closure value with already evaluated arguments. Extracts the
    /// environment and function pointer, then dispatches indirectly.
    fn invoke_closure(&mut self, cv: &Val, arg_vals: Vec<Val>) -> Val {
        let (params, ret) = match &cv.ty {
            CTy::Closure(p, r) => (p.clone(), (**r).clone()),
            _ => (Vec::new(), CTy::Int(64)),
        };
        let env = self.fresh();
        self.line(&format!("{env} = extractvalue {{ ptr, ptr }} {}, 0", cv.op));
        let fp = self.fresh();
        self.line(&format!("{fp} = extractvalue {{ ptr, ptr }} {}, 1", cv.op));
        let mut parts = vec![format!("ptr {env}")];
        for (i, v) in arg_vals.into_iter().enumerate() {
            let target = params.get(i).cloned().unwrap_or(v.ty.clone());
            let av = self.adapt(v, &target);
            parts.push(format!("{} {}", target.ll(), av.op));
        }
        let argstr = parts.join(", ");
        if matches!(ret, CTy::Void) {
            self.line(&format!("call void {fp}({argstr})"));
            return Val::new(CTy::Void, "");
        }
        let d = self.fresh();
        self.line(&format!("{d} = call {} {fp}({argstr})", ret.ll()));
        Val::new(ret, d)
    }

    /// Wraps an in-memory array aggregate as a slice `{ ptr, len }` of length n,
    /// spilling it to a stack slot first.
    fn slice_from_array(&mut self, arr: Val, n: usize) -> Val {
        let elem = match &arr.ty {
            CTy::Array(e, _) => (**e).clone(),
            _ => CTy::Int(64),
        };
        let slot = self.alloca(&arr.ty);
        self.line(&format!("store {} {}, ptr {slot}", arr.ty.ll(), arr.op));
        let a = self.fresh();
        self.line(&format!("{a} = insertvalue {{ ptr, i64 }} undef, ptr {slot}, 0"));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {{ ptr, i64 }} {a}, i64 {n}, 1"));
        Val::new(CTy::Slice(Box::new(elem)), b)
    }

    /// Materializes an array literal on the stack and views it as a slice, using
    /// the given element type as the layout hint.
    fn array_to_slice(&mut self, elems: &[Expr], elem: CTy) -> Val {
        let arr = self.gen_array_lit(elems, Some(elem));
        self.slice_from_array(arr, elems.len())
    }

    /// Lowers a collection argument for a functional builtin. A bare array
    /// literal is materialized on the stack and viewed as a slice; anything
    /// else (a slice-typed value) is lowered normally.
    fn gen_collection(&mut self, arg: &Expr) -> Val {
        if let ExprKind::Array(elems) = &arg.kind {
            let arr = self.gen_array_lit(elems, None);
            return self.slice_from_array(arr, elems.len());
        }
        self.gen_expr(arg)
    }

    /// Extracts the data pointer, length, and element type from a slice value.
    fn slice_parts(&mut self, sv: &Val) -> (String, String, CTy) {
        let elem = match &sv.ty {
            CTy::Slice(e) => (**e).clone(),
            _ => CTy::Int(64),
        };
        let data = self.fresh();
        self.line(&format!("{data} = extractvalue {{ ptr, i64 }} {}, 0", sv.op));
        let len = self.fresh();
        self.line(&format!("{len} = extractvalue {{ ptr, i64 }} {}, 1", sv.op));
        (data, len, elem)
    }

    /// Size in bytes of one element of `ty`, via a null pointer GEP and ptrtoint.
    fn elem_size(&mut self, ty: &CTy) -> String {
        let p = self.fresh();
        self.line(&format!("{p} = getelementptr {}, ptr null, i64 1", ty.ll()));
        let s = self.fresh();
        self.line(&format!("{s} = ptrtoint ptr {p} to i64"));
        s
    }

    /// foreach(xs, f): applies the closure to each element, for its side effects.
    fn gen_foreach(&mut self, args: &[Expr]) -> Val {
        if args.len() < 2 {
            return Val::new(CTy::Void, "");
        }
        let sv = self.gen_collection(&args[0]);
        let cv = self.gen_expr(&args[1]);
        let (data, len, elem) = self.slice_parts(&sv);
        let i = self.alloca_raw("i64");
        self.line(&format!("store i64 0, ptr {i}"));
        let cond = self.new_label();
        let body = self.new_label();
        let end = self.new_label();
        self.br(&cond);
        self.place_label(&cond);
        let iv = self.load(&CTy::Int(64), &i);
        let c = self.fresh();
        self.line(&format!("{c} = icmp slt i64 {iv}, {len}"));
        self.cond_br(&c, &body, &end);
        self.place_label(&body);
        let ep = self.fresh();
        self.line(&format!("{ep} = getelementptr {}, ptr {data}, i64 {iv}", elem.ll()));
        let ev = self.load(&elem, &ep);
        self.invoke_closure(&cv, vec![Val::new(elem.clone(), ev)]);
        let ni = self.op2("add", "i64", &iv, "1");
        self.line(&format!("store i64 {ni}, ptr {i}"));
        self.br(&cond);
        self.place_label(&end);
        Val::new(CTy::Void, "")
    }

    /// map(xs, f): a new heap slice holding f applied to each element.
    fn gen_map(&mut self, args: &[Expr]) -> Val {
        if args.len() < 2 {
            return Val::i0();
        }
        let sv = self.gen_collection(&args[0]);
        let cv = self.gen_expr(&args[1]);
        let (data, len, elem) = self.slice_parts(&sv);
        let out_elem = match &cv.ty {
            CTy::Closure(_, r) => (**r).clone(),
            _ => elem.clone(),
        };
        let esz = self.elem_size(&out_elem);
        let total = self.op2("mul", "i64", &len, &esz);
        let align = self.ctx.size_align(&out_elem).1;
        let out = self.gen_alloc_call(&total, align);
        let i = self.alloca_raw("i64");
        self.line(&format!("store i64 0, ptr {i}"));
        let cond = self.new_label();
        let body = self.new_label();
        let end = self.new_label();
        self.br(&cond);
        self.place_label(&cond);
        let iv = self.load(&CTy::Int(64), &i);
        let c = self.fresh();
        self.line(&format!("{c} = icmp slt i64 {iv}, {len}"));
        self.cond_br(&c, &body, &end);
        self.place_label(&body);
        let ep = self.fresh();
        self.line(&format!("{ep} = getelementptr {}, ptr {data}, i64 {iv}", elem.ll()));
        let ev = self.load(&elem, &ep);
        let rv = self.invoke_closure(&cv, vec![Val::new(elem.clone(), ev)]);
        let rop = self.coerce(&rv.ty, &rv.op, &out_elem);
        let op = self.fresh();
        self.line(&format!("{op} = getelementptr {}, ptr {out}, i64 {iv}", out_elem.ll()));
        self.line(&format!("store {} {rop}, ptr {op}", out_elem.ll()));
        let ni = self.op2("add", "i64", &iv, "1");
        self.line(&format!("store i64 {ni}, ptr {i}"));
        self.br(&cond);
        self.place_label(&end);
        let sty = CTy::Slice(Box::new(out_elem));
        let a = self.fresh();
        self.line(&format!("{a} = insertvalue {{ ptr, i64 }} undef, ptr {out}, 0"));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {{ ptr, i64 }} {a}, i64 {len}, 1"));
        Val::new(sty, b)
    }

    /// filter(xs, pred): a new heap slice of the elements where pred is true.
    fn gen_filter(&mut self, args: &[Expr]) -> Val {
        if args.len() < 2 {
            return Val::i0();
        }
        let sv = self.gen_collection(&args[0]);
        let cv = self.gen_expr(&args[1]);
        let (data, len, elem) = self.slice_parts(&sv);
        let esz = self.elem_size(&elem);
        let total = self.op2("mul", "i64", &len, &esz);
        let align = self.ctx.size_align(&elem).1;
        let out = self.gen_alloc_call(&total, align);
        let cnt = self.alloca_raw("i64");
        self.line(&format!("store i64 0, ptr {cnt}"));
        let i = self.alloca_raw("i64");
        self.line(&format!("store i64 0, ptr {i}"));
        let cond = self.new_label();
        let body = self.new_label();
        let keep = self.new_label();
        let next = self.new_label();
        let end = self.new_label();
        self.br(&cond);
        self.place_label(&cond);
        let iv = self.load(&CTy::Int(64), &i);
        let c = self.fresh();
        self.line(&format!("{c} = icmp slt i64 {iv}, {len}"));
        self.cond_br(&c, &body, &end);
        self.place_label(&body);
        let ep = self.fresh();
        self.line(&format!("{ep} = getelementptr {}, ptr {data}, i64 {iv}", elem.ll()));
        let ev = self.load(&elem, &ep);
        let pv = self.invoke_closure(&cv, vec![Val::new(elem.clone(), ev.clone())]);
        let cb = self.coerce(&pv.ty, &pv.op, &CTy::Bool);
        self.cond_br(&cb, &keep, &next);
        self.place_label(&keep);
        let kcnt = self.load(&CTy::Int(64), &cnt);
        let op = self.fresh();
        self.line(&format!("{op} = getelementptr {}, ptr {out}, i64 {kcnt}", elem.ll()));
        self.line(&format!("store {} {ev}, ptr {op}", elem.ll()));
        let ncnt = self.op2("add", "i64", &kcnt, "1");
        self.line(&format!("store i64 {ncnt}, ptr {cnt}"));
        self.br(&next);
        self.place_label(&next);
        let ni = self.op2("add", "i64", &iv, "1");
        self.line(&format!("store i64 {ni}, ptr {i}"));
        self.br(&cond);
        self.place_label(&end);
        let total_cnt = self.load(&CTy::Int(64), &cnt);
        let sty = CTy::Slice(Box::new(elem));
        let a = self.fresh();
        self.line(&format!("{a} = insertvalue {{ ptr, i64 }} undef, ptr {out}, 0"));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {{ ptr, i64 }} {a}, i64 {total_cnt}, 1"));
        Val::new(sty, b)
    }

    /// fold(xs, init, f): threads an accumulator left to right through f(acc, x).
    fn gen_fold(&mut self, args: &[Expr]) -> Val {
        if args.len() < 3 {
            return Val::i0();
        }
        let sv = self.gen_collection(&args[0]);
        let init = self.gen_expr(&args[1]);
        let cv = self.gen_expr(&args[2]);
        let (data, len, elem) = self.slice_parts(&sv);
        let acc_ty = match &cv.ty {
            CTy::Closure(_, r) => (**r).clone(),
            _ => init.ty.clone(),
        };
        let acc = self.alloca(&acc_ty);
        let iv0 = self.coerce(&init.ty, &init.op, &acc_ty);
        self.line(&format!("store {} {iv0}, ptr {acc}", acc_ty.ll()));
        let i = self.alloca_raw("i64");
        self.line(&format!("store i64 0, ptr {i}"));
        let cond = self.new_label();
        let body = self.new_label();
        let end = self.new_label();
        self.br(&cond);
        self.place_label(&cond);
        let iv = self.load(&CTy::Int(64), &i);
        let c = self.fresh();
        self.line(&format!("{c} = icmp slt i64 {iv}, {len}"));
        self.cond_br(&c, &body, &end);
        self.place_label(&body);
        let ep = self.fresh();
        self.line(&format!("{ep} = getelementptr {}, ptr {data}, i64 {iv}", elem.ll()));
        let ev = self.load(&elem, &ep);
        let av = self.load(&acc_ty, &acc);
        let rv = self.invoke_closure(
            &cv,
            vec![Val::new(acc_ty.clone(), av), Val::new(elem.clone(), ev)],
        );
        let rop = self.coerce(&rv.ty, &rv.op, &acc_ty);
        self.line(&format!("store {} {rop}, ptr {acc}", acc_ty.ll()));
        let ni = self.op2("add", "i64", &iv, "1");
        self.line(&format!("store i64 {ni}, ptr {i}"));
        self.br(&cond);
        self.place_label(&end);
        let r = self.load(&acc_ty, &acc);
        Val::new(acc_ty, r)
    }

    /// reduce(xs, f): folds with the first element as the seed, over the rest.
    fn gen_reduce(&mut self, args: &[Expr]) -> Val {
        if args.len() < 2 {
            return Val::i0();
        }
        let sv = self.gen_collection(&args[0]);
        let cv = self.gen_expr(&args[1]);
        let (data, len, elem) = self.slice_parts(&sv);
        let acc = self.alloca(&elem);
        let e0v = self.load(&elem, &data);
        self.line(&format!("store {} {e0v}, ptr {acc}", elem.ll()));
        let i = self.alloca_raw("i64");
        self.line(&format!("store i64 1, ptr {i}"));
        let cond = self.new_label();
        let body = self.new_label();
        let end = self.new_label();
        self.br(&cond);
        self.place_label(&cond);
        let iv = self.load(&CTy::Int(64), &i);
        let c = self.fresh();
        self.line(&format!("{c} = icmp slt i64 {iv}, {len}"));
        self.cond_br(&c, &body, &end);
        self.place_label(&body);
        let ep = self.fresh();
        self.line(&format!("{ep} = getelementptr {}, ptr {data}, i64 {iv}", elem.ll()));
        let ev = self.load(&elem, &ep);
        let av = self.load(&elem, &acc);
        let rv = self.invoke_closure(
            &cv,
            vec![Val::new(elem.clone(), av), Val::new(elem.clone(), ev)],
        );
        let rop = self.coerce(&rv.ty, &rv.op, &elem);
        self.line(&format!("store {} {rop}, ptr {acc}", elem.ll()));
        let ni = self.op2("add", "i64", &iv, "1");
        self.line(&format!("store i64 {ni}, ptr {i}"));
        self.br(&cond);
        self.place_label(&end);
        let r = self.load(&elem, &acc);
        Val::new(elem, r)
    }

    fn gen_print(&mut self, args: &[Expr]) -> Val {
        if let Some(a) = args.first() {
            let v = self.gen_expr(a);
            match &v.ty {
                CTy::Ptr(_) => self.line(&format!("call void @cool_println_cstr(ptr {})", v.op)),
                CTy::F64 | CTy::F32 => {
                    let d = self.coerce(&v.ty, &v.op, &CTy::F64);
                    self.line(&format!("call void @cool_print_f64(double {d})"));
                }
                CTy::Struct(_) | CTy::Enum(_) | CTy::Slice(_) | CTy::Array(..) | CTy::Void
                | CTy::Unknown => {}
                _ => {
                    let d = self.coerce(&v.ty, &v.op, &CTy::Int(64));
                    self.line(&format!("call void @cool_print_i64(i64 {d})"));
                }
            }
        }
        Val::new(CTy::Void, "")
    }

    /// sizeof(x): the byte size of x's type. The argument may be a value
    /// expression or a bare type name such as `int64` or a struct name.
    fn gen_sizeof(&mut self, args: &[Expr]) -> Val {
        let ty = match args.first() {
            Some(e) => match &e.kind {
                ExprKind::Ident(n) if !self.locals.contains_key(n) => {
                    lower_ty(&Type::Named(n.clone(), Vec::new()), &|x| self.ctx.nom(x))
                }
                _ => self.static_ty(e),
            },
            None => CTy::Int(64),
        };
        let sz = self.elem_size(&ty);
        Val::new(CTy::Int(64), sz)
    }

    /// alloc_bytes(n): raw, uninitialized n bytes through the in scope allocator,
    /// returned as `*int8`. The base primitive for arenas and growable buffers.
    fn gen_alloc_bytes(&mut self, args: &[Expr]) -> Val {
        let n = args.first().map(|a| self.gen_expr(a)).unwrap_or_else(Val::i0);
        let ni = self.coerce(&n.ty, &n.op, &CTy::Int(64));
        let p = self.gen_alloc_call(&ni, 8);
        Val::new(CTy::Ptr(Box::new(CTy::Int(8))), p)
    }

    /// ptr_add(p, n): the pointer n bytes past p, keeping p's pointer type. Raw
    /// byte arithmetic for arenas and buffers.
    fn gen_ptr_add(&mut self, args: &[Expr]) -> Val {
        let p = args.first().map(|a| self.gen_expr(a)).unwrap_or_else(Val::i0);
        let n = args.get(1).map(|a| self.gen_expr(a)).unwrap_or_else(Val::i0);
        let ni = self.coerce(&n.ty, &n.op, &CTy::Int(64));
        let ty = match &p.ty {
            CTy::Ptr(_) => p.ty.clone(),
            _ => CTy::Ptr(Box::new(CTy::Int(8))),
        };
        let d = self.fresh();
        self.line(&format!("{d} = getelementptr i8, ptr {}, i64 {ni}", p.op));
        Val::new(ty, d)
    }

    fn gen_alloc(&mut self, args: &[Expr]) -> Val {
        let value = args.first().map(|a| self.gen_expr(a));
        let pointee = value.as_ref().map(|v| v.ty.clone()).unwrap_or(CTy::Int(64));
        let align = self.ctx.size_align(&pointee).1;
        let szp = self.fresh();
        self.line(&format!("{szp} = getelementptr {}, ptr null, i64 1", pointee.ll()));
        let sz = self.fresh();
        self.line(&format!("{sz} = ptrtoint ptr {szp} to i64"));
        let p = self.gen_alloc_call(&sz, align);
        if let Some(v) = value {
            self.line(&format!("store {} {}, ptr {p}", pointee.ll(), v.op));
        }
        Val::new(CTy::Ptr(Box::new(pointee)), p)
    }

    /// Allocates `size` bytes through the in scope `using` allocator, dispatching
    /// statically on a concrete allocator type and through the vtable on an
    /// interface erased one. With no allocator in scope, uses the default heap.
    fn gen_alloc_call(&mut self, size: &str, align: u64) -> String {
        match self.allocator.clone() {
            Some((name, CTy::Struct(a))) => {
                let (_, lp) = self.locals.get(&name).cloned().unwrap();
                let av = self.load(&CTy::Struct(a.clone()), &lp);
                let p = self.fresh();
                self.line(&format!(
                    "{p} = call ptr @{a}.alloc(%{a} {av}, i64 {size}, i64 {align})"
                ));
                p
            }
            Some((name, CTy::Iface(i))) => {
                let (_, lp) = self.locals.get(&name).cloned().unwrap();
                let iv = self.load(&CTy::Iface(i.clone()), &lp);
                if let Some((data, fp)) = self.iface_slot(&iv, &i, "alloc") {
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = call ptr {fp}(ptr {data}, i64 {size}, i64 {align})"
                    ));
                    p
                } else {
                    let p = self.fresh();
                    self.line(&format!("{p} = call ptr @cool_alloc(i64 {size})"));
                    p
                }
            }
            _ => {
                let p = self.fresh();
                self.line(&format!("{p} = call ptr @cool_alloc(i64 {size})"));
                p
            }
        }
    }

    /// Frees a pointer through the in scope `using` allocator, or the default
    /// heap when none is in scope.
    fn gen_free_call(&mut self, p: &str) {
        match self.allocator.clone() {
            Some((name, CTy::Struct(a))) => {
                let (_, lp) = self.locals.get(&name).cloned().unwrap();
                let av = self.load(&CTy::Struct(a.clone()), &lp);
                self.line(&format!("call void @{a}.free(%{a} {av}, ptr {p})"));
            }
            Some((name, CTy::Iface(i))) => {
                let (_, lp) = self.locals.get(&name).cloned().unwrap();
                let iv = self.load(&CTy::Iface(i.clone()), &lp);
                if let Some((data, fp)) = self.iface_slot(&iv, &i, "free") {
                    self.line(&format!("call void {fp}(ptr {data}, ptr {p})"));
                } else {
                    self.line(&format!("call void @cool_free(ptr {p})"));
                }
            }
            _ => self.line(&format!("call void @cool_free(ptr {p})")),
        }
    }

    /// Loads an interface method slot, returning the data pointer and function
    /// pointer for an indirect call.
    fn iface_slot(&mut self, iv: &str, iface: &str, method: &str) -> Option<(String, String)> {
        let (idx, n) = {
            let (idx, _) = self.ctx.iface_method(iface, method)?;
            (idx, self.ctx.iface(iface)?.methods.len())
        };
        let data = self.fresh();
        self.line(&format!("{data} = extractvalue {{ ptr, ptr }} {iv}, 0"));
        let vt = self.fresh();
        self.line(&format!("{vt} = extractvalue {{ ptr, ptr }} {iv}, 1"));
        let slot = self.fresh();
        self.line(&format!("{slot} = getelementptr [{n} x ptr], ptr {vt}, i64 0, i64 {idx}"));
        let fp = self.fresh();
        self.line(&format!("{fp} = load ptr, ptr {slot}"));
        Some((data, fp))
    }
}

fn arith_opcode(op: BinOp, is_float: bool) -> &'static str {
    use BinOp::*;
    match (op, is_float) {
        (Add, false) => "add",
        (Sub, false) => "sub",
        (Mul, false) => "mul",
        (Div, false) => "sdiv",
        (Mod, false) => "srem",
        (Add, true) => "fadd",
        (Sub, true) => "fsub",
        (Mul, true) => "fmul",
        (Div, true) => "fdiv",
        (Mod, true) => "frem",
        _ => "add",
    }
}

fn cmp_opcode(op: BinOp, is_float: bool) -> (&'static str, &'static str) {
    use BinOp::*;
    if is_float {
        let c = match op {
            Eq => "oeq",
            Ne => "one",
            Lt => "olt",
            Le => "ole",
            Gt => "ogt",
            Ge => "oge",
            _ => "oeq",
        };
        ("fcmp", c)
    } else {
        let c = match op {
            Eq => "eq",
            Ne => "ne",
            Lt => "slt",
            Le => "sle",
            Gt => "sgt",
            Ge => "sge",
            _ => "eq",
        };
        ("icmp", c)
    }
}

fn int_ty(suffix: &Option<String>) -> CTy {
    match suffix.as_deref() {
        Some("i8") | Some("u8") => CTy::Int(8),
        Some("i16") | Some("u16") => CTy::Int(16),
        Some("i32") | Some("u32") => CTy::Int(32),
        _ => CTy::Int(64),
    }
}

fn collect_block(b: &Block, used: &mut Vec<String>, bound: &mut HashSet<String>) {
    for s in &b.stmts {
        collect_stmt(s, used, bound);
    }
}

fn collect_stmt(s: &Stmt, used: &mut Vec<String>, bound: &mut HashSet<String>) {
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

fn collect_match(m: &ast::Match, used: &mut Vec<String>, bound: &mut HashSet<String>) {
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

fn collect_expr(e: &Expr, used: &mut Vec<String>, bound: &mut HashSet<String>) {
    match &e.kind {
        ExprKind::Ident(n) => used.push(n.clone()),
        ExprKind::Unary(_, x) => collect_expr(x, used, bound),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b) => {
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
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn ir(src: &str) -> String {
        let (t, _) = lex(src);
        let (m, e) = parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        compile(&m)
    }

    #[test]
    fn scalar_core_still_works() {
        let out = ir(
            "func add(a: int64, b: int64) -> int64 { return a + b }\n\
             func main() -> int32 { return add(1, 2) }",
        );
        assert!(out.contains("define i64 @add(i64 %a0, i64 %a1)"));
        assert!(out.contains("call i64 @add"));
    }

    #[test]
    fn struct_type_and_method() {
        let out = ir(
            "struct Point { x: int64, y: int64 }\n\
             impl Point { func sum() -> int64 { return self.x + self.y } }\n\
             func main() -> int32 {\n  p := Point { x: 3, y: 4 }\n  return 0\n}",
        );
        assert!(out.contains("%Point = type { i64, i64 }"));
        assert!(out.contains("define i64 @Point.sum(%Point %a0)"));
        assert!(out.contains("insertvalue %Point"));
    }

    #[test]
    fn alloc_and_deref() {
        let out = ir("func f() -> int64 {\n  q: *int64 = alloc(100)\n  return *q\n}");
        assert!(out.contains("call ptr @cool_alloc"));
        assert!(out.contains("getelementptr i64, ptr null"));
    }

    #[test]
    fn array_literal_and_index() {
        let out = ir(
            "func f() -> int32 {\n  xs: int32[3] = [1, 2, 3]\n  return xs[1]\n}",
        );
        assert!(out.contains("alloca [3 x i32]"));
        assert!(out.contains("insertvalue [3 x i32]"));
        assert!(out.contains("getelementptr [3 x i32], ptr"));
    }

    #[test]
    fn slice_from_range_and_len() {
        let out = ir(
            "func f() -> int64 {\n  xs: int32[4] = [1, 2, 3, 4]\n  s: int32[] = xs[1..3]\n  return s.len\n}",
        );
        assert!(out.contains("insertvalue { ptr, i64 } undef, ptr"));
        assert!(out.contains("insertvalue { ptr, i64 }"));
        assert!(out.contains("extractvalue { ptr, i64 }"));
    }

    #[test]
    fn element_store() {
        let out = ir(
            "func f() -> int32 {\n  mut xs: int32[2] = [1, 2]\n  xs[0] = 9\n  return xs[0]\n}",
        );
        assert!(out.contains("store i32 "));
        assert!(out.contains("getelementptr [2 x i32], ptr"));
    }

    #[test]
    fn enum_type_and_construct() {
        let out = ir(
            "enum E { A, B(v: int64) }\n\
             func f() -> int64 {\n  x := E.B(7)\n  return 0\n}",
        );
        assert!(out.contains("%E = type { i8, [1 x i64] }"));
        assert!(out.contains("store i8 1"));
    }

    #[test]
    fn match_lowers_to_switch() {
        let out = ir(
            "enum E { A, B(v: int64) }\n\
             func f(e: E) -> int64 {\n  match e {\n    A => return 0,\n    B(n) => return n,\n  }\n}",
        );
        assert!(out.contains("switch i8"));
        assert!(out.contains("getelementptr %E, ptr"));
    }

    #[test]
    fn interface_vtable_and_dispatch() {
        let out = ir(
            "struct Dog { s: int64 }\n\
             interface Animal { speak() -> int64 }\n\
             impl Animal for Dog { func speak() -> int64 { return self.s } }\n\
             func describe(a: Animal) -> int64 { return a.speak() }\n\
             func main() -> int32 {\n  d := Dog { s: 7 }\n  return describe(d)\n}",
        );
        assert!(out.contains("@vtable.Animal.Dog = constant [1 x ptr]"));
        assert!(out.contains("define i64 @thunk.Animal.Dog.speak(ptr %d"));
        assert!(out.contains("extractvalue { ptr, ptr }"));
    }

    #[test]
    fn struct_boxes_into_interface_arg() {
        let out = ir(
            "struct Dog { s: int64 }\n\
             interface Animal { speak() -> int64 }\n\
             impl Animal for Dog { func speak() -> int64 { return self.s } }\n\
             func describe(a: Animal) -> int64 { return a.speak() }\n\
             func main() -> int32 {\n  d := Dog { s: 7 }\n  return describe(d)\n}",
        );
        assert!(out.contains("insertvalue { ptr, ptr } undef, ptr"));
        assert!(out.contains("ptr @vtable.Animal.Dog, 1"));
    }

    #[test]
    fn closure_captures_and_calls() {
        let out = ir(
            "func main() -> int32 {\n\
               base := 100\n\
               add := lambda (x: int64) -> int64 { return x + base }\n\
               return add(5)\n\
             }",
        );
        assert!(out.contains("define i64 @lambda.0(ptr %env, i64 %a0)"));
        assert!(out.contains("getelementptr { i64 }, ptr %env"));
        assert!(out.contains("insertvalue { ptr, ptr } undef, ptr"));
    }

    #[test]
    fn closure_without_capture_uses_null_env() {
        let out = ir(
            "func main() -> int32 {\n\
               f := lambda (a: int64, b: int64) -> int64 { return a * b }\n\
               return f(6, 7)\n\
             }",
        );
        assert!(out.contains("define i64 @lambda.0(ptr %env, i64 %a0, i64 %a1)"));
        assert!(out.contains("insertvalue { ptr, ptr } undef, ptr null, 0"));
    }

    #[test]
    fn using_allocator_dispatches_statically() {
        let out = ir(
            "struct Heap { id: int64 }\n\
             func work(using a: Heap) -> void {\n  p: *int64 = alloc(5)\n  free(p)\n}",
        );
        assert!(out.contains("@Heap.alloc(%Heap"), "{out}");
        assert!(out.contains("@Heap.free(%Heap"), "{out}");
        assert!(!out.contains("call ptr @cool_alloc"), "should not fall back to heap: {out}");
    }

    #[test]
    fn default_alloc_uses_heap() {
        let out = ir("func f() -> int64 {\n  p: *int64 = alloc(5)\n  return *p\n}");
        assert!(out.contains("call ptr @cool_alloc"));
    }

    #[test]
    fn array_literal_lowers_to_slice() {
        let out = ir("func f() -> void {\n  xs: int64[] = [1, 2, 3]\n}");
        assert!(out.contains("alloca [3 x i64]"));
        assert!(out.contains("insertvalue { ptr, i64 } undef, ptr"));
        assert!(out.contains("insertvalue { ptr, i64 }") && out.contains(", i64 3, 1"));
    }

    #[test]
    fn map_allocates_and_loops() {
        let out = ir(
            "func f(xs: int64[]) -> void {\n  ys := map(xs, lambda (n: int64) -> int64 { return n })\n}",
        );
        assert!(out.contains("call ptr @cool_alloc"), "map allocates result: {out}");
        assert!(out.contains("icmp slt i64"), "map loops: {out}");
        assert!(out.contains("call i64 "), "map invokes the closure indirectly: {out}");
    }

    #[test]
    fn foreach_calls_closure_per_element() {
        let out = ir(
            "func f(xs: int64[]) -> void {\n  foreach(xs, lambda (n: int64) -> void { println(n) })\n}",
        );
        assert!(out.contains("icmp slt i64"));
        assert!(out.contains("call void @cool_print_i64"));
    }

    #[test]
    fn builtin_accepts_array_literal_collection() {
        let out = ir(
            "func f() -> void {\n  foreach([1, 2, 3], lambda (n: int64) -> void { println(n) })\n}",
        );
        // The literal is spilled to a stack array and viewed as a slice.
        assert!(out.contains("alloca [3 x i64]"), "{out}");
        assert!(out.contains("insertvalue { ptr, i64 } undef, ptr"), "{out}");
    }

    #[test]
    fn sizeof_and_alloc_bytes() {
        let out = ir(
            "func f() -> void {\n  n := sizeof(int64)\n  b := alloc_bytes(n)\n  free(b)\n}",
        );
        // sizeof lowers to a null GEP + ptrtoint; alloc_bytes to a raw allocation.
        assert!(out.contains("getelementptr i64, ptr null, i64 1"), "{out}");
        assert!(out.contains("ptrtoint ptr"), "{out}");
        assert!(out.contains("call ptr @cool_alloc"), "{out}");
    }

    #[test]
    fn bool_widens_with_zext_not_sext() {
        // true widened to an int must be 1, not the -1 that sext would give.
        let out = ir("func f() -> void {\n  print(true)\n}");
        assert!(out.contains("zext i1"), "{out}");
        assert!(!out.contains("sext i1"), "{out}");
    }

    #[test]
    fn sizeof_of_struct_field_uses_field_type() {
        let out = ir(
            "struct S { a: int8, b: int64 }\n\
             func f() -> void {\n  s := S { a: 3i8, b: 9 }\n  n := sizeof(s.a)\n  print(n)\n}",
        );
        // The field is int8, so sizeof lowers through i8, not the i64 fallback.
        assert!(out.contains("getelementptr i8, ptr null, i64 1"), "{out}");
    }

    #[test]
    fn array_literal_coerces_to_slice_argument() {
        let out = ir(
            "func sum(xs: int64[]) -> int64 { return 0 }\n\
             func f() -> int64 { return sum([1, 2, 3]) }",
        );
        // The literal is spilled to a stack array and passed as a slice.
        assert!(out.contains("alloca [3 x i64]"), "{out}");
        assert!(out.contains("call i64 @sum({ ptr, i64 }"), "{out}");
    }

    #[test]
    fn generic_sizeof_uses_concrete_type() {
        // sizeof(T) in a generic function must lower to the instantiated type's
        // size: int8 -> 1, int64 -> 8.
        let out = ir(
            "func sz<T>(x: T) -> int64 { return sizeof(T) }\n\
             func main() -> int32 {\n  println(sz(5u8))\n  println(sz(5))\n  return 0\n}",
        );
        assert!(out.contains("getelementptr i8, ptr null, i64 1"), "{out}");
        assert!(out.contains("getelementptr i64, ptr null, i64 1"), "{out}");
    }

    #[test]
    fn generic_sizeof_of_composite_type() {
        // sizeof(T) where T is a slice must size the { ptr, i64 } fat pointer (16),
        // not fall through to the i64 default (8).
        let out = ir(
            "func sz<T>(x: T) -> int64 { return sizeof(T) }\n\
             func main() -> int32 {\n  a: int64[] = [1, 2, 3]\n  return sz(a)\n}",
        );
        assert!(out.contains("getelementptr { ptr, i64 }, ptr null, i64 1"), "{out}");
    }

    #[test]
    fn ptr_add_offsets_a_pointer() {
        let out = ir(
            "func f(p: *int8) -> *int8 {\n  return ptr_add(p, 16)\n}",
        );
        assert!(out.contains("getelementptr i8, ptr"), "{out}");
    }

    #[test]
    fn fold_threads_accumulator() {
        let out = ir(
            "func f(xs: int64[]) -> int64 {\n  return fold(xs, 0, lambda (a: int64, n: int64) -> int64 { return a + n })\n}",
        );
        assert!(out.contains("icmp slt i64"));
        assert!(out.contains("store i64 0, ptr"));
    }
}
