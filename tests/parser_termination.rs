//! Parser termination corpus. Milestone M2, task W1b.
//!
//! The parser must terminate on every input, well formed or not. Each recovery
//! loop in `parser::mod` records its token position at the loop head and routes
//! a no-progress iteration through the one shared `guard_progress` helper, which
//! emits a diagnostic and forces a bump. These tests feed representative
//! malformed programs through lex + parse under a wall-clock bound: a parse that
//! returns at all is the termination proof, and every malformed program must
//! surface at least one diagnostic rather than being silently accepted.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use dusk::lexer::lex;
use dusk::parser::parse;

/// A generous per-input wall-clock ceiling. A real parse of these tiny inputs
/// finishes in microseconds; the bound exists only to convert a hypothetical
/// non-terminating spin into a test failure instead of a hung suite.
const BOUND: Duration = Duration::from_secs(5);

/// Runs lex + parse on `src` on a worker thread and waits at most `BOUND` for
/// it, returning the parser's diagnostic messages. Panics naming the input if
/// the parse does not finish in time, which is the non-termination signal. The
/// worker gets an 8 MiB stack, matching the process main thread, so these tests
/// measure loop progress rather than the smaller default worker stack. Recursion
/// depth is bounded by the parser's own nesting ceiling; the deep-nesting tests at
/// the end of this file drive that ceiling and rely on the same 8 MiB worker.
fn parse_within(src: &str) -> Vec<String> {
    let owned = src.to_string();
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let (toks, _lex_errs) = lex(&owned);
            let (_module, perrs) = parse(toks);
            let msgs: Vec<String> = perrs.into_iter().map(|d| d.msg).collect();
            // The receiver may already be gone if the bound elapsed; ignore that.
            let _ = tx.send(msgs);
        })
        .expect("spawn parser worker");
    match rx.recv_timeout(BOUND) {
        Ok(msgs) => msgs,
        Err(_) => panic!("parser did not terminate within {BOUND:?} on input: {src:?}"),
    }
}

/// Representative malformed programs, one or more per recovery loop, plus
/// unbalanced-delimiter, token-flood, and truncation forms. Every entry is
/// syntactically broken, so a correct parser rejects each with a diagnostic.
fn corpus() -> Vec<String> {
    let mut c: Vec<String> = vec![
        // delimited item bodies fed pure garbage
        "struct S { , , , }".into(),
        "struct S { x int64 y int64 }".into(),
        "enum E { , , , }".into(),
        "enum E { A( , , ) }".into(),
        "interface I { + + + }".into(),
        "interface I { m( -> }".into(),
        "foreign \"C\" { + + + }".into(),
        "foreign \"C\" { func }".into(),
        "impl T { + + + }".into(),
        "impl I for { }".into(),
        "monad M { + + + }".into(),
        "monad { }".into(),
        // statement / block level garbage
        "func f() -> void { @ @ @ }".into(),
        "func f() -> void { ) ) ) }".into(),
        "func f() -> void { ]]] }".into(),
        "func f() -> void { match x { @ @ @ } }".into(),
        "func f() -> void { match { } }".into(),
        "func f() -> void { do { @ @ @ } }".into(),
        "func f() -> void { do { <- <- <- } }".into(),
        // module / directive level garbage
        "@ @ @ @".into(),
        "@import".into(),
        "@paradigm".into(),
        "123 456 789".into(),
        "export export export".into(),
        // type and generic pathology
        "func f() -> Foo<,,,> { return }".into(),
        "func f() -> Foo<<<<< { return }".into(),
        "func f(,,,) -> void { return }".into(),
        "func f() -> (,,,) { return }".into(),
        "func f() -> int[[[[ { return }".into(),
        "func f() -> Vec<int>= { return }".into(),
        // expression / call / index / array garbage
        "func f() -> void { g(,,,) }".into(),
        "func f() -> void { x[,,,] }".into(),
        "func f() -> void { x := [,,,] }".into(),
        "func f() -> void { (,,,) }".into(),
        "func f() -> void { P { , , } }".into(),
        "func f() -> void { return < < < }".into(),
        "func f() -> void { x := 1 |> |> |> }".into(),
        // keyword floods with no operands
        "func func func func".into(),
        "match match match".into(),
        "do do do do".into(),
        "return return return".into(),
        "if if if".into(),
        "while while while".into(),
        // unbalanced openers and closers
        "func f() -> void {".into(),
        "func f() -> void }".into(),
        "struct S {".into(),
        "interface I {".into(),
        "foreign \"C\" {".into(),
        "impl T {".into(),
        "func f(".into(),
        "func f<".into(),
    ];

    // Moderate-depth nested generics and parens with the closers deleted: a
    // malformed but linear parse the guards leave untouched. Depth is kept well
    // clear of the recursion-stack limit, which is a separate concern.
    c.push(format!(
        "func f() -> {}int {{ return }}",
        "Vec<".repeat(150)
    ));
    c.push(format!("func f() -> void {{ x := {}1 }}", "(".repeat(150)));
    c
}

