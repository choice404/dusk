//! Code generation: AST and typed IR to textual LLVM IR.

pub mod llvm;
pub mod lower;

pub use lower::compile;

use llvm::{Func, Module};

/// Host target. Made configurable (cross-compilation) post-0.1.0.
pub const DEFAULT_TRIPLE: &str = "x86_64-pc-linux-gnu";

/// Phase-0 spine: a hardcoded program that prints a line and an integer, then exits 0.
/// Proves source -> IR -> clang -> run without any front-end. Replaced at M5.
pub fn demo_module() -> Module {
    let mut m = Module::new("dusk_demo", DEFAULT_TRIPLE);
    m.declare("void @cool_println_cstr(ptr)");
    m.declare("void @cool_print_i64(i64)");

    let msg = m.cstring("hello from the dusk spine");

    let mut main = Func::new("i32", "main", "");
    main.call_void(&format!("@cool_println_cstr(ptr {msg})"));
    main.call_void("@cool_print_i64(i64 42)");
    main.ret("i32", "0");
    m.push_function(main.finish());

    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_ir_is_well_formed() {
        let ir = demo_module().render();
        assert!(ir.contains("define i32 @main()"));
        assert!(ir.contains("ret i32 0"));
        // string length includes the trailing NUL: 25 chars + 1
        assert!(ir.contains("[26 x i8]"));
    }
}
