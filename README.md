# dusk

Dusk is a small systems language that compiles to native code through textual LLVM IR. Every file picks a paradigm with `@paradigm procedural`, `functional`, or `oop`, and that choice unlocks the matching builtins. Values are immutable by default, memory is explicit, and errors are values you handle. The compiler is written in Rust with zero dependencies and links each program against a small C runtime.

Dawn is an accompanying package tool. A Dusk package is a git repository, inspired by the Go style of importing libraries and modules.

## Requirements

- Rust stable and Cargo.
- clang and LLVM 22.x on your path. The textual IR targets one LLVM major version.

## Install

The language is pre 1.0 and every minor release changes it. Installing today means tracking a moving target, which the development packages make explicit.

```sh
# from crates.io
cargo install dusk-lang

# on Arch Linux, from the AUR, builds the latest main
paru -S dusk-lang-git
```

Both install the `dusk` and `dawn` binaries. The compiler finds its standard library and C runtime beside itself, in the share directory for a packaged install or inside the cargo registry checkout for `cargo install`, and the `DUSK_HOME` environment variable overrides the search when you want a binary to use a different toolchain tree, such as a source checkout.

## Try it without building a binary

`cargo run` compiles the toolchain on first use and runs it in one step, so you never manage a binary yourself. Pick the binary with `--bin`, then pass arguments after `--`.

```sh
# compile and run an example program
cargo run --bin dusk -- run examples/app.dusk

# type check only, or print the version
cargo run --bin dusk -- check examples/m9.dusk
cargo run --bin dusk -- version

# run the test suite, unit tests plus golden program tests
cargo test
```

## Build the binaries

For standalone `dusk` and `dawn` binaries, build once and call them directly. They land in `target/release`.

```sh
cargo build --release
./target/release/dusk run examples/app.dusk
./target/release/dusk version
```

The `dusk` binary has eight commands. They are `lex`, `scan`, `parse`, `check`, `build`, `run`, `demo`, and `version`. `dusk run` forwards any trailing arguments to the program, so an argc and argv main sees them, and `demo` builds and runs a hardcoded IR spine as a toolchain smoke test.

## A taste of the language

```text
@paradigm functional

func main() -> int32 {
    nums: int64[] = [1, 2, 3, 4, 5]
    doubled := map(nums, lambda (n: int64) -> int64 { return n * 2 })
    foreach(doubled, lambda (n: int64) -> void { println(n) })
    return 0
}
```

Browse `examples/` for runnable programs. `examples/app.dusk` is a multi module sample.

## Language

- Primitive ints and floats, `bool`, `char`, `string`, slices `T[]`, and fixed arrays `T[N]`.
- `struct`, `enum` with payloads and exhaustive `match`, and `interface` with vtable dispatch.
- Monomorphized generics `<T>`.
- Lambdas and closures that capture outer variables by immutable copy.
- Functional builtins map, filter, reduce, fold, and foreach, plus monadic `do` notation.
- Manual memory with `alloc`, `free`, `defer`, pointers, and the raw primitives `sizeof`, `alloc_bytes`, and `ptr_add`.
- Immutability by default with `mut`, errors as values, and per file paradigm gating.

The standard library under `lib/std` is written in dusk. It ships `io`, `string`, `memory.arena`, `functional.maybe`, `functional.either`, and a generic amortized dynamic array `vector`.

## Packages with dawn

An import is a stdlib or local dotted path, or a quoted git path.

```text
@import std.io
@import "github.com/user/repo/module"
```

The first three segments of a git path, `host/user/repo`, name the repository. The rest names a module inside it. dawn clones each repository into a cache, either `$DAWN_CACHE` or `~/.dawn/cache`, and the dusk loader resolves git imports from there. dawn shells out to the system `git`, so git has to be on your path to fetch.

