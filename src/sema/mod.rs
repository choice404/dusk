//! Semantic analysis: name resolution, types, immutability, paradigm gating. M3, M4, M9.

pub mod paradigm;
pub mod resolve;
pub mod summary;
pub mod typeck;

use std::collections::HashSet;

use crate::diag::Diagnostic;
use crate::mono::MutTupleTypes;
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
pub fn check(module: &Module) -> (Vec<Diagnostic>, MutTupleTypes) {
    // Interprocedural escape analysis (M5, 0.5.0). Computed over the merged,
    // desugared surface module before the body checks run, then handed to the
    // surface type pass: the per-function summaries propagate each callee's
    // frame-view relation across its call sites, the per-lambda alias table
    // drives the higher-order gates, and the frame-store sites become
    // diagnostics outright. The ground re-check gets no table and enforces
    // nothing, matching the escape class's surface-only fidelity.
    let escape = summary::compute(module);
    let mut d = resolve::check(module);
    let (type_diags, muts) = typeck::check(module, &escape);
    d.extend(type_diags);
    if d.is_empty() {
        // Expand once, feeding mono's own diagnostics, the ground pass, and the
        // future table the ground pass reads to undo mono's future mangle. The
        // mutable-tuple storage table travels in too, so the ground module carries
        // the same stamped `Bind.ty` codegen's expansion will, and the ground pass
        // re-checks exactly what gets built.
        let (ground, mono_diags, future_table) = crate::mono::expand_with_diags(module, &muts);
        d.extend(mono_diags);
        let mut seen: HashSet<(u32, u32, String)> =
            d.iter().map(|x| (x.span.lo, x.span.hi, x.msg.clone())).collect();
        for g in typeck::check_ground(&ground, &future_table) {
            if seen.insert((g.span.lo, g.span.hi, g.msg.clone())) {
                d.push(g);
            }
        }
    }
    (d, muts)
}
