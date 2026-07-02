//! Orchestration: LLVM IR to native binary via clang, then run.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::codegen;
use crate::parser::ast::Module;

/// Paths produced by a build.
pub struct BuildArtifacts {
    pub ll: PathBuf,
    pub bin: PathBuf,
}

/// Compiles a checked module to IR, links it with the runtime, returns artifacts.
pub fn build_module(module: &Module, out_dir: &Path, stem: &str) -> Result<BuildArtifacts, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("mkdir {}: {e}", out_dir.display()))?;
    let ll = out_dir.join(format!("{stem}.ll"));
    let bin = out_dir.join(stem);
    let ir = codegen::compile(module);
    std::fs::write(&ll, &ir).map_err(|e| format!("write {}: {e}", ll.display()))?;
    let rt = runtime_sources();
    let mut inputs: Vec<&Path> = vec![ll.as_path()];
    inputs.extend(rt.iter().map(|p| p.as_path()));
    link(&inputs, &bin)?;
    Ok(BuildArtifacts { ll, bin })
}

/// Emit the demo module's IR, link it with the C runtime, return artifact paths.
pub fn build_demo(out_dir: &Path) -> Result<BuildArtifacts, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("mkdir {}: {e}", out_dir.display()))?;
    let ll = out_dir.join("demo.ll");
    let bin = out_dir.join("demo");

    let ir = codegen::demo_module().render();
    std::fs::write(&ll, &ir).map_err(|e| format!("write {}: {e}", ll.display()))?;

    let rt = runtime_sources();
    let mut inputs: Vec<&Path> = vec![ll.as_path()];
    inputs.extend(rt.iter().map(|p| p.as_path()));
    link(&inputs, &bin)?;
    Ok(BuildArtifacts { ll, bin })
}

/// Invoke clang to assemble + link the given inputs (`.ll` and/or `.c`) into
/// `bin`. `-pthread` rides along for toolchains older than glibc 2.34, where
/// pthreads is not yet folded into libc.
fn link(inputs: &[&Path], bin: &Path) -> Result<(), String> {
    let mut cmd = Command::new("clang");
    for input in inputs {
        cmd.arg(input);
    }
    cmd.arg("-pthread");
    cmd.arg("-o").arg(bin);
    let status = cmd.status().map_err(|e| format!("spawn clang: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("clang exited with {status}"))
    }
}

/// Run a built binary, returning its exit code.
pub fn run(bin: &Path) -> Result<i32, String> {
    run_with(bin, &[])
}

/// Run a built binary with program arguments, so an argc/argv main sees them.
pub fn run_with(bin: &Path, args: &[String]) -> Result<i32, String> {
    let status = Command::new(bin)
        .args(args)
        .status()
        .map_err(|e| format!("run {}: {e}", bin.display()))?;
    Ok(status.code().unwrap_or(-1))
}

/// The C runtime sources at the crate root, regardless of the caller's CWD.
fn runtime_sources() -> Vec<PathBuf> {
    let rt = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    vec![rt.join("runtime.c"), rt.join("thread.c")]
}