```sh
# without building a binary
cargo run --bin dawn -- get examples/app.dusk    # clone the git packages a file imports
cargo run --bin dawn -- run examples/app.dusk    # fetch, then compile and run

# or with the built binary
./target/release/dawn run examples/app.dusk
```

The `dawn` binary has four commands. They are `get`, `build`, `run`, and `version`. Currently an import resolves against the latest clone in the cache. Version pinning, a lock file, and fetching across a dependency graph come in a later release.

## Status

See [CHANGELOG.md](CHANGELOG.md) for the release by release history.

0.7.0. The compiler runs the whole pipeline. It lexes, parses, resolves names, type checks, monomorphizes, and emits code, backed by a golden and unit test suite. The standard library and the multi module sample both build and run. The language surface is frozen for the bootstrap as of 0.5.4: the releases from 0.6.x through 0.9.x rewrite the compiler in dusk without changing the language, so a program that compiles today keeps compiling across that line, and new surface resumes only after 1.0.0.

Releases 0.2.0 through 0.2.6 add memory safety. Strings have a growable `StringBuilder` with concatenation, the pointer layer splits into a managed `*T` and a raw `*raw T`, and the default heap is generational. Every managed pointer carries a generation that is checked at each dereference, so a use after free, a double free, or a stale pointer to a reused block faults instead of corrupting memory. A managed pointer is single owner, with `ref` for a non owning alias and `move` to transfer, and a return that lets a frame local escape is a compile error for the clear cases. A `foreign "C"` block then calls into libc across the raw pointer boundary, the first slice of the interop work.

The checker holds the line the spec draws. Integer and float widths never mix silently, immutability covers element and field stores, every array index and range slice is bounds checked, an allocation is sized by its declared type, a bound error must be handled, printing dispatches through `Display` or fails to compile, and a private name never leaves its file.

Release 0.3.0 adds threads, the first phase of concurrency. `spawn` starts an OS thread running a lambda whose captures copy into a private heap environment, `join` waits and retires the handle so a double join faults like a use after free, the generational heap is thread safe with the dereference check armed on every thread, and `std.concurrent.atomic` carries the sequentially consistent counter.

Release 0.3.1 adds channels. `std.concurrent.channel` carries a bounded, thread safe queue: `chan_send` blocks while the channel is full, `chan_recv` blocks while it is empty and errors once the channel is closed and drained, and `chan_send(c, move(p))` hands ownership of a heap record to the receiving thread with the sender's name dead at compile time.

Release 0.3.2 adds mutexes and condition variables. `std.concurrent.sync` guards shared memory with `lock`, `unlock`, and the `defer unlock(m)` idiom, `cond_wait` sleeps until a signal with the predicate rechecked in a loop, and every classic pthread misuse, relocking, unlocking without holding, freeing a held mutex, faults by name.

Release 0.3.3 completes the concurrency line with the thread pool and the async substrate. The `submit` builtin queues fire and forget tasks on a global worker pool without ever blocking the submitter, `chan_try_send`, `chan_try_recv`, and `chan_recv_timeout` refuse or time out instead of parking forever, and the offload example rehearses the park, wake, and offload loop the 0.4.x async releases build on.

Release 0.4.0 opens the async line with futures and the event loop. `std.async.future` carries a one shot `Future<T>` completed exactly once from any thread and consumed exactly once on the loop thread, `await` parks instead of polling with `await_timeout` and `try_poll` as the refusing forms, `sleep_async` turns timers into futures, and an await nothing can complete aborts by name instead of hanging. A consumed future retires in the generational heap, so awaiting it twice faults like a double join.

Release 0.4.1 adds the reactor, the second phase of the async line. `std.async.io` runs one C thread that turns file descriptor readiness into a one shot `Future<int64>` on the event loop, so `readable` and `writable` watches complete alongside timers and pool tasks with no polling loop anywhere. A non blocking byte surface over pipes, `read_nb`, `write_nb`, and friends, gives the watches something deterministic to test against, an armed watch raises a third gauge into the deadlock detector, and a watch left armed when the reactor stops faults by name rather than stranding a parked awaiter.

