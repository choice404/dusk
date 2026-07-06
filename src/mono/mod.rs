//! Monomorphization. Expands generic functions, structs, and enums into concrete
//! copies, one per distinct set of type arguments. Runs after sema and before
//! codegen, so codegen only ever sees ground types.
//!
//! Type arguments are taken from explicit annotations where present and inferred
//! from value argument types otherwise. Each instantiation is mangled with a `$`
//! separated suffix and added to a worklist; expanding one may request others.

use std::collections::{HashMap, HashSet};

use crate::diag::{Diagnostic, Span};
use crate::parser::ast::{
    Bind, Block, Enum, Expr, ExprKind, Field, Func, Item, Let, Module, Param, Stmt, Struct, Type,
    Variant,
};

type Subst = HashMap<String, Type>;
type Env = HashMap<String, Type>;
/// An async function's signature lifted for the frame-view checks: its named
/// parameters, its return type, and its span.
type AsyncSig = (Vec<(String, Type)>, Type, Span);

/// Storage types for the narrow class of unannotated mutable tuple bindings whose
/// initializer holds an array-literal member. Typeck infers such a member as a
/// slice, since a later reassignment may store a slice there, but the initializer
/// alone shapes the member as a fixed array, so codegen would size the slot from
/// the array and reject the fat slice a reassignment stores. The surface type pass
/// records the reconciled tuple type, keyed by the binding's value span, so mono
/// can stamp it onto `Bind.ty` and drive codegen's annotated-let path, which sizes
/// the slot as a slice and adapts the array-literal member into it. Only this class
/// appears here; every other binding keeps its inferred storage untouched.
pub type MutTupleTypes = HashMap<Span, Type>;

/// Expands all generics in a module into concrete monomorphic items. `muts`
/// carries the reconciled storage types of the narrow mutable-tuple class the
/// surface type pass recorded; an empty map leaves every binding's storage as its
/// initializer shapes it.
pub fn expand(module: &Module, muts: &MutTupleTypes) -> Module {
    expand_with_diags(module, muts).0
}

/// Expands the module and returns the ground result, the inference diagnostics
/// from a single run (a type parameter no call site pins down, or an impl block
/// on a generic type, which expansion would silently default or drop), and the
/// future instantiation table (each mangled `Future$T` name to its ground
/// element). Sema expands once through here so it can feed the diagnostics, the
/// ground type re-check, and that re-check's undo of the future mangle without
/// expanding twice. Reported so `dusk check` catches the problem at the source
/// line instead of codegen emitting a wrong program.
pub fn expand_with_diags(
    module: &Module,
    muts: &MutTupleTypes,
) -> (Module, Vec<Diagnostic>, HashMap<String, Type>) {
    let mut m = Mono::new(module, muts);
    let items = m.run();
    let out = Module {
        paradigms: module.paradigms.clone(),
        imports: module.imports.clone(),
        monads: module.monads.clone(),
        items,
    };
    (out, m.diags, m.future_table)
}

/// The instantiation budget the worklist drain may spend before it is declared
/// non-terminating. A real program's distinct instantiations number in the tens
/// or hundreds; a runaway generic, one whose expansion requests a strictly
/// larger instantiation forever, would spin without bound. The ceiling is loose
/// enough that no legitimate program reaches it and tight enough that the
/// divergence stops in bounded time with a diagnostic instead of a hang. This
/// guard is a permanent invariant of the drain, not a workaround for one bug.
const INSTANTIATION_LIMIT: usize = 10_000;

struct Mono<'a> {
    items: &'a [Item],
    gfuncs: HashMap<String, &'a Func>,
    gstructs: HashMap<String, &'a Struct>,
    genums: HashMap<String, &'a Enum>,
    ifaces: HashSet<String>,
    // Each non-generic function's declared return, paired with whether it is
    // async. An async call's runtime result is the `Future<ret>` the task mints,
    // not the bare `ret` the source spells, so the static typer wraps it before a
    // later use, a pass to a generic or an await, is resolved against a shape
    // that disagrees with the value's ground layout.
    fn_ret: HashMap<String, (Type, bool)>,
    requested: HashSet<String>,
    // Each queued instantiation carries the span of the site that first requested
    // it, so a backstop diagnostic, an interface type argument or the
    // non-termination ceiling, points at real source instead of the file head and
    // two distinct violations keep distinct spans through the sema dedup.
    worklist: Vec<(String, Vec<Type>, Span)>,
    out: Vec<Item>,
    diags: Vec<Diagnostic>,
    // Every `Future<T>` instantiation minted during expansion, its mangled
    // `Future$T` name mapped to the ground element type `T`. An async call still
    // types as the surface future, but a `Future<T>` in an annotation, a
    // parameter, or a container element lowers to this mangled struct, so the
    // ground type re-check reads this table to restore the future shape and let
    // the two forms meet. Every entry flows through `enqueue`, the single gate
    // both a spelled `Future<T>` and a forced async future pass.
    future_table: HashMap<String, Type>,
    // The reconciled storage types of the narrow mutable-tuple class, keyed by the
    // binding's value span, as the surface type pass recorded them. Read only when
    // a single unannotated mutable binding's value span is present, so no other
    // binding's storage is touched.
    mut_tuple_types: &'a MutTupleTypes,
}

impl<'a> Mono<'a> {
    fn new(module: &'a Module, muts: &'a MutTupleTypes) -> Self {
        let mut gfuncs = HashMap::new();
        let mut gstructs = HashMap::new();
        let mut genums = HashMap::new();
        let mut ifaces = HashSet::new();
        let mut fn_ret = HashMap::new();
        for item in &module.items {
            match item {
                Item::Func(f) if !f.generics.is_empty() => {
                    gfuncs.insert(f.name.clone(), f);
                }
                Item::Func(f) => {
                    fn_ret.insert(f.name.clone(), (f.ret.clone(), f.is_async));
                }
                Item::Struct(s) if !s.generics.is_empty() => {
                    gstructs.insert(s.name.clone(), s);
                }
                Item::Enum(e) if !e.generics.is_empty() => {
                    genums.insert(e.name.clone(), e);
                }
                Item::Interface(i) => {
                    ifaces.insert(i.name.clone());
                }
                _ => {}
            }
        }
        Mono {
            items: &module.items,
            gfuncs,
            gstructs,
            genums,
            ifaces,
            fn_ret,
            requested: HashSet::new(),
            worklist: Vec::new(),
            out: Vec::new(),
            diags: Vec::new(),
            future_table: HashMap::new(),
            mut_tuple_types: muts,
        }
    }

    fn run(&mut self) -> Vec<Item> {
        let items = self.items;
        for item in items {
            self.rewrite_item(item);
        }
        // An async call mints a Future<R> the source never spells, so force one
        // per async function before draining the worklist.
        self.force_future_instances();
        // The drain runs under a hard instantiation budget. A well formed program
        // requests a bounded, small set of instantiations; a runaway generic,
        // where each expansion asks for a strictly larger one, would never drain.
        // The mangled name ceiling in enqueue stops the classic unbounded growth,
        // but a general budget makes non termination impossible for any shape:
        // when it is spent the divergence is reported and the drain stops, so the
        // compiler fails loudly in bounded time instead of hanging.
        let mut spent = 0usize;
        while let Some((name, args, span)) = self.worklist.pop() {
            spent += 1;
            if spent > INSTANTIATION_LIMIT {
                self.diags.push(Diagnostic::new(
                    format!(
                        "monomorphization did not terminate within {INSTANTIATION_LIMIT} instantiations; a generic is expanding without bound, likely a recursive generic instantiated with an ever growing type argument"
                    ),
                    span,
                ));
                break;
            }
            self.expand_instance(&name, &args, span);
        }
        std::mem::take(&mut self.out)
    }

    fn is_generic(&self, name: &str) -> bool {
        self.gstructs.contains_key(name) || self.genums.contains_key(name)
    }


