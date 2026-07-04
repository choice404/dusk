//! Type checking. M4.
//!
//! Integers and floats carry their bit width, with width 0 standing for a bare
//! literal that adapts to the width around it, so `int32 + int64` is rejected
//! while `x + 1` still works at any width. Generics, methods, and imported types
//! become `Unknown`, which is compatible with everything, so advanced code is
//! accepted without false errors while real core type errors are still caught.
//! The unused variable rule and the bare identifier immutability rule live in the
//! resolver pass (M3); the projection form of immutability, the must handle rule,
//! and the ownership rules live here, where types are known.

use std::collections::{HashMap, HashSet};

use crate::diag::{Diagnostic, Span};
use crate::parser::ast::*;

/// Type checks a module, returning diagnostics.
pub fn check(module: &Module) -> Vec<Diagnostic> {
    let mut tc = TypeChecker::new();
    tc.collect_sigs(module);
    tc.run(module);
    tc.errors
}

/// A checked type. `Int(w)` and `Float(w)` carry the bit width; width 0 is a
/// bare numeric literal, compatible with any width of its kind, so a literal
/// adapts to its context and only two differently sized variables clash.
#[derive(Clone, Debug, PartialEq)]
enum Ty {
    Int(u32),
    Float(u32),
    Bool,
    Char,
    Str,
    Unit,
    Error,
    Thread,
    Ptr(Box<Ty>),
    RawPtr(Box<Ty>),
    Slice(Box<Ty>),
    Array(Box<Ty>, u64),
    Tuple(Vec<Ty>),
    Named(String),
    Func(Vec<Ty>, Box<Ty>),
    /// A one-shot completion future carrying an element of the boxed type. An
    /// async call mints one; awaiting it yields the element. Its element type is
    /// tracked here even though the `Future<T>` struct erases it at runtime, so
    /// awaits and the event-loop capture rules can reason about the element.
    Future(Box<Ty>),
    Unknown,
}

/// Ownership state of a managed pointer binding, for the single owner rules.
/// A non managed binding has no entry at all.
#[derive(Clone, Copy, PartialEq)]
enum Own {
    Owner,
    Borrow,
    Moved,
}

/// Why a value may not cross an async boundary, so the diagnostic names the
/// reason precisely: a future belongs to the loop thread, while a slice,
/// closure, or interface value views the frame it was built in.
#[derive(Clone, Copy, PartialEq)]
enum CrossFail {
    Ok,
    Future,
    View,
}

struct TypeChecker {
    sigs: HashMap<String, (Vec<Ty>, Ty)>,
    // Async function names mapped to their declared return type, the element of
    // the future a call mints. Used to type an await, to check async_run, and to
    // reject an async name used as a value.
    async_fns: HashMap<String, Ty>,
    ifaces: HashSet<String>,
    enums: HashMap<String, Vec<String>>,
    // Each enum variant's declared payload field types, keyed by variant name,
    // which is globally unique. Used by the escape walk to gate a fat payload
    // returned by value the same way a struct field is gated.
    variant_payloads: HashMap<String, Vec<Ty>>,
    // Each struct's generic flag and its declared fields, for struct literal
    // validation. A generic struct checks field names only, since matching a
    // field's type parameter against a concrete value needs inference.
    structs: HashMap<String, (bool, Vec<(String, Ty)>)>,
    // Interface conformance state. `iface_methods` holds each interface's
    // required method names and arities, `impls` the `(interface, type)` pairs an
    // `impl` block declares, and `raw_sigs` the unfixed parameter types, so a call
    // can tell a concrete struct from the interface a parameter expects.
    iface_methods: HashMap<String, Vec<(String, usize)>>,
    impls: HashSet<(String, String)>,
    raw_sigs: HashMap<String, Vec<Ty>>,
    // Unfixed field types of every struct and enum, for the spawn capture walk,
    // which must see a slice or an interface buried in a field where the fixed
    // tables erase them.
    embed_fields: HashMap<String, Vec<Ty>>,
    // Bindings annotated with an interface type, per scope. Their checked type
    // is Unknown after fix, so the spawn capture check reads this record to
    // reject capturing an interface value, whose data pointer may sit in the
    // spawning frame.
    iface_binds: Vec<HashSet<String>>,
    // Bindings annotated with a slice-of-interface type, per scope, mapped to
    // their unfixed slice type. A later assignment of a slice of concrete structs
    // is the covariance error the fixed table cannot see, so the raw type is kept.
    slice_iface_elem: Vec<HashMap<String, Ty>>,
    // Bindings that hold a closure capturing a frame local, per scope. Its
    // environment sits on this frame, so returning such a binding, directly or
    // as a tuple member, escapes and is rejected the same as a returned lambda
    // literal that captures a local.
    esc_closures: Vec<HashMap<String, bool>>,
    // Bindings that hold a slice viewing a frame local array, per scope. The
    // array materializes on this frame, so returning the binding, directly or as
    // a tuple member, escapes. Tracked here because a slice from an array literal
    // loses its array type at the binding, so a return of the bare name cannot be
    // caught by type alone. A scope maps a name to its escape flag, so a shadowing
    // or a reassignment writes a fresh flag that masks any outer or stale one.
    esc_slices: Vec<HashMap<String, bool>>,
    scopes: Vec<HashMap<String, Ty>>,
    owns: Vec<HashMap<String, Own>>,
    // Mutable binding names, tracked here so an element or field store can walk
    // to its root binding and reject writing through an immutable one. The bare
    // `x = v` form is the resolver's; this pass owns the projection forms.
    muts: Vec<HashSet<String>>,
    // Error typed bindings that have not been handled yet, reported when their
    // scope pops. Handling is one of exists, check, or ignore on the binding, or
    // returning it to the caller.
    err_binds: Vec<HashMap<String, Span>>,
    // How many conditional or loop bodies enclose the current statement. A defer
    // inside one is rejected, since defers replay lexically at every return and
    // a conditional registration cannot be honored.
    branch_depth: u32,
    // True while checking an async func body, so async_run and the awaited
    // control forms can reject what is illegal inside a task.
    in_async: bool,
    cur_generics: HashSet<String>,
    cur_ret: Ty,
    errors: Vec<Diagnostic>,
}

impl TypeChecker {
    fn new() -> Self {
        TypeChecker {
            sigs: HashMap::new(),
            async_fns: HashMap::new(),
            ifaces: HashSet::new(),
            enums: HashMap::new(),
            variant_payloads: HashMap::new(),
            structs: HashMap::new(),
            iface_methods: HashMap::new(),
            impls: HashSet::new(),
            raw_sigs: HashMap::new(),
            embed_fields: HashMap::new(),
            iface_binds: Vec::new(),
            slice_iface_elem: Vec::new(),
            esc_closures: Vec::new(),
            esc_slices: Vec::new(),
            scopes: Vec::new(),
            owns: Vec::new(),
            muts: Vec::new(),
            err_binds: Vec::new(),
            branch_depth: 0,
            in_async: false,
            cur_generics: HashSet::new(),
            cur_ret: Ty::Unit,
            errors: Vec::new(),
        }
    }

    fn collect_sigs(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Interface(i) => {
                    self.ifaces.insert(i.name.clone());
                    let methods = i.methods.iter().map(|m| (m.name.clone(), m.params.len())).collect();
                    self.iface_methods.insert(i.name.clone(), methods);
                }
                Item::Enum(e) => {
                    let variants = e.variants.iter().map(|v| v.name.clone()).collect();
                    self.enums.insert(e.name.clone(), variants);
                    let gens: HashSet<String> = e.generics.iter().cloned().collect();
                    let fields = e
                        .variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(|f| lower(&f.ty, &gens)))
                        .collect();
                    self.embed_fields.insert(e.name.clone(), fields);
                    for v in &e.variants {
                        if !v.fields.is_empty() {
                            // Stored unfixed, so an interface payload element keeps
                            // its name for the slice-covariance check; the escape
                            // walk reads only the top-level shape.
                            let payloads = v.fields.iter().map(|f| lower(&f.ty, &gens)).collect();
                            self.variant_payloads.insert(v.name.clone(), payloads);
                        }
                    }
                }
                Item::Impl(im) => {
                    if let Some(iface) = &im.iface {
                        if !self.impls.insert((iface.clone(), im.ty.clone())) {
                            self.err(
                                format!("duplicate 'impl {iface} for {}'; merge the two blocks", im.ty),
                                im.span,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        for item in &module.items {
            match item {
                Item::Func(f) => {
                    let gens: HashSet<String> = f.generics.iter().cloned().collect();
                    self.raw_sigs
                        .insert(f.name.clone(), f.params.iter().map(|p| lower(&p.ty, &gens)).collect());
                    let params = f.params.iter().map(|p| self.fix(lower(&p.ty, &gens))).collect();
                    let ret = self.fix(lower(&f.ret, &gens));
                    if f.is_async {
                        // A call of an async func mints a Future of its declared
                        // return, so the signature table records the wrapped type
                        // and call inference types the site as a future with no
                        // extra code. async_fns keeps the element for await and
                        // async_run.
                        self.async_fns.insert(f.name.clone(), ret.clone());
                        self.sigs
                            .insert(f.name.clone(), (params, Ty::Future(Box::new(ret))));
                    } else {
                        self.sigs.insert(f.name.clone(), (params, ret));
                    }
                }
                Item::Foreign(fb) => {
                    // A foreign function is non generic, so its signature lowers
                    // against an empty generic set. It joins the same table an
                    // ordinary call resolves against, so the call is type checked.
                    let gens = HashSet::new();
                    for ff in &fb.funcs {
                        let params =
                            ff.params.iter().map(|p| self.fix(lower(&p.ty, &gens))).collect();
                        let ret = self.fix(lower(&ff.ret, &gens));
                        self.sigs.insert(ff.name.clone(), (params, ret));
                    }
                }
                Item::Struct(s) => {
                    let gens: HashSet<String> = s.generics.iter().cloned().collect();
                    let is_gen = !gens.is_empty();
                    let fields = s
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), self.fix(lower(&f.ty, &gens))))
                        .collect();
                    self.structs.insert(s.name.clone(), (is_gen, fields));
                    let raw = s.fields.iter().map(|f| lower(&f.ty, &gens)).collect();
                    self.embed_fields.insert(s.name.clone(), raw);
                }
                _ => {}
            }
        }
    }

    /// Interface typed slots accept any value (the boxing and impl dispatch
    /// happen in codegen), so they lower to Unknown for compatibility purposes.
    fn fix(&self, t: Ty) -> Ty {
        match t {
            Ty::Named(n) if self.ifaces.contains(&n) => Ty::Unknown,
            Ty::Ptr(b) => Ty::Ptr(Box::new(self.fix(*b))),
            Ty::RawPtr(b) => Ty::RawPtr(Box::new(self.fix(*b))),
            Ty::Slice(b) => Ty::Slice(Box::new(self.fix(*b))),
            Ty::Array(b, n) => Ty::Array(Box::new(self.fix(*b)), n),
            Ty::Tuple(xs) => Ty::Tuple(xs.into_iter().map(|x| self.fix(x)).collect()),
            Ty::Func(ps, r) => Ty::Func(
                ps.into_iter().map(|p| self.fix(p)).collect(),
                Box::new(self.fix(*r)),
            ),
            Ty::Future(b) => Ty::Future(Box::new(self.fix(*b))),
            other => other,
        }
    }

