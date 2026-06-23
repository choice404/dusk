//! dawn: the package tool for the dusk language.
//!
//! Imports are git repository paths, the Go way. An `@import` like
//! `"github.com/user/repo/mod"` names a module `mod` inside the repository
//! `github.com/user/repo`. `dawn get` clones each repository into the package
//! cache (`$DAWN_CACHE` or `~/.dawn/cache`) so the dusk loader can resolve it.
//! Versioning and a lock file come in a later release.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use dusk::{analyze, driver, lexer, loader, parser};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("help") {
        "get" => cmd_get(args.get(1)),
        "build" => cmd_build(args.get(1), false),
        "run" => cmd_build(args.get(1), true),
        "version" | "--version" | "-V" => {
            println!("dawn {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            print_help();
            ExitCode::SUCCESS
        }
    }
}

/// Fetches every git package a file imports into the cache.
fn cmd_get(path: Option<&String>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("usage: dawn get <file.dusk>");
        return ExitCode::FAILURE;
    };
    match fetch_imports(path) {
        Ok(n) => {
            println!("dawn: {n} package(s) ready");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dawn: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Fetches packages, then compiles the file through the dusk pipeline, running
/// it when `run` is set.
fn cmd_build(path: Option<&String>, run: bool) -> ExitCode {
    let Some(path) = path else {
        eprintln!("usage: dawn {} <file.dusk>", if run { "run" } else { "build" });
        return ExitCode::FAILURE;
    };
    if let Err(e) = fetch_imports(path) {
        eprintln!("dawn: {e}");
        return ExitCode::FAILURE;
    }
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("dawn: read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (module, errs) = analyze(path, &src);
    for e in &errs {
        eprintln!("{e}");
    }
    let Some(module) = module else {
        return ExitCode::FAILURE;
    };
    let out = PathBuf::from("target").join("dawn-out");
    let art = match driver::build_module(&module, &out, stem(path)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("dawn: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("[dawn] {}", art.bin.display());
    if run {
        return match driver::run(&art.bin) {
            Ok(code) => ExitCode::from(code as u8),
            Err(e) => {
                eprintln!("dawn: {e}");
                ExitCode::FAILURE
            }
        };
    }
    ExitCode::SUCCESS
}

/// Parses a file's `@import` directives and clones every git package, skipping
/// ones already cached. Returns the number of git packages it ensured.
fn fetch_imports(path: &str) -> Result<usize, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let (tokens, _) = lexer::lex(&src);
    let (module, _) = parser::parse(tokens);
    let cache = loader::dawn_cache();
    let mut count = 0;
    for imp in &module.imports {
        if !imp.contains('/') {
            continue;
        }
        count += 1;
        let repo = repo_root(imp);
        let dest = cache.join(&repo);
        if dest.is_dir() {
            println!("dawn: cached {repo}");
            continue;
        }
        let url = format!("https://{repo}");
        println!("dawn: fetching {url}");
        clone(&url, &dest)?;
    }
    Ok(count)
}

/// The first three path segments, `host/user/repo`, are the repository root.
fn repo_root(import: &str) -> String {
    import.split('/').take(3).collect::<Vec<_>>().join("/")
}

/// Shallow clones a git repository into `dest`.
fn clone(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let status = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(url)
        .arg(dest)
        .status()
        .map_err(|e| format!("spawn git: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git clone {url} exited with {status}"))
    }
}

fn stem(path: &str) -> &str {
    Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("out")
}

fn print_help() {
    println!("dawn {} - package tool for the dusk language\n", env!("CARGO_PKG_VERSION"));
    println!("usage:");
    println!("  dawn get <file.dusk>     clone the git packages a file imports");
    println!("  dawn build <file.dusk>   fetch packages, then compile");
    println!("  dawn run <file.dusk>     fetch packages, compile, and run");
    println!("  dawn version             print version\n");
    println!("imports are git paths, e.g. @import \"github.com/user/repo/module\"");
    println!("cache: $DAWN_CACHE or ~/.dawn/cache");
}
