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
    self, Arm, BinOp, Block, Expr, ExprKind, For, Func, If, Item, Lambda, Let, Pattern, Stmt, Type,
    UnOp, While,
};

/// Compiles a module to LLVM IR text. Generics are monomorphized first. `muts`
/// carries the reconciled storage types of the narrow mutable-tuple class, which
/// mono stamps onto those bindings so their slots are sized as slices.
pub fn compile(module: &ast::Module, muts: &crate::mono::MutTupleTypes) -> Result<String, String> {
    let expanded = crate::mono::expand(module, muts);
    let module = &expanded;
    let ctx = Ctx::new(module);
    let mut m = Module::new("dusk", DEFAULT_TRIPLE);
    m.declare("void @cool_print_i64(i64)");
    m.declare("void @cool_println_i64(i64)");
    m.declare("void @cool_print_f64(double)");
    m.declare("void @cool_println_f64(double)");
    m.declare("void @cool_print_cstr(ptr)");
    m.declare("void @cool_println_cstr(ptr)");
    m.declare("void @cool_eprint_i64(i64)");
    m.declare("void @cool_eprint_f64(double)");
    m.declare("void @cool_eprint_cstr(ptr)");
    m.declare("ptr @cool_alloc(i64)");
    m.declare("void @cool_free(ptr)");
    m.declare("ptr @cool_gen_alloc(i64)");
    m.declare("void @cool_gen_free(ptr)");
    m.declare("void @cool_gen_fault()");
    m.declare("void @cool_null_fault()");
    m.declare("void @cool_bounds_fault()");
    m.declare("void @cool_shift_fault()");
    m.declare("i64 @cool_pow_i64(i64, i64)");
    m.declare("double @llvm.pow.f64(double, double)");
    m.declare("float @llvm.pow.f32(float, float)");
    m.declare("ptr @cool_debug_alloc(i64)");
    m.declare("void @cool_debug_free(ptr)");
    m.declare("i64 @cool_debug_leaks()");
    m.declare("i64 @cool_debug_double_frees()");
    m.declare("ptr @cool_thread_spawn(ptr, ptr)");
    m.declare("i64 @cool_thread_join(ptr, i64)");
    m.declare("i64 @cool_pool_submit(ptr, ptr)");
    m.declare("ptr @cool_alloc_env(i64)");
    m.declare("ptr @cool_task_new(ptr, i64, i64)");
    m.declare("ptr @cool_task_frame(ptr)");
    m.declare("void @cool_task_start(ptr)");
    m.declare("void @cool_task_await(ptr, ptr, i64)");
    m.declare("void @cool_task_return(ptr, ptr, i64)");
    m.declare("ptr @cool_task_env_alloc(ptr, i64)");
    m.declare("void @cool_future_take(ptr, i64, ptr, ptr)");
    m.declare("void @cool_loop_run(ptr, i64, ptr, i64)");
    m.declare("void @cool_task_state_fault()");
    m.declare("void @cool_gc_anchor(ptr)");
    m.declare("ptr @cool_collect_alloc(i64)");
    m.declare("void @cool_gc_collect()");
    m.declare("i64 @cool_gc_live_blocks()");
    m.declare("i64 @cool_gc_live_bytes()");
    m.declare("i64 @cool_gc_collections()");
    m.declare("void @cool_gc_add_region(ptr, i64)");
    m.declare("void @cool_gc_del_region(ptr)");
    m.declare("ptr @cool_read_file(ptr)");
    m.declare("i64 @cool_write_file(ptr, ptr)");
    m.declare("ptr @cool_read_line()");
    m.declare("ptr @cool_read_all()");
    m.declare("double @cool_parse_float(ptr, ptr)");
    // External C functions from a foreign block. Their signatures already sit in
    // ctx.fns, so a call lowers like any other user call; here they are declared
    // so clang binds them against libc at link.
    for item in &module.items {
        if let Item::Foreign(fb) = item {
            for ff in &fb.funcs {
                if let Some((ret, params)) = ctx.fns.get(&ff.name) {
                    let ps = params.iter().map(|p| p.ll()).collect::<Vec<_>>().join(", ");
                    m.declare(&format!("{} @{}({ps})", ret.ll(), ff.name));
                }
            }
        }
    }
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
            // `main(argc: int32, argv: string[])` cannot be the C entry directly,
            // since the runtime calls `main(int, char**)`. Emit the user body under
            // an internal name and a C ABI `@main` wrapper that wraps argc and argv
            // into the dusk string slice.
            // An async func lowers to one poll function over a heap frame plus a
            // framesize constant, never a plain C-ABI define; calling it mints a
            // task instead of jumping in.
            Item::Func(f) if f.is_async => {
                let def = crate::codegen::async_fn::gen_async_func(&mut m, &ctx, f);
                m.push_function(def);
            }
            Item::Func(f) if f.name == "main" && main_is_argc_argv(&ctx, f) => {
                let mut renamed = f.clone();
                renamed.name = "dusk__main".to_string();
                let def = gen_func(&mut m, &ctx, &renamed, None);
                m.push_function(def);
                m.push_function(emit_main_wrapper(&ctx, f));
            }
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
    // A lowering that could not produce sound IR recorded a fault instead of
    // aborting. Surface the accumulated faults as one build error so the module
    // never links; the render is discarded. This is the backstop for a sema
    // covariance sink a construct slipped past (an element type erased before the
    // guard could see it), keeping every such miss a clean, named build error.
    if !m.errors().is_empty() {
        return Err(m.errors().join("\n"));
    }
    Ok(m.render())
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
    // self is passed straight through as the data pointer, since methods now take
    // the receiver by pointer.
    let mut call_args = vec!["ptr %d".to_string()];
    for (i, p) in meth.params.iter().enumerate() {
        sig.push(format!("{} %a{i}", p.ll()));
        call_args.push(format!("{} %a{i}", p.ll()));
    }
    let ca = call_args.join(", ");
    let mut body = String::from("entry:\n");
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

/// Emits a forwarding thunk that adapts a bare top-level function to the closure
/// calling convention, so a plain function name can be used as a `{ env, fn }`
/// value. The thunk takes a leading env pointer it ignores, then the function's
/// own parameters, and forwards to the real symbol. It mirrors emit_thunk, the
/// vtable bridge; dusk functions all return by value (there is no sret), so even
/// an aggregate `(T, error)` return forwards unchanged.
fn emit_funcval_thunk(m: &mut Module, name: &str, ret: &CTy, params: &[CTy]) {
    let mut sig = vec!["ptr %env".to_string()];
    let mut call_args = Vec::new();
    for (i, p) in params.iter().enumerate() {
        sig.push(format!("{} %a{i}", p.ll()));
        call_args.push(format!("{} %a{i}", p.ll()));
    }
    let ca = call_args.join(", ");
    let mut body = String::from("entry:\n");
    if matches!(ret, CTy::Void) {
        body.push_str(&format!("  call void @{name}({ca})\n"));
        body.push_str("  ret void\n");
    } else {
        body.push_str(&format!("  %r = call {} @{name}({ca})\n", ret.ll()));
        body.push_str(&format!("  ret {} %r\n", ret.ll()));
    }
    m.push_function(format!(
        "define {} @{name}.funcval({}) {{\n{body}}}",
        ret.ll(),
        sig.join(", ")
    ));
}

#[derive(Clone, PartialEq)]
pub(crate) enum CTy {
    Int(u32),
    Char,
    F64,
    F32,
    Bool,
    Void,
    Ptr(Box<CTy>),
    RawPtr(Box<CTy>),
    Slice(Box<CTy>),
    Array(Box<CTy>, u64),
    Struct(String),
    Enum(String),
    Iface(String),
    Closure(Vec<CTy>, Box<CTy>),
    Error,
    Thread,
    Tuple(Vec<CTy>),
    Unknown,
}

impl CTy {
    pub(crate) fn ll(&self) -> String {
        match self {
            CTy::Int(n) => format!("i{n}"),
            CTy::Char => "i8".to_string(),
            CTy::F64 => "double".to_string(),
            CTy::F32 => "float".to_string(),
            CTy::Bool => "i1".to_string(),
            CTy::Void => "void".to_string(),
            CTy::Ptr(_) => "{ ptr, i64 }".to_string(),
            CTy::RawPtr(_) => "ptr".to_string(),
            CTy::Slice(_) => "{ ptr, i64 }".to_string(),
            CTy::Array(e, n) => format!("[{n} x {}]", e.ll()),
            CTy::Struct(n) | CTy::Enum(n) => format!("%{n}"),
            CTy::Iface(_) | CTy::Closure(..) => "{ ptr, ptr }".to_string(),
            CTy::Error => "ptr".to_string(),
            // A thread handle is a generational record pointer, the same fat
            // shape as a managed pointer, so join runs the standard check.
            CTy::Thread => "{ ptr, i64 }".to_string(),
            CTy::Tuple(ts) => {
                let inner = ts.iter().map(|t| t.ll()).collect::<Vec<_>>().join(", ");
                format!("{{ {inner} }}")
            }
            CTy::Unknown => "i64".to_string(),
        }
    }

    fn is_aggregate(&self) -> bool {
        matches!(
            self,
            CTy::Ptr(_)
                | CTy::Struct(_)
                | CTy::Enum(_)
                | CTy::Iface(_)
                | CTy::Closure(..)
                | CTy::Slice(_)
                | CTy::Array(..)
                | CTy::Tuple(_)
                | CTy::Thread
        )
    }

    fn is_float(&self) -> bool {
        matches!(self, CTy::F64 | CTy::F32)
    }

    fn int_bits(&self) -> Option<u32> {
        match self {
            CTy::Int(n) => Some(*n),
            CTy::Char => Some(8),
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

/// An async function's lowered signature, shared by its poll emission and every
/// call site: the parameter types (so both compute the same frame prefix), the
/// declared return type, and the post-mono `Future$...` struct a call packs.
pub(crate) struct AsyncInfo {
    pub(crate) params: Vec<CTy>,
    pub(crate) ret: CTy,
    pub(crate) fut_struct: String,
}

pub(crate) struct Ctx {
    structs: Vec<(String, Vec<(String, CTy)>)>,
    enums: Vec<EnumDef>,
    ifaces: Vec<IfaceDef>,
    impls: Vec<ImplInfo>,
    fns: HashMap<String, (CTy, Vec<CTy>)>,
    methods: HashMap<String, (CTy, Vec<CTy>)>,
    // Nominal kind by name, one map lookup per Named type instead of scanning
    // three item lists on every type lowering.
    noms: HashMap<String, Nom>,
    // Async functions by name. A call of one of these mints a task and returns a
    // future rather than lowering as an ordinary call.
    pub(crate) async_fns: HashMap<String, AsyncInfo>,
}

impl Ctx {
    fn new(module: &ast::Module) -> Self {
        let mut noms: HashMap<String, Nom> = HashMap::new();
        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    noms.insert(s.name.clone(), Nom::Struct);
                }
                Item::Enum(e) => {
                    noms.insert(e.name.clone(), Nom::Enum);
                }
                Item::Interface(i) => {
                    noms.insert(i.name.clone(), Nom::Iface);
                }
                _ => {}
            }
        }
        let nom = |n: &str| noms.get(n).copied().unwrap_or(Nom::None);
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
                Item::Foreign(fb) => {
                    for ff in &fb.funcs {
                        let ret = lower_ty(&ff.ret, &nom);
                        let params = ff.params.iter().map(|p| lower_ty(&p.ty, &nom)).collect();
                        fns.insert(ff.name.clone(), (ret, params));
                    }
                }
                _ => {}
            }
        }
        let mut async_fns = HashMap::new();
        for item in &module.items {
            if let Item::Func(f) = item {
                if f.is_async {
                    let ret = lower_ty(&f.ret, &nom);
                    let params = f.params.iter().map(|p| lower_ty(&p.ty, &nom)).collect();
                    // The future a call packs is the post-mono Future instance for
                    // this return type; mono force-instantiated it, so codegen
                    // mangles the identical name it emitted.
                    let fut_struct = crate::mono::mangle("Future", std::slice::from_ref(&f.ret));
                    async_fns.insert(
                        f.name.clone(),
                        AsyncInfo {
                            params,
                            ret,
                            fut_struct,
                        },
                    );
                }
            }
        }
        Ctx {
            structs,
            enums,
            ifaces,
            impls,
            fns,
            methods,
            noms,
            async_fns,
        }
    }

    fn nom(&self, name: &str) -> Nom {
        self.noms.get(name).copied().unwrap_or(Nom::None)
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
        self.enum_def(ename)?
            .variants
            .iter()
            .find(|v| v.name == vname)
    }

    /// Size and alignment in bytes of a lowered type.
    pub(crate) fn size_align(&self, ty: &CTy) -> (u64, u64) {
        match ty {
            CTy::Bool => (1, 1),
            CTy::Char => (1, 1),
            CTy::Int(n) => {
                let b = (*n as u64 / 8).max(1);
                (b, b)
            }
            CTy::F32 => (4, 4),
            CTy::F64 => (8, 8),
            CTy::Ptr(_) => (16, 8),
            CTy::RawPtr(_) => (8, 8),
            CTy::Slice(_) => (16, 8),
            CTy::Array(e, n) => {
                let (es, ea) = self.size_align(e);
                (align_up(es, ea) * n, ea)
            }
            CTy::Struct(name) => self.fields(name).map(|f| self.layout(f)).unwrap_or((8, 8)),
            CTy::Enum(name) => self.enum_size_align(name),
            CTy::Iface(_) | CTy::Closure(..) => (16, 8),
            CTy::Error => (8, 8),
            CTy::Thread => (16, 8),
            CTy::Tuple(ts) => {
                let mut size = 0u64;
                let mut align = 1u64;
                for t in ts {
                    let (s, a) = self.size_align(t);
                    size = align_up(size, a) + s;
                    align = align.max(a);
                }
                (align_up(size, align), align)
            }
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

/// A user-facing name for a lowered type, for a diagnostic message. A nominal
/// type reads as its source name; anything else falls back to its LLVM form.
fn ty_name(t: &CTy) -> String {
    match t {
        CTy::Iface(n) | CTy::Struct(n) | CTy::Enum(n) => n.clone(),
        _ => t.ll(),
    }
}

fn align_up(n: u64, align: u64) -> u64 {
    if align <= 1 {
        n
    } else {
        n.div_ceil(align) * align
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
            "char" => CTy::Char,
            "rune" => CTy::Int(32),
            "float64" => CTy::F64,
            "float32" => CTy::F32,
            "string" => CTy::RawPtr(Box::new(CTy::Char)),
            "error" => CTy::Error,
            "thread" => CTy::Thread,
            _ => match nom(n) {
                Nom::Struct => CTy::Struct(n.clone()),
                Nom::Enum => CTy::Enum(n.clone()),
                Nom::Iface => CTy::Iface(n.clone()),
                Nom::None => CTy::Unknown,
            },
        },
        Type::Ptr(b) => {
            // *void is the erased raw currency of the allocator and FFI layer
            // (D23), a thin pointer, not a managed generational one. The parser
            // lowers the `void` type to Unit, so a pointer to Unit is *void.
            if matches!(&**b, Type::Unit) {
                CTy::RawPtr(Box::new(CTy::Void))
            } else {
                CTy::Ptr(Box::new(lower_ty(b, nom)))
            }
        }
        Type::RawPtr(b) => CTy::RawPtr(Box::new(lower_ty(b, nom))),
        // A collector's runtime rep tracks the kind of value it wraps. A plain
        // element (scalar, managed, string, struct of those) is a managed `*T`,
        // the fat `{ptr, gen}`, and deref, field, and copy are the managed-pointer
        // path. A function element is a closure `{env, fn}` and a slice element is
        // a slice `{ptr, len}`: both carry the same rep as the value they wrap, so
        // a call dispatches through the closure and an index strides the slice with
        // no wrapper unwrap. Only the `Collect` node differs, minting the env or
        // the slice backing on the collected heap.
        Type::Collector(b) => match &**b {
            Type::Func(..) | Type::Slice(..) => lower_ty(b, nom),
            _ => CTy::Ptr(Box::new(lower_ty(b, nom))),
        },
        Type::Slice(b) => CTy::Slice(Box::new(lower_ty(b, nom))),
        Type::Array(b, n) => CTy::Array(Box::new(lower_ty(b, nom)), *n),
        Type::Func(ps, r) => CTy::Closure(
            ps.iter().map(|p| lower_ty(p, nom)).collect(),
            Box::new(lower_ty(r, nom)),
        ),
        Type::Tuple(ts) => CTy::Tuple(ts.iter().map(|t| lower_ty(t, nom)).collect()),
        Type::Unit => CTy::Void,
        // A hole reaching codegen means mono failed to resolve a do-continuation
        // and analyze should already have aborted on emit_ty's diagnostic. If this
        // fires, mono has a bug.
        Type::Infer => unreachable!("unresolved type hole reached codegen"),
    }
}

/// True for the `main(argc: int32, argv: string[])` form, the only main that
/// needs the C ABI entry wrapper. A no argument main is already a valid C entry.
fn main_is_argc_argv(ctx: &Ctx, f: &Func) -> bool {
    let nom = |n: &str| ctx.nom(n);
    f.params.len() == 2
        && matches!(lower_ty(&f.params[0].ty, &nom), CTy::Int(32))
        && matches!(lower_ty(&f.params[1].ty, &nom), CTy::Slice(_))
}

/// The C ABI entry wrapper for an argc/argv main. It receives `(int, char**)`,
/// builds the dusk `string[]` slice as `{ argv, argc }`, and calls the renamed
/// user main, handing back its result as the process exit code.
fn emit_main_wrapper(ctx: &Ctx, f: &Func) -> String {
    let nom = |n: &str| ctx.nom(n);
    let rty = lower_ty(&f.ret, &nom).ll();
    format!(
        "define {rty} @main(i32 %argc, ptr %argv) {{\n\
         entry:\n  \
         %len = sext i32 %argc to i64\n  \
         %s0 = insertvalue {{ ptr, i64 }} undef, ptr %argv, 0\n  \
         %s1 = insertvalue {{ ptr, i64 }} %s0, i64 %len, 1\n  \
         %r = call {rty} @dusk__main(i32 %argc, {{ ptr, i64 }} %s1)\n  \
         ret {rty} %r\n\
         }}"
    )
}

fn gen_func(m: &mut Module, ctx: &Ctx, f: &Func, self_ty: Option<&str>) -> String {
    let nom = |n: &str| ctx.nom(n);
    let mut params: Vec<(String, CTy)> = Vec::new();
    if let Some(t) = self_ty {
        // self comes in by pointer, so a method can mutate the receiver and a
        // stateful allocator's bump offset persists across calls.
        params.push((
            "self".to_string(),
            CTy::RawPtr(Box::new(CTy::Struct(t.to_string()))),
        ));
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
    // The C entry records the main thread stack high water for the collector as
    // its first instruction, so the anchor slot sits at the top of the outermost
    // frame. An argc/argv main is renamed dusk__main and called one frame below
    // the wrapper, still the outermost dusk frame, so it anchors there instead.
    if self_ty.is_none() && (name == "main" || name == "dusk__main") {
        let slot = fb.fresh();
        // Route the anchor slot through the funnel too, before any parameter or
        // local alloca, so it stays the first slot in the entry block: the
        // collector anchors its stack high water at this address and scans down
        // from it, so every other slot must live below it.
        fb.push_entry_alloca(&format!("{slot} = alloca i8"));
        fb.line(&format!("call void @cool_gc_anchor(ptr {slot})"));
    }
    for (i, (pname, ty)) in params.iter().enumerate() {
        if self_ty.is_some() && i == 0 {
            // The incoming pointer is self's storage. Bind it as a Struct lvalue,
            // not a fresh copy, so `self.field` reads and writes the caller's value.
            if let CTy::Ptr(inner) | CTy::RawPtr(inner) = ty {
                fb.locals
                    .insert(pname.clone(), ((**inner).clone(), format!("%a{i}")));
            }
            continue;
        }
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
    format!("{header}\n{}}}", fb.body_out())
}

pub(crate) struct Fb<'a> {
    pub(crate) m: &'a mut Module,
    pub(crate) ctx: &'a Ctx,
    pub(crate) ret: CTy,
    pub(crate) body: String,
    tmp: u32,
    label: u32,
    pub(crate) locals: HashMap<String, (CTy, String)>,
    defers: Vec<Expr>,
    pub(crate) terminated: bool,
    allocator: Option<(String, CTy)>,
    // In async mode this holds the task frame: alloca and alloca_raw route every
    // slot through it and return an entry-block GEP name rather than emitting an
    // alloca line, so no local's storage pointer is born mid-body where a resume
    // edge could bypass it. Sync functions leave it None and allocate inline.
    pub(crate) frame: Option<crate::codegen::frame::FrameCtx>,
    // Sync-mode alloca funnel. Every stack slot is buffered here and spliced into
    // the entry block at finish, not emitted at the binding site. A binding inside
    // a loop body would otherwise emit its alloca once per iteration, leaking an
    // unbounded stack slot every pass until the stack overflows; hoisting to entry
    // gives one slot reused across iterations, the standard LLVM frontend shape.
    // Async mode leaves this empty: its slots are heap frame GEPs born in a
    // synthesized entry block (see frame.rs), already hoisted by construction.
    entry_allocas: String,
}

impl<'a> Fb<'a> {
    pub(crate) fn new(m: &'a mut Module, ctx: &'a Ctx, ret: CTy) -> Self {
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
            frame: None,
            entry_allocas: String::new(),
        }
    }

    pub(crate) fn fresh(&mut self) -> String {
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
        // An alloca line must never reach an async frame body: every slot there
        // is a frame GEP born in the prepended entry block, so a stray inline
        // alloca would be storage the resume switch bypasses.
        debug_assert!(
            self.frame.is_none() || !s.trim_start().contains("= alloca "),
            "an inline alloca leaked into an async frame body: {s}"
        );
        self.body.push_str("  ");
        self.body.push_str(s);
        self.body.push('\n');
    }

    pub(crate) fn place_label(&mut self, l: &str) {
        self.body.push_str(l);
        self.body.push_str(":\n");
        self.terminated = false;
    }

    fn alloca(&mut self, ty: &CTy) -> String {
        if self.frame.is_some() {
            let sa = self.ctx.size_align(ty);
            return self.frame_slot(ty.ll(), sa);
        }
        let d = self.fresh();
        self.push_entry_alloca(&format!("{d} = alloca {}", ty.ll()));
        d
    }

    /// Buffers a stack slot in the entry-block funnel. In sync mode every alloca
    /// lands here so it is emitted once at function entry, not at the binding
    /// site: a loop-body binding would otherwise allocate a fresh slot each
    /// iteration and never reclaim it, overflowing the stack on large inputs.
    /// The store that initializes the slot stays at the binding site; only the
    /// slot reservation moves earlier, and the entry block dominates all uses.
    fn push_entry_alloca(&mut self, s: &str) {
        self.entry_allocas.push_str("  ");
        self.entry_allocas.push_str(s);
        self.entry_allocas.push('\n');
    }

    /// The finished body with the entry-block alloca funnel spliced in right
    /// after the entry label. Both sync assembly sites open the body with
    /// `entry:\n` before lowering, so the splice point is always present.
    fn body_out(&self) -> String {
        const HEAD: &str = "entry:\n";
        debug_assert!(
            self.body.starts_with(HEAD),
            "sync body must open with the entry label"
        );
        let mut out = String::with_capacity(self.body.len() + self.entry_allocas.len());
        out.push_str(HEAD);
        out.push_str(&self.entry_allocas);
        out.push_str(&self.body[HEAD.len()..]);
        out
    }

    /// Appends a frame slot in async mode and returns the SSA name its entry
    /// block GEP will define. The name is minted from the poll's counter so it
    /// is unique against every body temporary; the GEP itself is emitted later,
    /// when the entry block is synthesized and prepended.
    fn frame_slot(&mut self, llty: String, (size, align): (u64, u64)) -> String {
        let name = self.fresh();
        let frame = self.frame.as_mut().expect("frame_slot outside async mode");
        frame.slots.push(crate::codegen::frame::Slot {
            name: name.clone(),
            llty,
            size,
            align,
            off: 0,
        });
        name
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

    /// Builds a managed fat pointer value `{ data, gen }` from a raw data pointer
    /// and a generation.
    fn fat(&mut self, data: &str, gen: &str) -> String {
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {{ ptr, i64 }} undef, ptr {data}, 0"
        ));
        let b = self.fresh();
        self.line(&format!(
            "{b} = insertvalue {{ ptr, i64 }} {a}, i64 {gen}, 1"
        ));
        b
    }

    /// Extracts the raw data pointer from a managed fat pointer with no check.
    /// Used where a freed pointer is acceptable, like free itself.
    fn fat_data(&mut self, fat: &str) -> String {
        let d = self.fresh();
        self.line(&format!("{d} = extractvalue {{ ptr, i64 }} {fat}, 0"));
        d
    }

    /// Extracts the raw data pointer and emits the generation check. When the
    /// remembered generation is not the untracked sentinel 0, it must equal the
    /// live generation in the header word at data minus 8, or the program faults
    /// on a freed or stale pointer.
    fn fat_checked(&mut self, fat: &str) -> String {
        let data = self.fat_data(fat);
        let gen = self.fresh();
        self.line(&format!("{gen} = extractvalue {{ ptr, i64 }} {fat}, 1"));
        let chk = self.new_label();
        let skip = self.new_label();
        let z = self.fresh();
        self.line(&format!("{z} = icmp eq i64 {gen}, 0"));
        // The untracked generation zero path skips the header check, but a
        // zeroed fat pair, the drained channel's placeholder for one, is null
        // there, so it is tested and faults by name instead of a raw signal.
        let nullchk = self.new_label();
        self.cond_br(&z, &nullchk, &chk);
        self.place_label(&nullchk);
        let isnull = self.fresh();
        self.line(&format!("{isnull} = icmp eq ptr {data}, null"));
        let nfault = self.new_label();
        self.cond_br(&isnull, &nfault, &skip);
        self.place_label(&nfault);
        self.line("call void @cool_null_fault()");
        self.br(&skip);
        self.place_label(&chk);
        let hp = self.fresh();
        self.line(&format!("{hp} = getelementptr i8, ptr {data}, i64 -8"));
        // The header word is bumped atomically by free on any thread, so the
        // check reads it atomically too, keeping the machinery race free.
        let cur = self.fresh();
        self.line(&format!(
            "{cur} = load atomic i64, ptr {hp} seq_cst, align 8"
        ));
        let bad = self.fresh();
        self.line(&format!("{bad} = icmp ne i64 {cur}, {gen}"));
        let fault = self.new_label();
        self.cond_br(&bad, &fault, &skip);
        self.place_label(&fault);
        self.line("call void @cool_gen_fault()");
        self.br(&skip);
        self.place_label(&skip);
        data
    }

    /// Emits a bounds check for an array or slice index. An unsigned compare
    /// catches a negative index too, since it wraps to a large value, so one
    /// compare covers both ends. On a miss the program traps through the runtime.
    fn bounds_check(&mut self, i: &str, len: &str) {
        let bad = self.fresh();
        self.line(&format!("{bad} = icmp uge i64 {i}, {len}"));
        let fault = self.new_label();
        let ok = self.new_label();
        self.cond_br(&bad, &fault, &ok);
        self.place_label(&fault);
        self.line("call void @cool_bounds_fault()");
        self.br(&ok);
        self.place_label(&ok);
    }

    fn default_ret(&mut self) {
        let r = self.ret.clone();
        match &r {
            CTy::Void => self.line("ret void"),
            CTy::F64 | CTy::F32 => self.line(&format!("ret {} 0.0", r.ll())),
            CTy::RawPtr(_) => self.line("ret ptr null"),
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
            // A bool and a char are unsigned, so zero extend them when widening.
            // A bool widens to 1, not the all ones -1 that sext would give, and a
            // char byte at or above 128 widens to its 0 to 255 value, not a
            // negative number.
            let cast = if tb < fb {
                "trunc"
            } else if matches!(from, CTy::Bool | CTy::Char) {
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
        // A float widens or narrows between f32 and f64. Both are float here and
        // unequal (equal returned above), so f32->f64 is fpext and f64->f32 is
        // fptrunc. Without this a float32 reaches a `double` sink (the f64 print
        // and stderr printers) still typed `float`, which clang rejects at link.
        if from.is_float() && to.is_float() {
            let cast = if matches!(from, CTy::F64) {
                "fptrunc"
            } else {
                "fpext"
            };
            let d = self.fresh();
            self.line(&format!("{d} = {cast} {} {op} to {}", from.ll(), to.ll()));
            return d;
        }
        // A managed pointer narrows to a raw pointer by dropping its generation,
        // for returning or passing a *T where the raw *void layer is expected.
        if matches!(from, CTy::Ptr(_)) && matches!(to, CTy::RawPtr(_)) {
            return self.fat_data(op);
        }
        // A raw pointer widens to a managed pointer as untracked, generation 0.
        if matches!(from, CTy::RawPtr(_)) && matches!(to, CTy::Ptr(_)) {
            return self.fat(op, "0");
        }
        // Two tuples of equal arity but differing members are rebuilt member by
        // member: extract each, adapt it to the target member type, and reinsert
        // into the target aggregate, so the value's type tag matches the store or
        // return type. Each member routes through `adapt`, not raw `coerce`, so a
        // struct member boxes to an interface and an array member views as a
        // slice (both fat conversions live in adapt); a narrow integer literal
        // truncates to its declared width; a member already equal (a managed
        // pointer's fat { ptr, i64 }, an error word) copies unchanged. Nested
        // tuples recurse through this same arm. An identical tuple never reaches
        // here, since the `from == to` check above short-circuits it.
        if let (CTy::Tuple(fs), CTy::Tuple(ts)) = (from, to) {
            if fs.len() == ts.len() {
                let (fs, ts) = (fs.clone(), ts.clone());
                let mut agg = "undef".to_string();
                for (i, (ft, tt)) in fs.iter().zip(ts.iter()).enumerate() {
                    let m = self.fresh();
                    self.line(&format!("{m} = extractvalue {} {op}, {i}", from.ll()));
                    let cm = self.adapt(Val::new(ft.clone(), m), tt).op;
                    let d = self.fresh();
                    self.line(&format!(
                        "{d} = insertvalue {} {agg}, {} {cm}, {i}",
                        to.ll(),
                        tt.ll()
                    ));
                    agg = d;
                }
                return agg;
            }
        }
        // Two arrays of equal length but differing element types are rebuilt
        // element by element, exactly like the tuple arm: extract each, adapt it
        // to the target element type (boxing a struct to an interface, viewing an
        // inner array as a slice, recursing through further nesting), and reinsert
        // into the target array. Without this, an array of a fat element type
        // keeps the source element's layout (a raw struct where the target wants
        // a fat pointer), which either mismatches at clang or, when the two share
        // an LLVM type, strides off the end at runtime. An identical array never
        // reaches here, since the `from == to` check above short-circuits it.
        if let (CTy::Array(fe, fc), CTy::Array(te, tc)) = (from, to) {
            if fc == tc {
                let (fe, te, n) = ((**fe).clone(), (**te).clone(), *fc);
                let mut agg = "undef".to_string();
                for i in 0..n {
                    let m = self.fresh();
                    self.line(&format!("{m} = extractvalue {} {op}, {i}", from.ll()));
                    let cm = self.adapt(Val::new(fe.clone(), m), &te).op;
                    let d = self.fresh();
                    self.line(&format!(
                        "{d} = insertvalue {} {agg}, {} {cm}, {i}",
                        to.ll(),
                        te.ll()
                    ));
                    agg = d;
                }
                return agg;
            }
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
        if let (CTy::Slice(target_elem), CTy::Array(src_elem, n)) = (target, &v.ty) {
            let count = *n as usize;
            let target_elem = (**target_elem).clone();
            let src_elem = (**src_elem).clone();
            // When the array's element type differs from the slice's declared
            // element (an Array(Box) viewed as a Slice(Sized)), rebuild the array
            // at the target element type first, so the backing is [n x <slice
            // elem>] (the wide fat element) rather than [n x <source elem>]. The
            // slice then strides over storage that actually holds fat elements;
            // deriving the backing from the source element would size it for the
            // narrow struct and walk off the end.
            let arr = if src_elem == target_elem {
                v
            } else {
                let target_arr = CTy::Array(Box::new(target_elem), *n);
                self.adapt(v, &target_arr)
            };
            return self.slice_from_array(arr, count);
        }
        // Slice covariance backstop. A same-type slice (or one whose element is
        // the Unknown placeholder) reinterprets its fat pointer safely and falls
        // through unchanged. But a slice VALUE of a concrete element targeting a
        // slice of an interface cannot be reinterpreted: each element would need
        // reboxing into { data, vtable }, and a whole-slice cast leaves concrete
        // payloads read as fat pairs, dispatching through garbage into a SEGV.
        // dusk rejects this covariance; reboxing would also silently copy (the new
        // slice would not alias the source), a semantics the reject decision
        // rules out. So this is a hard, loud build error rather than silent IR:
        // if a typeck sink misses the reject, the class still cannot miscompile.
        // The array-literal case is the (Slice, Array) arm above and still boxes
        // per element; only a differing-element slice VALUE reaches here.
        if let (CTy::Slice(target_elem), CTy::Slice(src_elem)) = (target, &v.ty) {
            let same = src_elem == target_elem
                || matches!(**src_elem, CTy::Unknown)
                || matches!(**target_elem, CTy::Unknown);
            if !same && matches!(**target_elem, CTy::Iface(_)) {
                // A sema covariance sink should have rejected this. When one is
                // missed (an element type erased to Unknown before it, so the
                // guard could not see the concrete element), record a clean build
                // diagnostic and poison the value rather than aborting the process.
                // A whole-slice reinterpret cannot be reboxed soundly, so the
                // module must not link; the poison keeps IR generation going only
                // far enough to collect the diagnostic. This is the permanent net:
                // any missed sink is a loud, named build error, never a miscompile.
                self.m.error(format!(
                    "cannot pass a slice of '{}' as a slice of interface '{}'; a slice of concrete values cannot be reinterpreted as a slice of interfaces",
                    ty_name(src_elem),
                    ty_name(target_elem)
                ));
                return Val::new(target.clone(), "zeroinitializer".to_string());
            }
        }
        let op = self.coerce(&v.ty, &v.op, target);
        Val::new(target.clone(), op)
    }

    /// Storage for a fat value's backing: an inline frame slot in sync mode, a
    /// per-execution task-arena block in async mode. A boxed interface's data
    /// pointer and a slice's data pointer can outlive the loop iteration that
    /// created them, stored into an array or collection that lives on within the
    /// same task; a single reused frame slot would then alias every iteration's
    /// copy onto one backing. So in async mode the backing is a fresh
    /// cool_task_env_alloc block (freed at cool_task_return), the same treatment
    /// a closure environment gets. Sync inline alloca is already per-execution.
    fn backing_slot(&mut self, ty: &CTy) -> String {
        if self.frame.is_some() {
            let (size, _) = self.ctx.size_align(ty);
            let p = self.fresh();
            self.line(&format!(
                "{p} = call ptr @cool_task_env_alloc(ptr %frame, i64 {size})"
            ));
            return p;
        }
        self.alloca(ty)
    }

    /// Boxes a struct value as an interface fat pointer `{ data, vtable }`.
    fn box_iface(&mut self, v: &Val, iface: &str, ty: &str) -> Val {
        let slot = self.backing_slot(&v.ty);
        self.line(&format!("store {} {}, ptr {slot}", v.ty.ll(), v.op));
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {{ ptr, ptr }} undef, ptr {slot}, 0"
        ));
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

    /// The async return path: evaluate and adapt the value, replay defers at true
    /// completion, store the value into the frame result slot, hand it to
    /// cool_task_return, and return void from the poll. A bare return, a void
    /// return, and falling off the end all complete with size 0, copying nothing.
    pub(crate) fn gen_async_return(&mut self, e: Option<&Expr>) {
        match e {
            Some(e) => {
                let v = self.gen_expr(e);
                self.gen_async_return_val(v);
            }
            None => self.gen_async_return_unit(),
        }
    }

    /// Completes the task with an already-evaluated value. Shared by `return e`
    /// and `return await f` (which loads the taken element and returns it).
    fn gen_async_return_val(&mut self, v: Val) {
        let ret = self.ret.clone();
        let av = self.adapt(v, &ret);
        self.emit_defers();
        let (res, ret_size) = {
            let f = self
                .frame
                .as_ref()
                .expect("async return outside async mode");
            (f.res.clone(), f.ret_size)
        };
        let mut size = 0u64;
        if !matches!(ret, CTy::Void) {
            self.line(&format!("store {} {}, ptr {res}", ret.ll(), av.op));
            size = ret_size;
        }
        self.line(&format!(
            "call void @cool_task_return(ptr %frame, ptr {res}, i64 {size})"
        ));
        self.line("ret void");
        self.terminated = true;
    }

    /// Completes the task with no value (bare return, void return, fall-off-end).
    fn gen_async_return_unit(&mut self) {
        self.emit_defers();
        let res = self
            .frame
            .as_ref()
            .expect("async return outside async mode")
            .res
            .clone();
        self.line(&format!(
            "call void @cool_task_return(ptr %frame, ptr {res}, i64 0)"
        ));
        self.line("ret void");
        self.terminated = true;
    }

    /// The await suspend/resume protocol at one site. Evaluates the operand to a
    /// fat future value, stores its words to the shared pending slots and the
    /// state index, suspends with cool_task_await, and returns void from the
    /// poll. At the resume label the future words are reloaded from the pending
    /// slots and cool_future_take consumes the future into per-site element and
    /// error slots (both frame slots born in the entry block). Returns those two
    /// slot pointers and the element type, for the caller to destructure.
    ///
    /// Store order is pending words (offsets 8 and 16) then the state word
    /// (offset 0) then the await call then ret void; nothing read after the
    /// resume label is an SSA temporary defined before the ret, so no value
    /// crosses the resume edge. The await always yields one scheduler turn even
    /// on a ready future; that is cool_task_await's job, not the codegen's.
    fn gen_await_take(&mut self, op: &Expr, el: Option<&Type>) -> (String, String, CTy) {
        let el_ty = el.expect("an await site must carry its element type from mono");
        let el_cty = lower_ty(el_ty, &|n| self.ctx.nom(n));
        let futval = self.gen_expr(op);
        let d = self.fresh();
        self.line(&format!(
            "{d} = extractvalue {} {}, 0",
            futval.ty.ll(),
            futval.op
        ));
        let g = self.fresh();
        self.line(&format!(
            "{g} = extractvalue {} {}, 1",
            futval.ty.ll(),
            futval.op
        ));
        let (k, pend_d, pend_g) = {
            let f = self.frame.as_mut().expect("await outside async mode");
            f.await_count += 1;
            (f.await_count, f.pend_d.clone(), f.pend_g.clone())
        };
        self.line(&format!("store ptr {d}, ptr {pend_d}"));
        self.line(&format!("store i64 {g}, ptr {pend_g}"));
        self.line(&format!("store i64 {k}, ptr %frame"));
        self.line(&format!(
            "call void @cool_task_await(ptr %frame, ptr {d}, i64 {g})"
        ));
        self.line("ret void");
        self.terminated = true;
        let resume = format!("resume.{k}");
        self.frame
            .as_mut()
            .expect("await outside async mode")
            .resume_cases
            .push((k as u64, resume.clone()));
        self.place_label(&resume);
        let d2 = self.load(&CTy::RawPtr(Box::new(CTy::Void)), &pend_d);
        let g2 = self.load(&CTy::Int(64), &pend_g);
        // A void element still needs a word to absorb the runtime's minimum
        // one-byte copy, so a zero-size element routes to a scratch word slot.
        let el_slot = if self.ctx.size_align(&el_cty).0 == 0 {
            self.alloca_raw("i64")
        } else {
            self.alloca(&el_cty)
        };
        let err_slot = self.alloca_raw("ptr");
        self.line(&format!(
            "call void @cool_future_take(ptr {d2}, i64 {g2}, ptr {el_slot}, ptr {err_slot})"
        ));
        (el_slot, err_slot, el_cty)
    }

    /// `x := await f`, `x, e := await f`, or `x, y := await f` (tuple element).
    /// The shape follows section 5: a single bind takes the whole element; a
    /// tuple element with matching arity destructures member-wise (err discarded,
    /// task NULL or leaf's own error inside the tuple); otherwise two binds take
    /// the element and the error word.
    fn gen_await_let(&mut self, l: &Let, op: &Expr, el: Option<&Type>) {
        let (el_slot, err_slot, el_cty) = self.gen_await_take(op, el);
        let binds = &l.binds;
        if binds.len() == 1 {
            let ev = self.load(&el_cty, &el_slot);
            self.bind_await(&binds[0], Val::new(el_cty, ev));
        } else if let CTy::Tuple(ts) = el_cty.clone() {
            if ts.len() == binds.len() {
                let ev = self.load(&el_cty, &el_slot);
                for (i, bind) in binds.iter().enumerate() {
                    let m = self.fresh();
                    self.line(&format!("{m} = extractvalue {} {ev}, {i}", el_cty.ll()));
                    self.bind_await(bind, Val::new(ts[i].clone(), m));
                }
                return;
            }
            // A tuple element with a two-bind element+error shape falls through.
            let ev = self.load(&el_cty, &el_slot);
            self.bind_await(&binds[0], Val::new(el_cty, ev));
            let errv = self.load(&CTy::Error, &err_slot);
            self.bind_await(&binds[1], Val::new(CTy::Error, errv));
        } else {
            let ev = self.load(&el_cty, &el_slot);
            self.bind_await(&binds[0], Val::new(el_cty, ev));
            let errv = self.load(&CTy::Error, &err_slot);
            self.bind_await(&binds[1], Val::new(CTy::Error, errv));
        }
    }

    /// Binds one awaited value into a fresh frame slot, honoring an annotation.
    fn bind_await(&mut self, bind: &ast::Bind, v: Val) {
        let declared = bind
            .ty
            .as_ref()
            .map(|t| lower_ty(t, &|n| self.ctx.nom(n)))
            .unwrap_or_else(|| v.ty.clone());
        let av = self.adapt(v, &declared);
        let dst = self.alloca(&declared);
        self.line(&format!("store {} {}, ptr {dst}", declared.ll(), av.op));
        self.locals.insert(bind.name.clone(), (declared, dst));
    }

    /// `return await f`: the element is the whole declared return, so take it and
    /// replay the async return path (defers run at true completion, then
    /// cool_task_return). The error word is discarded; a task future's is NULL
    /// and a leaf tuple return carries its error inside the tuple.
    fn gen_await_return(&mut self, op: &Expr, el: Option<&Type>) {
        let (el_slot, _err_slot, el_cty) = self.gen_await_take(op, el);
        let ev = self.load(&el_cty, &el_slot);
        self.gen_async_return_val(Val::new(el_cty, ev));
    }

    /// `await f` void form: take and discard both the element and the error.
    fn gen_await_void(&mut self, op: &Expr, el: Option<&Type>) {
        self.gen_await_take(op, el);
    }

    pub(crate) fn gen_block(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            if self.terminated {
                break;
            }
            self.gen_stmt(s);
        }
    }

    fn gen_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(l) => {
                if let ExprKind::Await(op, el) = &l.value.kind {
                    self.gen_await_let(l, op, el.as_ref());
                } else {
                    self.gen_let(l);
                }
            }
            Stmt::Assign(lhs, rhs) => {
                if let Some((ty, ptr)) = self.gen_place(lhs) {
                    let v = self.gen_expr(rhs);
                    // adapt, not coerce: `s.f = someStruct` where f is an
                    // interface boxes, and `arr[i] = [..]` views an array literal
                    // as a slice; the fat conversions live in adapt.
                    let op = self.adapt(v, &ty).op;
                    self.line(&format!("store {} {op}, ptr {ptr}", ty.ll()));
                } else {
                    self.gen_expr(rhs);
                }
            }
            Stmt::AssignOp(op, lhs, rhs) => {
                // The place address is computed once, so its index side effects,
                // bounds checks, and generation checks all run exactly once. The
                // current value is loaded, combined with the right operand, and
                // stored back through the same pointer.
                if let Some((ty, ptr)) = self.gen_place(lhs) {
                    let cur = self.load(&ty, &ptr);
                    let v = self.gen_binop_vals(*op, Val::new(ty.clone(), cur), rhs);
                    let out = self.coerce(&v.ty, &v.op, &ty);
                    self.line(&format!("store {} {out}, ptr {ptr}", ty.ll()));
                } else {
                    self.gen_expr(rhs);
                }
            }
            Stmt::Return(Some(e)) => {
                if let ExprKind::Await(op, el) = &e.kind {
                    self.gen_await_return(op, el.as_ref());
                    return;
                }
                if self.frame.is_some() {
                    self.gen_async_return(Some(e));
                    return;
                }
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
                if self.frame.is_some() {
                    self.gen_async_return(None);
                    return;
                }
                self.emit_defers();
                self.default_ret();
            }
            Stmt::Defer(e) => self.defers.push(e.clone()),
            Stmt::If(i) => self.gen_if(i),
            Stmt::While(w) => self.gen_while(w),
            Stmt::Expr(e) => {
                if let ExprKind::Await(op, el) = &e.kind {
                    self.gen_await_void(op, el.as_ref());
                } else {
                    self.gen_expr(e);
                }
            }
            Stmt::Match(m) => self.gen_match(&m.scrut, &m.arms, None),
            Stmt::For(f) => self.gen_for(f),
        }
    }

    fn gen_let(&mut self, l: &Let) {
        if l.binds.len() != 1 {
            self.gen_let_destructure(l);
            return;
        }
        let bind = &l.binds[0];
        let declared = bind.ty.as_ref().map(|t| lower_ty(t, &|n| self.ctx.nom(n)));
        let v = match (&l.value.kind, &declared) {
            (ExprKind::Array(elems), Some(CTy::Array(elem, _))) => {
                let hint = (**elem).clone();
                self.gen_array_lit(elems, Some(hint))
            }
            (ExprKind::Array(elems), Some(CTy::Slice(elem))) => {
                let hint = (**elem).clone();
                self.array_to_slice(elems, hint)
            }
            // `p: *T = alloc(...)` sizes the block from the declared pointee, per
            // the spec, so `p: *Big = alloc()` allocates all of Big rather than
            // the 8 byte default the argument free form would fall to.
            (ExprKind::Call(f, cargs), Some(CTy::Ptr(inner))) if matches!(&f.kind, ExprKind::Ident(n) if n == "alloc" && !self.ctx.fns.contains_key(n)) =>
            {
                let value = cargs.first().map(|a| self.gen_expr(a));
                self.alloc_of((**inner).clone(), value)
            }
            _ => self.gen_expr(&l.value),
        };
        let ty = declared.unwrap_or_else(|| v.ty.clone());
        let av = self.adapt(v, &ty);
        let ptr = self.alloca(&ty);
        self.line(&format!("store {} {}, ptr {ptr}", ty.ll(), av.op));
        self.locals.insert(bind.name.clone(), (ty, ptr));
    }

    /// Destructures a tuple value into several locals, as in `q, e := f()`.
    fn gen_let_destructure(&mut self, l: &Let) {
        let v = self.gen_expr(&l.value);
        let elems = match &v.ty {
            CTy::Tuple(ts) => ts.clone(),
            _ => return,
        };
        for (i, bind) in l.binds.iter().enumerate() {
            let Some(ety) = elems.get(i).cloned() else {
                break;
            };
            let d = self.fresh();
            self.line(&format!("{d} = extractvalue {} {}, {i}", v.ty.ll(), v.op));
            let declared = bind
                .ty
                .as_ref()
                .map(|t| lower_ty(t, &|n| self.ctx.nom(n)))
                .unwrap_or_else(|| ety.clone());
            let av = self.adapt(Val::new(ety, d), &declared);
            let ptr = self.alloca(&declared);
            self.line(&format!("store {} {}, ptr {ptr}", declared.ll(), av.op));
            self.locals.insert(bind.name.clone(), (declared, ptr));
        }
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

    /// `for x in iter`. Resolves the iterable to a data pointer, a length, and an
    /// element type, then loops an index from zero, binding `x` to each element in
    /// a fresh slot the body reads. Mirrors the `foreach` builtin's loop shape.
    fn gen_for(&mut self, f: &For) {
        let (data, len, elem) = self.for_source(&f.iter);
        // The data pointer and length are born in the preheader but read from
        // the loop header and body. Spill both into slots and reload them per
        // block so no SSA value crosses a label boundary: a resume edge that
        // jumps straight into the body must never depend on a preheader temp.
        let data_slot = self.alloca_raw("ptr");
        self.line(&format!("store ptr {data}, ptr {data_slot}"));
        let len_slot = self.alloca_raw("i64");
        self.line(&format!("store i64 {len}, ptr {len_slot}"));
        let ptr_ty = CTy::RawPtr(Box::new(CTy::Void));
        let slot = self.alloca(&elem);
        self.locals
            .insert(f.var.clone(), (elem.clone(), slot.clone()));
        let i = self.alloca_raw("i64");
        self.line(&format!("store i64 0, ptr {i}"));
        let cond = self.new_label();
        let body = self.new_label();
        let end = self.new_label();
        self.br(&cond);
        self.place_label(&cond);
        let iv = self.load(&CTy::Int(64), &i);
        let lenv = self.load(&CTy::Int(64), &len_slot);
        let c = self.fresh();
        self.line(&format!("{c} = icmp slt i64 {iv}, {lenv}"));
        self.cond_br(&c, &body, &end);
        self.place_label(&body);
        let ivb = self.load(&CTy::Int(64), &i);
        let datav = self.load(&ptr_ty, &data_slot);
        let ep = self.fresh();
        self.line(&format!(
            "{ep} = getelementptr {}, ptr {datav}, i64 {ivb}",
            elem.ll()
        ));
        let ev = self.load(&elem, &ep);
        self.line(&format!("store {} {ev}, ptr {slot}", elem.ll()));
        self.gen_block(&f.body.stmts);
        if !self.terminated {
            // Reload the index after the body: the body may span a resume edge,
            // so the value loaded at the top of the block cannot be reused here.
            let iv2 = self.load(&CTy::Int(64), &i);
            let ni = self.op2("add", "i64", &iv2, "1");
            self.line(&format!("store i64 {ni}, ptr {i}"));
            self.br(&cond);
        }
        self.place_label(&end);
    }

    /// The data pointer, length, and element type a `for` loop iterates. An array
    /// lvalue iterates in place through its address with a static length. A slice
    /// uses its fat pointer parts. An array rvalue spills to a slot so its elements
    /// have an address.
    fn for_source(&mut self, iter: &Expr) -> (String, String, CTy) {
        if let Some((CTy::Array(elem, n), addr)) = self.gen_place(iter) {
            return (addr, n.to_string(), *elem);
        }
        let sv = self.gen_collection(iter);
        if let CTy::Array(elem, n) = sv.ty.clone() {
            let slot = self.alloca(&sv.ty);
            self.line(&format!("store {} {}, ptr {slot}", sv.ty.ll(), sv.op));
            return (slot, n.to_string(), *elem);
        }
        self.slice_parts(&sv)
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
                match pv.ty {
                    CTy::Ptr(inner) => {
                        let data = self.fat_checked(&pv.op);
                        Some((*inner, data))
                    }
                    CTy::RawPtr(inner) => Some((*inner, pv.op)),
                    _ => None,
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
                CTy::Array(elem, n) => {
                    self.bounds_check(i, &n.to_string());
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = getelementptr {}, ptr {bptr}, i64 0, i64 {i}",
                        bty.ll()
                    ));
                    Some(((**elem).clone(), p))
                }
                CTy::Slice(elem) => {
                    let lenp = self.fresh();
                    self.line(&format!(
                        "{lenp} = getelementptr {{ ptr, i64 }}, ptr {bptr}, i32 0, i32 1"
                    ));
                    let len = self.fresh();
                    self.line(&format!("{len} = load i64, ptr {lenp}"));
                    self.bounds_check(i, &len);
                    let data = self.load(&CTy::RawPtr(elem.clone()), &bptr);
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = getelementptr {}, ptr {data}, i64 {i}",
                        elem.ll()
                    ));
                    Some(((**elem).clone(), p))
                }
                CTy::RawPtr(elem) => {
                    let pv = self.load(&CTy::RawPtr(elem.clone()), &bptr);
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = getelementptr {}, ptr {pv}, i64 {i}",
                        elem.ll()
                    ));
                    Some(((**elem).clone(), p))
                }
                CTy::Ptr(elem) => {
                    let fat = self.load(&CTy::Ptr(elem.clone()), &bptr);
                    let pv = self.fat_checked(&fat);
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = getelementptr {}, ptr {pv}, i64 {i}",
                        elem.ll()
                    ));
                    Some(((**elem).clone(), p))
                }
                _ => None,
            }
        } else {
            let bv = self.gen_expr(base);
            match &bv.ty {
                CTy::Slice(elem) => {
                    let len = self.fresh();
                    self.line(&format!("{len} = extractvalue {{ ptr, i64 }} {}, 1", bv.op));
                    self.bounds_check(i, &len);
                    let data = self.fresh();
                    self.line(&format!(
                        "{data} = extractvalue {{ ptr, i64 }} {}, 0",
                        bv.op
                    ));
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = getelementptr {}, ptr {data}, i64 {i}",
                        elem.ll()
                    ));
                    Some(((**elem).clone(), p))
                }
                CTy::RawPtr(elem) => {
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = getelementptr {}, ptr {}, i64 {i}",
                        elem.ll(),
                        bv.op
                    ));
                    Some(((**elem).clone(), p))
                }
                CTy::Ptr(elem) => {
                    let pv = self.fat_checked(&bv.op);
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = getelementptr {}, ptr {pv}, i64 {i}",
                        elem.ll()
                    ));
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
            ExprKind::Char(c) => Val::new(CTy::Char, (*c as u8).to_string()),
            ExprKind::Rune(c) => Val::new(CTy::Int(32), (*c as u32).to_string()),
            ExprKind::Str(s) => Val::new(CTy::RawPtr(Box::new(CTy::Char)), self.m.cstring(s)),
            ExprKind::Ident(_) | ExprKind::Field(..) | ExprKind::Unary(UnOp::Deref, _) => {
                self.gen_load(e)
            }
            ExprKind::Index(base, idx) => {
                if let ExprKind::Range(lo, hi, inclusive) = &idx.kind {
                    self.gen_slice(base, lo, hi, *inclusive)
                } else {
                    self.gen_load(e)
                }
            }
            ExprKind::Array(elems) => self.gen_array_lit(elems, None),
            ExprKind::Tuple(elems) => self.gen_tuple(elems),
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
            ExprKind::Collect { ty, arg } => self.gen_collect(ty, arg),
            _ => Val::i0(),
        }
    }

    /// Mints a `collector<T>(value)` on the collected heap. The kind of element
    /// picks the rep and the minting path: a function element mints a collected
    /// closure environment, a slice element deep copies its backing onto the
    /// collected heap, and any other element is the plain managed-pointer mint.
    fn gen_collect(&mut self, ty: &Type, arg: &Expr) -> Val {
        match ty {
            Type::Func(..) => self.gen_collect_closure(arg),
            Type::Slice(..) => self.gen_collect_slice(ty, arg),
            _ => self.gen_collect_plain(ty, arg),
        }
    }

    /// The plain collector kind: mirrors `alloc_of` exactly, swapping only the
    /// allocator. The value is evaluated first, then a collected block is minted
    /// with `cool_collect_alloc`, then the value is stored into it. The collected
    /// block carries the same 16 byte header as a generational one, so the
    /// generation word sits at `p - 8` and the fat value reads it exactly as a
    /// managed pointer does. No allocation runs between the mint and the rooting
    /// store, so a nested mint can never sweep the fresh block before it is
    /// reachable.
    fn gen_collect_plain(&mut self, ty: &Type, arg: &Expr) -> Val {
        let value = self.gen_expr(arg);
        let pointee = lower_ty(ty, &|n| self.ctx.nom(n));
        let szp = self.fresh();
        self.line(&format!(
            "{szp} = getelementptr {}, ptr null, i64 1",
            pointee.ll()
        ));
        let sz = self.fresh();
        self.line(&format!("{sz} = ptrtoint ptr {szp} to i64"));
        let p = self.fresh();
        self.line(&format!("{p} = call ptr @cool_collect_alloc(i64 {sz})"));
        let hp = self.fresh();
        self.line(&format!("{hp} = getelementptr i8, ptr {p}, i64 -8"));
        let g = self.fresh();
        self.line(&format!("{g} = load atomic i64, ptr {hp} seq_cst, align 8"));
        let op = self.adapt(value, &pointee).op;
        self.line(&format!("store {} {op}, ptr {p}", pointee.ll()));
        let fat = self.fat(&p, &g);
        Val::new(CTy::Ptr(Box::new(pointee)), fat)
    }

    /// The closure collector kind: reuses lambda lowering, minting the captured
    /// environment on the collected heap instead of a stack alloca or the task
    /// env arena. The captures are loaded first, before the collected block is
    /// minted, so a collection triggered by the mint cannot sweep an environment
    /// no root reaches yet; then nothing allocates between the mint and the
    /// capture stores, and the `{ env, fn }` value the collector's stack root
    /// keeps live pins the environment. Sema guarantees a lambda literal here and
    /// that every capture is immortal safe, so the collected environment holds no
    /// frame view. A capture-free lambda has no environment to collect and stays a
    /// null-env closure.
    fn gen_collect_closure(&mut self, arg: &Expr) -> Val {
        let ExprKind::Lambda(l) = &arg.kind else {
            debug_assert!(false, "a collected closure must be a lambda literal");
            return Val::i0();
        };
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
            caps.iter()
                .map(|(_, t)| t.ll())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let env = if caps.is_empty() {
            "null".to_string()
        } else {
            // Load every capture before the mint. A capture load is a frame read
            // that never allocates, so a collection cannot run during it, but the
            // loads are pulled ahead of the mint anyway to keep the rooting
            // invariant explicit: only stores stand between the mint and the point
            // the environment becomes reachable.
            let loaded: Vec<(CTy, String)> = caps
                .iter()
                .map(|(cname, cty)| {
                    let (lty, lptr) = self.locals.get(cname).cloned().unwrap();
                    let v = self.load(&lty, &lptr);
                    (cty.clone(), v)
                })
                .collect();
            let szp = self.fresh();
            self.line(&format!("{szp} = getelementptr {env_ty}, ptr null, i64 1"));
            let sz = self.fresh();
            self.line(&format!("{sz} = ptrtoint ptr {szp} to i64"));
            let e = self.fresh();
            self.line(&format!("{e} = call ptr @cool_collect_alloc(i64 {sz})"));
            for (i, (cty, v)) in loaded.iter().enumerate() {
                let slot = self.fresh();
                self.line(&format!(
                    "{slot} = getelementptr {env_ty}, ptr {e}, i32 0, i32 {i}"
                ));
                self.line(&format!("store {} {v}, ptr {slot}", cty.ll()));
            }
            e
        };
        self.emit_lambda_fn(&fname, &env_ty, &caps, &params, &ret, &l.body);
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {{ ptr, ptr }} undef, ptr {env}, 0"
        ));
        let b = self.fresh();
        self.line(&format!(
            "{b} = insertvalue {{ ptr, ptr }} {a}, ptr {fname}, 1"
        ));
        let pt = params.iter().map(|(_, t)| t.clone()).collect();
        Val::new(CTy::Closure(pt, Box::new(ret)), b)
    }

    /// The slice collector kind: deep copies the source slice's elements onto the
    /// collected heap and yields a slice `{ ptr, len }` over that immortal
    /// backing. The source is evaluated first, then the backing is minted, then
    /// the copy loop runs; no allocation stands between the mint and the copy, so
    /// a collection cannot sweep the backing before it holds the elements, and the
    /// slice value the collector's stack root keeps live pins it. The elements are
    /// copied by value, which sema guarantees is a deep copy: the element type is
    /// immortal safe, so it buries no fat pointer that a shallow copy would leave
    /// viewing the frame. A frame-view source is therefore legal here, unlike the
    /// plain kind, since the copy immortalizes it.
    fn gen_collect_slice(&mut self, ty: &Type, arg: &Expr) -> Val {
        let sty = lower_ty(ty, &|n| self.ctx.nom(n));
        let elem = match &sty {
            CTy::Slice(e) => (**e).clone(),
            _ => CTy::Int(64),
        };
        // Evaluate the source and view it as a slice. An array literal or a bare
        // array binding materializes on the frame and adapts to a slice; a slice
        // value passes through. This all happens before the collected mint.
        let src = self.gen_collection(arg);
        let src = self.adapt(src, &sty);
        let (data, len, _) = self.slice_parts(&src);
        let esz = self.elem_size(&elem);
        let total = self.op2("mul", "i64", &len, &esz);
        let out = self.fresh();
        self.line(&format!(
            "{out} = call ptr @cool_collect_alloc(i64 {total})"
        ));
        // Element copy loop: out[i] = src[i]. No allocation runs in the loop, so
        // the backing stays unswept while it is filled.
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
        let sp = self.fresh();
        self.line(&format!(
            "{sp} = getelementptr {}, ptr {data}, i64 {iv}",
            elem.ll()
        ));
        let ev = self.load(&elem, &sp);
        let dp = self.fresh();
        self.line(&format!(
            "{dp} = getelementptr {}, ptr {out}, i64 {iv}",
            elem.ll()
        ));
        self.line(&format!("store {} {ev}, ptr {dp}", elem.ll()));
        let ni = self.op2("add", "i64", &iv, "1");
        self.line(&format!("store i64 {ni}, ptr {i}"));
        self.br(&cond);
        self.place_label(&end);
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {{ ptr, i64 }} undef, ptr {out}, 0"
        ));
        let b = self.fresh();
        self.line(&format!(
            "{b} = insertvalue {{ ptr, i64 }} {a}, i64 {len}, 1"
        ));
        Val::new(CTy::Slice(Box::new(elem)), b)
    }

    fn gen_load(&mut self, e: &Expr) -> Val {
        if let Some((ty, ptr)) = self.gen_place(e) {
            let v = self.load(&ty, &ptr);
            return Val::new(ty, v);
        }
        // A bare top-level function name used as a value (`f := id`) is not a
        // place, so gen_place misses it. Resolve it to a closure `{ env, fn }`
        // over a null env and a forwarding thunk, the same representation a
        // lambda gets, so a later `f(x)` dispatches through the closure path.
        // A local of the same name (a shadow) already resolved above; async
        // functions have no plain `@name` symbol, so they stay out.
        if let ExprKind::Ident(name) = &e.kind {
            if !self.locals.contains_key(name)
                && self.ctx.fns.contains_key(name)
                && !self.ctx.async_fns.contains_key(name)
            {
                return self.func_value(name);
            }
        }
        if let ExprKind::Field(base, field) = &e.kind {
            let bv = self.gen_expr(base);
            match &bv.ty {
                CTy::Struct(t) => {
                    if let Some((idx, fty)) = self.ctx.field(t, field) {
                        let d = self.fresh();
                        self.line(&format!(
                            "{d} = extractvalue {} {}, {idx}",
                            bv.ty.ll(),
                            bv.op
                        ));
                        return Val::new(fty, d);
                    }
                }
                CTy::Slice(elem) => {
                    let (idx, fty) = match field.as_str() {
                        "ptr" => (0, CTy::RawPtr(elem.clone())),
                        "len" => (1, CTy::Int(64)),
                        _ => return Val::i0(),
                    };
                    let d = self.fresh();
                    self.line(&format!(
                        "{d} = extractvalue {{ ptr, i64 }} {}, {idx}",
                        bv.op
                    ));
                    return Val::new(fty, d);
                }
                // A fixed array carries its length in its type, so `.len` is the
                // compile-time constant `n` as an int64, matching a slice's `.len`
                // width. sema rejects every other field on an array. The base value
                // is still materialized above so any side effect in it runs.
                CTy::Array(_, n) if field == "len" => {
                    return Val::new(CTy::Int(64), n.to_string());
                }
                // `e.message` reads the error's one field through the same null
                // guarded lowering as `e.toString()`; sema has already rejected
                // every other field name on an error.
                CTy::Error if field == "message" => {
                    return self.error_to_string(&bv);
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
            UnOp::BitNot => {
                let r = self.op2("xor", &v.ty.ll(), &v.op, "-1");
                Val::new(v.ty, r)
            }
            UnOp::Deref => v,
        }
    }

    fn gen_binary(&mut self, op: BinOp, a: &Expr, b: &Expr) -> Val {
        let av = self.gen_expr(a);
        self.gen_binop_vals(op, av, b)
    }

    /// Lowers a binary operation given a pre-evaluated left value and the right
    /// operand as an expression. Compound assignment reuses this after loading a
    /// place once, and passing the right operand as an expression keeps the shift
    /// guard able to see a literal amount for elision.
    fn gen_binop_vals(&mut self, op: BinOp, av: Val, b: &Expr) -> Val {
        use BinOp::*;
        let bv = self.gen_expr(b);
        let ty = av.ty.clone();
        // Integer `**` widens both operands to i64, calls the runtime helper, and
        // truncates back. Truncation commutes with modular multiplication, so this
        // is exact for the wrapping semantics the bare `mul` already has.
        if matches!(op, Pow) && !ty.is_float() {
            let a64 = self.coerce(&av.ty, &av.op, &CTy::Int(64));
            let e64 = self.coerce(&bv.ty, &bv.op, &CTy::Int(64));
            let d = self.fresh();
            self.line(&format!(
                "{d} = call i64 @cool_pow_i64(i64 {a64}, i64 {e64})"
            ));
            let back = self.coerce(&CTy::Int(64), &d, &ty);
            return Val::new(ty, back);
        }
        let bo = self.coerce(&bv.ty, &bv.op, &av.ty);
        let is_float = ty.is_float();
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
            BitAnd => {
                let r = self.op2("and", &ty.ll(), &av.op, &bo);
                Val::new(ty, r)
            }
            BitOr => {
                let r = self.op2("or", &ty.ll(), &av.op, &bo);
                Val::new(ty, r)
            }
            BitXor => {
                let r = self.op2("xor", &ty.ll(), &av.op, &bo);
                Val::new(ty, r)
            }
            Shl => self.gen_shift("shl", &av.op, b, &bo, ty),
            Shr => self.gen_shift("ashr", &av.op, b, &bo, ty),
            // Float `**` lowers to the LLVM pow intrinsic for the operand width.
            Pow => {
                let tyll = ty.ll();
                let intr = if matches!(ty, CTy::F32) {
                    "llvm.pow.f32"
                } else {
                    "llvm.pow.f64"
                };
                let d = self.fresh();
                self.line(&format!(
                    "{d} = call {tyll} @{intr}({tyll} {}, {tyll} {bo})",
                    av.op
                ));
                Val::new(ty, d)
            }
        }
    }

    /// Lowers a shift. LLVM `shl`/`ashr` are poison at an amount at or above the
    /// width, so a dynamic amount is guarded by a named runtime fault before the
    /// shift, shaped exactly like `bounds_check`. The guard is elided only when
    /// the amount is a literal already proven in range. `>>` is `ashr` for every
    /// integer because signedness is not tracked; it fills with the sign bit.
    fn gen_shift(&mut self, opcode: &str, a: &str, b: &Expr, bo: &str, ty: CTy) -> Val {
        if !shift_amount_safe(b, &ty) {
            self.shift_check(bo, &ty);
        }
        let r = self.op2(opcode, &ty.ll(), a, bo);
        Val::new(ty, r)
    }

    /// Emits the dynamic shift guard: fault when the amount is at or above the
    /// operand width. The compare runs in the operand's own width, so an int8
    /// shift checks against 8 in i8.
    fn shift_check(&mut self, amount: &str, ty: &CTy) {
        let w = ty.int_bits().unwrap_or(64);
        let tyll = ty.ll();
        let bad = self.fresh();
        self.line(&format!("{bad} = icmp uge {tyll} {amount}, {w}"));
        let fault = self.new_label();
        let ok = self.new_label();
        self.cond_br(&bad, &fault, &ok);
        self.place_label(&fault);
        self.line("call void @cool_shift_fault()");
        self.br(&ok);
        self.place_label(&ok);
    }

    fn gen_struct_lit(&mut self, name: &str, fields: &[(String, Expr)]) -> Val {
        if name == "error" {
            return self.gen_error_lit(fields);
        }
        let ty = CTy::Struct(name.to_string());
        let mut agg = "undef".to_string();
        for (fname, fexpr) in fields {
            let Some((idx, fty)) = self.ctx.field(name, fname) else {
                continue;
            };
            let fv = self.gen_expr(fexpr);
            // adapt, not coerce: a struct field of interface type boxes its
            // value, and a slice field views an array literal; both fat
            // conversions live in adapt. A scalar field falls through to coerce.
            let op = self.adapt(fv, &fty).op;
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

    /// Builds an `error` value. An error is a message string pointer, where a
    /// null pointer means no error. `error {}` is no error and `error { message:
    /// m }` carries the message m.
    fn gen_error_lit(&mut self, fields: &[(String, Expr)]) -> Val {
        let op = match fields.iter().find(|(n, _)| n == "message") {
            Some((_, e)) => self.gen_expr(e).op,
            None => "null".to_string(),
        };
        Val::new(CTy::Error, op)
    }

    fn gen_array_lit(&mut self, elems: &[Expr], hint: Option<CTy>) -> Val {
        // With a declared element type, thread it into the elements so a nested
        // array literal builds at the right inner type. A ragged nested literal
        // then does not guess a fixed length from its first sibling: each inner
        // literal is built to the hint (a slice element makes them all uniform
        // { ptr, i64 } regardless of length). Without a hint, lower hint-less and
        // guess the element type from the first element.
        let (elem_ty, vals) = match hint {
            Some(h) => {
                let inner_hint = match &h {
                    CTy::Slice(e) | CTy::Array(e, _) => Some((**e).clone()),
                    _ => None,
                };
                let mut vals = Vec::with_capacity(elems.len());
                for e in elems {
                    let v = match (&e.kind, &inner_hint) {
                        (ExprKind::Array(inner), Some(ih)) => {
                            self.gen_array_lit(inner, Some(ih.clone()))
                        }
                        _ => self.gen_expr(e),
                    };
                    vals.push(v);
                }
                (h, vals)
            }
            None => {
                let vals: Vec<Val> = elems.iter().map(|e| self.gen_expr(e)).collect();
                let elem_ty = vals.first().map(|v| v.ty.clone()).unwrap_or(CTy::Int(64));
                (elem_ty, vals)
            }
        };
        let aty = CTy::Array(Box::new(elem_ty.clone()), vals.len() as u64);
        let mut agg = "undef".to_string();
        for (i, v) in vals.into_iter().enumerate() {
            // Adapt, not raw coerce: a struct element boxes to an interface and
            // an inner array element views as a slice when the element type is fat
            // (both conversions live in adapt); a scalar element falls through to
            // coerce unchanged.
            let op = self.adapt(v, &elem_ty).op;
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

    /// Builds a tuple aggregate from its element values, as in `(q, e)`.
    fn gen_tuple(&mut self, elems: &[Expr]) -> Val {
        let vals: Vec<Val> = elems.iter().map(|e| self.gen_expr(e)).collect();
        let tty = CTy::Tuple(vals.iter().map(|v| v.ty.clone()).collect());
        let mut agg = "undef".to_string();
        for (i, v) in vals.into_iter().enumerate() {
            let d = self.fresh();
            self.line(&format!(
                "{d} = insertvalue {} {agg}, {} {}, {i}",
                tty.ll(),
                v.ty.ll(),
                v.op
            ));
            agg = d;
        }
        Val::new(tty, agg)
    }

    /// Builds a slice `{ ptr, len }` viewing `base[lo..hi]`. The range is
    /// validated: `lo <= hi` always, and `hi <= base.len` when the base carries a
    /// length (an array or a slice), so a slice can never fabricate a length
    /// that launders out of bounds reads past the index check. A raw pointer
    /// base has no length and stays the programmer's responsibility.
    fn gen_slice(&mut self, base: &Expr, lo: &Expr, hi: &Expr, inclusive: bool) -> Val {
        let lov = self.gen_expr(lo);
        let lo_i = self.coerce(&lov.ty, &lov.op, &CTy::Int(64));
        let hiv = self.gen_expr(hi);
        let mut hi_i = self.coerce(&hiv.ty, &hiv.op, &CTy::Int(64));
        // An inclusive `lo..=hi` is `lo..hi+1`. The +1 lands here, before the
        // `lo <= hi` and `hi <= len` checks, so a pathological hi of int64 max
        // wraps and faults on the bounds check like any other out of range end.
        if inclusive {
            hi_i = self.op2("add", "i64", &hi_i, "1");
        }
        let Some((data0, base_len, elem)) = self.slice_base(base) else {
            return Val::i0();
        };
        // An unsigned lo <= hi compare also traps a negative bound, which wraps
        // to a huge value; unsigned hi <= base.len then bounds the window.
        let bad = self.fresh();
        self.line(&format!("{bad} = icmp ugt i64 {lo_i}, {hi_i}"));
        let fault = self.new_label();
        let ok = self.new_label();
        self.cond_br(&bad, &fault, &ok);
        self.place_label(&fault);
        self.line("call void @cool_bounds_fault()");
        self.br(&ok);
        self.place_label(&ok);
        if let Some(bl) = &base_len {
            let over = self.fresh();
            self.line(&format!("{over} = icmp ugt i64 {hi_i}, {bl}"));
            let fault2 = self.new_label();
            let ok2 = self.new_label();
            self.cond_br(&over, &fault2, &ok2);
            self.place_label(&fault2);
            self.line("call void @cool_bounds_fault()");
            self.br(&ok2);
            self.place_label(&ok2);
        }
        let len = self.op2("sub", "i64", &hi_i, &lo_i);
        let data = self.fresh();
        self.line(&format!(
            "{data} = getelementptr {}, ptr {data0}, i64 {lo_i}",
            elem.ll()
        ));
        let sty = CTy::Slice(Box::new(elem));
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {{ ptr, i64 }} undef, ptr {data}, 0"
        ));
        let b = self.fresh();
        self.line(&format!(
            "{b} = insertvalue {{ ptr, i64 }} {a}, i64 {len}, 1"
        ));
        Val::new(sty, b)
    }

    /// Resolves a sliceable base to its element 0 pointer, its length when the
    /// type carries one, and its element type.
    fn slice_base(&mut self, base: &Expr) -> Option<(String, Option<String>, CTy)> {
        if let Some((bty, bptr)) = self.gen_place(base) {
            return match &bty {
                CTy::Array(elem, n) => {
                    let p = self.fresh();
                    self.line(&format!(
                        "{p} = getelementptr {}, ptr {bptr}, i64 0, i64 0",
                        bty.ll()
                    ));
                    Some((p, Some(n.to_string()), (**elem).clone()))
                }
                CTy::Slice(elem) => {
                    let fat = self.load(&bty, &bptr);
                    let data = self.fresh();
                    self.line(&format!("{data} = extractvalue {{ ptr, i64 }} {fat}, 0"));
                    let len = self.fresh();
                    self.line(&format!("{len} = extractvalue {{ ptr, i64 }} {fat}, 1"));
                    Some((data, Some(len), (**elem).clone()))
                }
                CTy::RawPtr(elem) => {
                    let pv = self.load(&bty, &bptr);
                    Some((pv, None, (**elem).clone()))
                }
                CTy::Ptr(elem) => {
                    let fat = self.load(&bty, &bptr);
                    let pv = self.fat_checked(&fat);
                    Some((pv, None, (**elem).clone()))
                }
                _ => None,
            };
        }
        let bv = self.gen_expr(base);
        match bv.ty.clone() {
            CTy::Slice(elem) => {
                let data = self.fresh();
                self.line(&format!(
                    "{data} = extractvalue {{ ptr, i64 }} {}, 0",
                    bv.op
                ));
                let len = self.fresh();
                self.line(&format!("{len} = extractvalue {{ ptr, i64 }} {}, 1", bv.op));
                Some((data, Some(len), *elem))
            }
            CTy::Array(elem, n) => {
                let slot = self.alloca(&bv.ty);
                self.line(&format!("store {} {}, ptr {slot}", bv.ty.ll(), bv.op));
                let p = self.fresh();
                self.line(&format!(
                    "{p} = getelementptr {}, ptr {slot}, i64 0, i64 0",
                    bv.ty.ll()
                ));
                Some((p, Some(n.to_string()), *elem))
            }
            CTy::RawPtr(elem) => Some((bv.op, None, *elem)),
            CTy::Ptr(elem) => {
                let pv = self.fat_checked(&bv.op);
                Some((pv, None, *elem))
            }
            _ => None,
        }
    }

    /// Recognizes `Enum.Variant` as an enum constructor reference. Only the
    /// qualified form is a constructor; sema rejects the unqualified `Variant`
    /// spelling before codegen, so a bare ident never reaches here as a variant.
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
        self.line(&format!(
            "{tp} = getelementptr {}, ptr {slot}, i32 0, i32 0",
            ety.ll()
        ));
        self.line(&format!("store i{tag_bits} {tag}, ptr {tp}"));
        if !args.is_empty() {
            let pp = self.fresh();
            self.line(&format!(
                "{pp} = getelementptr {}, ptr {slot}, i32 0, i32 1",
                ety.ll()
            ));
            for (i, a) in args.iter().enumerate() {
                let Some(fty) = fields.get(i).cloned() else {
                    self.gen_expr(a);
                    continue;
                };
                let av = self.gen_expr(a);
                // adapt, not coerce: an enum payload of interface type boxes its
                // value, a slice payload views an array literal.
                let op = self.adapt(av, &fty).op;
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
        // Always spill the scrutinee value into a fresh slot instead of matching
        // through a mid-function place address. The tag load and every payload
        // bind derive from this slot pointer, which is born at the alloca point
        // (hoisted to the entry block under the async transform) and so
        // dominates every arm reached across a resume edge. Pattern binds are
        // immutable reads, so matching a copy is identical to matching in place.
        let (ety, addr) = {
            let v = self.gen_expr(scrut);
            let slot = self.alloca(&v.ty);
            self.line(&format!("store {} {}, ptr {slot}", v.ty.ll(), v.op));
            (v.ty, slot)
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
        self.line(&format!(
            "{tp} = getelementptr {}, ptr {addr}, i32 0, i32 0",
            ety.ll()
        ));
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
        self.line(&format!(
            "switch i{tag_bits} {tag}, label %{default} [ {cases_str} ]"
        ));
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
        self.line(&format!(
            "{pp} = getelementptr {}, ptr {addr}, i32 0, i32 1",
            ety.ll()
        ));
        for (i, b) in binds.iter().enumerate() {
            let Some(fty) = fields.get(i).cloned() else {
                break;
            };
            let fp = self.field_ptr(&pp, offsets.get(i).copied().unwrap_or(0));
            // Copy the bound payload value into its own slot rather than aliasing
            // the payload GEP, which is born mid-block at the top of this arm. The
            // slot pointer is born at the alloca point (hoisted to entry under the
            // async transform), so a read of the bind after a resume edge inside
            // the arm still dominates. Pattern binds are immutable, so the copy is
            // exact.
            let val = self.load(&fty, &fp);
            let slot = self.alloca(&fty);
            self.line(&format!("store {} {val}, ptr {slot}", fty.ll()));
            self.locals.insert(b.clone(), (fty, slot));
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
                    // adapt, not coerce: a match expression whose result type is
                    // an interface boxes each arm's struct tail.
                    let op = self.adapt(v, rty).op;
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
            ExprKind::Char(_) => CTy::Char,
            ExprKind::Rune(_) => CTy::Int(32),
            ExprKind::Str(_) => CTy::RawPtr(Box::new(CTy::Char)),
            ExprKind::Ident(n) => self
                .locals
                .get(n)
                .map(|(t, _)| t.clone())
                .unwrap_or(CTy::Unknown),
            ExprKind::Binary(op, a, _) => {
                use BinOp::*;
                match op {
                    Eq | Ne | Lt | Le | Gt | Ge | And | Or => CTy::Bool,
                    _ => self.static_ty(a),
                }
            }
            ExprKind::Unary(UnOp::Not, _) => CTy::Bool,
            ExprKind::Unary(UnOp::Neg, x) | ExprKind::Unary(UnOp::BitNot, x) => self.static_ty(x),
            ExprKind::Call(f, _) => match &f.kind {
                ExprKind::Ident(n) => self
                    .ctx
                    .fns
                    .get(n)
                    .map(|(r, _)| r.clone())
                    .unwrap_or(CTy::Unknown),
                // A builtin error method resolves its return width here so a match
                // whose every arm tail is `e.toString()` sizes as a string, not the
                // i64 fallback that would store a pointer through an integer slot.
                ExprKind::Field(base, mname) if matches!(self.static_ty(base), CTy::Error) => {
                    match mname.as_str() {
                        "toString" => CTy::RawPtr(Box::new(CTy::Char)),
                        "exists" => CTy::Bool,
                        _ => CTy::Unknown,
                    }
                }
                _ => CTy::Unknown,
            },
            ExprKind::Field(base, name) => match self.static_ty(base) {
                CTy::Struct(t) => self
                    .ctx
                    .field(&t, name)
                    .map(|(_, ty)| ty)
                    .unwrap_or(CTy::Unknown),
                CTy::Slice(elem) => match name.as_str() {
                    "ptr" => CTy::RawPtr(elem),
                    "len" => CTy::Int(64),
                    _ => CTy::Unknown,
                },
                // `e.message` reads as the same string type `e.toString()` yields,
                // matching the gen_load lowering so a match over message reads sizes
                // its result slot as a pointer.
                CTy::Error if name == "message" => CTy::RawPtr(Box::new(CTy::Char)),
                _ => CTy::Unknown,
            },
            ExprKind::Index(base, _) => match self.static_ty(base) {
                CTy::Array(e, _) | CTy::Slice(e) | CTy::Ptr(e) | CTy::RawPtr(e) => *e,
                _ => CTy::Unknown,
            },
            ExprKind::StructLit(name, _) => {
                if name == "error" {
                    CTy::Error
                } else {
                    CTy::Struct(name.clone())
                }
            }
            ExprKind::Tuple(xs) => CTy::Tuple(xs.iter().map(|x| self.static_ty(x)).collect()),
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
            // An async function call mints a task and hands back a future; it
            // must be caught before the ordinary-call path, since an async func
            // also sits in ctx.fns under its declared (non-future) return.
            if self.ctx.async_fns.contains_key(name) {
                return self.gen_async_call(name, args);
            }
            // A user defined function of the same name wins over a builtin, so
            // builtin names stay paradigm agnostic and never shadow user code.
            if self.ctx.fns.contains_key(name) {
                return self.gen_user_call(name, args);
            }
            return match name.as_str() {
                "async_run" => self.gen_async_run(args),
                "print" => self.gen_print(args, false, false),
                "println" => self.gen_print(args, true, false),
                "printerr" => self.gen_print(args, true, true),
                "spawn" => self.gen_spawn(args),
                "join" => self.gen_join(args),
                "submit" => self.gen_submit(args),
                "alloc" => self.gen_alloc(args),
                "free" => {
                    if let Some(a) = args.first() {
                        let v = self.gen_expr(a);
                        // A managed pointer checks its remembered generation against
                        // the live header before freeing, so freeing a stale pointer
                        // to a reused block faults at the free rather than corrupting
                        // the live owner. A raw pointer is already a data pointer and
                        // carries no generation. Both free through the heap.
                        let data = match &v.ty {
                            CTy::Ptr(_) => self.fat_checked(&v.op),
                            _ => v.op.clone(),
                        };
                        self.gen_free_call(&data);
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
                "debug_alloc" => {
                    let n = args
                        .first()
                        .map(|a| self.gen_expr(a))
                        .unwrap_or_else(Val::i0);
                    let ni = self.coerce(&n.ty, &n.op, &CTy::Int(64));
                    let p = self.fresh();
                    self.line(&format!("{p} = call ptr @cool_debug_alloc(i64 {ni})"));
                    Val::new(CTy::RawPtr(Box::new(CTy::Int(8))), p)
                }
                "debug_free" => {
                    if let Some(a) = args.first() {
                        let v = self.gen_expr(a);
                        self.line(&format!("call void @cool_debug_free(ptr {})", v.op));
                    }
                    Val::new(CTy::Void, "")
                }
                "debug_leaks" => {
                    let d = self.fresh();
                    self.line(&format!("{d} = call i64 @cool_debug_leaks()"));
                    Val::new(CTy::Int(64), d)
                }
                "debug_double_frees" => {
                    let d = self.fresh();
                    self.line(&format!("{d} = call i64 @cool_debug_double_frees()"));
                    Val::new(CTy::Int(64), d)
                }
                "cstr" => {
                    // Reinterpret a NUL terminated char buffer as a string view.
                    // Both are an LLVM ptr, so this relabels the type and emits
                    // no instruction.
                    let v = args
                        .first()
                        .map(|a| self.gen_expr(a))
                        .unwrap_or_else(Val::i0);
                    Val::new(CTy::RawPtr(Box::new(CTy::Char)), v.op)
                }
                "move" => {
                    // move(x) transfers ownership to the caller. Its value is x's,
                    // a copy of the fat pointer; the source is invalidated by the
                    // static check, not at runtime.
                    args.first()
                        .map(|a| self.gen_expr(a))
                        .unwrap_or_else(Val::i0)
                }
                "read_file" => self.gen_read_file(args),
                "write_file" => self.gen_write_file(args),
                "read_line" => self.gen_stdin_read("cool_read_line", "end of input"),
                "read_all" => self.gen_stdin_read("cool_read_all", "cannot read stdin"),
                "parse_float" => self.gen_parse_float(args),
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
        // Resolve the receiver to a self pointer so the method can mutate it. An
        // lvalue passes its address; an rvalue struct is materialized to a slot; a
        // `*Struct` passes the stored pointer. Error and interface receivers keep
        // their own dispatch.
        let (tyname, selfptr) = match self.gen_place(base) {
            Some((CTy::Struct(t), pptr)) => (t, pptr),
            Some((CTy::Ptr(inner), pptr)) if matches!(*inner, CTy::Struct(_)) => {
                let CTy::Struct(t) = *inner else {
                    unreachable!()
                };
                // Load the full fat pointer and check its generation before
                // dispatch, so a method call on a freed receiver faults the same
                // way an explicit dereference does.
                let fat = self.load(&CTy::Ptr(Box::new(CTy::Struct(t.clone()))), &pptr);
                let p = self.fat_checked(&fat);
                (t, p)
            }
            Some((CTy::Error, pptr)) => {
                let v = self.load(&CTy::Error, &pptr);
                return self.gen_error_method(&Val::new(CTy::Error, v), mname, args);
            }
            Some((CTy::Iface(i), pptr)) => {
                let v = self.load(&CTy::Iface(i.clone()), &pptr);
                return self.gen_dyn_dispatch(&Val::new(CTy::Iface(i.clone()), v), &i, mname, args);
            }
            _ => {
                let bv = self.gen_expr(base);
                match &bv.ty {
                    CTy::Error => return self.gen_error_method(&bv, mname, args),
                    CTy::Iface(i) => {
                        let i = i.clone();
                        return self.gen_dyn_dispatch(&bv, &i, mname, args);
                    }
                    CTy::Struct(t) => {
                        let slot = self.alloca(&bv.ty);
                        self.line(&format!("store %{t} {}, ptr {slot}", bv.op));
                        (t.clone(), slot)
                    }
                    CTy::Ptr(inner) if matches!(**inner, CTy::Struct(_)) => {
                        let CTy::Struct(t) = (**inner).clone() else {
                            unreachable!()
                        };
                        let data = self.fat_checked(&bv.op);
                        (t, data)
                    }
                    _ => return None,
                }
            }
        };
        let key = format!("{tyname}.{mname}");
        let (ret, params) = self.ctx.methods.get(&key).cloned()?;
        let mut parts = vec![format!("ptr {selfptr}")];
        for (i, a) in args.iter().enumerate() {
            let v = self.gen_expr(a);
            let target = params.get(i + 1).cloned().unwrap_or(v.ty.clone());
            // adapt, not coerce, mirroring gen_user_call: a method argument of
            // interface type boxes a struct, a slice parameter views an array.
            let op = self.adapt(v, &target).op;
            parts.push(format!("{} {op}", target.ll()));
        }
        let argstr = parts.join(", ");
        if matches!(ret, CTy::Void) {
            self.line(&format!("call void @{tyname}.{mname}({argstr})"));
            return Some(Val::new(CTy::Void, ""));
        }
        let d = self.fresh();
        self.line(&format!(
            "{d} = call {} @{tyname}.{mname}({argstr})",
            ret.ll()
        ));
        Some(Val::new(ret, d))
    }

    /// Reads an error's message as a string. The empty error's message pointer
    /// is null; the read promises a string, so it hands back the empty string
    /// instead of a null that would crash the C printers. This is the one
    /// lowering for both `e.toString()` and the `e.message` field read, so the
    /// two can never diverge.
    fn error_to_string(&mut self, ev: &Val) -> Val {
        let empty = self.m.cstring("");
        let isnull = self.fresh();
        self.line(&format!("{isnull} = icmp eq ptr {}, null", ev.op));
        let s = self.fresh();
        self.line(&format!(
            "{s} = select i1 {isnull}, ptr {empty}, ptr {}",
            ev.op
        ));
        Val::new(CTy::RawPtr(Box::new(CTy::Char)), s)
    }

    /// Lowers the builtin methods on `error`: exists, toString, check, ignore.
    /// An error is a message pointer, null when there is no error.
    fn gen_error_method(&mut self, ev: &Val, mname: &str, args: &[Expr]) -> Option<Val> {
        match mname {
            "exists" => {
                let d = self.fresh();
                self.line(&format!("{d} = icmp ne ptr {}, null", ev.op));
                Some(Val::new(CTy::Bool, d))
            }
            "toString" => Some(self.error_to_string(ev)),
            "ignore" => Some(Val::new(CTy::Void, "")),
            "check" => {
                let cv = self.gen_expr(args.first()?);
                let cond = self.fresh();
                self.line(&format!("{cond} = icmp ne ptr {}, null", ev.op));
                let call_l = self.new_label();
                let end_l = self.new_label();
                self.cond_br(&cond, &call_l, &end_l);
                self.place_label(&call_l);
                self.invoke_closure(&cv, vec![Val::new(CTy::Error, ev.op.clone())]);
                if !self.terminated {
                    self.br(&end_l);
                }
                self.place_label(&end_l);
                Some(Val::new(CTy::Void, ""))
            }
            _ => None,
        }
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
        self.line(&format!(
            "{data} = extractvalue {{ ptr, ptr }} {}, 0",
            iv.op
        ));
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

    /// An async call: mint a task and its future, fill the frame parameter slots
    /// at the offsets frame_prefix assigns (the same offsets the poll reads), then
    /// schedule it. Nothing runs yet. The value is the fat Future$... packed from
    /// the record pointer and its header generation, the spawn epilogue shape.
    fn gen_async_call(&mut self, name: &str, args: &[Expr]) -> Val {
        let (ret, params, fut_struct) = {
            let info = self
                .ctx
                .async_fns
                .get(name)
                .expect("async callee is registered");
            (
                info.ret.clone(),
                info.params.clone(),
                info.fut_struct.clone(),
            )
        };
        let ret_sa = self.ctx.size_align(&ret);
        let param_sa: Vec<(u64, u64)> = params.iter().map(|p| self.ctx.size_align(p)).collect();
        let prefix = crate::codegen::frame::frame_prefix(ret_sa, &param_sa);
        // result_size is the future element size, at least one so a void task
        // still owns a byte the runtime can address.
        let result_size = ret_sa.0.max(1);
        let fs = self.fresh();
        self.line(&format!("{fs} = load i64, ptr @async.{name}.framesize"));
        let fut = self.fresh();
        self.line(&format!(
            "{fut} = call ptr @cool_task_new(ptr @async.{name}.poll, i64 {fs}, i64 {result_size})"
        ));
        let fr = self.fresh();
        self.line(&format!("{fr} = call ptr @cool_task_frame(ptr {fut})"));
        for (i, a) in args.iter().enumerate() {
            let v = self.gen_expr(a);
            let target = params.get(i).cloned().unwrap_or(v.ty.clone());
            let av = self.adapt(v, &target);
            let off = prefix.param_offs.get(i).copied().unwrap_or(0);
            let dst = self.fresh();
            self.line(&format!("{dst} = getelementptr i8, ptr {fr}, i64 {off}"));
            self.line(&format!("store {} {}, ptr {dst}", target.ll(), av.op));
        }
        self.line(&format!("call void @cool_task_start(ptr {fut})"));
        let hp = self.fresh();
        self.line(&format!("{hp} = getelementptr i8, ptr {fut}, i64 -8"));
        let g = self.fresh();
        self.line(&format!("{g} = load atomic i64, ptr {hp} seq_cst, align 8"));
        let sty = CTy::Struct(fut_struct);
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {} undef, ptr {fut}, 0",
            sty.ll()
        ));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {} {a}, i64 {g}, 1", sty.ll()));
        Val::new(sty, b)
    }

    /// async_run(g(args)): the only sync-to-async bridge. The argument is a direct
    /// call of an async func (typeck verified), so evaluating it mints and starts
    /// the task and yields its future; extract the handle words, crank the loop
    /// until it completes, and load the result. The err word is discarded here,
    /// since a task future completes with a NULL error and carries any failure in
    /// its returned value.
    fn gen_async_run(&mut self, args: &[Expr]) -> Val {
        let Some(arg) = args.first() else {
            return Val::i0();
        };
        let ret = match &arg.kind {
            ExprKind::Call(callee, _) => match &callee.kind {
                ExprKind::Ident(g) => self.ctx.async_fns.get(g).map(|i| i.ret.clone()),
                _ => None,
            },
            _ => None,
        }
        .unwrap_or(CTy::Int(32));
        let futval = self.gen_expr(arg);
        let h = self.fresh();
        self.line(&format!(
            "{h} = extractvalue {} {}, 0",
            futval.ty.ll(),
            futval.op
        ));
        let gen = self.fresh();
        self.line(&format!(
            "{gen} = extractvalue {} {}, 1",
            futval.ty.ll(),
            futval.op
        ));
        let (rsize, _) = self.ctx.size_align(&ret);
        if matches!(ret, CTy::Void) {
            let out = self.alloca_raw("i64");
            self.line(&format!(
                "call void @cool_loop_run(ptr {h}, i64 {gen}, ptr {out}, i64 0)"
            ));
            return Val::new(CTy::Void, "");
        }
        let out = self.alloca(&ret);
        self.line(&format!(
            "call void @cool_loop_run(ptr {h}, i64 {gen}, ptr {out}, i64 {rsize})"
        ));
        let r = self.load(&ret, &out);
        Val::new(ret, r)
    }

    fn alloca_raw(&mut self, llty: &str) -> String {
        if self.frame.is_some() {
            // Every raw scratch slot an async body mints today is a machine word
            // (a loop counter or a scratch pointer). Long-lived closure envs use
            // cool_task_env_alloc instead and do not reach here.
            debug_assert!(
                llty == "i64" || llty == "ptr",
                "an async frame raw slot must be a word, not {llty}"
            );
            return self.frame_slot(llty.to_string(), (8, 8));
        }
        let d = self.fresh();
        self.push_entry_alloca(&format!("{d} = alloca {llty}"));
        d
    }

    /// Free variables of a lambda that are bound in the enclosing function, with
    /// their lowered types. These become the captured environment.
    fn lambda_captures(&self, l: &Lambda) -> Vec<(String, CTy)> {
        let mut used = Vec::new();
        let mut bound: HashSet<String> = l.params.iter().map(|p| p.name.clone()).collect();
        ast::collect_block(&l.body, &mut used, &mut bound);
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
            caps.iter()
                .map(|(_, t)| t.ll())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let env = if caps.is_empty() {
            "null".to_string()
        } else {
            let e = if self.frame.is_some() {
                // In an async body the env cannot live in a frame slot: a closure
                // can outlive a poll turn, and a per-iteration closure needs its
                // own storage, which a reused slot would alias. Allocate it per
                // execution from the task's env arena, which cool_task_return
                // frees at completion. The alloca_raw frame path stays word-only,
                // so a multi-field env must never route through it.
                let (size, _) = self.ctx.layout(&caps);
                let p = self.fresh();
                self.line(&format!(
                    "{p} = call ptr @cool_task_env_alloc(ptr %frame, i64 {size})"
                ));
                p
            } else {
                self.alloca_raw(&env_ty)
            };
            for (i, (cname, cty)) in caps.iter().enumerate() {
                let (lty, lptr) = self.locals.get(cname).cloned().unwrap();
                let v = self.load(&lty, &lptr);
                let slot = self.fresh();
                self.line(&format!(
                    "{slot} = getelementptr {env_ty}, ptr {e}, i32 0, i32 {i}"
                ));
                self.line(&format!("store {} {v}, ptr {slot}", cty.ll()));
            }
            e
        };
        self.emit_lambda_fn(&fname, &env_ty, &caps, &params, &ret, &l.body);
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {{ ptr, ptr }} undef, ptr {env}, 0"
        ));
        let b = self.fresh();
        self.line(&format!(
            "{b} = insertvalue {{ ptr, ptr }} {a}, ptr {fname}, 1"
        ));
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
            .chain(
                params
                    .iter()
                    .enumerate()
                    .map(|(i, (_, t))| format!("{} %a{i}", t.ll())),
            )
            .collect::<Vec<_>>()
            .join(", ");
        let header = format!("define {} {fname}({sig}) {{", ret.ll());
        let mut fb = Fb::new(&mut *self.m, self.ctx, ret.clone());
        fb.body.push_str("entry:\n");
        for (i, (cname, cty)) in caps.iter().enumerate() {
            let slot = fb.fresh();
            fb.line(&format!(
                "{slot} = getelementptr {env_ty}, ptr %env, i32 0, i32 {i}"
            ));
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
        let def = format!("{header}\n{}}}", fb.body_out());
        self.m.push_function(def);
    }

    /// spawn(lambda () -> void { ... }): emits the lambda function, copies its
    /// captured environment into a C heap block, and hands both to the runtime,
    /// which owns the environment from that instant and frees it after the body
    /// returns. Produces a (thread, error) pair whose error fires when the OS
    /// refuses the thread. Sema guarantees the argument is a nullary void
    /// lambda literal, so its emitted form is already pthread start shaped.
    /// Emits a task lambda's function and its heap copied environment, the
    /// machinery spawn and submit share. The env lives on the C heap, not
    /// this frame, since the task outlives the caller's statement, and the
    /// runtime frees it after the body returns. Returns the function name and
    /// the env pointer operand.
    fn gen_task_env(&mut self, l: &Lambda) -> (String, String) {
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
            caps.iter()
                .map(|(_, t)| t.ll())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let env = if caps.is_empty() {
            "null".to_string()
        } else {
            let szp = self.fresh();
            self.line(&format!("{szp} = getelementptr {env_ty}, ptr null, i64 1"));
            let sz = self.fresh();
            self.line(&format!("{sz} = ptrtoint ptr {szp} to i64"));
            // cool_alloc_env aborts on exhaustion, so the capture stores below
            // never write through a null.
            let e = self.fresh();
            self.line(&format!("{e} = call ptr @cool_alloc_env(i64 {sz})"));
            for (i, (cname, cty)) in caps.iter().enumerate() {
                let (lty, lptr) = self.locals.get(cname).cloned().unwrap();
                let v = self.load(&lty, &lptr);
                let slot = self.fresh();
                self.line(&format!(
                    "{slot} = getelementptr {env_ty}, ptr {e}, i32 0, i32 {i}"
                ));
                self.line(&format!("store {} {v}, ptr {slot}", cty.ll()));
            }
            e
        };
        self.emit_lambda_fn(&fname, &env_ty, &caps, &params, &ret, &l.body);
        (fname, env)
    }

    /// spawn(lambda): starts an OS thread over the shared task env handoff.
    /// The runtime returns the handle record or NULL, and the NULL branch
    /// becomes the error half of the (thread, error) pair, so a refused spawn
    /// reads like any other error and the handle half is the null sentinel.
    fn gen_spawn(&mut self, args: &[Expr]) -> Val {
        let tty = CTy::Tuple(vec![CTy::Thread, CTy::Error]);
        let Some(ExprKind::Lambda(l)) = args.first().map(|a| &a.kind) else {
            for a in args {
                self.gen_expr(a);
            }
            return Val::new(tty, "zeroinitializer");
        };
        let l = l.clone();
        let (fname, env) = self.gen_task_env(&l);
        let rec = self.fresh();
        self.line(&format!(
            "{rec} = call ptr @cool_thread_spawn(ptr {fname}, ptr {env})"
        ));
        let hslot = self.alloca(&CTy::Thread);
        let eslot = self.alloca_raw("ptr");
        let isnull = self.fresh();
        self.line(&format!("{isnull} = icmp eq ptr {rec}, null"));
        let bad = self.new_label();
        let ok = self.new_label();
        let done = self.new_label();
        self.cond_br(&isnull, &bad, &ok);
        self.place_label(&ok);
        let hp = self.fresh();
        self.line(&format!("{hp} = getelementptr i8, ptr {rec}, i64 -8"));
        let g = self.fresh();
        self.line(&format!("{g} = load atomic i64, ptr {hp} seq_cst, align 8"));
        let fat_ok = self.fat(&rec, &g);
        self.line(&format!("store {{ ptr, i64 }} {fat_ok}, ptr {hslot}"));
        self.line(&format!("store ptr null, ptr {eslot}"));
        self.br(&done);
        self.place_label(&bad);
        let fat_bad = self.fat("null", "0");
        self.line(&format!("store {{ ptr, i64 }} {fat_bad}, ptr {hslot}"));
        let msg = self.m.cstring("cannot spawn thread");
        self.line(&format!("store ptr {msg}, ptr {eslot}"));
        self.br(&done);
        self.place_label(&done);
        let h = self.load(&CTy::Thread, &hslot);
        let e = self.load(&CTy::Error, &eslot);
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {} undef, {{ ptr, i64 }} {h}, 0",
            tty.ll()
        ));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {} {a}, ptr {e}, 1", tty.ll()));
        Val::new(tty, b)
    }

    /// submit(lambda): queues the task on the global pool through the spawn
    /// env handoff, verbatim. The pool frees the env after the body runs, or
    /// immediately when the submission is refused because the pool is not
    /// running, so no path leaks it. The error fires on a refusal.
    fn gen_submit(&mut self, args: &[Expr]) -> Val {
        let Some(ExprKind::Lambda(l)) = args.first().map(|a| &a.kind) else {
            for a in args {
                self.gen_expr(a);
            }
            return Val::new(CTy::Error, "null");
        };
        let l = l.clone();
        let (fname, env) = self.gen_task_env(&l);
        let rc = self.fresh();
        self.line(&format!(
            "{rc} = call i64 @cool_pool_submit(ptr {fname}, ptr {env})"
        ));
        let bad = self.fresh();
        self.line(&format!("{bad} = icmp ne i64 {rc}, 0"));
        let msg = self.m.cstring("the thread pool is not running");
        let err = self.fresh();
        self.line(&format!("{err} = select i1 {bad}, ptr {msg}, ptr null"));
        Val::new(CTy::Error, err)
    }

    /// join(t): the handle's data pointer and remembered generation both pass
    /// to the runtime, which checks and retires in one heap critical section.
    /// A double join faults there like a use after free, even when two threads
    /// race on copies of the handle. The error fires on a join failure.
    fn gen_join(&mut self, args: &[Expr]) -> Val {
        let Some(a) = args.first() else {
            return Val::new(CTy::Error, "null");
        };
        let v = self.gen_expr(a);
        let data = self.fat_data(&v.op);
        let gen = self.fresh();
        self.line(&format!("{gen} = extractvalue {{ ptr, i64 }} {}, 1", v.op));
        let rc = self.fresh();
        self.line(&format!(
            "{rc} = call i64 @cool_thread_join(ptr {data}, i64 {gen})"
        ));
        let bad = self.fresh();
        self.line(&format!("{bad} = icmp ne i64 {rc}, 0"));
        let msg = self.m.cstring("cannot join thread");
        let err = self.fresh();
        self.line(&format!("{err} = select i1 {bad}, ptr {msg}, ptr null"));
        Val::new(CTy::Error, err)
    }

    /// Materializes a bare top-level function name as a closure value: a null env
    /// paired with a forwarding thunk that carries the closure calling convention.
    /// The value's type is `Closure(params, ret)` drawn from the real lowered
    /// signature, so a call through it returns exactly what a direct call would.
    fn func_value(&mut self, name: &str) -> Val {
        let (ret, params) = self
            .ctx
            .fns
            .get(name)
            .cloned()
            .unwrap_or((CTy::Int(64), Vec::new()));
        if self.m.need_funcval_thunk(name) {
            emit_funcval_thunk(self.m, name, &ret, &params);
        }
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {{ ptr, ptr }} undef, ptr null, 0"
        ));
        let b = self.fresh();
        self.line(&format!(
            "{b} = insertvalue {{ ptr, ptr }} {a}, ptr @{name}.funcval, 1"
        ));
        Val::new(CTy::Closure(params, Box::new(ret)), b)
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
        let slot = self.backing_slot(&arr.ty);
        self.line(&format!("store {} {}, ptr {slot}", arr.ty.ll(), arr.op));
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {{ ptr, i64 }} undef, ptr {slot}, 0"
        ));
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
        self.line(&format!(
            "{data} = extractvalue {{ ptr, i64 }} {}, 0",
            sv.op
        ));
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
        self.line(&format!(
            "{ep} = getelementptr {}, ptr {data}, i64 {iv}",
            elem.ll()
        ));
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
        self.line(&format!(
            "{ep} = getelementptr {}, ptr {data}, i64 {iv}",
            elem.ll()
        ));
        let ev = self.load(&elem, &ep);
        let rv = self.invoke_closure(&cv, vec![Val::new(elem.clone(), ev)]);
        let rop = self.adapt(rv, &out_elem).op;
        let op = self.fresh();
        self.line(&format!(
            "{op} = getelementptr {}, ptr {out}, i64 {iv}",
            out_elem.ll()
        ));
        self.line(&format!("store {} {rop}, ptr {op}", out_elem.ll()));
        let ni = self.op2("add", "i64", &iv, "1");
        self.line(&format!("store i64 {ni}, ptr {i}"));
        self.br(&cond);
        self.place_label(&end);
        let sty = CTy::Slice(Box::new(out_elem));
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {{ ptr, i64 }} undef, ptr {out}, 0"
        ));
        let b = self.fresh();
        self.line(&format!(
            "{b} = insertvalue {{ ptr, i64 }} {a}, i64 {len}, 1"
        ));
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
        self.line(&format!(
            "{ep} = getelementptr {}, ptr {data}, i64 {iv}",
            elem.ll()
        ));
        let ev = self.load(&elem, &ep);
        let pv = self.invoke_closure(&cv, vec![Val::new(elem.clone(), ev.clone())]);
        let cb = self.coerce(&pv.ty, &pv.op, &CTy::Bool);
        self.cond_br(&cb, &keep, &next);
        self.place_label(&keep);
        let kcnt = self.load(&CTy::Int(64), &cnt);
        let op = self.fresh();
        self.line(&format!(
            "{op} = getelementptr {}, ptr {out}, i64 {kcnt}",
            elem.ll()
        ));
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
        self.line(&format!(
            "{a} = insertvalue {{ ptr, i64 }} undef, ptr {out}, 0"
        ));
        let b = self.fresh();
        self.line(&format!(
            "{b} = insertvalue {{ ptr, i64 }} {a}, i64 {total_cnt}, 1"
        ));
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
        let iv0 = self.adapt(init, &acc_ty).op;
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
        self.line(&format!(
            "{ep} = getelementptr {}, ptr {data}, i64 {iv}",
            elem.ll()
        ));
        let ev = self.load(&elem, &ep);
        let av = self.load(&acc_ty, &acc);
        let rv = self.invoke_closure(
            &cv,
            vec![Val::new(acc_ty.clone(), av), Val::new(elem.clone(), ev)],
        );
        let rop = self.adapt(rv, &acc_ty).op;
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
        let err = self.alloca_raw("ptr");
        let seed = self.new_label();
        let empty = self.new_label();
        let cond = self.new_label();
        let body = self.new_label();
        let done = self.new_label();
        // An empty slice has no seed element, so it takes the error path instead
        // of reading element 0 out of bounds.
        let nonempty = self.fresh();
        self.line(&format!("{nonempty} = icmp sgt i64 {len}, 0"));
        self.cond_br(&nonempty, &seed, &empty);
        self.place_label(&empty);
        self.line(&format!("store {} zeroinitializer, ptr {acc}", elem.ll()));
        let msg = self.m.cstring("reduce on empty slice");
        self.line(&format!("store ptr {msg}, ptr {err}"));
        self.br(&done);
        self.place_label(&seed);
        let e0v = self.load(&elem, &data);
        self.line(&format!("store {} {e0v}, ptr {acc}", elem.ll()));
        self.line(&format!("store ptr null, ptr {err}"));
        let i = self.alloca_raw("i64");
        self.line(&format!("store i64 1, ptr {i}"));
        self.br(&cond);
        self.place_label(&cond);
        let iv = self.load(&CTy::Int(64), &i);
        let c = self.fresh();
        self.line(&format!("{c} = icmp slt i64 {iv}, {len}"));
        self.cond_br(&c, &body, &done);
        self.place_label(&body);
        let ep = self.fresh();
        self.line(&format!(
            "{ep} = getelementptr {}, ptr {data}, i64 {iv}",
            elem.ll()
        ));
        let ev = self.load(&elem, &ep);
        let av = self.load(&elem, &acc);
        let rv = self.invoke_closure(
            &cv,
            vec![Val::new(elem.clone(), av), Val::new(elem.clone(), ev)],
        );
        let rop = self.adapt(rv, &elem).op;
        self.line(&format!("store {} {rop}, ptr {acc}", elem.ll()));
        let ni = self.op2("add", "i64", &iv, "1");
        self.line(&format!("store i64 {ni}, ptr {i}"));
        self.br(&cond);
        self.place_label(&done);
        let r = self.load(&elem, &acc);
        let e = self.load(&CTy::Error, &err);
        let tty = CTy::Tuple(vec![elem.clone(), CTy::Error]);
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {} undef, {} {r}, 0",
            tty.ll(),
            elem.ll()
        ));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {} {a}, ptr {e}, 1", tty.ll()));
        Val::new(tty, b)
    }

    /// print, println, and printerr. `print` writes with no newline, `println`
    /// appends one, and `printerr` is a println to stderr. A string literal
    /// first argument is a format string at any arity, so brace escaping and
    /// hole checking do not change shape when the value arguments disappear.
    fn gen_print(&mut self, args: &[Expr], newline: bool, errout: bool) -> Val {
        let fmt_lit = matches!(args.first().map(|a| &a.kind), Some(ExprKind::Str(_)));
        if fmt_lit {
            return self.gen_format_print(args, newline, errout);
        }
        match args.first() {
            Some(a) => {
                let v = self.gen_expr(a);
                self.print_value(&v, newline, errout);
            }
            // A bare println() prints just the newline.
            None if newline => {
                let empty = self.m.cstring("");
                let f = if errout {
                    "cool_eprint_cstr"
                } else {
                    "cool_println_cstr"
                };
                self.line(&format!("call void @{f}(ptr {empty})"));
                if errout {
                    self.print_newline(true);
                }
            }
            None => {}
        }
        Val::new(CTy::Void, "")
    }

    fn print_newline(&mut self, errout: bool) {
        let nl = self.m.cstring("\n");
        let f = if errout {
            "cool_eprint_cstr"
        } else {
            "cool_print_cstr"
        };
        self.line(&format!("call void @{f}(ptr {nl})"));
    }

    /// Prints one value, routed to its runtime printer by type. `newline` picks
    /// the `println` variant, `errout` the stderr printers, which never append a
    /// newline themselves so one code path serves print and println. A struct
    /// prints through its Display impl's `toString`; sema rejects everything
    /// else that has no printer.
    fn print_value(&mut self, v: &Val, newline: bool, errout: bool) {
        if errout {
            match &v.ty {
                CTy::RawPtr(_) | CTy::Error => {
                    self.line(&format!("call void @cool_eprint_cstr(ptr {})", v.op))
                }
                CTy::F64 | CTy::F32 => {
                    let d = self.coerce(&v.ty, &v.op, &CTy::F64);
                    self.line(&format!("call void @cool_eprint_f64(double {d})"));
                }
                CTy::Struct(_) => {
                    if let Some(s) = self.display_string(v) {
                        self.line(&format!("call void @cool_eprint_cstr(ptr {s})"));
                    }
                }
                CTy::Enum(_)
                | CTy::Slice(_)
                | CTy::Array(..)
                | CTy::Void
                | CTy::Tuple(_)
                | CTy::Thread
                | CTy::Unknown => {}
                _ => {
                    let d = self.coerce(&v.ty, &v.op, &CTy::Int(64));
                    self.line(&format!("call void @cool_eprint_i64(i64 {d})"));
                }
            }
            if newline {
                self.print_newline(true);
            }
            return;
        }
        let suffix = if newline { "ln" } else { "" };
        match &v.ty {
            CTy::RawPtr(_) | CTy::Error => {
                self.line(&format!("call void @cool_print{suffix}_cstr(ptr {})", v.op))
            }
            CTy::F64 | CTy::F32 => {
                let d = self.coerce(&v.ty, &v.op, &CTy::F64);
                self.line(&format!("call void @cool_print{suffix}_f64(double {d})"));
            }
            CTy::Struct(_) => {
                if let Some(s) = self.display_string(v) {
                    self.line(&format!("call void @cool_print{suffix}_cstr(ptr {s})"));
                }
            }
            CTy::Enum(_)
            | CTy::Slice(_)
            | CTy::Array(..)
            | CTy::Void
            | CTy::Tuple(_)
            | CTy::Thread
            | CTy::Unknown => {}
            _ => {
                let d = self.coerce(&v.ty, &v.op, &CTy::Int(64));
                self.line(&format!("call void @cool_print{suffix}_i64(i64 {d})"));
            }
        }
    }

    /// Renders a struct value to a string through its `toString` method, the
    /// Display protocol. Returns None when the type has no such method; sema has
    /// already rejected that program, so this is only a belt over braces.
    fn display_string(&mut self, v: &Val) -> Option<String> {
        let CTy::Struct(t) = &v.ty else {
            return None;
        };
        let key = format!("{t}.toString");
        if !self.ctx.methods.contains_key(&key) {
            return None;
        }
        let slot = self.alloca(&v.ty);
        self.line(&format!("store {} {}, ptr {slot}", v.ty.ll(), v.op));
        let d = self.fresh();
        self.line(&format!("{d} = call ptr @{key}(ptr {slot})"));
        Some(d)
    }

    /// A formatted print. The format string is a literal, validated in sema, so
    /// it expands at compile time into the literal segments printed verbatim and
    /// the holes printed by value type, with one trailing newline for `println`.
    /// No runtime format parser and no allocation.
    fn gen_format_print(&mut self, args: &[Expr], newline: bool, errout: bool) -> Val {
        let segs = match &args[0].kind {
            ExprKind::Str(s) => crate::fmt::parse(s).unwrap_or_default(),
            _ => Vec::new(),
        };
        let lit_printer = if errout {
            "cool_eprint_cstr"
        } else {
            "cool_print_cstr"
        };
        let mut ai = 1;
        for seg in &segs {
            match seg {
                crate::fmt::Seg::Lit(text) => {
                    let c = self.m.cstring(text);
                    self.line(&format!("call void @{lit_printer}(ptr {c})"));
                }
                crate::fmt::Seg::Hole => {
                    if let Some(a) = args.get(ai) {
                        let v = self.gen_expr(a);
                        self.print_value(&v, false, errout);
                    }
                    ai += 1;
                }
            }
        }
        if newline {
            self.print_newline(errout);
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
        let n = args
            .first()
            .map(|a| self.gen_expr(a))
            .unwrap_or_else(Val::i0);
        let ni = self.coerce(&n.ty, &n.op, &CTy::Int(64));
        let p = self.gen_alloc_call(&ni, 8);
        Val::new(CTy::RawPtr(Box::new(CTy::Int(8))), p)
    }

    /// read_file(path): slurps the whole file into a heap string, returning a
    /// `(string, error)` pair. On failure the data is the empty string and the
    /// error carries a message, so the caller's must handle rule still fires.
    fn gen_read_file(&mut self, args: &[Expr]) -> Val {
        let str_ty = CTy::RawPtr(Box::new(CTy::Char));
        let p = args
            .first()
            .map(|a| self.gen_expr(a))
            .unwrap_or_else(Val::i0);
        let pp = self.coerce(&p.ty, &p.op, &str_ty);
        let buf = self.fresh();
        self.line(&format!("{buf} = call ptr @cool_read_file(ptr {pp})"));
        let isnull = self.fresh();
        self.line(&format!("{isnull} = icmp eq ptr {buf}, null"));
        let msg = self.m.cstring("cannot read file");
        let empty = self.m.cstring("");
        let err = self.fresh();
        self.line(&format!("{err} = select i1 {isnull}, ptr {msg}, ptr null"));
        let data = self.fresh();
        self.line(&format!(
            "{data} = select i1 {isnull}, ptr {empty}, ptr {buf}"
        ));
        let tty = CTy::Tuple(vec![str_ty, CTy::Error]);
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {} undef, ptr {data}, 0",
            tty.ll()
        ));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {} {a}, ptr {err}, 1", tty.ll()));
        Val::new(tty, b)
    }

    /// write_file(path, contents): writes the string to the file, returning an
    /// `error` that exists when the write fails.
    fn gen_write_file(&mut self, args: &[Expr]) -> Val {
        let str_ty = CTy::RawPtr(Box::new(CTy::Char));
        let path = args
            .first()
            .map(|a| self.gen_expr(a))
            .unwrap_or_else(Val::i0);
        let pp = self.coerce(&path.ty, &path.op, &str_ty);
        let data = args
            .get(1)
            .map(|a| self.gen_expr(a))
            .unwrap_or_else(Val::i0);
        let dp = self.coerce(&data.ty, &data.op, &str_ty);
        let rc = self.fresh();
        self.line(&format!(
            "{rc} = call i64 @cool_write_file(ptr {pp}, ptr {dp})"
        ));
        let bad = self.fresh();
        self.line(&format!("{bad} = icmp slt i64 {rc}, 0"));
        let msg = self.m.cstring("cannot write file");
        let err = self.fresh();
        self.line(&format!("{err} = select i1 {bad}, ptr {msg}, ptr null"));
        Val::new(CTy::Error, err)
    }

    /// A stdin reader builtin. Calls the named runtime function, which returns a
    /// heap string or null, and packages the result as a `(string, error)` pair.
    /// A null hands back the empty string and an error that exists, so a read
    /// loop stops on it. `read_line` reads one line with its newline stripped and
    /// nulls at end of input, `read_all` reads the whole stream and nulls only on
    /// allocation failure.
    fn gen_stdin_read(&mut self, runtime_fn: &str, err_msg: &str) -> Val {
        let str_ty = CTy::RawPtr(Box::new(CTy::Char));
        let buf = self.fresh();
        self.line(&format!("{buf} = call ptr @{runtime_fn}()"));
        let isnull = self.fresh();
        self.line(&format!("{isnull} = icmp eq ptr {buf}, null"));
        let msg = self.m.cstring(err_msg);
        let empty = self.m.cstring("");
        let err = self.fresh();
        self.line(&format!("{err} = select i1 {isnull}, ptr {msg}, ptr null"));
        let data = self.fresh();
        self.line(&format!(
            "{data} = select i1 {isnull}, ptr {empty}, ptr {buf}"
        ));
        let tty = CTy::Tuple(vec![str_ty, CTy::Error]);
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {} undef, ptr {data}, 0",
            tty.ll()
        ));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {} {a}, ptr {err}, 1", tty.ll()));
        Val::new(tty, b)
    }

    /// parse_float(s): parses a base 10 float through the runtime strtod, which
    /// signals validity through an out pointer. Returns a `(float64, error)` pair
    /// whose error exists when the string is empty or not fully a number.
    fn gen_parse_float(&mut self, args: &[Expr]) -> Val {
        let str_ty = CTy::RawPtr(Box::new(CTy::Char));
        let s = args
            .first()
            .map(|a| self.gen_expr(a))
            .unwrap_or_else(Val::i0);
        let sp = self.coerce(&s.ty, &s.op, &str_ty);
        let okslot = self.alloca_raw("i64");
        let val = self.fresh();
        self.line(&format!(
            "{val} = call double @cool_parse_float(ptr {sp}, ptr {okslot})"
        ));
        let ok = self.load(&CTy::Int(64), &okslot);
        let bad = self.fresh();
        self.line(&format!("{bad} = icmp eq i64 {ok}, 0"));
        let msg = self.m.cstring("cannot parse float");
        let err = self.fresh();
        self.line(&format!("{err} = select i1 {bad}, ptr {msg}, ptr null"));
        let tty = CTy::Tuple(vec![CTy::F64, CTy::Error]);
        let a = self.fresh();
        self.line(&format!(
            "{a} = insertvalue {} undef, double {val}, 0",
            tty.ll()
        ));
        let b = self.fresh();
        self.line(&format!("{b} = insertvalue {} {a}, ptr {err}, 1", tty.ll()));
        Val::new(tty, b)
    }

    /// ptr_add(p, n): the pointer n bytes past p, keeping p's pointer type. Raw
    /// byte arithmetic for arenas and buffers.
    fn gen_ptr_add(&mut self, args: &[Expr]) -> Val {
        let p = args
            .first()
            .map(|a| self.gen_expr(a))
            .unwrap_or_else(Val::i0);
        let n = args
            .get(1)
            .map(|a| self.gen_expr(a))
            .unwrap_or_else(Val::i0);
        let ni = self.coerce(&n.ty, &n.op, &CTy::Int(64));
        // ptr_add works in raw pointers. A managed *T is rejected in sema, but
        // drop its generation defensively here so a fat value never reaches a
        // gep, and always hand back a raw pointer.
        let (op, ty) = match &p.ty {
            CTy::RawPtr(_) => (p.op.clone(), p.ty.clone()),
            CTy::Ptr(inner) => (self.fat_data(&p.op), CTy::RawPtr(inner.clone())),
            _ => (p.op.clone(), CTy::RawPtr(Box::new(CTy::Int(8)))),
        };
        let d = self.fresh();
        self.line(&format!("{d} = getelementptr i8, ptr {op}, i64 {ni}"));
        Val::new(ty, d)
    }

    fn gen_alloc(&mut self, args: &[Expr]) -> Val {
        let value = args.first().map(|a| self.gen_expr(a));
        let pointee = value.as_ref().map(|v| v.ty.clone()).unwrap_or(CTy::Int(64));
        self.alloc_of(pointee, value)
    }

    /// Allocates one `pointee` on the heap, storing `value` into it when given.
    /// The declared pointee sizes the block; a value of a narrower literal type
    /// coerces into it, so the store never writes past the allocation.
    fn alloc_of(&mut self, pointee: CTy, value: Option<Val>) -> Val {
        let align = self.ctx.size_align(&pointee).1;
        let szp = self.fresh();
        self.line(&format!(
            "{szp} = getelementptr {}, ptr null, i64 1",
            pointee.ll()
        ));
        let sz = self.fresh();
        self.line(&format!("{sz} = ptrtoint ptr {szp} to i64"));
        // The default heap is the generational allocator: it writes a generation
        // into the header word at p minus 8, read into the fat pointer so every
        // deref is checked. A `using` allocator hands back untracked memory, so
        // the generation is the sentinel 0 and the deref check is skipped.
        let (p, gen) = if self.allocator.is_none() {
            let p = self.fresh();
            self.line(&format!("{p} = call ptr @cool_gen_alloc(i64 {sz})"));
            let hp = self.fresh();
            self.line(&format!("{hp} = getelementptr i8, ptr {p}, i64 -8"));
            let g = self.fresh();
            self.line(&format!("{g} = load atomic i64, ptr {hp} seq_cst, align 8"));
            (p, g)
        } else {
            (self.gen_alloc_call(&sz, align), "0".to_string())
        };
        if let Some(v) = value {
            let op = self.adapt(v, &pointee).op;
            self.line(&format!("store {} {op}, ptr {p}", pointee.ll()));
        }
        let fat = self.fat(&p, &gen);
        Val::new(CTy::Ptr(Box::new(pointee)), fat)
    }

    /// Allocates `size` bytes through the in scope `using` allocator, dispatching
    /// statically on a concrete allocator type and through the vtable on an
    /// interface erased one. With no allocator in scope, uses the default heap.
    fn gen_alloc_call(&mut self, size: &str, align: u64) -> String {
        match self.allocator.clone() {
            Some((name, CTy::Struct(a))) => {
                // Pass the allocator by pointer so a stateful allocator advances in
                // place across alloc calls.
                let (_, lp) = self.locals.get(&name).cloned().unwrap();
                let p = self.fresh();
                self.line(&format!(
                    "{p} = call ptr @{a}.alloc(ptr {lp}, i64 {size}, i64 {align})"
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
                // The default heap is the generational allocator. Every block,
                // managed or a raw buffer, carries a header so free can return it
                // to the size matched free list and the generation stays sound.
                let p = self.fresh();
                self.line(&format!("{p} = call ptr @cool_gen_alloc(i64 {size})"));
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
                self.line(&format!("call void @{a}.free(ptr {lp}, ptr {p})"));
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
            _ => self.line(&format!("call void @cool_gen_free(ptr {p})")),
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
        self.line(&format!(
            "{slot} = getelementptr [{n} x ptr], ptr {vt}, i64 0, i64 {idx}"
        ));
        let fp = self.fresh();
        self.line(&format!("{fp} = load ptr, ptr {slot}"));
        Some((data, fp))
    }
}

/// Whether a shift amount is a literal already proven in range, so the runtime
/// guard can be elided. Only a bare integer literal in `[0, width)` qualifies;
/// anything else, including a value produced by mono substitution, is guarded.
fn shift_amount_safe(b: &Expr, ty: &CTy) -> bool {
    if let ExprKind::Int(v, _) = &b.kind {
        let w = ty.int_bits().unwrap_or(64) as i64;
        return *v >= 0 && *v < w;
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn ir(src: &str) -> String {
        let (t, _) = lex(src);
        let (m, e) = parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        compile(&m, &crate::mono::MutTupleTypes::new()).expect("codegen error")
    }

    #[test]
    fn foreign_block_declares_and_calls() {
        let out = ir("foreign \"C\" { func abs(n: int32) -> int32 }\n\
             func main() -> int32 { return abs(-5) }");
        assert!(out.contains("declare i32 @abs(i32)"), "{out}");
        assert!(out.contains("call i32 @abs("), "{out}");
    }

    #[test]
    fn scalar_core_still_works() {
        let out = ir("func add(a: int64, b: int64) -> int64 { return a + b }\n\
             func main() -> int32 { return add(1, 2) }");
        assert!(out.contains("define i64 @add(i64 %a0, i64 %a1)"));
        assert!(out.contains("call i64 @add"));
    }

    #[test]
    fn bitwise_ops_lower_to_llvm_bitops() {
        let out = ir("func f(x: int64, y: int64) -> int64 { return (x & y) | (x ^ y) }");
        assert!(out.contains("and i64"), "{out}");
        assert!(out.contains("or i64"), "{out}");
        assert!(out.contains("xor i64"), "{out}");
    }

    #[test]
    fn bitnot_lowers_to_xor_minus_one() {
        let out = ir("func f(x: int64) -> int64 { return ~x }");
        assert!(out.contains("xor i64"), "{out}");
        assert!(out.contains(", -1"), "{out}");
    }

    #[test]
    fn right_shift_is_arithmetic() {
        // Signedness is not tracked, so `>>` is `ashr` for every integer, never
        // the logical `lshr`, so a negative value fills with its sign bit.
        let out = ir("func f(x: int64) -> int64 { return x >> 2 }");
        assert!(out.contains("ashr i64"), "{out}");
        assert!(!out.contains("lshr"), "{out}");
    }

    #[test]
    fn dynamic_shift_is_guarded() {
        let out = ir("func f(x: int64, n: int64) -> int64 { return x << n }");
        assert!(out.contains("icmp uge i64"), "{out}");
        assert!(out.contains("call void @cool_shift_fault()"), "{out}");
    }

    #[test]
    fn constant_in_range_shift_is_not_guarded() {
        // A literal amount proven in range needs no runtime guard, so the fault
        // call is absent even though the fault is always declared.
        let out = ir("func f() -> int64 { return 1 << 3 }");
        assert!(out.contains("shl i64 1, 3"), "{out}");
        assert!(!out.contains("call void @cool_shift_fault()"), "{out}");
    }

    #[test]
    fn compound_index_assign_computes_place_once() {
        // `xs[i] += 1` must compute the element address exactly once and use it
        // for both the load and the store, so a single getelementptr appears and
        // a single bounds check fires.
        let out = ir(
            "func main() -> int32 {\n  mut xs: int64[3] = [10, 20, 30]\n  i := 1\n  xs[i] += 1\n  return 0\n}",
        );
        let geps = out.matches("getelementptr [3 x i64]").count();
        assert_eq!(
            geps, 1,
            "expected one place computation, got {geps}:\n{out}"
        );
        let checks = out.matches("call void @cool_bounds_fault()").count();
        assert_eq!(checks, 1, "expected one bounds check, got {checks}:\n{out}");
    }

    #[test]
    fn inclusive_slice_adds_one_before_bounds_checks() {
        // `xs[1..=3]` is `xs[1..4]`: the +1 lands before the `lo <= hi` and
        // `hi <= len` checks, so an inclusive end past the array still faults.
        let out = ir(
            "func f(xs: int64[5]) -> int64 {\n  s := xs[1..=3]\n  return s[0]\n}\nfunc main() -> int32 { return 0 }",
        );
        let add_pos = out.find("add i64 3, 1").expect("expected the inclusive +1");
        let ugt_pos = out.find("icmp ugt").expect("expected a bounds compare");
        assert!(
            add_pos < ugt_pos,
            "the +1 must precede the bounds checks:\n{out}"
        );
    }

    #[test]
    fn exclusive_slice_has_no_extra_add() {
        let out = ir(
            "func f(xs: int64[5]) -> int64 {\n  s := xs[1..3]\n  return s[0]\n}\nfunc main() -> int32 { return 0 }",
        );
        assert!(!out.contains("add i64 3, 1"), "{out}");
    }

    #[test]
    fn integer_pow_calls_runtime_helper() {
        let out = ir("func f(x: int64, y: int64) -> int64 { return x ** y }");
        assert!(out.contains("call i64 @cool_pow_i64(i64"), "{out}");
    }

    #[test]
    fn float_pow_calls_the_intrinsic() {
        let out = ir("func f(a: float64, b: float64) -> float64 { return a ** b }");
        assert!(out.contains("call double @llvm.pow.f64(double"), "{out}");
    }

    #[test]
    fn struct_type_and_method() {
        let out = ir("struct Point { x: int64, y: int64 }\n\
             impl Point { func sum() -> int64 { return self.x + self.y } }\n\
             func main() -> int32 {\n  p := Point { x: 3, y: 4 }\n  return 0\n}");
        assert!(out.contains("%Point = type { i64, i64 }"));
        assert!(out.contains("define i64 @Point.sum(ptr %a0)"));
        assert!(out.contains("insertvalue %Point"));
    }

    #[test]
    fn method_takes_self_by_pointer_and_mutates_in_place() {
        let out = ir("struct C { n: int64 }\n\
             impl C { func bump() -> void { self.n = self.n + 1 } }\n\
             func main() -> int32 {\n  mut c := C { n: 0 }\n  c.bump()\n  return 0\n}");
        // self is a pointer, the call passes the receiver address, and the
        // mutation writes through that pointer so it persists in the caller.
        assert!(out.contains("define void @C.bump(ptr %a0)"), "{out}");
        assert!(out.contains("@C.bump(ptr "), "{out}");
        assert!(out.contains("getelementptr %C, ptr %a0"), "{out}");
    }

    #[test]
    fn alloc_and_deref() {
        let out = ir("func f() -> int64 {\n  q: *int64 = alloc(100)\n  return *q\n}");
        // alloc goes through the generational heap and the deref checks the
        // remembered generation against the header, faulting on a stale pointer.
        assert!(out.contains("call ptr @cool_gen_alloc"), "{out}");
        assert!(out.contains("getelementptr i64, ptr null"), "{out}");
        assert!(out.contains("call void @cool_gen_fault()"), "{out}");
    }

    #[test]
    fn array_literal_and_index() {
        let out = ir("func f() -> int32 {\n  xs: int32[3] = [1, 2, 3]\n  return xs[1]\n}");
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
        let out =
            ir("func f() -> int32 {\n  mut xs: int32[2] = [1, 2]\n  xs[0] = 9\n  return xs[0]\n}");
        assert!(out.contains("store i32 "));
        assert!(out.contains("getelementptr [2 x i32], ptr"));
    }

    #[test]
    fn enum_type_and_construct() {
        let out = ir("enum E { A, B(v: int64) }\n\
             func f() -> int64 {\n  x := E.B(7)\n  return 0\n}");
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
        let out = ir("struct Dog { s: int64 }\n\
             interface Animal { speak() -> int64 }\n\
             impl Animal for Dog { func speak() -> int64 { return self.s } }\n\
             func describe(a: Animal) -> int64 { return a.speak() }\n\
             func main() -> int32 {\n  d := Dog { s: 7 }\n  return describe(d)\n}");
        assert!(out.contains("@vtable.Animal.Dog = constant [1 x ptr]"));
        assert!(out.contains("define i64 @thunk.Animal.Dog.speak(ptr %d"));
        assert!(out.contains("extractvalue { ptr, ptr }"));
    }

    #[test]
    fn struct_boxes_into_interface_arg() {
        let out = ir("struct Dog { s: int64 }\n\
             interface Animal { speak() -> int64 }\n\
             impl Animal for Dog { func speak() -> int64 { return self.s } }\n\
             func describe(a: Animal) -> int64 { return a.speak() }\n\
             func main() -> int32 {\n  d := Dog { s: 7 }\n  return describe(d)\n}");
        assert!(out.contains("insertvalue { ptr, ptr } undef, ptr"));
        assert!(out.contains("ptr @vtable.Animal.Dog, 1"));
    }

    #[test]
    fn closure_captures_and_calls() {
        let out = ir("func main() -> int32 {\n\
               base := 100\n\
               add := lambda (x: int64) -> int64 { return x + base }\n\
               return add(5)\n\
             }");
        assert!(out.contains("define i64 @lambda.0(ptr %env, i64 %a0)"));
        assert!(out.contains("getelementptr { i64 }, ptr %env"));
        assert!(out.contains("insertvalue { ptr, ptr } undef, ptr"));
    }

    #[test]
    fn closure_without_capture_uses_null_env() {
        let out = ir("func main() -> int32 {\n\
               f := lambda (a: int64, b: int64) -> int64 { return a * b }\n\
               return f(6, 7)\n\
             }");
        assert!(out.contains("define i64 @lambda.0(ptr %env, i64 %a0, i64 %a1)"));
        assert!(out.contains("insertvalue { ptr, ptr } undef, ptr null, 0"));
    }

    #[test]
    fn using_allocator_dispatches_statically() {
        let out = ir("struct Heap { id: int64 }\n\
             func work(using a: Heap) -> void {\n  p: *int64 = alloc(5)\n  free(p)\n}");
        assert!(out.contains("@Heap.alloc(ptr"), "{out}");
        assert!(out.contains("@Heap.free(ptr"), "{out}");
        assert!(
            !out.contains("call ptr @cool_alloc"),
            "should not fall back to heap: {out}"
        );
    }

    #[test]
    fn default_alloc_uses_heap() {
        let out = ir("func f() -> int64 {\n  p: *int64 = alloc(5)\n  return *p\n}");
        assert!(out.contains("call ptr @cool_gen_alloc"), "{out}");
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
        assert!(
            out.contains("call ptr @cool_gen_alloc"),
            "map allocates result: {out}"
        );
        assert!(out.contains("icmp slt i64"), "map loops: {out}");
        assert!(
            out.contains("call i64 "),
            "map invokes the closure indirectly: {out}"
        );
    }

    #[test]
    fn foreach_calls_closure_per_element() {
        let out = ir(
            "func f(xs: int64[]) -> void {\n  foreach(xs, lambda (n: int64) -> void { println(n) })\n}",
        );
        assert!(out.contains("icmp slt i64"));
        assert!(out.contains("call void @cool_println_i64"));
    }

    #[test]
    fn print_omits_the_newline_and_println_appends_it() {
        // Swapping the two runtime calls would invert the trailing newline, the
        // exact regression the 0.1.5 print split fixed. print must reach the bare
        // printer and println the ln one, each checked in isolation so a swap can
        // not hide behind the other call also being present.
        let p = ir("func f(x: int64) -> void {\n  print(x)\n}");
        assert!(p.contains("call void @cool_print_i64"), "{p}");
        assert!(!p.contains("call void @cool_println_i64"), "{p}");

        let pl = ir("func f(x: int64) -> void {\n  println(x)\n}");
        assert!(pl.contains("call void @cool_println_i64"), "{pl}");
        assert!(!pl.contains("call void @cool_print_i64"), "{pl}");
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
        let out =
            ir("func f() -> void {\n  n := sizeof(int64)\n  b := alloc_bytes(n)\n  free(b)\n}");
        // sizeof lowers to a null GEP + ptrtoint; alloc_bytes to a raw allocation.
        assert!(out.contains("getelementptr i64, ptr null, i64 1"), "{out}");
        assert!(out.contains("ptrtoint ptr"), "{out}");
        assert!(out.contains("call ptr @cool_gen_alloc"), "{out}");
    }

    #[test]
    fn bool_widens_with_zext_not_sext() {
        // true widened to an int must be 1, not the -1 that sext would give.
        let out = ir("func f() -> void {\n  print(true)\n}");
        assert!(out.contains("zext i1"), "{out}");
        assert!(!out.contains("sext i1"), "{out}");
    }

    #[test]
    fn char_widens_with_zext_not_sext() {
        // A char is an unsigned byte. Widening to an int must zero extend so a
        // byte at or above 128 stays 0 to 255, not a negative from sext.
        let out = ir("func f() -> void {\n  print('A')\n}");
        assert!(out.contains("zext i8"), "{out}");
        assert!(!out.contains("sext i8"), "{out}");
    }

    #[test]
    fn multi_bind_destructures_tuple() {
        // `q, e := f()` must extract both tuple elements into locals.
        let out = ir("func f() -> (int64, error) { return (5, error {}) }\n\
             func g() -> void {\n  q, e := f()\n  print(q)\n}");
        assert!(out.contains("extractvalue"), "{out}");
    }

    #[test]
    fn error_exists_is_a_null_check() {
        let out = ir("func f() -> (int64, error) { return (5, error {}) }\n\
             func g() -> void {\n  q, e := f()\n  if e.exists() { print(q) }\n}");
        assert!(out.contains("icmp ne ptr"), "{out}");
    }

    #[test]
    fn reduce_returns_pair_and_guards_empty() {
        let out = ir(
            "func f(xs: int64[]) -> void {\n  s, e := reduce(xs, lambda (a: int64, b: int64) -> int64 { return a + b })\n  print(s)\n}",
        );
        // The empty case stores an error message instead of reading element 0.
        assert!(out.contains("reduce on empty slice"), "{out}");
        // reduce yields a { i64, ptr } pair.
        assert!(out.contains("{ i64, ptr }"), "{out}");
    }

    #[test]
    fn sizeof_of_struct_field_uses_field_type() {
        let out = ir("struct S { a: int8, b: int64 }\n\
             func f() -> void {\n  s := S { a: 3i8, b: 9 }\n  n := sizeof(s.a)\n  print(n)\n}");
        // The field is int8, so sizeof lowers through i8, not the i64 fallback.
        assert!(out.contains("getelementptr i8, ptr null, i64 1"), "{out}");
    }

    #[test]
    fn array_literal_coerces_to_slice_argument() {
        let out = ir("func sum(xs: int64[]) -> int64 { return 0 }\n\
             func f() -> int64 { return sum([1, 2, 3]) }");
        // The literal is spilled to a stack array and passed as a slice.
        assert!(out.contains("alloca [3 x i64]"), "{out}");
        assert!(out.contains("call i64 @sum({ ptr, i64 }"), "{out}");
    }

    #[test]
    fn generic_sizeof_uses_concrete_type() {
        // sizeof(T) in a generic function must lower to the instantiated type's
        // size: int8 -> 1, int64 -> 8.
        let out = ir("func sz<T>(x: T) -> int64 { return sizeof(T) }\n\
             func main() -> int32 {\n  println(sz(5i8))\n  println(sz(5))\n  return 0\n}");
        assert!(out.contains("getelementptr i8, ptr null, i64 1"), "{out}");
        assert!(out.contains("getelementptr i64, ptr null, i64 1"), "{out}");
    }

    #[test]
    fn generic_sizeof_of_composite_type() {
        // sizeof(T) where T is a slice must size the { ptr, i64 } fat pointer (16),
        // not fall through to the i64 default (8).
        let out = ir("func sz<T>(x: T) -> int64 { return sizeof(T) }\n\
             func main() -> int32 {\n  a: int64[] = [1, 2, 3]\n  return sz(a)\n}");
        assert!(
            out.contains("getelementptr { ptr, i64 }, ptr null, i64 1"),
            "{out}"
        );
    }

    #[test]
    fn ptr_add_offsets_a_pointer() {
        let out = ir("func f(p: *raw int8) -> *raw int8 {\n  return ptr_add(p, 16)\n}");
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
