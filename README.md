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

0.4.3. The compiler runs the whole pipeline. It lexes, parses, resolves names, type checks, monomorphizes, and emits code, backed by a golden and unit test suite. The standard library and the multi module sample both build and run.

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

## License

Dual licensed under MIT or Apache 2.0. Pick whichever one fits your use. The full text lives in LICENSE-MIT and LICENSE-APACHE.
