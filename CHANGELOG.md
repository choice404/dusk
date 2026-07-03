# Changelog

Notable changes to the dusk compiler, the standard library, and the dawn package tool. Each entry matches a tagged release, newest first. Commit messages carry the highlights and this file carries the detail.

## 0.4.1

The reactor, the second phase of the async line. File descriptor readiness becomes a one shot future on the event loop, one C thread that turns `epoll` readiness into a completion behind a new `std.async.io`, with pipes as the deterministic rig every golden runs it through. Zero compiler changes and one Rust line, the new runtime file joining the link; everything else is the runtime file, one standard library module, and the reference.

The reactor thread and its lifecycle.

- `reactor_start() -> error` starts the thread; the error fires on a double start, an operating system refusal setting up its epoll descriptor and its stop sentinel, or a start landing while a concurrent stop is still in flight. `reactor_stop() -> void` flips the reactor stopped, signals the thread through an eventfd, waits for it to finish delivering everything already ready, then joins.
- The reactor is restartable: a fresh epoll descriptor and eventfd on every start, mirroring the loop it serves, since none of its state survives a stop. The sanctioned order is `loop_init`, `reactor_start`, every watch armed and fired, `reactor_stop`, `loop_free`.
- The reactor thread never raises the live thread gauge; it is spawned with a bare `pthread_create`, not the tracked spawn path, or the deadlock gate could never fire while the reactor idles. Its own mutex guards only lifecycle fields and is never held at the same time as the loop's, the lock ordering rule stated in the runtime file's header comment.

The watches and the gauge.

- `readable(fd) -> Future<int64>` and `writable(fd) -> Future<int64>` in `std.async.io` arm a one shot watch and return a future completed with the readiness mask, 1 readable, 2 writable, 4 hangup, 8 error, ORed together. The watch fires exactly once by construction and the reactor drops it the instant it fires.
- Only one armed watch is allowed per file descriptor at a time; a second watch on an fd that already carries one faults rather than errors, since the signatures carry no error channel. `future_free` does not disarm a watch: a freed future's watch stays armed until it later fires against a dead record and loses like any other refused completer.
- Arming a watch raises a third gauge into the deadlock gate beside the live thread count and the pool's in flight count: an armed watch is a possible completer, so an otherwise idle await keeps parking instead of aborting. The gauge is raised before the watch is registered and dropped strictly after its completion is visible under the loop's lock, so the idle fatal can never fire against a completion still in flight, and every drop still kicks the loop even when the completion it followed was refused.

The byte surface.

- `pipe_new() -> (Pipe, error)` makes a close on exec, blocking by default pipe; `fd_nonblock(fd) -> error` sets a descriptor non blocking; `fd_close(fd) -> error` closes one. `read_nb` and `write_nb` move bytes through a caller staged buffer, the channel element idiom, and never block.
- Both refuse with "would block" when the operating system has nothing to give or take, the one canonical recoverable string in both directions. A `read_nb` returning a count of zero with no error is end of stream, every writer closed.
- Writing to a pipe whose read end is closed delivers SIGPIPE and kills the process. Signal hardening is deferred to a later phase; the honesty note lands in the reference and no golden writes to a pipe with no reader.

The fault family.

- Five new fatals, each named: "the reactor is not running" arming before a start or after a stop, "the file descriptor already has an armed watch" for a second watch on an armed fd, "a readiness watch was armed on an invalid file descriptor" for a closed or nonexistent fd, "a regular file cannot report readiness" for a descriptor epoll cannot poll, and "the reactor stopped while a watch is still armed", the watchleak rule: stopping with a watch not yet fired would otherwise strand a parked awaiter forever or drop the gauge and lie to the deadlock gate later, so the violation gets a name instead. Two narrower fatals guard the reactor's own internals, "the reactor could not wait for readiness" and "the reactor could not be signalled to stop", and watch record exhaustion reuses "out of memory".

Hardening the adversarial soundness review forced.

