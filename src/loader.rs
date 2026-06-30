//! Module loader. M10.
//!
//! Resolves `@import` directives, parses each referenced module file, and merges
//! their items into one program. Import paths are dotted, like `std.memory.arena`.
//! A path resolves against the importing file's directory first, then the stdlib
//! root (`lib/` beside the compiler). The merged module then flows through
//! desugaring, semantic analysis, and codegen as a single unit. Imported names
//! are flat globals for now; qualified call syntax is deferred past 0.1.0.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::diag::Diagnostic;
use crate::lexer;
use crate::parser::ast::{Block, Expr, ExprKind, Item, Match, Module, Stmt};
use crate::parser::{self};

/// One source file in a loaded program, kept so a merged stage diagnostic can be
/// rendered against the file it actually came from. `base` is the offset added to
/// that file's spans when merging, so spans across the whole program are unique.
pub struct FileSrc {
    pub path: String,
    pub base: u32,
    pub src: String,
}

/// The outcome of loading a program: the merged module when every file parsed,
/// any lexer or parser errors already rendered against their own file, and the
/// per file sources for rendering later semantic diagnostics.
pub struct Program {
    pub module: Option<Module>,
    pub errors: Vec<String>,
    pub files: Vec<FileSrc>,
}

/// Renders a merged stage diagnostic against the file its span falls in. Spans
/// are shifted by each file's `base` at load time, so the file is the one with
/// the greatest base not past the span.
pub fn render_diag(files: &[FileSrc], d: &Diagnostic) -> String {
    match files.iter().rev().find(|f| d.span.lo >= f.base) {
        Some(f) => format!("{}: {}", f.path, d.render_local(&f.src, d.span.lo - f.base)),
        None => format!("error: {}", d.msg),
    }
}

/// Loads the root file and everything it imports, transitively, merging items.
pub fn load(root_path: &str) -> Program {
    let mut errors = Vec::new();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib");
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut items = Vec::new();
    let mut files: Vec<FileSrc> = Vec::new();
    let mut base: u32 = 0;

    let Some((mut root, root_src)) = parse_file(root_path, &mut errors) else {
        return Program {
            module: None,
            errors,
            files,
        };
    };
    visited.insert(canon(root_path));
    let root_dir = dir_of(Path::new(root_path));

    register(&mut files, &mut base, root_path, root_src, &mut root);
    let root_imports = root.imports.clone();
    let root_paradigms = root.paradigms.clone();
    let mut work = vec![(root_imports.clone(), root_dir, root_path.to_string())];
    items.extend(root.items);

    let mut exports: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    let mut namespaces: HashSet<String> = HashSet::new();
    while let Some((imports, dir, importer)) = work.pop() {
        for imp in &imports {
            let Some((file, leaf)) = resolve(imp, &dir, &stdlib) else {
                errors.push(format!("{importer}: cannot resolve import '{imp}'"));
                continue;
            };
            // Register the module's dotted path as a callable namespace, so a
            // later `std.io.println(x)` folds to the merged global `println`. A
            // leaf import names a symbol, so its module is the path minus the leaf.
            if !imp.contains('/') {
                let modpath = match &leaf {
                    Some(_) => imp.rsplit_once('.').map(|(m, _)| m).unwrap_or(imp.as_str()),
                    None => imp.as_str(),
                };
                add_namespace(&mut namespaces, modpath);
            }
            let cfile = canon(&file.to_string_lossy());
            let path = file.to_string_lossy().into_owned();
            if visited.insert(cfile.clone()) {
                if let Some((mut m, src)) = parse_file(&path, &mut errors) {
                    exports.insert(cfile.clone(), exported_names(&m));
                    work.push((m.imports.clone(), dir_of(&file), path.clone()));
                    register(&mut files, &mut base, &path, src, &mut m);
                    items.extend(m.items);
                }
            }
            // A leaf import, like `std.io.print_line`, names a symbol. The parent
            // module must export it, otherwise the import is wrong.
            if let Some(leaf) = leaf {
                if let Some(names) = exports.get(&cfile) {
                    if !names.contains(&leaf) {
                        let modpath = imp
                            .rsplit_once(['.', '/'])
                            .map(|(m, _)| m)
                            .unwrap_or(imp.as_str());
                        errors.push(format!(
                            "{importer}: import '{imp}' names '{leaf}', but '{modpath}' exports no such symbol"
                        ));
                    }
                }
            }
        }
    }

    // Fold qualified module calls into bare calls now that every imported
    // namespace is known, so `std.io.println(x)` reaches the merged global.
    for it in &mut items {
        fold_item(it, &namespaces);
    }

    Program {
        module: Some(Module {
            paradigms: root_paradigms,
            imports: root_imports,
            items,
        }),
        errors,
        files,
    }
}

