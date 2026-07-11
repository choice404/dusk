//! dusk: compiler for the dusk language. Targets textual LLVM IR.

use std::path::PathBuf;
use std::process::ExitCode;

use dusk::sema::summary;
use dusk::{analyze, Analysis};
use dusk::{codegen, desugar, driver, lexer, loader, parser, prescan};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    match cmd {
        "demo" => cmd_demo(),
        "lex" => cmd_lex(args.get(1)),
        "scan" => cmd_scan(args.get(1)),
        "parse" => cmd_parse(args.get(1)),
        "load" => cmd_load(args.get(1)),
        "desugar" => cmd_desugar(args.get(1)),
        "check" => cmd_check(args.get(1)),
        "mono" => cmd_mono(args.get(1)),
        "esc" => cmd_esc(args.get(1)),
        "build" => cmd_build(args.get(1)),
        "ir" => cmd_ir(args.get(1)),
        "run" => cmd_run(args.get(1), args.get(2..).unwrap_or(&[])),
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

/// Renders a token kind for the lex dump. The dump is a differential interchange
/// contract, not a Rust value view, so the two forms a textual second compiler
/// cannot reproduce from Rust's own Debug are given canonical forms it can. A float
/// prints as its IEEE 754 bits, `Float(0x{:016X})`, so equal values print equal
/// text with no shortest-decimal rounding to match. A string, char, or rune escapes
/// every byte that is not printable ASCII as `\u{hex}`, so the escaping needs no
/// Unicode property tables, only the code point. Every other kind keeps its Debug
/// form, which is already a plain scalar or ordered list a second compiler matches.
fn render_kind(kind: &dusk::lexer::token::TokenKind) -> String {
    use dusk::lexer::token::TokenKind;
    match kind {
        TokenKind::Float { val, .. } => format!("Float(0x{:016X})", val.to_bits()),
        TokenKind::Str(s) => format!("Str(\"{}\")", parser::escape_canonical(s.chars())),
        TokenKind::Char(c) => format!("Char('{}')", parser::escape_canonical(std::iter::once(*c))),
        TokenKind::Rune(c) => format!("Rune('{}')", parser::escape_canonical(std::iter::once(*c))),
        other => format!("{other:?}"),
    }
}

/// Dumps the token stream for a source file.
fn cmd_lex(path: Option<&String>) -> ExitCode {
    let Some((path, src)) = read_src(path, "lex") else {
        return ExitCode::FAILURE;
    };
    let (tokens, errors) = lexer::lex(&src);
    for t in &tokens {
        println!(
            "{:>4}..{:<4} nl_before={} {}",
            t.span.lo,
            t.span.hi,
            t.nl_before,
            render_kind(&t.kind)
        );
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
    println!("{}", parser::render_module(&module));
    for e in lex_errs.iter().chain(parse_errs.iter()) {
        eprintln!("{}: {}", path, e.render(&src));
    }
    if lex_errs.is_empty() && parse_errs.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Loads a root file and its imports, dumping the merged AST.
fn cmd_load(path: Option<&String>) -> ExitCode {
    let Some(path) = required_path(path, "load") else {
        return ExitCode::FAILURE;
    };
    let prog = loader::load(&path);
    if let Some(module) = &prog.module {
        println!("{}", parser::render_module(module));
    }
    for e in &prog.errors {
        eprintln!("{e}");
    }
    if prog.errors.is_empty() && prog.module.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Loads and desugars a root file and its imports, dumping the resulting AST.
fn cmd_desugar(path: Option<&String>) -> ExitCode {
    let Some(path) = required_path(path, "desugar") else {
        return ExitCode::FAILURE;
    };
    let prog = loader::load(&path);
    let module = prog.module.as_ref().map(desugar::run);
    if let Some(module) = &module {
        println!("{}", parser::render_module(module));
    }
    for e in &prog.errors {
        eprintln!("{e}");
    }
    if prog.errors.is_empty() && module.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Lexes, parses, and resolves names for a source file, reporting diagnostics.
fn cmd_check(path: Option<&String>) -> ExitCode {
    let Some((path, _)) = read_src(path, "check") else {
        return ExitCode::FAILURE;
    };
    let (analysis, errs) = analyze(&path);
    for e in &errs {
        eprintln!("{e}");
    }
    if errs.is_empty() && analysis.is_some() {
        println!("ok: {path}");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Checks a source file and dumps the monomorphized ground AST when clean.
fn cmd_mono(path: Option<&String>) -> ExitCode {
    let Some((path, _)) = read_src(path, "mono") else {
        return ExitCode::FAILURE;
    };
    let (analysis, errs) = analyze(&path);
    for e in &errs {
        eprintln!("{e}");
    }
    if errs.is_empty() {
        if let Some(analysis) = analysis {
            println!("{}", parser::render_module(&analysis.ground_module()));
            return ExitCode::SUCCESS;
        }
    }
    ExitCode::FAILURE
}

/// Loads and desugars a source file, then dumps the escape summary oracle.
fn cmd_esc(path: Option<&String>) -> ExitCode {
    let Some(path) = required_path(path, "esc") else {
        return ExitCode::FAILURE;
    };
    let prog = loader::load(&path);
    for e in &prog.errors {
        eprintln!("{e}");
    }
    let Some(module) = &prog.module else {
        return ExitCode::FAILURE;
    };
    if !prog.errors.is_empty() {
        return ExitCode::FAILURE;
    }
    let desugared = desugar::run(module);
    let escape = summary::compute(&desugared);
    println!("{}", summary::render_escape_info(&escape));
    ExitCode::SUCCESS
}

/// Runs the front end. Prints diagnostics; returns the checked program only when
/// clean, so the build path has both the module and its mutable-tuple table.
fn front_end(path: &str) -> Option<Analysis> {
    let (analysis, errs) = analyze(path);
    for e in &errs {
        eprintln!("{e}");
    }
    analysis
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
    let Some((path, _)) = read_src(path, "build") else {
        return ExitCode::FAILURE;
    };
    let Some(analysis) = front_end(&path) else {
        return ExitCode::FAILURE;
    };
    let out = PathBuf::from("target").join("dusk-out");
    match driver::build_module(
        &analysis.module,
        &analysis.mut_tuple_types,
        &out,
        &stem_of(&path),
    ) {
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

/// Compiles a source file to LLVM IR text and prints it to stdout, without
/// invoking clang.
fn cmd_ir(path: Option<&String>) -> ExitCode {
    let Some((path, _)) = read_src(path, "ir") else {
        return ExitCode::FAILURE;
    };
    let Some(analysis) = front_end(&path) else {
        return ExitCode::FAILURE;
    };
    match codegen::compile(&analysis.module, &analysis.mut_tuple_types) {
        Ok(ir) => {
            print!("{ir}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[dusk] {e}");
            ExitCode::FAILURE
        }
    }
}

/// Compiles and runs a source file, forwarding any trailing arguments to the
/// program, so an argc/argv main sees them.
fn cmd_run(path: Option<&String>, prog_args: &[String]) -> ExitCode {
    let Some((path, _)) = read_src(path, "run") else {
        return ExitCode::FAILURE;
    };
    let Some(analysis) = front_end(&path) else {
        return ExitCode::FAILURE;
    };
    let out = PathBuf::from("target").join("dusk-out");
    let art = match driver::build_module(
        &analysis.module,
        &analysis.mut_tuple_types,
        &out,
        &stem_of(&path),
    ) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[dusk] {e}");
            return ExitCode::FAILURE;
        }
    };
    match driver::run_with(&art.bin, prog_args) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("[dusk] {e}");
            ExitCode::FAILURE
        }
    }
}

/// Reads a required path argument, printing command usage on failure.
fn required_path(path: Option<&String>, cmd: &str) -> Option<String> {
    let Some(path) = path else {
        eprintln!("usage: dusk {cmd} <file.dusk>");
        return None;
    };
    Some(path.clone())
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
    println!("  dusk load <file>     load imports, dump the merged AST");
    println!("  dusk desugar <file>  load + desugar, dump the AST");
    println!("  dusk check <file>    lex + parse + resolve + typecheck");
    println!("  dusk mono <file>     check + dump the monomorphized AST");
    println!("  dusk esc <file>      dump the escape summary oracle");
    println!("  dusk build <file>    compile to a native binary");
    println!("  dusk ir <file>       compile to LLVM IR, print to stdout");
    println!("  dusk run <file>      compile and run");
    println!("  dusk version         print version");
}
