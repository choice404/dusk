//! dusk: compiler for the dusk language. Targets textual LLVM IR.

use std::path::PathBuf;
use std::process::ExitCode;

use dusk::analyze;
use dusk::{driver, lexer, parser, prescan};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    match cmd {
        "demo" => cmd_demo(),
        "lex" => cmd_lex(args.get(1)),
        "scan" => cmd_scan(args.get(1)),
        "parse" => cmd_parse(args.get(1)),
        "check" => cmd_check(args.get(1)),
        "build" => cmd_build(args.get(1)),
        "run" => cmd_run(args.get(1)),
        "version" | "--version" | "-V" => {
            println!("dusk {VERSION}");
            ExitCode::SUCCESS
        }
        "help" | "--help" | "-h" => {
            print_help();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("dusk: unknown command '{other}'\n");
            print_help();
            ExitCode::FAILURE
        }
    }
}

/// Builds and runs the Phase 0 spine: hardcoded IR linked and executed.
fn cmd_demo() -> ExitCode {
    let out = PathBuf::from("target").join("dusk-out");
    let art = match driver::build_demo(&out) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[dusk] build error: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("[dusk] emitted IR : {}", art.ll.display());
    println!("[dusk] linked bin : {}", art.bin.display());
    println!("[dusk] running ->\n");
    match driver::run(&art.bin) {
        Ok(code) => {
            println!("\n[dusk] exit code  : {code}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[dusk] run error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Dumps the token stream for a source file.
fn cmd_lex(path: Option<&String>) -> ExitCode {
    let Some((path, src)) = read_src(path, "lex") else {
        return ExitCode::FAILURE;
    };
    let (tokens, errors) = lexer::lex(&src);
    for t in &tokens {
        println!("{:>4}..{:<4} {:?}", t.span.lo, t.span.hi, t.kind);
    }
    for e in &errors {
        eprintln!("{}: {}", path, e.render(&src));
    }
    if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Dumps the pre scan summary (paradigms and imports) for a source file.
fn cmd_scan(path: Option<&String>) -> ExitCode {
    let Some((path, src)) = read_src(path, "scan") else {
        return ExitCode::FAILURE;
    };
    let (pre, errors) = prescan::scan(&src);
    println!("paradigms: {:?}", pre.effective());
    println!("imports:");
    for imp in &pre.imports {
        println!("  {imp}");
    }
    for e in &errors {
        eprintln!("{}: {}", path, e.render(&src));
    }
    if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Lexes and parses a source file, dumping the AST.
fn cmd_parse(path: Option<&String>) -> ExitCode {
    let Some((path, src)) = read_src(path, "parse") else {
        return ExitCode::FAILURE;
    };
    let (tokens, lex_errs) = lexer::lex(&src);
    let (module, parse_errs) = parser::parse(tokens);
    println!("{module:#?}");
    for e in lex_errs.iter().chain(parse_errs.iter()) {
        eprintln!("{}: {}", path, e.render(&src));
    }
    if lex_errs.is_empty() && parse_errs.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Lexes, parses, and resolves names for a source file, reporting diagnostics.
fn cmd_check(path: Option<&String>) -> ExitCode {
    let Some((path, src)) = read_src(path, "check") else {
        return ExitCode::FAILURE;
    };
    let (module, errs) = analyze(&path, &src);
    for e in &errs {
        eprintln!("{e}");
    }
    if errs.is_empty() && module.is_some() {
        println!("ok: {path}");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Runs the front end. Prints diagnostics; returns the module only when clean.
fn front_end(path: &str, src: &str) -> Option<parser::ast::Module> {
    let (module, errs) = analyze(path, src);
    for e in &errs {
        eprintln!("{e}");
    }
    module
}

fn stem_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("out")
        .to_string()
}

/// Compiles a source file to a native binary.
fn cmd_build(path: Option<&String>) -> ExitCode {
    let Some((path, src)) = read_src(path, "build") else {
        return ExitCode::FAILURE;
    };
    let Some(module) = front_end(&path, &src) else {
        return ExitCode::FAILURE;
    };
    let out = PathBuf::from("target").join("dusk-out");
    match driver::build_module(&module, &out, &stem_of(&path)) {
        Ok(art) => {
            println!("[dusk] {}", art.bin.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[dusk] {e}");
            ExitCode::FAILURE
        }
    }
}

/// Compiles and runs a source file.
fn cmd_run(path: Option<&String>) -> ExitCode {
    let Some((path, src)) = read_src(path, "run") else {
        return ExitCode::FAILURE;
    };
    let Some(module) = front_end(&path, &src) else {
        return ExitCode::FAILURE;
    };
    let out = PathBuf::from("target").join("dusk-out");
    let art = match driver::build_module(&module, &out, &stem_of(&path)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[dusk] {e}");
            return ExitCode::FAILURE;
        }
    };
    match driver::run(&art.bin) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("[dusk] {e}");
            ExitCode::FAILURE
        }
    }
}

/// Reads a source file, printing a usage or read error on failure.
fn read_src(path: Option<&String>, cmd: &str) -> Option<(String, String)> {
    let Some(path) = path else {
        eprintln!("usage: dusk {cmd} <file.dusk>");
        return None;
    };
    match std::fs::read_to_string(path) {
        Ok(s) => Some((path.clone(), s)),
        Err(e) => {
            eprintln!("[dusk] read {path}: {e}");
            None
        }
    }
}

fn print_help() {
    println!("dusk {VERSION} - compiler for the dusk language\n");
    println!("usage:");
    println!("  dusk demo            build + run the Phase 0 LLVM spine");
    println!("  dusk lex <file>      dump the token stream");
    println!("  dusk scan <file>     dump paradigms + imports (pre scan)");
    println!("  dusk parse <file>    lex + parse, dump the AST");
    println!("  dusk check <file>    lex + parse + resolve + typecheck");
    println!("  dusk build <file>    compile to a native binary");
    println!("  dusk run <file>      compile and run");
    println!("  dusk version         print version");
}