- Consuming or releasing a future now retires its record before the loop mutex unlocks, not after, closing a window where a completion racing the retire could write its element into a block already handed back to a new allocation. The generation bump and the completer's own gen and state checks now run under the same lock, so a completion that already passed those checks has written before the retire can free the block, and one that has not yet checked finds the bumped generation and is refused.
- The reactor keys each armed watch in a registry from fd to its current watch record, guarded by the reactor mutex, and deletes the kernel registration only when the firing watch still owns the entry. Without it, a close while armed misuse could let the fd number be reused and armed again before the stale watch's event was handled, and the stale fire's `EPOLL_CTL_DEL` would tear down the innocent successor's registration by fd number alone and hang its awaiter; an arm on a reused fd now overwrites the registry entry, so a stale fire finds it already gone and skips the delete.
- Reactor lifecycle races get the pool's discipline: a `reactor_stop` racing a concurrent one waits on a condition variable until the winner finishes draining, joining, and closing, so every caller returns holding the same stop guarantee instead of one racing ahead of the teardown. A `reactor_start` landing while a stop is still in flight is refused rather than building a fresh epoll and thread the resuming stop would then close out from under it.

Examples and goldens: seven clean, the reactor's lifecycle standing alone with no loop, the non blocking byte surface round tripping a value through a pipe end to end including EOF, a readiness watch completing a parked await with no other completer in the picture, a spawned thread waking a parked await through the reactor with the thread exit gauge dropping first, timers and a readiness watch sharing one loop with awaits returning in exact program order, four pool workers writing through four watches funnelling to one exact sum, and a writability watch completing on an already writable pipe end with mask 2; three fault, stopping the reactor with a watch still armed, a second watch colliding on one fd, and a watch armed on an invalid fd. All named `reactorlife`, `wouldblock`, `readywait`, `pipewake`, `timerinterleave`, `reactorsum`, `writewatch`, `watchleak`, `doublewatch`, and `badfdwatch`.

The release also lands a documented local ThreadSanitizer recipe, `docs/tsan.md`: rebuild a golden's emitted IR alongside the four runtime files under `clang -fsanitize=thread`, then run it in a loop. It was run against `reactorsum`, `pipewake`, and `racingcomplete` before this release, the arm and fire path, a cross thread wake racing the reactor's own gauge drop, and the racing completer path the reactor's fire step reuses.

The 0.4.x line continues with the async and await keywords in 0.4.2.

## 0.4.0

Futures and the event loop, the first phase of the async line. A one shot completion future replaces the hand rolled channel and counter of the 0.3.3 offload shape, the loop parks instead of polling, and an await that can never finish aborts by name instead of hanging. One compiler change only, the channel element ban extended to the future's minting sites; everything else is a runtime file, three standard library modules, examples, and the reference, riding the pool and monitor machinery the 0.3.x line built.

The future.

- `std.async.future` ships `Future<T>`, a one shot completion slot minted pending with `future_new`, the element type pinned by the binding annotation like `chan_new`. The handle is a plain pair of words and copies freely, every copy naming the same future, which is how a pool lambda captures it.
- `complete(f, v, e)` stores the value and the error together, from any thread, and wakes the loop. The awaiter reads exactly the pair the completer supplied, so an offloaded body hands its own failure through unchanged and there is no rejection channel anywhere, the errors as values rule extended to completion. The second completion is refused with "future already completed" and its value is dropped, whether the loser lands before or after the awaiter consumes the future, so racing completers stay visible instead of silently last writer wins and never need to outrun the awaiter. The adversarial review forced that last clause: the first cut faulted a loser that arrived after the consume, which an interleaving probe caught in one run out of eight.
- The channel element ban applies at `future_new` and `future_wrap`: an element containing a slice, closure, or interface value is rejected at compile time, since a view of the completing thread's frame would dangle in the awaiter. The one compiler change of the release, five lines in monomorphization beside the channel arm it mirrors.
- The record lives in the generational heap and consuming it retires it, so a future is awaited once the way a thread is joined once, and the second consume faults with "use of a dead future", the double join machinery on a future.
- `await` parks until completion, `await_timeout` parks at most the given milliseconds against the monotonic clock and leaves the future live on "await timed out", the recoverable escape hatch, and `try_poll` never parks, reporting "future is pending" until it consumes a ready future. `future_free` releases a future that will never be consumed.