Release 0.4.2 is the largest of the 0.4.x line, two tracks landing together. The complete operator set arrives: bitwise `& | ^ ~ << >>`, the full compound assignment family `+= -= *= /= %= &= |= ^= <<= >>=`, postfix `++` and `--`, a right associative exponent `**`, the pipe `|>`, and an inclusive range `..=`, over a documented thirteen level precedence ladder. Alongside it, `async func`, `await`, and `async_run` land the keyword layer the async line has been building toward. An async func compiles to a state machine over a heap allocated task frame; `await` suspends inside one in exactly four statement positions, `x := await f`, `x, e := await f`, a void discarding `await f`, and `return await f`; and `async_run(f(args))` is the only bridge a synchronous `main` uses to crank the loop, one enqueue and one scheduler turn per await, run to completion with no cancellation, and a named fatal for every way it can go wrong, a double await, two tasks on one future, or an idle loop with work still pending. Underneath both tracks, the escape check that keeps a frame local slice or closure from being returned is completed to see through every carrier, tuples, structs, enums, fixed arrays, and generic fields, at any nesting depth, and interface values boxed inside a struct field, an enum payload, or an array element now dispatch correctly.

```text
async func fetch(n: int64) -> int64 {
    return n * 2
}

func main() -> int32 {
    le := loop_init()
    le.ignore()
    rc := async_run(fetch(21))
    loop_free()
    println(rc)   // 42
    return 0
}
```

Release 0.4.3 rounds out the async line with networking, an awaitable channel, and the generic monad. `std.async.net` puts TCP over the reactor's readiness futures: `tcp_listen`, `tcp_accept`, `tcp_connect`, `tcp_read`, and `tcp_write` are async functions that await a socket the same way a task awaits any future, so a server accept loop and a client both compose under `async_run` with no polling. `chan_recv_async` makes a blocking channel receive awaitable on the loop, a detached helper completing the future off thread so the loop never stalls. `do` notation now composes over any generic monad, `Maybe`, `Either`, or a user `monad` block, monomorphizing a fresh `bind` and `unit` pair per `do` site, and `std.functional.io` ships an `IO<T>` that rides it. The same change hardened an old soundness seam: a width mismatch inside a generic `do` continuation, once silently truncated, is now caught by a second type check over the fully concrete program.

Release 0.4.4 is the second platform and the hardening pass. The reactor's poller splits behind a six function seam, `create`, `destroy`, `arm`, `disarm`, `wait`, and `wake`, over a normalized readiness mask, so a second backend sits beside the first. `reactor_epoll.c` is the Linux path, the existing epoll and eventfd code lifted verbatim so every reactor golden and the seam under ThreadSanitizer stay unchanged, and `reactor_kqueue.c` is a kqueue backend for the BSDs and macOS, written and statically reviewed against the same seam but not yet run on a BSD or macOS host, with a bring-up runbook in [docs/kqueue.md](docs/kqueue.md) for the person who runs it. The syscall surface hardens: `SIGPIPE` is ignored process wide so a write to a closed peer returns a `broken pipe` error instead of killing the process, every blocking call retries on `EINTR`, and file descriptor exhaustion surfaces as a handled `too many open files` error that leaks nothing and leaves the reactor usable. Four async stress goldens, 2000 timers, 1000 tasks, 100 connections, and 10000 pool sends, pin the runtime under load, the pool saturation and accept storm among them ThreadSanitizer clean.

