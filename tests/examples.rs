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
fn pool_shutdown_from_a_pool_task_faults() {
    let (out, err, ok) = run_raw("poolself.dusk");
    assert!(!ok, "a pool task shutting the pool down must fault");
    assert_eq!(out, "submitting\n", "the print before the drain survives the abort");
    assert!(err.contains("cannot shut down from inside a pool task"), "{err}");
}

#[test]
fn freeing_a_condvar_with_a_waiter_faults() {
    let (out, err, ok) = run_raw("condfree.dusk");
    assert!(!ok, "freeing a condvar with a parked waiter must fault");
    assert_eq!(out, "freeing\n", "the print before the free survives the abort");
    assert!(err.contains("condvar freed while threads wait"), "{err}");
}

#[test]
fn awaiting_a_consumed_future_faults() {
    let (out, err, ok) = run_raw("doubleawait.dusk");
    assert!(!ok, "the second await of one future must fault");
    assert_eq!(out, "1\n", "the first await's value prints before the fault");
    assert!(err.contains("fatal: use of a dead future"), "{err}");
}

#[test]
fn awaiting_off_the_loop_thread_faults() {
    let (_, err, ok) = run_raw("offthreadawait.dusk");
    assert!(!ok, "an await from a spawned thread must fault");
    assert!(err.contains("fatal: the event loop was touched off its thread"), "{err}");
}

#[test]
fn an_unfinishable_await_faults_instead_of_hanging() {
    let (_, err, ok) = run_raw("idledeadlock.dusk");
    assert!(!ok, "an await nothing can complete must fault");
    assert!(err.contains("fatal: the event loop is idle but work is still pending"), "{err}");
}

#[test]
fn a_completer_exiting_reawakens_the_deadlock_gate() {
    let (_, err, ok) = run_raw("threadexitdeadlock.dusk");
    assert!(!ok, "the await must fault once its last possible completer exits");
    assert!(err.contains("fatal: the event loop is idle but work is still pending"), "{err}");
}

#[test]
fn a_future_before_loop_init_faults() {
    let (_, err, ok) = run_raw("noloop.dusk");
    assert!(!ok, "minting a future before loop_init must fault");
    assert!(err.contains("fatal: the event loop is not running"), "{err}");
}

#[test]
fn a_reactor_stopped_with_an_armed_watch_faults() {
    let (out, err, ok) = run_raw("watchleak.dusk");
    assert!(!ok, "stopping the reactor with a watch still armed must fault");
    assert_eq!(out, "await timed out\n0\n", "the timeout prints before the fault");
    assert!(err.contains("fatal: the reactor stopped while a watch is still armed"), "{err}");
}

#[test]
fn a_second_watch_on_an_armed_fd_faults() {
    let (out, err, ok) = run_raw("doublewatch.dusk");
    assert!(!ok, "a second watch on an already armed fd must fault");
    assert_eq!(out, "future is pending\n0\n", "the pending poll prints before the fault");
    assert!(err.contains("fatal: the file descriptor already has an armed watch"), "{err}");
}

#[test]
fn a_watch_on_an_invalid_fd_faults() {
    let (out, err, ok) = run_raw("badfdwatch.dusk");
    assert!(!ok, "arming a watch on an invalid fd must fault");
    assert_eq!(out, "arming\n", "the print before the arm survives the fault");
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
    assert!(err.contains("'&' mixes int32 and int64; match the widths"), "{err}");
}

#[test]
fn bitwise_and_on_bools_is_rejected() {
    let err = check_fails("boolbit.dusk");
    assert!(err.contains("bitwise operators need integer operands"), "{err}");
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
    assert!(err.contains("shift amount 32 is out of range for int32"), "{err}");
}

#[test]
fn a_constant_negative_integer_exponent_is_rejected() {
    let err = check_fails("powneg.dusk");
    assert!(err.contains("'**' on integers needs a nonnegative exponent"), "{err}");
}

#[test]
fn pow_mixing_int_and_float_is_rejected() {
    let err = check_fails("powmix.dusk");
    assert!(err.contains("'**' needs two operands of the same numeric type"), "{err}");
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
    assert!(err.contains("expected an expression, found PlusPlus"), "{err}");
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
    assert!(err.contains("'await' is only legal inside an async func"), "{err}");
}

#[test]
fn await_mid_expression_is_rejected() {
    let err = check_fails("asyncawaitmidexpr.dusk");
    assert!(err.contains("'await' cannot appear mid-expression"), "{err}");
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
    assert!(err.contains("an async func cannot take type parameters"), "{err}");
}

