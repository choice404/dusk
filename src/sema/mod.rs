//! Semantic analysis: name resolution, types, immutability, paradigm gating. M3, M4, M9.

pub mod paradigm;
pub mod resolve;
pub mod typeck;

use crate::diag::Diagnostic;
use crate::parser::ast::Module;

/// Runs all semantic passes, returning the combined diagnostics.
pub fn check(module: &Module) -> Vec<Diagnostic> {
    let mut d = resolve::check(module);
    d.extend(typeck::check(module));
    d
}