The loop.

- The loop is a process singleton like the pool, started with `loop_init` in `std.async.loop` on the thread that consumes futures. Completion is legal from any thread; every other touch asserts the owner and faults by name off it, so the single threaded discipline is mechanical, not documentary.
- `std.async.time` ships `sleep_async`, a future the loop's timer heap completes with 0 at its deadline. Timers fire while any await or poll runs, and two timers sharing a deadline complete in creation order, the heap keyed by deadline then sequence.
- An await that provably cannot finish is a deadlock, not a hang: with no timer pending, no spawned thread alive, and no pool task in flight, nothing can complete the future, and the wait aborts with "the event loop is idle but work is still pending". The gauges drop only after their bodies finish and every drop wakes the loop, so the gate never fires against a completion still in flight.
- The fault family is named end to end: consuming a dead future, touching the loop off its owner thread, touching it before `loop_init`, and the idle deadlock. The reference gains the futures section, the completion edge in the memory model, the two honest leak notes, and the cost paragraph.

Examples and goldens: the offload flagship rewritten on futures, three awaited reads folding to the same sum with the tick loop and counter gone, a plain spawned thread as the completer, two completers racing to one exact number in every interleaving, three timers awaited out of creation order, a parse failure crossing a future intact, the refused second completion, the pending then consuming poll, the timed await against a future completed later, and five named fault goldens, the dead future, the off thread touch, the loop never started, and the idle deadlock proven both immediately and after the last completer exits.

The 0.4.x line continues with the epoll reactor in 0.4.1 and the async and await keywords in 0.4.2.

## 0.3.3

The thread pool and the async substrate, closing the 0.3.x concurrency line. The non blocking and timed channel operations land, a global worker pool runs fire and forget tasks behind a new `submit` builtin, and the flagship example rehearses the exact park, wake, and offload shape the 0.4.0 event loop lowers onto, proven in user code before the async design starts.

The operations that refuse instead of parking.

- `chan_try_send` reports "channel is full" without waiting for room, `chan_try_recv` reports "channel is empty" without waiting for a value, and both still report the closed message their blocking twins use. The runtime side never sleeps: one lock, one check, one copy.
- `chan_recv_timeout(c, ms)` parks at most ms milliseconds against the monotonic clock the condvars were built on in 0.3.1, so a wall clock step cannot stretch or shrink the wait. Its error distinguishes "receive timed out" from the closed message, and the loop rechecks the ring after every wakeup, so a spurious wake cannot fabricate a timeout while a value sits ready.
- The value beside any refusing error is the zero pattern for `T`, the drained receive's contract.

The pool and the submit builtin.

- The pool is a process singleton in the runtime, a fixed worker count over an unbounded FIFO queue, deliberately below the language: a dusk level channel of closures would copy environments that point at the sender's stack, so the sound task queue sits in C. It starts once per process and stays down after a shutdown.
- `submit` is an always available builtin sharing spawn's whole argument rule, the lambda literal, the nullary void shape, the view free capture ban, and the borrowed captured pointers, through the same checker path and the same codegen env handoff. It returns only an error: the pool owns the task and results flow through a channel.
- A submission never blocks the submitter, the contract the 0.4.0 event loop needs for its offload path. The error exists only when the pool is not running, and on that path the runtime frees the environment itself, so a refused task leaks nothing.
- `pool_start(workers)`, `pool_shutdown()`, and `ncpu()` live in the new `std.concurrent.pool`. The shutdown stops new submissions, drains everything already queued, and joins the workers, so a fold over ten thousand submitted increments prints the exact count with no wait loop in sight.
- The shutdown guarantee holds for every caller, not just one: when two threads race into `pool_shutdown`, the loser waits until the winner's drain and join complete instead of returning early with tasks still queued. A pool task calling `pool_shutdown` itself is fatal by name, since the worker would otherwise join its own thread, undefined in POSIX, or wait forever on its own completion. A start the operating system refuses unwinds to pristine, so a transient thread limit does not poison the pool for the rest of the process.

