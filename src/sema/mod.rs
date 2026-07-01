//! Semantic analysis: name resolution, types, immutability, paradigm gating. M3, M4, M9.

pub mod paradigm;
pub mod resolve;
pub mod typeck;

use crate::diag::Diagnostic;
use crate::parser::ast::Module;

/// Runs all semantic passes, returning the combined diagnostics. The
/// monomorphizer contributes its inference diagnostics too, so a type parameter
/// no site pins down fails `check` at its source line instead of codegen
/// emitting a wrong program. It runs only on an otherwise clean module, since
/// expansion over ill formed code would just repeat the earlier errors.
pub fn check(module: &Module) -> Vec<Diagnostic> {
    let mut d = resolve::check(module);
    d.extend(typeck::check(module));
    if d.is_empty() {
        d.extend(crate::mono::diagnose(module));
    }
    d
}