    fn run(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Func(f) => {
                    if f.name == "main" {
                        self.check_main_sig(f);
                    }
                    self.func(f, false);
                }
                Item::Impl(im) => {
                    self.check_impl_complete(im);
                    for m in &im.methods {
                        self.func(m, true);
                    }
                }
                Item::Foreign(fb) => self.check_foreign(fb),
                _ => {}
            }
        }
    }

    /// The only main signatures the C entry supports: no parameters, or exactly
    /// `(argc: int32, argv: string[])`, which the wrapper bridges. Any other
    /// shape would be emitted as the C `main` with the wrong parameters and read
    /// garbage registers, so it is rejected here.
    fn check_main_sig(&mut self, f: &Func) {
        if f.is_async {
            self.err(
                "main cannot be async; call an async func with async_run instead",
                f.span,
            );
            return;
        }
        if f.params.is_empty() {
            return;
        }
        let ok = f.params.len() == 2
            && !f.params.iter().any(|p| p.using)
            && self.lower(&f.params[0].ty) == Ty::Int(32)
            && matches!(self.lower(&f.params[1].ty), Ty::Slice(ref e) if matches!(**e, Ty::Str));
        if !ok {
            self.err(
                "main takes no parameters or (argc: int32, argv: string[]); the allocator form is not supported yet",
                f.span,
            );
        }
    }

    /// A foreign block ties external C symbols into the program. The abi must be
    /// "C", and every parameter and return type sits on the raw pointer layer, a
    /// scalar, a `*raw T`, or a `*void`. A managed `*T` carries a generation
    /// header C cannot read, so it never crosses, and an aggregate passed by value
    /// is left to a later interop release.
    fn check_foreign(&mut self, fb: &Foreign) {
        if fb.abi != "C" {
            self.err(
                format!("unsupported foreign abi \"{}\", only \"C\" is supported", fb.abi),
                fb.span,
            );
        }
        let empty = HashSet::new();
        for ff in &fb.funcs {
            for p in &ff.params {
                let ty = self.fix(lower(&p.ty, &empty));
                self.check_foreign_ty(&ty, &ff.name, fb.span);
            }
            let ret = self.fix(lower(&ff.ret, &empty));
            self.check_foreign_ty(&ret, &ff.name, fb.span);
        }
    }

    fn check_foreign_ty(&mut self, ty: &Ty, fname: &str, span: Span) {
        if foreign_ty_ok(ty) {
            return;
        }
        let msg = if is_managed(ty) {
            format!(
                "foreign function '{fname}' uses a managed pointer at the C boundary; \
                 use *raw T or *void, since a managed *T carries a generation C cannot read"
            )
        } else {
            format!(
                "foreign function '{fname}' uses a type the C boundary does not support yet; \
                 a foreign signature takes scalars and raw pointers, *raw T or *void"
            )
        };
        self.err(msg, span);
    }

    /// Validates a struct literal against the struct's declared fields. Every
    /// field value is inferred first, then the set is checked for a duplicate, an
    /// unknown field, and a missing field, with a type check on each field of a
    /// non generic struct. An unknown struct name stays permissive, since it may
    /// be imported or a generic instantiation the checker does not track yet.
    fn check_struct_lit(&mut self, name: &str, fields: &[(String, Expr)], span: Span) {
        let vals: Vec<Ty> = fields.iter().map(|(_, v)| self.infer(v)).collect();
        let Some((is_gen, declared)) = self.structs.get(name).cloned() else {
            return;
        };
        for i in 0..fields.len() {
            for j in (i + 1)..fields.len() {
                if fields[i].0 == fields[j].0 {
                    self.err(format!("field '{}' is set more than once in '{name}'", fields[i].0), span);
                }
            }
        }
        // The unfixed field types keep an interface slice element's name, which
        // the fixed table above erases, for the slice-covariance check.
        let raw_fields = self.embed_fields.get(name).cloned().unwrap_or_default();
        for ((fname, fexpr), vty) in fields.iter().zip(&vals) {
            match declared.iter().find(|(dn, _)| dn == fname) {
                None => self.err(format!("'{name}' has no field '{fname}'"), fexpr.span),
                Some((_, dty)) => {
                    if !is_gen && !compatible(dty, vty) {
                        self.err(
                            format!("field '{fname}' of '{name}' is set to a value of the wrong type"),
                            fexpr.span,
                        );
                    }
                    if let Some(idx) = declared.iter().position(|(dn, _)| dn == fname) {
                        if let Some(raw) = raw_fields.get(idx) {
                            self.check_slice_covariance(&raw.clone(), vty, fexpr, fexpr.span);
                        }
                    }
                }
            }
        }
        for (dn, _) in &declared {
            if !fields.iter().any(|(fname, _)| fname == dn) {
                self.err(format!("struct literal for '{name}' is missing field '{dn}'"), span);
            }
        }
    }

    fn func(&mut self, f: &Func, is_method: bool) {
        self.cur_generics = f.generics.iter().cloned().collect();
        self.cur_ret = lower(&f.ret, &self.cur_generics);
        self.branch_depth = 0;
        self.in_async = f.is_async;
        if f.is_async {
            self.check_async_sig(f);
        } else if matches!(&self.cur_ret, Ty::Named(n) if self.ifaces.contains(n)) {
            // Returning an interface value by value is not supported: codegen
            // boxes the payload into a frame slot that dangles once the function
            // returns, so it is refused here rather than miscompiled. An async
            // func already rejects it through the frame-view signature walk.
            self.err(
                "returning an interface value is not supported; return the concrete type or a pointer to it",
                f.span,
            );
        }
        self.push_scope();
        if is_method {
            self.declare("self", Ty::Unknown);
            // A method takes its receiver by pointer, so writing through
            // `self.field` mutates the caller's value by design.
            self.declare_mut("self");
        }
        for p in &f.params {
            let ty = self.lower(&p.ty);
            // A pointer parameter is a borrow with no keyword; the callee never
            // owns or frees it.
            if is_managed(&ty) {
                self.declare_own(&p.name, Own::Borrow);
            }
            let raw = lower(&p.ty, &self.cur_generics);
            if matches!(&raw, Ty::Named(n) if self.ifaces.contains(n)) {
                self.declare_iface_bind(&p.name);
            }
            self.declare(&p.name, ty);
        }
        for s in &f.body.stmts {
            self.stmt(s);
        }
        self.pop_scope();
        // A non void function must return on every path; falling off the end
        // would otherwise hand back a zeroed value with no diagnostic.
        if !matches!(self.cur_ret, Ty::Unit) && !block_returns(&f.body) {
            self.err(
                format!("not all paths in '{}' return a value", f.name),
                f.span,
            );
        }
        self.in_async = false;
    }

    /// The extra signature rules an async func obeys. Its frame outlives the
    /// call, so a value that views the caller's frame, a slice, a closure, an
    /// interface value, or a future the loop owns, may not cross as a parameter
    /// or a return. It also takes no type parameters, since the task frame and
    /// its future are laid out at the single declared shape.
    fn check_async_sig(&mut self, f: &Func) {
        if !f.generics.is_empty() {
            self.err("an async func cannot take type parameters", f.span);
        }
        for p in &f.params {
            // The raw lowering keeps interface and future names intact, which the
            // fixed form would erase to Unknown. A burial behind a generic type
            // argument is beyond this erased walk and is caught in mono.
            let raw = lower(&p.ty, &self.cur_generics);
            let mut seen = HashSet::new();
            match self.async_cross_reason(&raw, &mut seen) {
                CrossFail::Future => self.err(
                    format!("an async func cannot take '{}': a future belongs to the event loop thread; await it in the caller instead", p.name),
                    f.span,
                ),
                CrossFail::View => self.err(
                    format!("an async func cannot take '{}': a slice, closure, or interface value may view the caller's frame, which the task outlives", p.name),
                    f.span,
                ),
                CrossFail::Ok => {}
            }
        }
        let raw_ret = lower(&f.ret, &self.cur_generics);
        let mut seen = HashSet::new();
        match self.async_cross_reason(&raw_ret, &mut seen) {
            CrossFail::Future => self.err(
                "an async func cannot return a future; a future belongs to the event loop thread, so await it in the caller instead",
                f.span,
            ),
            CrossFail::View => self.err(
                "an async func cannot return a slice, closure, or interface value; the value would outlive the task frame it views",
                f.span,
            ),
            CrossFail::Ok => {}
        }
    }

    /// Why a value may not cross an async boundary, so a diagnostic can name the
    /// reason. A future is loop-thread property; a slice, closure, or interface
    /// value views the frame. Walks the same shape as `spawn_capturable` but over
    /// the raw `Ty`, so it sees a future or interface a struct field buries; a
    /// burial behind a generic type argument is erased here and caught in mono.
    fn async_cross_reason(&self, t: &Ty, seen: &mut HashSet<String>) -> CrossFail {
        match t {
            Ty::Future(_) => CrossFail::Future,
            Ty::Slice(_) | Ty::Func(..) => CrossFail::View,
            Ty::Array(e, _) => self.async_cross_reason(e, seen),
            Ty::Tuple(ts) => ts
                .iter()
                .map(|x| self.async_cross_reason(x, seen))
                .find(|r| !matches!(r, CrossFail::Ok))
                .unwrap_or(CrossFail::Ok),
            Ty::Named(n) => {
                if self.ifaces.contains(n) {
                    return CrossFail::View;
                }
                if !seen.insert(n.clone()) {
                    return CrossFail::Ok;
                }
                match self.embed_fields.get(n).cloned() {
                    Some(fs) => fs
                        .iter()
                        .map(|f| self.async_cross_reason(f, seen))
                        .find(|r| !matches!(r, CrossFail::Ok))
                        .unwrap_or(CrossFail::Ok),
                    None => CrossFail::Ok,
                }
            }
            _ => CrossFail::Ok,
        }
    }

    fn lower(&self, t: &Type) -> Ty {
        self.fix(lower(t, &self.cur_generics))
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.owns.push(HashMap::new());
        self.muts.push(HashSet::new());
        self.err_binds.push(HashMap::new());
        self.iface_binds.push(HashSet::new());
        self.slice_iface_elem.push(HashMap::new());
        self.esc_closures.push(HashMap::new());
        self.esc_slices.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.owns.pop();
        self.muts.pop();
        self.iface_binds.pop();
        self.slice_iface_elem.pop();
        self.esc_closures.pop();
        self.esc_slices.pop();
        // Every error bound in this scope must have been handled by now. The
        // handled ones were removed at their handling site; the rest report.
        if let Some(pending) = self.err_binds.pop() {
            let mut pending: Vec<(String, Span)> = pending.into_iter().collect();
            pending.sort_by_key(|(_, s)| s.lo);
            for (name, span) in pending {
                self.err(
                    format!(
                        "the error '{name}' is never handled; inspect it with exists, handle it with check, or discard it with ignore"
                    ),
                    span,
                );
            }
        }
    }

    /// Marks a binding as mutable in the current scope.
    fn declare_mut(&mut self, name: &str) {
        if let Some(scope) = self.muts.last_mut() {
            scope.insert(name.to_string());
        }
    }

    fn is_mutable(&self, name: &str) -> bool {
        self.muts.iter().rev().any(|s| s.contains(name))
    }

    /// Records a binding annotated with an interface type in the current scope.
    fn declare_iface_bind(&mut self, name: &str) {
        if let Some(scope) = self.iface_binds.last_mut() {
            scope.insert(name.to_string());
        }
    }

    fn is_iface_bind(&self, name: &str) -> bool {
        self.iface_binds.iter().rev().any(|s| s.contains(name))
    }

    fn is_esc_closure(&self, name: &str) -> bool {
        Self::esc_flag(&self.esc_closures, name)
    }

    fn is_esc_slice(&self, name: &str) -> bool {
        Self::esc_flag(&self.esc_slices, name)
    }

    /// The escape flag of a name from the innermost scope that binds it. Stopping
    /// at the first scope that has the key lets an inner binding, a shadow or a
    /// reassignment, write `false` and mask an outer or stale `true`.
    fn esc_flag(scopes: &[HashMap<String, bool>], name: &str) -> bool {
        for scope in scopes.iter().rev() {
            if let Some(&flag) = scope.get(name) {
                return flag;
            }
        }
        false
    }

    /// Records, in the current scope, whether a binding holds an escaping slice or
    /// closure. The flag is written unconditionally, not removed, so a reassign to
    /// a clean value writes `false` and masks a stale `true` in the same scope,
    /// and a shadowing bind writes its own flag into the inner scope.
    fn set_esc(&mut self, name: &str, slice: bool, closure: bool) {
        if let Some(scope) = self.esc_slices.last_mut() {
            scope.insert(name.to_string(), slice);
        }
        if let Some(scope) = self.esc_closures.last_mut() {
            scope.insert(name.to_string(), closure);
        }
    }

    /// Updates the escape flags of a reassigned binding in the scope that owns it,
    /// not the current branch scope which a pop would discard. Inside a
    /// conditional or loop body the update is a may-join: it can only raise a
    /// flag, since the branch may or may not run, so `if c { t = escaping }`
    /// leaves t able to escape and `if c { t = clean }` cannot prove it clean. At
    /// straight line the last assignment wins, so an unconditional reassign to a
    /// clean value clears a prior escape.
    fn assign_esc(&mut self, name: &str, slice: bool, closure: bool) {
        let conditional = self.branch_depth > 0;
        Self::update_owner(&mut self.esc_slices, name, slice, conditional);
        Self::update_owner(&mut self.esc_closures, name, closure, conditional);
    }

    fn update_owner(scopes: &mut [HashMap<String, bool>], name: &str, flag: bool, conditional: bool) {
        for scope in scopes.iter_mut().rev() {
            if let Some(entry) = scope.get_mut(name) {
                *entry = if conditional { *entry || flag } else { flag };
                return;
            }
        }
        if let Some(scope) = scopes.last_mut() {
            scope.insert(name.to_string(), flag);
        }
    }

    /// Raises the escape flags of a binding, never clearing them. A field store,
    /// `s.rows = ...`, can only add a frame-local view to a struct binding, since
    /// another field may still hold one, so it joins into the owning scope like a
    /// conditional assignment regardless of branch depth.
    fn raise_esc(&mut self, name: &str, slice: bool, closure: bool) {
        Self::update_owner(&mut self.esc_slices, name, slice, true);
        Self::update_owner(&mut self.esc_closures, name, closure, true);
    }

    /// Classifies a bound value as holding a frame-local slice and/or closure, so
    /// returning the binding later is caught even when the return names the bare
    /// binding. A slice into a local array (an array literal or a range slice of a
    /// local array), a lambda that captures a local, an alias of a binding that
    /// already holds either, or a tuple whose member does, all propagate. A slice
    /// from a parameter or a heap array, and a closure capturing only parameters
    /// or globals, do not, so a legal return is not over-rejected.
    fn value_escape(&self, e: &Expr) -> (bool, bool) {
        match &e.kind {
            ExprKind::Array(_) => (true, false),
            ExprKind::Index(base, idx) if matches!(idx.kind, ExprKind::Range(..)) => {
                // A range slice escapes when its base is a local array, or when the
                // base is a binding already known to hold a frame-local slice; an
                // unannotated array literal binding infers to a slice, so the type
                // check alone would miss re-slicing it.
                let base_local = matches!(self.chain_ty(base), Ty::Array(..))
                    || matches!(&base.kind, ExprKind::Ident(n) if self.is_esc_slice(n));
                (base_local, false)
            }
            ExprKind::Lambda(l) => (false, self.lambda_captures_local(l)),
            ExprKind::Ident(n) => (self.is_esc_slice(n), self.is_esc_closure(n)),
            // A projection extracts a fat sub-value out of an aggregate. When the
            // aggregate roots to a local binding holding a frame-local view, the
            // projected slice, closure, tuple, struct, or enum views it too. A
            // scalar member, or a projection rooted in a param or heap aggregate,
            // does not escape. The range-index re-slice is handled above.
            ExprKind::Field(..) | ExprKind::Index(..) => self.projection_escape(e),
            ExprKind::Tuple(members) => {
                let mut slice = false;
                let mut closure = false;
                for m in members {
                    let (s, c) = self.value_escape(m);
                    slice |= s;
                    closure |= c;
                }
                (slice, closure)
            }
            // A struct returned by value carries its fat fields, so a field
            // holding a frame-local view escapes through it. Only a reference
            // shaped field, a slice, closure, tuple, or nested struct, can view a
            // frame; a fixed array or scalar field holds its value inline.
            ExprKind::StructLit(name, fields) => {
                let mut slice = false;
                let mut closure = false;
                if let Some((_, decl)) = self.structs.get(name).cloned() {
                    for (fname, init) in fields {
                        if let Some((_, fty)) = decl.iter().find(|(n, _)| n == fname) {
                            let (s, c) = self.field_kind_escape(init, fty);
                            slice |= s;
                            closure |= c;
                        }
                    }
                }
                (slice, closure)
            }
            // An enum constructor returned by value carries its payload, so a fat
            // payload viewing a frame local escapes. The payload is gated by the
            // variant's declared field types, the same discipline as a struct
            // field. Both the bare `V(x)` and the qualified `E.V(x)` forms resolve
            // to the variant by its globally unique name.
            ExprKind::Call(callee, args) => {
                let variant = match &callee.kind {
                    ExprKind::Ident(v) => Some(v),
                    ExprKind::Field(base, v)
                        if matches!(&base.kind, ExprKind::Ident(en) if self.enums.contains_key(en)) =>
                    {
                        Some(v)
                    }
                    _ => None,
                };
                let mut slice = false;
                let mut closure = false;
                if let Some(payloads) = variant.and_then(|v| self.variant_payloads.get(v)).cloned() {
                    for (arg, pty) in args.iter().zip(&payloads) {
                        let (s, c) = self.field_kind_escape(arg, pty);
                        slice |= s;
                        closure |= c;
                    }
                }
                (slice, closure)
            }
            // A match used as a return value builds its result from the arm tails.
            // A tail that names one of the arm's payload bindings projects the
            // scrutinee's payload, so it inherits the scrutinee's escape; any other
            // tail is classified on its own.
            ExprKind::Match(m) => {
                let (ss, sc) = self.value_escape(&m.scrut);
                let mut slice = false;
                let mut closure = false;
                for arm in &m.arms {
                    if let Some(Stmt::Expr(tail)) = arm.body.stmts.last() {
                        let projects_payload = matches!(
                            (&tail.kind, &arm.pat),
                            (ExprKind::Ident(n), Pattern::Variant(_, binds)) if binds.contains(n)
                        );
                        let (s, c) = if projects_payload {
                            (ss, sc)
                        } else {
                            self.value_escape(tail)
                        };
                        slice |= s;
                        closure |= c;
                    }
                }
                (slice, closure)
            }
            _ => (false, false),
        }
    }

    /// The escape of a projection, a field access or a non-range index. Only a fat
    /// projected member can carry a view; the projection escapes when its base
    /// roots to a binding recorded as holding a frame-local slice or closure.
    fn projection_escape(&self, e: &Expr) -> (bool, bool) {
        if !self.member_carries_view(&self.chain_ty(e)) {
            return (false, false);
        }
        match self.projection_root(e) {
            Some(root) => (self.is_esc_slice(&root), self.is_esc_closure(&root)),
            None => (false, false),
        }
    }

    /// The base binding of a projection chain, rooting through every field access
    /// and index, whether the indexed base is an array or a slice. Unlike
    /// value_chain_root, which stops the immutability walk at a slice indirection,
    /// this reaches the binding whose escape flag governs the projected view, since
    /// indexing a slice that views a frame local still yields a frame-local view.
    /// A pointer dereference roots to nothing: its target is heap.
    fn projection_root(&self, e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            ExprKind::Field(base, _) | ExprKind::Index(base, _) => self.projection_root(base),
            _ => None,
        }
    }

    /// Whether a type can carry a frame-local view when projected out by value: a
    /// slice, closure, tuple, struct, enum, interface, or a fat array of them. A
    /// scalar, a pointer, or a scalar array is copied or heap-backed and cannot.
    fn member_carries_view(&self, t: &Ty) -> bool {
        match t {
            Ty::Slice(_) | Ty::Func(..) | Ty::Tuple(_) => true,
            Ty::Named(n) => {
                self.structs.contains_key(n) || self.enums.contains_key(n) || self.ifaces.contains(n)
            }
            Ty::Array(e, _) => self.member_carries_view(e),
            _ => false,
        }
    }

    /// The escape of a struct field or enum payload initializer, gated by the
    /// declared field type. A reference shaped field, a slice, closure, tuple,
    /// struct, enum, or fat fixed array, views whatever its initializer does; a
    /// fixed array of scalars or a scalar field copies its initializer inline, so
    /// a local array literal in it does not escape. This covers the SAME carrier
    /// set as the top-level escape walk, so a carrier nested one level down, a
    /// field whose type is an enum or fat array, is caught at any depth.
    fn field_kind_escape(&self, init: &Expr, fty: &Ty) -> (bool, bool) {
        match fty {
            Ty::Slice(_) | Ty::Func(..) | Ty::Tuple(_) => self.value_escape(init),
            Ty::Named(n)
                if self.structs.contains_key(n)
                    || self.enums.contains_key(n)
                    || self.ifaces.contains(n) =>
            {
                self.value_escape(init)
            }
            // A fat fixed array mirrors the top-level array walk: a literal is
            // checked per element by the element type, a binding by its recorded
            // flags. Routing straight to value_escape would flag a param-backed
            // array literal, so the element gate is kept here too.
            Ty::Array(elem, _)
                if matches!(**elem, Ty::Slice(_) | Ty::Func(..) | Ty::Tuple(_) | Ty::Named(_)) =>
            {
                if let ExprKind::Array(elems) = &init.kind {
                    let mut slice = false;
                    let mut closure = false;
                    for el in elems {
                        let (s, c) = self.field_kind_escape(el, elem);
                        slice |= s;
                        closure |= c;
                    }
                    (slice, closure)
                } else {
                    self.value_escape(init)
                }
            }
            // A generic type parameter, or an interface field, erases to Unknown,
            // so the declared type cannot say the field is fat. Fall back to the
            // initializer's own dataflow: a frame-local view buried behind a type
            // parameter is caught, while a param or heap init is still accepted.
            Ty::Unknown => self.value_escape(init),
            _ => (false, false),
        }
    }

    /// Registers an error typed binding as pending until a handling site clears it.
    fn declare_err(&mut self, name: &str, span: Span) {
        if let Some(scope) = self.err_binds.last_mut() {
            scope.insert(name.to_string(), span);
        }
    }

    /// Clears an error binding's pending state, from innermost scope outward.
    fn mark_err_handled(&mut self, name: &str) {
        for scope in self.err_binds.iter_mut().rev() {
            if scope.remove(name).is_some() {
                return;
            }
        }
    }

    /// Marks every error binding an expression mentions as handled. Used for
    /// `return e` and `return (v, e)`, which hand the error to the caller.
    fn mark_errs_in(&mut self, e: &Expr) {
        let mut used = Vec::new();
        let mut bound = HashSet::new();
        collect_expr(e, &mut used, &mut bound);
        for name in used {
            self.mark_err_handled(&name);
        }
    }

    fn declare(&mut self, name: &str, ty: Ty) {
        if name.is_empty() {
            return;
        }
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }

    fn lookup(&self, name: &str) -> Ty {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.get(name) {
                return t.clone();
            }
        }
        if let Some((params, ret)) = self.sigs.get(name) {
            return Ty::Func(params.clone(), Box::new(ret.clone()));
        }
        Ty::Unknown
    }

    /// Whether a name is a local binding in some enclosing scope, so a global
    /// like an async function name it shadows is not consulted.
    fn is_local(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains_key(name))
    }

    /// The unfixed slice-of-interface type a binding was annotated with, if any,
    /// so an assignment into it can be checked for slice covariance.
    fn slice_iface_of(&self, name: &str) -> Option<Ty> {
        for scope in self.slice_iface_elem.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    /// Records the ownership state of a managed pointer binding in the current
    /// scope. Only `*T` bindings get an entry; raw and non pointer bindings do
    /// not participate in the single owner rules.
    fn declare_own(&mut self, name: &str, own: Own) {
        if let Some(scope) = self.owns.last_mut() {
            scope.insert(name.to_string(), own);
        }
    }

    fn own_of(&self, name: &str) -> Option<Own> {
        for scope in self.owns.iter().rev() {
            if let Some(o) = scope.get(name) {
                return Some(*o);
            }
        }
        None
    }

    fn set_moved(&mut self, name: &str) {
        for scope in self.owns.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), Own::Moved);
                return;
            }
        }
    }

    fn err(&mut self, msg: impl Into<String>, span: Span) {
        self.errors.push(Diagnostic::new(msg, span));
    }

    fn block(&mut self, b: &Block) {
        // Snapshot ownership so a move inside a conditional or loop branch does
        // not leak out to the straight line code after it. The static pass is
        // single block with no cross branch data flow; the runtime generation
        // check backstops a move that a branch actually took.
        let saved = self.owns.clone();
        self.push_scope();
        for s in &b.stmts {
            self.stmt(s);
        }
        self.pop_scope();
        self.owns = saved;
    }

    /// A block that hangs off a conditional or loop, tracked so a `defer` inside
    /// one can be rejected.
    fn branch_block(&mut self, b: &Block) {
        self.branch_depth += 1;
        self.block(b);
        self.branch_depth -= 1;
    }

    /// Enforces immutability through projections: an element or field store whose
    /// chain stays in value territory, arrays and struct fields, must root in a
    /// mutable binding. A store through a pointer, a slice, or a string writes a
    /// buffer the binding merely views, which the resolver's function scope rules
    /// govern instead. The bare `x = v` form is the resolver's; this pass adds
    /// the `xs[i] = v` and `s.f = v` forms it cannot see.
    fn check_assign_target(&mut self, lhs: &Expr) {
        if matches!(&lhs.kind, ExprKind::Ident(_)) {
            return;
        }
        // A field store on a pointer never reaches memory today; the language
        // requires an explicit dereference, so say so instead of dropping it.
        if let ExprKind::Field(base, _) = &lhs.kind {
            if matches!(self.chain_ty(base), Ty::Ptr(_) | Ty::RawPtr(_)) {
                self.err(
                    "a field store through a pointer needs an explicit dereference; write (*p).field",
                    lhs.span,
                );
                return;
            }
        }
        if let Some((root, span)) = self.value_chain_root(lhs) {
            if root != "self" && !self.is_mutable(&root) {
                self.err(
                    format!("cannot assign through immutable '{root}'; declare it 'mut'"),
                    span,
                );
            }
        }
    }

    /// The root binding of an assignment chain that stays in value territory, or
    /// None once the chain crosses a pointer, slice, or string indirection.
    fn value_chain_root(&self, e: &Expr) -> Option<(String, Span)> {
        match &e.kind {
            ExprKind::Ident(n) => Some((n.clone(), e.span)),
            ExprKind::Field(base, _) => self.value_chain_root(base),
            ExprKind::Index(base, _) => match self.chain_ty(base) {
                Ty::Array(..) => self.value_chain_root(base),
                _ => None,
            },
            _ => None,
        }
    }

    /// A side effect free type walk for assignment chains. Unlike `infer`, it
    /// never emits diagnostics, so walking the same expression twice is safe.
    fn chain_ty(&self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Ident(n) => self.lookup(n),
            ExprKind::Field(base, fname) => match self.chain_ty(base) {
                Ty::Named(s) => self
                    .structs
                    .get(&s)
                    .and_then(|(_, fs)| fs.iter().find(|(n, _)| n == fname).map(|(_, t)| t.clone()))
                    .unwrap_or(Ty::Unknown),
                _ => Ty::Unknown,
            },
            ExprKind::Index(base, _) => elem_of(&self.chain_ty(base)),
            ExprKind::Unary(UnOp::Deref, p) => match self.chain_ty(p) {
                Ty::Ptr(inner) | Ty::RawPtr(inner) => *inner,
                _ => Ty::Unknown,
            },
            _ => Ty::Unknown,
        }
    }

    /// Rejects an integer literal that cannot fit the width it is bound to, as in
    /// `x: int8 = 300`. Only a direct literal (or its negation) is checked; a
    /// computed value is the programmer's to range.
    fn check_int_fits(&mut self, e: &Expr, ty: &Ty) {
        let Ty::Int(w @ (8 | 16 | 32)) = ty else {
            return;
        };
        let (neg, v) = match &e.kind {
            ExprKind::Int(v, None) => (false, *v),
            ExprKind::Unary(UnOp::Neg, inner) => match &inner.kind {
                ExprKind::Int(v, None) => (true, *v),
                _ => return,
            },
            _ => return,
        };
        let val = if neg { -(v as i128) } else { v as i128 };
        // Signedness is not tracked yet, so accept the union of the signed and
        // unsigned ranges for the width; a value outside both cannot be right.
        let lo = -(1i128 << (w - 1));
        let hi = (1i128 << w) - 1;
        if val < lo || val > hi {
            self.err(format!("literal {val} does not fit in {w} bits"), e.span);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(l) => self.let_stmt(l),
            Stmt::Assign(lhs, rhs) => {
                let lt = self.infer(lhs);
                let rt = self.infer(rhs);
                if !compatible(&lt, &rt) {
                    self.err("assignment type mismatch", lhs.span);
                }
                self.check_int_fits(rhs, &lt);
                self.check_assign_target(lhs);
                if is_managed(&lt) {
                    // `q = p` aliases two owners exactly like the let form copy, so
                    // flag it the same. This takes priority, naming the precise
                    // mistake when both sides are owners.
                    let copy_of_owner = match (&lhs.kind, &rhs.kind) {
                        (ExprKind::Ident(_), ExprKind::Ident(src)) => {
                            matches!(self.own_of(src), Some(Own::Owner))
                        }
                        _ => false,
                    };
                    if copy_of_owner {
                        self.err(
                            "cannot copy an owning pointer; bind a `ref` alias or `move` it",
                            rhs.span,
                        );
                    } else if let ExprKind::Ident(dst) = &lhs.kind {
                        // Reassigning an owning pointer drops the allocation it
                        // holds without freeing it, a leak. A borrowing cursor, as
                        // in a list walk, is not an owner and may advance.
                        if matches!(self.own_of(dst), Some(Own::Owner)) {
                            self.err(
                                "cannot reassign an owning pointer; it leaks the allocation it holds, free it first or bind a new pointer",
                                lhs.span,
                            );
                        }
                    }
                }
                // Reassigning a bare binding updates its escape flag in the scope
                // that owns it, so a rebind inside a branch is not lost when the
                // branch scope pops. A field store, `s.rows = ...`, cannot rebind
                // the whole binding but can put a frame-local view into a struct
                // field, so it raises the root binding's flag, never clears it.
                if let ExprKind::Ident(dst) = &lhs.kind {
                    let (esc_s, esc_c) = self.value_escape(rhs);
                    self.assign_esc(dst, esc_s, esc_c);
                    // Assigning a slice of concrete structs to a slice-of-interface
                    // binding is the covariance error the fixed binding type hides.
                    if let Some(raw) = self.slice_iface_of(dst) {
                        self.check_slice_covariance(&raw, &rt, rhs, rhs.span);
                    }
                } else if let Some((root, _)) = self.value_chain_root(lhs) {
                    let (esc_s, esc_c) = self.value_escape(rhs);
                    if esc_s || esc_c {
                        self.raise_esc(&root, esc_s, esc_c);
                    }
                }
            }
            Stmt::AssignOp(op, lhs, rhs) => {
                // The place type governs the operation. The result must be
                // compatible with it, and the mut rules are the plain assignment's.
                let lt = self.infer(lhs);
                let rt = self.infer(rhs);
                let result = self.check_binary(*op, &lt, &rt, lhs.span);
                match op {
                    BinOp::Shl | BinOp::Shr => self.check_shift_amount(rhs, &lt, lhs.span),
                    BinOp::Pow => self.check_pow_exponent(&lt, &rt, rhs, lhs.span),
                    _ => {}
                }
                if !compatible(&result, &lt) {
                    self.err("assignment type mismatch", lhs.span);
                }
                self.check_int_fits(rhs, &lt);
                self.check_assign_target(lhs);
            }
            Stmt::Return(Some(e)) => {
                // `return await f` returns the awaited element; its type must
                // match the async func's declared return, and the operand's
                // error bindings are handed to the caller like any returned tuple.
                if let ExprKind::Await(op, _) = &e.kind {
                    let el = self.infer_await(op, e.span);
                    let ret = self.cur_ret.clone();
                    if !compatible(&ret, &el) {
                        self.err(
                            "return type does not match the function's return type",
                            e.span,
                        );
                    }
                    self.mark_errs_in(op);
                    return;
                }
                let t = self.infer(e);
                let ret = self.cur_ret.clone();
                // Returning a concrete struct where an interface is declared is
                // the boxing site; it needs an impl, checked precisely here, and
                // the plain mismatch error would misfire on the valid case.
                let iface_ret = matches!((&ret, &t), (Ty::Named(i), Ty::Named(_)) if self.ifaces.contains(i));
                if iface_ret {
                    self.check_conformance(&ret, &t, e.span);
                } else if self.tuple_iface_mismatch(&ret, &t, e.span) {
                    // A precise interface-in-tuple error already fired; the generic
                    // mismatch would otherwise double it, since the unfixed return
                    // tuple never `compatible`s against the concrete member.
                } else if !compatible(&ret, &t) {
                    self.err("return type does not match the function's return type", e.span);
                }
                self.check_int_fits(e, &ret);
                self.check_escape(e, &t);
                // A returned error is the caller's to handle.
                self.mark_errs_in(e);
            }
            Stmt::Return(None) => {
                if !compatible(&self.cur_ret.clone(), &Ty::Unit) {
                    self.err("missing return value", Span::new(0, 0));
                }
            }
            Stmt::Defer(e) => {
                if self.branch_depth > 0 {
                    self.err(
                        "a defer inside a conditional or loop is not supported; register it at the top level of the function",
                        e.span,
                    );
                }
                self.infer(e);
            }
            Stmt::If(i) => {
                let c = self.infer(&i.cond);
                if !compatible(&Ty::Bool, &c) {
                    self.err("if condition must be a bool", i.cond.span);
                }
                self.branch_block(&i.then);
                if let Some(els) = &i.els {
                    self.branch_block(els);
                }
            }
            Stmt::While(w) => {
                let c = self.infer(&w.cond);
                if !compatible(&Ty::Bool, &c) {
                    self.err("while condition must be a bool", w.cond.span);
                }
                self.branch_block(&w.body);
            }
            Stmt::For(f) => {
                self.infer(&f.iter);
                self.push_scope();
                self.declare(&f.var, Ty::Unknown);
                self.branch_block(&f.body);
                self.pop_scope();
            }
            Stmt::Match(m) => self.walk_match(m),
            Stmt::Expr(e) => {
                // The void await form discards its value; it is legal only when
                // the awaited element is void, else the value must be bound.
                if let ExprKind::Await(op, _) = &e.kind {
                    let el = self.infer_await(op, e.span);
                    if !compatible(&Ty::Unit, &el) {
                        self.err(
                            "'await f' discards a value; bind it, as in v, e := await f",
                            e.span,
                        );
                    }
                    return;
                }
                // A bare call of an async func mints a future that is dropped
                // before it can be awaited or released.
                if let ExprKind::Call(callee, _) = &e.kind {
                    if let ExprKind::Ident(g) = &callee.kind {
                        if self.async_fns.contains_key(g) && !self.is_local(g) {
                            self.err(
                                format!("the future from '{g}' is never awaited; bind it so it can be awaited or released"),
                                e.span,
                            );
                        }
                    }
                }
                let t = self.infer(e);
                // A fallible expression used as a bare statement drops its error.
                // The spec requires every error to be handled, so bind the result
                // and handle the error with exists, check, or ignore. A blessed
                // handler returns bool or unit, so it is not flagged here.
                if ty_has_error(&t) {
                    self.err(
                        "this expression's error result is ignored; bind it and handle the error with exists, check, or ignore",
                        e.span,
                    );
                }
            }
        }
    }

    fn let_stmt(&mut self, l: &Let) {
        // An awaited value binds by the await result shape rule, not the ordinary
        // value path, so intercept it before inference reaches the backstop.
        if let ExprKind::Await(op, _) = &l.value.kind {
            self.let_await(l, op);
            return;
        }
        // `p: *T = alloc()` is the uninitialized allocation; its size comes from
        // the annotation, so the form is only valid with a pointer annotation on
        // a single binding. Intercepted before infer, whose alloc arm rejects
        // every other zero argument alloc site.
        if is_zero_arg_alloc(&l.value, &self.sigs) && l.binds.len() == 1 {
            let b = &l.binds[0];
            let ty = match &b.ty {
                Some(t) => {
                    let lt = self.lower(t);
                    if !matches!(lt, Ty::Ptr(_)) {
                        self.err(
                            "alloc() with no value takes its size from a pointer annotation; write p: *T = alloc()",
                            l.value.span,
                        );
                    }
                    lt
                }
                None => {
                    self.err(
                        "alloc() with no value needs a pointer type annotation to size the allocation; write p: *T = alloc()",
                        l.value.span,
                    );
                    Ty::Unknown
                }
            };
            self.declare(&b.name, ty.clone());
            if l.mutable {
                self.declare_mut(&b.name);
            }
            if is_managed(&ty) {
                self.declare_own(&b.name, Own::Owner);
            }
            return;
        }
        let vt = self.infer(&l.value);
        if l.binds.len() == 1 {
            let b = &l.binds[0];
            let ty = match &b.ty {
                Some(t) => {
                    let lt = self.lower(t);
                    // The annotation with the interface name intact, so binding a
                    // struct to an interface checks its impl instead of emitting
                    // a reference to a vtable that does not exist.
                    let raw = lower(t, &self.cur_generics);
                    self.check_conformance(&raw, &vt, l.value.span);
                    self.check_slice_covariance(&raw, &vt, &l.value, l.value.span);
                    if matches!(&raw, Ty::Named(n) if self.ifaces.contains(n)) {
                        self.declare_iface_bind(&b.name);
                    }
                    // Remember a slice-of-interface binding so a later assignment
                    // of a slice of concrete structs is caught as covariance.
                    if matches!(&raw, Ty::Slice(el) if matches!(&**el, Ty::Named(n) if self.ifaces.contains(n))) {
                        if let Some(scope) = self.slice_iface_elem.last_mut() {
                            scope.insert(b.name.clone(), raw.clone());
                        }
                    }
                    if !compatible(&lt, &vt) {
                        self.err(format!("'{}' has a type annotation that does not match its value", b.name), l.value.span);
                    }
                    self.check_int_fits(&l.value, &lt);
                    lt
                }
                // An unannotated binding of a bare literal takes the default
                // width, int64 or float64, so the binding is concretely typed
                // and cannot launder a width mismatch later.
                None => harden(vt.clone()),
            };
            self.declare(&b.name, ty.clone());
            if l.mutable {
                self.declare_mut(&b.name);
            }
            if matches!(ty, Ty::Error) {
                self.declare_err(&b.name, l.value.span);
            }
            if is_managed(&ty) {
                let own = self.binding_own(l, &vt);
                self.declare_own(&b.name, own);
            }
            // Record whether the binding holds a frame-local slice or closure, so
            // returning the bare name, or an alias of it, is caught as an escape
            // even though the return expression is not the escaping literal.
            let (esc_s, esc_c) = self.value_escape(&l.value);
            self.set_esc(&b.name, esc_s, esc_c);
            return;
        }
        let parts = match &vt {
            Ty::Tuple(ts) if ts.len() == l.binds.len() => ts.clone(),
            Ty::Unknown => vec![Ty::Unknown; l.binds.len()],
            _ => {
                self.err("destructuring binding expects a tuple of matching arity", l.value.span);
                vec![Ty::Unknown; l.binds.len()]
            }
        };
        for (b, pt) in l.binds.iter().zip(parts) {
            let ty = match &b.ty {
                Some(t) => self.lower(t),
                None => harden(pt),
            };
            // A destructured managed pointer is conservatively an owner, so it is
            // tracked and freeing it is not wrongly rejected. The static copy
            // check for the destructure itself falls to the runtime backstop.
            if is_managed(&ty) {
                self.declare_own(&b.name, Own::Owner);
            }
            if l.mutable {
                self.declare_mut(&b.name);
            }
            if matches!(ty, Ty::Error) {
                self.declare_err(&b.name, l.value.span);
            }
            self.declare(&b.name, ty);
        }
        // Destructuring a tuple literal binds each name to a member, so a member
        // that holds a frame-local slice or closure marks its bound name escaping.
        if let ExprKind::Tuple(members) = &l.value.kind {
            if members.len() == l.binds.len() {
                for (b, m) in l.binds.iter().zip(members) {
                    let (esc_s, esc_c) = self.value_escape(m);
                    self.set_esc(&b.name, esc_s, esc_c);
                }
            }
        }
    }

    /// The ownership of a single managed pointer binding. A `ref` binding is a
    /// non owning alias; alloc, move, or a pointer returning call produce an
    /// owner; a plain copy of a borrow is a borrow; and a plain copy of an owner
    /// is the flagged single owner violation.
    fn binding_own(&mut self, l: &Let, vt: &Ty) -> Own {
        if l.is_ref {
            return Own::Borrow;
        }
        match &l.value.kind {
            // A call owns its managed `*T` result, since a pointer return moves
            // ownership out. A call that returns `*void` or a raw pointer, like
            // an arena bump, hands back a view into something else, not a fresh
            // owner, so the binding borrows.
            ExprKind::Call(..) => {
                if is_managed(vt) {
                    Own::Owner
                } else {
                    Own::Borrow
                }
            }
            ExprKind::Ident(src) => match self.own_of(src) {
                Some(Own::Owner) => {
                    self.err(
                        "cannot copy an owning pointer; bind a `ref` alias or `move` it",
                        l.value.span,
                    );
                    Own::Owner
                }
                _ => Own::Borrow,
            },
            // A projection that detaches a managed pointer, a deref, field, or
            // index, is treated as an owner so freeing it is not rejected; the
            // runtime generation check backstops any aliasing this does not see.
            _ => Own::Owner,
        }
    }

    /// Infers the element type of an awaited operand. The operand must be a
    /// future; its element is what the await yields. A non-future operand is
    /// rejected, and an Unknown operand stays permissive.
    fn infer_await(&mut self, op: &Expr, span: Span) -> Ty {
        match self.infer(op) {
            Ty::Future(el) => *el,
            Ty::Unknown => Ty::Unknown,
            _ => {
                self.err("the operand of await is not a future", span);
                Ty::Unknown
            }
        }
    }

    /// Types the bindings of `binds := await op` by the await result shape rule.
    /// A tuple element destructures member-wise, so the must-handle rule fires on
    /// an error member; a non-tuple element binds its value and, with a second
    /// name, the completer's error word; a single name discards that error word.
    fn let_await(&mut self, l: &Let, op: &Expr) {
        let el = self.infer_await(op, l.value.span);
        let n = l.binds.len();
        match &el {
            Ty::Tuple(ts) if n > 1 => {
                if ts.len() != n {
                    self.err(
                        format!(
                            "await destructures this future into {} values, but {n} names are bound",
                            ts.len()
                        ),
                        l.value.span,
                    );
                }
                for (b, pt) in l.binds.iter().zip(ts) {
                    self.declare_await_bind(l, b, pt.clone());
                }
                // Bind any surplus names to Unknown so later references resolve.
                for b in l.binds.iter().skip(ts.len()) {
                    self.declare(&b.name, Ty::Unknown);
                }
            }
            _ if n == 2 => {
                // element, then the completer's error word.
                self.declare_await_bind(l, &l.binds[0], el.clone());
                self.declare_await_bind(l, &l.binds[1], Ty::Error);
            }
            _ if n == 1 => {
                self.declare_await_bind(l, &l.binds[0], el.clone());
            }
            _ => {
                let values = match &el {
                    Ty::Tuple(ts) => ts.len(),
                    _ => 2,
                };
                self.err(
                    format!("await destructures this future into {values} values, but {n} names are bound"),
                    l.value.span,
                );
                for b in &l.binds {
                    self.declare(&b.name, Ty::Unknown);
                }
            }
        }
    }

    /// Declares one binding produced by an await, honoring a type annotation,
    /// hardening a bare width, and registering mutability, ownership, and a
    /// pending error the same way an ordinary let binding does.
    fn declare_await_bind(&mut self, l: &Let, b: &Bind, pt: Ty) {
        let ty = match &b.ty {
            Some(t) => self.lower(t),
            None => harden(pt),
        };
        if l.mutable {
            self.declare_mut(&b.name);
        }
        if is_managed(&ty) {
            self.declare_own(&b.name, Own::Owner);
        }
        if matches!(ty, Ty::Error) {
            self.declare_err(&b.name, l.value.span);
        }
        self.declare(&b.name, ty);
    }

    /// Rejects the clear cases of a value escaping its frame through a return: a
    /// slice that views a frame local fixed array, and a closure that captures a
    /// local. A managed pointer escape is covered by the generation check, not
    /// here, since dusk has no address of operator and so every pointer is heap.
    /// The walk is driven by the declared return shape so an escaping fat value
    /// buried in a returned tuple is caught, not only a whole-return fat value.
    fn check_escape(&mut self, e: &Expr, t: &Ty) {
        let declared = self.cur_ret.clone();
        self.escape_walk(e, &declared, t);
    }

    /// Walks a returned value against its declared type, rejecting a fat member
    /// whose backing lives on this frame. A tuple recurses per position when the
    /// return expression is a tuple literal, so `return ([1,2,3], 42)` and
    /// `return (add, 7)` are checked member by member.
    fn escape_walk(&mut self, e: &Expr, declared: &Ty, inferred: &Ty) {
        match declared {
            // A slice that views a frame local array dangles once the function
            // returns and the stack array is reclaimed. A heap backed slice, like
            // a map result, or a slice parameter, whose backing the caller owns,
            // is fine.
            Ty::Slice(_) if self.slice_escapes(e, inferred) => {
                self.err(
                    "a slice into a local array escapes its frame; put the backing on the heap",
                    e.span,
                );
            }
            // A closure that captures a frame local keeps its environment on this
            // frame, so returning it, as a lambda literal or as a binding that
            // holds one, dangles. A closure with no captures is a plain function
            // pointer and may be returned.
            Ty::Func(..) if self.expr_is_local_closure(e) => {
                self.err(
                    "a closure that captures a local escapes its frame; it cannot be returned",
                    e.span,
                );
            }
            Ty::Tuple(ds) => {
                if let ExprKind::Tuple(es) = &e.kind {
                    // A tuple literal exposes its members; recurse per position.
                    if es.len() == ds.len() {
                        let members = match inferred {
                            Ty::Tuple(ts) if ts.len() == ds.len() => Some(ts),
                            _ => None,
                        };
                        for (i, (ei, di)) in es.iter().zip(ds).enumerate() {
                            let ii = members.and_then(|ts| ts.get(i)).unwrap_or(&Ty::Unknown);
                            self.escape_walk(ei, di, ii);
                        }
                    }
                } else {
                    // A tuple returned by a bare name, an alias, or a match carries
                    // no per-position expression, so the recorded escape flags of
                    // the returned value decide it. Codegen would still build the
                    // fat members on this frame, so an escaping one must be caught.
                    let (esc_s, esc_c) = self.value_escape(e);
                    if esc_s {
                        self.err(
                            "a slice into a local array escapes its frame; put the backing on the heap",
                            e.span,
                        );
                    } else if esc_c {
                        self.err(
                            "a closure that captures a local escapes its frame; it cannot be returned",
                            e.span,
                        );
                    }
                }
            }
            // A struct or enum returned by value carries its fat fields or payload.
            // A literal escapes when a field or payload initializer views a frame
            // local; a binding, alias, or match reflects the flags recorded on it.
            // Both are decided by value_escape, one shape over from the tuple case.
            Ty::Named(sname) if self.structs.contains_key(sname) || self.enums.contains_key(sname) => {
                let (esc_s, esc_c) = self.value_escape(e);
                if esc_s {
                    self.err(
                        "a slice into a local array escapes its frame; put the backing on the heap",
                        e.span,
                    );
                } else if esc_c {
                    self.err(
                        "a closure that captures a local escapes its frame; it cannot be returned",
                        e.span,
                    );
                }
            }
            // A fixed array returned by value copies its elements, so it escapes
            // only when the element type is a reference shape, a slice, closure,
            // tuple, or named aggregate, that views a frame local. A scalar array
            // is copied whole and never escapes. A literal recurses per element;
            // a binding or alias reflects its recorded flags.
            Ty::Array(elem, _)
                if matches!(**elem, Ty::Slice(_) | Ty::Func(..) | Ty::Tuple(_) | Ty::Named(_)) =>
            {
                if let ExprKind::Array(elems) = &e.kind {
                    for el in elems {
                        self.escape_walk(el, elem, &Ty::Unknown);
                    }
                } else {
                    let (esc_s, esc_c) = self.value_escape(e);
                    if esc_s {
                        self.err(
                            "a slice into a local array escapes its frame; put the backing on the heap",
                            e.span,
                        );
                    } else if esc_c {
                        self.err(
                            "a closure that captures a local escapes its frame; it cannot be returned",
                            e.span,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether returning a slice-typed expression would dangle: its backing is a
    /// frame local array. The sliced base or the returned array literal must have
    /// array type, or the returned name is a binding recorded as holding a local
    /// array slice; a slice parameter or a heap backed slice does not escape.
    fn slice_escapes(&mut self, e: &Expr, inferred: &Ty) -> bool {
        match &e.kind {
            ExprKind::Index(base, idx) if matches!(idx.kind, ExprKind::Range(..)) => {
                // A local array base, or a binding already known to hold a
                // frame-local slice, since re-slicing a frame-local slice stays
                // frame-local and its unannotated array literal infers to a slice.
                matches!(self.infer(base), Ty::Array(..))
                    || matches!(&base.kind, ExprKind::Ident(n) if self.is_esc_slice(n))
            }
            // An array literal materializes in this frame, so returning it as a
            // slice views a dead frame no matter what its type says.
            ExprKind::Array(_) => true,
            ExprKind::Ident(n) if self.is_esc_slice(n) => true,
            // A local-array return, or any other shape the leaf predicate catches:
            // a projection out of a local aggregate, or a match arm that projects
            // an escaping payload, all reach the bare slice return through here.
            _ => matches!(inferred, Ty::Array(..)) || self.value_escape(e).0,
        }
    }

    /// Whether a returned expression is a closure whose environment sits on this
    /// frame: a lambda literal that captures a local, a binding recorded as
    /// holding one, or a projection or match that yields such a closure.
    fn expr_is_local_closure(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Lambda(l) => self.lambda_captures_local(l),
            ExprKind::Ident(n) => self.is_esc_closure(n),
            _ => self.value_escape(e).1,
        }
    }

    /// The argument rule `spawn` and `submit` share: exactly one argument, a
    /// lambda literal, nullary and void, because only the literal site knows
    /// the environment layout that codegen heap copies for the task. Captures
    /// cross as heap copies, so a capture that may view this frame, a slice, a
    /// closure, or an interface value, even buried in a struct or enum field,
    /// is rejected. A captured managed pointer becomes a borrow inside the
    /// task, so the body can read through it but never free or move it, and a
    /// moved away pointer keeps its moved state so capturing it stays the
    /// error a plain lambda gets.
    fn check_task_lambda(&mut self, name: &str, args: &[Expr], span: Span) {
        if args.len() != 1 {
            self.err(
                format!("{name} takes one argument, a lambda literal of type () -> void"),
                span,
            );
            for a in args {
                self.infer(a);
            }
            return;
        }
        let ExprKind::Lambda(l) = &args[0].kind else {
            self.err(
                format!(
                    "{name} takes a lambda literal written at the call site; wrap a closure variable as lambda () -> void {{ g() }}"
                ),
                args[0].span,
            );
            self.infer(&args[0]);
            return;
        };
        if !l.params.is_empty() || !matches!(self.lower(&l.ret), Ty::Unit) {
            self.err(
                format!(
                    "a lambda passed to {name} takes no parameters and returns void; pass data by capture and results through a channel"
                ),
                args[0].span,
            );
        }
        let caps = self.spawn_lambda_captures(l);
        let mut borrowed = Vec::new();
        for c in &caps {
            let t = self.lookup(c);
            let mut seen = HashSet::new();
            if matches!(t, Ty::Future(_)) {
                // A future is loop-thread property; a completer thread carries
                // the raw handle words, not the typed future.
                self.err(
                    format!("{name} cannot capture '{c}': a future belongs to the event loop thread"),
                    args[0].span,
                );
            } else if !self.spawn_capturable(&t, &mut seen) || self.is_iface_bind(c) {
                self.err(
                    format!(
                        "{name} cannot capture '{c}'; a slice, closure, or interface value may view the spawning frame, so move the data to the heap or send it through a channel"
                    ),
                    args[0].span,
                );
            }
            if is_managed(&t) && !matches!(self.own_of(c), Some(Own::Moved)) {
                borrowed.push(c.clone());
            }
        }
        self.infer_lambda(l, args[0].span, &borrowed);
    }

    /// Whether a value of this type may cross a `spawn` as a heap copied
    /// capture. A slice, a closure, or an interface value may carry a pointer
    /// into the spawning frame that the copy would dangle, so they are rejected
    /// wherever they sit, including inside a struct or enum field. A pointer
    /// field is fine: every pointer targets the heap, where the generation
    /// check covers it. `seen` breaks recursive type cycles.
    fn spawn_capturable(&self, t: &Ty, seen: &mut HashSet<String>) -> bool {
        match t {
            // A future belongs to the event loop thread; a completer carries the
            // raw handle words instead, so a typed future is not a capturable
            // value.
            Ty::Future(_) => false,
            Ty::Slice(_) | Ty::Func(..) => false,
            Ty::Array(e, _) => self.spawn_capturable(e, seen),
            Ty::Tuple(ts) => ts.iter().all(|x| self.spawn_capturable(x, seen)),
            Ty::Named(n) => {
                if self.ifaces.contains(n) {
                    return false;
                }
                if !seen.insert(n.clone()) {
                    return true;
                }
                match self.embed_fields.get(n).cloned() {
                    Some(fs) => fs.iter().all(|f| self.spawn_capturable(f, seen)),
                    None => true,
                }
            }
            _ => true,
        }
    }

    /// The names a lambda captures from enclosing scopes, in first use order,
    /// for the spawn capture checks.
    fn spawn_lambda_captures(&self, l: &Lambda) -> Vec<String> {
        let mut used = Vec::new();
        let mut bound: HashSet<String> = l.params.iter().map(|p| p.name.clone()).collect();
        crate::parser::ast::collect_block(&l.body, &mut used, &mut bound);
        let mut seen = HashSet::new();
        used.into_iter()
            .filter(|n| {
                !bound.contains(n)
                    && self.scopes.iter().any(|s| s.contains_key(n))
                    && seen.insert(n.clone())
            })
            .collect()
    }

    /// Whether a lambda reads any variable bound in an enclosing scope, which
    /// makes its environment a frame local that must not escape.
    fn lambda_captures_local(&self, l: &Lambda) -> bool {
        let mut used = Vec::new();
        let mut bound: HashSet<String> = l.params.iter().map(|p| p.name.clone()).collect();
        crate::parser::ast::collect_block(&l.body, &mut used, &mut bound);
        used.iter()
            .any(|n| !bound.contains(n) && self.scopes.iter().any(|s| s.contains_key(n)))
    }

    fn infer(&mut self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Int(v, suffix) => {
                let w = int_suffix_width(suffix);
                if w != 0 {
                    self.check_int_fits_suffixed(*v, w, e.span);
                }
                Ty::Int(w)
            }
            ExprKind::Float(_, suffix) => Ty::Float(float_suffix_width(suffix)),
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Char(_) => Ty::Char,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Ident(name) => {
                if matches!(self.own_of(name), Some(Own::Moved)) {
                    self.err("use of a moved pointer", e.span);
                }
                // An async function's name in value position cannot be stored or
                // passed, only called. A direct call resolves its callee without
                // this arm, so a plain call never trips it; a local of the same
                // name shadows the function.
                if self.async_fns.contains_key(name) && !self.is_local(name) {
                    self.err(
                        format!("'{name}' is async; call it with await or start it with async_run"),
                        e.span,
                    );
                }
                self.lookup(name)
            }
            ExprKind::Unary(op, x) => {
                let t = self.infer(x);
                self.check_unary(*op, &t, e.span)
            }
            ExprKind::Binary(op, a, b) => {
                let ta = self.infer(a);
                let tb = self.infer(b);
                let result = self.check_binary(*op, &ta, &tb, e.span);
                match op {
                    BinOp::Shl | BinOp::Shr => self.check_shift_amount(b, &result, e.span),
                    BinOp::Pow => self.check_pow_exponent(&ta, &tb, b, e.span),
                    _ => {}
                }
                result
            }
            ExprKind::Call(f, args) => self.infer_call(f, args),
            ExprKind::Field(x, name) => {
                // A fixed array exposes only `.len`, the int64 count a slice's
                // `.len` also yields. Any other field on an array is rejected here
                // so it is a clear error, not a silent zero from codegen. Every
                // other base stays permissive and resolves its field in codegen.
                let tx = self.infer(x);
                if let Ty::Array(..) = tx {
                    if name == "len" {
                        return Ty::Int(64);
                    }
                    self.err(
                        format!("a fixed array has no field '{name}'; only '.len' is available"),
                        e.span,
                    );
                }
                Ty::Unknown
            }
            ExprKind::Index(x, i) => {
                let tx = self.infer(x);
                self.infer(i);
                if matches!(i.kind, ExprKind::Range(..)) {
                    Ty::Slice(Box::new(elem_of(&tx)))
                } else {
                    elem_of(&tx)
                }
            }
            ExprKind::Range(a, b, _) => {
                self.infer(a);
                self.infer(b);
                Ty::Unknown
            }
            ExprKind::Tuple(xs) => Ty::Tuple(xs.iter().map(|x| self.infer(x)).collect()),
            ExprKind::Array(xs) => {
                let mut elem = Ty::Unknown;
                for x in xs {
                    elem = self.infer(x);
                }
                Ty::Slice(Box::new(elem))
            }
            ExprKind::StructLit(name, fields) => {
                self.check_struct_lit(name, fields, e.span);
                if name == "Future" {
                    // The future struct's words do not carry its element type, so
                    // a literal infers as a future of an unknown element, which
                    // the compatibility rule accepts against any Future<T>.
                    Ty::Future(Box::new(Ty::Unknown))
                } else {
                    named_ty(name)
                }
            }
            ExprKind::Lambda(l) => self.infer_lambda(l, e.span, &[]),
            ExprKind::Match(m) => {
                self.walk_match(m);
                Ty::Unknown
            }
            ExprKind::Do(_, binds) => {
                for b in binds {
                    self.infer(&b.expr);
                }
                Ty::Unknown
            }
            // An await reached through plain inference is buried mid-expression;
            // the sanctioned statement positions intercept it before this arm, so
            // anything landing here is a misuse. This is the backstop for the
            // parser's own rejection, catching an await a rewrite might smuggle in.
            ExprKind::Await(op, _) => {
                self.infer(op);
                self.err(
                    "'await' cannot appear mid-expression; give the awaited value a name, as in v, e := await f",
                    e.span,
                );
                Ty::Unknown
            }
            ExprKind::SizeofType(_) => Ty::Int(64),
        }
    }

    /// Rejects a suffixed literal whose value cannot fit its own suffix, like
    /// `300i8`, which would silently truncate in codegen.
    fn check_int_fits_suffixed(&mut self, v: i64, w: u32, span: Span) {
        if w >= 64 {
            return;
        }
        let val = v as i128;
        let lo = -(1i128 << (w - 1));
        let hi = (1i128 << w) - 1;
        if val < lo || val > hi {
            self.err(format!("literal {val} does not fit in {w} bits"), span);
        }
    }

    fn infer_call(&mut self, f: &Expr, args: &[Expr]) -> Ty {
        // Method call syntax. The builtin `error` methods have known types; every
        // other method call stays permissive and returns Unknown for now.
        if let ExprKind::Field(base, mname) = &f.kind {
            let is_error = matches!(self.infer(base), Ty::Error);
            let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer(a)).collect();
            // Enum constructor covariance: a variant payload of slice-of-interface
            // type must not receive a slice of concrete structs reinterpreted.
            if let ExprKind::Ident(en) = &base.kind {
                if self.enums.contains_key(en) {
                    if let Some(payloads) = self.variant_payloads.get(mname).cloned() {
                        for (i, (pty, aty)) in payloads.iter().zip(&arg_tys).enumerate() {
                            if let Some(arg) = args.get(i) {
                                self.check_slice_covariance(pty, aty, arg, arg.span);
                            }
                        }
                    }
                }
            }
            if is_error {
                // exists, check, and ignore are the three ways to handle an
                // error, so a bound error the call is invoked on is discharged.
                if matches!(mname.as_str(), "exists" | "check" | "ignore") {
                    if let ExprKind::Ident(bname) = &base.kind {
                        self.mark_err_handled(bname);
                    }
                }
                return match mname.as_str() {
                    "exists" => Ty::Bool,
                    "toString" => Ty::Str,
                    "check" | "ignore" => Ty::Unit,
                    _ => Ty::Unknown,
                };
            }
            return Ty::Unknown;
        }
        // A builtin with a known return type, unless a user function of the same
        // name shadows it, in which case the normal signature path wins.
        if let ExprKind::Ident(name) = &f.kind {
            if !self.sigs.contains_key(name) {
                if let Some(ty) = builtin_ret(name) {
                    for a in args {
                        self.infer(a);
                    }
                    return ty;
                }
                // print, println, and printerr take an optional format string. A
                // string literal first argument is always a format string, so a
                // stray hole or a doubled brace behaves the same at any arity,
                // and every printed value must have a printable type.
                if name == "print" || name == "println" || name == "printerr" {
                    let tys: Vec<Ty> = args.iter().map(|a| self.infer(a)).collect();
                    let fmt_lit = matches!(args.first().map(|a| &a.kind), Some(ExprKind::Str(_)));
                    if fmt_lit {
                        self.check_format(args);
                    }
                    let value_start = if fmt_lit { 1 } else { 0 };
                    for (a, t) in args.iter().zip(&tys).skip(value_start) {
                        self.check_printable(t, a.span);
                    }
                    return Ty::Unit;
                }
                if name == "spawn" {
                    self.check_task_lambda("spawn", args, f.span);
                    return Ty::Tuple(vec![Ty::Thread, Ty::Error]);
                }
                if name == "submit" {
                    // A pool task is a thread body that runs later, so submit
                    // shares spawn's whole argument rule. It returns only an
                    // error: the pool owns the task and results flow through a
                    // channel, never a handle.
                    self.check_task_lambda("submit", args, f.span);
                    return Ty::Error;
                }
                if name == "join" {
                    let t = args.first().map(|a| self.infer(a)).unwrap_or(Ty::Unknown);
                    if args.len() != 1 || !compatible(&Ty::Thread, &t) {
                        self.err("join takes one thread handle", f.span);
                    }
                    return Ty::Error;
                }
                if name == "async_run" {
                    return self.infer_async_run(args, f.span);
                }
                if name == "alloc" {
                    // alloc(v) yields a managed *T owner of the value's type, so
                    // the single owner pass engages even for the inferred form.
                    // The zero argument form sizes from a pointer annotation and
                    // is only valid where let_stmt intercepted it.
                    if args.is_empty() {
                        self.err(
                            "alloc() with no value is only valid as p: *T = alloc(), where the annotation sizes the allocation",
                            f.span,
                        );
                        return Ty::Ptr(Box::new(Ty::Unknown));
                    }
                    let inner = args.first().map(|a| self.infer(a)).unwrap_or(Ty::Unknown);
                    return Ty::Ptr(Box::new(inner));
                }
                if name == "ptr_add" {
                    // ptr_add is raw byte arithmetic over the raw pointer layer.
                    // A managed *T is a fat value, not a raw operand, so reject it
                    // and point at *raw T. A *void, a pointer to Unit, stays raw.
                    let pt = args.first().map(|a| self.infer(a)).unwrap_or(Ty::Unknown);
                    if matches!(&pt, Ty::Ptr(inner) if !matches!(&**inner, Ty::Unit)) {
                        self.err(
                            "ptr_add takes a raw pointer; use *raw T or *void, not a managed *T",
                            f.span,
                        );
                    }
                    for a in args.iter().skip(1) {
                        self.infer(a);
                    }
                    return pt;
                }
                if name == "move" {
                    // move(x) transfers ownership; its value and type are x's, and
                    // the source binding is invalidated so a later use is rejected.
                    let t = args.first().map(|a| self.infer(a)).unwrap_or(Ty::Unknown);
                    if let Some(a) = args.first() {
                        if let ExprKind::Ident(src) = &a.kind {
                            if matches!(self.own_of(src), Some(Own::Borrow)) {
                                self.err(
                                    "cannot move a borrowed pointer; only its owner can be moved",
                                    a.span,
                                );
                            }
                            self.set_moved(src);
                        }
                    }
                    return t;
                }
                if name == "free" {
                    // Only an owner frees. Freeing a borrow, a ref alias or a
                    // pointer parameter, is rejected; its owner frees it instead.
                    if let Some(a) = args.first() {
                        let t = self.infer(a);
                        if is_managed(&t) {
                            if let ExprKind::Ident(p) = &a.kind {
                                if matches!(self.own_of(p), Some(Own::Borrow)) {
                                    self.err(
                                        "cannot free a borrowed pointer; only its owner frees it",
                                        a.span,
                                    );
                                }
                            }
                        }
                    }
                    return Ty::Unit;
                }
            }
        }
        // A bare function name is resolved from the signature table directly, not
        // through the value-position inference of its ident, so an async function
        // called at the call site types as its future instead of tripping the
        // guard that forbids using its name as a value.
        let callee = match &f.kind {
            ExprKind::Ident(name) if self.sigs.contains_key(name) && !self.is_local(name) => {
                self.lookup(name)
            }
            _ => self.infer(f),
        };
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer(a)).collect();
        if let Ty::Func(params, ret) = callee {
            if params.len() != arg_tys.len() {
                self.err(
                    format!("expected {} argument(s), found {}", params.len(), arg_tys.len()),
                    f.span,
                );
            } else {
                for (i, (p, a)) in params.iter().zip(&arg_tys).enumerate() {
                    if !compatible(p, a) {
                        self.err(format!("argument {} has the wrong type", i + 1), args[i].span);
                    }
                }
                // The fixed parameter types lower an interface to Unknown, so they
                // accept any value. The unfixed signature keeps the interface name,
                // so a concrete struct passed for an interface is checked here.
                if let ExprKind::Ident(name) = &f.kind {
                    if let Some(raw) = self.raw_sigs.get(name).cloned() {
                        for (i, (rp, a)) in raw.iter().zip(&arg_tys).enumerate() {
                            self.check_conformance(rp, a, f.span);
                            // Reject a concrete struct where a tuple member is an
                            // interface, the same as the return position, since
                            // boxing inside a tuple is not supported.
                            let span = args.get(i).map(|x| x.span).unwrap_or(f.span);
                            self.tuple_iface_mismatch(rp, a, span);
                            // Reject reinterpreting a slice of concrete structs as a
                            // slice of interfaces.
                            if let Some(arg) = args.get(i) {
                                self.check_slice_covariance(rp, a, arg, span);
                            }
                        }
                    }
                }
            }
            return *ret;
        }
        Ty::Unknown
    }

    /// Checks `async_run(g(args))`, the only sync-to-async bridge. It cranks the
    /// event loop until a directly-called async func's future completes, then
    /// yields its result, so it is illegal inside an async body and its argument
    /// must be a literal call of an async func, never a stored future.
    fn infer_async_run(&mut self, args: &[Expr], span: Span) -> Ty {
        if self.in_async {
            self.err(
                "async_run cannot be called inside an async func; await the call instead",
                span,
            );
        }
        // Infer every argument so nested errors surface and inner types check.
        for a in args {
            self.infer(a);
        }
        let g = match args.first().map(|a| &a.kind) {
            Some(ExprKind::Call(callee, _)) => match &callee.kind {
                ExprKind::Ident(g) if self.async_fns.contains_key(g) && !self.is_local(g) => {
                    Some(g.clone())
                }
                _ => None,
            },
            _ => None,
        };
        match g {
            Some(g) if args.len() == 1 => self.async_fns.get(&g).cloned().unwrap_or(Ty::Unknown),
            _ => {
                self.err(
                    "async_run takes a direct call of an async func, written at the call site",
                    span,
                );
                Ty::Unknown
            }
        }
    }

    /// Rejects passing a concrete struct where an interface is expected unless the
    /// struct implements it. Only fires when both sides are known by name, an
    /// interface expected and a concrete struct given, so an Unknown or a generic
    /// stays permissive.
    fn check_conformance(&mut self, expected: &Ty, actual: &Ty, span: Span) {
        if let (Ty::Named(iface), Ty::Named(concrete)) = (expected, actual) {
            if self.ifaces.contains(iface)
                && !self.ifaces.contains(concrete)
                && self.structs.contains_key(concrete)
                && !self.impls.contains(&(iface.clone(), concrete.clone()))
            {
                self.err(
                    format!("'{concrete}' does not implement interface '{iface}'; add an 'impl {iface} for {concrete}'"),
                    span,
                );
            }
        }
    }

    /// Rejects a concrete struct value where a tuple member is an interface type.
    /// Boxing a struct to an interface value works at a whole value position but
    /// not inside a tuple, so it is refused consistently in both return and
    /// argument positions rather than accepted at one and miscompiled. Returns
    /// whether any such member was found, so a caller can skip the generic
    /// mismatch error it would otherwise also emit. `expected` must be the unfixed
    /// type, so the interface name survives; the fixed table erases it to Unknown.
    fn tuple_iface_mismatch(&mut self, expected: &Ty, actual: &Ty, span: Span) -> bool {
        let (Ty::Tuple(es), Ty::Tuple(as_)) = (expected, actual) else {
            return false;
        };
        if es.len() != as_.len() {
            return false;
        }
        let mut found = false;
        for (ex, ac) in es.iter().zip(as_) {
            if matches!(ex, Ty::Tuple(_)) {
                found |= self.tuple_iface_mismatch(ex, ac, span);
            } else if let Ty::Named(iface) = ex {
                if self.ifaces.contains(iface)
                    && matches!(ac, Ty::Named(c) if self.structs.contains_key(c))
                {
                    self.err(
                        "an interface value inside a tuple is not supported; return or pass the concrete type, or box it outside the tuple",
                        span,
                    );
                    found = true;
                }
            }
        }
        found
    }

    /// Rejects passing an existing slice of concrete structs where a slice of an
    /// interface is expected, a covariant reinterpretation that shares the same
    /// LLVM shape but reads each element as a boxed interface, silently corrupting
    /// memory. An array literal of structs is exempt: it is coerced element by
    /// element, boxing each, not reinterpreted. `expected` must be the unfixed
    /// type, so the interface element name survives the fix that erases it.
    fn check_slice_covariance(&mut self, expected: &Ty, actual: &Ty, value: &Expr, span: Span) {
        let (Ty::Slice(exp_elem), Ty::Slice(act_elem)) = (expected, actual) else {
            return;
        };
        let Ty::Named(iface) = &**exp_elem else {
            return;
        };
        if !self.ifaces.contains(iface) {
            return;
        }
        // An array literal boxes each element as it coerces, so only a slice value
        // reinterpreted whole is unsound.
        if matches!(&value.kind, ExprKind::Array(_)) {
            return;
        }
        if matches!(&**act_elem, Ty::Named(c) if self.structs.contains_key(c) && !self.ifaces.contains(c)) {
            let concrete = match &**act_elem {
                Ty::Named(c) => c.as_str(),
                _ => "",
            };
            self.err(
                format!("cannot pass a slice of '{concrete}' as a slice of interface '{iface}'; a slice of concrete values cannot be reinterpreted as a slice of interfaces"),
                span,
            );
        }
    }

    /// Checks that an `impl I for T` provides every method the interface requires,
    /// each with the same parameter count, so an incomplete impl is rejected before
    /// codegen emits an undefined vtable.
    fn check_impl_complete(&mut self, im: &Impl) {
        let Some(iface) = im.iface.clone() else {
            return;
        };
        let Some(required) = self.iface_methods.get(&iface).cloned() else {
            return;
        };
        for (mname, arity) in &required {
            match im.methods.iter().find(|m| &m.name == mname) {
                None => self.err(
                    format!("impl {iface} for {} is missing method '{mname}'", im.ty),
                    im.span,
                ),
                Some(m) if m.params.len() != *arity => self.err(
                    format!(
                        "method '{mname}' in impl {iface} for {} has {} parameter(s), the interface requires {arity}",
                        im.ty,
                        m.params.len()
                    ),
                    m.span,
                ),
                Some(_) => {}
            }
        }
    }

    /// Checks a formatted `print` or `println`. The first argument must be a
    /// string literal, and its hole count must match the number of value
    /// arguments, so codegen can expand the call with no runtime format parser.
    fn check_format(&mut self, args: &[Expr]) {
        let ExprKind::Str(fmt) = &args[0].kind else {
            self.err(
                "a formatted print needs a string literal as its first argument",
                args[0].span,
            );
            return;
        };
        match crate::fmt::parse(fmt) {
            Ok(segs) => {
                let holes = crate::fmt::holes(&segs);
                let given = args.len() - 1;
                if holes != given {
                    self.err(
                        format!("format string has {holes} hole(s) but {given} argument(s) were given"),
                        args[0].span,
                    );
                }
            }
            Err(msg) => self.err(msg, args[0].span),
        }
    }

    /// Whether a value of this type can be printed. Scalars, strings, and errors
    /// have printers; a struct prints through its Display impl's toString; and
    /// everything else is rejected, since a silently empty print is the worst of
    /// the options.
    fn check_printable(&mut self, t: &Ty, span: Span) {
        match t {
            Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char | Ty::Str | Ty::Error | Ty::Unknown => {}
            Ty::Named(n) => {
                if self.structs.contains_key(n) {
                    if !self.impls.contains(&("Display".to_string(), n.clone())) {
                        self.err(
                            format!("'{n}' has no Display impl; add 'impl Display for {n}' with 'toString() -> string' to print it"),
                            span,
                        );
                    }
                } else if self.enums.contains_key(n) {
                    self.err(
                        format!("cannot print the enum '{n}'; match on it and print its parts"),
                        span,
                    );
                }
                // An unknown named type, a generic or an import the checker does
                // not track, stays permissive.
            }
            Ty::Unit => self.err("cannot print a void value", span),
            Ty::Thread => self.err("cannot print a thread handle; join it instead", span),
            Ty::Ptr(_) | Ty::RawPtr(_) => {
                self.err("cannot print a pointer; dereference it or print its fields", span)
            }
            _ => self.err(
                format!("cannot print {}; print its elements instead", ty_str(t)),
                span,
            ),
        }
    }

    /// Checks a lambda body. `borrowed` marks captured managed pointers as
    /// borrows inside the body, used by `spawn` so a thread cannot free or move
    /// a pointer its spawner still owns.
    fn infer_lambda(&mut self, l: &Lambda, span: Span, borrowed: &[String]) -> Ty {
        let saved_ret = self.cur_ret.clone();
        let saved_depth = self.branch_depth;
        self.cur_ret = self.lower(&l.ret);
        self.branch_depth = 0;
        let params: Vec<Ty> = l.params.iter().map(|p| self.lower(&p.ty)).collect();
        self.push_scope();
        for (p, ty) in l.params.iter().zip(&params) {
            self.declare(&p.name, ty.clone());
        }
        for name in borrowed {
            self.declare_own(name, Own::Borrow);
        }
        for s in &l.body.stmts {
            self.stmt(s);
        }
        self.pop_scope();
        let ret = self.cur_ret.clone();
        if !matches!(ret, Ty::Unit) && !block_returns(&l.body) {
            self.err("not all paths in this lambda return a value", span);
        }
        self.cur_ret = saved_ret;
        self.branch_depth = saved_depth;
        Ty::Func(params, Box::new(ret))
    }

    fn walk_match(&mut self, m: &Match) {
        let st = self.infer(&m.scrut);
        self.branch_depth += 1;
        for arm in &m.arms {
            self.push_scope();
            match &arm.pat {
                Pattern::Variant(_, binds) => {
                    for b in binds {
                        self.declare(b, Ty::Unknown);
                    }
                }
                Pattern::Ident(name) => self.declare(name, Ty::Unknown),
                Pattern::Wildcard => {}
            }
            for s in &arm.body.stmts {
                self.stmt(s);
            }
            self.pop_scope();
        }
        self.branch_depth -= 1;
        // match is defined over enum values. A known enum checks coverage; any
        // other known type is rejected, since codegen has no dispatch for it. An
        // Unknown, a generic or an unresolved import, stays permissive.
        match &st {
            Ty::Named(ename) => {
                if let Some(variants) = self.enums.get(ename).cloned() {
                    self.check_coverage(&variants, &m.arms, m.scrut.span);
                } else if self.structs.contains_key(ename) || self.ifaces.contains(ename) {
                    self.err(
                        format!("match needs an enum value, and {} is not an enum", ty_str(&st)),
                        m.scrut.span,
                    );
                }
            }
            Ty::Unknown => {}
            _ => self.err(
                format!("match needs an enum value, not {}", ty_str(&st)),
                m.scrut.span,
            ),
        }
    }

    /// Checks a match over an enum is exhaustive and free of unreachable arms. A
    /// wildcard or a plain identifier that is not a variant is a catch all.
    fn check_coverage(&mut self, variants: &[String], arms: &[Arm], span: Span) {
        let mut covered: HashSet<String> = HashSet::new();
        let mut catch_all = false;
        for arm in arms {
            if catch_all {
                self.err("unreachable match arm after a catch all pattern", span);
                continue;
            }
            match &arm.pat {
                Pattern::Wildcard => catch_all = true,
                Pattern::Ident(n) if !variants.contains(n) => catch_all = true,
                Pattern::Ident(n) | Pattern::Variant(n, _) => {
                    if !covered.insert(n.clone()) {
                        self.err(format!("unreachable match arm, '{n}' is already covered"), span);
                    }
                }
            }
        }
        if !catch_all {
            for v in variants {
                if !covered.contains(v) {
                    self.err(format!("non exhaustive match, missing variant '{v}'"), span);
                }
            }
        }
    }

    fn check_unary(&mut self, op: UnOp, t: &Ty, span: Span) -> Ty {
        match op {
            UnOp::Deref => match t {
                Ty::Ptr(inner) | Ty::RawPtr(inner) => (**inner).clone(),
                Ty::Unknown => Ty::Unknown,
                _ => {
                    self.err("cannot dereference a non pointer value", span);
                    Ty::Unknown
                }
            },
            UnOp::Neg => {
                if matches!(t, Ty::Int(_) | Ty::Float(_) | Ty::Unknown) {
                    t.clone()
                } else {
                    self.err("unary minus needs a numeric operand", span);
                    Ty::Unknown
                }
            }
            UnOp::Not => {
                if matches!(t, Ty::Bool | Ty::Unknown) {
                    Ty::Bool
                } else {
                    self.err("'!' needs a bool operand", span);
                    Ty::Bool
                }
            }
            UnOp::BitNot => {
                if matches!(t, Ty::Int(_) | Ty::Unknown) {
                    t.clone()
                } else {
                    self.err("'~' needs an integer operand", span);
                    Ty::Unknown
                }
            }
        }
    }

    fn check_binary(&mut self, op: BinOp, a: &Ty, b: &Ty, span: Span) -> Ty {
        use BinOp::*;
        let unknown = matches!(a, Ty::Unknown) || matches!(b, Ty::Unknown);
        match op {
            Add | Sub | Mul | Div | Mod => {
                if unknown {
                    return if matches!(a, Ty::Unknown) { b.clone() } else { a.clone() };
                }
                match (a, b) {
                    // Same kind: the widths must agree, with a bare literal
                    // (width 0) adapting to the other side. Mixing widths would
                    // silently truncate in codegen, so it is an error here.
                    (Ty::Int(x), Ty::Int(y)) | (Ty::Float(x), Ty::Float(y)) => {
                        if x == y || *x == 0 || *y == 0 {
                            let w = (*x).max(*y);
                            if matches!(a, Ty::Int(_)) { Ty::Int(w) } else { Ty::Float(w) }
                        } else {
                            self.err(
                                format!(
                                    "arithmetic mixes {} and {}; match the widths",
                                    ty_str(a),
                                    ty_str(b)
                                ),
                                span,
                            );
                            Ty::Unknown
                        }
                    }
                    _ => {
                        self.err("arithmetic needs two operands of the same numeric type", span);
                        Ty::Unknown
                    }
                }
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                if !compatible(a, b) {
                    self.err("comparison needs two operands of the same type", span);
                }
                Ty::Bool
            }
            And | Or => {
                if !(unknown || matches!(a, Ty::Bool) && matches!(b, Ty::Bool)) {
                    self.err("logical operators need bool operands", span);
                }
                Ty::Bool
            }
            BitAnd | BitOr | BitXor | Shl | Shr => {
                if unknown {
                    return if matches!(a, Ty::Unknown) { b.clone() } else { a.clone() };
                }
                match (a, b) {
                    // Bitwise and shift operators take integers only, both the
                    // same width with a width 0 literal adapting, exactly the
                    // arithmetic rule. Mixing widths would silently truncate.
                    (Ty::Int(x), Ty::Int(y)) => {
                        if x == y || *x == 0 || *y == 0 {
                            Ty::Int((*x).max(*y))
                        } else {
                            self.err(
                                format!(
                                    "'{}' mixes {} and {}; match the widths",
                                    binop_spelling(op),
                                    ty_str(a),
                                    ty_str(b)
                                ),
                                span,
                            );
                            Ty::Unknown
                        }
                    }
                    _ => {
                        let msg = if matches!(op, Shl | Shr) {
                            "shift operators need integer operands"
                        } else {
                            "bitwise operators need integer operands"
                        };
                        self.err(msg, span);
                        Ty::Unknown
                    }
                }
            }
            Pow => {
                if unknown {
                    return if matches!(a, Ty::Unknown) { b.clone() } else { a.clone() };
                }
                match (a, b) {
                    // Both integer, same width (a literal adapting), wraps like the
                    // bare `mul`; or both float, same width, the `llvm.pow` path.
                    (Ty::Int(x), Ty::Int(y)) | (Ty::Float(x), Ty::Float(y)) => {
                        if x == y || *x == 0 || *y == 0 {
                            let w = (*x).max(*y);
                            if matches!(a, Ty::Int(_)) { Ty::Int(w) } else { Ty::Float(w) }
                        } else {
                            self.err(
                                format!(
                                    "'**' mixes {} and {}; match the widths",
                                    ty_str(a),
                                    ty_str(b)
                                ),
                                span,
                            );
                            Ty::Unknown
                        }
                    }
                    _ => {
                        self.err("'**' needs two operands of the same numeric type", span);
                        Ty::Unknown
                    }
                }
            }
        }
    }

    /// Rejects a constant negative exponent on integer `**`. A dynamic negative
    /// exponent faults at runtime; a float exponent may be negative.
    fn check_pow_exponent(&mut self, base: &Ty, exp: &Ty, exp_expr: &Expr, span: Span) {
        if !matches!(base, Ty::Int(_)) || !matches!(exp, Ty::Int(_)) {
            return;
        }
        if let Some(v) = const_int(exp_expr) {
            if v < 0 {
                self.err("'**' on integers needs a nonnegative exponent", span);
            }
        }
    }

    /// Rejects a constant shift amount outside `[0, width)`. A width 0 adaptable
    /// literal result counts as int64. A dynamic amount is guarded at runtime.
    fn check_shift_amount(&mut self, amount: &Expr, result: &Ty, span: Span) {
        let Some(v) = const_int(amount) else {
            return;
        };
        if v < 0 {
            self.err("shift amount is negative", span);
            return;
        }
        let w = match result {
            Ty::Int(0) => 64,
            Ty::Int(w) => *w as i128,
            _ => return,
        };
        if v >= w {
            self.err(format!("shift amount {v} is out of range for int{w}"), span);
        }
    }
}

/// The source spelling of a binary operator, for diagnostics that name the fix.
fn binop_spelling(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::Pow => "**",
    }
}