Examples and goldens: one hundred tasks on four workers folding to the arithmetic sum, the ten thousand submission stress proving the drain, both refusal windows around the pool's lifetime, a try_recv polling loop against a slow producer, the three outcomes of a timed receive, and the offload flagship, a main loop ticking on `chan_recv_timeout` while pool workers run blocking file reads and completions drain through a channel.

The 0.3.x concurrency line is complete: threads and atomics in 0.3.0, channels in 0.3.1, mutexes and condition variables in 0.3.2, and the pool substrate here. Next is the 0.4.x async line on top of it.

## 0.3.2

Mutexes and condition variables, the third phase of the concurrency line. Shared mutable state gains its sanctioned shape, a raw buffer guarded by a lock, with every classic pthread misuse turned into a named fault. No compiler changes at all: the whole release is runtime shims, a standard library module, examples, and the reference.

The primitives.

- `std.concurrent.sync` ships `Mutex` and `Condvar` as one word handles over runtime shims, the channel's pattern. `lock` blocks until the mutex is free, `unlock` releases it, `cond_wait` releases the mutex around its sleep and reacquires it before returning, `cond_signal` wakes one waiter, `cond_broadcast` wakes all.
- The mutex is the error checking pthread kind, so relocking a mutex the thread already holds and unlocking one it does not hold, both undefined in the default flavor, fault by name. The runtime adds what the kind cannot: a trylock probe makes freeing a held mutex fatal, and the fault paths branch on the actual error code, so operating on a mutex already freed reports an invalid mutex instead of blaming a holder that does not exist.
- The condvar record carries a waiter count beside the pthread object, raised before the wait releases the mutex, so freeing a condition variable a thread waits on faults by name. The bare destroy would hang forever on glibc, quiescing for a waiter no signal will ever release, the worst failure shape in the toolbox.
- Condition variables run on a CLOCK_MONOTONIC clock like the channel's, ready for the timed waits arriving in 0.3.3. Wakeups can be spurious, and the reference states the rule: a wait always sits in a loop that rechecks its predicate under the lock, and every concurrent wait on one condvar names the same mutex.
- The blessed idiom inside a function body is `lock(m)` then `defer unlock(m)`, so every return path releases, verified working end to end.

The memory model.

- The mutex edge joins the sanctioned list: an `unlock` happens before the next `lock` of the same mutex. Hand built `*raw` sharing stays on the honor system unless a mutex guards every touch, and the reference now says exactly that.
- Blocking waits still have no timeout until 0.3.3, so the reference names the deadlock hazard plainly.

Examples and goldens: four threads driving one counter to exactly ten thousand under a mutex, a two account transfer loop whose invariant holds to the digit, a bounded buffer hand built from one mutex and two condition variables proving the primitives express what channels provide natively, a condition variable ping pong whose six lines alternate deterministically, and three named fault goldens, freeing a held mutex, unlocking an unheld one, and freeing a condition variable with a parked waiter, the last made deterministic by the waiter count rising before the wait releases the mutex.

Planned next in the line: the timed and non blocking channel operations, the thread pool, and the async substrate rehearsal in 0.3.3.

## 0.3.1

Channels, the second phase of the concurrency line. A bounded, thread safe queue moves values and ownership between threads, built as a standard library generic over a textbook monitor in the runtime, with no new syntax. One new rule guards the crossing: a channel element must be free of frame views, the same ban spawn captures enforce.

The channel.

