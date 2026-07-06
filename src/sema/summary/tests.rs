//! Unit tests for the escape summary fixpoint, the per-lambda alias table,
//! and the frame-store collection. Split from `mod`, which holds the fixpoint
//! driver and the summary shape, to keep each file focused.

use super::*;
use crate::lexer::lex;
use crate::parser::parse;

fn summarize(src: &str) -> EscapeInfo {
    let (t, le) = lex(src);
    assert!(le.is_empty(), "lex errors: {le:?}");
    let (m, pe) = parse(t);
    assert!(pe.is_empty(), "parse errors: {pe:?}");
    compute(&m)
}

fn returns_of(info: &EscapeInfo, f: &str) -> Vec<u8> {
    info.fns.get(f).expect("summary present").returns_alias.to_vec()
}

fn flows_of(info: &EscapeInfo, f: &str) -> Vec<(u8, u8)> {
    info.fns.get(f).expect("summary present").flows_into.clone()
}

fn reads_of(info: &EscapeInfo, f: &str) -> Vec<u8> {
    info.fns.get(f).expect("summary present").reads_through.to_vec()
}

fn sinks_of(info: &EscapeInfo, f: &str) -> Vec<u8> {
    info.fns.get(f).expect("summary present").sinks.to_vec()
}

fn method_sinks_of(info: &EscapeInfo, ty: &str, m: &str) -> Vec<u8> {
    info.method_summaries
        .get(&(ty.to_string(), m.to_string()))
        .expect("method summary present")
        .sinks
        .to_vec()
}

fn method_flows_of(info: &EscapeInfo, ty: &str, m: &str) -> Vec<(u8, u8)> {
    info.method_summaries
        .get(&(ty.to_string(), m.to_string()))
        .expect("method summary present")
        .flows_into
        .clone()
}

/// Every lambda literal's recorded sink set, ascending, for a source with a
/// known number of lambdas.
fn lambda_sink_sets(info: &EscapeInfo) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = info.lambda_sinks.values().map(|ps| ps.to_vec()).collect();
    v.sort();
    v
}

#[test]
fn pointer_passthrough_reads_through_its_param() {
    // A managed pointer handed straight back is not a view origin, so
    // returns_alias stays empty, but the pointee is exposed, so the result
    // reads through the pointer parameter.
    let m = summarize(
        "struct Box { rows: int64[] }\n\
         func same(p: *Box) -> *Box { return p }",
    );
    assert!(returns_of(&m, "same").is_empty());
    assert_eq!(reads_of(&m, "same"), vec![0]);
}

#[test]
fn reading_a_view_back_out_of_a_pointer_reads_through_it() {
    // Reading a fat field out of `(*p)` exposes whatever the caller stored
    // behind the pointer, so the returned view reads through the pointer.
    let m = summarize(
        "struct Box { rows: int64[] }\n\
         func first(p: *Box) -> int64[] { return (*p).rows }",
    );
    assert!(returns_of(&m, "first").is_empty());
    assert_eq!(reads_of(&m, "first"), vec![0]);
}

#[test]
fn a_pointer_read_back_threads_through_a_local() {
    // Binding the read-back to a local and returning it later still reads
    // through the pointer parameter.
    let m = summarize(
        "struct Box { rows: int64[] }\n\
         func first(p: *Box) -> int64[] {\n\
           got := (*p).rows\n\
           return got\n\
         }",
    );
    assert_eq!(reads_of(&m, "first"), vec![0]);
}

#[test]
fn a_scalar_pointer_passthrough_still_reads_through_but_never_leaks() {
    // A `*int64` passthrough reads through its parameter; the enforcement
    // checks the caller's argument, which can never hold a frame view behind a
    // scalar pointee, so this is imprecise-but-sound and never over-rejects.
    let m = summarize("func passthru(p: *int64) -> *int64 { return p }");
    assert!(returns_of(&m, "passthru").is_empty());
    assert_eq!(reads_of(&m, "passthru"), vec![0]);
}

#[test]
fn identity_passthrough_returns_its_param() {
    let m = summarize("func identity(s: int64[]) -> int64[] { return s }");
    assert_eq!(returns_of(&m, "identity"), vec![0]);
    assert!(flows_of(&m, "identity").is_empty());
}