/// The signed value of a constant integer literal, including a negated literal,
/// or None for any computed expression. Used by the shift range check.
fn const_int(e: &Expr) -> Option<i128> {
    match &e.kind {
        ExprKind::Int(v, _) => Some(*v as i128),
        ExprKind::Unary(UnOp::Neg, inner) => match &inner.kind {
            ExprKind::Int(v, _) => Some(-(*v as i128)),
            _ => None,
        },
        _ => None,
    }
}

fn elem_of(t: &Ty) -> Ty {
    match t {
        Ty::Slice(e) | Ty::Array(e, _) => (**e).clone(),
        Ty::Str => Ty::Char,
        _ => Ty::Unknown,
    }
}

/// Whether an expression is a call to the `alloc` builtin with no arguments,
/// the uninitialized allocation form. A user function named alloc shadows the
/// builtin, so the signature table is consulted.
fn is_zero_arg_alloc(e: &Expr, sigs: &HashMap<String, (Vec<Ty>, Ty)>) -> bool {
    match &e.kind {
        ExprKind::Call(f, args) if args.is_empty() => {
            matches!(&f.kind, ExprKind::Ident(n) if n == "alloc" && !sigs.contains_key(n))
        }
        _ => false,
    }
}

/// Whether every path through a block reaches a `return`. A block returns when
/// any statement in it does: a plain return, an if whose branches both return,
/// or a match whose arms all return (exhaustiveness is checked separately).
/// Loops never count, since their bodies may run zero times.
fn block_returns(b: &Block) -> bool {
    b.stmts.iter().any(stmt_returns)
}