/// Shifts a parsed module's spans into the program wide coordinate space and
/// records its source, so a later semantic diagnostic renders against the right
/// file rather than the root.
fn register(files: &mut Vec<FileSrc>, base: &mut u32, path: &str, src: String, module: &mut Module) {
    shift_module(module, *base);
    let len = src.len() as u32;
    files.push(FileSrc {
        path: path.to_string(),
        base: *base,
        src,
    });
    // A one byte gap between files keeps spans from neighboring files distinct.
    *base += len + 1;
}

/// Reads and parses one file, appending any lexer or parser errors rendered
/// with the file path. Returns None when the file fails to read or parse.
fn parse_file(path: &str, errors: &mut Vec<String>) -> Option<(Module, String)> {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            errors.push(format!("{path}: read error: {e}"));
            return None;
        }
    };
    let (tokens, lex_errs) = lexer::lex(&src);
    let (module, parse_errs) = parser::parse(tokens);
    let mut ok = true;
    for d in lex_errs.iter().chain(parse_errs.iter()) {
        errors.push(format!("{path}: {}", d.render(&src)));
        ok = false;
    }
    if !ok {
        return None;
    }
    // Gate paradigms per file, against this file's own directives, so an imported
    // library is judged by its own `@paradigm`, not the root program's.
    for d in crate::sema::paradigm::check(&module) {
        errors.push(format!("{path}: {}", d.render(&src)));
        ok = false;
    }
    ok.then_some((module, src))
}

/// Adds `base` to every span in a module, so that after several files merge into
/// one program each span still points back into its own file's source.
fn shift_module(m: &mut Module, base: u32) {
    if base == 0 {
        return;
    }
    for it in &mut m.items {
        shift_item(it, base);
    }
}

fn shift_item(it: &mut Item, base: u32) {
    match it {
        Item::Func(f) => shift_block(&mut f.body, base),
        Item::Impl(i) => {
            for m in &mut i.methods {
                shift_block(&mut m.body, base);
            }
        }
        Item::Struct(_) | Item::Enum(_) | Item::Interface(_) | Item::Foreign(_) => {}
    }
}

fn shift_block(b: &mut Block, base: u32) {
    for s in &mut b.stmts {
        shift_stmt(s, base);
    }
}

fn shift_stmt(s: &mut Stmt, base: u32) {
    match s {
        Stmt::Let(l) => shift_expr(&mut l.value, base),
        Stmt::Assign(a, b) => {
            shift_expr(a, base);
            shift_expr(b, base);
        }
        Stmt::Return(Some(e)) | Stmt::Defer(e) | Stmt::Expr(e) => shift_expr(e, base),
        Stmt::Return(None) => {}
        Stmt::If(i) => {
            shift_expr(&mut i.cond, base);
            shift_block(&mut i.then, base);
            if let Some(e) = &mut i.els {
                shift_block(e, base);
            }
        }
        Stmt::While(w) => {
            shift_expr(&mut w.cond, base);
            shift_block(&mut w.body, base);
        }
        Stmt::For(f) => {
            shift_expr(&mut f.iter, base);
            shift_block(&mut f.body, base);
        }
        Stmt::Match(m) => shift_match(m, base),
    }
}

fn shift_match(m: &mut Match, base: u32) {
    shift_expr(&mut m.scrut, base);
    for a in &mut m.arms {
        shift_block(&mut a.body, base);
    }
}

fn shift_expr(e: &mut Expr, base: u32) {
    e.span.lo += base;
    e.span.hi += base;
    match &mut e.kind {
        ExprKind::Unary(_, x) => shift_expr(x, base),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b) => {
            shift_expr(a, base);
            shift_expr(b, base);
        }
        ExprKind::Call(f, args) => {
            shift_expr(f, base);
            for a in args {
                shift_expr(a, base);
            }
        }
        ExprKind::Field(x, _) => shift_expr(x, base),
        ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
            for x in xs {
                shift_expr(x, base);
            }
        }
        ExprKind::StructLit(_, fs) => {
            for (_, x) in fs {
                shift_expr(x, base);
            }
        }
        ExprKind::Lambda(l) => shift_block(&mut l.body, base),
        ExprKind::Match(m) => shift_match(m, base),
        ExprKind::Do(_, binds) => {
            for b in binds {
                shift_expr(&mut b.expr, base);
            }
        }
        ExprKind::Int(..)
        | ExprKind::Float(..)
        | ExprKind::Str(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Ident(_)
        | ExprKind::SizeofType(_) => {}
    }
}