#[test]
fn two_hop_propagates_returns_alias() {
    let m = summarize(
        "func identity(s: int64[]) -> int64[] { return s }\n\
         func two_hop(s: int64[]) -> int64[] { return identity(s) }",
    );
    assert_eq!(returns_of(&m, "two_hop"), vec![0]);
}

#[test]
fn chan_send_sinks_its_pointer_param() {
    // A relay(ch, c) helper forwards its pointer parameter to chan_send, so the
    // parameter's pointee crosses a thread boundary the receiver outlives; the
    // sink is recorded by name, independent of chan_send's own library summary.
    let m = summarize(
        "struct Box { rows: int64[] }\n\
         func relay(ch: *void, c: *Box) -> error { return chan_send(ch, c) }",
    );
    assert_eq!(sinks_of(&m, "relay"), vec![1]);
}

#[test]
fn chan_send_sinks_its_slice_param() {
    // A slice parameter sent directly is a view origin, so it enters the sink
    // set through its origins the same way a pointer enters through its reads.
    let m = summarize("func relay(ch: *void, s: int64[]) -> error { return chan_send(ch, s) }");
    assert_eq!(sinks_of(&m, "relay"), vec![1]);
}

#[test]
fn chan_try_send_sinks_its_param() {
    // The non-blocking send is a sink the same way the blocking one is: the
    // element still crosses to the receiver thread (the u10 hole).
    let m = summarize(
        "struct Box { rows: int64[] }\n\
         func relay(ch: *void, c: *Box) -> error { return chan_try_send(ch, c) }",
    );
    assert_eq!(sinks_of(&m, "relay"), vec![1]);
}

#[test]
fn a_method_that_sends_self_sinks_parameter_zero() {
    // A method's hidden first parameter is the by-pointer receiver, so a body
    // that hands `self` to chan_send sinks parameter 0: the receiver crosses a
    // thread boundary the receiver thread outlives. The declared parameter `ch`
    // follows self at index 1, so the sink names 0, not the channel.
    let m = summarize(
        "struct Box { rows: int64[] }\n\
         impl Box { func ship(ch: *void) -> error { return chan_send(ch, self) } }",
    );
    assert_eq!(method_sinks_of(&m, "Box", "ship"), vec![0]);
}

#[test]
fn a_method_that_stores_a_param_through_self_flows_into_zero() {
    // A method storing its slice parameter through the receiver, `(*self).rows =
    // s`, records the store edge from the declared parameter (index 1, since self
    // occupies 0) into self (index 0). The call-site flow application then
    // pollutes the receiver's binding when the caller supplies a frame view.
    let m = summarize(
        "struct Box { rows: int64[] }\n\
         impl Box { func fill(s: int64[]) -> void { (*self).rows = s } }",
    );
    assert_eq!(method_flows_of(&m, "Box", "fill"), vec![(1, 0)]);
}

#[test]
fn a_relay_helper_propagates_the_sink_two_hops() {
    // relay2 forwards to relay, which sends; the sink threads up the chain so a
    // caller of relay2 is warned its argument crosses the channel two hops away.
    let m = summarize(
        "struct Box { rows: int64[] }\n\
         func relay(ch: *void, c: *Box) -> error { return chan_send(ch, c) }\n\
         func relay2(ch: *void, c: *Box) -> error { return relay(ch, c) }",
    );
    assert_eq!(sinks_of(&m, "relay"), vec![1]);
    assert_eq!(sinks_of(&m, "relay2"), vec![1]);
}

#[test]
fn a_fused_store_then_send_closes_the_sink_over_the_store_edge() {
    // One body both stores the slice parameter into the pointer's heap object and
    // sends that pointer, so `flows_into (2, 1)` and `sinks {1}` co-exist. The
    // sink set closes backward over the store edge, so the source parameter 2
    // sinks too: a caller handing a frame view in that position crosses the
    // channel, which the pre-call sink check cannot otherwise see.
    let m = summarize(
        "struct Cell { rows: int64[] }\n\
         func evil(ch: *void, c: *Cell, s: int64[]) -> error {\n\
           (*c).rows = s\n\
           return chan_send(ch, c)\n\
         }",
    );
    assert_eq!(flows_of(&m, "evil"), vec![(2, 1)]);
    assert_eq!(sinks_of(&m, "evil"), vec![1, 2]);
}

