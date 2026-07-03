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
fn for_loop_over_array() {
    assert_eq!(run("forloop.dusk"), "6\n");
}

#[test]
fn main_argc_argv_builds_slice() {
    // With no extra arguments argv holds just the program name. Both runs share
    // one test, since each `dusk run` writes the same output binary path.
    assert_eq!(run("args.dusk"), "1\n");
    // `dusk run file.dusk a b` hands the trailing arguments to the program, so
    // argv counts the program name plus both of them.
    let bin = env!("CARGO_BIN_EXE_dusk");
    let path = format!("{}/examples/args.dusk", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(bin)
        .args(["run", &path, "a", "b"])
        .output()
        .expect("spawn dusk");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "3\n");
}

/// Runs `dusk check` on an example that must be rejected, returning stderr.
fn check_fails(example: &str) -> String {
    let bin = env!("CARGO_BIN_EXE_dusk");
    let path = format!("{}/examples/{}", env!("CARGO_MANIFEST_DIR"), example);
    let out = Command::new(bin)
        .args(["check", &path])
        .output()
        .expect("spawn dusk");
    assert!(!out.status.success(), "{example} must fail to check");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn bare_call_to_imported_private_name_is_rejected() {
    let err = check_fails("privacy_bare.dusk");
    assert!(err.contains("undefined name 'helper'"), "{err}");
}

#[test]
fn out_of_range_slice_faults() {
    let (out, err, ok) = run_raw("slicebound.dusk");
    assert!(!ok, "an out of range slice must fault");
    assert_eq!(out, "2\n", "the valid slice prints before the fault");
    assert!(err.contains("index out of bounds"), "{err}");
}

#[test]
fn double_join_faults() {
    let (out, err, ok) = run_raw("doublejoin.dusk");
    assert!(!ok, "a second join must fault");
    assert_eq!(out, "once\n", "the first join completes before the fault");
    assert!(err.contains("freed or stale pointer"), "{err}");
}

#[test]
fn cross_thread_use_after_free_faults() {
    // The free is ordered before the dereference by an atomic flag, so the
    // generation fault is deterministic, not a lucky race.
    let (out, err, ok) = run_raw("uafthread.dusk");
    assert!(!ok, "the thread's dereference of the freed pointer must fault");
    assert_eq!(out, "freeing\n");
    assert!(err.contains("freed or stale pointer"), "{err}");
}

#[test]
fn spawning_a_closure_variable_is_rejected() {
    let err = check_fails("spawnvar.dusk");
    assert!(err.contains("lambda literal"), "{err}");
}

#[test]
fn spawning_with_a_slice_capture_is_rejected() {
    let err = check_fails("spawncap.dusk");
    assert!(err.contains("cannot capture"), "{err}");
}

#[test]
fn using_a_pointer_after_sending_it_moved_is_rejected() {
    let err = check_fails("handoffuse.dusk");
    assert!(err.contains("moved pointer"), "{err}");
}

#[test]
fn sending_a_slice_through_a_channel_is_rejected() {
    let err = check_fails("chanview.dusk");
    assert!(err.contains("channel element"), "{err}");
}

#[test]
fn dereferencing_a_drained_recv_placeholder_faults_by_name() {
    let (out, err, ok) = run_raw("chandrain.dusk");
    assert!(!ok, "the null placeholder dereference must fault");
    assert_eq!(out, "1\n", "the drained error is visible before the fault");
    assert!(err.contains("dereference of a null pointer"), "{err}");
}

#[test]
fn freeing_a_held_mutex_faults() {
    let (out, err, ok) = run_raw("mutexheld.dusk");
    assert!(!ok, "freeing a held mutex must fault");
    assert_eq!(out, "locking\n", "the print before the free survives the abort");
    assert!(err.contains("mutex freed while held"), "{err}");
}

#[test]
fn unlocking_an_unheld_mutex_faults() {
    let (out, err, ok) = run_raw("mutexunlock.dusk");
    assert!(!ok, "unlocking an unheld mutex must fault");
    assert_eq!(out, "unlocking\n", "the print before the unlock survives the abort");
    assert!(err.contains("does not hold it"), "{err}");
}

#[test]
fn freeing_a_condvar_with_a_waiter_faults() {
    let (out, err, ok) = run_raw("condfree.dusk");
    assert!(!ok, "freeing a condvar with a parked waiter must fault");
    assert_eq!(out, "freeing\n", "the print before the free survives the abort");
    assert!(err.contains("condvar freed while threads wait"), "{err}");
}

#[test]
fn vector_get_out_of_bounds_faults() {
    let (out, err, ok) = run_raw("vecbound.dusk");
    assert!(!ok, "vec_get past the length must fault");
    assert_eq!(out, "20\n", "the valid get prints before the fault");
    assert!(err.contains("vector index out of bounds"), "{err}");
}

#[test]
fn foreign_calls_libc() {
    // A foreign block declares libc abs and labs, called like ordinary functions.
    assert_eq!(run("foreign.dusk"), "5\n7\n");
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
    // The free now runs the generation check, so a double free faults as a freed
    // or stale pointer at the free site, the same check a dereference runs.
    assert!(err.contains("freed or stale pointer"), "{err}");
}

#[test]
fn array_index_out_of_bounds_faults() {
    let (out, err, ok) = run_raw("bounds.dusk");
    assert!(!ok, "an out of bounds index must fault");
    assert_eq!(out, "1\n", "the in bounds index prints before the fault");
    assert!(err.contains("index out of bounds"), "{err}");
}

#[test]
fn stale_free_of_reused_block_faults() {
    let (out, err, ok) = run_raw("stalefree.dusk");
    assert!(!ok, "freeing a stale pointer to a reused block must fault");
    assert_eq!(out, "2\n", "q's valid deref prints before the stale free faults");
    assert!(err.contains("freed or stale pointer"), "{err}");
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
golden!(allocbig, "allocbig.dusk", "1\n4\n7\n");
golden!(spawnjoin, "spawnjoin.dusk", "worker\ndone\n");
golden!(atomiccount, "atomiccount.dusk", "20000\n");
golden!(capturecopy, "capturecopy.dusk", "6\n");
golden!(pipeline, "pipeline.dusk", "110\n");
golden!(fanin, "fanin.dusk", "820\n");
golden!(chanclose, "chanclose.dusk", "1\n2\n3\nclosed\n");
golden!(chanblock, "chanblock.dusk", "5050\n");
golden!(handoff, "handoff.dusk", "41\nhanded off\n");
golden!(countermutex, "countermutex.dusk", "10000\n");
golden!(bank, "bank.dusk", "60\n40\n100\n");
golden!(bounded, "bounded.dusk", "1275\n");
golden!(pingpong, "pingpong.dusk", "ping\npong\nping\npong\nping\npong\ndone\n");
golden!(display, "display.dusk", "point\npoint\n");
golden!(fmtesc, "fmtesc.dusk", "{}\na {b} c\n{} 1\n");
golden!(emptyerr, "emptyerr.dusk", "\nafter\n");
golden!(privacy, "privacy.dusk", "1\n2\n");

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