- `std.concurrent.channel` ships `Channel<T>` with free functions in the `Vector` pattern. `chan_new(cap)` builds a bounded queue whose element type the binding annotation pins, the sizing rule `alloc` already uses, so a bare `jobs := chan_new(8)` is a compile error. `chan_send` blocks while the channel is full, `chan_recv` blocks while it is empty, `chan_close` idempotently wakes every blocked party and discards nothing buffered, and `chan_free` releases the monitor.
- `chan_send`'s error exists when the channel is closed. `chan_recv`'s error exists once the channel is closed and drained, so a loop breaking on `e.exists()` consumes everything senders delivered. The value beside that error is the zero pattern for `T` and means nothing.
- The runtime monitor is one mutex, two condition variables on a CLOCK_MONOTONIC clock so the timed receive planned for 0.3.3 cannot be confused by a wall clock step, a ring buffer, a closed flag, and a count of blocked waiters. Construction aborts on a capacity below one or exhaustion, the allocator's contract, since a channel that cannot exist has no error path a fresh program could act on.
- Freeing a channel while a thread is blocked inside a send or receive is fatal with a named message, caught best effort under the monitor lock. The sanctioned shutdown order is close, then join, then free, and the language reference documents it.

Ownership and types.

- Ownership crosses threads by moving a managed pointer through a channel: `chan_send(c, move(p))` kills the sender's name through the ordinary argument position move, and the receiver binds a fresh owner through the ordinary call returns ownership rule. The compile fail twin proves the sender cannot touch the record again.
- A channel element containing a slice, a closure, or an interface value, wherever it sits, including buried in struct or enum fields, is a compile error at the instantiation, since each may view the sending frame and the ring would deliver a dangling view to another thread. The walk is the mono side twin of the spawn capture check.
- A `*raw T` now passes anywhere `*void` is expected, the direction the channel's element staging needs. Codegen always lowered `*void` to the same bare word, so the gap was the checker's alone. The reverse direction stays rejected, because a `*void` that could become a typed `*raw T` would let a managed pointer launder through `*void` into a dereferenceable alias the generation check cannot see.
- Dereferencing a null managed pointer now faults by name instead of dying by raw signal: the untracked generation zero path tests for null and calls the new null fault, which flushes stdout first like every fault. The drained receive's zero pattern for a pointer element is exactly this null, so the natural consumer mistake gets a named message.
- A moved send refused by a closed channel loses its record, and managed pointers still buffered at `chan_free` leak as raw bytes. Both are documented in the reference and the module, neither is corruption, and neither happens in the sanctioned close, join, free order where senders finish first.
- The memory model gains the channel edge: a `chan_recv` happens after the `chan_send` that delivered its value.
- The foreign function section now states that a symbol resolves against anything the binary links, libc and the dusk runtime today, the loosening the concurrent modules already rely on.

Examples and goldens: a three stage pipeline folding to one sum, a four worker fan in, the receive until closed idiom, one hundred sends through a capacity one channel to force the blocking path, the ownership handoff plus its compile fail twin, the rejected slice element buried in a struct field, and the named null fault on a drained receive's placeholder.

Planned next in the line: mutex and condvar in 0.3.2, the timed and non blocking channel operations and the thread pool in 0.3.3.

## 0.3.0

Threads, the first phase of the concurrency line. OS threads with zero new syntax, a thread safe runtime underneath them, and atomics so parallel programs can prove themselves deterministically.

The thread primitive.

- `spawn(f: () -> void) -> (thread, error)` starts an OS pthread running a lambda literal, and `join(t: thread) -> error` waits for it. Both are always available builtins, paradigm agnostic like `alloc` and the error machinery, and both failures ride the must handle rule.
- `thread` is an opaque builtin type. The handle is a record in the generational heap and `join` retires it, so a double `join` faults through the same generation check a use after free hits.
- The spawned lambda's captured environment is copied into a private heap block the runtime frees after the body returns, so a thread never reads another thread's stack. A nullary void lambda already compiles to the pthread start shape, so the trampoline is direct.
- `spawn` requires the lambda literal at the call site, since only the literal knows the environment layout. Spawning a closure variable is a compile error naming the wrap in a literal fix.