fn stmt_returns(s: &Stmt) -> bool {
    match s {
        Stmt::Return(_) => true,
        Stmt::If(i) => match &i.els {
            Some(els) => block_returns(&i.then) && block_returns(els),
            None => false,
        },
        Stmt::Match(m) => !m.arms.is_empty() && m.arms.iter().all(|a| block_returns(&a.body)),
        _ => false,
    }
}

/// Whether a type is a managed pointer subject to the single owner rules. A
/// `*void`, which is `Ty::Ptr(Unit)`, is the raw allocator currency and is
/// exempt, as is every `*raw T`.
fn is_managed(ty: &Ty) -> bool {
    matches!(ty, Ty::Ptr(inner) if !matches!(&**inner, Ty::Unit))
}

/// The types a foreign C signature may use. The boundary is the raw pointer
/// layer, so a scalar, a `*raw T`, or a `*void` crosses, and `void` is a valid
/// return. A managed `*T` and an aggregate passed by value do not cross.
fn foreign_ty_ok(ty: &Ty) -> bool {
    match ty {
        Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char | Ty::Unit => true,
        Ty::RawPtr(_) => true,
        Ty::Ptr(inner) => matches!(**inner, Ty::Unit),
        _ => false,
    }
}

/// Whether a type carries an error, directly or as a component of a tuple, so a
/// bare statement of this type would drop an error that must be handled.
fn ty_has_error(t: &Ty) -> bool {
    match t {
        Ty::Error => true,
        Ty::Tuple(ts) => ts.iter().any(ty_has_error),
        _ => false,
    }
}