#[test]
fn an_async_func_cannot_take_a_slice_param() {
    let err = check_fails("asyncsliceparam.dusk");
    assert!(err.contains("an async func cannot take 'xs'"), "{err}");
}

#[test]
fn an_async_func_cannot_take_a_future_param() {
    let err = check_fails("asyncfutureparam.dusk");
    assert!(err.contains("a future belongs to the event loop thread"), "{err}");
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
    assert!(err.contains("the future from 'g' is never awaited"), "{err}");
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

#[test]
fn async_run_inside_an_async_func_is_rejected() {
    let err = check_fails("asyncruninside.dusk");
    assert!(err.contains("async_run cannot be called inside an async func"), "{err}");
}

#[test]
fn async_run_of_a_bound_future_is_rejected() {
    let err = check_fails("asyncrunnondirect.dusk");
    assert!(err.contains("async_run takes a direct call of an async func"), "{err}");
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
    let bin = env!("CARGO_BIN_EXE_dusk");
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
fn reassigning_a_tuple_binding_to_a_clean_value_checks_ok() {
    // The reassign-to-clean no-over-reject: a mut binding that started escaping is
    // reassigned to a param slice, so the stale flag is cleared and the return is
    // legal. Checked only, since building a tuple whose member representation
    // changes across the reassignment is a separate codegen concern.
    check_ok("tuple_reassign_clean_ok.dusk");
}

#[test]
fn a_conditional_reassignment_to_an_escaping_tuple_is_rejected() {
    let err = check_fails("flowmerge_if_tuple.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_conditional_re_slice_of_a_local_array_is_rejected() {
    let err = check_fails("flowmerge_reslice_in_if.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_conditional_reassignment_to_a_capturing_lambda_is_rejected() {
    let err = check_fails("flowmerge_closure_if.dusk");
    assert!(err.contains("a closure that captures a local escapes its frame"), "{err}");
}

#[test]
fn a_reassignment_to_an_escaping_value_in_a_while_body_is_rejected() {
    let err = check_fails("flowmerge_while.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_reassignment_to_an_escaping_value_nested_in_two_ifs_is_rejected() {
    let err = check_fails("flowmerge_nested_if.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_reassignment_to_an_escaping_value_in_one_if_arm_is_rejected() {
    let err = check_fails("flowmerge_one_arm.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
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
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_struct_field_reassigned_to_a_local_view_is_rejected() {
    let err = check_fails("escstruct_field_reassign.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_struct_with_a_closure_field_capturing_a_local_is_rejected() {
    let err = check_fails("escstruct_closure_field.dusk");
    assert!(err.contains("a closure that captures a local escapes its frame"), "{err}");
}

#[test]
fn a_frame_local_struct_laundered_through_an_alias_is_rejected() {
    let err = check_fails("escstruct_via_alias.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_nested_struct_with_a_buried_local_view_is_rejected() {
    let err = check_fails("escstruct_nested.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_conditional_struct_field_store_of_a_local_view_is_rejected() {
    let err = check_fails("escstruct_branch_reassign.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
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
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_frame_local_enum_laundered_through_a_binding_is_rejected() {
    let err = check_fails("escenum_via_binding.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_conditional_enum_reassignment_to_a_local_payload_is_rejected() {
    let err = check_fails("escenum_branch.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_fixed_array_whose_elements_view_a_local_is_rejected() {
    let err = check_fails("escarray_slice_elems.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_frame_local_array_of_slices_via_a_binding_is_rejected() {
    let err = check_fails("escarray_via_binding.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_conditional_array_reassignment_to_local_element_views_is_rejected() {
    let err = check_fails("escarray_branch.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
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
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn an_enum_payload_of_enum_type_wrapping_a_local_is_rejected() {
    let err = check_fails("escdepth_enum_of_enum.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_struct_field_of_fat_array_type_viewing_a_local_is_rejected() {
    let err = check_fails("escdepth_struct_of_fatarray.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_generic_struct_field_burying_a_local_view_is_rejected() {
    let err = check_fails("escdepth_generic_box.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_nested_carrier_laundered_through_a_binding_is_rejected() {
    let err = check_fails("escdepth_via_binding.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_conditional_reassignment_of_a_nested_carrier_is_rejected() {
    let err = check_fails("escdepth_branch.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
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
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn projecting_an_element_out_of_a_local_array_of_slices_is_rejected() {
    let err = check_fails("escproj_index.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_match_arm_projecting_an_escaping_payload_is_rejected() {
    let err = check_fails("escproj_match.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_projection_placed_into_a_struct_field_is_rejected() {
    let err = check_fails("escproj_into_struct.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn returning_an_interface_value_by_value_is_rejected() {
    let err = check_fails("esciface_return.dusk");
    assert!(err.contains("returning an interface value is not supported"), "{err}");
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
fn projecting_a_slice_field_out_of_a_param_backed_struct_runs() {
    // The no-over-reject for projections: a member of a param-backed aggregate is
    // caller-owned and legal to return.
    assert_eq!(run("proj_from_param_ok.dusk"), "10\n");
}

#[test]
fn indexing_a_slice_field_of_a_local_struct_is_rejected() {
    let err = check_fails("escsliceidx.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
}

#[test]
fn a_slice_index_projection_via_a_binding_is_rejected() {
    let err = check_fails("escsliceidx_via_binding.dusk");
    assert!(err.contains("a slice into a local array escapes its frame"), "{err}");
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
    assert!(err.contains("arithmetic mixes int32 and int64; match the widths"), "{err}");
}

#[test]
fn regression_pin_generic_do_annotation_element_clash_is_rejected() {
    // REGRESSION-PIN (0.4.3 F-M1 Option C): before the fix, a generic `do`
    // binding's annotation could clash with the element type produced inside
    // the `do` and reach clang unchecked. The types-only re-check must reject
    // it at `dusk check`.
    let err = check_fails("genericpin.dusk");
    assert!(err.contains("return type does not match the function's return type"), "{err}");
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
    // F-M3's IO is eager over its carried value precisely because a field
    // cannot hold a suspended thunk that escapes the frame that built it; a
    // lazy IO storing a capturing lambda in a struct field, returned out of
    // its constructing function, must be rejected the same way any other
    // closure escape is.
    let err = check_fails("iomonadbad.dusk");
    assert!(
        err.contains("a closure that captures a local escapes its frame; it cannot be returned"),
        "{err}"
    );
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
golden!(poolsum, "poolsum.dusk", "5050\n");
golden!(poolstress, "poolstress.dusk", "10000\n");
golden!(submitshut, "submitshut.dusk", "refused before start\n7\nrefused after shutdown\n");
golden!(trypoll, "trypoll.dusk", "full\n9\n");
golden!(recvtimeout, "recvtimeout.dusk", "timed out\n0\n5\nclosed\n0\n");
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
golden!(doublecomplete, "doublecomplete.dusk", "future already completed\n4\n");
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
golden!(fdexhaust_pipe, "fdexhaust_pipe.dusk", "too many open files\nok\n");
golden!(fdexhaust_connect, "fdexhaust_connect.dusk", "too many open files\nok\n");
golden!(fdexhaust_accept, "fdexhaust_accept.dusk", "too many open files\nok\n");
golden!(display, "display.dusk", "point\npoint\n");
golden!(fmtesc, "fmtesc.dusk", "{}\na {b} c\n{} 1\n");
golden!(emptyerr, "emptyerr.dusk", "\nafter\n");
golden!(privacy, "privacy.dusk", "1\n2\n");
golden!(bitops, "bitops.dusk", "8\n14\n6\n-13\n255\n-1\n48\n");
golden!(shifts, "shifts.dusk", "8\n10\n-4\n-1\n4611686018427387904\n8\n-128\n");
golden!(powers, "powers.dusk", "1024\n512\n4\n1\n-27\n32\n1024\n6.25\n");
golden!(precedence, "precedence.dusk", "24\n32\n7\n18\n1\n2\n");
golden!(compound, "compound.dusk", "3\n0.5\n");
golden!(singleval, "singleval.dusk", "9\n25\n11\n");
golden!(incdec, "incdec.dusk", "6\n-128\n1\n");
golden!(pipes, "pipes.dusk", "10\n13\n6\n");
golden!(inclusive, "inclusive.dusk", "9\n0\n15\n");
golden!(genericmaybe, "genericmaybe.dusk", "Some(30)\nnone\n");
golden!(doasync, "doasync.dusk", "17\n");
golden!(iomonad, "iomonad.dusk", "30\n");
golden!(tcplocal, "tcplocal.dusk", "ping\n");
golden!(acceptloop, "acceptloop.dusk", "6\n");
golden!(stress_timers, "stress_timers.dusk", "2000\n");
golden!(stress_tasks, "stress_tasks.dusk", "1000\n");
golden!(stress_accept, "stress_accept.dusk", "4950\n");
golden!(stress_pool, "stress_pool.dusk", "49995000\n");

#[test]
fn awaiting_a_net_future_outside_an_async_func_is_rejected() {
    let err = check_fails("netbadawait.dusk");
    assert!(err.contains("'await' is only legal inside an async func"), "{err}");
}

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