    fn enqueue(&mut self, name: &str, args: &[Type], span: Span) {
        let m = mangle(name, args);
        // Guard against runaway polymorphic recursion, where each instantiation
        // requests a strictly larger one and the worklist never drains. A real
        // program's mangled names stay short; an unbounded one grows without
        // limit, so a length ceiling stops the divergence cheaply.
        if m.len() > 1024 {
            return;
        }
        // Record every future instantiation, its mangled name mapped to the
        // ground element, so the ground type re-check can undo the mangle. The
        // args are already expanded here, so the element is ground. Recorded
        // before the dedup gate is harmless: the same key maps to the same
        // element, so a repeat write is idempotent.
        if name == "Future" && args.len() == 1 {
            self.future_table.insert(m.clone(), args[0].clone());
        }
        if self.requested.insert(m) {
            // Backstop: an interface has no single ground layout, so it cannot
            // stand in for a type parameter a generic is monomorphized over. Sema
            // already rejects this at the source annotation, but mono is also
            // driven straight from codegen with no sema in front, so the rule is
            // enforced here too rather than expanding a shape that has no layout.
            // Nested burials pass through emit_ty, which enqueues each level, so
            // checking this instantiation's own arguments is enough. Reported
            // once per distinct instantiation, inside the dedup gate.
            for a in args {
                if let Type::Named(n, _) = a {
                    if self.ifaces.contains(n) {
                        self.diags.push(Diagnostic::new(
                            "an interface cannot be a generic type argument; generics are monomorphized over concrete types",
                            span,
                        ));
                    }
                }
            }
            self.worklist.push((name.to_string(), args.to_vec(), span));
        }
    }

    /// Requests an instantiation from an expression site and returns its mangled
    /// name. Type arguments are lowered through `emit_ty` first so a nested
    /// generic argument mangles the same way it would from a type annotation,
    /// keeping the construction site and the emitted definition in agreement.
    fn instantiate(&mut self, name: &str, args: &[Type], span: Span) -> String {
        let cargs: Vec<Type> = args.iter().map(|a| self.emit_ty(a, span)).collect();
        self.enqueue(name, &cargs, span);
        mangle(name, &cargs)
    }