fn compatible(a: &Ty, b: &Ty) -> bool {
    if a == b || matches!(a, Ty::Unknown) || matches!(b, Ty::Unknown) {
        return true;
    }
    match (a, b) {
        // A width of 0 is a bare literal, which adapts to any width of its kind.
        // Two concrete widths must match, so an int32 never silently truncates
        // an int64 or widens into one.
        (Ty::Int(x), Ty::Int(y)) | (Ty::Float(x), Ty::Float(y)) => {
            *x == 0 || *y == 0 || x == y
        }
        (Ty::Array(x, _), Ty::Slice(y)) | (Ty::Slice(x), Ty::Array(y, _)) => compatible(x, y),
        (Ty::Slice(x), Ty::Slice(y)) => compatible(x, y),
        (Ty::Array(x, n), Ty::Array(y, m)) if n == m => compatible(x, y),
        (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => {
            xs.iter().zip(ys).all(|(x, y)| compatible(x, y))
        }
        (Ty::Func(xp, xr), Ty::Func(yp, yr)) if xp.len() == yp.len() => {
            xp.iter().zip(yp).all(|(x, y)| compatible(x, y)) && compatible(xr, yr)
        }
        (Ty::RawPtr(x), Ty::RawPtr(y)) => compatible(x, y),
        // Two futures agree when their elements agree; an Unknown element, from
        // a generic Future<T> or the element-erased struct literal, wildcards.
        (Ty::Future(x), Ty::Future(y)) => {
            matches!(**x, Ty::Unknown) || matches!(**y, Ty::Unknown) || compatible(x, y)
        }
        (Ty::Ptr(x), Ty::Ptr(y)) => {
            matches!(**x, Ty::Unit) || matches!(**y, Ty::Unit) || compatible(x, y)
        }
        // *void is the erased currency of the allocator and FFI layer, a thin
        // untracked pointer, so any *raw T passes where *void is expected;
        // codegen already lowers *void to the same bare word. The reverse
        // direction stays an error: a *void that could become a typed *raw
        // would let a managed pointer launder through *void into a
        // dereferenceable alias the generation check cannot see.
        (Ty::Ptr(u), Ty::RawPtr(_)) => matches!(**u, Ty::Unit),
        // A char is an ASCII byte, freely compared with and assigned to ints.
        (Ty::Char, Ty::Int(_)) | (Ty::Int(_), Ty::Char) => true,
        _ => false,
    }
}

fn lower(t: &Type, generics: &HashSet<String>) -> Ty {
    match t {
        // The one-shot future carries its element type through the checker even
        // though the runtime struct erases it; a generic element becomes Unknown,
        // which the compatibility rule wildcards.
        Type::Named(n, args) if n == "Future" && args.len() == 1 => {
            Ty::Future(Box::new(lower(&args[0], generics)))
        }
        Type::Named(n, _) => {
            if generics.contains(n) {
                Ty::Unknown
            } else {
                named_ty(n)
            }
        }
        Type::Ptr(b) => Ty::Ptr(Box::new(lower(b, generics))),
        Type::RawPtr(b) => Ty::RawPtr(Box::new(lower(b, generics))),
        Type::Slice(b) => Ty::Slice(Box::new(lower(b, generics))),
        Type::Array(b, n) => Ty::Array(Box::new(lower(b, generics)), *n),
        Type::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| lower(t, generics)).collect()),
        Type::Func(ps, r) => Ty::Func(
            ps.iter().map(|t| lower(t, generics)).collect(),
            Box::new(lower(r, generics)),
        ),
        Type::Unit => Ty::Unit,
    }
}