#[test]
fn the_closed_sink_threads_through_a_helper_that_calls_the_fused_sender() {
    // The two-hop twin: a helper forwards its arguments to the fused store-and-send
    // callee. The callee's closed sink set {1, 2} propagates through the call, and
    // the helper's own store edge closes identically, so the helper sinks both its
    // pointer and its slice parameter one hop up.
    let m = summarize(
        "struct Cell { rows: int64[] }\n\
         func evil(ch: *void, c: *Cell, s: int64[]) -> error {\n\
           (*c).rows = s\n\
           return chan_send(ch, c)\n\
         }\n\
         func helper(ch: *void, c: *Cell, s: int64[]) -> error {\n\
           return evil(ch, c, s)\n\
         }",
    );
    assert_eq!(sinks_of(&m, "evil"), vec![1, 2]);
    assert_eq!(sinks_of(&m, "helper"), vec![1, 2]);
}

#[test]
fn a_store_of_a_non_sunk_param_does_not_become_a_sink() {
    // The closure is directed by the sink set, not by the store edges alone: a
    // parameter stored into a pointer that is never sent stays out of the sink
    // set, so the accept side is not over-rejected.
    let m = summarize(
        "struct Cell { rows: int64[] }\n\
         func stash(c: *Cell, s: int64[]) -> void {\n\
           (*c).rows = s\n\
         }",
    );
    assert_eq!(flows_of(&m, "stash"), vec![(1, 0)]);
    assert!(sinks_of(&m, "stash").is_empty());
}

#[test]
fn sending_a_fresh_scalar_records_no_sink() {
    // A scalar local sent over a channel carries no parameter view or pointee,
    // so no sink is recorded and no caller argument is over-rejected.
    let m = summarize(
        "func emit(ch: *void) -> error {\n\
           x := 5\n\
           return chan_send(ch, x)\n\
         }",
    );
    assert!(sinks_of(&m, "emit").is_empty());
}

#[test]
fn a_lambda_sending_its_pointer_param_records_a_lambda_sink() {
    // A lambda bound to a local sends its own parameter over a channel. It carries
    // no computed summary, so the walk records its sink set keyed by span, which
    // the checker reads at a direct call of the bound name. The captured channel
    // contributes no origin, so only the sent parameter is named.
    let m = summarize(
        "struct Cell { rows: int64[] }\n\
         func emit(ch: Channel<*Cell>, c: *Cell) -> void {\n\
           sender := lambda (x: *Cell) -> void {\n\
             e := chan_send(ch, x)\n\
             e.ignore()\n\
           }\n\
           sender(c)\n\
         }",
    );
    assert_eq!(lambda_sink_sets(&m), vec![vec![0]]);
}

#[test]
fn an_opaque_function_value_call_sinks_the_argument_it_forwards() {
    // run(f, c) { f(c) } hands its pointer parameter to an opaque function value.
    // The callee may send it across a channel, so run sinks that parameter, the
    // relation that catches a sinking lambda passed as f one hop up (the w2 hole).
    let m = summarize(
        "struct Cell { rows: int64[] }\n\
         func run(f: (*Cell) -> void, c: *Cell) -> void { f(c) }",
    );
    assert_eq!(sinks_of(&m, "run"), vec![1]);
}

#[test]
fn a_named_funcvalue_binding_resolves_to_its_target_and_does_not_sink() {
    // A local bound once to a known function, never reassigned, resolves to that
    // function's real summary instead of the opaque TOP, so `f := id; f(data)`
    // aliases the argument through id's return relation and records no sink. This
    // is what keeps a channel-free func-value call from a spurious send diagnostic.
    let m = summarize(
        "func id(s: int64[]) -> int64[] { return s }\n\
         func viewer(data: int64[]) -> int64[] { f := id\n return f(data) }",
    );
    assert_eq!(returns_of(&m, "viewer"), vec![0]);
    assert!(sinks_of(&m, "viewer").is_empty(), "{:?}", sinks_of(&m, "viewer"));
}

