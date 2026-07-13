//! Golden integration tests: compile and run each example, check its stdout.
//! These exercise the whole pipeline end to end and guard against regressions.

use std::io::Write;
use std::process::{Command, Stdio};

/// Resolves the compiler under test. `DUSK_BIN` overrides the cargo built
/// binary so the golden suite can run against a bootstrap stage.
fn dusk_bin() -> String {
    std::env::var("DUSK_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_dusk").to_string())
}

/// Compiles and runs an example through the built `dusk` binary, returning its
/// stdout. Panics if the compiler itself fails.
fn run(example: &str) -> String {
    let bin = dusk_bin();
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
    let bin = dusk_bin();
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
fn else_if_chain_selects_one_arm() {
    // An `else if` chain parses as an `else` whose body is a single nested `if`,
    // so a full chain fires exactly one arm and a chain with no tail `else` falls
    // through silently when nothing matches. `classify` prints one line per call;
    // `grade(50)` matches no arm and prints nothing, the last line here is `C`.
    assert_eq!(run("elseif.dusk"), "negative\nzero\nsmall\nbig\nA\nB\nC\n");
}

#[test]
fn else_if_condition_is_type_checked() {
    // The desugared inner `if` is checked like any other, so a non-bool `else if`
    // condition is rejected at the condition with a caret on it, not coerced.
    let err = check_fails("elseif_badcond.dusk");
    assert!(err.contains("if condition must be a bool"), "{err}");
    assert!(err.contains("} else if 3 {"), "missing source line: {err}");
}

#[test]
fn float32_prints_through_the_f64_printer() {
    // A float32 is fpext'd to double before the f64 runtime printer, direct,
    // computed, and through a format hole. Without the widening the module fails
    // to link, so a clean run is the whole point.
    assert_eq!(run("f32print.dusk"), "3.5\n7\ng = 3.5\n");
}

#[test]
fn fixed_array_len_is_the_element_count() {
    // `.len` on a fixed array is its compile-time length, not the silent zero the
    // struct-field fallback used to give.
    assert_eq!(run("arraylen.dusk"), "3\n5\n1\n");
}

#[test]
fn main_argc_argv_builds_slice() {
    // With no extra arguments argv holds just the program name. Both runs share
    // one test, since each `dusk run` writes the same output binary path.
    assert_eq!(run("args.dusk"), "1\n");
    // `dusk run file.dusk a b` hands the trailing arguments to the program, so
    // argv counts the program name plus both of them.
    let bin = dusk_bin();
    let path = format!("{}/examples/args.dusk", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(bin)
        .args(["run", &path, "a", "b"])
        .output()
        .expect("spawn dusk");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "3\n");
}

/// Runs `dusk check` on an example that must be rejected, returning stderr.
fn check_fails(example: &str) -> String {
    let bin = dusk_bin();
    let path = format!("{}/examples/{}", env!("CARGO_MANIFEST_DIR"), example);
    let out = Command::new(bin)
        .args(["check", &path])
        .output()
        .expect("spawn dusk");
    assert!(!out.status.success(), "{example} must fail to check");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs `dusk build` on an example that type-checks but must be refused by
/// codegen, returning stderr. This is the backstop class: the surface pass is
/// permissive, so `dusk check` passes, but the backend records a named build
/// error rather than emitting unsound IR.
fn build_fails(example: &str) -> String {
    let bin = dusk_bin();
    let path = format!("{}/examples/{}", env!("CARGO_MANIFEST_DIR"), example);
    let out = Command::new(bin)
        .args(["build", &path])
        .output()
        .expect("spawn dusk");
    assert!(!out.status.success(), "{example} must fail to build");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn bare_call_to_imported_private_name_is_rejected() {
    let err = check_fails("privacy_bare.dusk");
    assert!(err.contains("undefined name 'helper'"), "{err}");
    // The diagnostic carries a caret under the offending source line, not just
    // a line:col header.
    assert!(
        err.contains("println(helper())"),
        "missing source line: {err}"
    );
    assert!(err.contains("^~~~~~"), "missing caret run: {err}");
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
    assert!(
        !ok,
        "the thread's dereference of the freed pointer must fault"
    );
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
fn a_bare_function_bound_to_a_value_calls_in_return_position() {
    // `g := inc; return g(41)` and `f := id; return f(data)`: a plain top-level
    // function used as a value lowers to a closure over a null env and a
    // forwarding thunk, so the call dispatches through the closure path in both
    // scalar and fat-slice return shapes. Before the func-value lowering the
    // call was dropped and the return was a bogus `ret { ptr, i64 } 0`, which
    // clang rejected outright.
    assert_eq!(run("funcvalue.dusk"), "42\n111\n444\n");
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
    assert_eq!(
        out, "locking\n",
        "the print before the free survives the abort"
    );
    assert!(err.contains("mutex freed while held"), "{err}");
}

#[test]
fn unlocking_an_unheld_mutex_faults() {
    let (out, err, ok) = run_raw("mutexunlock.dusk");
    assert!(!ok, "unlocking an unheld mutex must fault");
    assert_eq!(
        out, "unlocking\n",
        "the print before the unlock survives the abort"
    );
    assert!(err.contains("does not hold it"), "{err}");
}

#[test]
fn pool_shutdown_from_a_pool_task_faults() {
    let (out, err, ok) = run_raw("poolself.dusk");
    assert!(!ok, "a pool task shutting the pool down must fault");
    assert_eq!(
        out, "submitting\n",
        "the print before the drain survives the abort"
    );
    assert!(
        err.contains("cannot shut down from inside a pool task"),
        "{err}"
    );
}

#[test]
fn freeing_a_condvar_with_a_waiter_faults() {
    let (out, err, ok) = run_raw("condfree.dusk");
    assert!(!ok, "freeing a condvar with a parked waiter must fault");
    assert_eq!(
        out, "freeing\n",
        "the print before the free survives the abort"
    );
    assert!(err.contains("condvar freed while threads wait"), "{err}");
}

#[test]
fn awaiting_a_consumed_future_faults() {
    let (out, err, ok) = run_raw("doubleawait.dusk");
    assert!(!ok, "the second await of one future must fault");
    assert_eq!(
        out, "1\n",
        "the first await's value prints before the fault"
    );
    assert!(err.contains("fatal: use of a dead future"), "{err}");
}

#[test]
fn awaiting_off_the_loop_thread_faults() {
    let (_, err, ok) = run_raw("offthreadawait.dusk");
    assert!(!ok, "an await from a spawned thread must fault");
    assert!(
        err.contains("fatal: the event loop was touched off its thread"),
        "{err}"
    );
}

#[test]
fn an_unfinishable_await_faults_instead_of_hanging() {
    let (_, err, ok) = run_raw("idledeadlock.dusk");
    assert!(!ok, "an await nothing can complete must fault");
    assert!(
        err.contains("fatal: the event loop is idle but work is still pending"),
        "{err}"
    );
}

#[test]
fn a_completer_exiting_reawakens_the_deadlock_gate() {
    let (_, err, ok) = run_raw("threadexitdeadlock.dusk");
    assert!(
        !ok,
        "the await must fault once its last possible completer exits"
    );
    assert!(
        err.contains("fatal: the event loop is idle but work is still pending"),
        "{err}"
    );
}

#[test]
fn a_future_before_loop_init_faults() {
    let (_, err, ok) = run_raw("noloop.dusk");
    assert!(!ok, "minting a future before loop_init must fault");
    assert!(
        err.contains("fatal: the event loop is not running"),
        "{err}"
    );
}

#[test]
fn a_reactor_stopped_with_an_armed_watch_faults() {
    let (out, err, ok) = run_raw("watchleak.dusk");
    assert!(
        !ok,
        "stopping the reactor with a watch still armed must fault"
    );
    assert_eq!(
        out, "await timed out\n0\n",
        "the timeout prints before the fault"
    );
    assert!(
        err.contains("fatal: the reactor stopped while a watch is still armed"),
        "{err}"
    );
}

#[test]
fn a_second_watch_on_an_armed_fd_faults() {
    let (out, err, ok) = run_raw("doublewatch.dusk");
    assert!(!ok, "a second watch on an already armed fd must fault");
    assert_eq!(
        out, "future is pending\n0\n",
        "the pending poll prints before the fault"
    );
    assert!(
        err.contains("fatal: the file descriptor already has an armed watch"),
        "{err}"
    );
}

#[test]
fn a_watch_on_an_invalid_fd_faults() {
    let (out, err, ok) = run_raw("badfdwatch.dusk");
    assert!(!ok, "arming a watch on an invalid fd must fault");
    assert_eq!(
        out, "arming\n",
        "the print before the arm survives the fault"
    );
    assert!(
        err.contains("fatal: a readiness watch was armed on an invalid file descriptor"),
        "{err}"
    );
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
fn collector_floor_smoke() {
    // The collected heap through its C floor: mint blocks, force a collection on
    // the main thread, hold one block across it. Nine blocks live before any
    // collection, one collection run, the held block survives with its contents,
    // and the program exits clean.
    assert_eq!(run("gcprobe.dusk"), "9\n1\n99\nok\n");
}

#[test]
fn std_memory_collector_wrappers_reach_the_collected_heap() {
    // The std.memory.collector surface, the stdlib twin of the gcprobe C floor.
    // Mint nine collected blocks through the collector<T> type, read the gauges
    // through the stdlib wrappers, force a collection through gc_collect, and hold
    // one block on the stack across it. Nine blocks and 72 payload bytes live
    // before the collection, one collection ran, every junk block was read (sum
    // 28), and the held block survives the sweep with its value intact. Proves the
    // control and stats wrappers resolve and reach the collected heap through the
    // import path.
    assert_eq!(run("stdcollector.dusk"), "9\n72\n1\n28\n99\nok\n");
}

#[test]
fn collector_anchor_is_set_once() {
    // A recursive main re-runs the anchor from an inner frame and collects deep
    // in the recursion. The anchor is set once, so the outer frame holding a
    // collected block stays in the scan and the block survives with its value
    // intact; a reset anchor would sweep it live and a reused slot would print
    // the churn value instead of 99.
    assert_eq!(run("gcreanchor.dusk"), "99\n");
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
    assert_eq!(
        out, "2\n",
        "q's valid deref prints before the stale free faults"
    );
    assert!(err.contains("freed or stale pointer"), "{err}");
}

#[test]
fn method_call_after_free_faults() {
    let (out, err, ok) = run_raw("methodfree.dusk");
    assert!(!ok, "a method call on a freed receiver must fault");
    assert_eq!(out, "7\n", "the valid call prints before the fault");
    assert!(err.contains("use of a freed or stale pointer"), "{err}");
}

#[test]
fn a_dynamic_shift_out_of_range_faults() {
    let (out, err, ok) = run_raw("shiftfault.dusk");
    assert!(!ok, "a shift amount at or past the width must fault");
    assert_eq!(out, "0\n", "the print before the shift survives the fault");
    assert!(err.contains("shift amount out of range"), "{err}");
}

#[test]
fn a_dynamic_negative_exponent_faults() {
    let (out, err, ok) = run_raw("powfault.dusk");
    assert!(!ok, "a negative dynamic integer exponent must fault");
    assert_eq!(out, "7\n", "the print before the pow survives the fault");
    assert!(err.contains("negative exponent in integer '**'"), "{err}");
}

#[test]
fn an_inclusive_slice_past_the_end_faults() {
    let (out, err, ok) = run_raw("inclbound.dusk");
    assert!(!ok, "xs[0..=len] must fault on the bounds check");
    assert_eq!(out, "3\n", "the valid slice length prints before the fault");
    assert!(err.contains("index out of bounds"), "{err}");
}

#[test]
fn bitwise_and_across_mismatched_widths_is_rejected() {
    let err = check_fails("bitmix.dusk");
    assert!(
        err.contains("'&' mixes int32 and int64; match the widths"),
        "{err}"
    );
}

#[test]
fn bitwise_and_on_bools_is_rejected() {
    let err = check_fails("boolbit.dusk");
    assert!(
        err.contains("bitwise operators need integer operands"),
        "{err}"
    );
}

#[test]
fn bitwise_not_on_a_float_is_rejected() {
    let err = check_fails("tildefloat.dusk");
    assert!(err.contains("'~' needs an integer operand"), "{err}");
}

#[test]
fn a_constant_negative_shift_amount_is_rejected() {
    let err = check_fails("shiftneg.dusk");
    assert!(err.contains("shift amount is negative"), "{err}");
}

#[test]
fn a_constant_oversize_shift_amount_is_rejected() {
    let err = check_fails("shiftwide.dusk");
    assert!(
        err.contains("shift amount 32 is out of range for int32"),
        "{err}"
    );
}

#[test]
fn a_constant_negative_integer_exponent_is_rejected() {
    let err = check_fails("powneg.dusk");
    assert!(
        err.contains("'**' on integers needs a nonnegative exponent"),
        "{err}"
    );
}

#[test]
fn pow_mixing_int_and_float_is_rejected() {
    let err = check_fails("powmix.dusk");
    assert!(
        err.contains("'**' needs two operands of the same numeric type"),
        "{err}"
    );
}

#[test]
fn compound_assignment_to_an_immutable_binding_is_rejected() {
    let err = check_fails("compoundmut.dusk");
    assert!(err.contains("cannot assign to immutable 'x'"), "{err}");
}

#[test]
fn compound_assignment_mixing_widths_is_rejected() {
    let err = check_fails("compoundmix.dusk");
    assert!(err.contains("mixes int32 and int64"), "{err}");
}

#[test]
fn incrementing_an_immutable_binding_is_rejected() {
    let err = check_fails("incimm.dusk");
    assert!(err.contains("cannot assign to immutable 'x'"), "{err}");
}

#[test]
fn increment_has_no_value_to_bind() {
    let err = check_fails("incval.dusk");
    assert!(
        err.contains("expected an expression, found PlusPlus"),
        "{err}"
    );
}

#[test]
fn a_pipe_into_a_non_callable_is_rejected() {
    let err = check_fails("pipebad.dusk");
    assert!(
        err.contains("the right side of '|>' must be a function name or call"),
        "{err}"
    );
}

// Track B back half: an async func lowers to a poll function over a heap frame,
// calling it mints a task and a future without running anything, and async_run
// cranks the loop until the task returns. These skeleton goldens have no awaits.

#[test]
fn async_empty_runs_through_async_run() {
    // amain prints "in" while the task runs and returns 7; main prints the 7 the
    // loop hands back after cranking the task to completion.
    assert_eq!(run("asyncempty.dusk"), "in\n7\n");
}

#[test]
fn async_args_land_at_the_frame_offsets_the_poll_reads() {
    // The call site writes the int64 and the string into the frame; the poll
    // echoes them, proving both sides agree on the parameter offsets. The task
    // returns a computed int64 the loop hands to main.
    assert_eq!(run("asyncargs.dusk"), "value=21\n42\n");
}

#[test]
fn async_lambda_env_lives_in_the_task_arena_not_a_frame_slot() {
    // A capturing lambda inside an async body allocates its environment from the
    // task env arena at its real size, so a two-capture env does not overwrite
    // the next frame slot and a single-capture env does not trip the word-only
    // frame path. two -> 10+5+7, one -> 10+5.
    assert_eq!(run("asynclambda.dusk"), "22\n15\n");
}

#[test]
fn await_chains_through_three_tasks() {
    // amain awaits mid, mid returns `await leaf`; leaf completes with (21, ok).
    // The tuple element destructures through amain, and each future is taken
    // exactly once as its task returns. Prints twice the value.
    assert_eq!(run("chain.dusk"), "42\n");
}

#[test]
fn two_tasks_fan_in_through_single_bind_awaits() {
    // Both futures are minted and started before the first await, so two tasks
    // are in flight; the single-bind form takes each element and they sum.
    assert_eq!(run("gofanin.dusk"), "42\n");
}

#[test]
fn awaiting_leaf_timers_yields_their_zero_completion() {
    // sleep_async mints a timer future the loop completes with 0; the two-bind
    // form takes the element and a null error word, in sequence.
    assert_eq!(run("awaitleaf.dusk"), "0\n0\n");
}

#[test]
fn a_void_async_func_runs_under_async_run() {
    // async_run of a void async func drives the loop with a raw scratch word
    // and a zero copy size; there is no result to load back, and the body
    // still suspends through a timer leaf before printing.
    assert_eq!(run("voidasyncrun.dusk"), "0\nvoid run done\nafter\n");
}

#[test]
fn returning_an_awaited_void_element_replays_the_bare_return() {
    // `return await g()` with a void element takes the completion and replays
    // the bare async return path; nothing is loaded from the element slot.
    assert_eq!(run("retawaitvoid.dusk"), "0\nh returned\n");
}

#[test]
fn async_offload_bridges_the_pool_and_sums_three_fetches() {
    // A fetch task parks on a leaf future a pool worker completes from another
    // thread; the loop's gate waits on the in-flight pool task rather than
    // faulting, and the completion wakes the parked task. Three fetches sum.
    assert_eq!(run("asyncoffload.dusk"), "60\n");
}

#[test]
fn mixed_width_tuple_members_adapt_to_the_declared_type() {
    // A tuple literal with a narrow-integer member builds as { i64, i64 } from
    // the default-i64 literals; the declared (int32, int64) / (int32, int8)
    // return forces a per-member width adapt so the aggregate matches. Sync and
    // async both exercised.
    assert_eq!(run("mixtuple.dusk"), "7\n100\n9\n3\n");
}

#[test]
fn non_escaping_fat_member_tuples_build_and_carry_both_words() {
    // A slice from a param carried through a tuple return keeps its { ptr, i64 }
    // whole; an array-literal member adapts to a slice member at the call site
    // (through adapt, so the fat conversion fires) without a type mismatch.
    assert_eq!(run("fattuple.dusk"), "20\n5\n106\n");
}

#[test]
fn shadowed_locals_across_awaits_each_get_a_distinct_slot() {
    // A same-named x bound in each if arm around an await, then in the outer
    // scope, then in a nested scope; each shadow is a distinct frame slot, so the
    // values never alias across a resume.
    assert_eq!(run("samename_locals.dusk"), "10\n20\n30\n");
}

#[test]
fn defers_replay_at_completion_in_reverse_across_awaits() {
    // Three defers registered before two awaits and an early return replay at
    // true completion, in reverse order, exactly once; free(p) reads the managed
    // pointer's frame slot and runs between the two printing defers, never at a
    // suspension.
    assert_eq!(run("defer_async.dusk"), "1\nfirst\nlast\n");
}

#[test]
fn per_iteration_closure_envs_stay_distinct_across_a_suspension() {
    // The C2 guard: a loop with an await creates one closure per iteration; each
    // iteration's env is a distinct cool_task_env_alloc, so the two stored
    // closures see distinct captured values. A reused frame slot would print 11
    // twice; the arena prints 0 then 11.
    assert_eq!(run("lambda_loop_async.dusk"), "0\n11\n");
}

#[test]
fn a_closure_env_survives_the_await_between_capture_and_call() {
    // The closure captures a local before an await and is called after it; the
    // env arena lives to task completion, so the capture is intact. 5 + 100.
    assert_eq!(run("lambda_in_async.dusk"), "105\n");
}

#[test]
fn a_boxed_interface_dispatches_across_a_suspension() {
    // The interface backing is a per-execution task-arena block living to task
    // completion, so a method dispatch after the await reads the boxed value.
    // 42 + 1.
    assert_eq!(run("iface_async.dusk"), "43\n");
}

#[test]
fn an_array_literal_slice_reads_across_a_suspension() {
    // The slice's backing is a per-execution task-arena block living to task
    // completion, so a read after the await is valid. 20 + 1.
    assert_eq!(run("arrayslice_async.dusk"), "21\n");
}

#[test]
fn an_array_literal_reassigns_into_a_slice_binding() {
    // The assignment path adapts an array literal into a slice, materializing a
    // backing and storing the fat pointer back into the binding's slot. Both
    // branches then index a valid slice: the reassigned literal sums to 6, the
    // untouched param slice sums to 60.
    assert_eq!(run("sliceassign.dusk"), "6\n60\n");
}

#[test]
fn per_iteration_boxed_interfaces_carry_distinct_backings() {
    // An async loop boxes a distinct value each iteration and stores it; each box
    // backing is a fresh cool_task_env_alloc, so the stored interfaces do not
    // alias one reused frame slot. 1, 11, 21 (not 21 thrice).
    assert_eq!(run("loopbox_async.dusk"), "1\n11\n21\n");
}

#[test]
fn per_iteration_slices_carry_distinct_backings() {
    // An async loop makes a slice from an array literal each iteration and stores
    // it; each backing is a distinct arena block, so the stored slices stay
    // distinct. 100, 110, 120 (not 120 thrice).
    assert_eq!(run("loopslice_async.dusk"), "100\n110\n120\n");
}

#[test]
fn an_array_literal_of_interface_elements_boxes_each_element() {
    // An array literal whose element type is an interface boxes each struct
    // element into its fat pointer, so the aggregate type matches and per-element
    // dispatch works.
    assert_eq!(run("arrayofiface.dusk"), "1\n2\n3\n");
}

#[test]
fn a_nested_array_of_interface_elements_boxes_through_both_levels() {
    // An array-of-array-of-interface: the (Array, Array) coerce recursion boxes
    // each innermost struct and matches the aggregate types through both levels.
    assert_eq!(run("nested_array_iface.dusk"), "1\n2\n3\n4\n");
}

#[test]
fn an_array_of_interface_slices_sizes_its_backing_at_the_fat_element() {
    // An array whose element is a slice-of-interface: each inner array builds at
    // the interface width and views as a slice, so a dispatch strides over fat
    // pointers, not off the end of a struct-sized backing. Was a silent SEGV.
    assert_eq!(run("array_of_iface_slices.dusk"), "1\n2\n3\n");
}

#[test]
fn an_array_of_interface_slices_dispatches_across_a_suspension() {
    // The same nested boxing in an async body: the arena-backed inner slices
    // survive the await and dispatch on the right vtable. 2 + 0.
    assert_eq!(run("nested_iface_async.dusk"), "2\n");
}

#[test]
fn an_enum_payload_of_interface_type_is_boxed() {
    // An enum variant whose payload is an interface boxes the concrete struct at
    // the constructor, so the match arm dispatches. 5.
    assert_eq!(run("enum_iface_payload.dusk"), "5\n");
}

#[test]
fn an_unqualified_enum_constructor_is_rejected() {
    // `Some(7)` written without its enum prefix is not a constructor. Sema refuses
    // the bare form and names the qualified fix `Opt.Some`, before any codegen
    // path can resolve the variant by its global name and collide with a like-
    // named function, a stale local out of scope, or an ambiguous generic
    // instance. The only supported spelling is the enum-qualified one.
    let err = check_fails("enum_bare_ctor_rejected.dusk");
    assert!(
        err.contains("use the qualified form 'Opt.Some' to construct an enum value"),
        "{err}"
    );
    assert_eq!(
        err.matches("is not a constructor").count(),
        1,
        "single diagnostic: {err}"
    );
}

#[test]
fn an_enum_constructor_with_the_wrong_arity_is_rejected() {
    // `Opt.Some` declares one payload field, so `Opt.Some()` with no argument is
    // refused at the constructor site, naming the arity, rather than slipping
    // through as the Unknown the constructor otherwise infers as.
    let err = check_fails("enum_arity.dusk");
    assert!(
        err.contains("'Opt.Some' takes 1 argument(s), but 0 were given"),
        "{err}"
    );
}

#[test]
fn an_enum_constructor_with_a_mistyped_payload_is_rejected() {
    // `Opt.Some` declares an int64 payload, so `Opt.Some(true)` hands a bool where
    // the int belongs and is refused at the constructor, rather than constructing
    // a mistyped value the match arm would read back at the wrong width.
    let err = check_fails("enum_payloadty.dusk");
    assert!(
        err.contains("argument 1 to 'Opt.Some' has the wrong type"),
        "{err}"
    );
}

#[test]
fn a_generic_call_that_pins_the_element_rejects_a_mistyped_ctor_payload() {
    // FIX-A: `keep(0, ...)` pins the element to int64, so `Box.Has(true)` hands a
    // bool where int64 belongs. The generic payload wildcards at the surface, so
    // the ground type re-check names the mismatch instead of relabeling the bool.
    let err = check_fails("enum_relabel_fail.dusk");
    assert!(
        err.contains("argument 1 to 'Box.Has' has the wrong type"),
        "{err}"
    );
}

#[test]
fn an_uninferable_enum_ctor_argument_is_rejected() {
    // FIX-B: `Opt.None` at a generic argument pins nothing, so `take(Opt.None)`
    // reports the uninferable parameter at the call rather than laundering a silent
    // int64 default through the argument.
    let err = check_fails("enum_none_arg_fail.dusk");
    assert!(
        err.contains("cannot infer the type parameter 'T' for 'take'"),
        "{err}"
    );
}

#[test]
fn an_error_laundered_through_a_generic_call_is_still_unhandled() {
    // FIX-C: `sink(fst(e, e2))` hands sink the passthrough result but never hands
    // off either error, so the discharge is narrowed to a bare binding and both
    // stay pending.
    let err = check_fails("err_launder_fail.dusk");
    assert!(err.contains("'e' is never handled"), "{err}");
    assert!(err.contains("'e2' is never handled"), "{err}");
}

#[test]
fn an_error_read_into_a_fresh_error_is_still_unhandled() {
    // FIX-C: `sink(remap(e))` reads e into a fresh error, so e is never handed off
    // and stays pending.
    let err = check_fails("err_makelaunder_fail.dusk");
    assert!(err.contains("'e' is never handled"), "{err}");
}

#[test]
fn an_error_laundered_through_a_method_argument_is_still_unhandled() {
    // FIX-D: the method discharge is narrowed too, so `l.note(remap(e))` hands note
    // a fresh error and e stays pending.
    let err = check_fails("err_method_fail.dusk");
    assert!(err.contains("'e' is never handled"), "{err}");
}

#[test]
fn a_nested_generic_ctor_payload_of_the_wrong_width_is_rejected() {
    // FIX-E with FIX-A: the outer ctor threads `Opt<int32>` as the inner payload's
    // expected, but a declared int64 does not fit an int32 slot, so the ground type
    // re-check names the mismatch instead of letting clang fault.
    let err = check_fails("enum_nested_width_fail.dusk");
    assert!(
        err.contains("argument 1 to 'Opt.Some' has the wrong type"),
        "{err}"
    );
}

#[test]
fn an_error_parameter_dropped_in_the_callee_is_rejected() {
    // FIX-1: an error parameter carries a must-handle obligation, so a callee that
    // drops it is rejected, closing the hand-off launder where the caller's
    // obligation is discharged while no one inspects the error.
    let err = check_fails("err_param_unhandled_fail.dusk");
    assert!(err.contains("the error 'err' is never handled"), "{err}");
}

#[test]
fn a_monad_bind_with_an_undetermined_result_element_is_rejected() {
    // FIX-4: the phantom-source suppression applies only when the monad op's result
    // element is determined. A do-chain that returns its empty source directly
    // leaves the whole element uninferable, so it is reported, not defaulted.
    let err = check_fails("monad_live_element_fail.dusk");
    assert!(
        err.contains("cannot infer the type parameter 'A' for 'Evil.bind'"),
        "{err}"
    );
}

#[test]
fn a_struct_field_of_interface_type_is_boxed() {
    // A struct literal boxes a concrete value into an interface field. 9.
    assert_eq!(run("struct_iface_field.dusk"), "9\n");
}

#[test]
fn a_struct_field_of_slice_type_views_an_array_literal() {
    // A struct literal views an array-literal field as a slice, carrying data and
    // length. 5, 6.
    assert_eq!(run("struct_slice_field.dusk"), "5\n6\n");
}

#[test]
fn a_ragged_three_level_nested_interface_array_builds() {
    // The declared element type threads into each nested literal, so ragged
    // sibling lengths view as uniform slices instead of clashing on a guessed
    // fixed length. 1, 2, 3, 4.
    assert_eq!(run("ragged_nested_array.dusk"), "1\n2\n3\n4\n");
}

#[test]
fn an_async_func_can_recurse() {
    // Each recursive call mints its own task and frame; the framesize constant is
    // loaded at every call site, so a self-call needs no emission order. 5+4+3+2+1.
    assert_eq!(run("recursion_async.dusk"), "15\n");
}

#[test]
fn two_async_workers_interleave_in_a_deterministic_fifo_order() {
    // The single loop thread, the FIFO ready queue, and one turn per await make
    // the interleave exact: each worker yields at each await tick().
    assert_eq!(run("roundrobin.dusk"), "a0\nb0\na1\nb1\na2\nb2\n");
}

#[test]
fn a_double_await_of_one_future_faults() {
    // The first await retires the record generationally, so the second, on a copy
    // of the binding, faults by name.
    let (_, err, ok) = run_raw("doubleawaitasync.dusk");
    assert!(!ok, "double await must fault");
    assert!(err.contains("fatal: use of a dead future"), "{err}");
}

#[test]
fn two_tasks_awaiting_one_future_fault() {
    // A future carries a single awaiter; the second task parking on one record
    // faults by name.
    let (_, err, ok) = run_raw("twowaiters.dusk");
    assert!(!ok, "two waiters must fault");
    assert!(err.contains("fatal: two tasks await one future"), "{err}");
}

#[test]
fn async_run_re_entry_faults() {
    // A sync helper calling async_run while the loop is already cranking is
    // refused by name.
    let (_, err, ok) = run_raw("asyncrunreenter.dusk");
    assert!(!ok, "async_run re-entry must fault");
    assert!(
        err.contains("fatal: async_run re-entered the event loop"),
        "{err}"
    );
}

#[test]
fn await_inside_a_while_reenters_after_each_resume() {
    // The loop back-edge re-enters the body after every resume; the counter
    // lives in a frame slot and the state re-stores each turn.
    assert_eq!(run("await_in_while.dusk"), "0\n1\n2\n");
}

#[test]
fn await_inside_both_if_arms() {
    // The condition is an awaited value; the taken arm's await runs while both
    // resume labels sit in the switch.
    assert_eq!(run("await_in_if.dusk"), "10\n");
}

#[test]
fn await_inside_a_for_over_a_named_array() {
    // B0's spilled data pointer, length, and index reload per block, so the for
    // body re-entered after a resume reads live values. Named array, not a
    // literal (escaping backings are B6).
    assert_eq!(run("await_in_for.dusk"), "60\n");
}

#[test]
fn await_inside_a_match_arm_reads_the_payload_after_resume() {
    // The arm binds a payload, awaits, then uses the bind; B0's payload copy
    // survives the resume edge, so the read is 21, not stale data. 21 + 5.
    assert_eq!(run("await_in_match.dusk"), "26\n");
}

// Track B front half: the async/await checker rejects every illegal async
// construct. Each twin is a legal-except-one-thing program, so the asserted
// diagnostic is the one that actually fires, not an earlier undefined name or
// import failure. These are compile-fail only; the matching positive goldens
// wait on codegen lowering, which does not exist yet.

#[test]
fn await_outside_an_async_func_is_rejected() {
    let err = check_fails("asyncawaitoutside.dusk");
    assert!(
        err.contains("'await' is only legal inside an async func"),
        "{err}"
    );
}

#[test]
fn await_mid_expression_is_rejected() {
    let err = check_fails("asyncawaitmidexpr.dusk");
    assert!(
        err.contains("'await' cannot appear mid-expression"),
        "{err}"
    );
}

#[test]
fn await_inside_a_lambda_is_rejected() {
    let err = check_fails("asyncawaitinlambda.dusk");
    assert!(err.contains("a lambda cannot await"), "{err}");
}

#[test]
fn await_under_defer_is_rejected() {
    let err = check_fails("asyncawaitdefer.dusk");
    assert!(err.contains("'await' cannot appear under defer"), "{err}");
}

#[test]
fn main_cannot_be_async() {
    let err = check_fails("asyncmain.dusk");
    assert!(err.contains("main cannot be async"), "{err}");
}

#[test]
fn an_async_func_cannot_take_type_parameters() {
    let err = check_fails("asyncgeneric.dusk");
    assert!(
        err.contains("an async func cannot take type parameters"),
        "{err}"
    );
}

#[test]
fn an_async_func_cannot_take_a_slice_param() {
    let err = check_fails("asyncsliceparam.dusk");
    assert!(err.contains("an async func cannot take 'xs'"), "{err}");
}

#[test]
fn an_async_func_cannot_take_a_future_param() {
    let err = check_fails("asyncfutureparam.dusk");
    assert!(
        err.contains("a future belongs to the event loop thread"),
        "{err}"
    );
}

#[test]
fn an_async_name_in_value_position_is_rejected() {
    let err = check_fails("asyncnamevalue.dusk");
    assert!(
        err.contains("'g' is async; call it with await or start it with async_run"),
        "{err}"
    );
}

#[test]
fn an_unhandled_error_from_an_await_is_rejected() {
    let err = check_fails("asyncunhandlederr.dusk");
    assert!(err.contains("the error 'e' is never handled"), "{err}");
}

#[test]
fn an_unused_bound_future_is_rejected() {
    let err = check_fails("asyncunused.dusk");
    assert!(err.contains("unused variable 'fa'"), "{err}");
}

#[test]
fn a_bare_async_call_that_is_never_awaited_is_rejected() {
    let err = check_fails("asyncbaredisard.dusk");
    assert!(
        err.contains("the future from 'g' is never awaited"),
        "{err}"
    );
}

#[test]
fn spawn_capturing_a_future_is_rejected() {
    let err = check_fails("asyncspawnfuture.dusk");
    assert!(
        err.contains("spawn cannot capture 'f': a future belongs to the event loop thread"),
        "{err}"
    );
}

#[test]
fn submit_capturing_a_future_is_rejected() {
    let err = check_fails("asyncsubmitfuture.dusk");
    assert!(
        err.contains("submit cannot capture 'f': a future belongs to the event loop thread"),
        "{err}"
    );
}

// The three twins of the future-in-a-container goldens: widening where a future
// may be stored, passed, or annotated left the guard rails that watch every
// other future position firing unchanged.

#[test]
fn a_future_stored_but_never_awaited_is_still_dropped() {
    // futurefan stores futures in a vector, but a bare async call whose future is
    // never bound is still discarded before it can be awaited or released.
    let err = check_fails("futuredrop.dusk");
    assert!(
        err.contains("the future from 'one' is never awaited"),
        "{err}"
    );
}

#[test]
fn spawn_capturing_a_relayable_future_is_still_rejected() {
    // futurearg passes a future to a same-thread relay, but a spawn still cannot
    // capture one into a worker thread.
    let err = check_fails("futurespawn.dusk");
    assert!(
        err.contains("spawn cannot capture 'f': a future belongs to the event loop thread"),
        "{err}"
    );
}

#[test]
fn a_frame_viewing_future_element_in_a_container_is_still_rejected() {
    // A Future<int64[]> stored for a Vector still trips the future-element ban at
    // its minting site; the container position does not launder a frame-viewing
    // element.
    let err = check_fails("futureframe.dusk");
    assert!(
        err.contains("a future element cannot contain a slice, closure, or interface value"),
        "{err}"
    );
}

#[test]
fn async_run_inside_an_async_func_is_rejected() {
    let err = check_fails("asyncruninside.dusk");
    assert!(
        err.contains("async_run cannot be called inside an async func"),
        "{err}"
    );
}

#[test]
fn async_run_of_a_bound_future_is_rejected() {
    let err = check_fails("asyncrunnondirect.dusk");
    assert!(
        err.contains("async_run takes a direct call of an async func"),
        "{err}"
    );
}

#[test]
fn using_a_pointer_moved_into_an_awaited_async_call_is_rejected() {
    let err = check_fails("asyncmovedptr.dusk");
    assert!(err.contains("use of a moved pointer"), "{err}");
}

#[test]
fn an_async_func_cannot_return_a_slice() {
    let err = check_fails("asyncsliceret.dusk");
    assert!(
        err.contains("an async func cannot return a slice, closure, or interface value"),
        "{err}"
    );
}

#[test]
fn a_void_await_that_discards_a_value_is_rejected() {
    let err = check_fails("asyncvoiddiscard.dusk");
    assert!(err.contains("'await f' discards a value"), "{err}");
}

#[test]
fn a_method_cannot_be_async() {
    let err = check_fails("asyncmethod.dusk");
    assert!(err.contains("a method cannot be async"), "{err}");
}

/// Runs `dusk check` on an example that must pass, asserting a clean exit. Used
/// for a front-end acceptance a golden cannot run yet because a matching codegen
/// path is still landing.
fn check_ok(example: &str) {
    let bin = dusk_bin();
    let path = format!("{}/examples/{}", env!("CARGO_MANIFEST_DIR"), example);
    let out = Command::new(bin)
        .args(["check", &path])
        .output()
        .expect("spawn dusk");
    assert!(
        out.status.success(),
        "{example} must check cleanly: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_slice_in_a_returned_tuple_that_escapes_is_rejected() {
    let err = check_fails("esctuple_slice.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_closure_escaping_through_a_binding_is_rejected() {
    let err = check_fails("esclosure_bind.dusk");
    assert!(
        err.contains("a closure that captures a local escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_local_capturing_closure_in_a_returned_tuple_is_rejected() {
    let err = check_fails("esctuple_closure.dusk");
    assert!(
        err.contains("a closure that captures a local escapes its frame"),
        "{err}"
    );
}

#[test]
fn an_interface_member_in_a_returned_tuple_is_rejected() {
    let err = check_fails("ifacetuple_ret.dusk");
    assert!(
        err.contains("an interface value inside a tuple is not supported"),
        "{err}"
    );
}

#[test]
fn an_interface_member_in_a_tuple_argument_is_rejected() {
    // The same rule as the return position, so the two are consistent.
    let err = check_fails("ifacetuple_arg.dusk");
    assert!(
        err.contains("an interface value inside a tuple is not supported"),
        "{err}"
    );
}

#[test]
fn a_non_escaping_slice_from_a_param_in_a_tuple_return_checks_ok() {
    // Proof the escape recursion does not over-reject: a slice a caller owns is a
    // legal tuple member.
    check_ok("tuple_sliceparam_ok.dusk");
}

// Interprocedural escape enforcement (M5). A frame-local view laundered through
// a call is caught by the escape summary: a call that returns one of its frame
// arguments is a returns-alias reject, a call that stores one into another
// argument's place is a flows-into reject. The messages name the arguments.
const RETURNS_VIEW: &str = "this call may return a view of argument";
const STORES_VIEW: &str = "view is stored into argument";

#[test]
fn a_frame_slice_laundered_through_a_passthrough_call_is_rejected() {
    let err = check_fails("call_passthrough.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_frame_slice_laundered_through_two_hops_is_rejected() {
    // Only the transitive summary fixpoint sees the escape across f -> g -> id.
    let err = check_fails("call_twohop.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_frame_slice_wrapped_in_a_tuple_by_a_callee_is_rejected() {
    // The reject twin of tuple_sliceparam_ok: same tuple wrap, frame-local arg.
    let err = check_fails("call_tuple.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_frame_slice_through_a_recursive_passthrough_is_rejected() {
    // Self-recursion: the summary climbs from bottom to returns-argument-0.
    let err = check_fails("call_recursive.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_frame_slice_through_a_mutually_recursive_passthrough_is_rejected() {
    // The mutual-recursion cycle converges to returns-argument-0 for both funcs.
    let err = check_fails("call_mutual.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_frame_slice_through_a_closure_value_call_is_rejected() {
    // An opaque closure callee gets the conservative TOP: it may return any arg.
    let err = check_fails("call_closure_callee.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_frame_slice_through_a_func_value_call_is_rejected() {
    // `f := id; return f(local[0..4])`: the func value dispatches through the
    // closure path, so the callee is opaque (TOP).
    let err = check_fails("call_funcvalue.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_frame_slice_stored_into_a_parameter_place_is_rejected() {
    // The store edge is caught at the call site: the slice flows into the vector
    // the caller owns through a pointer parameter.
    let err = check_fails("stash_param.dusk");
    assert!(err.contains(STORES_VIEW), "{err}");
}

#[test]
fn a_frame_slice_stored_into_a_returned_local_vector_is_rejected() {
    // The slice is pushed into a local vector, which is then returned; the store
    // edge that polluted the vector names the escape at the returned pointer.
    let err = check_fails("stash_vector.dusk");
    assert!(err.contains(STORES_VIEW), "{err}");
}

#[test]
fn a_passthrough_of_a_slice_the_frame_does_not_own_checks_ok() {
    // The accept side of the interprocedural line: relay returns its PARAMETER
    // slice, whose backing the caller owns, so the passthrough is not rejected.
    check_ok("passthrough_ok.dusk");
}

#[test]
fn a_frame_view_laundered_through_a_call_but_used_in_frame_checks_ok() {
    // A frame view returned by a passthrough but consumed within the owning frame
    // never dangles, so it stays accepted.
    check_ok("calluse_local.dusk");
}

#[test]
fn an_escaping_slice_tuple_returned_by_name_is_rejected() {
    let err = check_fails("esctuple_slice_bind.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn an_escaping_slice_tuple_laundered_through_an_alias_is_rejected() {
    let err = check_fails("esctuple_slice_alias.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn an_escaping_closure_tuple_returned_by_name_is_rejected() {
    let err = check_fails("esctuple_closure_bind.dusk");
    assert!(
        err.contains("a closure that captures a local escapes its frame"),
        "{err}"
    );
}

#[test]
fn an_escaping_slice_tuple_built_by_a_match_is_rejected() {
    let err = check_fails("esctuple_match.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_param_slice_carried_through_a_tuple_binding_runs() {
    // The escape guard rejects frame-local slices in a tuple even via a binding,
    // but a param slice a caller owns must still build and run.
    assert_eq!(run("tuple_param_via_bind_ok.dusk"), "10\n5\n");
}

#[test]
fn reassigning_a_tuple_binding_to_an_escaping_value_is_rejected() {
    let err = check_fails("esctuple_reassign.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn reassigning_a_closure_binding_to_a_capturing_lambda_is_rejected() {
    let err = check_fails("esclosure_reassign.dusk");
    assert!(
        err.contains("a closure that captures a local escapes its frame"),
        "{err}"
    );
}

#[test]
fn re_slicing_a_local_array_literal_binding_in_a_tuple_is_rejected() {
    let err = check_fails("reslice_local_tuple.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_slice_of_a_slice_of_a_local_array_in_a_tuple_is_rejected() {
    let err = check_fails("reslice_slice_of_slice.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn an_inner_param_slice_shadowing_an_outer_local_array_runs() {
    // The scope-shadow no-over-reject: the inner binding's clean flag masks the
    // outer escaping one, so returning the inner name is legal.
    assert_eq!(run("scope_shadow_ok.dusk"), "1\n10\n");
}

#[test]
fn reassigning_a_tuple_binding_to_a_clean_value_runs() {
    // The reassign-to-clean no-over-reject: a mut binding that started escaping is
    // reassigned to a param slice, so the stale flag is cleared and the return is
    // legal. Now built and run too: the surface pass records the binding's slice
    // tuple storage, so codegen sizes the slot as a slice and the array-literal
    // initializer and the slice reassignment both store into it. s[0] is 10 from
    // the param slice, n is the reassigned 9.
    assert_eq!(run("tuple_reassign_clean_ok.dusk"), "10\n9\n");
}

#[test]
fn a_mutable_tuple_with_an_array_literal_member_stores_as_a_slice() {
    // The narrow mutable-tuple storage class: an unannotated `mut t := ([..], n)`
    // infers its array-literal member as a slice, since the later `t = (xs, m)`
    // stores one, so the slot is sized as a slice tuple. Reads the slice member's
    // element sum and the int member on both sides of the reassignment.
    assert_eq!(run("muttuple.dusk"), "6\n5\n15\n9\n");
}

#[test]
fn the_mutable_tuple_slice_storage_still_rejects_a_frame_escape() {
    // The storage reshape must not weaken the escape guard: the reshaped binding is
    // reassigned to another frame-local array tuple and returned, so its slice
    // member views a dead frame and the return is rejected exactly as before.
    let err = check_fails("muttuple_escape.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_mutable_tuple_slice_storage_runs_inside_an_async_body() {
    // The same class inside an async body, where every local slot is backed by the
    // task frame arena. The slice tuple slot is sized on the frame and both the
    // initializer and the reassignment store into it. Sums are 6+5 then 15+9.
    assert_eq!(run("muttuple_async.dusk"), "11\n24\n");
}

#[test]
fn a_conditional_reassignment_to_an_escaping_tuple_is_rejected() {
    let err = check_fails("flowmerge_if_tuple.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_conditional_re_slice_of_a_local_array_is_rejected() {
    let err = check_fails("flowmerge_reslice_in_if.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_conditional_reassignment_to_an_array_literal_slice_then_returned_is_rejected() {
    // The assignment-path array-literal coercion is legal, but the may-join keeps
    // r's escape flag raised, so returning the frame-local backing is still caught.
    let err = check_fails("sliceassign_escape.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_conditional_reassignment_to_a_capturing_lambda_is_rejected() {
    let err = check_fails("flowmerge_closure_if.dusk");
    assert!(
        err.contains("a closure that captures a local escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_reassignment_to_an_escaping_value_in_a_while_body_is_rejected() {
    let err = check_fails("flowmerge_while.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_reassignment_to_an_escaping_value_nested_in_two_ifs_is_rejected() {
    let err = check_fails("flowmerge_nested_if.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_reassignment_to_an_escaping_value_in_one_if_arm_is_rejected() {
    let err = check_fails("flowmerge_one_arm.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_conditional_reassignment_that_only_ever_holds_param_slices_runs() {
    // The may-join no-over-reject: both the initial and the branch value are param
    // slices, so the join must never flag t.
    assert_eq!(run("flowmerge_param_ok.dusk"), "40\n9\n");
}

#[test]
fn an_unconditional_reassignment_to_clean_after_a_branch_checks_ok() {
    // The straight-line overwrite no-over-reject: a branch may have made r escape,
    // but the unconditional reassign to a param slice after it is the last word.
    // Checked only, since assigning an array literal to a slice binding in the
    // branch is a separate codegen coercion concern.
    check_ok("reassign_clean_after_branch_ok.dusk");
}

#[test]
fn a_struct_with_a_slice_field_viewing_a_local_is_rejected() {
    let err = check_fails("escstruct_slice_field.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_struct_field_reassigned_to_a_local_view_is_rejected() {
    let err = check_fails("escstruct_field_reassign.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_struct_with_a_closure_field_capturing_a_local_is_rejected() {
    let err = check_fails("escstruct_closure_field.dusk");
    assert!(
        err.contains("a closure that captures a local escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_frame_local_struct_laundered_through_an_alias_is_rejected() {
    let err = check_fails("escstruct_via_alias.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_nested_struct_with_a_buried_local_view_is_rejected() {
    let err = check_fails("escstruct_nested.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_conditional_struct_field_store_of_a_local_view_is_rejected() {
    let err = check_fails("escstruct_branch_reassign.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_struct_with_a_slice_field_from_a_param_runs() {
    // The no-over-reject: a struct holding a caller-owned slice is a common legal
    // shape and must build and run.
    assert_eq!(run("struct_slice_from_param_ok.dusk"), "10\n");
}

#[test]
fn a_struct_with_a_non_capturing_closure_field_runs() {
    assert_eq!(run("struct_closure_from_param_ok.dusk"), "6\n");
}

#[test]
fn an_enum_with_a_slice_payload_viewing_a_local_is_rejected() {
    let err = check_fails("escenum_slice_payload.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_frame_local_enum_laundered_through_a_binding_is_rejected() {
    let err = check_fails("escenum_via_binding.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_conditional_enum_reassignment_to_a_local_payload_is_rejected() {
    let err = check_fails("escenum_branch.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_fixed_array_whose_elements_view_a_local_is_rejected() {
    let err = check_fails("escarray_slice_elems.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_frame_local_array_of_slices_via_a_binding_is_rejected() {
    let err = check_fails("escarray_via_binding.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_conditional_array_reassignment_to_local_element_views_is_rejected() {
    let err = check_fails("escarray_branch.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn an_enum_with_a_slice_payload_from_a_param_runs() {
    // The no-over-reject: an enum holding a caller-owned slice payload is legal.
    assert_eq!(run("enum_payload_from_param_ok.dusk"), "10\n");
}

#[test]
fn a_fixed_array_of_param_slices_runs() {
    // The no-over-reject: a by-value array of caller-owned slices is legal.
    assert_eq!(run("array_of_param_slices_ok.dusk"), "10\n");
}

#[test]
fn a_struct_field_of_enum_type_wrapping_a_local_is_rejected() {
    let err = check_fails("escdepth_struct_of_enum.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn an_enum_payload_of_enum_type_wrapping_a_local_is_rejected() {
    let err = check_fails("escdepth_enum_of_enum.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_struct_field_of_fat_array_type_viewing_a_local_is_rejected() {
    let err = check_fails("escdepth_struct_of_fatarray.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_generic_struct_field_burying_a_local_view_is_rejected() {
    let err = check_fails("escdepth_generic_box.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_nested_carrier_laundered_through_a_binding_is_rejected() {
    let err = check_fails("escdepth_via_binding.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_conditional_reassignment_of_a_nested_carrier_is_rejected() {
    let err = check_fails("escdepth_branch.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_struct_field_of_enum_type_from_a_param_runs() {
    // The no-over-reject at depth: an enum payload from a param is caller-owned.
    assert_eq!(run("depth_enum_from_param_ok.dusk"), "10\n");
}

#[test]
fn a_struct_field_of_fat_array_from_params_runs() {
    assert_eq!(run("depth_fatarray_from_param_ok.dusk"), "10\n");
}

#[test]
fn a_generic_struct_field_from_a_param_runs() {
    // The generic-burial no-over-reject: a param-backed generic field is legal.
    assert_eq!(run("depth_generic_from_param_ok.dusk"), "10\n");
}

#[test]
fn projecting_a_slice_field_out_of_a_local_struct_is_rejected() {
    let err = check_fails("escproj_field.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn projecting_an_element_out_of_a_local_array_of_slices_is_rejected() {
    let err = check_fails("escproj_index.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_match_arm_projecting_an_escaping_payload_is_rejected() {
    let err = check_fails("escproj_match.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_projection_placed_into_a_struct_field_is_rejected() {
    let err = check_fails("escproj_into_struct.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn returning_an_interface_value_by_value_is_rejected() {
    let err = check_fails("esciface_return.dusk");
    assert!(
        err.contains("returning an interface value is not supported"),
        "{err}"
    );
}

#[test]
fn a_slice_of_concrete_structs_as_a_slice_of_interface_is_rejected() {
    let err = check_fails("slicecovariance.dusk");
    assert!(
        err.contains("cannot pass a slice of 'Box' as a slice of interface 'Sized'"),
        "{err}"
    );
}

#[test]
fn a_method_call_lowers_its_receiver_exactly_once() {
    // The discard idiom `make().ignore()` evaluates `make()` a single time. Before
    // the fix the base was lowered, the method failed to resolve, and the generic
    // call path re-lowered the whole field expression, running the side effect
    // twice; `made` would print twice.
    assert_eq!(run("methodbaseonce.dusk"), "made\ndone\n");
}

#[test]
fn an_unresolvable_method_is_a_named_build_error_not_a_silent_zero() {
    // `toString` on a plain struct with no Display impl has no lowering. The
    // surface pass is permissive (the result is Unknown), so `dusk check` passes,
    // but codegen must refuse the module with a named error rather than emit a
    // garbage zero and evaluate the receiver twice.
    let err = build_fails("methodunresolved_fail.dusk");
    assert!(err.contains("no method 'toString' on type 'V'"), "{err}");
}

#[test]
fn projecting_a_slice_field_out_of_a_param_backed_struct_runs() {
    // The no-over-reject for projections: a member of a param-backed aggregate is
    // caller-owned and legal to return.
    assert_eq!(run("proj_from_param_ok.dusk"), "10\n");
}

#[test]
fn indexing_a_slice_field_of_a_local_struct_is_rejected() {
    let err = check_fails("escsliceidx.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_slice_index_projection_via_a_binding_is_rejected() {
    let err = check_fails("escsliceidx_via_binding.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_slice_of_structs_into_a_struct_field_of_interface_is_rejected() {
    let err = check_fails("covariance_field.dusk");
    assert!(
        err.contains("cannot pass a slice of 'Box' as a slice of interface 'Sized'"),
        "{err}"
    );
}

#[test]
fn a_slice_of_structs_into_an_enum_payload_of_interface_is_rejected() {
    let err = check_fails("covariance_enum.dusk");
    assert!(
        err.contains("cannot pass a slice of 'Box' as a slice of interface 'Sized'"),
        "{err}"
    );
}

#[test]
fn a_slice_of_structs_assigned_to_a_slice_of_interface_binding_is_rejected() {
    let err = check_fails("covariance_assign.dusk");
    assert!(
        err.contains("cannot pass a slice of 'Box' as a slice of interface 'Sized'"),
        "{err}"
    );
}

#[test]
fn indexing_a_param_slice_of_slices_runs() {
    // The no-over-reject for slice-index projection: a param-backed inner slice.
    assert_eq!(run("sliceidx_param_ok.dusk"), "10\n");
}

#[test]
fn a_slice_of_interface_passed_as_a_slice_of_interface_runs() {
    // The no-over-reject for covariance: same-type slice passing is not a reinterpret.
    assert_eq!(run("covariance_same_type_ok.dusk"), "42\n");
}

#[test]
fn a_slice_of_structs_at_a_method_arg_of_interface_is_rejected() {
    // M3: the method-arg site now runs the same covariance guard as a direct call,
    // so this reject lands as a clean diagnostic (exit 1) instead of the codegen
    // backstop panic (exit 101) this slice-value path used to reach.
    let err = check_fails("methcov_fail.dusk");
    assert!(
        err.contains("cannot pass a slice of 'Sq' as a slice of interface 'Shape'"),
        "{err}"
    );
}

#[test]
fn an_array_literal_at_a_method_arg_of_interface_runs() {
    // The accept twin: an array literal at a slice-of-interface method argument
    // boxes each element into a fat pointer, a real slice of interfaces.
    assert_eq!(run("methcov_ok.dusk"), "25\n");
}

#[test]
fn an_unknown_field_on_an_error_is_rejected() {
    // M2: an error carries only `message`; any other field is rejected in sema
    // rather than silently lowered to a zero.
    let err = check_fails("errmessage_field_fail.dusk");
    assert!(
        err.contains("error has no field 'code'; it carries only 'message'"),
        "{err}"
    );
}

#[test]
fn a_slice_of_structs_at_an_interface_receiver_method_is_rejected() {
    // FIX-A: a dynamic-dispatch call recovers its erased interface receiver's name
    // to run the covariance guard, so a slice of concrete structs at an interface
    // method's slice-of-interface parameter is a clean diagnostic, not the codegen
    // backstop panic (exit 101) this path used to reach.
    let err = check_fails("dyncovfail.dusk");
    assert!(
        err.contains("cannot pass a slice of 'Sq' as a slice of interface 'Shape'"),
        "{err}"
    );
}

#[test]
fn a_same_type_slice_at_an_interface_receiver_method_checks_clean() {
    // The accept twin: the covariance guard does not over-reject a genuine
    // slice-of-interface at an interface method. (A run is blocked by the separate,
    // conservative escape guard on interface calls, so this is a clean-check accept.)
    check_ok("dyncovok.dusk");
}

#[test]
fn a_map_laundered_slice_at_an_interface_method_faults_cleanly() {
    // FIX-A backstop, the HIGH repro: a heap slice from `map` erases its element
    // type, so no checker sink can see the concrete element, and the slice dodges
    // escape. The construct that `dusk check` accepts must not panic at codegen;
    // the covariance backstop is now a clean, named build error (exit 1), not a
    // Rust panic (exit 101).
    let (_out, err, ok) = run_raw("dyncovmap_fail.dusk");
    assert!(!ok, "expected a build fault, got a clean run");
    assert!(
        err.contains("cannot pass a slice of 'Sq' as a slice of interface 'Shape'"),
        "{err}"
    );
}

#[test]
fn assigning_to_an_errors_message_is_rejected() {
    // FIX-B: an error's message is a read-only pointer with no writable place, so
    // `e.message = ...` has no store to lower; sema refuses it rather than letting
    // codegen silently drop the write.
    let err = check_fails("errmsgassign_fail.dusk");
    assert!(
        err.contains("an error's message is read only; build a new error instead"),
        "{err}"
    );
}

#[test]
fn a_slice_of_structs_in_a_tuple_member_of_interface_is_rejected() {
    // FIX-D: the covariance guard descends a tuple literal, so a slice of concrete
    // structs at a slice-of-interface tuple member is caught at check.
    let err = check_fails("covtuple_fail.dusk");
    assert!(
        err.contains("cannot pass a slice of 'Sq' as a slice of interface 'Shape'"),
        "{err}"
    );
}

#[test]
fn a_slice_of_structs_as_an_array_of_slices_element_is_rejected() {
    // FIX-D: the covariance guard descends an array literal element by element, so
    // a slice of concrete structs at a slice-of-interface array element is caught.
    let err = check_fails("covarr_fail.dusk");
    assert!(
        err.contains("cannot pass a slice of 'Sq' as a slice of interface 'Shape'"),
        "{err}"
    );
}

#[test]
fn a_slice_of_structs_at_a_function_value_argument_faults_cleanly() {
    // FIX-D backstop: a function value's call is indirect, so its parameter type is
    // erased and the checker cannot see the slice-of-interface element. The codegen
    // covariance backstop is now a clean, named build error (exit 1), not a panic.
    let (_out, err, ok) = run_raw("covfnval_fail.dusk");
    assert!(!ok, "expected a build fault, got a clean run");
    assert!(
        err.contains("cannot pass a slice of 'Sq' as a slice of interface 'Shape'"),
        "{err}"
    );
}

#[test]
fn a_do_over_a_struct_with_no_monad_block_is_rejected() {
    // F-M1's generic `bind`/`unit` only exist once a type opts in with a
    // `monad Name { ... }` block; `do Foo { ... }` over a plain generic struct
    // desugars to `Foo.bind`/`Foo.unit`, which name nothing.
    let err = check_fails("genericmaybebad.dusk");
    assert!(err.contains("undefined name 'Foo.bind'"), "{err}");
    assert!(err.contains("undefined name 'Foo.unit'"), "{err}");
}

#[test]
fn a_do_over_a_monad_bind_with_the_wrong_arity_is_rejected() {
    // Every real monad's bind takes the value and the continuation; a bind
    // missing the continuation parameter cannot back a multi-step `do`, so the
    // desugared call passing both arguments is an arity mismatch.
    let err = check_fails("doasyncbad.dusk");
    assert!(err.contains("expected 1 argument(s), found 2"), "{err}");
}

#[test]
fn regression_pin_generic_do_width_mismatch_is_rejected() {
    // REGRESSION-PIN (0.4.3 F-M1 Option C): before the fix, a generic `do`'s
    // continuation body escaped width/type checking entirely (`Ty::Unknown`),
    // so mixing int32 and int64 inside the continuation silently truncated
    // instead of being rejected. The types-only re-check over the
    // mono-expanded module must catch this like any other arithmetic
    // mismatch, or the miscompile is back.
    let err = check_fails("genericwidth.dusk");
    assert!(
        err.contains("arithmetic mixes int32 and int64; match the widths"),
        "{err}"
    );
}

#[test]
fn regression_pin_generic_do_annotation_element_clash_is_rejected() {
    // REGRESSION-PIN (0.4.3 F-M1 Option C): before the fix, a generic `do`
    // binding's annotation could clash with the element type produced inside
    // the `do` and reach clang unchecked. The types-only re-check must reject
    // it at `dusk check`.
    let err = check_fails("genericpin.dusk");
    assert!(
        err.contains("return type does not match the function's return type"),
        "{err}"
    );
}

#[test]
fn a_malformed_interface_body_is_rejected_in_bounded_time() {
    // A stray `func` where a method name is expected must not spin the parser.
    let err = check_fails("malformed_interface.dusk");
    assert!(err.contains("unexpected token in interface body"), "{err}");
}

#[test]
fn a_malformed_foreign_body_is_rejected_in_bounded_time() {
    let err = check_fails("malformed_foreign.dusk");
    assert!(err.contains("unexpected token in foreign block"), "{err}");
}

#[test]
fn a_struct_field_holding_a_capturing_lambda_returned_out_of_its_frame_is_rejected() {
    // std.functional.io's IO is lazy through a collected thunk field; a plain
    // function-typed field cannot hold a suspended thunk that escapes the frame
    // that built it, which is why the collector kinds exist. A capturing lambda
    // stored in a bare `() -> T` field and returned out of its constructing
    // function must be rejected the same way any other closure escape is.
    let err = check_fails("iomonadbad.dusk");
    assert!(
        err.contains("a closure that captures a local escapes its frame; it cannot be returned"),
        "{err}"
    );
}

#[test]
fn a_do_io_continuation_capturing_a_frame_view_is_rejected() {
    // The lazy IO do desugar mints each continuation into a closure collector that
    // outlives the frame, so a continuation capturing a frame-view slice would
    // dangle the captured fat pointer. The mint is rejected, naming the capture.
    // The accept twin is iomonad, whose continuations capture only scalars.
    let err = check_fails("lazyiocap_fail.dusk");
    assert!(
        err.contains("cannot collect a closure that captures 's': it may view a frame"),
        "{err}"
    );
}

#[test]
fn io_pure_over_a_frame_view_slice_is_rejected() {
    // io_pure lifts its argument into a collected thunk that captures it, so a
    // slice viewing a local array would leave a fat pointer into a dead frame once
    // the thunk outlives io_pure. The caller-side mint check rejects the argument,
    // proving genericity does not launder the capture past the ground pass.
    let err = check_fails("iopureslice_fail.dusk");
    assert!(
        err.contains("'io_pure' collects 's', but it holds a view of the frame"),
        "{err}"
    );
}

#[test]
fn a_submit_capturing_an_io_value_is_rejected() {
    // An IO<T> holds a collected thunk, so it is confined to the main thread; a
    // pool worker runs on another thread, where the collected environment would be
    // swept while parked. The submit capture is refused, naming the IO value.
    let err = check_fails("iosubmit_fail.dusk");
    assert!(
        err.contains("submit cannot capture 'm': a collected value stays on the main thread; it cannot cross to another thread"),
        "{err}"
    );
}

#[test]
fn a_channel_of_io_values_is_rejected() {
    // An IO<T> holds a collected thunk, so it stays on the main thread; a channel
    // carries its element to another thread, where the suspended environment would
    // sit unrooted in the ring and be swept while live. The element type is refused
    // at the mint.
    let err = check_fails("iochan_fail.dusk");
    assert!(
        err.contains("a collected value stays on the main thread; it cannot cross through a channel to another thread"),
        "{err}"
    );
}

// M5 gate false-accept fixes: each family of view-laundering the escape analysis
// missed, now rejected. The heap-graph launderings (Family A) read a frame view
// back out of a heap object a store edge polluted; the point fixes catch a
// loop-carried alias chain, a higher-order element passthrough, a non-literal
// tuple destructure, a for-loop variable, a re-sliced call result, and a store
// through a borrowed-parameter pointer.

#[test]
fn a_frame_view_read_back_out_of_a_vector_is_rejected() {
    let err = check_fails("escvecget_readback.dusk");
    assert!(err.contains("escapes its frame"), "{err}");
}

#[test]
fn allocating_a_struct_whose_slice_field_views_a_local_is_rejected() {
    let err = check_fails("escalloc_view.dusk");
    assert!(
        err.contains("this returns a pointer to an object that stores a view of the current frame"),
        "{err}"
    );
}

#[test]
fn returning_a_polluted_pointer_through_move_is_rejected() {
    let err = check_fails("escmove_polluted.dusk");
    assert!(
        err.contains("this returns a pointer to an object that stores a view of the current frame"),
        "{err}"
    );
}

#[test]
fn returning_a_polluted_pointer_through_a_passthrough_is_rejected() {
    let err = check_fails("escptr_passthrough.dusk");
    assert!(
        err.contains("this returns a pointer to an object that stores a view of the current frame"),
        "{err}"
    );
}

#[test]
fn storing_a_view_through_a_borrowed_parameter_pointer_is_rejected() {
    let err = check_fails("escptr_borrow.dusk");
    assert!(
        err.contains("a frame view is stored through a pointer that borrows argument 1"),
        "{err}"
    );
}

#[test]
fn a_loop_carried_alias_chain_that_launders_a_frame_view_is_rejected() {
    let err = check_fails("escloop_carried.dusk");
    assert!(err.contains("views the current frame"), "{err}");
}

#[test]
fn an_intraprocedural_loop_carried_alias_chain_is_rejected() {
    let err = check_fails("escloop_carried_intra.dusk");
    assert!(err.contains("escapes its frame"), "{err}");
}

#[test]
fn a_struct_wrapping_a_polluted_pointer_field_is_rejected() {
    let err = check_fails("escholder_ptr.dusk");
    assert!(err.contains("may outlive this frame"), "{err}");
}

#[test]
fn a_map_over_frame_view_elements_with_an_identity_lambda_is_rejected() {
    let err = check_fails("escmap_identity.dusk");
    assert!(err.contains("views the current frame"), "{err}");
}

#[test]
fn a_fold_returning_a_frame_view_init_is_rejected() {
    let err = check_fails("escfold_init.dusk");
    assert!(err.contains("views the current frame"), "{err}");
}

#[test]
fn destructuring_a_call_result_that_launders_a_frame_view_is_rejected() {
    let err = check_fails("escdestructure_call.dusk");
    assert!(err.contains("escapes its frame"), "{err}");
}

#[test]
fn destructuring_a_tuple_binding_holding_a_frame_view_is_rejected() {
    let err = check_fails("escdestructure_bind.dusk");
    assert!(err.contains("escapes its frame"), "{err}");
}

#[test]
fn a_for_loop_variable_over_a_laundered_frame_view_iterand_is_rejected() {
    let err = check_fails("escforvar_call.dusk");
    assert!(err.contains("escapes its frame"), "{err}");
}

#[test]
fn a_for_loop_variable_over_a_frame_local_slice_of_slices_is_rejected() {
    let err = check_fails("escforvar_reslice.dusk");
    assert!(err.contains("escapes its frame"), "{err}");
}

#[test]
fn re_slicing_a_laundered_call_result_is_rejected() {
    let err = check_fails("escreslice_call.dusk");
    assert!(err.contains("escapes its frame"), "{err}");
}

// M5 gate round two: the structural unification. Stores route through one
// abstract-value walk (frame bit, parameter origins, pointer reads all travel
// every join), parameters seed by type reachability, lambda bodies get the same
// walk a function does, filter and fold are set-side models, and the spawn
// capture check consults the flow flags.

#[test]
fn a_direct_store_of_a_frame_view_into_a_parameter_place_is_rejected() {
    let err = check_fails("escstore_param.dusk");
    assert!(
        err.contains("stored into a place reachable through parameter"),
        "{err}"
    );
}

#[test]
fn a_readback_through_a_struct_wrapped_pointer_param_is_rejected() {
    let err = check_fails("escstruct_ptr_param.dusk");
    assert!(err.contains("escapes its frame"), "{err}");
}

#[test]
fn a_pointer_readback_stored_into_a_second_parameter_is_rejected() {
    let err = check_fails("escreadback_store.dusk");
    assert!(err.contains("view is stored into argument"), "{err}");
}

#[test]
fn a_store_through_a_borrow_laundered_by_a_call_is_rejected() {
    let err = check_fails("escborrow_call.dusk");
    assert!(err.contains("borrows argument 1"), "{err}");
}

#[test]
fn a_store_through_a_borrow_laundered_by_a_destructure_is_rejected() {
    let err = check_fails("escborrow_destructure.dusk");
    assert!(err.contains("borrows argument 1"), "{err}");
}

#[test]
fn a_map_lambda_aliasing_its_element_through_a_local_is_rejected() {
    let err = check_fails("escmap_alias.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_map_lambda_laundering_its_element_through_a_call_is_rejected() {
    let err = check_fails("escmap_launder.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_named_function_mapper_that_returns_its_param_is_rejected() {
    let err = check_fails("escmap_named.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_map_lambda_wrapping_its_element_in_a_tuple_is_rejected() {
    let err = check_fails("escmap_tuple.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_foreach_lambda_stashing_its_element_through_a_capture_is_rejected() {
    let err = check_fails("escforeach_stash.dusk");
    assert!(
        err.contains("stored into a place reachable through parameter"),
        "{err}"
    );
}

#[test]
fn a_filter_over_view_elements_is_rejected_regardless_of_predicate() {
    let err = check_fails("escfilter_elems.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_fold_lambda_returning_its_element_parameter_is_rejected() {
    let err = check_fails("escfold_element.dusk");
    assert!(err.contains(RETURNS_VIEW), "{err}");
}

#[test]
fn a_polluted_pointer_destructured_out_of_a_tuple_is_rejected() {
    let err = check_fails("escptr_destructure.dusk");
    assert!(
        err.contains("this returns a pointer to an object that stores a view of the current frame"),
        "{err}"
    );
}

#[test]
fn a_polluted_pointer_returned_through_a_field_projection_is_rejected() {
    let err = check_fails("escptr_field.dusk");
    assert!(
        err.contains("this returns a pointer to an object that stores a view of the current frame"),
        "{err}"
    );
}

#[test]
fn spawning_a_lambda_that_captures_a_polluted_pointer_is_rejected() {
    let err = check_fails("escspawn_polluted.dusk");
    assert!(err.contains("spawn cannot capture 'c'"), "{err}");
}

#[test]
fn sending_a_polluted_pointer_over_a_channel_is_rejected() {
    // A channel of pointers passes the element-type ban, but a sent pointer
    // whose heap object stores a view of the sending frame would dangle in the
    // receiver, so the send check consults the binding's flow flags, the same
    // flow a spawn or submit capture is refused for.
    let err = check_fails("escchan_polluted.dusk");
    assert!(err.contains("chan_send cannot send 'c'"), "{err}");
}

#[test]
fn sending_a_polluted_pointer_through_a_relay_helper_is_rejected() {
    // The interprocedural twin: the send happens inside a relay(ch, c) helper one
    // hop from the frame that owns the pointer, so the leaf-site send check cannot
    // see it. The escape summary records that relay sinks its parameter into a
    // channel, and the caller is rejected for handing it a polluted pointer, the
    // store that polluted it naming the site (the u6 hole).
    let err = check_fails("escchan_helper.dusk");
    assert!(err.contains("'relay' sends 'c' across a channel"), "{err}");
}

#[test]
fn a_helper_that_fuses_a_frame_store_and_a_channel_send_is_rejected() {
    // The fused twin of escchan_helper: one helper both stashes the caller's slice
    // into the pointer's heap object and sends that pointer over a channel in the
    // same body. The store edge and the sink live in one summary, so the call-site
    // sink check reads the pointer as clean (the store has not raised its flag
    // yet). The store-edge closure of the sink set lifts the sink to the source
    // position, so the frame view the caller supplies in that argument is caught.
    let err = check_fails("escchan_stash_send.dusk");
    assert!(
        err.contains("'stash_send' sends 'local' across a channel"),
        "{err}"
    );
}

#[test]
fn sending_a_polluted_pointer_through_a_direct_lambda_call_is_rejected() {
    // The closure twin of escchan_helper: the send lives inside a lambda bound to
    // a local, and the caller calls it directly one hop from the frame that owns
    // the pointer. A lambda carries no computed summary, so the escape pass
    // records the lambda's own sink set by span and the checker reads it at the
    // direct call, rejecting the polluted argument the same as a named relay
    // helper would. Single-fires: only the closure sink check names it.
    let err = check_fails("escchan_lambda.dusk");
    assert!(err.contains("'sender' sends 'c' across a channel"), "{err}");
    assert_eq!(
        err.matches("across a channel").count(),
        1,
        "single diagnostic: {err}"
    );
}

#[test]
fn sending_a_polluted_pointer_through_an_opaque_higher_order_call_is_rejected() {
    // The opaque higher-order twin: run(f, c) { f(c) } forwards its pointer
    // parameter to a function value, and the caller passes a sinking lambda and a
    // polluted pointer. run sends nothing itself, but calling an opaque function
    // value may hand the argument to a channel, so run sinks its parameter and the
    // caller is rejected. The single managed argument leaves the TOP cross-flow no
    // second place to route through, so the sink relation is the sole catch.
    let err = check_fails("escchan_hof.dusk");
    assert!(err.contains("'run' sends 'c' across a channel"), "{err}");
}

#[test]
fn a_polluted_pointer_through_a_lambda_reassigned_to_a_sinking_one_is_rejected() {
    // The value-flow wash the default-deny closes: a mut binding first holds a
    // clean lambda, then is reassigned to a sinking one and called with a polluted
    // pointer. The reassignment re-records the new lambda's sink set on the
    // binding, so the direct call is checked against exactly what it now holds and
    // the polluted argument is refused, the same reject a direct chan_send earns.
    let err = check_fails("escchan_reassign.dusk");
    assert!(err.contains("'s' sends 'c' across a channel"), "{err}");
    assert_eq!(
        err.matches("across a channel").count(),
        1,
        "single diagnostic: {err}"
    );
}

#[test]
fn a_polluted_pointer_through_a_lambda_in_a_struct_field_callee_is_rejected() {
    // A sinking lambda laundered into a struct field and invoked through the field
    // callee, box.f(c), on a polluted pointer. A field callee is opaque to the
    // send analysis, so the conservative send-reject fires: dusk cannot see which
    // argument it hands to a channel, so a polluted managed pointer is refused.
    let err = check_fails("escchan_field.dusk");
    assert!(
        err.contains("this call may send 'c' across a channel"),
        "{err}"
    );
    assert_eq!(
        err.matches("across a channel").count(),
        1,
        "single diagnostic: {err}"
    );
}

#[test]
fn sending_a_polluted_receiver_through_a_method_is_rejected() {
    // The method-receiver twin of escchan_helper: the channel send lives inside a
    // method whose hidden first parameter is the by-pointer receiver,
    // ship(ch) { chan_send(ch, self) }, so the sent value is the receiver itself.
    // A method call hides that receiver from the leaf and helper send checks (the
    // callee is a field expression and the receiver is not in the argument list),
    // so the escape summary computes the method with self as parameter 0 and marks
    // it a self-sink, and the call c.ship(ch) threads its receiver as effective
    // argument 0. A pointer whose heap object a store edge polluted with a frame
    // view is rejected exactly as a direct chan_send(ch, c) is. Single-fires.
    let err = check_fails("escchan_method.dusk");
    assert!(err.contains("'ship' sends 'c' across a channel"), "{err}");
    assert_eq!(
        err.matches("across a channel").count(),
        1,
        "single diagnostic: {err}"
    );
}

#[test]
fn returning_self_where_a_pointer_is_declared_is_rejected() {
    // `self` is the receiver value, of the concrete struct type; a bare `self`
    // loads that value in codegen, so `return self` against a `*Cell` return
    // hands the struct where the fat pointer belongs. The checker types `self` as
    // the value and rejects the pointer-position use by name, on the surface pass,
    // rather than letting the backend fault on the type. Single-fires: the precise
    // self message suppresses the generic return mismatch that would double it.
    let err = check_fails("escself_ptr.dusk");
    assert!(
        err.contains("cannot use 'self' where a pointer is required"),
        "{err}"
    );
    assert_eq!(
        err.matches("where a pointer is required").count(),
        1,
        "single diagnostic: {err}"
    );
}

#[test]
fn passing_self_by_value_into_a_pointer_method_parameter_is_rejected() {
    // The method-call argument twin of escself_ptr: `s.grab(self)` hands the
    // value `self` into `grab`'s `*Cell` parameter. A method call is otherwise
    // opaque to inference, but the callee's parameters are known from the impl,
    // so the value-self-in-pointer use earns the same precise message a direct
    // call and a return already get, rather than a stray backend fault. Single-
    // fires: only the self-value message, not a doubled generic mismatch.
    let err = check_fails("escself_methodarg.dusk");
    assert!(
        err.contains("cannot use 'self' where a pointer is required"),
        "{err}"
    );
    assert_eq!(
        err.matches("where a pointer is required").count(),
        1,
        "single diagnostic: {err}"
    );
}

#[test]
fn an_impl_on_an_enum_receiver_is_rejected() {
    // Codegen dispatches a method call only on a struct receiver; a method on an
    // enum emits no call and a `match self` in its body falls to the non-enum
    // path whose arms are unconditional, so it would silently yield a wrong
    // value. Sema rejects the impl on an enum before that lowering is reached,
    // naming the fix, so the illegal form fails loudly at the source.
    let err = check_fails("escimpl_enum.dusk");
    assert!(
        err.contains("methods on the enum 'Opt' are not supported"),
        "{err}"
    );
}

#[test]
fn returning_self_by_value_compiles_and_runs() {
    // The value-return twin of escself_ptr: the method returns `self` where the
    // return type is `Cell`, the receiver value itself, so codegen loads the
    // receiver and returns the struct and the call binds a fresh copy. Proves
    // `return self` stays legal against a value return; only the pointer position
    // is rejected.
    assert_eq!(run("selfvalue_ok.dusk"), "7\n");
}

#[test]
fn a_polluted_pointer_through_a_lambda_destructured_from_a_tuple_is_rejected() {
    // A sinking lambda packed into a tuple, destructured back out, and called
    // through the destructured binding, g(c), on a polluted pointer. A binding
    // sourced from a tuple destructure is opaque to the send analysis, so the
    // conservative send-reject refuses the polluted managed pointer argument.
    let err = check_fails("escchan_tuple.dusk");
    assert!(
        err.contains("this call may send 'c' across a channel"),
        "{err}"
    );
    assert_eq!(
        err.matches("across a channel").count(),
        1,
        "single diagnostic: {err}"
    );
}

#[test]
fn a_polluted_pointer_through_a_local_bound_to_a_sinking_module_function_is_rejected() {
    // The resolvable-fn-bind leaf path: f := relay resolves the local to relay's
    // known relation, so calling f(ch, c) with a polluted pointer is checked
    // against relay's sink set and refused exactly as a direct relay(ch, c) is. A
    // name proven bound to one fixed sinking function is not opaque, but its
    // precise sink relation still catches the send, one diagnostic, not the
    // spurious cross-flow the opaque TOP would have added.
    let err = check_fails("escchan_fnbind.dusk");
    assert!(err.contains("'f' sends 'c' across a channel"), "{err}");
    assert_eq!(
        err.matches("across a channel").count(),
        1,
        "single diagnostic: {err}"
    );
}

#[test]
fn a_polluted_pointer_returned_through_a_match_arm_binder_is_rejected() {
    let err = check_fails("escptr_match.dusk");
    assert!(
        err.contains("this returns a pointer to an object that stores a view of the current frame"),
        "{err}"
    );
}

#[test]
fn a_frame_view_read_back_through_a_bare_dereference_is_rejected() {
    let err = check_fails("escderef_slice.dusk");
    assert!(err.contains("escapes its frame"), "{err}");
}

#[test]
fn a_frame_view_stored_through_a_captured_pointer_in_a_lambda_is_rejected() {
    // A lambda bound to a local stores its parameter through a captured managed
    // pointer, and the caller invokes it with a view of a frame-local array. The
    // captured place is not one of the lambda's arguments, so the escape pass
    // records the capture-flow edge by the lambda's span and the checker raises the
    // captured binding's flag at the direct call; returning the pointer then dangles.
    let err = check_fails("esccapture_store.dusk");
    assert!(
        err.contains("this returns a pointer to an object that stores a view of the current frame"),
        "{err}"
    );
}

#[test]
fn a_frame_slice_handed_to_an_opaque_field_lambda_callee_is_rejected() {
    // The struct-field variant: the capturing lambda is laundered into a struct
    // field and invoked through the field callee on a bare frame slice. A field
    // callee is opaque to the escape analysis, so a frame slice handed to it is
    // refused conservatively, the capture store hidden behind a struct field.
    let err = check_fails("esccapture_field.dusk");
    assert!(
        err.contains("this call may store a view of the current frame beyond it"),
        "{err}"
    );
}

#[test]
fn a_frame_capturing_closure_handed_to_an_opaque_field_lambda_callee_is_rejected() {
    // The closure variant of esccapture_field: a lambda that captures a frame
    // local, bad := lambda () { return local[0] }, is handed to a struct-field
    // lambda callee, box.f(bad), whose body stashes it through a captured managed
    // pointer, (*h).g = cb. A field callee is opaque to the escape analysis, so a
    // frame-capturing closure handed to it is refused conservatively, the capture
    // store hidden behind a struct field the flow model cannot follow. Returning
    // h then dangles, since (*h).g holds a closure viewing the dead frame.
    let err = check_fails("esccapture_closure.dusk");
    assert!(
        err.contains("this call may store a closure that captures the current frame beyond it"),
        "{err}"
    );
}

// M5 alias-set propagation rejects. A frame view stored through one name of an
// alias group taints every name in the group, so a later return of a different
// name is caught. Each names the same returned-pointer escape: the store landed
// in the heap object the returned pointer reaches, through the alias edge.
const ALIAS_ESCAPE: &str =
    "this returns a pointer to an object that stores a view of the current frame";

#[test]
fn a_frame_slice_stored_through_a_struct_embedded_pointer_is_rejected() {
    // The direct store: `(*st.c).rows = local[0..4]` where `st` embeds `c`; the
    // aggregate-embed alias edge carries the raised flag from `st` to `c`.
    let err = check_fails("escalias_embed.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_ref_alias_is_rejected() {
    // A `ref q := c` aliases `c`; a store through `q` taints `c`.
    let err = check_fails("escalias_ref.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_closure_stored_by_an_interface_method_through_the_receiver_is_rejected() {
    // A method `(*self.h).g = cb` pollutes the object the receiver embeds; the
    // alias edge from `st` to `h` catches the returned `h`.
    let err = check_fails("escalias_method.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_closure_stored_by_a_helper_through_a_struct_embedded_pointer_is_rejected() {
    // A named helper `(*st.h).g = cb` raises `st`; the embed edge taints `h`.
    let err = check_fails("escalias_helper.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_by_a_helper_through_a_struct_embedded_pointer_is_rejected() {
    // The slice twin of escalias_helper: `(*st.c).rows = s` taints the embedded c.
    let err = check_fails("escalias_slice.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_destructured_pointer_binder_is_rejected() {
    // `t := (c, 1); a, n := t` links `a` to `c` transitively; a store through `a`
    // taints `c`.
    let err = check_fails("escalias_destructure.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_ref_alias_chain_is_rejected() {
    // `ref c := p; ref q := c` resolves transitively; a store through `q` taints p.
    let err = check_fails("escalias_borrowchain.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_conditionally_reassigned_alias_is_rejected() {
    // The may-join: `mut q := c; if cnd { q = d }` unions `d` on top of `c`, so a
    // store through `q` taints both. Returning the first-branch pointer is caught,
    // which a straight-line replace would wrongly accept.
    let err = check_fails("escalias_reassign.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_call_result_alias_is_rejected() {
    // Generation point 5: `d := same(c)` where `same`'s summary returns argument 0
    // links `d` to `c`; a store through `d` taints `c`.
    let err = check_fails("escalias_call.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_deeply_nested_embedded_pointer_is_rejected() {
    // The embed walk descends nested aggregates: `o := Outer { inner: Inner { c:
    // c } }` links `o` to `c` a layer down, so `(*o.inner.c).rows = local[0..4]`
    // raises `o` and the edge taints `c`.
    let err = check_fails("escalias_nested.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_field_projection_rebind_is_rejected() {
    // Generation point 6a: `x := st.c` reads a managed pointer by value out of a
    // struct field and joins the root's alias group, so a store through `x`
    // taints the embedded `c` a later return escapes.
    let err = check_fails("escalias_proj.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_an_array_index_rebind_is_rejected() {
    // Generation point 6b: `x := arr[0]` reads a managed pointer by value out of
    // an array element and joins the array's alias group, so a store through `x`
    // taints the embedded `c` a later return escapes.
    let err = check_fails("escalias_index.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_deref_projection_rebind_is_rejected() {
    // Generation point 6c: `b := alloc(Box { c: c })` embeds `c` in the heap
    // object, `x := (*b).c` reads the pointer back out and joins the group, so a
    // store through `x` taints `c` a later return escapes.
    let err = check_fails("escalias_derefproj.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_generic_field_projection_rebind_is_rejected() {
    // Generation point 6d: `x := st.c` where `st: Box<*Cell>` reads the managed
    // pointer out of a GENERIC field whose erased type `T` makes `chain_ty`
    // resolve to Unknown. The projection gate treats Unknown as a maybe and still
    // joins the group, so the store through `x` taints the embedded `c` a later
    // return escapes. The concrete twin is escalias_proj; without the Unknown
    // widening the erased field read as unmanaged and the store escaped silently.
    let err = check_fails("escalias_generic.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_generic_deref_projection_rebind_is_rejected() {
    // Generation point 6e: `x := (*w).inner` where `w: *Wrap<*Cell>` reads the
    // managed pointer back out of the heap object through a generic field of
    // erased type `T`, so `chain_ty` resolves to Unknown and the projection gate
    // joins the group on the maybe. The store through `x` taints `c` a later
    // return escapes. The concrete twin is escalias_derefproj.
    let err = check_fails("escalias_genderef.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_name_bound_intermediate_aggregate_is_rejected() {
    // The generalized embed: `inner := Inner { c: c }; outer := Outer { inner:
    // inner }` binds the intermediate aggregate to a name before embedding it. The
    // embed walk links `outer` to the name `inner` (whose type reaches a managed
    // pointer) and the group reaches `c`, so the projection `x := outer.inner.c`
    // joins it and a store through `x` taints `c`. The nested-literal twin is
    // escalias_nested; here the layer is a named binding, which the bare-pointer
    // embed gate used to walk past.
    let err = check_fails("escalias_aggbind.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_directly_through_a_name_bound_intermediate_aggregate_is_rejected() {
    // The direct-store twin of escalias_aggbind: `(*outer.inner.c).rows =
    // local[0..4]` with no projection binding. The embed edge from `outer` to the
    // name-bound `inner` still reaches `c`, so the store's root raises the group
    // and returning `c` is caught.
    let err = check_fails("escalias_aggdirect.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_two_level_generic_wrapper_is_rejected() {
    // A managed pointer buried two erased-generic layers deep: `inner: Box<*Cell>`
    // then `outer: Box<Box<*Cell>>`. The embed walk links `outer` to the name
    // `inner` because `Box<*Cell>` reaches a managed pointer, and the Unknown-erased
    // projection `x := outer.c.c` joins the group, so a store through `x` taints
    // `c`. Both the reaches-managed embed widening and the Unknown projection
    // widening are needed; either alone misses the two-level case.
    let err = check_fails("escalias_twolevel.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_struct_returned_by_value_that_shares_a_polluted_pointer_is_rejected() {
    // A struct copied by value shares the pointer it embeds: `inner := Inner { c:
    // alloc(...) }` and `outer := Outer { inner: inner }` hold two copies of the
    // same `*Cell`. A frame view stored through `outer.inner.c` raises `outer` and
    // the embed edge carries the flag back to `inner`, so returning the `inner`
    // struct by value hands out a struct whose pointer reaches the dangling view.
    // Caught here as a fat-value escape on the returned struct.
    let err = check_fails("escalias_structcopy.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn a_frame_slice_stored_through_an_aggregate_projection_rebind_is_rejected() {
    // A projection that reads out an intermediate aggregate, not a bare pointer:
    // `y := outer.inner` binds a concrete `Inner` (not managed, not Unknown), but
    // `Inner` reaches a managed pointer, so the projection gate links `y` to the
    // root `outer` and the group reaches `c`. A store through `y.c` taints `c` and
    // returning `c` is caught. The bare-pointer projection twin is escalias_proj;
    // this proves the gate widened from is-managed to reaches-managed.
    let err = check_fails("escalias_aggproj.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_destructured_aggregate_member_is_rejected() {
    // A destructure binder that takes an intermediate aggregate member, not a bare
    // pointer: `a, n := (inner, 1)` binds `a` to `Inner`, whose type reaches a
    // managed pointer, so the destructure links `a` to `inner` and transitively to
    // `c`. The projection `x := a.c` joins the group and a store through `x` taints
    // `c`. The bare-pointer destructure twin is escalias_destructure; this proves
    // the destructure member gate widened from is-managed to reaches-managed, the
    // same predicate the embed walk and projection gate now share.
    let err = check_fails("escalias_aggdestructure.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_an_aggregate_member_destructured_from_a_tuple_binding_is_rejected()
{
    // The whole-value twin of escalias_aggdestructure: the aggregate member is
    // taken through a tuple BINDING, not a tuple literal, so `a, n := t` routes the
    // no-per-position-expression path. `st := Store { c: c }` embeds `c`, `t := (st,
    // 7)` links `t` to `st` to `c`, and the binder `a` (a `Store`, whose type
    // reaches a managed pointer) joins `t`'s whole group through the shared
    // binding-alias choke. A store through `a.c` taints `c` and returning `c` is
    // caught. This proves the whole-value binder gate widened from is-managed to
    // reaches-managed, so an aggregate binder that only buries a pointer joins too;
    // the accept twin, storing only a scalar, is aliastupledestr_ok.
    let err = check_fails("escalias_tupledestr.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_match_payload_binder_is_rejected() {
    // The binding-alias choke reaches every binding-introduction site, not only
    // the let form: a match payload binder projects the scrutinee's payload, so
    // `o := Some(c)` embeds `c`, `Some(p)` links `p` to `o` (to `c`), and a store
    // through `(*p).rows` taints `c`. Returning `c` is caught. The accept twin,
    // storing only a scalar, is aliasmatch_ok.
    let err = check_fails("escalias_matchbind.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_for_loop_variable_is_rejected() {
    // The for-var binding site through the same choke: `arr := [c]` embeds `c`,
    // iterating `arr[0..1]` binds `p` to an element so `p` aliases the array's
    // group, and a store through `(*p).rows` taints `c`. Returning `c` is caught.
    // The accept twin, storing only a scalar, is aliasforvar_ok.
    let err = check_fails("escalias_forvar.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

#[test]
fn a_frame_slice_stored_through_a_reassigned_aggregate_is_rejected() {
    // The Assign site through the same choke, with the reassign join: `mut outer
    // := Outer { inner: d }` then `outer = Outer { inner: c }` drops the old edge
    // to `d` and embeds `c`, so a store through `(*outer.inner).rows` taints `c`.
    // Returning `c` is caught, which the pre-choke Assign path missed by dropping
    // the group on any aggregate reassign. The accept twin, storing only a scalar,
    // is aliasassign_ok.
    let err = check_fails("escalias_assignembed.dusk");
    assert!(err.contains(ALIAS_ESCAPE), "{err}");
}

// M5 alias-set documented residual (deferred): an alias buried inside an aggregate
// returned by a call is not surfaced by the escape summary, so `st := wrap(c)` with
// `wrap` returning `Store { c: c }` forms no edge from `st` to `c`, and a store
// through `st.c` then returning `c` is accepted here even though it dangles. This
// marks the current front-end acceptance boundary; a later milestone that surfaces
// aggregate-buried aliases will flip it to a reject. It is a check-only marker, not
// a run golden, since running it would read a dangling view.
#[test]
fn an_alias_buried_in_a_returned_aggregate_is_a_documented_residual() {
    check_ok("escalias_wrap_residual.dusk");
}

#[test]
fn a_match_binder_alias_with_only_a_scalar_store_is_accepted() {
    // The accept twin of escalias_matchbind: the payload binder aliases the
    // scrutinee's embedded pointer, but storing only a scalar through it raises
    // no frame-view flag, so the coarse alias link rejects nothing. Now a run
    // golden: the local-enum construct-and-match codegen path lands the bare
    // `Some(c)` constructor, so `(*p).n = 42` writes through the copied pointer
    // and `(*c).n` reads it back as 42. The payload copies rather than aliasing
    // the enum blob, so no new frame view is minted and the program is accepted.
    assert_eq!(run("aliasmatch_ok.dusk"), "42\n");
}

#[test]
fn short_circuit_and_or_skip_the_unneeded_operand() {
    // `&&` short circuits on a false left operand and `||` short circuits on a
    // true one, so the right side's side effect (a println inside `f`) only
    // fires when the left side did not already decide the answer. The guard
    // `i < n && a[i] == 10` with `i` past the end proves the same discipline
    // stops an out of bounds read rather than just skipping a print, and the
    // while loop and the value-position `v := true && false` exercise the
    // same lowering in a condition and in an ordinary expression.
    assert_eq!(
        run("shortcircuit.dusk"),
        "0\n1\nf\n1\nf\n0\nguarded\n3\n0\n"
    );
}

#[test]
fn a_non_bool_operand_to_a_logical_operator_is_rejected() {
    // `&&` and `||` take bool on both sides; an int operand is rejected at the
    // operator rather than coerced through some truthiness rule dusk doesn't have.
    let err = check_fails("shortcircuit_fail.dusk");
    assert!(
        err.contains("logical operators need bool operands"),
        "{err}"
    );
}

#[test]
fn strings_compare_by_content_with_eq_and_ne() {
    // A string built at runtime (through str_from_chars and substring) compares
    // equal to an equal literal by content, not by address, and the empty string
    // and an error's empty message both compare equal to the empty literal.
    assert_eq!(run("streq.dusk"), "1\n0\n1\n0\n1\n1\n0\n1\n");
}

#[test]
fn a_relational_compare_between_strings_is_rejected() {
    // Strings have no ordering, only equality, so `<` between two strings is
    // rejected instead of comparing them by address or byte order silently.
    let err = check_fails("strlt_fail.dusk");
    assert!(
        err.contains("strings compare with == and !=; they have no ordering"),
        "{err}"
    );
}

#[test]
fn a_pointer_equality_compare_is_rejected() {
    // Comparing two pointers with == would compare addresses, not the values
    // they point to, so it is rejected rather than silently doing the wrong thing.
    let err = check_fails("ptreq_fail.dusk");
    assert!(
        err.contains("pointers do not compare; compare the values they point to"),
        "{err}"
    );
}

#[test]
fn an_array_equality_compare_is_rejected() {
    // An array has no whole-value comparison; == between two arrays is rejected
    // so a caller compares elements instead of getting an address compare.
    let err = check_fails("arrcmp_fail.dusk");
    assert!(
        err.contains("cannot compare an array; compare its parts instead"),
        "{err}"
    );
}

#[test]
fn string_concatenation_builds_and_appends() {
    // `+` concatenates two strings into a fresh heap string and chains left to
    // right, and `+=` on a mut string rebinds it to a new concatenation each
    // call, so repeated appends grow the same binding.
    assert_eq!(run("strconcat.dusk"), "hello, world\na!?\nxyz\n3\n");
}

#[test]
fn a_width_cast_truncates_narrowing_and_extends_widening() {
    // int32/int8/int64/char applied like a call convert an integer explicitly:
    // int32(300) keeps 300, int8(300) truncates to 44 by two's complement,
    // char(101) and int64 back round trip through 'e', int16(-1) keeps -1
    // narrowing, and int64 widening an int8 sign extends a negative value.
    assert_eq!(run("casts.dusk"), "300\n44\ne\n101\n-1\n-5\n");
}

#[test]
fn an_assignment_needs_a_place_on_the_left() {
    // A range slice is an rvalue view; the store would silently vanish.
    let err = check_fails("assignplace_fail.dusk");
    assert!(
        err.contains("the left side of an assignment must be a place"),
        "{err}"
    );
}

#[test]
fn a_function_cannot_take_a_primitive_type_name() {
    // The width names double as the cast builtins; a user function named
    // int32 would make every call ambiguous.
    let err = check_fails("castshadow_fail.dusk");
    assert!(
        err.contains("'int32' is a primitive type name; a function cannot take it"),
        "{err}"
    );
}

#[test]
fn a_float_operand_to_a_width_cast_is_rejected() {
    // A width cast only takes an integer-family value; a float operand is
    // rejected rather than silently truncating toward zero.
    let err = check_fails("castfloat_fail.dusk");
    assert!(
        err.contains("a width cast takes an integer value; float64 does not cast"),
        "{err}"
    );
}

#[test]
fn break_and_continue_bind_to_the_innermost_loop() {
    // break exits its own loop and continue skips to its next iteration, both
    // binding to the innermost enclosing loop: an inner while's break never
    // escapes the outer while around it, a for loop's continue and break work
    // together over an array, and a for loop over a string's bytes shows the
    // same continue/break pair walking chars instead of array elements.
    assert_eq!(run("breakcontinue.dusk"), "3\n7\n4\n2\n");
}

#[test]
fn a_bare_break_outside_a_loop_is_rejected() {
    // break outside any loop has nothing to exit, so it is rejected at the
    // statement rather than accepted as a no-op.
    let err = check_fails("break_fail.dusk");
    assert!(err.contains("break is only legal inside a loop"), "{err}");
}

#[test]
fn a_bare_continue_outside_a_loop_is_rejected() {
    // continue outside any loop has no iteration to skip to, so it is rejected
    // at the statement rather than accepted as a no-op.
    let err = check_fails("continue_fail.dusk");
    assert!(
        err.contains("continue is only legal inside a loop"),
        "{err}"
    );
}

#[test]
fn a_break_inside_a_lambda_body_is_rejected_even_inside_a_loop() {
    // A lambda is its own function boundary, so a break inside a lambda body
    // called from inside an enclosing while loop is still rejected; the loop
    // the lambda executes under does not lend it a break target.
    let err = check_fails("breaklambda_fail.dusk");
    assert!(err.contains("break is only legal inside a loop"), "{err}");
}

#[test]
fn an_out_of_bounds_index_fault_names_its_file_and_line() {
    // A runtime fault carries the source location of the operation that
    // faulted, not just a bare message, so the index that actually went out
    // of bounds is named by file and line.
    let (out, err, ok) = run_raw("faultloc.dusk");
    assert!(!ok, "an out of bounds index must fault");
    assert_eq!(out, "8\n", "the in bounds index prints before the fault");
    assert!(err.contains("index out of bounds at "), "{err}");
    assert!(err.contains("faultloc.dusk:8"), "{err}");
}

#[test]
fn a_use_after_free_fault_names_its_file_and_line() {
    // The same location tagging on the generation check: a use after free
    // faults at the deref that reads through the stale pointer, not just at
    // some generic runtime trap site.
    let (out, err, ok) = run_raw("uafloc.dusk");
    assert!(!ok, "use after free must fault");
    assert_eq!(out, "9\n", "the valid deref prints before the fault");
    assert!(err.contains("use of a freed or stale pointer at "), "{err}");
    assert!(err.contains("uafloc.dusk:9"), "{err}");
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
// M5 interprocedural-escape accept goldens: a passthrough of a non-frame slice,
// and a frame view laundered through a call but consumed in the owning frame.
golden!(passthrough_ok, "passthrough_ok.dusk", "10\n30\n20\n60\n");
golden!(calluse_local, "calluse_local.dusk", "111\n222\n333\n444\n");
// M5 gate no-over-reject accepts: a heap view pushed into a vector and read back
// is legal, and a frame view laundered through a call inside an async body but
// only read stays legal. The accept side of the false-accept fixes above.
golden!(vec_heap_push_ok, "vec_heap_push_ok.dusk", "1\n");
golden!(async_launder_ok, "async_launder_ok.dusk", "111\n");
// M5 round-two no-over-reject accepts, the twins of the structural fixes: a
// heap view stored through a parameter place, a filter over scalars, a fold
// returning its heap accumulator, a minting map over view elements, and a heap
// view read back out of a clean vector and returned.
golden!(storeheap_param_ok, "storeheap_param_ok.dusk", "10\n40\n");
golden!(filter_scalar_ok, "filter_scalar_ok.dusk", "3\n12\n30\n");
golden!(foldfresh_ok, "foldfresh_ok.dusk", "9\n9\n");
golden!(mapfresh_views_ok, "mapfresh_views_ok.dusk", "111\n111\n");
golden!(
    vecget_heap_return_ok,
    "vecget_heap_return_ok.dusk",
    "10\n40\n"
);
golden!(derefheap_ok, "derefheap_ok.dusk", "1\n4\n");
// M5 gate round-three no-over-reject accepts: a heap-clean pointer sent over a
// channel (the accept twin of escchan_polluted), and a minting identity map
// whose fresh scalar array is stored through a parameter place (the store twin
// of the always-accepted return-position map).
golden!(chanheap_ok, "chanheap_ok.dusk", "10\n");
// M5 gate round-four accept twin: a heap-clean pointer sent through a relay(ch, c)
// helper that sinks its parameter into a channel. The sink relation rejects only a
// polluted argument (escchan_helper), so a heap-backed cell relays and runs.
golden!(chanrelay_ok, "chanrelay_ok.dusk", "42\n");
// M5 method-receiver accept twin of escchan_method: a method fill(s) stores its
// slice parameter through the by-pointer receiver, and the caller hands it a
// heap-backed slice, so the receiver stays clean and the pointer sent over the
// channel carries no frame view. Proves the method-call receiver threading does
// not over-reject a clean method call and the program runs; a frame-local slice
// stashed through the same method would pollute the receiver (the unit twin).
golden!(chanmethod_ok, "chanmethod_ok.dusk", "10\n");
// M5 gate round-five accept twin: the fused stash-and-send helper handed a
// heap-backed slice. The sink set's store-edge closure rejects only a frame view
// in the source position (escchan_stash_send), so a heap slice stashed into the
// cell and relayed across the channel runs and the receiver reads the heap value.
golden!(chanstash_heap_ok, "chanstash_heap_ok.dusk", "10\n");
// M5 alias-set no-over-reject accepts, the twins of the alias-propagation rejects:
// an embedded heap pointer used only in frame, a member moved into a struct then
// returned, a struct with an unrelated frame-view sibling field whose raise must
// not taint the embedded pointer, and a `ref` alias with only a scalar store.
golden!(aliasembed_ok, "aliasembed_ok.dusk", "1\n");
golden!(aliasmove_ok, "aliasmove_ok.dusk", "1\n");
golden!(aliassibling_ok, "aliassibling_ok.dusk", "222\n1\n");
golden!(aliasref_ok, "aliasref_ok.dusk", "5\n");
// M5 projection-source no-over-reject accept twin of the projection rejects:
// `x := st.c` reads a managed pointer out of a struct field and forms the alias
// link, but storing only a scalar through it raises no frame-view flag, so the
// in-frame mutation is accepted and runs, printing the value written through the
// projected alias.
golden!(aliasproj_ok, "aliasproj_ok.dusk", "42\n");
// M5 generic-projection no-over-reject accept twin of escalias_generic: `x := st.c`
// where `st: Box<*Cell>` reads the managed pointer out of an Unknown-erased generic
// field and forms the alias link on the maybe, but storing only a scalar through it
// raises no frame-view flag, so the coarse link rejects nothing and the in-frame
// mutation is accepted and runs, printing the value written through the alias.
golden!(aliasgen_ok, "aliasgen_ok.dusk", "42\n");
// M5 generalized-embed no-over-reject accept twin of the two-layer aggregate
// rejects: a pointer buried through a name-bound intermediate aggregate and used
// only in frame never dangles. `inner := Inner { c: c }` and `outer := Outer {
// inner: inner }` link `outer` to `inner` and transitively to `c`, and `x :=
// outer.inner.c` joins the group, but storing only a scalar through it raises no
// frame-view flag, so the coarse links reject nothing and the in-frame mutation
// is accepted and runs, printing the value written through the two-layer alias.
golden!(aliasagg_ok, "aliasagg_ok.dusk", "42\n");
// M5 binding-hook-unification accept twins of the for-var and reassign rejects:
// the loop variable and the reassigned aggregate each alias the embedded pointer
// through the single binding-alias choke, but storing only a scalar through the
// alias raises no frame-view flag, so the coarse link rejects nothing and the
// in-frame mutation runs, printing the value written through the alias. The match
// twin (aliasmatch_ok) is a check_ok pending the local-enum match codegen path.
golden!(aliasforvar_ok, "aliasforvar_ok.dusk", "42\n");
golden!(aliasassign_ok, "aliasassign_ok.dusk", "42\n");
// M5 destructure binding-source accept twin: an aggregate member destructured from
// a tuple BINDING (`t := (st, 9); a, n := t`) aliases the pointer `st` buries
// through the same whole-value binding-alias choke that catches escalias_tupledestr,
// but storing only a scalar through `a.c` raises no frame-view flag, so the coarse
// link rejects nothing and the in-frame mutation runs, printing the destructured
// scalar and the value written through the alias.
golden!(aliastupledestr_ok, "aliastupledestr_ok.dusk", "9\n42\n");
// M5 gate round-six accept twin: a heap-clean pointer sent through a direct call
// of a lambda bound to a local, the closure counterpart of chanrelay_ok. The
// lambda's recorded sink set rejects only a polluted argument (escchan_lambda),
// so a heap-backed cell handed to the sinking closure relays and runs.
golden!(chanlambda_ok, "chanlambda_ok.dusk", "42\n");
// M5 gate round-seven accept twins of the default-deny reversal: a clean lambda
// bound to a local sends a heap-clean pointer (escchan_clean_lambda_ok), a clean
// lambda called with a polluted pointer it only reads is the precision layer that
// proves an empty sink set accepts (cleanlambda_polluted_ok), a struct-field
// lambda callee handed a clean pointer proves the reject gates on argument
// pollution not on the opaque callee shape (fieldcall_ok), and a mut lambda
// reversed from sinking to clean then called with a polluted pointer proves the
// reassignment re-records the new empty sink set rather than dropping to opaque
// (reassign_clean_ok).
golden!(
    escchan_clean_lambda_ok,
    "escchan_clean_lambda_ok.dusk",
    "42\n"
);
golden!(
    cleanlambda_polluted_ok,
    "cleanlambda_polluted_ok.dusk",
    "111\n"
);
golden!(fieldcall_ok, "fieldcall_ok.dusk", "42\n");
golden!(reassign_clean_ok, "reassign_clean_ok.dusk", "111\n");
golden!(mapcopy_store_ok, "mapcopy_store_ok.dusk", "111\n");
// M5 gate round-eight accept twins of the capture-store fixes: a lambda whose
// capture store raises the captured pointer's flag but that pointer is used only
// in the owning frame, never returned, stays legal (capstore_local_ok), and a
// heap-backed slice stashed through an opaque struct-field lambda callee proves
// the opaque store reject gates on argument pollution, not on the callee shape
// (esccapture_field_ok). The reject twins are esccapture_store and esccapture_field.
golden!(capstore_local_ok, "capstore_local_ok.dusk", "111\n");
golden!(esccapture_field_ok, "esccapture_field_ok.dusk", "42\n");
// M5 gate closure-face accept twins of esccapture_closure: a non-capturing
// closure handed to an opaque struct-field lambda callee proves the closure
// reject gates on the argument capturing a frame local, not on the opaque callee
// shape (closurearg_ok), and a synchronous error handler, e.check(h), invokes its
// capturing handler in place and never stores it, so the frame-capturing closure
// idiom is exempted and runs (syncheck_capture_ok). The reject twin is
// esccapture_closure.
golden!(closurearg_ok, "closurearg_ok.dusk", "42\n");
golden!(syncheck_capture_ok, "syncheck_capture_ok.dusk", "7\n0\n");
golden!(m5, "m5.dusk", "42\n55\n10\n");
golden!(m6, "m6.dusk", "7\n3\n100\n");
golden!(m6b, "m6b.dusk", "10\n40\n100\n99\n2\n99\n30\n129\n");
golden!(m6c, "m6c.dusk", "6\n3\n2\n4\n42\n");
golden!(m7, "m7.dusk", "75\n24\n0\n");
golden!(m7b, "m7b.dusk", "1\n2\n0\n99\n");
golden!(m7c, "m7c.dusk", "7\n2.5\n3\n4\n42\n99\n");
golden!(m7d, "m7d.dusk", "21\n21\n42\n");
// The qualified variant constructor built and matched in one frame: `Opt.Some(v)`
// bound-and-matched, passed as a by-value argument, and the nullary `Opt.None`.
// Proves the tag dispatch and the aggregate arg pass are correct for the sole
// supported constructor form, the enum-prefixed spelling.
golden!(enumlocal, "enumlocal.dusk", "99\n7\n42\n");
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
golden!(
    printing,
    "printing.dusk",
    "score: 42\nabc\nAda is 36\n{braces} and 7\n"
);
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
golden!(
    pingpong,
    "pingpong.dusk",
    "ping\npong\nping\npong\nping\npong\ndone\n"
);
golden!(poolsum, "poolsum.dusk", "5050\n");
golden!(poolstress, "poolstress.dusk", "10000\n");
golden!(
    submitshut,
    "submitshut.dusk",
    "refused before start\n7\nrefused after shutdown\n"
);
golden!(trypoll, "trypoll.dusk", "full\n9\n");
golden!(
    recvtimeout,
    "recvtimeout.dusk",
    "timed out\n0\n5\nclosed\n0\n"
);
golden!(offload, "offload.dusk", "60\n");
golden!(awaitoffload, "awaitoffload.dusk", "60\n");
golden!(chanbridge, "chanbridge.dusk", "42\n");
golden!(
    chanbridgeclosed,
    "chanbridgeclosed.dusk",
    "receive on a closed, drained channel\n0\n"
);
golden!(spawnfuture, "spawnfuture.dusk", "42\n");
golden!(racingcomplete, "racingcomplete.dusk", "2\n");
golden!(sleepsum, "sleepsum.dusk", "42\n");
golden!(failfuture, "failfuture.dusk", "invalid digit for base\n0\n");
golden!(
    doublecomplete,
    "doublecomplete.dusk",
    "future already completed\n4\n"
);
golden!(trypending, "trypending.dusk", "future is pending\n0\n9\n");
golden!(awaittimeout, "awaittimeout.dusk", "await timed out\n0\n8\n");
golden!(reactorlife, "reactorlife.dusk", "ok\n");
golden!(wouldblock, "wouldblock.dusk", "would block\n0\n7\n0\n");
golden!(readywait, "readywait.dusk", "1\n7\n");
golden!(pipewake, "pipewake.dusk", "armed\n1\n7\n");
golden!(timerinterleave, "timerinterleave.dusk", "0\n1\n0\n");
golden!(reactorsum, "reactorsum.dusk", "10\n");
golden!(writewatch, "writewatch.dusk", "2\n");
golden!(sigpipe, "sigpipe.dusk", "broken pipe\n0\n");
golden!(
    fdexhaust_pipe,
    "fdexhaust_pipe.dusk",
    "too many open files\nok\n"
);
golden!(
    fdexhaust_connect,
    "fdexhaust_connect.dusk",
    "too many open files\nok\n"
);
golden!(
    fdexhaust_accept,
    "fdexhaust_accept.dusk",
    "too many open files\nok\n"
);
golden!(display, "display.dusk", "point\npoint\n");
golden!(fmtesc, "fmtesc.dusk", "{}\na {b} c\n{} 1\n");
golden!(emptyerr, "emptyerr.dusk", "\nafter\n");
// Reading `e.message` yields the error's message string through the same
// null-guarded lowering as `e.toString()`. Two reads (a format hole and a string
// binding) print "boom"; the empty error's null message reads back as the empty
// string, so its branch prints a blank line rather than crashing the C printers.
golden!(errmessage, "errmessage.dusk", "0\nboom\nboom\n\n");
// FIX-C: a match whose arm tails read an error's message sizes its result slot as
// a string, so the match no longer stores a pointer through an int64 slot. The
// taken arm yields the message; the sibling reads through toString.
golden!(errmsgmatch, "errmsgmatch.dusk", "0\nnegative\n");
golden!(privacy, "privacy.dusk", "1\n2\n");
golden!(bitops, "bitops.dusk", "8\n14\n6\n-13\n255\n-1\n48\n");
golden!(
    shifts,
    "shifts.dusk",
    "8\n10\n-4\n-1\n4611686018427387904\n8\n-128\n"
);
golden!(
    powers,
    "powers.dusk",
    "1024\n512\n4\n1\n-27\n32\n1024\n6.25\n"
);
golden!(precedence, "precedence.dusk", "24\n32\n7\n18\n1\n2\n");
golden!(compound, "compound.dusk", "3\n0.5\n");
golden!(singleval, "singleval.dusk", "9\n25\n11\n");
golden!(incdec, "incdec.dusk", "6\n-128\n1\n");
golden!(pipes, "pipes.dusk", "10\n13\n6\n");
golden!(inclusive, "inclusive.dusk", "9\n0\n15\n");
golden!(genericmaybe, "genericmaybe.dusk", "Some(30)\nnone\n");
// A user-defined lazy monad over a collected thunk, driven through `do Lz`. The
// chain is built without running, so "before" prints ahead of every effect, then
// forcing runs the effects in order and yields the summed value. Proves mono's
// per-site inference instantiates a fresh bind/unit pair whose continuation is a
// collector-wrapped lambda.
golden!(
    lazydo,
    "lazydo.dusk",
    "before\neffect a\neffect b\nresult 30\n"
);
// W1 gate accept twins. A generic call whose element the ground pass validates
// (enum_relabel_ok), a payload-carrying ctor that pins its element
// (enum_some_arg_ok), a direct error hand-off to an error parameter
// (err_handoff_ok) and to an error method parameter (err_method_ok), and a nested
// generic ctor instantiated at the element the outer ctor fixes (enum_nested_ok).
golden!(enum_relabel_ok, "enum_relabel_ok.dusk", "has 7\n");
golden!(enum_some_arg_ok, "enum_some_arg_ok.dusk", "1\n");
golden!(err_handoff_ok, "err_handoff_ok.dusk", "0\nsunk\n");
golden!(err_method_ok, "err_method_ok.dusk", "0\nnoted\n");
golden!(enum_nested_ok, "enum_nested_ok.dusk", "some\n");
// stdlib-phase gate accept twins. An error parameter inspected in the callee
// (err_param_handled_ok), a sibling argument pinning a bare type parameter past a
// poisoned ctor read (enum_infer_from_sibling_ok), and expected threading into a
// struct-literal field, a non-generic call argument, an assignment, and an array
// element, each instantiating an annotated `Opt.None` at its element.
golden!(err_param_handled_ok, "err_param_handled_ok.dusk", "0\n1\n");
golden!(
    enum_infer_from_sibling_ok,
    "enum_infer_from_sibling_ok.dusk",
    "2.5\n"
);
golden!(
    enum_annot_struct_field_ok,
    "enum_annot_struct_field_ok.dusk",
    "none\n"
);
golden!(
    enum_annot_call_arg_ok,
    "enum_annot_call_arg_ok.dusk",
    "none\n"
);
golden!(enum_annot_assign_ok, "enum_annot_assign_ok.dusk", "none\n");
golden!(enum_annot_array_ok, "enum_annot_array_ok.dusk", "none\n");
golden!(doasync, "doasync.dusk", "17\n");
golden!(iomonad, "iomonad.dusk", "30\n");
// The lazy IO rework. std.functional.io's IO<T> now holds a collected thunk, so a
// chain built with io_and_then and io_map suspends until run forces it: lazyio
// prints "before" ahead of every effect, then the effects in order. lazyiogc
// forces a collection between the build and run, proving the chain is rooted
// through the last thunk on the stack. iohelpers exercises io_pure, io_map,
// io_and_then, and io_println one each.
golden!(
    lazyio,
    "lazyio.dusk",
    "before\neffect a\neffect b\nresult 30\n"
);
golden!(lazyiogc, "lazyiogc.dusk", "42\n");
golden!(iohelpers, "iohelpers.dusk", "21\n10\neffect\n1\n");
golden!(tcplocal, "tcplocal.dusk", "ping\n");
golden!(acceptloop, "acceptloop.dusk", "6\n");
golden!(stress_timers, "stress_timers.dusk", "2000\n");
golden!(stress_tasks, "stress_tasks.dusk", "1000\n");
golden!(stress_accept, "stress_accept.dusk", "4950\n");
golden!(stress_pool, "stress_pool.dusk", "49995000\n");
// A future from a direct async call, once nameable and awaitable only, now
// crosses into a container, across a function argument, and through an
// annotation. Ten tasks fan into a vector before any await; a relay hands a
// future by value and back; an annotated binding and an array literal accept the
// call the same as an unannotated one.
golden!(futurefan, "futurefan.dusk", "10\n");
golden!(futurearg, "futurearg.dusk", "42\n");
golden!(futureannot, "futureannot.dusk", "3\n");

#[test]
fn awaiting_a_net_future_outside_an_async_func_is_rejected() {
    let err = check_fails("netbadawait.dusk");
    assert!(
        err.contains("'await' is only legal inside an async func"),
        "{err}"
    );
}

/// Runs an example feeding `input` to its stdin, so a program that reads with
/// `read_line` can be exercised deterministically from a pipe.
fn run_stdin(example: &str, input: &str) -> String {
    let bin = dusk_bin();
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

#[test]
fn collector_mint_deref_field_copy_and_return() {
    // The plain collector kind end to end: a scalar and a struct mint, a read and
    // a store through the deref, a copy that shares the collected block, and a
    // collector returned across a frame the block outlives.
    let out = run("collectorbox.dusk");
    assert_eq!(out, "10\n1\n2\n30\n30\n3\n4\n");
}

#[test]
fn a_collector_may_hold_a_managed_pointer() {
    // The accept twin of collectorview_fail: a managed *T is not a frame view, so
    // a struct field holding one is legal in a collector. Proves the view reject
    // is not over-wide.
    let out = run("collectorptr.dusk");
    assert_eq!(out, "5\n");
}

#[test]
fn collecting_a_value_that_reaches_a_frame_view_is_rejected() {
    // Escape neutrality holds only when the element cannot carry a frame view. A
    // struct reaching a slice, minted into a collected block that outlives the
    // frame, would dangle the slice. Without this reject the collector path was a
    // silent bypass of the frame-slice escape rule (a use after free).
    let err = check_fails("collectorview_fail.dusk");
    assert!(
        err.contains(
            "a collected value cannot hold a slice, function, or interface; \
             collect a scalar, a pointer, or a struct of those"
        ),
        "{err}"
    );
}

#[test]
fn collector_stays_an_identifier_in_a_comparison() {
    // `collector` is contextual: `collector < n` is a comparison, not a malformed
    // mint. The lookahead only mints on a balanced `<T>` followed by `(`.
    let out = run("collectorident.dusk");
    assert_eq!(out, "1\n");
}

#[test]
fn collecting_a_pointer_to_a_frame_view_is_rejected() {
    // A managed pointer element is allowed only when its pointee is heap owned.
    // A pointer whose pointee stores a frame-local slice would dangle once the
    // collected block outlives the frame, so the mint is refused as an outliving
    // sink, the same as the rejected `return p`.
    let err = check_fails("collectorptrview_fail.dusk");
    assert!(
        err.contains(
            "this collects a pointer to an object that stores a view of the current frame"
        ),
        "{err}"
    );
}

#[test]
fn collecting_a_struct_embedding_a_frame_view_pointer_is_rejected() {
    // The struct-embed shape: the frame view sits one struct layer down, behind a
    // managed pointer field. Minting the struct still dangles the slice.
    let err = check_fails("collectorptrfield_fail.dusk");
    assert!(
        err.contains("a slice into a local array escapes its frame"),
        "{err}"
    );
}

#[test]
fn collecting_a_pointer_to_a_frame_slice_is_rejected() {
    // The slice-behind-pointer shape: a pointer to a slice that views a frame
    // local array. The collected block outlives the frame, so the slice dangles.
    let err = check_fails("collectorptrslice_fail.dusk");
    assert!(
        err.contains(
            "this collects a pointer to an object that stores a view of the current frame"
        ),
        "{err}"
    );
}

#[test]
fn passing_a_frame_view_into_a_minting_helper_is_rejected() {
    // The interprocedural mint-sink: mk mints its pointer parameter, so it is an
    // outliving sink on that parameter, and a caller handing it a pointer whose
    // pointee stores a frame view is refused at the call. The mint reject is
    // collect-flavored, not channel-flavored, so it names the collect and the
    // block the value outlives.
    let err = check_fails("collectorptrhelper_fail.dusk");
    assert!(err.contains("holds a view of the frame"), "{err}");
    assert!(err.contains("collects"), "{err}");
    assert!(
        !err.contains("channel"),
        "collect reject must not mention a channel: {err}"
    );
}

#[test]
fn a_collector_survives_a_suspension_and_forced_collections() {
    // A collector minted inside an async func is held across the await in the task
    // frame, a registered collector root, so a forced collection before and after
    // the suspension cannot sweep it. The read yields the minted value.
    let out = run("gccollectorasync.dusk");
    assert_eq!(out, "777\n");
}

#[test]
fn freeing_a_collector_is_rejected() {
    // A collected value is reclaimed by the collector, so free is refused. The
    // accept twin is the copy path in collectorbox; an owned *T still frees.
    let err = check_fails("collectorfree_fail.dusk");
    assert!(
        err.contains("a collected value is not freed; the collector reclaims it"),
        "{err}"
    );
}

#[test]
fn moving_a_collector_is_rejected() {
    // A collected value is not owned, so there is nothing to move. The accept
    // twin is the plain copy `d := c` in collectorbox.
    let err = check_fails("collectormove_fail.dusk");
    assert!(
        err.contains("a collected value is not owned; copy it directly"),
        "{err}"
    );
}

#[test]
fn ref_aliasing_a_collector_is_rejected() {
    // A ref alias of an unowned collected value is meaningless. The accept twin
    // is the plain copy `d := c` in collectorbox.
    let err = check_fails("collectorref_fail.dusk");
    assert!(
        err.contains("a collected value is not borrowed with ref; copy it directly"),
        "{err}"
    );
}

#[test]
fn a_closure_collector_outlives_the_frame_that_built_it() {
    // The closure collector kind: a func mints a thunk that captures a frame-local
    // scalar, then returns it. The environment lives on the collected heap, so the
    // caller calls the thunk after the building frame is gone and reads the value.
    let out = run("collectorthunk.dusk");
    assert_eq!(out, "42\n");
}

#[test]
fn a_closure_collector_lives_in_a_struct_field() {
    // A collected thunk stored in a struct field, the struct returned, the thunk
    // called through the field. The field's rep is the closure { env, fn }.
    let out = run("collectorclosurefield.dusk");
    assert_eq!(out, "42\n");
}

#[test]
fn a_slice_collector_deep_copies_a_frame_view_source() {
    // The slice collector kind: a frame-local array is deep copied onto the
    // collected heap at the mint, so the returned slice views immortal backing. A
    // forced collection after the building frame is gone leaves it intact, and the
    // collector indexes and ranges as a slice.
    let out = run("collectorslice.dusk");
    assert_eq!(out, "100\n20\n");
}

#[test]
fn a_lazy_chain_of_collected_thunks_survives_a_collection() {
    // A hand-rolled chain of collected thunks, each capturing the previous through
    // a struct-of-collector parameter. A forced collection before the single force
    // proves the whole chain is rooted transitively through the last thunk, so the
    // sweep keeps every environment and the forced value is exact.
    let out = run("lazychain.dusk");
    assert_eq!(out, "111\n");
}

#[test]
fn a_closure_collector_may_capture_a_collector_typed_slice() {
    // The accept twin of collectorcapview_fail: a collector-typed capture reads as
    // immortal safe because its backing lives on the collected heap. Proves the
    // capture rule is not over-wide.
    let out = run("collectorcapok.dusk");
    assert_eq!(out, "12\n");
}

#[test]
fn collecting_a_closure_that_captures_a_frame_view_is_rejected() {
    // The closure capture rule: a lambda capturing a frame-view slice, minted into
    // a collected environment that outlives the frame, would dangle the captured
    // fat pointer. The mint is rejected, naming the capture.
    let err = check_fails("collectorcapview_fail.dusk");
    assert!(
        err.contains("cannot collect a closure that captures 's': it may view a frame"),
        "{err}"
    );
}

#[test]
fn collecting_a_slice_of_slices_is_rejected() {
    // A slice collector deep copies one level, so a slice-of-slices element leaves
    // each inner fat pointer viewing the frame. The mint is rejected. The accept
    // twin is collector<int64[]>, whose scalar element is copied whole.
    let err = check_fails("collectorslicenest_fail.dusk");
    assert!(
        err.contains(
            "a collected slice's element cannot itself hold a slice, function, or interface"
        ),
        "{err}"
    );
}

#[test]
fn collecting_an_interface_value_is_rejected() {
    // An interface value's data pointer may sit in the frame, so a collector over
    // an interface element is rejected. The accept twin is a collector over the
    // concrete type or a managed pointer to it.
    let err = check_fails("collectoriface_fail.dusk");
    assert!(
        err.contains("a collected value cannot hold a slice, function, or interface"),
        "{err}"
    );
}

#[test]
fn a_slice_collector_over_clean_managed_pointer_structs_is_allowed() {
    // The buried-view guard is precise, not a blanket reject of managed-pointer
    // slice elements. Here each pointee is heap owned, so the deep copy is sound: a
    // forced collection keeps the outer backing and each generational pointee.
    let out = run("collectorslicecell.dusk");
    assert_eq!(out, "33\n");
}

#[test]
fn collecting_a_slice_of_pointers_to_frame_views_is_rejected() {
    // The slice-kind twin of collectorptrview_fail: the element reaches a managed
    // pointer whose pointee stores a frame view. The deep copy does not immortalize
    // the pointee, so it dangles; the guard looks past the outer frame slice and
    // rejects the tainted pointer.
    let err = check_fails("collectorsliceburied_fail.dusk");
    assert!(
        err.contains(
            "a collected slice element holds a pointer to an object that stores a view of the current frame"
        ),
        "{err}"
    );
}

#[test]
fn collecting_a_closure_that_captures_a_buried_frame_view_is_rejected() {
    // The closure-kind twin of collectorptrview_fail: a captured managed pointer
    // whose pointee stores a frame view. Its type is immortal safe, but the buried
    // slice dangles once the frame is gone, so the capture rule runs the plain
    // mint's escape check to catch it, not just a view-typed capture.
    let err = check_fails("collectorcapbury_fail.dusk");
    assert!(
        err.contains("cannot collect a closure that captures 'p': it may view a frame"),
        "{err}"
    );
}

#[test]
fn a_lambda_widens_into_a_closure_collector_parameter() {
    // The mono rewrite mints a bare lambda literal at a collector<F> parameter,
    // and a collector<F> value passes the other way where a plain F is expected.
    // Both directions leave the environment on the collected heap and dispatch
    // through the closure rep.
    let out = run("collectorwiden.dusk");
    assert_eq!(out, "7\n35\n");
}

#[test]
fn a_bare_lambda_at_a_method_collector_parameter_is_rejected() {
    // The mono rewrite mints a lambda into a collector only at a direct top-level
    // call, never a method argument, so a bare lambda there would lower to a stack
    // environment typed as a collector and dangle. Rejected, pointing at the mint.
    let err = check_fails("collectormethodlam_fail.dusk");
    assert!(
        err.contains("a bare lambda cannot become a closure collector at a method argument"),
        "{err}"
    );
}

#[test]
fn an_explicit_mint_at_a_method_argument_works() {
    // The accept twin: an explicit mint at a method argument lands the environment
    // on the collected heap, so the closure survives the building frame and a
    // forced collection.
    let out = run("collectormethodmint.dusk");
    assert_eq!(out, "43\n");
}

#[test]
fn a_bare_lambda_at_an_indirect_call_collector_parameter_is_rejected() {
    // An indirect callee is not rewritten by mono, so a bare lambda at its
    // closure-collector parameter would skip the mint and dangle. Rejected.
    let err = check_fails("collectorindirectlam_fail.dusk");
    assert!(
        err.contains("a bare lambda cannot become a closure collector through an indirect call"),
        "{err}"
    );
}

#[test]
fn an_explicit_mint_through_an_indirect_call_works() {
    // The accept twin: an explicit mint fed to an indirect call mints before the
    // call, so the collector value the callee receives is already on the collected
    // heap and survives a forced collection.
    let out = run("collectorindirectmint.dusk");
    assert_eq!(out, "5\n");
}

#[test]
fn a_closure_capturing_a_buried_frame_view_pointer_parameter_is_rejected() {
    // The interprocedural closure-capture sink: a minting helper captures its
    // pointer parameter, and a caller buries a frame view behind its pointee. The
    // closure mint records a sink on the captured pointer, so the caller is caught
    // one hop up, matching the plain and slice kinds. The reject is collect
    // flavored, naming the collect rather than a channel.
    let err = check_fails("collectorcapparam_fail.dusk");
    assert!(err.contains("holds a view of the frame"), "{err}");
    assert!(err.contains("collects"), "{err}");
    assert!(
        !err.contains("channel"),
        "collect reject must not mention a channel: {err}"
    );
}

#[test]
fn a_closure_capturing_a_clean_pointer_parameter_is_allowed() {
    // The accept twin: the caller passes a pointer to a heap-clean object, so the
    // sink does not fire and the closure is sound across a forced collection.
    let out = run("collectorcapparamok.dusk");
    assert_eq!(out, "77\n");
}

#[test]
fn the_widening_path_enforces_the_capture_rule() {
    // A bare lambda at a collector parameter is minted, so a frame-view capture is
    // rejected there too, exactly as in the explicit mint form.
    let err = check_fails("collectorwidencap_fail.dusk");
    assert!(
        err.contains("cannot collect a closure that captures 's': it may view a frame"),
        "{err}"
    );
}

#[test]
fn collectors_survive_a_suspension_held_in_the_task_frame() {
    // Three collectors minted inside an async func, all held across the await in
    // the task frame, a registered collector root. A collection forced before and
    // after the suspension cannot sweep them, so each derefs to its minted value
    // in order. Proves the task-frame root keeps more than one collected ref live.
    let out = run("gcasync.dusk");
    assert_eq!(out, "100\n20\n3\n123\n");
}

#[test]
fn a_vector_of_collectors_grows_past_collections() {
    // A vector of collectors grown past ten collections. The vector buffer is a
    // generational block the collector scans as a root region, so every element
    // stays live through the growth and the read. Sum is the arithmetic answer,
    // proving the generation registry roots a same-thread container of collectors.
    // A same-thread container is not confined away, so this must build and run.
    let out = run("gcvector.dusk");
    assert_eq!(out, "135\n");
}

#[test]
fn two_hundred_async_tasks_each_hold_a_collector_across_a_collection() {
    // Two hundred async tasks each mint a collector and hold it across a timer,
    // read back with collections interleaved. Every parked task frame is a
    // registered collector root, so the block a task holds across its suspension
    // survives a collection forced while all are parked and on every read.
    let out = run("gcstress.dusk");
    assert_eq!(out, "19900\n");
}

#[test]
fn an_async_func_may_return_a_collector_through_its_future() {
    // An async func returns a collector, so its future carries a collected value.
    // A future record is a generational block the collector scans, so the value is
    // rooted from completion until the read, a collection forced inside the task
    // and after the loop leaving it intact. A future completes on the loop thread
    // the value was minted on, so the thread confinement does not ban it the way
    // it bans a channel element or a spawn capture. This locks the allowance in.
    let out = run("gcfuturecollector.dusk");
    assert_eq!(out, "42\n");
}

#[test]
fn a_channel_of_collectors_is_rejected() {
    // A collected value stays on the main thread; a channel carries its element to
    // another thread, where it would sit unrooted in the ring and be swept while
    // live. The element type is refused at the mint. The accept twin is chanheap_ok,
    // a channel of managed pointers whose heap objects the collector still roots.
    let err = check_fails("chancollector_fail.dusk");
    assert!(
        err.contains("a collected value stays on the main thread; it cannot cross through a channel to another thread"),
        "{err}"
    );
}

#[test]
fn a_spawn_capturing_a_collector_is_rejected() {
    // A collected value stays on the main thread; a spawned frame runs on another
    // thread. The capture is refused. The accept twin is a spawn capturing a
    // managed pointer or scalar, backstopped by the generation check.
    let err = check_fails("spawncollector_fail.dusk");
    assert!(
        err.contains("spawn cannot capture 'c': a collected value stays on the main thread; it cannot cross to another thread"),
        "{err}"
    );
}

#[test]
fn a_submit_capturing_a_collector_is_rejected() {
    // A collected value stays on the main thread; a pool worker runs on another
    // thread. The capture is refused. The accept twin is poolsum, whose submit
    // captures a channel and a scalar and sends the result back.
    let err = check_fails("submitcollector_fail.dusk");
    assert!(
        err.contains("submit cannot capture 'c': a collected value stays on the main thread; it cannot cross to another thread"),
        "{err}"
    );
}

#[test]
fn boxing_a_collector_into_an_interface_is_rejected() {
    // An interface value is a fat pointer that can ride a channel or a spawn
    // capture off the main thread, and a collected value stays on the main thread,
    // so the boxing is refused. Distinct from collectoriface_fail, a collector
    // over an interface element. The accept twin is boxing a concrete implementer.
    let err = check_fails("collectorifacebox_fail.dusk");
    assert!(
        err.contains(
            "a collected value cannot be boxed into an interface; it stays on the main thread"
        ),
        "{err}"
    );
}

// A collector reached through a managed pointer must not cross a thread either.
// The collector scans anchor-side roots only, never a worker stack, so a
// collected ref carried behind a pointer to another thread is swept while that
// thread still holds it, a confirmed use after free. The reach walk finds a
// collector behind a pointer, bare or buried in a struct, spelled out or through
// a generic. The accept twins prove an ordinary managed pointer still crosses.

#[test]
fn a_channel_of_pointers_to_collectors_is_rejected() {
    let err = check_fails("chancollectorptr_fail.dusk");
    assert!(
        err.contains("a collected value stays on the main thread; it cannot cross through a channel to another thread"),
        "{err}"
    );
}

#[test]
fn a_channel_of_pointers_to_a_collector_bearing_struct_is_rejected() {
    let err = check_fails("chancollectorcell_fail.dusk");
    assert!(
        err.contains("a collected value stays on the main thread; it cannot cross through a channel to another thread"),
        "{err}"
    );
}

#[test]
fn a_generic_channel_at_a_pointer_to_a_collector_is_rejected() {
    let err = check_fails("chancollectorgeneric_fail.dusk");
    assert!(
        err.contains("a collected value stays on the main thread; it cannot cross through a channel to another thread"),
        "{err}"
    );
}

#[test]
fn a_spawn_capturing_a_pointer_to_a_collector_is_rejected() {
    // The repro gpt-5.5 reduced to a use after free: check accepted it, run
    // faulted. The reach walk now refuses it at check.
    let err = check_fails("spawncollectorptr_fail.dusk");
    assert!(
        err.contains("spawn cannot capture 'p': a collected value stays on the main thread; it cannot cross to another thread"),
        "{err}"
    );
}

#[test]
fn a_spawn_capturing_a_pointer_to_a_collector_bearing_struct_is_rejected() {
    let err = check_fails("spawncollectorcell_fail.dusk");
    assert!(
        err.contains("cannot capture 'cell': a collected value stays on the main thread; it cannot cross to another thread"),
        "{err}"
    );
}

#[test]
fn a_channel_of_pointers_to_scalar_cells_still_crosses() {
    // Accept twin: no collector is reachable through the pointer, so the send
    // crosses freely. Proves the pointer-reaches-collector ban is not over-wide.
    let out = run("chanptrscalar_ok.dusk");
    assert_eq!(out, "9\n");
}

#[test]
fn a_spawn_capturing_a_pointer_to_a_scalar_cell_still_works() {
    // Accept twin: an ordinary managed pointer is still capturable.
    let out = run("spawnptrscalar_ok.dusk");
    assert_eq!(out, "7\n");
}

// Boxing a collector into an interface is refused wherever the coercion happens,
// not only at an argument, binding, or return: a struct field, an array element,
// and a tuple element route through the same reject with a clean diagnostic
// rather than reaching codegen as a fat-rep mismatch.

#[test]
fn boxing_a_collector_into_an_interface_field_is_rejected() {
    let err = check_fails("collectorfieldbox_fail.dusk");
    assert!(
        err.contains(
            "a collected value cannot be boxed into an interface; it stays on the main thread"
        ),
        "{err}"
    );
}

#[test]
fn boxing_a_collector_into_an_interface_array_element_is_rejected() {
    let err = check_fails("collectorarraybox_fail.dusk");
    assert!(
        err.contains(
            "a collected value cannot be boxed into an interface; it stays on the main thread"
        ),
        "{err}"
    );
}

#[test]
fn boxing_a_collector_into_an_interface_tuple_element_is_rejected() {
    let err = check_fails("collectortuplebox_fail.dusk");
    assert!(
        err.contains(
            "a collected value cannot be boxed into an interface; it stays on the main thread"
        ),
        "{err}"
    );
}

#[test]
fn boxing_a_concrete_implementer_into_an_interface_field_still_works() {
    // Accept twin: the ordinary interface path at a field still works, so the
    // collector iface-box reject is not over-wide.
    let out = run("collectorfieldconcrete_ok.dusk");
    assert_eq!(out, "42\n");
}

// Unicode milestone U1: \u{...} escapes and their rejections.

#[test]
fn unicode_escapes_decode_to_utf8() {
    // One and two digit code points, a CJK pair, and an astral emoji all decode
    // through a single string literal and print as their UTF-8 bytes.
    assert_eq!(run("unicodeescape.dusk"), "Hi 中文 😀\n");
}

#[test]
fn unicode_escape_with_too_many_digits_is_rejected() {
    let err = check_fails("escbadhex.dusk");
    assert!(err.contains("\\u escape needs 1 to 6 hex digits"), "{err}");
}

#[test]
fn unicode_escape_surrogate_is_rejected() {
    let err = check_fails("escsurrogate.dusk");
    assert!(
        err.contains("\\u escape is a surrogate code point, not a scalar value"),
        "{err}"
    );
}

#[test]
fn unicode_escape_above_maximum_is_rejected() {
    let err = check_fails("escrange.dusk");
    assert!(
        err.contains("\\u escape is above 0x10FFFF, the Unicode maximum"),
        "{err}"
    );
}

#[test]
fn unicode_escape_without_closing_brace_is_rejected() {
    let err = check_fails("escunterminated.dusk");
    assert!(
        err.contains("unterminated \\u escape; expected '}'"),
        "{err}"
    );
}

#[test]
fn wide_escape_in_char_literal_is_rejected() {
    let err = check_fails("charwideescape.dusk");
    assert!(
        err.contains(
            "a char is one byte; this escape does not fit, use a rune literal or a string"
        ),
        "{err}"
    );
}

#[test]
fn rune_literals_and_int_flows_run() {
    assert_eq!(run("runebasics.dusk"), "97\n20013\n128512\n20014\ntrue\n");
}

#[test]
fn rune_values_survive_generic_monomorphization() {
    assert_eq!(run("runegeneric.dusk"), "128512\n20013\n12\n");
}

#[test]
fn char_and_rune_share_int_flows_without_mixing() {
    assert_eq!(run("runechar.dusk"), "65\n20013\ntrue\n");
}

#[test]
fn rune_to_char_assignment_is_rejected() {
    let err = check_fails("runecharmix_fail.dusk");
    assert!(
        err.contains("type annotation that does not match its value"),
        "{err}"
    );
}

#[test]
fn char_to_rune_assignment_and_argument_are_rejected() {
    let err = check_fails("charrunemix_fail.dusk");
    assert!(
        err.contains("type annotation that does not match its value"),
        "{err}"
    );
    assert!(err.contains("argument 1 has the wrong type"), "{err}");
}

#[test]
fn string_to_rune_assignment_is_rejected() {
    let err = check_fails("runestring_fail.dusk");
    assert!(
        err.contains("type annotation that does not match its value"),
        "{err}"
    );
}

#[test]
fn unicode_decode_rune_walks_a_mixed_string() {
    // std.unicode's decode loop over "aß中😀": one, two, three, and four byte
    // scalars, then rune_count and str_len agreeing on the same view.
    assert_eq!(
        run("runedecode.dusk"),
        "97 1\n223 2\n20013 3\n128512 4\n4\n10\n"
    );
}

#[test]
fn unicode_sb_push_rune_and_encode_rune_round_trip() {
    // sb_push_rune rebuilds "aß中😀" scalar by scalar, and encode_rune writes
    // the same emoji's 4 UTF-8 bytes directly into a raw buffer.
    assert_eq!(run("runebuild.dusk"), "aß中😀\n240 159 152 128\n");
}

#[test]
fn unicode_utf8_valid_rejects_every_boundary_violation() {
    // utf8_valid accepts well formed UTF-8, then rejects a lone continuation
    // byte, a truncated sequence, an overlong encoding, a surrogate, and a
    // scalar above 0x10FFFF; decode_rune resyncs each bad lead to (0xFFFD, 1).
    assert_eq!(
        run("utf8valid.dusk"),
        "true\nfalse\nfalse\nfalse\nfalse\nfalse\n65533 1\n65533 1\n65533 1\n65533 1\n65533 1\n"
    );
}

#[test]
fn unicode_rune_count_counts_each_invalid_byte_as_one_scalar() {
    // rune_count agrees with decode_rune's resync: a stray continuation byte
    // and an overlong lead each count as exactly one scalar, never desyncing.
    assert_eq!(run("runecount.dusk"), "4\n0\n5\n5\n3\n");
}

#[test]
fn unicode_decode_pins_every_utf8_boundary() {
    // Overlong lower bounds, the surrogate gap, the U+10FFFF ceiling, the
    // C1/C2 floor, invalid leads, width-2 and width-4 truncation, a nonzero
    // start offset, and the genuine width 3 U+FFFD that stays accepted rather
    // than reading as the width 1 resync signature.
    assert_eq!(
        run("utf8bounds.dusk"),
        "invalid 65533 1\nvalid 2048 3\ninvalid 65533 1\nvalid 65536 4\n\
         valid 55295 3\ninvalid 65533 1\nvalid 1114111 4\ninvalid 65533 1\n\
         invalid 65533 1\nvalid 128 2\ninvalid 65533 1\ninvalid 65533 1\n\
         invalid 65533 1\ninvalid 65533 1\ninvalid 65533 1\nvalid 65533 3\n\
         223 2\n20013 3\n128512 4\n"
    );
}

#[test]
fn unicode_encode_pins_every_scalar_boundary() {
    // encode_rune folds invalid scalars to EF BF BD at width 3, pins the exact
    // bytes for each valid boundary scalar, and rune_len agrees; the same
    // scalars round trip back through sb_push_rune and decode_rune.
    assert_eq!(
        run("runebounds.dusk"),
        "3: 239 191 189 len=3\n3: 239 191 189 len=3\n3: 239 191 189 len=3\n\
         1: 127 len=1\n2: 194 128 len=2\n2: 223 191 len=2\n3: 224 160 128 len=3\n\
         3: 239 191 191 len=3\n4: 240 144 128 128 len=4\n4: 244 143 191 191 len=4\n\
         127 1\n128 2\n2047 2\n2048 3\n65535 3\n65536 4\n1114111 4\n"
    );
}

#[test]
fn unicode_rune_count_survives_a_large_input_on_the_default_stack() {
    // Regression guard for the entry-block alloca funnel: the decode loop binds
    // `r, w := decode_rune(s, i)` each iteration. With the funnel that slot is
    // reserved once at entry and reused, so half a million scalars count on the
    // default 8 MB stack instead of overflowing it with a per-iteration alloca.
    assert_eq!(run("unicodebig.dusk"), "500000\n");
}

#[test]
fn logging_gates_by_level_and_set_level_lowers_the_threshold() {
    // Default level is Info: log_debug is dropped, the rest fire. After
    // log_set_level(LogLevel.Debug), log_debug fires too. The exit is clean, so
    // run_raw is used only to keep stdout and stderr apart, not for a fault.
    let (out, err, ok) = run_raw("logging.dusk");
    assert!(ok, "logging.dusk must run cleanly: {err}");
    assert_eq!(out, "phase1\nphase2\n");
    assert_eq!(
        err,
        "[info] first\n[warn] careful\n[error] broken\n[debug] shown\n"
    );
}

// 0.5.3 W2/W4: the Result monad and its helpers, and the new Maybe/Either
// helpers. Bool prints as its 1/0 word, not the words "true"/"false".
golden!(resultdo, "resultdo.dusk", "ok 21\nerr too big\n");
golden!(resultbridge, "resultbridge.dusk", "err boom\nok 42\n");
golden!(
    resulthelpers,
    "resulthelpers.dusk",
    "1\n1\n0\n10\nrelabeled\n105\n0\n11\n"
);
golden!(maybehelpers, "maybehelpers.dusk", "1\n10\n105\n9\n");
golden!(eitherhelpers, "eitherhelpers.dusk", "-1\n3\n6\n107\n4\n8\n");

#[test]
fn an_unpinned_result_ctor_is_diagnosed() {
    // W2 reject twin: `Result.Err("too big")` carries no T payload, and
    // nothing here pins it, so the binding cannot infer T. Compare
    // resulthelpers.dusk, where every ctor is pinned by an annotation or by a
    // plain, T-bearing argument.
    let err = check_fails("resultctor_fail.dusk");
    assert!(
        err.contains("cannot infer the type parameter 'T' for 'Result'"),
        "{err}"
    );
}

// 0.5.4 M4: three pinned regressions, no compiler change behind them.

#[test]
fn generic_struct_over_an_interface_argument_rejects_in_bounded_time() {
    // `Box<Speaker>` used to hang the checker (a mono worklist loop), closed in
    // 0.5.0. Pinned here so a regression shows up as a failing test, not a
    // multi-minute hang: check_fails itself is bounded by the process exiting.
    let err = check_fails("ifacetarg_fail.dusk");
    assert!(
        err.contains("an interface cannot be a generic type argument"),
        "{err}"
    );
}

golden!(genfieldarr, "genfieldarr.dusk", "2\n");

#[test]
fn do_over_a_private_imported_monad_is_undefined_not_silently_bound() {
    // privmonad_lib.dusk exports its struct W but keeps bind and unit private;
    // a `do W { ... }` here must reach the renamed, private pair, so it is
    // undefined, not the wrong bind silently accepted.
    let err = check_fails("privmonad_fail.dusk");
    assert!(err.contains("undefined name 'W.bind'"), "{err}");
    assert!(err.contains("undefined name 'W.unit'"), "{err}");
}

// ---------------------------------------------------------------------------
// Spec-audit fixes (0.5.4): each rejects an illegal form the checker used to
// accept, paired with a twin that proves the legal form still works.
// ---------------------------------------------------------------------------

#[test]
fn enum_constructor_payload_literal_out_of_range_is_rejected() {
    let err = check_fails("ctorfit_fail.dusk");
    assert!(
        err.contains("literal 4294967297 does not fit in 32 bits"),
        "{err}"
    );
}

golden!(ctorfit_ok, "ctorfit_ok.dusk", "2147483647\n127\n");

#[test]
fn struct_literal_field_literal_out_of_range_is_rejected() {
    let err = check_fails("structlitwidth_fail.dusk");
    assert!(
        err.contains("literal 2147483648 does not fit in 32 bits"),
        "{err}"
    );
}

golden!(structlitwidth_ok, "structlitwidth.dusk", "2147483647\n");

#[test]
fn assigning_to_a_string_element_is_rejected() {
    let err = check_fails("stridxassign_fail.dusk");
    assert!(err.contains("a string is immutable"), "{err}");
    assert!(err.contains("StringBuilder"), "{err}");
}

golden!(stridxread_ok, "stridxread_ok.dusk", "h\n");

#[test]
fn char_values_print_as_text_bytes() {
    // A char, a char array, and a char slice write their bytes as text through
    // every print shape: direct, format hole, and stderr. The numeric reading
    // stays one int-annotated binding away, and a multibyte string's bytes
    // pass through untouched so the glyphs survive.
    let (out, err, ok) = run_raw("charprint.dusk");
    assert!(ok, "{err}");
    assert_eq!(
        out,
        "h\nHello\nell\neHelloell\nchar e in Hello\n101\nhéllo\n"
    );
    assert_eq!(err, "Hello\n");
}

#[test]
fn a_string_literal_initializes_a_char_array_by_exact_byte_count() {
    // The exact fit, the mut reassign, an element write after the copy, a
    // multibyte literal counted in bytes, and a decoded escape.
    assert_eq!(run("chararrlit.dusk"), "Hello\nxyz\nqyz\nhéllo\na\nb\n");
}

#[test]
fn a_char_array_refuses_a_literal_of_the_wrong_byte_length() {
    // "héllo" is six bytes; the reject names both counts.
    let err = check_fails("chararrlit_fail.dusk");
    assert!(
        err.contains("the string literal has 6 byte(s); the annotation says char[5]"),
        "{err}"
    );
}

#[test]
fn a_string_value_never_converts_to_a_char_array() {
    // Only a literal converts; a binding does not launder the conversion.
    let err = check_fails("chararrval_fail.dusk");
    assert!(
        err.contains("has a type annotation that does not match its value"),
        "{err}"
    );
}

#[test]
fn a_char_array_call_argument_stays_rejected() {
    // The literal conversion is scoped to let and assignment.
    let err = check_fails("chararrarg_fail.dusk");
    assert!(err.contains("argument 1 has the wrong type"), "{err}");
}

#[test]
fn a_for_loop_iterates_a_string_by_byte() {
    // Six bytes for the five glyphs of "héllo", glyphs intact through byte
    // printing, and the loop var widens to an int like any char.
    assert_eq!(run("forstring.dusk"), "héllo\n6\n97\n98\n");
}

#[test]
fn a_for_loop_rejects_a_non_iterable_source() {
    // Was accept-then-clang-error; now a clean check reject.
    let err = check_fails("forstring_fail.dusk");
    assert!(
        err.contains(
            "cannot iterate an integer literal; a for loop takes an array, a slice, or a string"
        ),
        "{err}"
    );
}

#[test]
fn a_string_range_slice_validates_its_window() {
    // "abc"[1..9] reaches past the NUL scanned length and faults like any
    // other out of range window.
    let (out, err, ok) = run_raw("strrangefault.dusk");
    assert!(!ok, "must fault");
    assert_eq!(out, "");
    assert!(err.contains("index out of bounds"), "{err}");
}

#[test]
fn in_bounds_string_ranges_slice_every_base_shape() {
    // A binding, a literal, a call result, the full window, and the empty
    // window all mint valid char slices.
    assert_eq!(run("strrange.dusk"), "bc\nabcd\nbc\nwx\n0\nbc\n");
}

#[test]
fn a_raw_pointer_refuses_a_range_slice() {
    // A raw pointer has no length to validate a range against.
    let err = check_fails("rawrange_fail.dusk");
    assert!(
        err.contains("cannot take a range slice of a raw pointer"),
        "{err}"
    );
}

#[test]
fn str_from_chars_copies_a_char_slice_into_a_heap_string() {
    // The bridge from stack text to the dynamic string world; the result is
    // a real heap string that str_len and concat accept.
    assert_eq!(run("strfromchars.dusk"), "Hello\n5\nell!\n");
}

#[test]
fn an_embedded_nul_is_an_ordinary_char_array_byte() {
    // The literal copy carries the NUL and the byte printer writes through
    // it; the plain heap string keeps its C string reading and stops there.
    assert_eq!(run("charnul.dusk"), "120 0 121\nx\n");
}

#[test]
fn a_literal_backslash_before_a_brace_is_not_an_escape() {
    // Six plain bytes, in the char array count and in the printed text.
    assert_eq!(run("charbackslash.dusk"), "\\u{41}\n\\u{7a}\n");
}

#[test]
fn a_destructuring_annotation_must_match_its_member() {
    let err = check_fails("destructannot_fail.dusk");
    assert!(
        err.contains("'a' has a type annotation that does not match its value"),
        "{err}"
    );
}

#[test]
fn a_void_pointer_refuses_a_range_slice() {
    // The raw pointer refusal covers *void, which has no length either.
    let err = check_fails("voidrange_fail.dusk");
    assert!(
        err.contains("cannot take a range slice of a raw pointer"),
        "{err}"
    );
}

#[test]
fn a_non_char_array_still_has_no_printer() {
    // The char text arms must not open printing for other element types.
    let err = check_fails("charprint_fail.dusk");
    assert!(err.contains("cannot print"), "{err}");
}

#[test]
fn a_parameter_of_an_undeclared_type_is_rejected() {
    let err = check_fails("usingphantom_fail.dusk");
    assert!(err.contains("unknown type 'Collector'"), "{err}");
}

// The accept twin is the shipped allocator example, which uses a real `using`
// parameter of a declared type and runs cleanly.
golden!(usingphantom_accept, "allocator.dusk", "24\n");

#[test]
fn binding_a_whole_fallible_tuple_is_rejected() {
    let err = check_fails("tuplebind_fail.dusk");
    assert!(
        err.contains("a fallible result must be destructured"),
        "{err}"
    );
}

golden!(tuplebind_ok, "tuplebind_ok.dusk", "7\n");

#[test]
fn an_impl_without_the_oop_paradigm_is_rejected() {
    let err = check_fails("implgate_fail.dusk");
    assert!(err.contains("requires the oop paradigm"), "{err}");
}

#[test]
fn a_monad_block_missing_bind_or_unit_is_rejected() {
    let err = check_fails("monaddecl_fail.dusk");
    assert!(err.contains("must define both 'bind' and 'unit'"), "{err}");
}

golden!(monaddecl_ok, "monaddecl_ok.dusk", "7\n");

#[test]
fn a_method_call_on_an_enum_value_is_rejected() {
    let err = check_fails("enummethod_fail.dusk");
    assert!(
        err.contains("methods on the enum 'Maybe' are not supported"),
        "{err}"
    );
}

golden!(enummethod_ok, "enummethod_ok.dusk", "1\n");

#[test]
fn a_functional_builtin_with_the_wrong_arity_is_rejected() {
    let err = check_fails("foldarity_fail.dusk");
    assert!(
        err.contains("fold takes 3 argument(s), but 4 were given"),
        "{err}"
    );
}

golden!(foldarity_ok, "foldarity_ok.dusk", "6\n");

#[test]
fn an_unsigned_integer_type_name_is_reserved() {
    let err = check_fails("uinttype_fail.dusk");
    assert!(err.contains("unsigned integers are reserved"), "{err}");
}

#[test]
fn an_unsigned_integer_literal_suffix_is_reserved() {
    let err = check_fails("uintsuffix_fail.dusk");
    assert!(err.contains("unsigned integers are reserved"), "{err}");
}

golden!(uintsigned_ok, "uintsigned_ok.dusk", "5\n7\n");

#[test]
fn a_loop_pumping_call_inside_an_async_func_is_rejected() {
    let err = check_fails("asyncpump_fail.dusk");
    assert!(
        err.contains("pumps the event loop and cannot be called inside an async func"),
        "{err}"
    );
}

// Bootstrap prerequisites: std.string formatting, the float constant IR tokens,
// std.os process control, and deterministic map iteration.

golden!(
    string_formatting_helpers,
    "strfmt.dusk",
    "0\n12345\n-42\n-9223372036854775808\n9223372036854775807\n\
     0x0000000000000000\n0xFFFFFFFFFFFFFFFF\n0x00000000000000FF\n\
     hello\nworld\nllo\n\nyes\nno\nyes\n\
     n=-1000 0 -9223372036854775808\n"
);

// The expected tokens are the IEEE 754 bits computed on the host: f64_to_ir_hex
// is f64::to_bits, which is what the host compiler emits for a float constant, so
// this pins the two stages to the same textual IR. f32_to_ir_hex is the bits of
// the double a float32 literal rounds to.
golden!(
    float_constant_ir_hex_tokens,
    "floatbits.dusk",
    "0x0000000000000000\n0x3FF0000000000000\n0xBFF0000000000000\n\
     0x3FB999999999999A\n0x3FF8000000000000\n0x400921F9F01B866E\n\
     0x3FB99999A0000000\n0x3FF8000000000000\n0x400921FA00000000\n"
);

// The quoted argument carries a literal backslash, so the expected line escapes
// it. run's decode returns the child exit code, and an unset variable reads back
// as the empty string with length 0.
golden!(
    os_process_run_quote_env,
    "osproc.dusk",
    "7\n0\n1\n'plain'\n'it'\\''s a test'\n''\n0\n"
);

// map_keys walks the keys in insertion order across a grow and past an overwrite,
// so "two" reads back its overwritten value 22 in its first insertion slot and no
// key is duplicated.
golden!(
    map_deterministic_iteration,
    "mapkeys.dusk",
    "8\n1\n22\n3\n4\n5\n6\n7\n8\n"
);

#[test]
fn a_foreign_signature_rejects_a_string_parameter() {
    // A string is typed apart from a raw pointer, so it cannot cross the C
    // boundary directly; std.os copies it into a *raw char buffer first. The
    // reject is why that copy exists.
    let err = check_fails("foreignstr_fail.dusk");
    assert!(err.contains("C boundary does not support"), "{err}");
}

#[test]
fn an_installed_layout_resolves_assets_through_the_share_directory() {
    // A packaged install puts the binary under prefix/bin and the stdlib and
    // runtime under prefix/share/dusk-lang. The compiler walks up from its own
    // executable to find them, with no DUSK_HOME set and the working directory
    // pointing nowhere useful. The compiler under test is itself copied into
    // the fake prefix, so the walk under test is the installed binary's own.
    let bin = dusk_bin();
    let root = env!("CARGO_MANIFEST_DIR");
    let prefix = std::env::temp_dir().join(format!("dusk_install_{}", std::process::id()));
    let bin_dir = prefix.join("bin");
    let share = prefix.join("share").join("dusk-lang");
    std::fs::create_dir_all(&bin_dir).expect("mkdir bin");
    std::fs::create_dir_all(&share).expect("mkdir share");
    let installed = bin_dir.join("dusk");
    std::fs::copy(&bin, &installed).expect("install binary");
    let copy_tree = |name: &str| {
        let status = Command::new("cp")
            .args(["-r"])
            .arg(format!("{root}/{name}"))
            .arg(share.join(name))
            .status()
            .expect("copy asset tree");
        assert!(status.success(), "copying {name} into the fake install");
    };
    copy_tree("lib");
    copy_tree("runtime");

    let out = Command::new(&installed)
        .args(["run", &format!("{root}/examples/hello.dusk")])
        .current_dir(&prefix)
        .env_remove("DUSK_HOME")
        .output()
        .expect("run installed compiler");
    let _ = std::fs::remove_dir_all(&prefix);
    assert!(
        out.status.success(),
        "installed compiler did not run: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello, world\n");
}

#[test]
fn bootstrap_scaffold_demo() {
    // The dusk1 demo under compiler/ mirrors stage0's demo command through the
    // shared driver: assemble the spine module, write the IR, link it with clang,
    // and run it. Its stdout is byte for byte the stage0 demo's stdout, progress
    // lines included. The two runs are sequential because both write the same
    // target/dusk-out/demo artifacts.
    let bin = dusk_bin();
    let main = format!("{}/compiler/main.dusk", env!("CARGO_MANIFEST_DIR"));

    let stage0_demo = Command::new(&bin).arg("demo").output().expect("spawn dusk");
    assert!(
        stage0_demo.status.success(),
        "stage0 demo did not run cleanly: {}",
        String::from_utf8_lossy(&stage0_demo.stderr)
    );

    let demo = Command::new(&bin)
        .args(["run", &main, "demo"])
        .output()
        .expect("spawn dusk");
    assert!(
        demo.status.success(),
        "scaffold demo did not run cleanly: {}",
        String::from_utf8_lossy(&demo.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&demo.stdout),
        String::from_utf8_lossy(&stage0_demo.stdout),
        "the dusk1 demo prints the stage0 demo's stdout byte for byte"
    );

    let version = Command::new(&bin)
        .args(["run", &main, "version"])
        .output()
        .expect("spawn dusk");
    assert!(
        version.status.success(),
        "scaffold version did not run cleanly: {}",
        String::from_utf8_lossy(&version.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        "dusk 1.2.0\n",
        "version prints the canonical compiler version"
    );
}

#[test]
fn bootstrap_lex_and_scan_match_stage0() {
    // The dusk1 lexer and pre scan reproduce stage0's lex and scan dumps byte for
    // byte, the differential parity the self hosting bootstrap depends on. This is
    // a smoke check over a spread of files: plain source, Unicode escape strings,
    // rune literals, float constants, a multi paradigm header, and two lex rejects
    // that must exit non zero in both. The full oracle is tools/differential.sh
    // over all 581 corpus files.
    //
    // The scaffold is built under a unique entry stem in a temp directory so its
    // build output cannot collide with another test that builds compiler/main.dusk.
    let bin = dusk_bin();
    let root = env!("CARGO_MANIFEST_DIR");
    let compiler = std::path::Path::new(root).join("compiler");
    let tmp = std::env::temp_dir().join(format!("dusk1_parity_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mkdir temp");
    for entry in std::fs::read_dir(&compiler).expect("read compiler dir") {
        let p = entry.expect("dir entry").path();
        if p.extension().and_then(|e| e.to_str()) == Some("dusk") {
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            // Rename the entry so its build output has a stem no other test uses.
            let dest = if name == "main.dusk" {
                tmp.join("duskparity.dusk")
            } else {
                tmp.join(&name)
            };
            std::fs::copy(&p, &dest).expect("copy module");
        }
    }
    let entry = tmp.join("duskparity.dusk");
    let build = Command::new(&bin)
        .args(["build", entry.to_str().unwrap()])
        .output()
        .expect("spawn dusk");
    assert!(
        build.status.success(),
        "parity scaffold build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let scaffold = format!("{root}/target/dusk-out/duskparity");

    let samples = [
        "hello.dusk",
        "unicodeescape.dusk",
        "runebasics.dusk",
        "floatbits.dusk",
        "m9d.dusk",
        "escbadhex.dusk",
        "uintsuffix_fail.dusk",
    ];
    for cmd in ["lex", "scan"] {
        for ex in samples {
            let path = format!("{root}/examples/{ex}");
            let want = Command::new(&bin)
                .args([cmd, &path])
                .output()
                .expect("spawn stage0");
            let got = Command::new(&scaffold)
                .args([cmd, &path])
                .output()
                .expect("spawn dusk1");
            assert_eq!(
                got.stdout,
                want.stdout,
                "{cmd} stdout differs on {ex}:\nstage0: {}\ndusk1:  {}",
                String::from_utf8_lossy(&want.stdout),
                String::from_utf8_lossy(&got.stdout)
            );
            assert_eq!(
                got.status.code(),
                want.status.code(),
                "{cmd} exit code differs on {ex}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