/// A complete, well-formed program used as the source for prefix-truncation
/// mutations. Cutting it at arbitrary byte offsets yields inputs that end mid
/// construct; some cuts land on a valid prefix and parse clean, so truncations
/// are asserted for termination only, not for rejection.
const WHOLE: &str = "@paradigm procedural\nfunc helper(a: int64, b: int64) -> int64 {\n  \
     mut s: int64 = 0\n  for i in 0..a {\n    s = s + b\n  }\n  return s\n}\n\
     func main() -> int32 {\n  x := helper(3, 4)\n  match x {\n    _ => return 0,\n  }\n}\n";

#[test]
fn corpus_all_terminates_and_is_rejected() {
    for src in corpus() {
        let msgs = parse_within(&src);
        assert!(
            !msgs.is_empty(),
            "malformed input parsed with no diagnostic: {src:?}"
        );
    }
}

#[test]
fn wide_generic_arg_list_terminates() {
    // A very wide comma-separated argument list drives the type-argument loop
    // through thousands of iterations at a single depth, the breadth stress for
    // the list loop rather than a recursion-depth stress. It must terminate; the
    // trailing garbage `>` mismatch guarantees a diagnostic.
    let args = "int,".repeat(5000);
    let src = format!("func f() -> Foo<{args} +> {{ return }}");
    let msgs = parse_within(&src);
    assert!(!msgs.is_empty(), "wide generic arg list accepted");
}

#[test]
fn wide_flat_statement_flood_terminates() {
    // Thousands of sibling garbage statements exercise the block recovery loop's
    // breadth: each stall must force one bump and move on.
    let body = "@ ".repeat(5000);
    let src = format!("func f() -> void {{ {body} }}");
    let msgs = parse_within(&src);
    assert!(!msgs.is_empty(), "statement flood accepted");
}

#[test]
fn interleaved_delimiter_flood_terminates() {
    // A dense mix of every delimiter and operator with no structure, the worst
    // case for recovery loops. It must terminate and be rejected.
    let src = "{([<,>])}+*-/|&^ ".repeat(500);
    let full = format!("func f() -> void {{ {src} }}");
    let msgs = parse_within(&full);
    assert!(!msgs.is_empty(), "delimiter flood accepted");
}

#[test]
fn prefix_truncations_all_terminate() {
    // Every prefix cut of a whole program, at one-byte granularity, must
    // terminate. Rejection is not asserted: some prefixes are valid programs in
    // their own right. This is the byte-truncation half of the fuzz corpus.
    let bytes = WHOLE.as_bytes();
    for cut in 1..WHOLE.len() {
        if let Ok(s) = std::str::from_utf8(&bytes[..cut]) {
            // The call returning at all is the termination proof.
            let _ = parse_within(s);
        }
    }
}

#[test]
fn deep_paren_nesting_terminates_with_a_diagnostic() {
    // Twenty thousand open parens once overflowed the stack and raised SIGABRT.
    // The parser's depth ceiling now unwinds the recursion into a diagnostic on
    // the 8 MiB worker instead of crashing, and the call returning at all is the
    // proof it no longer aborts the process.
    let src = format!("func f() -> void {{ x := {}1 }}", "(".repeat(20_000));
    let msgs = parse_within(&src);
    assert!(
        msgs.iter()
            .any(|m| m == "expression nesting is too deep; simplify the expression"),
        "deep paren nesting not reported as too deep: first = {:?}",
        msgs.first()
    );
}

#[test]
fn deep_type_nesting_terminates_with_a_diagnostic() {
    // Deeply nested generics recurse through the type grammar; the same ceiling
    // stops them with a diagnostic rather than a stack overflow.
    let src = format!("func f() -> {}int {{ return }}", "Vec<".repeat(5_000));
    let msgs = parse_within(&src);
    assert!(
        msgs.iter()
            .any(|m| m == "type nesting is too deep; simplify the type"),
        "deep type nesting not reported as too deep: first = {:?}",
        msgs.first()
    );
}