/// Inserts a dotted module path and every dotted prefix of it into the namespace
/// set, so `std.memory.arena` registers `std`, `std.memory`, and `std.memory.arena`
/// as callable prefixes.
fn add_namespace(ns: &mut HashSet<String>, modpath: &str) {
    let mut acc = String::new();
    for seg in modpath.split('.') {
        if !acc.is_empty() {
            acc.push('.');
        }
        acc.push_str(seg);
        ns.insert(acc.clone());
    }
}

/// The dotted path of a callee built only from identifiers and field accesses,
/// like `std.io.println`. Returns None for any callee with a non identifier in
/// the chain, which is a field access on a real value, not a module path.
fn flatten_path(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field(b, name) => Some(format!("{}.{}", flatten_path(b)?, name)),
        _ => None,
    }
}

/// Rewrites a qualified module call to a bare call to its leaf. A call whose
/// callee is `<prefix>.<leaf>` where `prefix` names an imported module, like
/// `std.io.println(x)`, becomes `println(x)`, the flat global the loader merged.
/// A field access on a value, like `v.push(x)` or `e.exists()`, has a prefix that
/// is not a namespace, so it is left as a method call.
fn fold_item(it: &mut Item, ns: &HashSet<String>) {
    match it {
        Item::Func(f) => fold_block(&mut f.body, ns),
        Item::Impl(i) => {
            for m in &mut i.methods {
                fold_block(&mut m.body, ns);
            }
        }
        Item::Struct(_) | Item::Enum(_) | Item::Interface(_) | Item::Foreign(_) => {}
    }
}

fn fold_block(b: &mut Block, ns: &HashSet<String>) {
    for s in &mut b.stmts {
        fold_stmt(s, ns);
    }
}

fn fold_stmt(s: &mut Stmt, ns: &HashSet<String>) {
    match s {
        Stmt::Let(l) => fold_expr(&mut l.value, ns),
        Stmt::Assign(a, b) => {
            fold_expr(a, ns);
            fold_expr(b, ns);
        }
        Stmt::Return(Some(e)) | Stmt::Defer(e) | Stmt::Expr(e) => fold_expr(e, ns),
        Stmt::Return(None) => {}
        Stmt::If(i) => {
            fold_expr(&mut i.cond, ns);
            fold_block(&mut i.then, ns);
            if let Some(e) = &mut i.els {
                fold_block(e, ns);
            }
        }
        Stmt::While(w) => {
            fold_expr(&mut w.cond, ns);
            fold_block(&mut w.body, ns);
        }
        Stmt::For(f) => {
            fold_expr(&mut f.iter, ns);
            fold_block(&mut f.body, ns);
        }
        Stmt::Match(m) => fold_match(m, ns),
    }
}

fn fold_match(m: &mut Match, ns: &HashSet<String>) {
    fold_expr(&mut m.scrut, ns);
    for a in &mut m.arms {
        fold_block(&mut a.body, ns);
    }
}

fn fold_expr(e: &mut Expr, ns: &HashSet<String>) {
    match &mut e.kind {
        ExprKind::Call(f, args) => {
            fold_expr(f, ns);
            for a in args.iter_mut() {
                fold_expr(a, ns);
            }
            let rewrite = match &f.kind {
                ExprKind::Field(base, leaf) => match flatten_path(base) {
                    Some(prefix) if ns.contains(&prefix) => Some(leaf.clone()),
                    _ => None,
                },
                _ => None,
            };
            if let Some(leaf) = rewrite {
                f.kind = ExprKind::Ident(leaf);
            }
        }
        ExprKind::Unary(_, x) => fold_expr(x, ns),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b) => {
            fold_expr(a, ns);
            fold_expr(b, ns);
        }
        ExprKind::Field(x, _) => fold_expr(x, ns),
        ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
            for x in xs {
                fold_expr(x, ns);
            }
        }
        ExprKind::StructLit(_, fs) => {
            for (_, x) in fs {
                fold_expr(x, ns);
            }
        }
        ExprKind::Lambda(l) => fold_block(&mut l.body, ns),
        ExprKind::Match(m) => fold_match(m, ns),
        ExprKind::Do(_, binds) => {
            for b in binds {
                fold_expr(&mut b.expr, ns);
            }
        }
        ExprKind::Int(..)
        | ExprKind::Float(..)
        | ExprKind::Str(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Ident(_)
        | ExprKind::SizeofType(_) => {}
    }
}