    fn expand_instance(&mut self, name: &str, args: &[Type], span: Span) {
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
                    ty: self.emit_field_ty(&fl.ty, &subst, span),
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
                            ty: self.emit_field_ty(&fl.ty, &subst, span),
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

    fn emit_field_ty(&mut self, ty: &Type, subst: &Subst, span: Span) -> Type {
        let applied = subst_apply(ty, subst);
        // A struct or enum field that names a Future with a frame-viewing element
        // is a spell site too; the requesting instantiation's span points the
        // diagnostic at the site that asked for this field rather than the file
        // head.
        self.check_future_spell(&applied, span);
        self.emit_ty(&applied, span)
    }


    /// Mangles ground generic references and requests their instantiation. The
    /// span travels to `enqueue` so a backstop the requested instantiation trips
    /// points at the requesting site.
    fn emit_ty(&mut self, ty: &Type, span: Span) -> Type {
        match ty {
            Type::Named(n, args) if !args.is_empty() => {
                let cargs: Vec<Type> = args.iter().map(|a| self.emit_ty(a, span)).collect();
                if self.is_generic(n) {
                    self.enqueue(n, &cargs, span);
                    Type::Named(mangle(n, &cargs), Vec::new())
                } else {
                    Type::Named(n.clone(), cargs)
                }
            }
            Type::Named(n, _) => Type::Named(n.clone(), Vec::new()),
            Type::Ptr(b) => Type::Ptr(Box::new(self.emit_ty(b, span))),
            Type::RawPtr(b) => Type::RawPtr(Box::new(self.emit_ty(b, span))),
            Type::Slice(b) => Type::Slice(Box::new(self.emit_ty(b, span))),
            Type::Array(b, n) => Type::Array(Box::new(self.emit_ty(b, span)), *n),
            Type::Tuple(xs) => Type::Tuple(xs.iter().map(|x| self.emit_ty(x, span)).collect()),
            Type::Func(ps, r) => Type::Func(
                ps.iter().map(|p| self.emit_ty(p, span)).collect(),
                Box::new(self.emit_ty(r, span)),
            ),
            Type::Unit => Type::Unit,
            // A hole reaching emit means a do-continuation's element type was never
            // pinned by per-site inference. Report it and fall back to a ground
            // type so mangling stays total; analyze fails on the diagnostic first,
            // so codegen never sees the fallback.
            Type::Infer => {
                self.diags.push(Diagnostic::new(
                    "could not infer the type of this do-continuation; annotate the monad element",
                    span,
                ));
                named("int64")
            }
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
                        ty: self.emit_field_ty(&fl.ty, &Subst::new(), Span::new(0, 0)),
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
                                ty: self.emit_field_ty(&fl.ty, &Subst::new(), Span::new(0, 0)),
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
                    span: im.span,
                    methods,
                }));
            }
            // An impl on a generic type would be dropped silently and its method
            // calls miscompiled, so it is a diagnostic until instantiation of
            // impl blocks lands.
            Item::Impl(im) => {
                self.diags.push(Diagnostic::new(
                    format!(
                        "methods on the generic type '{}' are not supported yet; write free functions over it, the way std.vector does",
                        im.ty
                    ),
                    im.span,
                ));
            }
            Item::Interface(i) => self.out.push(Item::Interface(i.clone())),
            // A foreign block has no generics, so it passes through untouched. It
            // must be carried forward, since codegen reads it for the declares.
            Item::Foreign(fb) => self.out.push(Item::Foreign(fb.clone())),
            _ => {}
        }
    }

    /// A channel element crosses a thread boundary by copy through the ring, so
    /// an element type that can view the sending frame, a slice, a closure, or
    /// an interface value, would dangle in the receiver. The instantiation is
    /// rejected here, where the concrete element type is first known, the same
    /// ban spawn captures enforce at the capture site. A future's element
    /// crosses the same way through its completion slot, so the two minting
    /// sites take the same ban; the consuming operations need none, since an
    /// element that cannot be minted can never reach them.
    fn check_chan_element(&mut self, name: &str, targs: &[Type], span: Span) {
        let chan = matches!(
            name,
            "chan_new"
                | "chan_send"
                | "chan_recv"
                | "chan_try_send"
                | "chan_try_recv"
                | "chan_recv_timeout"
                | "chan_close"
                | "chan_free"
        );
        let future = matches!(name, "future_new" | "future_wrap");
        if !chan && !future {
            return;
        }
        let Some(t) = targs.first() else { return };
        if !self.chan_element_ok(t, &HashSet::new()) {
            let msg = if chan {
                "a channel element cannot contain a slice, closure, or interface value; a view of the sending thread's frame would dangle in the receiver; send heap owned data instead"
            } else {
                "a future element cannot contain a slice, closure, or interface value; a view of the completing thread's frame would dangle in the awaiter; send heap owned data instead"
            };
            self.diags.push(Diagnostic::new(msg.to_string(), span));
        }
    }

    /// Whether a channel element may cross a thread boundary by copy: it must not
    /// view the sending frame. The future-carrying flag is off, since a channel
    /// element is not itself a future.
    fn chan_element_ok(&self, t: &Type, path: &HashSet<String>) -> bool {
        self.crossable(t, false, path)
    }

    /// Whether a value of this concrete type may cross a boundary that outlives
    /// the frame it was built in: an async call, an await result, a spawn/submit
    /// capture, or a channel send. A slice, closure, or interface value can view
    /// that frame; with `ban_future` a future is refused too, since a future is
    /// event-loop-thread property. The walk substitutes concrete type arguments
    /// into generic struct and enum fields, so a burial behind a type parameter,
    /// which the checker's erased walk cannot see, is caught here.
    ///
    /// `path` is the set of struct and enum names on the branch from the root to
    /// the current node. A name already on the path is a recursive reference and
    /// is assumed to cross, which terminates even a polymorphic recursion whose
    /// mangled name grows without bound. Each descent gets its own path copy, so a
    /// sibling instantiation of the same generic with a different, non-crossable
    /// argument is judged on its own and never masked by a crossable sibling.
    fn crossable(&self, t: &Type, ban_future: bool, path: &HashSet<String>) -> bool {
        match t {
            Type::Slice(_) | Type::Func(..) => false,
            Type::Named(n, _) if ban_future && n == "Future" => false,
            // A pointer targets the heap, which outlives the frame, so a slice or
            // interface value behind it crosses fine. A future is the exception: it
            // is event-loop-thread property, so a future smuggled behind a pointer,
            // whether bare or buried in a channel handle like `*Channel<Future<T>>`,
            // still faults when a foreign thread awaits it. Under the future ban,
            // hunt the pointee for a buried future and refuse only that; without the
            // ban a pointer crosses freely, as it does today.
            Type::Ptr(b) | Type::RawPtr(b) if ban_future => !self.ptr_reaches_future(b, path),
            Type::Array(b, _) => self.crossable(b, ban_future, path),
            Type::Tuple(ts) => ts.iter().all(|x| self.crossable(x, ban_future, path)),
            Type::Named(n, targs) => {
                if path.contains(n) {
                    return true;
                }
                let mut child = path.clone();
                child.insert(n.clone());
                if !targs.iter().all(|x| self.crossable(x, ban_future, &child)) {
                    return false;
                }
                if let Some(s) = self.gstructs.get(n.as_str()).copied() {
                    let subst = bind(&s.generics, targs);
                    return s
                        .fields
                        .iter()
                        .all(|fl| self.crossable(&subst_apply(&fl.ty, &subst), ban_future, &child));
                }
                if let Some(e) = self.genums.get(n.as_str()).copied() {
                    let subst = bind(&e.generics, targs);
                    return e.variants.iter().all(|v| {
                        v.fields
                            .iter()
                            .all(|fl| self.crossable(&subst_apply(&fl.ty, &subst), ban_future, &child))
                    });
                }
                for item in self.items {
                    match item {
                        Item::Struct(s) if s.name == *n => {
                            return s.fields.iter().all(|fl| self.crossable(&fl.ty, ban_future, &child));
                        }
                        Item::Enum(e) if e.name == *n => {
                            return e.variants.iter().all(|v| {
                                v.fields.iter().all(|fl| self.crossable(&fl.ty, ban_future, &child))
                            });
                        }
                        Item::Interface(i) if i.name == *n => return false,
                        _ => {}
                    }
                }
                true
            }
            // A hole is never a concrete carrier; it is resolved before any
            // crossing check matters, so it is trivially crossable here.
            Type::Infer => true,
            _ => true,
        }
    }

    /// Whether a future is reachable through this type once a pointer has been
    /// crossed. A future belongs to the event loop thread, so burying one behind a
    /// managed or raw pointer and carrying it across an async boundary still faults
    /// when the completer thread awaits it. A slice or interface value behind a
    /// pointer is fine, so only a future counts here, and the walk descends through
    /// nested pointers, arrays, tuples, and the type arguments and fields of a
    /// generic struct or enum. `path` carries the struct and enum names on the
    /// branch, so a recursive type terminates the same way `crossable` does.
    fn ptr_reaches_future(&self, t: &Type, path: &HashSet<String>) -> bool {
        match t {
            Type::Named(n, _) if n == "Future" => true,
            Type::Ptr(b) | Type::RawPtr(b) | Type::Slice(b) | Type::Array(b, _) => {
                self.ptr_reaches_future(b, path)
            }
            Type::Tuple(ts) => ts.iter().any(|x| self.ptr_reaches_future(x, path)),
            Type::Named(n, targs) => {
                if path.contains(n) {
                    return false;
                }
                let mut child = path.clone();
                child.insert(n.clone());
                if targs.iter().any(|x| self.ptr_reaches_future(x, &child)) {
                    return true;
                }
                if let Some(s) = self.gstructs.get(n.as_str()).copied() {
                    let subst = bind(&s.generics, targs);
                    return s
                        .fields
                        .iter()
                        .any(|fl| self.ptr_reaches_future(&subst_apply(&fl.ty, &subst), &child));
                }
                if let Some(e) = self.genums.get(n.as_str()).copied() {
                    let subst = bind(&e.generics, targs);
                    return e.variants.iter().any(|v| {
                        v.fields
                            .iter()
                            .any(|fl| self.ptr_reaches_future(&subst_apply(&fl.ty, &subst), &child))
                    });
                }
                for item in self.items {
                    match item {
                        Item::Struct(s) if s.name == *n => {
                            return s.fields.iter().any(|fl| self.ptr_reaches_future(&fl.ty, &child));
                        }
                        Item::Enum(e) if e.name == *n => {
                            return e.variants.iter().any(|v| {
                                v.fields.iter().any(|fl| self.ptr_reaches_future(&fl.ty, &child))
                            });
                        }
                        _ => {}
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// Reports a future spelled with an element that could view a frame, wherever
    /// its type appears: a binding annotation, a parameter, a field, or a return.
    /// future_new and future_wrap already guard the mint site, but a hand-built or
    /// relabeled `Future{}` struct infers as an unknown element and slips past the
    /// mint guard, so every place a `Future<T>` type is named is checked too.
    fn check_future_spell(&mut self, t: &Type, span: Span) {
        match t {
            Type::Named(n, targs) if n == "Future" && targs.len() == 1 => {
                if !self.crossable(&targs[0], false, &HashSet::new()) {
                    self.diags.push(Diagnostic::new(
                        "a future element cannot contain a slice, closure, or interface value; a view of the completing thread's frame would dangle in the awaiter; send heap owned data instead",
                        span,
                    ));
                }
                self.check_future_spell(&targs[0], span);
            }
            Type::Named(_, targs) => {
                for a in targs {
                    self.check_future_spell(a, span);
                }
            }
            Type::Ptr(b) | Type::RawPtr(b) | Type::Slice(b) | Type::Array(b, _) => {
                self.check_future_spell(b, span)
            }
            Type::Tuple(xs) => {
                for x in xs {
                    self.check_future_spell(x, span);
                }
            }
            Type::Func(ps, r) => {
                for p in ps {
                    self.check_future_spell(p, span);
                }
                self.check_future_spell(r, span);
            }
            Type::Unit => {}
            Type::Infer => {}
        }
    }

    /// Rejects a spawn or submit lambda capture whose type views the spawning
    /// frame, seeing through generic type arguments the checker's erased walk
    /// misses. The checker already rejects every direct capture, so this only
    /// catches a burial behind a type parameter, which reaches here because such
    /// a program is otherwise clean.
    fn check_task_captures(
        &mut self,
        name: &str,
        l: &crate::parser::ast::Lambda,
        env: &Env,
        span: Span,
    ) {
        let mut used = Vec::new();
        let mut bound: HashSet<String> = l.params.iter().map(|p| p.name.clone()).collect();
        crate::parser::ast::collect_block(&l.body, &mut used, &mut bound);
        let mut done = HashSet::new();
        for c in used {
            if bound.contains(&c) || !done.insert(c.clone()) {
                continue;
            }
            if let Some(t) = env.get(&c) {
                if !self.crossable(t, true, &HashSet::new()) {
                    self.diags.push(Diagnostic::new(
                        format!("{name} cannot capture '{c}': it holds a slice, closure, interface value, or future that would view the spawning frame"),
                        span,
                    ));
                }
            }
        }
    }

    /// Reports type parameters an instantiation site could not pin down. The
    /// expansion still defaults them to int64 so codegen can proceed, but the
    /// program is wrong, so `dusk check` surfaces it here.
    fn report_missing(&mut self, missing: &[String], what: &str, span: Span) {
        for g in missing {
            self.diags.push(Diagnostic::new(
                format!(
                    "cannot infer the type parameter '{g}' for '{what}'; add an annotation that pins it"
                ),
                span,
            ));
        }
    }

    fn rw_func(&mut self, f: &Func, subst: &Subst, mangled: Option<String>) -> Func {
        let name = mangled.unwrap_or_else(|| f.name.clone());
        let mut env = Env::new();
        let mut params = Vec::with_capacity(f.params.len());
        for p in &f.params {
            let applied = subst_apply(&p.ty, subst);
            // A Future named in a parameter or return with a frame-viewing
            // element is rejected, so a bad element cannot enter through a
            // signature that names the concrete Future<T>.
            self.check_future_spell(&applied, f.span);
            env.insert(p.name.clone(), applied.clone());
            params.push(Param {
                using: p.using,
                name: p.name.clone(),
                ty: self.emit_ty(&applied, f.span),
            });
        }
        let ret_applied = subst_apply(&f.ret, subst);
        self.check_future_spell(&ret_applied, f.span);
        let ret = self.emit_ty(&ret_applied, f.span);
        let body = self.rw_block(&f.body, subst, &mut env, &ret_applied);
        Func {
            exported: f.exported,
            is_async: f.is_async,
            name,
            span: f.span,
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
                // A Future spelled on a binding annotation with a frame-viewing
                // element is rejected, closing the relabeled-future hole a mint
                // guard cannot see.
                for b in &l.binds {
                    if let Some(t) = &b.ty {
                        let a = subst_apply(t, subst);
                        self.check_future_spell(&a, l.value.span);
                    }
                }
                let value = self.rw_expr(&l.value, subst, env, exp.as_ref());
                let vt = exp
                    .clone()
                    .or_else(|| self.static_ty(&l.value, subst, env));
                // A destructuring bind takes its tuple's element types, one per
                // name; recording the whole tuple against each name would make a
                // later generic call unify against the tuple and instantiate the
                // wrong monomorph.
                let parts: Option<Vec<Type>> = match (&vt, l.binds.len()) {
                    (Some(Type::Tuple(ts)), n) if n > 1 && ts.len() == n => Some(ts.clone()),
                    _ => None,
                };
                // The reconciled storage type for the narrow mutable-tuple class,
                // if the surface pass recorded this binding. Present only for a
                // single unannotated mutable binding whose value is a tuple with an
                // array-literal member, so no ordinary binding is reshaped.
                let table_ty: Option<Type> = if l.mutable && l.binds.len() == 1 && l.binds[0].ty.is_none() {
                    self.mut_tuple_types.get(&l.value.span).cloned()
                } else {
                    None
                };
                let mut binds = Vec::with_capacity(l.binds.len());
                for (i, b) in l.binds.iter().enumerate() {
                    let ty = match &b.ty {
                        Some(t) => {
                            let a = subst_apply(t, subst);
                            Some(self.emit_ty(&a, l.value.span))
                        }
                        None => table_ty.as_ref().map(|t| self.emit_ty(t, l.value.span)),
                    };
                    let bt = b
                        .ty
                        .as_ref()
                        .map(|t| subst_apply(t, subst))
                        .or_else(|| table_ty.clone())
                        .or_else(|| parts.as_ref().map(|p| p[i].clone()))
                        .or_else(|| if l.binds.len() == 1 { vt.clone() } else { None });
                    if let Some(t) = bt {
                        env.insert(b.name.clone(), t);
                    }
                    binds.push(Bind {
                        name: b.name.clone(),
                        ty,
                    });
                }
                Stmt::Let(Let {
                    mutable: l.mutable,
                    is_ref: l.is_ref,
                    infer: l.infer,
                    binds,
                    value,
                })
            }
            Stmt::Assign(lhs, rhs) => Stmt::Assign(
                self.rw_expr(lhs, subst, env, None),
                self.rw_expr(rhs, subst, env, None),
            ),
            Stmt::AssignOp(op, lhs, rhs) => Stmt::AssignOp(
                *op,
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
                self.rw_struct_lit(name, fields, subst, env, expected, e.span)
            }
            ExprKind::Field(base, name) => {
                if let ExprKind::Ident(g) = &base.kind {
                    if self.genums.contains_key(g) && self.enum_has_variant(g, name) {
                        let targs = self.enum_args(g, expected, &[], subst, env, name);
                        let mg = node(ExprKind::Ident(self.instantiate(g, &targs, base.span)), base.span);
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
            ExprKind::Range(a, b, incl) => ExprKind::Range(
                Box::new(self.rw_expr(a, subst, env, None)),
                Box::new(self.rw_expr(b, subst, env, None)),
                *incl,
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
                            ty: self.emit_ty(&subst_apply(&p.ty, subst), e.span),
                        })
                        .collect(),
                    ret: self.emit_ty(&ret, e.span),
                    body,
                })
            }
            ExprKind::Match(m) => {
                let _ = expected;
                ExprKind::Match(Box::new(self.rw_match(m, subst, env, &Type::Unit)))
            }
            ExprKind::Await(op, _) => {
                // Fill the element type the codegen state machine sizes the
                // awaited slot from. It is left None by the parser and typeck; the
                // concrete operand type is first known here.
                let rop = self.rw_expr(op, subst, env, None);
                let el = self.await_element_ty(op, subst, env).map(|t| self.emit_ty(&t, e.span));
                if el.is_none() {
                    self.diags.push(Diagnostic::new(
                        "the element type of this await could not be inferred; bind the future with an annotation first",
                        e.span,
                    ));
                }
                ExprKind::Await(Box::new(rop), el)
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
        // A spawn or submit capture is checked here, where env carries un-mangled
        // types, so a frame-viewing value buried behind a generic type parameter
        // is caught even though the checker's erased walk let it through.
        if let ExprKind::Ident(name) = &callee.kind {
            if (name == "spawn" || name == "submit") && args.len() == 1 {
                if let ExprKind::Lambda(l) = &args[0].kind {
                    self.check_task_captures(name, l, env, callee.span);
                }
            }
        }
        if let ExprKind::Field(base, v) = &callee.kind {
            if let ExprKind::Ident(g) = &base.kind {
                if self.genums.contains_key(g) && self.enum_has_variant(g, v) {
                    let cargs: Vec<Expr> =
                        args.iter().map(|a| self.rw_expr(a, subst, env, None)).collect();
                    let targs = self.enum_args(g, expected, args, subst, env, v);
                    let mg = node(ExprKind::Ident(self.instantiate(g, &targs, base.span)), base.span);
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
                        return ExprKind::SizeofType(self.emit_ty(&ty, args[0].span));
                    }
                }
            }
            if let Some(gf) = self.gfuncs.get(f).copied() {
                // Infer the type arguments before rewriting the value arguments, so
                // a continuation lambda's open (`Infer`) parameter and return holes
                // can be filled with the now-concrete types the declared signature
                // pins. This is what lets a `do` over a generic monad instantiate a
                // fresh bind/unit pair per site.
                let (targs, missing) = self.solve_call(gf, args, subst, env, expected);
                self.report_missing(&missing, f, callee.span);
                self.check_chan_element(f, &targs, callee.span);
                let inf_full = bind(&gf.generics, &targs);
                let cargs: Vec<Expr> = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| self.rw_arg(a, gf.params.get(i).map(|p| &p.ty), &inf_full, subst, env))
                    .collect();
                let mg = node(ExprKind::Ident(self.instantiate(f, &targs, callee.span)), callee.span);
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
        span: Span,
    ) -> ExprKind {
        let new_fields: Vec<(String, Expr)> = fields
            .iter()
            .map(|(n, v)| (n.clone(), self.rw_expr(v, subst, env, None)))
            .collect();
        if let Some(gs) = self.gstructs.get(name).copied() {
            let (targs, missing) = self.infer_struct_args(gs, fields, expected, subst, env);
            self.report_missing(&missing, name, span);
            return ExprKind::StructLit(self.instantiate(name, &targs, span), new_fields);
        }
        ExprKind::StructLit(name.to_string(), new_fields)
    }


    /// Infers a generic function's type arguments at one call site, seeing
    /// through continuation lambdas. Non-lambda arguments and the expected type
    /// bind the parameters they mention first, so those bindings are authoritative
    /// (`unify` keeps the first binding for each name). Then each lambda argument
    /// is typed with its own parameters pinned to the bindings so far, and its
    /// returned expression's type pins any parameter that lives only in the
    /// lambda's result. This resolves the open bind/unit pair a `do` over a
    /// generic monad desugars to, one instantiation per site.
    fn solve_call(
        &self,
        gf: &Func,
        args: &[Expr],
        subst: &Subst,
        env: &Env,
        expected: Option<&Type>,
    ) -> (Vec<Type>, Vec<String>) {
        let params: HashSet<String> = gf.generics.iter().cloned().collect();
        let mut inf = Subst::new();
        // Non-lambda arguments first. Their types are authoritative, so a later
        // lambda pass can never override a binding an argument already made.
        for (i, decl) in gf.params.iter().enumerate() {
            let Some(a) = args.get(i) else { continue };
            if matches!(a.kind, ExprKind::Lambda(_)) {
                continue;
            }
            if let Some(at) = self.static_ty(a, subst, env) {
                unify(&decl.ty, &at, &params, &mut inf);
            }
        }
        // Push down the expected (annotation) type so a parameter appearing only
        // in the return position is inferred instead of silently defaulting.
        if let Some(et) = expected {
            unify(&gf.ret, et, &params, &mut inf);
        }
        // Lambda arguments last. Each is typed with its parameters bound to the
        // inference so far; its body's returned expression then pins any parameter
        // that lives only in the lambda's result type.
        for (i, decl) in gf.params.iter().enumerate() {
            let Some(a) = args.get(i) else { continue };
            let ExprKind::Lambda(l) = &a.kind else { continue };
            let Type::Func(pdecl, rdecl) = &decl.ty else { continue };
            let merged = union(subst, &inf);
            let mut env2 = env.clone();
            for (j, lp) in l.params.iter().enumerate() {
                if let Some(pt) = pdecl.get(j) {
                    env2.insert(lp.name.clone(), subst_apply(pt, &merged));
                }
            }
            // Desugar-emitted continuations are exactly `{ return E }`; only that
            // shape lets the body pin a return-only parameter, so anything else is
            // left to the argument and expected passes.
            if let [Stmt::Return(Some(e))] = l.body.stmts.as_slice() {
                if let Some(rt) = self.static_ty(e, subst, &env2) {
                    unify(rdecl, &rt, &params, &mut inf);
                }
            }
        }
        solve(&gf.generics, &inf)
    }

    /// Rewrites one call argument, filling a continuation lambda's open holes with
    /// the concrete types the callee's declared parameter pins. A non-lambda
    /// argument, or a lambda in a call whose matching parameter is not a function
    /// type, is rewritten unchanged.
    fn rw_arg(
        &mut self,
        arg: &Expr,
        decl: Option<&Type>,
        inf_full: &Subst,
        subst: &Subst,
        env: &Env,
    ) -> Expr {
        if let ExprKind::Lambda(l) = &arg.kind {
            if let Some(Type::Func(pdecl, rdecl)) = decl {
                let params = l
                    .params
                    .iter()
                    .enumerate()
                    .map(|(j, lp)| Param {
                        using: lp.using,
                        name: lp.name.clone(),
                        ty: pdecl
                            .get(j)
                            .map(|pt| subst_apply(pt, inf_full))
                            .unwrap_or_else(|| lp.ty.clone()),
                    })
                    .collect();
                let patched = crate::parser::ast::Lambda {
                    params,
                    ret: subst_apply(rdecl, inf_full),
                    body: l.body.clone(),
                };
                let e = node(ExprKind::Lambda(patched), arg.span);
                return self.rw_expr(&e, subst, env, None);
            }
        }
        self.rw_expr(arg, subst, env, None)
    }

    fn infer_struct_args(
        &self,
        gs: &Struct,
        fields: &[(String, Expr)],
        expected: Option<&Type>,
        subst: &Subst,
        env: &Env,
    ) -> (Vec<Type>, Vec<String>) {
        if let Some(Type::Named(en, eargs)) = expected {
            if en == &gs.name && !eargs.is_empty() {
                return (eargs.iter().map(|t| subst_apply(t, subst)).collect(), Vec::new());
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
        solve(&ge.generics, &inf).0
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
                crate::parser::ast::UnOp::Neg | crate::parser::ast::UnOp::BitNot => {
                    self.static_ty(x, subst, env)
                }
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
                    if let Some(gf) = self.gfuncs.get(f).copied() {
                        // Full call-site inference, including the lambda pass, so a
                        // nested `bind(...)` in a continuation body reports its real
                        // element type instead of falling to None. A parameter the
                        // site could not pin is left unbound; if the return type
                        // still mentions it the result is unknown, matching the old
                        // behavior, otherwise the ground return type is returned.
                        let (targs, missing) = self.solve_call(gf, args, subst, env, None);
                        let miss: HashSet<String> = missing.into_iter().collect();
                        if mentions(&gf.ret, &miss) {
                            None
                        } else {
                            let inf = bind(&gf.generics, &targs);
                            Some(subst_apply(&gf.ret, &inf))
                        }
                    } else if let Some((ret, is_async)) = self.fn_ret.get(f) {
                        // An async call's runtime value is the Future the task
                        // mints, not the declared return the source spells. Typing
                        // it as Future<ret> keeps a bare-future binding agreeing
                        // with its ground layout, so a later pass of it to a
                        // generic instantiates over Future<ret> and the ground
                        // re-check finds matching types instead of a spurious
                        // mismatch. A direct await unwraps the Future arm again.
                        if *is_async {
                            Some(Type::Named("Future".to_string(), vec![ret.clone()]))
                        } else {
                            Some(ret.clone())
                        }
                    } else {
                        builtin_ret(f)
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
                    let (targs, _) = self.infer_struct_args(gs, fields, None, subst, env);
                    Some(Type::Named(name.clone(), targs))
                } else {
                    Some(named(name))
                }
            }
            _ => None,
        }
    }

    /// The concrete element type of an awaited operand. A direct async call
    /// reports its declared return, which is exactly the element the task future
    /// carries; a value of type `Future<t>`, from a leaf future binding or a
    /// future-returning call, yields `t`. When the operand's type cannot be
    /// resolved statically the caller reports it and asks for an annotation.
    fn await_element_ty(&self, op: &Expr, subst: &Subst, env: &Env) -> Option<Type> {
        let t = self.static_ty(op, subst, env)?;
        match t {
            Type::Named(n, targs) if n == "Future" && targs.len() == 1 => {
                Some(subst_apply(&targs[0], subst))
            }
            other => Some(other),
        }
    }

    /// Forces one Future instantiation per async function, so the struct codegen
    /// packs an async call's result into exists even when no source line names
    /// `Future<R>`. Reuses the future element ban as a backstop, and requires the
    /// Future struct to be in scope, since an async call cannot lower without it.
    fn force_future_instances(&mut self) {
        // Async funcs are non-generic, so their param and return types are already
        // un-mangled with type arguments intact, exactly what the frame-view walk
        // needs to see through a generic burial.
        let asyncs: Vec<AsyncSig> = self
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Func(f) if f.is_async => Some((
                    f.params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect(),
                    f.ret.clone(),
                    f.span,
                )),
                _ => None,
            })
            .collect();
        if asyncs.is_empty() {
            return;
        }
        if !self.gstructs.contains_key("Future") {
            self.diags.push(Diagnostic::new(
                "calling an async func needs Future from std.async.future; add @import std.async.future",
                asyncs[0].2,
            ));
            return;
        }
        for (params, ret, span) in asyncs {
            // No parameter may view the caller's frame or carry a future, seen
            // through generic type arguments the erased checker walk misses. The
            // checker already rejects the direct cases, so this only fires on a
            // burial in an otherwise clean program.
            for (pname, pty) in &params {
                if !self.crossable(pty, true, &HashSet::new()) {
                    self.diags.push(Diagnostic::new(
                        format!("an async func cannot take '{pname}': it holds a slice, closure, interface value, or future that would view the caller's frame, which the task outlives"),
                        span,
                    ));
                }
            }
            if !self.crossable(&ret, true, &HashSet::new()) {
                self.diags.push(Diagnostic::new(
                    "an async func cannot return a slice, closure, interface value, or future that would outlive the task frame it views",
                    span,
                ));
            }
            let r = self.emit_ty(&ret, span);
            self.enqueue("Future", &[r], span);
        }
    }
}

/// Return types of the builtins that carry one, so a binding of a builtin result
/// types its names for later generic inference instead of falling to unknown.
fn builtin_ret(name: &str) -> Option<Type> {
    let pair = |t: Type| Type::Tuple(vec![t, named("error")]);
    match name {
        "read_file" | "read_line" | "read_all" => Some(pair(named("string"))),
        "parse_float" => Some(pair(named("float64"))),
        "write_file" => Some(named("error")),
        "cstr" => Some(named("string")),
        "sizeof" => Some(named("int64")),
        "spawn" => Some(pair(named("thread"))),
        "join" => Some(named("error")),
        "submit" => Some(named("error")),
        _ => None,
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

/// Merges two substitutions, the second overriding the first on a shared name.
/// Used to apply a callee's declared lambda parameter type, which names the
/// callee's own type parameters, under both the outer monomorphization
/// substitution and the freshly inferred bindings.
fn union(base: &Subst, over: &Subst) -> Subst {
    let mut out = base.clone();
    for (k, v) in over {
        out.insert(k.clone(), v.clone());
    }
    out
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
        Type::Infer => false,
    }
}

/// Resolves each type parameter from the inferred substitution. A parameter no
/// site pinned defaults to int64 so expansion can proceed, and its name is
/// returned so the caller can report it; a silent default converts an inference
/// gap into a wrong program.
fn solve(generics: &[String], inf: &Subst) -> (Vec<Type>, Vec<String>) {
    let mut missing = Vec::new();
    let out = generics
        .iter()
        .map(|g| {
            inf.get(g).cloned().unwrap_or_else(|| {
                missing.push(g.clone());
                named("int64")
            })
        })
        .collect();
    (out, missing)
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
        Type::Infer => Type::Infer,
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

/// The mangled name of an instantiation, `name$arg$arg`. Exposed so codegen can
/// compute the identical `Future$...` name an async call's result packs into.
pub(crate) fn mangle(name: &str, args: &[Type]) -> String {
    if args.is_empty() {
        return name.to_string();
    }
    let parts: Vec<String> = args.iter().map(flat).collect();
    format!("{name}${}", parts.join("$"))
}

/// Flattens a type to an injective token-safe string. Nested generic
/// references carry an arity prefix so siblings and nesting never alias
/// (`A$B$1$C$D` and `A$B$C$1$D` stay distinct), and non nominal constructors
/// use a leading `$` marker that no source identifier can begin with. Exposed
/// alongside `mangle` so codegen mangles a future element identically.
pub(crate) fn flat(ty: &Type) -> String {
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
        // Poison: a hole should never reach mangling, since emit_ty reports it and
        // analyze fails first. The token keeps flat total if it ever slips through.
        Type::Infer => "$infer".to_string(),
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
        let out = expand(&m, &MutTupleTypes::new());
        out.items
            .iter()
            .map(|i| match i {
                Item::Func(f) => f.name.clone(),
                Item::Struct(s) => s.name.clone(),
                Item::Enum(en) => en.name.clone(),
                Item::Impl(im) => format!("impl {}", im.ty),
                Item::Interface(it) => it.name.clone(),
                Item::Foreign(_) => "foreign".to_string(),
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
        let out = expand(&m, &MutTupleTypes::new());
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
                            !a.is_empty() || n != "T" && n != "A",
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
    fn destructured_bindings_take_element_types() {
        // `a, b := pair()` must record each name with its tuple element type, so
        // a later generic call instantiates id$float64, not a tuple monomorph.
        let n = names(
            "func id<T>(x: T) -> T { return x }\n\
             func pair() -> (float64, float64) { return (1.5, 2.5) }\n\
             func main() -> int32 {\n  a, b := pair()\n  println(id(a))\n  println(b)\n  return 0\n}",
        );
        assert!(n.contains(&"id$float64".to_string()), "{n:?}");
        assert!(
            !n.iter().any(|x| x.starts_with("id$$")),
            "no tuple typed monomorph may appear: {n:?}"
        );
    }

    #[test]
    fn builtin_results_type_their_bindings() {
        // read_file returns (string, error); the destructured data must drive
        // generic inference as a string, not fall to the int64 default.
        let n = names(
            "func id<T>(x: T) -> T { return x }\n\
             func main() -> int32 {\n  data, e := read_file(\"x\")\n  e.ignore()\n  println(id(data))\n  return 0\n}",
        );
        assert!(n.contains(&"id$string".to_string()), "{n:?}");
    }

    fn diags(src: &str) -> Vec<String> {
        let (t, _) = lex(src);
        let (m, e) = parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        expand_with_diags(&m, &MutTupleTypes::new()).1.into_iter().map(|d| d.msg).collect()
    }

    #[test]
    fn uninferred_type_parameter_is_diagnosed() {
        let d = diags(
            "func pick<T>() -> T { return pick() }\n\
             func main() -> int32 {\n  x := pick()\n  println(x)\n  return 0\n}",
        );
        assert!(
            d.iter().any(|m| m.contains("cannot infer the type parameter 'T' for 'pick'")),
            "{d:?}"
        );
    }

    #[test]
    fn annotated_call_pins_the_parameter_and_is_clean() {
        let d = diags(
            "func pick<T>() -> T { return pick() }\n\
             func main() -> int32 {\n  x: int64 = pick()\n  println(x)\n  return 0\n}",
        );
        assert!(!d.iter().any(|m| m.contains("cannot infer")), "{d:?}");
    }

    #[test]
    fn impl_on_generic_type_is_diagnosed() {
        let d = diags(
            "struct Box<T> { v: T }\n\
             impl Box { func get() -> int64 { return 0 } }\n\
             func main() -> int32 { return 0 }",
        );
        assert!(
            d.iter().any(|m| m.contains("methods on the generic type 'Box'")),
            "{d:?}"
        );
    }

    #[test]
    fn channel_element_with_a_frame_view_is_diagnosed() {
        let d = diags(
            "func chan_send<T>(c: int64, x: T) -> int64 { return c }\n\
             func main() -> int32 {\n  xs: int64[3] = [1, 2, 3]\n  s: int64[] = xs[0..2]\n  r := chan_send(1, s)\n  println(r)\n  return 0\n}",
        );
        assert!(d.iter().any(|m| m.contains("channel element")), "{d:?}");
    }

    #[test]
    fn channel_element_with_a_buried_slice_is_diagnosed() {
        let d = diags(
            "struct Wrap { s: int64[] }\n\
             func chan_new<T>(cap: int64) -> T { return chan_new(cap) }\n\
             func main() -> int32 {\n  w: Wrap = chan_new(1)\n  println(w.s[0])\n  return 0\n}",
        );
        assert!(d.iter().any(|m| m.contains("channel element")), "{d:?}");
    }

    #[test]
    fn channel_element_of_plain_data_is_clean() {
        let d = diags(
            "func chan_send<T>(c: int64, x: T) -> int64 { return c }\n\
             func main() -> int32 {\n  r := chan_send(1, 42)\n  println(r)\n  return 0\n}",
        );
        assert!(!d.iter().any(|m| m.contains("channel element")), "{d:?}");
    }

    #[test]
    fn future_element_with_a_frame_view_is_diagnosed() {
        let d = diags(
            "func future_new<T>() -> T { return future_new() }\n\
             func main() -> int32 {\n  s: int64[] = future_new()\n  println(s[0])\n  return 0\n}",
        );
        assert!(d.iter().any(|m| m.contains("future element")), "{d:?}");
    }

    #[test]
    fn future_element_of_plain_data_is_clean() {
        let d = diags(
            "func future_new<T>() -> T { return future_new() }\n\
             func main() -> int32 {\n  n: int64 = future_new()\n  println(n)\n  return 0\n}",
        );
        assert!(!d.iter().any(|m| m.contains("future element")), "{d:?}");
    }

    // A minimal Future struct so the forced instantiation finds its definition,
    // standing in for the stdlib import in these unit tests.
    const FUTURE_DECL: &str = "struct Future<T> { h: *void, gen: int64 }\n";

    /// The element-type annotation the monomorphizer filled on the first await
    /// found while walking the module, and whether any await was found at all.
    fn first_await_ty(src: &str) -> (bool, Option<Type>) {
        let (t, _) = lex(src);
        let (m, e) = parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        let out = expand(&m, &MutTupleTypes::new());
        fn walk_block(b: &Block) -> Option<(bool, Option<Type>)> {
            b.stmts.iter().find_map(walk_stmt)
        }
        fn walk_stmt(s: &Stmt) -> Option<(bool, Option<Type>)> {
            match s {
                Stmt::Let(l) => walk_expr(&l.value),
                Stmt::Return(Some(e)) | Stmt::Expr(e) | Stmt::Defer(e) => walk_expr(e),
                Stmt::If(i) => walk_block(&i.then).or_else(|| i.els.as_ref().and_then(walk_block)),
                Stmt::While(w) => walk_block(&w.body),
                Stmt::For(f) => walk_block(&f.body),
                Stmt::Match(m) => m.arms.iter().find_map(|a| walk_block(&a.body)),
                _ => None,
            }
        }
        fn walk_expr(e: &Expr) -> Option<(bool, Option<Type>)> {
            if let ExprKind::Await(_, ty) = &e.kind {
                return Some((true, ty.clone()));
            }
            None
        }
        for it in &out.items {
            if let Item::Func(f) = it {
                if let Some(r) = walk_block(&f.body) {
                    return r;
                }
            }
        }
        (false, None)
    }

    #[test]
    fn await_of_an_async_call_fills_the_declared_return() {
        let (found, ty) = first_await_ty(&format!(
            "{FUTURE_DECL}async func val(x: int64) -> int64 {{ return x }}\n\
             async func amain() -> int64 {{\n  a := await val(3)\n  return a\n}}"
        ));
        assert!(found, "an await must be present");
        assert_eq!(ty, Some(named("int64")), "element is the async func's declared return");
    }

    #[test]
    fn await_of_an_annotated_future_binding_fills_the_element() {
        let (found, ty) = first_await_ty(&format!(
            "{FUTURE_DECL}func fnew() -> Future<int64> {{ return Future {{ h: fnew_h(), gen: 0 }} }}\n\
             async func amain() -> int64 {{\n  fa: Future<int64> = fnew()\n  a := await fa\n  return a\n}}"
        ));
        assert!(found);
        assert_eq!(ty, Some(named("int64")), "element unwraps the annotated Future<int64>");
    }

    #[test]
    fn await_of_an_async_tuple_call_fills_the_tuple_element() {
        let (found, ty) = first_await_ty(&format!(
            "{FUTURE_DECL}async func leaf() -> (int64, error) {{ return (1, error {{}}) }}\n\
             async func amain() -> int64 {{\n  v, e := await leaf()\n  e.ignore()\n  return v\n}}"
        ));
        assert!(found);
        assert_eq!(
            ty,
            Some(Type::Tuple(vec![named("int64"), named("error")])),
            "element is the whole declared tuple return"
        );
    }

    #[test]
    fn await_with_an_unresolvable_operand_is_diagnosed() {
        let d = diags(&format!(
            "{FUTURE_DECL}func fwrap<T>(h: *void) -> Future<T> {{ return Future {{ h: h, gen: 0 }} }}\n\
             async func amain() -> int64 {{\n  a := await fwrap(nullh())\n  return a\n}}"
        ));
        assert!(
            d.iter().any(|m| m.contains("the element type of this await could not be inferred")),
            "{d:?}"
        );
    }

    #[test]
    fn async_without_a_future_struct_is_diagnosed() {
        let d = diags(
            "async func amain() -> int64 {\n  return 1\n}\n\
             func main() -> int32 {\n  r := async_run(amain())\n  println(r)\n  return 0\n}",
        );
        assert!(
            d.iter().any(|m| m.contains("calling an async func needs Future from std.async.future")),
            "{d:?}"
        );
    }

    #[test]
    fn async_param_generic_burial_rejects() {
        // A slice buried behind a generic type parameter must reject, though the
        // checker's erased walk cannot see it.
        let d = diags(&format!(
            "{FUTURE_DECL}struct Box<T> {{ x: T }}\n\
             async func g(b: Box<int64[]>) -> int64 {{ return 1 }}\n\
             async func amain() -> int64 {{ return 1 }}\n\
             func main() -> int32 {{ r := async_run(amain())\n  println(r)\n  return 0 }}"
        ));
        assert!(
            d.iter().any(|m| m.contains("an async func cannot take 'b'") && m.contains("view the caller's frame")),
            "{d:?}"
        );
    }

    #[test]
    fn async_param_generic_future_burial_rejects() {
        let d = diags(&format!(
            "{FUTURE_DECL}struct Box<T> {{ x: T }}\n\
             async func g(b: Box<Future<int64>>) -> int64 {{ return 1 }}\n\
             async func amain() -> int64 {{ return 1 }}\n\
             func main() -> int32 {{ r := async_run(amain())\n  println(r)\n  return 0 }}"
        ));
        assert!(d.iter().any(|m| m.contains("an async func cannot take 'b'")), "{d:?}");
    }

    #[test]
    fn async_return_generic_burial_rejects() {
        let d = diags(&format!(
            "{FUTURE_DECL}struct Box<T> {{ x: T }}\n\
             async func g() -> Box<int64[]> {{ return Box {{ x: [] }} }}\n\
             func main() -> int32 {{ r := async_run(g())\n  println(1)\n  return 0 }}"
        ));
        assert!(d.iter().any(|m| m.contains("an async func cannot return")), "{d:?}");
    }

    #[test]
    fn spawn_capture_generic_burial_rejects() {
        let d = diags(
            "struct Box<T> { x: T }\n\
             func main() -> int32 {\n  b: Box<int64[]> = Box { x: [1, 2] }\n  t, e := spawn(lambda () -> void {\n    println(b.x.len)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n  return 0\n}",
        );
        assert!(d.iter().any(|m| m.contains("spawn cannot capture 'b'")), "{d:?}");
    }

    #[test]
    fn submit_capture_generic_burial_rejects() {
        let d = diags(
            "struct Box<T> { x: T }\n\
             func main() -> int32 {\n  b: Box<int64[]> = Box { x: [1, 2] }\n  s := submit(lambda () -> void {\n    println(b.x.len)\n  })\n  s.ignore()\n  return 0\n}",
        );
        assert!(d.iter().any(|m| m.contains("submit cannot capture 'b'")), "{d:?}");
    }

    #[test]
    fn relabeled_future_slice_annotation_rejects() {
        // A hand-built Future struct relabeled to a slice element must reject at
        // the annotation, since it infers as an unknown element past the mint guard.
        let d = diags(&format!(
            "{FUTURE_DECL}func main() -> int32 {{\n  fs: Future<int64[]> = Future {{ h: hh(), gen: 0 }}\n  return 0\n}}"
        ));
        assert!(d.iter().any(|m| m.contains("a future element cannot contain a slice")), "{d:?}");
    }

    #[test]
    fn scalar_generic_element_async_param_accepts() {
        // The false-positive guard: a scalar element behind a generic must pass.
        let d = diags(&format!(
            "{FUTURE_DECL}struct Box<T> {{ x: T }}\n\
             async func g(b: Box<int64>) -> int64 {{ return b.x }}\n\
             async func amain() -> int64 {{ return 1 }}\n\
             func main() -> int32 {{ r := async_run(amain())\n  println(r)\n  return 0 }}"
        ));
        assert!(
            !d.iter().any(|m| m.contains("an async func cannot take")),
            "a scalar generic element must be accepted: {d:?}"
        );
    }

    #[test]
    fn polymorphic_recursive_async_param_terminates() {
        // A non-regular recursive type grows its mangled name at every level, so a
        // mangle-keyed cycle guard never repeats and loops forever. The path-keyed
        // guard terminates; the test completing at all proves it. L<int64> holds
        // only nested L's and an int64, no frame view, so the param is accepted.
        let d = diags(&format!(
            "{FUTURE_DECL}struct L<T> {{ next: L<L<T>>, val: T }}\n\
             async func g(x: L<int64>) -> void {{ return }}\n\
             async func amain() -> int64 {{ return 1 }}\n\
             func main() -> int32 {{ r := async_run(amain())\n  println(r)\n  return 0 }}"
        ));
        assert!(!d.iter().any(|m| m.contains("an async func cannot take 'x'")), "{d:?}");
    }

    #[test]
    fn polymorphic_recursive_channel_element_terminates() {
        // The channel side of crossable takes the same non-regular recursion; the
        // test completing proves chan_element_ok terminates too. L<int64> is
        // crossable, so no future-element error is expected.
        let d = diags(
            "struct L<T> { next: L<L<T>>, val: T }\n\
             func future_new<T>() -> T { return future_new() }\n\
             func main() -> int32 {\n  n: L<int64> = future_new()\n  println(n.val)\n  return 0\n}",
        );
        assert!(!d.iter().any(|m| m.contains("future element")), "{d:?}");
    }

    #[test]
    fn sibling_generic_with_a_bad_arg_is_not_masked() {
        // Path-keyed descent judges each sibling instantiation of a generic on its
        // own, so a crossable Box<int64> does not mask a non-crossable Box<int64[]>.
        let d = diags(&format!(
            "{FUTURE_DECL}struct Box<T> {{ x: T }}\n\
             struct Pair<A, B> {{ a: A, b: B }}\n\
             async func g(p: Pair<Box<int64>, Box<int64[]>>) -> void {{ return }}\n\
             async func amain() -> int64 {{ return 1 }}\n\
             func main() -> int32 {{ r := async_run(amain())\n  println(r)\n  return 0 }}"
        ));
        assert!(d.iter().any(|m| m.contains("an async func cannot take 'p'")), "{d:?}");
    }

    #[test]
    fn async_func_forces_its_future_instance() {
        let n = names(&format!(
            "{FUTURE_DECL}async func amain() -> int64 {{\n  return 1\n}}\n\
             func main() -> int32 {{\n  r := async_run(amain())\n  println(r)\n  return 0\n}}"
        ));
        assert!(n.contains(&"Future$int64".to_string()), "the Future<int64> instance must be forced: {n:?}");
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

    fn diag_msgs(src: &str) -> Vec<String> {
        let (t, _) = lex(src);
        let (m, e) = parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        let (_, diags, _) = expand_with_diags(&m, &MutTupleTypes::new());
        diags.into_iter().map(|d| d.msg).collect()
    }

    fn raw_diags(src: &str) -> Vec<Diagnostic> {
        let (t, _) = lex(src);
        let (m, e) = parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        expand_with_diags(&m, &MutTupleTypes::new()).1
    }

    #[test]
    fn interface_generic_arg_backstop() {
        // Sema rejects an interface type argument at the annotation, but mono runs
        // straight from codegen too, so its own backstop must catch the request.
        // Here the interface argument is inferred from the field value, a shape the
        // erased checker walk cannot see, so only the mono backstop fires.
        let msgs = diag_msgs(
            "interface Speaker { speak() -> int64 }\n\
             struct Dog { name: int64 }\n\
             impl Speaker for Dog { func speak() -> int64 { return self.name } }\n\
             struct Box<T> { v: T }\n\
             func main() -> int32 {\n\
               s: Speaker = Dog { name: 3 }\n\
               b := Box { v: s }\n\
               println(b.v.speak())\n\
               return 0\n\
             }",
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("an interface cannot be a generic type argument")),
            "mono must backstop an interface generic argument: {msgs:?}"
        );
    }

    #[test]
    fn interface_backstop_points_at_the_requesting_site() {
        // The backstop diagnostic carries the requesting site's span, not the file
        // head, so two distinct violations keep distinct spans and the sema dedup
        // reports both instead of collapsing them.
        let d = raw_diags(
            "interface Speaker { speak() -> int64 }\n\
             struct Dog { name: int64 }\n\
             impl Speaker for Dog { func speak() -> int64 { return self.name } }\n\
             struct Box<T> { v: T }\n\
             func main() -> int32 {\n\
               s: Speaker = Dog { name: 3 }\n\
               b := Box { v: s }\n\
               println(b.v.speak())\n\
               return 0\n\
             }",
        );
        let backstop = d
            .iter()
            .find(|x| x.msg.contains("an interface cannot be a generic type argument"));
        assert!(backstop.is_some(), "backstop must fire: {d:?}");
        let s = backstop.unwrap().span;
        assert_ne!((s.lo, s.hi), (0, 0), "the backstop must point at a real site: {s:?}");
    }

    #[test]
    fn async_param_pointer_to_future_rejects() {
        // A future behind a managed pointer still reaches a foreign thread that
        // would await it off the loop thread, so the future ban must see through
        // the pointer at an async boundary.
        let d = diags(&format!(
            "{FUTURE_DECL}async func g(p: *Future<int64>) -> int64 {{ return 1 }}\n\
             async func amain() -> int64 {{ return 1 }}\n\
             func main() -> int32 {{ r := async_run(amain())\n  println(r)\n  return 0 }}"
        ));
        assert!(d.iter().any(|m| m.contains("an async func cannot take 'p'")), "{d:?}");
    }

    #[test]
    fn async_param_pointer_to_channel_of_futures_rejects() {
        // The future ban reaches through a pointer into a generic handle's type
        // argument, closing `*Channel<Future<T>>` the same way the bare pointer is
        // closed.
        let d = diags(&format!(
            "{FUTURE_DECL}struct Chan<T> {{ h: int64 }}\n\
             async func g(p: *Chan<Future<int64>>) -> int64 {{ return 1 }}\n\
             async func amain() -> int64 {{ return 1 }}\n\
             func main() -> int32 {{ r := async_run(amain())\n  println(r)\n  return 0 }}"
        ));
        assert!(d.iter().any(|m| m.contains("an async func cannot take 'p'")), "{d:?}");
    }

    #[test]
    fn async_param_pointer_to_a_slice_bearing_struct_accepts() {
        // The pointer future ban is future-only: a pointer to heap data that holds a
        // slice must still cross, since the pointer targets the heap the generation
        // check covers.
        let d = diags(&format!(
            "{FUTURE_DECL}struct Holder {{ s: int64[] }}\n\
             async func g(p: *Holder) -> int64 {{ return 1 }}\n\
             async func amain() -> int64 {{ return 1 }}\n\
             func main() -> int32 {{ r := async_run(amain())\n  println(r)\n  return 0 }}"
        ));
        assert!(!d.iter().any(|m| m.contains("an async func cannot take 'p'")), "{d:?}");
    }

    #[test]
    fn instantiation_budget_stops_runaway() {
        // The drain runs under a hard instantiation budget so no shape can hang the
        // compiler. Seeding the worklist past the ceiling with entries that expand
        // to nothing exercises the guard directly: the drain must stop and report
        // instead of spinning.
        let (t, _) = lex("func main() -> int32 { return 0 }");
        let (m, e) = parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        let muts = MutTupleTypes::new();
        let mut mono = Mono::new(&m, &muts);
        for i in 0..(INSTANTIATION_LIMIT + 5) {
            mono.worklist
                .push((format!("Runaway{i}"), Vec::new(), Span::new(i as u32 + 1, i as u32 + 2)));
        }
        let _ = mono.run();
        let limit = mono
            .diags
            .iter()
            .find(|d| d.msg.contains("did not terminate"));
        assert!(
            limit.is_some(),
            "the instantiation budget must stop a runaway drain: {:?}",
            mono.diags.iter().map(|d| &d.msg).collect::<Vec<_>>()
        );
        // The ceiling diagnostic carries the tripping instantiation's span, not the
        // file head, so distinct runaway sites stay distinct through the sema dedup.
        assert_ne!(
            (limit.unwrap().span.lo, limit.unwrap().span.hi),
            (0, 0),
            "the non-termination diagnostic must point at a real requesting site"
        );
    }
}