Safety across threads.

- Captures cross by immutable copy, the rule lambdas always had. Capturing a slice, a closure, or an interface value is rejected wherever it sits, including buried in a struct or enum field, since each may view the spawning frame.
- A captured managed pointer is a borrow inside the thread: reading through it works, freeing or moving it there is a compile error, and a moved away pointer keeps its moved state, so capturing it stays the error a plain lambda gets. An owner freeing while a thread still holds the borrow is caught by the generation check at the thread's next dereference.
- The generational heap is thread safe: one mutex guards the free list and the debug tables, the generation word is bumped and read atomically on both sides of the check, and the dereference check stays armed on every thread.
- join's generation check and the handle's retirement run in one heap critical section, so a double join faults deterministically even when two threads race on copies of the same handle. The spawn environment allocation aborts on exhaustion rather than writing captures through a null.
- The language reference gains a Threads and the Memory Model section that states the data race stance honestly: races are undefined, sanctioned paths (spawn copies, atomics, join, and the coming channels and mutexes) provide the ordering they name, and the generation check degrades from a guarantee to a best effort backstop under a true race.

Standard library and tooling.

- `std.concurrent.atomic` ships `AtomicInt` with sequentially consistent `atomic_load`, `atomic_store`, `atomic_add`, and `atomic_cas` over a heap word, the sanctioned shared counter before mutexes land.
- `std.concurrent.thread` ships `sleep_ms`.
- The runtime grows `runtime/thread.c` with the spawn trampoline, join, sleep, and the atomic shims, and the driver links it with `-pthread` for older toolchains.
- Examples and goldens cover spawn and join ordering, a two thread atomic counter, per iteration capture copies, a deterministic cross thread use after free fault, a double join fault, and the compile time rejections.

Planned next in the line: channels in 0.3.1, mutex and condvar in 0.3.2, the timed channel operations and the thread pool the async release builds on in 0.3.3.

## 0.2.6

Hardens the whole 0.2.x line one level deeper. Where 0.2.5 closed exact reproductions, this release closes each rule's family, the range as well as the index, the binding site as well as the call site, the bare call as well as the qualified one, and lands the rules deferred along the way.

Memory and bounds.

- `alloc()` with no value sizes the block from the declared pointer annotation, so `p: *Big = alloc()` allocates all of `Big` instead of an 8 byte default that corrupted the heap. The unannotated form `x := alloc()` is a compile error.
- Returning an array literal where a slice is expected is caught by the escape check, closing a dangling stack slice the range form already rejected.
- A range slice validates `lo <= hi <= base.len` and traps on a miss, so a slice can no longer fabricate a length that launders out of bounds reads past the index check.
- `FixedBuffer` and `Arena` check capacity and honor alignment, aborting on exhaustion instead of handing out memory past the buffer. `vec_get` is bounds checked. `parse_int_radix` rejects overflow instead of wrapping.

Types.

- Integer and float widths are tracked in the checker. `int32 + int64` is a compile error instead of a silent truncation, in arithmetic, comparison, assignment, argument passing, and returns.
- A bare literal adapts to the width beside it, an unannotated literal binding hardens to the default width, and a literal that cannot fit its annotated or suffixed width is rejected.
- Immutability covers projections. `xs[i] = v` and `s.f = v` need a `mut` root binding, while a store through a pointer dereference or a slice stays governed by the pointee.
- A field store on an undereferenced pointer, which previously compiled and did nothing, is an error that names the `(*p).field` fix.

Semantic analysis.