/// Pins a bare literal's type to its default width, int64 or float64, when it
/// lands in an unannotated binding, matching how codegen sizes the slot.
fn harden(t: Ty) -> Ty {
    match t {
        Ty::Int(0) => Ty::Int(64),
        Ty::Float(0) => Ty::Float(64),
        Ty::Tuple(ts) => Ty::Tuple(ts.into_iter().map(harden).collect()),
        other => other,
    }
}

/// The width an integer literal suffix pins, or 0 for a bare adaptable literal.
fn int_suffix_width(suffix: &Option<String>) -> u32 {
    match suffix.as_deref() {
        Some("i8") | Some("u8") => 8,
        Some("i16") | Some("u16") => 16,
        Some("i32") | Some("u32") => 32,
        Some("i64") | Some("u64") => 64,
        _ => 0,
    }
}

/// The width a float literal suffix pins, or 0 for a bare adaptable literal.
fn float_suffix_width(suffix: &Option<String>) -> u32 {
    match suffix.as_deref() {
        Some("f32") => 32,
        Some("f64") => 64,
        _ => 0,
    }
}

fn named_ty(n: &str) -> Ty {
    match n {
        "int8" | "uint8" => Ty::Int(8),
        "int16" | "uint16" => Ty::Int(16),
        "int32" | "uint32" => Ty::Int(32),
        "int64" | "uint64" => Ty::Int(64),
        "float32" => Ty::Float(32),
        "float64" => Ty::Float(64),
        "bool" => Ty::Bool,
        "char" => Ty::Char,
        "string" => Ty::Str,
        "error" => Ty::Error,
        "thread" => Ty::Thread,
        "void" => Ty::Unit,
        _ => Ty::Named(n.to_string()),
    }
}

