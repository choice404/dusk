//! Golden integration tests: compile and run each example, check its stdout.
//! These exercise the whole pipeline end to end and guard against regressions.

use std::io::Write;
use std::process::{Command, Stdio};

/// Compiles and runs an example through the built `dusk` binary, returning its
/// stdout. Panics if the compiler itself fails.
fn run(example: &str) -> String {
    let bin = env!("CARGO_BIN_EXE_dusk");
    let path = format!("{}/examples/{}", env!("CARGO_MANIFEST_DIR"), example);
    let out = Command::new(bin)
        .arg("run")
        .arg(&path)
        .output()
        .expect("spawn dusk");
    assert!(
        out.status.success(),
        "{example} did not run cleanly: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Compiles and runs an example, returning its stdout, stderr, and whether it
/// exited cleanly. Unlike `run`, it tolerates a non zero exit, so a program that
/// faults at runtime, like a use after free, can be checked.
fn run_raw(example: &str) -> (String, String, bool) {
    let bin = env!("CARGO_BIN_EXE_dusk");
    let path = format!("{}/examples/{}", env!("CARGO_MANIFEST_DIR"), example);
    let out = Command::new(bin)
        .arg("run")
        .arg(&path)
        .output()
        .expect("spawn dusk");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn use_after_free_faults() {
    let (out, err, ok) = run_raw("uaf.dusk");
    assert!(!ok, "use after free must fault");
    assert_eq!(out, "42\n", "the valid deref prints before the fault");
    assert!(err.contains("use of a freed or stale pointer"), "{err}");
}

#[test]
fn double_free_faults() {
    let (out, err, ok) = run_raw("doublefree.dusk");
    assert!(!ok, "double free must fault");
    assert_eq!(out, "5\n");
    assert!(err.contains("double free"), "{err}");
}

#[test]
fn method_call_after_free_faults() {
    let (out, err, ok) = run_raw("methodfree.dusk");
    assert!(!ok, "a method call on a freed receiver must fault");
    assert_eq!(out, "7\n", "the valid call prints before the fault");
    assert!(err.contains("use of a freed or stale pointer"), "{err}");
}

macro_rules! golden {
    ($name:ident, $file:literal, $expected:literal) => {
        #[test]
        fn $name() {
            assert_eq!(run($file), $expected);
        }
    };
}

golden!(hello, "hello.dusk", "hello, world\n");
golden!(m5, "m5.dusk", "42\n55\n10\n");
golden!(m6, "m6.dusk", "7\n3\n100\n");
golden!(m6b, "m6b.dusk", "10\n40\n100\n99\n2\n99\n30\n129\n");
golden!(m6c, "m6c.dusk", "6\n3\n2\n4\n42\n");
golden!(m7, "m7.dusk", "75\n24\n0\n");
golden!(m7b, "m7b.dusk", "1\n2\n0\n99\n");
golden!(m7c, "m7c.dusk", "7\n2.5\n3\n4\n42\n99\n");
golden!(m7d, "m7d.dusk", "21\n21\n42\n");
golden!(m8, "m8.dusk", "70\n99\n");
golden!(m8b, "m8b.dusk", "105\n120\n42\n");
golden!(m8c, "m8c.dusk", "42\n");
golden!(m9, "m9.dusk", "2\n4\n6\n8\n10\n2\n4\n15\n120\n");
golden!(m9b, "m9b.dusk", "11\n21\n31\n21\n");
golden!(m9c, "m9c.dusk", "30\n5\n");
golden!(m9d, "m9d.dusk", "5\n-1\n0\n-2\n24\n");
golden!(m9e, "m9e.dusk", "30\n28\n");
golden!(app, "app.dusk", "42\n42\n99\n-5\n0\n5\n");
golden!(vec, "vec.dusk", "6\n0\n10\n20\n30\n40\n50\n");
golden!(allocator, "allocator.dusk", "24\n");
golden!(stdalloc, "stdalloc.dusk", "16\n");
golden!(arena_use, "arena_use.dusk", "16\n");
golden!(debugalloc, "debugalloc.dusk", "1\n1\n");
golden!(qualified, "qualified.dusk", "qualified\n9\n");
golden!(map, "map.dusk", "3\n1\n22\n3\n-1\n");
golden!(fileio, "fileio.dusk", "persisted\n9\n");
golden!(parse, "parse.dusk", "255\n255\n10\n15\n-42\n-1\n4\n-2\n");
golden!(printing, "printing.dusk", "score: 42\nabc\nAda is 36\n{braces} and 7\n");
golden!(strbuf, "strbuf.dusk", "dusk and dawn\n13\nhello, world\n");
golden!(genref, "genref.dusk", "10\n15\n3\n4\n30\n");
golden!(ownership, "ownership.dusk", "2\n2\n");

/// Runs an example feeding `input` to its stdin, so a program that reads with
/// `read_line` can be exercised deterministically from a pipe.
fn run_stdin(example: &str, input: &str) -> String {
    let bin = env!("CARGO_BIN_EXE_dusk");
    let path = format!("{}/examples/{}", env!("CARGO_MANIFEST_DIR"), example);
    let mut child = Command::new(bin)
        .arg("run")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dusk");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait dusk");
    assert!(
        out.status.success(),
        "input.dusk did not run cleanly: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn input_reads_lines_from_stdin() {
    let out = run_stdin("input.dusk", "Alice\nfoo\nbar\n");
    assert_eq!(
        out,
        "what is your name?\nAlice\nenter lines, end with ctrl-d:\nfoo\nbar\n2\n"
    );
}

#[test]
fn readnum_reads_typed_input() {
    let out = run_stdin("readnum.dusk", "21\n2.5\n");
    assert_eq!(out, "enter an int:\n42\nenter a float:\n3.5\n");
}
