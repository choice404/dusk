//! Semantic analysis: name resolution, types, immutability, paradigm gating. M3, M4, M9.

pub mod paradigm;
pub mod resolve;
pub mod typeck;

use std::collections::HashSet;

use crate::diag::Diagnostic;
use crate::parser::ast::Module;

/// Runs all semantic passes, returning the combined diagnostics. The
/// monomorphizer contributes its inference diagnostics too, so a type parameter
/// no site pins down fails `check` at its source line instead of codegen
/// emitting a wrong program. It runs only on an otherwise clean module, since
/// expansion over ill formed code would just repeat the earlier errors.
///
/// After the surface passes, a second type-only pass runs over the mono-expanded
/// (ground) module. A `do` over a generic monad desugars its continuations with
/// `Type::Infer` holes that lower to `Unknown`, which the compatibility rule
/// wildcards, so the surface pass cannot width-check a continuation body. Once
/// mono makes those types ground, re-running the real type/width checks recovers
/// exactly that suppressed class with no duplicated width logic. Only the
/// type/width/argument/exhaustiveness class fires in the ground pass; the
/// ownership, escape, and must-handle classes are suppressed there, since the
/// surface pass already ran them at full fidelity on the un-erased AST.
pub fn check(module: &Module) -> Vec<Diagnostic> {
    let mut d = resolve::check(module);
    d.extend(typeck::check(module));
    if d.is_empty() {
        // Expand once, feeding both mono's own diagnostics and the ground pass.
        let (ground, mono_diags) = crate::mono::expand_with_diags(module);
        d.extend(mono_diags);
        let mut seen: HashSet<(u32, u32, String)> =
            d.iter().map(|x| (x.span.lo, x.span.hi, x.msg.clone())).collect();
        for g in typeck::check_ground(&ground) {
            if seen.insert((g.span.lo, g.span.hi, g.msg.clone())) {
                d.push(g);
            }
        }
    }
    d
}
