//! dusk: a small systems language compiler that targets textual LLVM IR.
//!
//! The library exposes the compiler passes so both the `dusk` compiler binary and
//! the `dawn` package tool can share the lexer, parser, and loader.

#![allow(dead_code)]

pub mod codegen;
pub mod desugar;
pub mod diag;
pub mod driver;
pub mod lexer;
pub mod loader;
pub mod mono;
pub mod parser;
pub mod prescan;
pub mod sema;

/// Loads a program and its imports, gates paradigms per file, desugars, and runs
/// semantic analysis. Returns the desugared module, ready to build, and any
/// diagnostics already rendered with their file path.
pub fn analyze(path: &str, src: &str) -> (Option<parser::ast::Module>, Vec<String>) {
    let prog = loader::load(path);
    if !prog.errors.is_empty() || prog.module.is_none() {
        return (None, prog.errors);
    }
    let module = prog.module.unwrap();
    let desugared = desugar::run(&module);
    let diags = sema::check(&desugared);
    if diags.is_empty() {
        (Some(desugared), Vec::new())
    } else {
        let rendered = diags.iter().map(|d| format!("{path}: {}", d.render(src))).collect();
        (None, rendered)
    }
}