- Binding or returning a struct where an interface is expected requires the impl, the same conformance rule call sites gained in 0.2.5, so a missing impl is a checker error instead of an undefined vtable in clang.
- `match` requires an enum scrutinee. A non enum previously executed every arm in sequence.
- A `defer` inside a conditional or loop is rejected, since registration is lexical and every return replays the list, which cannot honor a conditional registration.
- The binding level must handle rule lands. A bound error must reach `exists`, `check`, or `ignore`, or be returned to the caller, and printing it does not count.
- A non void function must return on every path, where falling off the end silently produced a zeroed value.
- `main`'s signature is validated. The allocator form is rejected until its entry wrapper exists, so it cannot read garbage registers, and any other unsupported shape is named.

Monomorphization.

- A destructured binding takes its tuple element's type, so valid generic code over a destructure no longer dies with a type mismatch in clang.
- A type parameter no call site pins down fails `dusk check` at the source line instead of silently defaulting to `int64` and reinterpreting values.
- An impl block on a generic type is diagnosed instead of silently dropped, and a duplicate `impl I for T` is a checker error instead of a duplicate symbol at link.
- Builtin results such as `read_file`'s `(string, error)` pair type their bindings for generic inference.

Modules.

- The loader renames each imported module's private top level items with a per file suffix before the merge and rewrites the module's own references to match. A bare call can no longer reach another file's private helper, and two modules may each keep a private `helper` without colliding. Exported names and foreign functions never change.

Printing.

- A struct prints through its `Display` impl's `toString`, and a struct without one is a compile error. Printing an enum, a slice, a tuple, or a pointer is an error instead of silence.
- `printerr` lands as a stderr println, flushing stdout first so program output precedes the message even when the program aborts right after.
- A string literal first argument is a format string at any arity, so `println("{}")` with no value is an error and `println("{{}}")` prints `{}` consistently.
- `toString()` on the empty error is the empty string, and the runtime printers guard a null pointer, closing a crash in `puts`.
- A bare `println()` prints a newline.

Tooling and internals.

- `dusk run` forwards trailing arguments to the program, so an argc and argv main sees them.
- The `monad` block is gated to the functional paradigm, matching `do` notation.
- Diagnostics for foreign blocks, impl completeness, and whole function errors carry real source spans instead of pointing at the file's first character.
- Identical string literals intern to one IR global, nominal type lookups in codegen go through a map instead of scanning item lists, and clippy is clean across all targets.
- The language reference documents every rule above. The suite grows to 195 unit and 49 golden tests.

## 0.2.5

Closes the gaps a specification review found, where a construct parsed and partly checked but leaned on permissive typing or late runtime behavior.

- `free` of a managed pointer runs the generation check, so freeing a stale pointer to a reused block faults at the free instead of corrupting the live owner.
- `for` loops lower to codegen over an array or a slice, where they were silently dropped.
- A bare statement that drops a fallible call's error is rejected, the first enforcement of the must handle rule.
- Reassigning an owning pointer is rejected as a leak, while a borrowing cursor still advances.
- Array and slice indexing is bounds checked and traps out of range, negatives included.
- `main(argc, argv)` gains a C ABI entry wrapper that builds the string slice, so `argv.len` matches argc.
- Passing a struct where an interface is expected requires an impl with every method, and an incomplete impl is rejected, both in the checker.
- Struct literals validate field names, duplicates, and completeness.
- A qualified module call to a private name is rejected.
- The language reference's string and error representations match the implementation.

## 0.2.4

The minimal foreign function interface, riding the raw pointer layer.

- A `foreign "C"` block declares external functions with dusk types and no body. Each binds to a C symbol of the same name at link, and a call type checks and lowers like any other.
- The boundary is the raw pointer layer only. Parameters and returns are scalars, `*raw T`, or `*void`. A managed `*T`, an aggregate by value, and an abi other than `"C"` are rejected.
- libc is the reachable library, since the toolchain already links it.

## 0.2.3

Escaping value lifetimes, the last memory safety phase.