#[test]
fn a_reassigned_funcvalue_binding_stays_opaque_and_sinks() {
    // The resolution is sound only for a name bound to one fixed function: a name
    // reassigned is poisoned, so its call falls back to the opaque TOP that
    // conservatively sinks the forwarded argument.
    let m = summarize(
        "func id(s: int64[]) -> int64[] { return s }\n\
         func other(s: int64[]) -> int64[] { return s }\n\
         func pick(data: int64[]) -> int64[] {\n\
           f := id\n\
           f = other\n\
           return f(data)\n\
         }",
    );
    assert_eq!(sinks_of(&m, "pick"), vec![0]);
}

#[test]
fn tuple_wrap_return_aliases_member_param() {
    let m = summarize("func wrap(s: int64[]) -> (int64[], int64) { return (s, 5) }");
    assert_eq!(returns_of(&m, "wrap"), vec![0]);
}

#[test]
fn recursion_converges_to_alias() {
    let m = summarize(
        "func rec(s: int64[], n: int64) -> int64[] {\n\
           if n == 0 { return s }\n\
           return rec(s, n - 1)\n\
         }",
    );
    assert_eq!(returns_of(&m, "rec"), vec![0]);
}

#[test]
fn mutual_recursion_converges_to_alias() {
    let m = summarize(
        "func ping(s: int64[], n: int64) -> int64[] {\n\
           if n == 0 { return s }\n\
           return pong(s, n - 1)\n\
         }\n\
         func pong(s: int64[], n: int64) -> int64[] {\n\
           if n == 0 { return s }\n\
           return ping(s, n - 1)\n\
         }",
    );
    assert_eq!(returns_of(&m, "ping"), vec![0]);
    assert_eq!(returns_of(&m, "pong"), vec![0]);
}

#[test]
fn closure_callee_is_top() {
    // g is an opaque function-typed parameter, so the call is TOP: both view
    // arguments may be returned, and each may flow into the other.
    let m = summarize(
        "func via2(g: (int64[], int64[]) -> int64[], a: int64[], b: int64[]) -> int64[] {\n\
           return g(a, b)\n\
         }",
    );
    assert_eq!(returns_of(&m, "via2"), vec![1, 2]);
    let flows = flows_of(&m, "via2");
    assert!(flows.contains(&(1, 2)), "flows: {flows:?}");
    assert!(flows.contains(&(2, 1)), "flows: {flows:?}");
}

#[test]
fn store_into_param_records_flow_edge() {
    let m = summarize(
        "func store_into(s: int64[], dst: int64[][]) -> void {\n\
           dst[0] = s\n\
         }",
    );
    assert_eq!(flows_of(&m, "store_into"), vec![(0, 1)]);
    assert!(returns_of(&m, "store_into").is_empty());
}

#[test]
fn store_through_pointer_param_records_flow_edge() {
    let m = summarize(
        "struct Box { rows: int64[] }\n\
         func stash(s: int64[], dst: *Box) -> void {\n\
           (*dst).rows = s\n\
         }",
    );
    assert_eq!(flows_of(&m, "stash"), vec![(0, 1)]);
}

#[test]
fn scalar_fn_is_empty() {
    let m = summarize("func add(a: int64, b: int64) -> int64 { return a + b }");
    assert!(returns_of(&m, "add").is_empty());
    assert!(flows_of(&m, "add").is_empty());
}

#[test]
fn managed_pointer_param_is_not_an_origin() {
    // A managed *T is heap-backed and covered by the generation backstop, so
    // returning it does not alias a frame view: the summary stays empty.
    let m = summarize("func passthru(p: *int64) -> *int64 { return p }");
    assert!(returns_of(&m, "passthru").is_empty());
    assert!(flows_of(&m, "passthru").is_empty());
}

#[test]
fn move_builtin_is_identity_but_managed_arg_is_clean() {
    // move(view) aliases the argument; move(*T) does not, since a managed
    // pointer is not a view origin.
    let m = summarize(
        "func mv_view(s: int64[]) -> int64[] { return move(s) }\n\
         func mv_ptr(p: *int64) -> *int64 { return move(p) }",
    );
    assert_eq!(returns_of(&m, "mv_view"), vec![0]);
    assert!(returns_of(&m, "mv_ptr").is_empty());
}

