# dusk

Dusk is a small systems language that compiles to native code through textual LLVM IR. Every file picks a paradigm with `@paradigm procedural`, `functional`, or `oop`, and that choice unlocks the matching builtins. Values are immutable by default, memory is explicit, and errors are values you handle. The compiler is written in Rust with zero dependencies and links each program against a small C runtime.

Dawn is an accompanying package tool. A Dusk package is a git repository, inspired by the Go style of importing libraries and modules.

## Requirements

- Rust stable and Cargo.
- clang and LLVM 22.x on your path. The textual IR targets one LLVM major version.

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

The `dusk` binary has seven commands. They are `lex`, `scan`, `parse`, `check`, `build`, `run`, and `version`.

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

0.2.4. The compiler runs the whole pipeline. It lexes, parses, resolves names, type checks, monomorphizes, and emits code, backed by a golden and unit test suite. The standard library and the multi module sample both build and run.

The 0.2.x line adds memory safety. Strings have a growable `StringBuilder` with concatenation, the pointer layer splits into a managed `*T` and a raw `*raw T`, and the default heap is generational. Every managed pointer carries a generation that is checked at each dereference, so a use after free, a double free, or a stale pointer to a reused block faults instead of corrupting memory. A managed pointer is single owner, with `ref` for a non owning alias and `move` to transfer, and a return that lets a frame local escape is a compile error for the clear cases. A `foreign "C"` block then calls into libc across the raw pointer boundary, the first slice of the interop work.

## License

Dual licensed under MIT or Apache 2.0. Pick whichever one fits your use. The full text lives in LICENSE-MIT and LICENSE-APACHE.
