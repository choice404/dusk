//! dusk: a small systems language compiler that targets textual LLVM IR.
//!
//! The library exposes the compiler passes so both the `dusk` compiler binary and
//! the `dawn` package tool can share the lexer, parser, and loader.

#![allow(dead_code)]

pub mod codegen;
pub mod desugar;
pub mod diag;
pub mod driver;
pub mod fmt;
pub mod home;
pub mod lexer;
pub mod loader;
pub mod mono;
pub mod parser;
pub mod prescan;
pub mod privatize;
pub mod sema;

/// A checked program ready to build: the desugared module plus the mutable-tuple
/// storage table the type pass reconciled, which the build path threads to mono so
/// codegen sizes those slots as slices. Bundled so the table rides beside the
/// module it belongs to instead of a bare tuple every caller must destructure.
pub struct Analysis {
    pub module: parser::ast::Module,
    pub mut_tuple_types: mono::MutTupleTypes,
}

/// Loads a program and its imports, gates paradigms per file, desugars, and runs
/// semantic analysis. Returns the checked program, ready to build, and any
/// diagnostics already rendered with their file path.
pub fn analyze(path: &str) -> (Option<Analysis>, Vec<String>) {
    let prog = loader::load(path);
    if !prog.errors.is_empty() || prog.module.is_none() {
        return (None, prog.errors);
    }
    let module = prog.module.as_ref().unwrap();
    let desugared = desugar::run(module);
    let (diags, mut_tuple_types) = sema::check(&desugared);
    if diags.is_empty() {
        (
            Some(Analysis {
                module: desugared,
                mut_tuple_types,
            }),
            Vec::new(),
        )
    } else {
        // Each diagnostic renders against the file its span falls in, so an error
        // in an imported module points at that module, not the root.
        let rendered = diags
            .iter()
            .map(|d| loader::render_diag(&prog.files, d))
            .collect();
        (None, rendered)
    }
}