#[test]
fn reslice_of_param_aliases_it() {
    let m = summarize("func head(s: int64[]) -> int64[] { return s[0..2] }");
    assert_eq!(returns_of(&m, "head"), vec![0]);
}

#[test]
fn local_array_slice_does_not_alias_a_param() {
    // A slice into a local array is a frame view, not an alias of any
    // parameter, so returns_alias stays empty (the frame flag, tracked but
    // not enforced here, is the intraprocedural concern).
    let m = summarize(
        "func mk() -> int64[] {\n\
           xs := [1, 2, 3]\n\
           return xs[0..3]\n\
         }",
    );
    assert!(returns_of(&m, "mk").is_empty());
}

#[test]
fn a_readback_stored_into_a_second_param_records_a_flow_edge() {
    // The stored value's reads travel the store edge exactly as its origins
    // do, so a pointer read-back landing in another parameter's place is an
    // edge the caller can apply its pointer argument's taint to.
    let m = summarize(
        "struct Cell { rows: int64[] }\n\
         func shuffle(p: *Cell, dst: int64[][]) -> void {\n\
           dst[0] = (*p).rows\n\
         }",
    );
    assert_eq!(flows_of(&m, "shuffle"), vec![(0, 1)]);
}

#[test]
fn a_struct_wrapped_pointer_param_seeds_reads_through() {
    // The seed is by type reachability: a Holder that buries a *Cell exposes
    // the pointee the same way a bare pointer parameter does.
    let m = summarize(
        "struct Cell { rows: int64[] }\n\
         struct Holder { c: *Cell }\n\
         func get_rows(h: Holder) -> int64[] { return (*h.c).rows }",
    );
    assert_eq!(reads_of(&m, "get_rows"), vec![0]);
}

#[test]
fn a_map_lambda_aliasing_its_element_through_a_local_leaks_the_collection() {
    // The lambda body gets the same abstract walk a function does, so the
    // local alias chain `t := r; return t` still reaches the element.
    let m = summarize(
        "func mk(rows: int64[][]) -> int64[][] {\n\
           return map(rows, lambda (r: int64[]) -> int64[] {\n\
             t := r\n\
             return t\n\
           })\n\
         }",
    );
    assert_eq!(returns_of(&m, "mk"), vec![0]);
}

#[test]
fn a_map_lambda_laundering_its_element_through_a_call_leaks_the_collection() {
    let m = summarize(
        "func id(s: int64[]) -> int64[] { return s }\n\
         func mk(rows: int64[][]) -> int64[][] {\n\
           return map(rows, lambda (r: int64[]) -> int64[] { return id(r) })\n\
         }",
    );
    assert_eq!(returns_of(&m, "mk"), vec![0]);
}

#[test]
fn a_map_lambda_minting_a_scalar_stays_clean() {
    let m = summarize(
        "func mk(rows: int64[][]) -> int64[] {\n\
           return map(rows, lambda (r: int64[]) -> int64 { return r[0] })\n\
         }",
    );
    assert!(returns_of(&m, "mk").is_empty());
}

#[test]
fn filter_aliases_its_collection_whenever_elements_carry_views() {
    // The predicate is irrelevant: filter's result is a subset of the
    // collection's elements, so a view element type leaks the collection and
    // a scalar one cannot.
    let m = summarize(
        "func keep(rows: int64[][]) -> int64[][] {\n\
           return filter(rows, lambda (r: int64[]) -> bool { return true })\n\
         }\n\
         func nums(xs: int64[]) -> int64[] {\n\
           return filter(xs, lambda (x: int64) -> bool { return x > 0 })\n\
         }",
    );
    assert_eq!(returns_of(&m, "keep"), vec![0]);
    assert!(returns_of(&m, "nums").is_empty());
}

#[test]
fn a_fold_lambda_returning_its_element_leaks_the_collection() {
    // fold threads both the seed (through the accumulator parameter) and the
    // collection (through the element parameter); this lambda returns the
    // element, so the collection argument aliases the result.
    let m = summarize(
        "func mk(rows: int64[][], seed: int64[]) -> int64[] {\n\
           return fold(rows, seed, lambda (acc: int64[], x: int64[]) -> int64[] { return x })\n\
         }",
    );
    assert_eq!(returns_of(&m, "mk"), vec![0]);
}