- Returning a slice that views a frame local fixed array is a compile error, since the array is reclaimed with the frame. A heap backed slice or a slice parameter still returns fine.
- Returning a closure that captures a frame local is a compile error, while a capture free closure is a plain function pointer and may be returned.
- Pointer escapes were already covered at runtime by the generation check, since every pointer is heap allocated.

## 0.2.2

Single owner pointers, the static half of the ownership story.

- The checker tracks each managed pointer binding as an owner or a borrow. A plain copy of an owner is rejected and points at `ref` to alias or `move` to transfer.
- `move(x)` transfers ownership and invalidates the source, so a later use is rejected.
- A `ref` binding is a non owning alias, and a pointer parameter borrows. Freeing or moving a borrow is rejected, since only the owner does either.
- The raw layer, `*void` and `*raw T`, is exempt, and the runtime generation check backstops what the single block static pass cannot see.

## 0.2.1

Generational references, the runtime foundation of the memory safety line.

- A managed `*T` is a fat pointer, the data pointer paired with a remembered generation. The default heap writes a live generation in a header before each block, and `free` bumps it and parks the block on a size matched free list.
- Every managed dereference compares the remembered generation against the header and faults on a use after free, a double free, or a stale pointer to a reused block, in every build.
- The thin layer lands alongside. `*raw T` and `*void` are one word pointers with no generation, carrying strings, slice data, receivers, and collection buffers, with `ptr_add` for byte arithmetic.
- A generation of zero is the untracked sentinel, so a `using` allocator hands back unchecked memory and custom allocators keep working.

## 0.2.0

Mutable strings and concatenation.

- `std.string` ships `StringBuilder`, a growable heap buffer that keeps a NUL after the last character so a string view costs nothing.
- `concat` joins two strings into a fresh builder the caller owns, and the `cstr` builtin reinterprets a NUL terminated `*char` as a `string` at no cost.

## 0.1.5

Formatted printing.

- `print` writes with no newline and `println` appends one, where both previously appended it.
- `print` and `println` take a format string whose `{}` holes the value arguments fill in order, with `{{` and `}}` for a literal brace. The literal expands at compile time into typed prints, no runtime parser, no allocation, and a mismatched hole count is a compile error.

## 0.1.4

Console input and parsing.

- `read_line` reads one line and `read_all` the whole stream, each a `(string, error)` pair.
- `parse_int` and `parse_int_radix` parse signed integers, base 2 to 36, and `parse_float` parses a float through the runtime, each returning a value with an error.
- `read_int` and `read_float` in `std.io` compose the readers with the parsers.

## 0.1.3

Completeness of the planned core.

- Qualified call syntax. `std.io.print_line(x)` folds to the merged global, while method calls and enum constructors keep their meaning.
- `std.map`, a string keyed `Map<V>` with open addressing, doubling, and a `Maybe<V>` get, written in dusk.
- File I/O. `read_file` returns a `(string, error)` pair and `write_file` an `error`, both global builtins.

## 0.1.2

Methods and allocators.

- Every method takes its receiver by pointer, so a method mutates the receiver in place and a stateful allocator's bump offset persists across calls.
- The `using` `Allocator` interface works end to end, with `Heap`, `FixedBuffer`, `Arena`, and `Debug` in the stdlib, dispatched statically on a concrete type and through the vtable when erased.

## 0.1.1

Correctness and diagnostics.

- `char` is the unsigned range and zero extends, errors as values lower end to end with `exists`, `toString`, `check`, and `ignore`, and `reduce` returns `(T, error)` and guards the empty slice.
- Per file source tracking renders a merged diagnostic against the file its span falls in, an import's leaf segment is validated against the module's exports, and several monads coexist through `monad Name` blocks.

## 0.1.0

The core language, end to end. Paradigm directives gating procedural, functional, and oop features per file, structs, methods, enums with exhaustive `match`, interfaces with vtables, closures, monomorphized generics, functional builtins, `do` notation, errors as values under the must handle rule, explicit memory with `alloc`, `free`, and `defer`, a module system with a stdlib seed, and a golden test suite compiling and running every example.
