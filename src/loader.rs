//! Module loader. M10.
//!
//! Resolves `@import` directives, parses each referenced module file, and merges
//! their items into one program. Import paths are dotted, like `std.memory.arena`.
//! A path resolves against the importing file's directory first, then the stdlib
//! root (`lib/` beside the compiler). The merged module then flows through
//! desugaring, semantic analysis, and codegen as a single unit. Imported names
//! are flat globals for now; qualified call syntax is deferred past 0.1.0.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::lexer;
use crate::parser::{self, ast::Module};

/// The outcome of loading a program: the merged module when every file parsed,
/// and any lexer or parser errors, already rendered against their own file.
pub struct Program {
    pub module: Option<Module>,
    pub errors: Vec<String>,
}

/// Loads the root file and everything it imports, transitively, merging items.
pub fn load(root_path: &str) -> Program {
    let mut errors = Vec::new();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib");
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut items = Vec::new();

    let Some(root) = parse_file(root_path, &mut errors) else {
        return Program { module: None, errors };
    };
    visited.insert(canon(root_path));
    let root_dir = dir_of(Path::new(root_path));

    let mut work = vec![(root.imports.clone(), root_dir, root_path.to_string())];
    items.extend(root.items.clone());

    while let Some((imports, dir, importer)) = work.pop() {
        for imp in &imports {
            let Some(file) = resolve(imp, &dir, &stdlib) else {
                errors.push(format!("{importer}: cannot resolve import '{imp}'"));
                continue;
            };
            if !visited.insert(canon(&file.to_string_lossy())) {
                continue;
            }
            let path = file.to_string_lossy().into_owned();
            if let Some(m) = parse_file(&path, &mut errors) {
                work.push((m.imports.clone(), dir_of(&file), path));
                items.extend(m.items);
            }
        }
    }

    Program {
        module: Some(Module {
            paradigms: root.paradigms,
            imports: root.imports,
            items,
        }),
        errors,
    }
}

/// Reads and parses one file, appending any lexer or parser errors rendered
/// with the file path. Returns None when the file fails to read or parse.
fn parse_file(path: &str, errors: &mut Vec<String>) -> Option<Module> {
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
    ok.then_some(module)
}

/// Resolves an import to a `.dusk` file. A path containing a slash, like
/// `github.com/user/repo/mod`, is a dawn package and resolves against the dawn
/// cache. A dotted path, like `std.io`, resolves against the importing file's
/// directory then the stdlib root. Each form also tries the path as a leaf
/// symbol whose parent path is the module file.
fn resolve(import: &str, base: &Path, stdlib: &Path) -> Option<PathBuf> {
    if import.contains('/') {
        return resolve_in(import, &[dawn_cache()]);
    }
    let rel = import.replace('.', "/");
    resolve_in(&rel, &[base.to_path_buf(), stdlib.to_path_buf()])
}

/// Looks up `rel.dusk`, then `<parent of rel>.dusk`, under each root in order.
fn resolve_in(rel: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    for root in roots {
        let full = root.join(format!("{rel}.dusk"));
        if full.is_file() {
            return Some(full);
        }
        if let Some(idx) = rel.rfind('/') {
            let parent = root.join(format!("{}.dusk", &rel[..idx]));
            if parent.is_file() {
                return Some(parent);
            }
        }
    }
    None
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

    fn stdlib() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("lib")
    }

    #[test]
    fn resolves_module_file_in_stdlib() {
        let base = Path::new("/nonexistent");
        let f = resolve("std.io", base, &stdlib()).expect("std.io should resolve");
        assert!(f.ends_with("std/io.dusk"), "{}", f.display());
    }

    #[test]
    fn resolves_nested_module_file() {
        let base = Path::new("/nonexistent");
        let f = resolve("std.memory.arena", base, &stdlib()).expect("arena should resolve");
        assert!(f.ends_with("std/memory/arena.dusk"), "{}", f.display());
    }

    #[test]
    fn resolves_leaf_symbol_to_parent_file() {
        // std.io.print_int has no file; it resolves to the std/io.dusk parent.
        let base = Path::new("/nonexistent");
        let f = resolve("std.io.print_int", base, &stdlib()).expect("leaf should resolve");
        assert!(f.ends_with("std/io.dusk"), "{}", f.display());
    }

    #[test]
    fn unknown_import_does_not_resolve() {
        let base = Path::new("/nonexistent");
        assert!(resolve("std.nope.gone", base, &stdlib()).is_none());
    }
}