#[test]
fn a_foreach_lambda_storing_its_element_through_a_capture_records_the_edge() {
    // The lambda body is walked with its element parameter seeded from the
    // collection, and the captured destination resolves to the enclosing
    // parameter, so the store is a flow edge from arg 0 into arg 1.
    let m = summarize(
        "func fill(rows: int64[][], dst: int64[][]) -> void {\n\
           foreach(rows, lambda (r: int64[]) -> void {\n\
             dst[0] = r\n\
           })\n\
         }",
    );
    assert!(flows_of(&m, "fill").contains(&(0, 1)), "{:?}", flows_of(&m, "fill"));
}

#[test]
fn a_direct_frame_view_store_into_a_param_place_is_a_frame_store() {
    // The frame bit rides the store edge: a view of fill's own frame landing
    // in a parameter place is recorded as a frame-store site, which the
    // checker turns into a diagnostic outright.
    let m = summarize(
        "func fill(dst: int64[][]) -> void {\n\
           local: int64[4] = [1, 2, 3, 4]\n\
           dst[0] = local[0..4]\n\
         }",
    );
    assert_eq!(m.frame_stores.len(), 1, "{:?}", m.frame_stores);
    assert_eq!(m.frame_stores[0].1, 0);
}

#[test]
fn a_heap_view_store_into_a_param_place_is_not_a_frame_store() {
    let m = summarize(
        "func heap() -> int64[] { return map([1, 2][0..2], lambda (x: int64) -> int64 { return x }) }\n\
         func fill(dst: int64[][]) -> void {\n\
           dst[0] = heap()\n\
         }",
    );
    assert!(m.frame_stores.is_empty(), "{:?}", m.frame_stores);
}

#[test]
fn a_frame_store_through_a_borrowed_pointer_lambda_capture_is_recorded() {
    // The capture-store side of the frame-store table: the foreach lambda
    // stores a view of fill's frame through the captured parameter.
    let m = summarize(
        "func fill(dst: int64[][]) -> void {\n\
           a: int64[2] = [1, 2]\n\
           pair: int64[][2] = [a[0..2], a[0..2]]\n\
           foreach(pair[0..2], lambda (r: int64[]) -> void {\n\
             dst[0] = r\n\
           })\n\
         }",
    );
    assert_eq!(m.frame_stores.len(), 1, "{:?}", m.frame_stores);
    assert_eq!(m.frame_stores[0].1, 0);
}

#[test]
fn a_lambda_that_stores_its_param_through_a_captured_pointer_records_a_capture_flow() {
    // The capture-flow table: setter's parameter 0 is stored through the captured
    // pointer c, which is neither one of the lambda's parameters nor a local of its
    // body, so the edge names the captured binding by name. The argument-to-argument
    // store model cannot see this, so the checker reads this table at the call.
    let m = summarize(
        "struct Cell { rows: int64[] }\n\
         func mk(c: *Cell) -> void {\n\
           setter := lambda (s: int64[]) -> void { (*c).rows = s }\n\
           local: int64[4] = [1, 2, 3, 4]\n\
           setter(local[0..4])\n\
         }",
    );
    let edges: Vec<Vec<(u8, String)>> = m.lambda_capture_flows.values().cloned().collect();
    assert_eq!(edges.len(), 1, "one lambda records a capture flow: {edges:?}");
    assert_eq!(edges[0], vec![(0u8, "c".to_string())], "param 0 flows into capture c");
}

#[test]
fn a_lambda_that_only_reads_its_capture_records_no_capture_flow() {
    // A capturing lambda that never stores its parameter through the capture has no
    // capture-flow edge, so the checker raises nothing at its call. Only a store
    // whose destination roots at a free, captured name records an edge.
    let m = summarize(
        "struct Cell { rows: int64[] }\n\
         func mk(c: *Cell) -> int64 {\n\
           reader := lambda () -> int64 { return (*c).rows[0] }\n\
           return reader()\n\
         }",
    );
    assert!(m.lambda_capture_flows.is_empty(), "{:?}", m.lambda_capture_flows);
}