/// Resolves an import to a `.dusk` file. A path containing a slash, like
/// `github.com/user/repo/mod`, is a dawn package and resolves against the dawn
/// cache. A dotted path, like `std.io`, resolves against the importing file's
/// directory then the stdlib root. Each form also tries the path as a leaf
/// symbol whose parent path is the module file.
fn resolve(import: &str, base: &Path, stdlib: &Path) -> Option<(PathBuf, Option<String>)> {
    if import.contains('/') {
        return resolve_in(import, &[dawn_cache()]);
    }
    let rel = import.replace('.', "/");
    resolve_in(&rel, &[base.to_path_buf(), stdlib.to_path_buf()])
}

/// Looks up `rel.dusk`, then `<parent of rel>.dusk`, under each root in order.
/// Returns the file and, when the parent matched, the trailing leaf segment that
/// names a symbol the parent module must export.
fn resolve_in(rel: &str, roots: &[PathBuf]) -> Option<(PathBuf, Option<String>)> {
    for root in roots {
        let full = root.join(format!("{rel}.dusk"));
        if full.is_file() {
            return Some((full, None));
        }
        if let Some(idx) = rel.rfind('/') {
            let parent = root.join(format!("{}.dusk", &rel[..idx]));
            if parent.is_file() {
                return Some((parent, Some(rel[idx + 1..].to_string())));
            }
        }
    }
    None
}

/// The names a module exports, the `export` marked top level items.
fn exported_names(m: &Module) -> HashSet<String> {
    let mut names = HashSet::new();
    for it in &m.items {
        let (exported, name) = match it {
            Item::Func(f) => (f.exported, &f.name),
            Item::Struct(s) => (s.exported, &s.name),
            Item::Enum(e) => (e.exported, &e.name),
            Item::Interface(i) => (i.exported, &i.name),
            Item::Impl(_) => continue,
            // A foreign block has no single name or export marker. Its functions
            // are module local; an exported dusk wrapper is what crosses modules.
            Item::Foreign(_) => continue,
        };
        if exported {
            names.insert(name.clone());
        }
    }
    names
}

/// The dawn package cache directory, `$DAWN_CACHE` or `~/.dawn/cache`.
pub fn dawn_cache() -> PathBuf {
    if let Ok(dir) = std::env::var("DAWN_CACHE") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".dawn").join("cache")
}

fn dir_of(path: &Path) -> PathBuf {
    path.parent().map(|p| p.to_path_buf()).unwrap_or_default()
}