#[test]
fn deep_if_block_nesting_terminates_with_a_diagnostic() {
    // Two thousand nested `if` blocks recurse through the statement grammar as
    // `if_ -> block -> stmt -> if_`, a path the expression and type guards did not
    // cover, so it once overflowed the stack and raised SIGABRT. Counting `block`
    // now feeds the shared ceiling: it is crossed while parsing an inner condition,
    // so the surfaced diagnostic names the expression, but the block guard is what
    // accumulates the depth. Without it this input still aborts the process; the
    // call returning at all with a too-deep diagnostic is the proof it does not.
    let depth = 2_000;
    let src = format!(
        "func f() -> void {{ {}{} }}",
        "if x { ".repeat(depth),
        "}".repeat(depth)
    );
    let msgs = parse_within(&src);
    assert!(
        msgs.iter().any(|m| m.contains("nesting is too deep")),
        "deep if-block nesting not reported as too deep: first = {:?}",
        msgs.first()
    );
}

#[test]
fn deep_do_block_nesting_reports_the_block_ceiling() {
    // A `do { ... }` body recurses `do_stmt -> do_block_elems -> stmt -> do_stmt`
    // without a condition to parse, so its own block guard, not the expression
    // guard, is the one that crosses the ceiling. This pins the block diagnostic's
    // exact wording and proves the `do` body path is bounded, since `do` does not
    // route through `block`.
    let depth = 2_000;
    let src = format!(
        "func f() -> void {{ {}0{} }}",
        "do { ".repeat(depth),
        "}".repeat(depth)
    );
    let msgs = parse_within(&src);
    assert!(
        msgs.iter()
            .any(|m| m == "block nesting is too deep; simplify the function"),
        "deep do-block nesting not reported as too deep: first = {:?}",
        msgs.first()
    );
}

#[test]
fn deep_else_if_chain_terminates_with_a_diagnostic() {
    // Twenty thousand `else if` links recurse `if_ -> if_` straight past both block
    // guards: each `then` block returns the shared depth to zero before the next
    // link, so the depth stayed flat while the call stack grew one frame per link,
    // and the chain overflowed the stack and raised SIGABRT. Counting the else-if
    // descent now feeds the shared ceiling and unwinds the chain into a diagnostic.
    // Within each link the condition parses before the descent, so at the boundary
    // the inner condition crosses the ceiling first and the surfaced diagnostic
    // names the expression, exactly as the deep if-block path does; the else-if
    // guard is what accumulates the depth to that point. The call returning at all
    // with a too-deep diagnostic is the proof it no longer aborts the process.
    let src = format!(
        "func f() -> void {{ if x {{}} {} }}",
        "else if x {} ".repeat(20_000)
    );
    let msgs = parse_within(&src);
    assert!(
        msgs.iter().any(|m| m.contains("nesting is too deep")),
        "deep else-if chain not reported as too deep: first = {:?}",
        msgs.first()
    );
}

#[test]
fn shallow_else_if_chain_parses_clean() {
    // Fifty `else if` links sit far below the ceiling, so the else-if guard must not
    // perturb an ordinary chain: this parses with no parser diagnostic.
    let src = format!(
        "func f() -> void {{ if x {{}} {} }}",
        "else if x {} ".repeat(50)
    );
    let msgs = parse_within(&src);
    assert!(
        msgs.is_empty(),
        "shallow else-if chain spuriously rejected: {msgs:?}"
    );
}

#[test]
fn shallow_block_nesting_parses_clean() {
    // Ten nested `if` blocks sit far below the ceiling, so the guard must not
    // perturb ordinary nested control flow: this parses with no parser diagnostic.
    let depth = 10;
    let src = format!(
        "func f() -> void {{ {}{} }}",
        "if x { ".repeat(depth),
        "}".repeat(depth)
    );
    let msgs = parse_within(&src);
    assert!(
        msgs.is_empty(),
        "shallow block nesting spuriously rejected: {msgs:?}"
    );
}

#[test]
fn valid_program_still_parses_clean() {
    // The invariant must not perturb well-formed input: a normal program parses
    // with no parser diagnostics.
    let src = "func main() -> int32 {\n  x := 1 + 2 * 3\n  return x\n}\n";
    let msgs = parse_within(src);
    assert!(
        msgs.is_empty(),
        "valid program spuriously rejected: {msgs:?}"
    );
}