Release 0.5.0 is the ledger, closing debt recorded across the 0.4.x line with no new language surface. Escape analysis becomes interprocedural: a summary computed for every function and lambda tracks which parameters a return value may alias, expose through a pointer, store into another parameter's place, or hand to a channel send, so a frame view laundered out through a call, a store, a send, a closure, or a pointer alias is now caught, not only one returned directly. The alias model became scope stacked and binding driven too, so a view does not have to keep its own name to keep the checker's attention. Alongside it, an interface bound as a generic type argument is rejected instead of hanging the compiler, the parser terminates on malformed or deeply nested input with a diagnostic instead of a hang or a stack overflow, a future minted by a direct async call can be passed as an argument, annotated, and stored in a container, a mutable tuple's fat member survives a reassignment, and a bare function value called in return position lowers correctly.

Release 0.5.1 activates the `collector<T>` syntax the spec reserved from 0.1.0: a second, conservative mark and sweep heap sits beside the generational one, opted into per value through `collector<T>(e)` and never ambient. A plain, a closure, and a slice kind cover a scalar or managed pointer, a lambda whose environment moves to the collected heap, and a deep copied slice source. Minting is escape neutral, a collected value returns cleanly, but the mint itself is an outliving sink that rejects a frame view the same way a `return` already does, and a `collector<F>` closure carries its own capture rule requiring every capture to be immortal safe. A collected value is never freed, moved, or borrowed with `ref`, since the collector reclaims it, and it stays confined to the single thread it anchors to: a channel, a spawned or submitted capture, and an interface box are all rejected at compile time, while a `Future<collector<T>>`, an async func returning one, and a same thread `Vector<collector<T>>` are allowed, since the loop and the collector share one thread and the vector's own buffer is a scanned root. `collector` is a contextual reserved word, so a variable named `collector` compared against something, `collector < n`, still parses as an identifier. `std.memory.collector` adds `gc_collect`, `gc_live_blocks`, `gc_live_bytes`, and `gc_collections` as control and gauges over the collected heap; a `Collector` type implementing `Allocator` was drafted and withheld, since the allocator interface's untyped pointer would erase the tracking the checker needs to keep a collected reference on its own thread.

Release 0.5.2 adds Unicode strings. A new `rune` primitive holds a single 4 byte Unicode scalar value, distinct from the 1 byte `char`, with a `r'...'` literal and the same widen and truncate rule against `int` that `char` already follows, though a `rune` and a `char` never mix with each other directly. `\u{...}` names a scalar by its hex codepoint inside a string or rune literal, and a string literal that is not valid UTF-8 is now a compile time rejection rather than a silent replacement. `std.unicode`, written entirely in dusk, decodes and encodes UTF-8 over the string's existing NUL terminated byte view with `decode_rune`, `encode_rune`, `rune_len`, `rune_count`, `utf8_valid`, and `sb_push_rune`, every one total and never reading past what a malformed sequence resyncs from. Alongside the surface work, a codegen fix funnels every sync mode stack slot into the function's entry block instead of allocating fresh at each loop iteration, so a decode loop over an unbounded input no longer overflows the stack.

Release 0.5.3 is the stdlib. `std.functional.io`'s `IO<T>` is rebuilt as a true lazy monad over a collected thunk: `bind` and `unit` now build a suspended chain instead of running anything, so a `do IO { ... }` block performs no effect until `run` forces it on the calling thread, no event loop or thread pool required, replacing the eager form's pool offload. `std.functional.result` ships `Result<T, E>`, an `Ok` or `Err` enum with a `do Result { ... }` block fixed to string errors and a `result_from` bridge off the `(value, error)` pair a fallible call returns, and `Maybe` and `Either` each round out with `map`, `and_then`, and `or_else` style helpers. `std.logging` adds level gated logging to stderr, `Debug` through `Error`, ranked and gated behind one process wide atomic threshold. Underneath the stdlib work, two soundness gaps close: an ambiguous generic enum constructor, `Opt.None` at a struct field, a call argument, an assignment, or an array element, now instantiates from its surrounding expected type or names the ones it still can't instead of defaulting silently and dying inside `clang`, and an error handed to a parameter declared `error` now discharges the caller's must handle obligation while binding that same obligation onto the callee's own parameter, closing a launder where the hand-off ended with no one accountable.