fn canon(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::Func;

    fn stdlib() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("lib")
    }

    #[test]
    fn resolves_module_file_in_stdlib() {
        let base = Path::new("/nonexistent");
        let (f, leaf) = resolve("std.io", base, &stdlib()).expect("std.io should resolve");
        assert!(f.ends_with("std/io.dusk"), "{}", f.display());
        assert!(leaf.is_none(), "a module import has no leaf");
    }

    #[test]
    fn resolves_nested_module_file() {
        let base = Path::new("/nonexistent");
        let (f, leaf) = resolve("std.memory.arena", base, &stdlib()).expect("arena should resolve");
        assert!(f.ends_with("std/memory/arena.dusk"), "{}", f.display());
        assert!(leaf.is_none());
    }

    #[test]
    fn resolves_leaf_symbol_to_parent_file() {
        // std.io.print_int has no file; it resolves to the std/io.dusk parent with
        // print_int as the leaf symbol.
        let base = Path::new("/nonexistent");
        let (f, leaf) = resolve("std.io.print_int", base, &stdlib()).expect("leaf should resolve");
        assert!(f.ends_with("std/io.dusk"), "{}", f.display());
        assert_eq!(leaf.as_deref(), Some("print_int"));
    }

    #[test]
    fn exported_names_collects_only_exported_items() {
        let src = "export func a() -> void {}\nfunc b() -> void {}\nexport struct C {}\n";
        let (t, _) = lexer::lex(src);
        let (m, e) = parser::parse(t);
        assert!(e.is_empty(), "parse errors: {e:?}");
        let names = exported_names(&m);
        assert!(names.contains("a"), "a is exported");
        assert!(names.contains("C"), "C is exported");
        assert!(!names.contains("b"), "b is private");
    }

    #[test]
    fn unknown_import_does_not_resolve() {
        let base = Path::new("/nonexistent");
        assert!(resolve("std.nope.gone", base, &stdlib()).is_none());
    }

    #[test]
    fn render_diag_attributes_to_the_imported_file() {
        use crate::diag::Span;
        let files = vec![
            FileSrc {
                path: "root.dusk".into(),
                base: 0,
                src: "func main() {}\n".into(),
            },
            FileSrc {
                path: "lib.dusk".into(),
                base: 100,
                src: "func a() {}\nfunc b() {}\n".into(),
            },
        ];
        // Global offset 112 lands in lib.dusk at local offset 12, line 2 column 1.
        let d = Diagnostic::new("boom", Span::new(112, 113));
        let out = render_diag(&files, &d);
        assert!(out.starts_with("lib.dusk: 2:1:"), "{out}");
        assert!(out.contains("boom"), "{out}");
    }

    #[test]
    fn render_diag_attributes_to_the_root_file() {
        use crate::diag::Span;
        let files = vec![FileSrc {
            path: "root.dusk".into(),
            base: 0,
            src: "ab\ncd\n".into(),
        }];
        let d = Diagnostic::new("x", Span::new(3, 4));
        let out = render_diag(&files, &d);
        assert!(out.starts_with("root.dusk: 2:1:"), "{out}");
    }

    #[test]
    fn add_namespace_inserts_all_prefixes() {
        let mut ns = HashSet::new();
        add_namespace(&mut ns, "std.memory.arena");
        assert!(ns.contains("std"));
        assert!(ns.contains("std.memory"));
        assert!(ns.contains("std.memory.arena"));
        assert!(!ns.contains("std.memory.arena.alloc"));
    }

    fn parse_module(src: &str) -> Module {
        let (t, le) = lexer::lex(src);
        assert!(le.is_empty(), "lex errors: {le:?}");
        let (m, pe) = parser::parse(t);
        assert!(pe.is_empty(), "parse errors: {pe:?}");
        m
    }

    fn first_call_callee(f: &Func, idx: usize) -> &ExprKind {
        let Stmt::Expr(e) = &f.body.stmts[idx] else {
            panic!("stmt {idx} is not an expression: {:?}", f.body.stmts[idx]);
        };
        let ExprKind::Call(callee, _) = &e.kind else {
            panic!("stmt {idx} is not a call: {:?}", e.kind);
        };
        &callee.kind
    }

    #[test]
    fn fold_rewrites_qualified_call_and_keeps_method_call() {
        let mut m = parse_module(
            "func f(v: *int64) -> void {\n  std.io.print_line(\"hi\")\n  v.exists()\n}",
        );
        let mut ns = HashSet::new();
        add_namespace(&mut ns, "std.io");
        for it in &mut m.items {
            fold_item(it, &ns);
        }
        let Item::Func(f) = &m.items[0] else { panic!() };
        // The qualified module call folds to the bare leaf function.
        assert!(
            matches!(first_call_callee(f, 0), ExprKind::Ident(n) if n == "print_line"),
            "qualified call should fold to a bare ident: {:?}",
            first_call_callee(f, 0)
        );
        // A method call on a value keeps its field access, since its prefix `v`
        // is not a module namespace.
        assert!(
            matches!(first_call_callee(f, 1), ExprKind::Field(..)),
            "method call should stay a field access: {:?}",
            first_call_callee(f, 1)
        );
    }

    #[test]
    fn fold_leaves_enum_variant_constructors_alone() {
        // `Maybe.Some(1)` is an enum constructor, not a module call. `Maybe` is not
        // a namespace, so the fold must not touch it.
        let mut m = parse_module("func f() -> void {\n  Maybe.Some(1)\n}");
        let mut ns = HashSet::new();
        add_namespace(&mut ns, "std.functional.maybe");
        for it in &mut m.items {
            fold_item(it, &ns);
        }
        let Item::Func(f) = &m.items[0] else { panic!() };
        assert!(
            matches!(first_call_callee(f, 0), ExprKind::Field(..)),
            "enum constructor should stay a field access: {:?}",
            first_call_callee(f, 0)
        );
    }
}
