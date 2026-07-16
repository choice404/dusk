# Embedding dusk in a C program

## About

This walks you through compiling a dusk module into a C library and calling it from another language. You write a dusk function, mark it `export "C"`, build it with `dusk build --lib`, and link the resulting archive against a C host or load it from anything with a C FFI. By the end you will have called dusk from C and from Python with the same archive.

## Contents

- [Write an exported function](#write-an-exported-function)
- [Build the library](#build-the-library)
- [Call it from C](#call-it-from-c)
- [Loading from other languages](#loading-from-other-languages)
- [What may cross](#what-may-cross)
- [Library mode rules](#library-mode-rules)

## Write an exported function

An `export "C"` function is a dusk function a C caller reaches by its own bare symbol. Write one in a file with no `main`:

```text
// mathlib.dusk
@paradigm procedural

export "C" func mathlib_add(a: int64, b: int64) -> int64 {
    return a + b
}

export "C" func mathlib_scale(x: float64, k: float64) -> float64 {
    return x * k
}
```

The `"C"` after `export` names the calling convention, and `"C"` is the only one supported. A plain `export` without it only makes a name visible to other dusk files; the `"C"` marks it a C ABI entry point besides.

Type check it the same way you check any dusk file:

```sh
dusk check mathlib.dusk
```

## Build the library

Build the module into an archive and a header with `--lib`:

```sh
DUSK_HOME=$PWD dusk build --lib mathlib.dusk
```

You get two files in `target/dusk-out`:

```sh
ls target/dusk-out/libmathlib.a target/dusk-out/mathlib.h
```

`libmathlib.a` is a static archive bundling your module and the whole dusk runtime, so it is self contained: a host links it and needs nothing else of dusk's. `mathlib.h` is a generated C header with one prototype per export. Read it:

```sh
cat target/dusk-out/mathlib.h
```

You will see stdint types inside an `extern "C"` guard, so a C or a C++ host includes it the same way:

```c
int64_t mathlib_add(int64_t a0, int64_t a1);
double mathlib_scale(double a0, double a1);
```

## Call it from C

Write a host that includes the header and calls the exports:

```c
// host.c
#include <stdio.h>
#include "mathlib.h"

int main(void) {
    printf("add=%lld\n", (long long)mathlib_add(40, 2));
    printf("scale=%.2f\n", mathlib_scale(1.5, 3.0));
    return 0;
}
```

Compile it against the archive. Point `clang` at `target/dusk-out` for both the header and the library, and add the runtime link line the header names:

```sh
clang host.c -I target/dusk-out -L target/dusk-out -lmathlib -pthread -lm -o host
./host
```

You get:

```text
add=42
scale=4.50
```

## Loading from other languages

The archive reaches any toolchain that statically links a C library into an executable. Zig links it with `zig build-exe host.zig target/dusk-out/libmathlib.a -lc`, Rust links it from a `build.rs` that prints `cargo:rustc-link-lib=static=mathlib` and `cargo:rustc-link-search=target/dusk-out`, and a C++ host includes the same header, since it is guarded with `extern "C"`. In each case you name a symbol whose C type follows the header: an `int64` is a 64 bit integer, a `float64` a `double`, a `char` an unsigned byte, a `rune` a 32 bit integer, and a `*raw T` or a `*void` a `void*`.

A loader that opens a shared object at run time, Python's `ctypes` and Ruby's `Fiddle` among them, needs a position independent `.so` rather than a static `.a`. `dusk build --lib` emits a static archive only in this release: the archive's objects are not position independent, and the runtime's thread local storage uses the local exec model a shared object cannot take, so the archive does not link straight into a `.so`. A shared object build is planned for a later release. Until it lands, reach dusk from a dlopen based FFI by linking the archive into a small C extension that the loader imports, the same way a C extension module wraps any static library.

## What may cross

An exported function's parameters and return must be C legal, the same set the foreign boundary already means: a scalar (an integer of any width, `float32`, `float64`, `bool`, `char`, or `rune`), a `*raw T`, or a `*void`, and the return may also be `void`. A string, a slice, a managed `*T`, a closure, an interface value, a collected value, a future, and an error never cross. A struct by value on an export is not supported yet; pass a pointer to it, or write a small `@csource` C adapter for the shape.

Three names are refused, each with its own diagnostic: an export may not be generic, may not be named `main`, and may not begin with `cool_`, which the runtime reserves. The one collision the compiler cannot catch is a symbol your host already links under the same name, so give your exports a library prefix, `mathlib_` above.

## Library mode rules

A library has no dusk `main`, and that has two consequences worth stating plainly.

The collected heap needs the anchor a dusk `main` records, and a library never records it, so an exported function that mints a `collector<T>` or forces a collection aborts by name with `fatal: the collector runs on the main thread only`. Keep your exports over plain values and raw pointers.

The event loop and the async surface run on whichever host thread calls an exported function that drives them, one such thread at a time. A host that calls exports from several threads at once is sharing data across threads under the same rules dusk's own threads follow; the generational heap stays safe there, the collector does not.