/// The return type of a builtin that carries one, so a destructure or an error
/// method on its result types precisely. Builtins without an entry stay
/// permissive and infer to Unknown.
fn builtin_ret(name: &str) -> Option<Ty> {
    match name {
        "read_file" | "read_line" | "read_all" => Some(Ty::Tuple(vec![Ty::Str, Ty::Error])),
        "parse_float" => Some(Ty::Tuple(vec![Ty::Float(64), Ty::Error])),
        "write_file" => Some(Ty::Error),
        "cstr" => Some(Ty::Str),
        _ => None,
    }
}

/// A short human name for a type in diagnostics.
fn ty_str(t: &Ty) -> String {
    match t {
        Ty::Int(0) => "an integer literal".to_string(),
        Ty::Int(w) => format!("int{w}"),
        Ty::Float(0) => "a float literal".to_string(),
        Ty::Float(w) => format!("float{w}"),
        Ty::Bool => "bool".to_string(),
        Ty::Char => "char".to_string(),
        Ty::Str => "string".to_string(),
        Ty::Unit => "void".to_string(),
        Ty::Error => "error".to_string(),
        Ty::Thread => "a thread handle".to_string(),
        Ty::Ptr(_) => "a managed pointer".to_string(),
        Ty::RawPtr(_) => "a raw pointer".to_string(),
        Ty::Slice(_) => "a slice".to_string(),
        Ty::Array(..) => "an array".to_string(),
        Ty::Tuple(_) => "a tuple".to_string(),
        Ty::Named(n) => format!("'{n}'"),
        Ty::Func(..) => "a function".to_string(),
        Ty::Future(_) => "a future".to_string(),
        Ty::Unknown => "an unknown type".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn errs(src: &str) -> Vec<Diagnostic> {
        let (t, le) = lex(src);
        assert!(le.is_empty(), "lex errors: {le:?}");
        let (m, pe) = parse(t);
        assert!(pe.is_empty(), "parse errors: {pe:?}");
        check(&m)
    }

    #[test]
    fn arithmetic_mismatch() {
        let e = errs("func f() -> int64 { return 1 + true }");
        assert!(!e.is_empty());
    }

    #[test]
    fn bitwise_ops_typecheck() {
        let e = errs("func f() -> int64 { return (12 & 10) | (3 ^ 1) }");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn bitwise_width_mix_rejected() {
        let e = errs(
            "func f() -> int64 {\n  x: int32 = 5\n  y: int64 = 7\n  return x & y\n}",
        );
        assert!(
            e.iter().any(|d| d.msg == "'&' mixes int32 and int64; match the widths"),
            "{e:?}"
        );
    }

    #[test]
    fn bitwise_on_bool_rejected() {
        let e = errs("func f() -> bool { return true & false }");
        assert!(
            e.iter().any(|d| d.msg == "bitwise operators need integer operands"),
            "{e:?}"
        );
    }

    #[test]
    fn shift_on_float_rejected() {
        let e = errs("func f() -> int64 {\n  x: float64 = 1.0\n  return x << 2\n}");
        assert!(
            e.iter().any(|d| d.msg == "shift operators need integer operands"),
            "{e:?}"
        );
    }

    #[test]
    fn bitnot_on_float_rejected() {
        let e = errs("func f() -> float64 { return ~1.5 }");
        assert!(
            e.iter().any(|d| d.msg == "'~' needs an integer operand"),
            "{e:?}"
        );
    }

    #[test]
    fn negative_constant_shift_rejected() {
        let e = errs("func f() -> int64 { return 1 << -1 }");
        assert!(
            e.iter().any(|d| d.msg == "shift amount is negative"),
            "{e:?}"
        );
    }

    #[test]
    fn oversize_constant_shift_rejected() {
        let e = errs("func f() -> int32 {\n  x: int32 = 1\n  return x << 32\n}");
        assert!(
            e.iter().any(|d| d.msg == "shift amount 32 is out of range for int32"),
            "{e:?}"
        );
    }

    #[test]
    fn shift_by_width_of_wide_int_ok() {
        // `1 << 32` on two adaptable literals defaults to int64, where 32 is in
        // range, so no error fires.
        let e = errs("func f() -> int64 { return 1 << 32 }");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn pow_int_and_float_typecheck() {
        let e = errs(
            "func f() -> int64 { return 2 ** 10 }\n\
             func g() -> float64 { return 2.0 ** 3.0 }",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn pow_mixing_int_and_float_rejected() {
        let e = errs("func f() -> int64 {\n  x: int64 = 2\n  return x ** 2.0\n}");
        assert!(
            e.iter().any(|d| d.msg == "'**' needs two operands of the same numeric type"),
            "{e:?}"
        );
    }

    #[test]
    fn pow_negative_constant_exponent_rejected() {
        let e = errs("func f() -> int64 { return 2 ** -1 }");
        assert!(
            e.iter().any(|d| d.msg == "'**' on integers needs a nonnegative exponent"),
            "{e:?}"
        );
    }

    #[test]
    fn pow_float_negative_exponent_ok() {
        // A float exponent may be negative; only integer `**` requires it be
        // nonnegative.
        let e = errs("func f() -> float64 { return 2.0 ** -1.0 }");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn compound_assignment_width_mix_rejected() {
        let e = errs(
            "func f() -> void {\n  mut x: int32 = 1\n  y: int64 = 2\n  x += y\n  return\n}",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("mixes int32 and int64")),
            "{e:?}"
        );
    }

    #[test]
    fn compound_assignment_ok() {
        let e = errs(
            "func f() -> void {\n  mut x: int64 = 1\n  x += 2\n  x <<= 3\n  x &= 7\n  return\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn compound_oversize_shift_rejected() {
        let e = errs(
            "func f() -> void {\n  mut x: int32 = 1\n  x <<= 40\n  return\n}",
        );
        assert!(
            e.iter().any(|d| d.msg == "shift amount 40 is out of range for int32"),
            "{e:?}"
        );
    }

    #[test]
    fn foreign_managed_pointer_rejected() {
        let e = errs(
            "foreign \"C\" { func bad(p: *int64) -> int32 }\nfunc main() -> int32 { return 0 }",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("managed pointer at the C boundary")),
            "{e:?}"
        );
    }

    #[test]
    fn foreign_bad_abi_rejected() {
        let e = errs(
            "foreign \"Rust\" { func abs(n: int32) -> int32 }\nfunc main() -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg.contains("only \"C\" is supported")), "{e:?}");
    }

    #[test]
    fn foreign_raw_pointer_ok() {
        let e = errs(
            "foreign \"C\" { func memset(dst: *raw int8, c: int32, n: int64) -> *void }\n\
             func main() -> int32 { return 0 }",
        );
        assert!(e.is_empty(), "raw pointer boundary should be accepted: {e:?}");
    }

    #[test]
    fn struct_literal_unknown_field_rejected() {
        let e = errs("struct S { x: int64 }\nfunc main() -> int32 {\n  s := S { y: 5 }\n  return 0\n}");
        assert!(e.iter().any(|d| d.msg.contains("no field 'y'")), "{e:?}");
    }

    #[test]
    fn struct_literal_missing_field_rejected() {
        let e = errs("struct S { x: int64, y: int64 }\nfunc main() -> int32 {\n  s := S { x: 1 }\n  return 0\n}");
        assert!(e.iter().any(|d| d.msg.contains("missing field 'y'")), "{e:?}");
    }

    #[test]
    fn struct_literal_complete_ok() {
        let e = errs("struct S { x: int64, y: int64 }\nfunc main() -> int32 {\n  s := S { x: 1, y: 2 }\n  return 0\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn passing_struct_without_impl_is_rejected() {
        let e = errs("interface I { get() -> int64 }\nstruct S { x: int64 }\nfunc take(i: I) -> int64 { return i.get() }\nfunc f() -> void {\n  s := S { x: 1 }\n  take(s)\n}");
        assert!(e.iter().any(|d| d.msg.contains("does not implement interface 'I'")), "{e:?}");
    }

    #[test]
    fn incomplete_impl_is_rejected() {
        let e = errs("interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { }");
        assert!(e.iter().any(|d| d.msg.contains("missing method 'get'")), "{e:?}");
    }

    #[test]
    fn struct_with_impl_satisfies_interface() {
        let e = errs("interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return self.x } }\nfunc take(i: I) -> int64 { return i.get() }\nfunc f() -> void {\n  s := S { x: 1 }\n  take(s)\n}");
        assert!(!e.iter().any(|d| d.msg.contains("implement interface")), "{e:?}");
    }

    #[test]
    fn discarding_a_fallible_call_is_rejected() {
        let e = errs("func fail() -> error { return error { message: \"x\" } }\nfunc f() -> void {\n  fail()\n}");
        assert!(e.iter().any(|d| d.msg.contains("error result is ignored")), "{e:?}");
    }

    #[test]
    fn handling_a_fallible_result_is_ok() {
        let e = errs("func fail() -> error { return error { message: \"x\" } }\nfunc f() -> void {\n  e := fail()\n  e.ignore()\n}");
        assert!(!e.iter().any(|d| d.msg.contains("error result is ignored")), "{e:?}");
    }

    #[test]
    fn condition_must_be_bool() {
        let e = errs("func f() -> void {\n  if 5 {\n    return\n  }\n  return\n}");
        assert!(e.iter().any(|d| d.msg.contains("bool")));
    }

    #[test]
    fn return_type_mismatch() {
        let e = errs("func f() -> bool { return 5 }");
        assert!(!e.is_empty());
    }

    #[test]
    fn arg_count_mismatch() {
        let e = errs("func g(a: int64) -> int64 { return a }\nfunc f() -> int64 { return g(1, 2) }");
        assert!(e.iter().any(|d| d.msg.contains("argument")));
    }

    #[test]
    fn deref_non_pointer() {
        let e = errs("func f() -> int64 {\n  x := 5\n  return *x\n}");
        assert!(e.iter().any(|d| d.msg.contains("dereference")));
    }

    #[test]
    fn copying_an_owner_is_rejected() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc(5)\n  q: *int64 = p\n  free(q)\n}");
        assert!(e.iter().any(|d| d.msg.contains("copy an owning pointer")), "{e:?}");
    }

    #[test]
    fn use_after_move_is_rejected() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc(5)\n  q: *int64 = move(p)\n  free(p)\n  free(q)\n}");
        assert!(e.iter().any(|d| d.msg.contains("moved pointer")), "{e:?}");
    }

    #[test]
    fn freeing_a_borrow_is_rejected() {
        let e = errs("func sink(p: *int64) -> void {\n  free(p)\n}");
        assert!(e.iter().any(|d| d.msg.contains("borrowed pointer")), "{e:?}");
    }

    #[test]
    fn ref_alias_and_move_are_allowed() {
        // A ref aliases without copying ownership, and move transfers it; neither
        // is an ownership error, so the only diagnostics here are unrelated.
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(5)\n  ref r: *int64 = p\n  println(*r)\n  q: *int64 = move(p)\n  free(q)\n}",
        );
        assert!(
            !e.iter().any(|d| d.msg.contains("owning")
                || d.msg.contains("moved pointer")
                || d.msg.contains("borrowed pointer")),
            "{e:?}"
        );
    }

    #[test]
    fn assignment_copy_of_owner_is_rejected() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc(5)\n  mut q: *int64 = alloc(9)\n  q = p\n  free(p)\n}");
        assert!(e.iter().any(|d| d.msg.contains("copy an owning pointer")), "{e:?}");
    }

    #[test]
    fn reassigning_an_owner_is_rejected() {
        let e = errs("func f() -> void {\n  mut p: *int64 = alloc(5)\n  p = alloc(9)\n  free(p)\n}");
        assert!(e.iter().any(|d| d.msg.contains("reassign an owning pointer")), "{e:?}");
    }

    #[test]
    fn reassigning_a_borrow_cursor_is_allowed() {
        // A borrowing cursor, like a list walk variable, is not an owner, so it
        // may advance without tripping the reassignment or copy rules.
        let e = errs("func walk(head: *int64) -> void {\n  mut cur: *int64 = head\n  cur = head\n  println(*cur)\n}");
        assert!(
            !e.iter().any(|d| d.msg.contains("reassign") || d.msg.contains("owning")),
            "{e:?}"
        );
    }

    #[test]
    fn moving_a_borrow_is_rejected() {
        let e = errs("func sink(p: *int64) -> void {\n  q: *int64 = move(p)\n  free(q)\n}");
        assert!(e.iter().any(|d| d.msg.contains("move a borrowed pointer")), "{e:?}");
    }

    #[test]
    fn inferred_alloc_binding_is_an_owner() {
        // alloc infers to a managed pointer, so the inferred `:=` form is tracked
        // and a copy of it is the single owner violation.
        let e = errs("func f() -> void {\n  x := alloc(5)\n  y := x\n  free(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("copy an owning pointer")), "{e:?}");
    }

    #[test]
    fn a_conditional_move_does_not_leak_past_the_branch() {
        // The static pass is single block: a move inside an if does not invalidate
        // the binding after the branch; the runtime generation check backstops it.
        let e = errs("func f() -> void {\n  p: *int64 = alloc(5)\n  if true {\n    q: *int64 = move(p)\n    free(q)\n    return\n  }\n  free(p)\n}");
        assert!(!e.iter().any(|d| d.msg.contains("moved pointer")), "{e:?}");
    }

    #[test]
    fn slice_into_local_array_cannot_escape() {
        let e = errs("func f() -> int64[] {\n  xs: int64[3] = [1, 2, 3]\n  return xs[0..3]\n}");
        assert!(e.iter().any(|d| d.msg.contains("escapes its frame")), "{e:?}");
    }

    #[test]
    fn capturing_closure_cannot_escape() {
        let e = errs(
            "func f() -> (int64) -> int64 {\n  x: int64 = 10\n  return lambda (n: int64) -> int64 { return n + x }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("escapes its frame")), "{e:?}");
    }

    #[test]
    fn returning_a_slice_of_a_slice_parameter_is_allowed() {
        let e = errs("func f(xs: int64[]) -> int64[] {\n  return xs[0..2]\n}");
        assert!(!e.iter().any(|d| d.msg.contains("escapes")), "{e:?}");
    }

    #[test]
    fn returning_a_non_capturing_closure_is_allowed() {
        let e = errs(
            "func f() -> (int64) -> int64 {\n  return lambda (n: int64) -> int64 { return n + 1 }\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("escapes")), "{e:?}");
    }

    #[test]
    fn exhaustive_match_clean() {
        let e = errs(
            "enum E { A, B, C }\n\
             func f(e: E) -> int64 {\n  match e {\n    A => return 1,\n    B => return 2,\n    C => return 3,\n  }\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn non_exhaustive_match_errors() {
        let e = errs(
            "enum E { A, B, C }\n\
             func f(e: E) -> int64 {\n  match e {\n    A => return 1,\n    B => return 2,\n  }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("non exhaustive")), "{e:?}");
    }

    #[test]
    fn wildcard_makes_match_exhaustive() {
        let e = errs(
            "enum E { A, B, C }\n\
             func f(e: E) -> int64 {\n  match e {\n    A => return 1,\n    _ => return 0,\n  }\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn unreachable_arm_after_catch_all_errors() {
        let e = errs(
            "enum E { A, B }\n\
             func f(e: E) -> int64 {\n  match e {\n    _ => return 0,\n    A => return 1,\n  }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("unreachable")), "{e:?}");
    }

    #[test]
    fn good_program_clean() {
        let e = errs(
            "func add(a: int64, b: int64) -> int64 { return a + b }\n\
             func f() -> int64 {\n  x := add(1, 2)\n  if x == 3 {\n    return x\n  }\n  return 0\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn mixed_integer_widths_are_rejected() {
        let e = errs(
            "func f() -> void {\n  a: int32 = 5\n  b: int64 = 9\n  c := a + b\n  println(c)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("mixes int32 and int64")), "{e:?}");
    }

    #[test]
    fn literal_adapts_to_any_width() {
        let e = errs(
            "func f() -> int32 {\n  a: int32 = 5\n  b := a + 1\n  return b\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn cross_width_assignment_is_rejected() {
        let e = errs("func f(w: int64) -> void {\n  x: int8 = w\n  println(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("annotation that does not match")), "{e:?}");
    }

    #[test]
    fn literal_too_wide_for_annotation_is_rejected() {
        let e = errs("func f() -> void {\n  x: int8 = 300\n  println(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("does not fit in 8 bits")), "{e:?}");
    }

    #[test]
    fn suffixed_literal_out_of_range_is_rejected() {
        let e = errs("func f() -> void {\n  x := 300i8\n  println(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("does not fit in 8 bits")), "{e:?}");
    }

    #[test]
    fn returning_an_array_literal_as_a_slice_is_rejected() {
        let e = errs("func make() -> int64[] { return [1, 2, 3] }");
        assert!(e.iter().any(|d| d.msg.contains("escapes its frame")), "{e:?}");
    }

    #[test]
    fn unsized_alloc_needs_a_pointer_annotation() {
        let e = errs("func f() -> void {\n  x := alloc()\n  free(x)\n}");
        assert!(e.iter().any(|d| d.msg.contains("pointer type annotation")), "{e:?}");
    }

    #[test]
    fn unsized_alloc_with_pointer_annotation_is_ok() {
        let e = errs("func f() -> void {\n  p: *int64 = alloc()\n  *p = 5\n  free(p)\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn element_store_into_immutable_array_is_rejected() {
        let e = errs("func f() -> void {\n  xs: int64[3] = [1, 2, 3]\n  xs[0] = 99\n  println(xs[0])\n}");
        assert!(e.iter().any(|d| d.msg.contains("assign through immutable 'xs'")), "{e:?}");
    }

    #[test]
    fn field_store_into_immutable_struct_is_rejected() {
        let e = errs(
            "struct P { x: int64 }\nfunc f() -> void {\n  p := P { x: 1 }\n  p.x = 42\n  println(p.x)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("assign through immutable 'p'")), "{e:?}");
    }

    #[test]
    fn element_store_into_mut_array_is_ok() {
        let e = errs("func f() -> void {\n  mut xs: int64[3] = [1, 2, 3]\n  xs[0] = 99\n  println(xs[0])\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn store_through_a_pointer_deref_is_ok() {
        // Mutation through a pointer writes the pointee, not the binding, so the
        // binding's immutability does not apply.
        let e = errs(
            "struct P { x: int64 }\nfunc f(p: *P) -> void {\n  (*p).x = 42\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("immutable")), "{e:?}");
    }

    #[test]
    fn field_store_through_pointer_needs_explicit_deref() {
        let e = errs(
            "struct P { x: int64 }\nfunc f(p: *P) -> void {\n  p.x = 42\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("explicit dereference")), "{e:?}");
    }

    #[test]
    fn binding_a_struct_to_an_interface_needs_an_impl() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nfunc f() -> void {\n  s := S { x: 7 }\n  i: I = s\n  println(i.get())\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("does not implement interface 'I'")), "{e:?}");
    }

    #[test]
    fn binding_a_struct_to_an_interface_with_impl_is_ok() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return self.x } }\nfunc f() -> void {\n  s := S { x: 7 }\n  i: I = s\n  println(i.get())\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("implement interface")), "{e:?}");
    }

    #[test]
    fn returning_a_struct_as_an_interface_needs_an_impl() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nfunc mk() -> I {\n  return S { x: 7 }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("does not implement interface 'I'")), "{e:?}");
    }

    #[test]
    fn match_over_a_non_enum_is_rejected() {
        let e = errs(
            "func f() -> void {\n  x := 5\n  match x {\n    a => println(1),\n    b => println(2),\n  }\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("match needs an enum value")), "{e:?}");
    }

    #[test]
    fn defer_inside_a_conditional_is_rejected() {
        let e = errs(
            "func f() -> void {\n  if true {\n    defer println(1)\n  }\n  println(2)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("defer inside a conditional or loop")), "{e:?}");
    }

    #[test]
    fn defer_at_function_top_level_is_ok() {
        let e = errs("func f() -> void {\n  defer println(1)\n  println(2)\n}");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn printing_a_bound_error_does_not_handle_it() {
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func f() -> void {\n  v, e := fail()\n  println(v)\n  println(e)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("'e' is never handled")), "{e:?}");
    }

    #[test]
    fn exists_check_ignore_and_return_all_handle_an_error() {
        let e = errs(
            "func fail() -> (int64, error) { return (0, error { message: \"x\" }) }\n\
             func a() -> void {\n  v, e := fail()\n  println(v)\n  if e.exists() {\n    return\n  }\n}\n\
             func b() -> void {\n  v, e := fail()\n  println(v)\n  e.ignore()\n}\n\
             func c() -> (int64, error) {\n  v, e := fail()\n  return (v, e)\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("never handled")), "{e:?}");
    }

    #[test]
    fn printing_a_struct_without_display_is_rejected() {
        let e = errs(
            "struct P { x: int64 }\nfunc f() -> void {\n  p := P { x: 1 }\n  println(p)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("no Display impl")), "{e:?}");
    }

    #[test]
    fn printing_a_struct_with_display_is_ok() {
        let e = errs(
            "@paradigm oop\n@paradigm procedural\ninterface Display { toString() -> string }\nstruct P { x: int64 }\nimpl Display for P { func toString() -> string { return \"P\" } }\nfunc f() -> void {\n  p := P { x: 1 }\n  println(p)\n}",
        );
        assert!(!e.iter().any(|d| d.msg.contains("Display")), "{e:?}");
    }

    #[test]
    fn single_argument_format_string_is_checked() {
        let e = errs("func f() -> void {\n  println(\"{}\")\n}");
        assert!(e.iter().any(|d| d.msg.contains("1 hole(s) but 0 argument(s)")), "{e:?}");
    }

    #[test]
    fn missing_return_on_a_path_is_rejected() {
        let e = errs("func f() -> int64 {\n  println(1)\n}");
        assert!(e.iter().any(|d| d.msg.contains("not all paths in 'f' return")), "{e:?}");
    }

    #[test]
    fn returns_in_both_branches_satisfy_missing_return() {
        let e = errs(
            "func f(c: bool) -> int64 {\n  if c {\n    return 1\n  } else {\n    return 2\n  }\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn duplicate_impl_for_same_pair_is_rejected() {
        let e = errs(
            "interface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return 1 } }\nimpl I for S { func get() -> int64 { return 2 } }",
        );
        assert!(e.iter().any(|d| d.msg.contains("duplicate 'impl I for S'")), "{e:?}");
    }

    #[test]
    fn allocator_main_form_is_rejected_for_now() {
        let e = errs(
            "func main(argc: int32, argv: string[], using a: int64) -> int32 { return 0 }",
        );
        assert!(e.iter().any(|d| d.msg.contains("allocator form is not supported yet")), "{e:?}");
    }

    #[test]
    fn argc_argv_main_is_accepted() {
        let e = errs("func main(argc: int32, argv: string[]) -> int32 { return argc }");
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn spawn_and_join_type_check_clean() {
        let e = errs(
            "func f() -> void {\n  t, e := spawn(lambda () -> void {\n    println(1)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn spawn_needs_a_lambda_literal() {
        let e = errs(
            "func f() -> void {\n  g := lambda () -> void { println(1) }\n  t, e := spawn(g)\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("lambda literal")), "{e:?}");
    }

    #[test]
    fn spawn_lambda_must_be_nullary_void() {
        let e = errs(
            "func f() -> void {\n  t, e := spawn(lambda (n: int64) -> void { println(n) })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("no parameters and returns void")), "{e:?}");
    }

    #[test]
    fn spawn_rejects_a_slice_capture() {
        let e = errs(
            "func f(xs: int64[]) -> void {\n  t, e := spawn(lambda () -> void {\n    println(xs[0])\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("cannot capture 'xs'")), "{e:?}");
    }

    #[test]
    fn spawned_thread_cannot_free_a_captured_pointer() {
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(5)\n  t, e := spawn(lambda () -> void {\n    free(p)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n  free(p)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("borrowed pointer")), "{e:?}");
    }

    #[test]
    fn a_dropped_join_error_is_rejected() {
        let e = errs(
            "func f() -> void {\n  t, e := spawn(lambda () -> void { println(1) })\n  e.ignore()\n  join(t)\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("error result is ignored")), "{e:?}");
    }

    #[test]
    fn spawn_rejects_a_slice_smuggled_in_a_struct_field() {
        let e = errs(
            "struct Wrap { s: int64[] }\nfunc f(xs: int64[]) -> void {\n  w := Wrap { s: xs }\n  t, e := spawn(lambda () -> void {\n    println(w.s[0])\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("cannot capture 'w'")), "{e:?}");
    }

    #[test]
    fn spawn_rejects_an_interface_value_capture() {
        let e = errs(
            "@paradigm oop\ninterface I { get() -> int64 }\nstruct S { x: int64 }\nimpl I for S { func get() -> int64 { return self.x } }\nfunc f() -> void {\n  s := S { x: 1 }\n  i: I = s\n  t, e := spawn(lambda () -> void {\n    println(i.get())\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("cannot capture 'i'")), "{e:?}");
    }

    #[test]
    fn spawn_capturing_a_moved_pointer_is_rejected() {
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(5)\n  q := move(p)\n  free(q)\n  t, e := spawn(lambda () -> void {\n    println(*p)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("moved pointer")), "{e:?}");
    }

    #[test]
    fn spawn_capturing_a_plain_struct_is_ok() {
        let e = errs(
            "struct P { x: int64, y: int64 }\nfunc f() -> void {\n  p := P { x: 1, y: 2 }\n  t, e := spawn(lambda () -> void {\n    println(p.x)\n  })\n  e.ignore()\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn printing_a_thread_handle_is_rejected() {
        let e = errs(
            "func f() -> void {\n  t, e := spawn(lambda () -> void { println(1) })\n  e.ignore()\n  println(t)\n  je := join(t)\n  je.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("thread handle")), "{e:?}");
    }

    #[test]
    fn submit_of_a_closure_variable_is_rejected() {
        let e = errs(
            "func f() -> void {\n  g := lambda () -> void { println(1) }\n  e := submit(g)\n  e.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("lambda literal")), "{e:?}");
    }

    #[test]
    fn submit_with_a_slice_capture_is_rejected() {
        let e = errs(
            "func f() -> void {\n  xs: int64[3] = [1, 2, 3]\n  s: int64[] = xs[0..2]\n  e := submit(lambda () -> void {\n    println(s[0])\n  })\n  e.ignore()\n}",
        );
        assert!(e.iter().any(|d| d.msg.contains("cannot capture")), "{e:?}");
    }

    #[test]
    fn submit_of_a_plain_task_is_ok() {
        let e = errs(
            "func f() -> void {\n  n := 5\n  e := submit(lambda () -> void {\n    println(n)\n  })\n  e.ignore()\n}",
        );
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn submit_freeing_a_captured_pointer_is_rejected() {
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(5)\n  e := submit(lambda () -> void {\n    free(p)\n  })\n  e.ignore()\n  free(p)\n}",
        );
        assert!(!e.is_empty(), "a task must not free its borrow");
    }

    #[test]
    fn raw_pointer_passes_where_void_pointer_is_expected() {
        let e = errs(
            "func take(p: *void) -> void { }\nfunc f() -> void {\n  b: *raw int8 = alloc_bytes(8)\n  take(b)\n  free(b)\n}",
        );
        assert!(e.is_empty(), "*raw int8 should pass as *void: {e:?}");
    }

    #[test]
    fn void_pointer_is_rejected_where_raw_pointer_is_expected() {
        // The reverse direction would let a managed pointer launder through
        // *void into a typed raw alias the generation check cannot see, so a
        // *void value binds a *void annotation only.
        let e = errs(
            "foreign \"C\" { func memset(dst: *raw int8, c: int32, n: int64) -> *void }\nfunc take(p: *raw int8) -> void { }\nfunc f() -> void {\n  b: *raw int8 = alloc_bytes(8)\n  v := memset(b, 0, 8)\n  take(v)\n  free(b)\n}",
        );
        assert!(!e.is_empty(), "*void must not pass as *raw int8");
    }

    #[test]
    fn managed_pointer_cannot_launder_to_raw_through_void() {
        let e = errs(
            "func f() -> void {\n  p: *int64 = alloc(41)\n  v: *void = p\n  r: *raw int64 = v\n  free(p)\n}",
        );
        assert!(!e.is_empty(), "*void must not become a typed raw alias");
    }

    #[test]
    fn raw_pointer_still_rejected_for_a_managed_pointer() {
        let e = errs(
            "func take(p: *int64) -> void { }\nfunc f() -> void {\n  b: *raw int64 = alloc_bytes(8)\n  take(b)\n  free(b)\n}",
        );
        assert!(!e.is_empty(), "*raw int64 must not pass as managed *int64");
    }

    // A legal async program is the baseline every rejection test contrasts with.
    const ASYNC_PRELUDE: &str = "async func leaf() -> (int64, error) { return (1, error {}) }\n\
         async func val(x: int64) -> int64 { return x }\n";

    #[test]
    fn legal_async_program_checks_clean() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}\
             async func amain() -> int64 {{\n  v, e := await leaf()\n  e.ignore()\n  a := await val(v)\n  return a\n}}\n\
             func main() -> int32 {{\n  r := async_run(amain())\n  println(r)\n  return 0\n}}"
        ));
        assert!(e.is_empty(), "a legal async program must check clean: {e:?}");
    }

    #[test]
    fn main_cannot_be_async() {
        let e = errs("async func main() -> int32 { return 0 }");
        assert!(e.iter().any(|d| d.msg == "main cannot be async; call an async func with async_run instead"), "{e:?}");
    }

    #[test]
    fn async_func_cannot_take_type_parameters() {
        let e = errs("async func g<T>(x: T) -> T { return x }");
        assert!(e.iter().any(|d| d.msg == "an async func cannot take type parameters"), "{e:?}");
    }

    #[test]
    fn async_func_cannot_take_a_slice_param() {
        let e = errs("async func g(xs: int64[]) -> void { return }");
        assert!(e.iter().any(|d| d.msg.contains("an async func cannot take 'xs'")), "{e:?}");
    }

    #[test]
    fn async_func_cannot_take_a_future_param() {
        let e = errs("async func g(f: Future<int64>) -> void { return }");
        assert!(
            e.iter().any(|d| d.msg.contains("a future belongs to the event loop thread; await it in the caller instead")),
            "{e:?}"
        );
    }

    #[test]
    fn async_param_with_a_directly_buried_future_reports_the_future_message() {
        // A future buried in a non-generic struct field reports the future reason,
        // not the generic slice/closure/interface message.
        let e = errs(
            "struct FBox { x: Future<int64> }\nasync func g(b: FBox) -> int64 { return 1 }",
        );
        assert!(
            e.iter().any(|d| d.msg.contains("an async func cannot take 'b'") && d.msg.contains("a future belongs to the event loop thread")),
            "{e:?}"
        );
    }

    #[test]
    fn async_func_cannot_return_a_slice() {
        let e = errs("async func g() -> int64[] { return [] }");
        assert!(
            e.iter().any(|d| d.msg == "an async func cannot return a slice, closure, or interface value; the value would outlive the task frame it views"),
            "{e:?}"
        );
    }

    #[test]
    fn async_name_in_value_position_is_rejected() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}func main() -> int32 {{\n  h := val\n  return 0\n}}"
        ));
        assert!(
            e.iter().any(|d| d.msg == "'val' is async; call it with await or start it with async_run"),
            "{e:?}"
        );
    }

    #[test]
    fn bare_async_call_is_never_awaited() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  val(3)\n  return 0\n}}"
        ));
        assert!(
            e.iter().any(|d| d.msg == "the future from 'val' is never awaited; bind it so it can be awaited or released"),
            "{e:?}"
        );
    }

    #[test]
    fn void_await_that_discards_a_value_is_rejected() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  await val(3)\n  return 0\n}}"
        ));
        assert!(
            e.iter().any(|d| d.msg == "'await f' discards a value; bind it, as in v, e := await f"),
            "{e:?}"
        );
    }

    #[test]
    fn awaited_error_must_be_handled() {
        // The err word of a two-bind await is a pending error like any other.
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  a, e := await val(3)\n  return a\n}}"
        ));
        assert!(e.iter().any(|d| d.msg.contains("the error 'e' is never handled")), "{e:?}");
    }

    #[test]
    fn single_bind_await_discards_the_error_word() {
        // O1: a single-bind await is allowed and drops the completer's err word.
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  a := await val(3)\n  return a\n}}"
        ));
        assert!(e.is_empty(), "single-bind await must check clean: {e:?}");
    }

    #[test]
    fn async_run_inside_an_async_func_is_rejected() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}async func amain() -> int64 {{\n  r := async_run(val(3))\n  return r\n}}"
        ));
        assert!(
            e.iter().any(|d| d.msg == "async_run cannot be called inside an async func; await the call instead"),
            "{e:?}"
        );
    }

    #[test]
    fn async_run_of_a_bound_future_is_rejected() {
        let e = errs(&format!(
            "{ASYNC_PRELUDE}func main() -> int32 {{\n  fa: Future<int64> = fnew()\n  r := async_run(fa)\n  println(r)\n  return 0\n}}"
        ));
        assert!(
            e.iter().any(|d| d.msg == "async_run takes a direct call of an async func, written at the call site"),
            "{e:?}"
        );
    }

    #[test]
    fn spawn_capturing_a_future_is_rejected() {
        let e = errs(
            "func main() -> int32 {\n  f: Future<int64> = fnew()\n  t, s := spawn(lambda () -> void {\n    use(f)\n  })\n  s.ignore()\n  return 0\n}",
        );
        assert!(
            e.iter().any(|d| d.msg == "spawn cannot capture 'f': a future belongs to the event loop thread"),
            "{e:?}"
        );
    }

    #[test]
    fn submit_capturing_a_future_is_rejected() {
        let e = errs(
            "func main() -> int32 {\n  f: Future<int64> = fnew()\n  s := submit(lambda () -> void {\n    use(f)\n  })\n  s.ignore()\n  return 0\n}",
        );
        assert!(
            e.iter().any(|d| d.msg == "submit cannot capture 'f': a future belongs to the event loop thread"),
            "{e:?}"
        );
    }
}