Release 0.5.4 is polish and the freeze, finishing up 0.5 with an audit of the spec against the compiler and no new language. Diagnostics now render three lines, the header, the source line, and a caret run measured in Unicode columns; `e.message` reads through the same null guarded path `toString` uses instead of a silent zero; and the last slice covariance case that could panic the compiler, a concrete element erased before the sema check could see it, is downgraded to a named build error so a missed sink can never miscompile. An audit hardening batch tightens ten checks the spec claimed but the compiler had left open, among them a constructor payload literal ranged against its field width, a rejected string index assignment, a rejected whole fallible tuple bind, the `@paradigm oop` gate on `interface` and `impl`, the `bind` and `unit` requirement on a `monad` block, a rejected method call on an enum value, and a compile time reject for an async body that pumps the loop's blocking `await`, `await_timeout`, or `try_poll` by hand. The unsigned integer names and the `u` suffixes are reserved out of the surface until after 1.0.0. The spec declares the bootstrap freeze, and the changelog carries a permanent ledger folding every 0.5.0 through 0.5.3 deferral into one accounted list.

Release 0.6.0 opens the bootstrap line the freeze made room for: the language holds still and the compiler starts being rewritten in itself. `compiler/` is new, a tree of dusk source that is dusk1, the stage1 compiler's front end scaffold, built and run today by stage0 the same way any other dusk program is. It carries a command line front end, a diagnostic renderer over a located source buffer, the driver that finds the toolchain and shells out to `clang`, and a full lexer, every literal form, the complete escape set, and span tracking, all `@paradigm procedural` with no collector, async, interface, or lambda in it yet. A parity gate, `tools/differential.sh`, holds it to the bar the rest of the bootstrap will run on: `dusk1`'s `lex` and `scan` dumps match stage0's byte for byte, and every lexer reject lands at the same file and line, across all 579 `.dusk` files in `examples/` and `lib/std`. The dump formats behind that gate are now a documented interchange contract, `docs/dumps.md`, and `tools/pyramid.sh` climbs the stage ladder itself, building each stage from the one under it toward a fixpoint. Building the scaffold surfaced real bugs along the way, in the stdlib growing to support it, `std.os` and new `std.string` and `std.map` helpers among them, and in stage0's own pre scan, all fixed and recorded in the changelog rather than folded quietly into the feature list.

Release 0.6.1 writes `else if` into the spec. The parser has always read an `else` followed by an `if` as one nested conditional, so a chain takes any number of arms with no new syntax; the release pins that behavior with a golden and a compile fail twin, checks each chained condition as a bool, and stops a runaway chain at the same expression depth ceiling every other nested form respects.

Release 0.7.0 gives dusk1 a parser, a loader, and a desugar pass, so the front end that used to stop at a token dump now carries the whole surface grammar through to a merged, monad expanded module. The parser reads the AST into a set of parse order arenas, a fixed slot record per expression, statement, type, and pattern kind, with every variable length list living in one shared slice backed vector and every name interned once, and it holds the same 500 deep nesting guard and the same async and collector position checks stage0's own parser does. The loader ports the full import search, beside the file, then the stdlib root or `DUSK_HOME`, then the dawn cache for a git path, with the same private name renaming and qualified call folding, and desugar expands `do` notation into `bind` and `unit` calls the same way stage0's does. `dusk1 parse`, `load`, and `desugar` now match stage0's dumps byte for byte, `parse` over the full corpus and `load` and `desugar` over all but the one file whose difference is a paradigm gate that lands with the sema port still to come, and a 63 file fixture tree pins the edge cases the corpus alone doesn't reach. No language surface changes here; this is the bootstrap line doing what the freeze promised.

## License

Dual licensed under MIT or Apache 2.0. Pick whichever one fits your use. The full text lives in LICENSE-MIT and LICENSE-APACHE.
