# dusk Language Specification

## Status, 0.1.0 baseline with 0.2.x additions

This is the language reference for dusk. The sections below describe the 0.1.0 core. The 0.2.x line layers memory safety on top, so where this document says strings are immutable, pointers are a single kind, or memory safety is debug only, the current language differs. It has a growable `StringBuilder` with concatenation, a split between a managed `*T` and a raw `*raw T` or `*void`, and a default generational heap that checks every managed dereference, faulting on a use after free or a double free.

---

## Table of Contents

1. [Core Philosophy](#core-philosophy)
2. [Source Files, Directives, Imports, Exports](#source-files-directives-imports-exports)
3. [Paradigm System](#paradigm-system)
4. [Type System](#type-system)
5. [Memory Management](#memory-management)
6. [Functions](#functions)
7. [Object Oriented Concepts](#object-oriented-concepts)
8. [Functional Concepts](#functional-concepts)
9. [Error Handling](#error-handling)
10. [Builtins](#builtins)

---

## Core Philosophy

- Immutability by default. All values are immutable unless explicitly declared mutable. (I don't like mutability in languages ¯\\_(ツ)_/¯)
- Explicit over implicit. Allocations, dereferences, paradigm usage, and error handling are never hidden.
- Multiple paradigms with enforced discipline. Paradigms are opt in per file through directives. Undeclared paradigm features are compile errors in that file.
- Systems level control. Manual memory management by default. No garbage collector unless explicitly opted into through the standard library.
- All declared variables must be used. An unused variable is a compile error. This is never suppressible.
- All errors must be handled. Ignoring an error return is a compile error.

---

## Source Files, Directives, Imports, Exports

A source file has two kinds of top of file syntax. Directives start with `@` and configure the file. Declarations define types, functions, and values, and can carry modifier keywords like `export` and `mut`.

### Directives

Directives appear at the top of the file, before declarations.

```text
@paradigm functional
@paradigm procedural

@import std.io
@import std.functional.maybe
```

0.1.0 has two directives.

- `@paradigm <name>` declares a paradigm the file uses. It can be repeated to stack paradigms. See [Paradigm System](#paradigm-system).
- `@import <path>` brings a module or a symbol into the file. See below.

### Imports

Imports are based on directories and files. A dotted path walks the project tree.

```text
/
  myLib/
    myFile.dusk
  main.dusk
```

In `main.dusk`.

```text
@import myLib.myFile.someFunc   // import a leaf symbol
```

A dotted path resolves to one of two things.

- A module, a directory or a file. You then call through the qualified name.
- A leaf symbol, a function, type, or value inside a file. The leaf name is then in scope unqualified.

```text
@import std                 // module: call std.io.println(...)
@import std.io              // module: call std.io.println(...)
@import std.io.println      // symbol: call println(...)
```

Resolution walks directories, then files, then symbols, so the compiler can tell where the file path ends and the symbol name begins.

Imports are independent of paradigm directives. Importing a module does not grant any paradigm. The two systems do not interact.

> The source file extension is `.dusk`.

### Exports

By default every declaration is private to its file. The `export` keyword makes a declaration visible to other files.

```text
export struct Point { x: float64, y: float64 }

export func area(s: Shape) -> float64 { ... }
```

Only exported names can be imported elsewhere. There is no paradigm restriction on exports. An exported function or type is usable from any file regardless of either file's paradigm directives. This keeps the cross file story simple and matches the rule that user defined names are paradigm agnostic.

A private name never crosses a file boundary, neither as a qualified call nor as a bare one, and two imported modules may each keep a private helper of the same name without colliding.

---

## Paradigm System

### Overview

Each source file declares which paradigms it uses with `@paradigm` directives. A file with no directive defaults to procedural. Directives unlock the builtins, syntax, and keywords associated with each paradigm. They do not affect which functions from other files can be called. Only builtins and syntax are gated within the current file.

### What Each Paradigm Unlocks

| Directive              | Unlocks                                                                                     |
| ---------------------- | ------------------------------------------------------------------------------------------- |
| `@paradigm functional` | map, filter, reduce, fold, foreach, do notation, `monad` keyword, pure function enforcement |
| `@paradigm procedural` | for, while, do while, `mut` variables                                                       |
| `@paradigm oop`        | interface, composition syntax                                                               |

Directives stack. The set of available builtins and syntax is the union of all declared paradigms.

```text
@paradigm functional
@paradigm procedural
```

This file can use functional builtins like map, filter, and reduce, and procedural constructs like for, while, and mutable state.

### Default Behavior

If no `@paradigm` directive is present, the file defaults to procedural.

### Cross File Rules

- Functions and types defined in any file are paradigm agnostic. They can be called or used from any other file regardless of paradigm directives.
- Gating is per file and covers builtins and syntax. A file without `@paradigm functional` cannot call `map` directly, but it can call a user defined function that internally uses `map`.
- The check is intra file and runs during semantic analysis. There is no link time paradigm check, since calls through user defined functions are never gated.

---

## Type System

### Primitive Types

| Type    | Size    | Description                        |
| ------- | ------- | ---------------------------------- |
| int8    | 1 byte  | signed 8 bit integer               |
| int16   | 2 bytes | signed 16 bit integer              |
| int32   | 4 bytes | signed 32 bit integer              |
| int64   | 8 bytes | signed 64 bit integer              |
| uint8   | 1 byte  | unsigned 8 bit integer             |
| uint16  | 2 bytes | unsigned 16 bit integer            |
| uint32  | 4 bytes | unsigned 32 bit integer            |
| uint64  | 8 bytes | unsigned 64 bit integer            |
| float32 | 4 bytes | 32 bit floating point              |
| float64 | 8 bytes | 64 bit floating point              |
| bool    | 1 byte  | true or false                      |
| char    | 1 byte  | single ASCII character             |
| string  | fat ptr | built in string type (see Strings) |
| error   | builtin | built in error type (see Errors)   |

### Type Inference

Compile time inference uses the `:=` operator. The compiler infers the type from the right hand side. There is no runtime type resolution.

```text
x := 5          // inferred as int64 (default integer type)
y := 3.14       // inferred as float64 (default float type)
z := true       // inferred as bool
```

Explicit type annotation is always available.

```text
x: int32 = 5
```

Inference uses these defaults.

- Integer literals become `int64`.
- Float literals become `float64`.
- For other types such as `uint8` or `float32`, use a literal suffix or an annotation.

Numeric widths never mix silently. Arithmetic, comparison, assignment, and argument passing take operands of one width, so an `int32` next to an `int64` is a compile error rather than a truncation. A bare literal adapts to the width beside it, and a literal that cannot fit its annotated width is rejected.

Literal suffixes select a non default type without an annotation.

```text
a := 5u8        // uint8
b := 3.14f32    // float32
c := 200u64     // uint64
```

### Strings

A string is a pointer to a NUL terminated buffer of `char`, a read only view that costs one machine word. String literals do not heap allocate, since the literal bytes live in static storage.

```text
s: string = "hello"   // a pointer to the NUL terminated bytes
```

- A string value is immutable. The growable `StringBuilder` in `std.string`, added in 0.2.0, builds and concatenates strings on the heap.
- A string's length is found by scanning to the NUL, which `std.string`'s `str_len` does. The NUL keeps a string view compatible with C and the foreign interface.
- The `cstr` builtin reinterprets a NUL terminated `*char` buffer as a string at no runtime cost.

Unicode handling is deferred past the 0.2.x line.

### Arrays and Slices

Two aggregate forms hold a sequence of a single element type `T`.

- Fixed array `T[N]`. `N` elements stored inline. The size is known at compile time. Stack allocated like any value, passed by value as a copy.
- Slice `T[]`. A fat pointer `{ ptr: *T, len: int64 }` that views a contiguous run of elements without owning them. Same shape as `string`, which is effectively `char[]`.

```text
xs: int32[4] = [1, 2, 3, 4]   // fixed array, 16 bytes inline
s:  int32[]  = xs[1..3]       // slice viewing xs[1], xs[2], length 2
argv: string[]                // slice of strings, as passed to main
```

- Slice length is always known. No scanning, no null terminator.
- Every array and slice index is bounds checked and traps when it misses, negatives included.
- A range slice validates `lo <= hi <= len` against its base, so a slice can never claim a length past its backing.
- A dynamic array is provided in the standard library as `std.vector`, a heap backed generic type.

### Immutability and Mutability

All variables are immutable by default. Mutability is declared with `mut`.

```text
x: int32 = 5       // immutable, cannot be reassigned
mut y: int32 = 5   // mutable, can be reassigned
```

Function scope restriction on mutability.

A mutable variable is only mutable within the function it was declared in. Nested function definitions and closures can read it but cannot mutate it.

Immutability covers projections. An element or field store, `xs[i] = v` or `p.x = v`, needs its root binding declared `mut`, the same as the bare `xs = v` form. A store through a pointer dereference or through a slice writes the buffer the binding views, not the binding, so it is governed by the pointee's rules instead.

```text
func outer() -> void {
    mut x: int32 = 5
    x = 10             // allowed, same function

    func inner() -> void {
        x = 15         // COMPILE ERROR, x not mutable in this scope
        y := x + 1     // allowed, reading x is fine
    }
}
```

Scope here means the declaring function body. Ordinary blocks in the same function, such as loop bodies and `if` branches, can mutate the variable. Only nested function definitions and closures lose mutation rights. So `mut x = 0` followed by a `for` loop that runs `x = x + 1` is allowed, while mutating `x` from inside a nested `inner()` is not. This forces explicit data passing into inner scopes and prevents hidden state mutation through closures.

### Pointers

Pointers are immutable. Once a pointer is assigned it cannot be reassigned to a different address. Pointers exist only as the result of an explicit heap allocation through `alloc`. There is no address of operator for stack variables. Stack variables are passed by value.

```text
p: *int64 = alloc(100)   // p points to a heap int64 initialized to 100
```

After `free(p)`, the binding `p` is consumed. Using it again is a compile error where statically determinable, and a trapping poison value in debug builds.

### Foreign Functions

Added in 0.2.4. A `foreign` block declares functions that live in an external C library, so dusk code can call into libc and other C code. The functions have no body. Each binds to a C symbol of the same name at link.

```text
foreign "C" {
    func abs(n: int32) -> int32
    func write(fd: int32, buf: *raw int8, count: int64) -> int64
}
```

The boundary is the raw pointer layer only. A parameter or return type is a scalar, a `*raw T`, or a `*void`. A managed `*T` is rejected, since it is a fat value carrying a generation that C cannot read, so a buffer crosses as `*raw T` and an opaque pointer as `*void`. Once declared, a foreign function is called like any other function.

- Only the `"C"` calling convention is supported.
- A struct passed by value across the boundary, a variadic foreign function, and a library other than libc are deferred to a later interop release.

### Sum Types (Enums)

Tagged unions are a first class, paradigm agnostic data type, like structs. A value is exactly one of several named variants, each optionally carrying payload data. Sum types back the standard library monads (Maybe, Either, Result) and pattern matching, and writing the compiler itself for a future bootstrap needs them.

```text
enum Shape {
    Circle(radius: float64),
    Rect(w: float64, h: float64),
    Empty,
}
```

Values are inspected with `match`, which must be exhaustive.

```text
func area(s: Shape) -> float64 {
    match s {
        Circle(r)  => return 3.14159 * r * r,
        Rect(w, h) => return w * h,
        Empty      => return 0.0,
    }
}
```

- Variants can be empty (`Empty`) or carry named fields.
- `match` is exhaustive. A missing variant is a compile error.
- `match` is defined over enum values only. A scalar or struct scrutinee is a compile error.
- Generic sum types are written `Maybe<T>`. They are monomorphized at compile time.
- Layout is a tag (the smallest integer that fits the variant count) plus storage for the largest variant's payload.

---

## Memory Management

### Philosophy

Manual memory management is the default. There is no garbage collector built into the language. A garbage collector is available later through the standard library as an allocator strategy.

### Stack Allocation

Normal variable declaration results in stack allocation. No explicit action is needed.

```text
x: int32 = 5    // stack allocated
```

### The Allocator Interface

An allocator is any type that implements the built in `Allocator` interface.

```text
interface Allocator {
    alloc(size: int64, align: int64) -> *void
    free(p: *void) -> void
}
```

The standard library ships four allocators that implement this interface. A heap allocator, the default, backed by libc. An arena allocator that frees everything at once. A fixed buffer allocator with no heap, for embedded or scratch use. A debug allocator that reports leaks and catches a double free. Users can write their own allocator by implementing the interface.

### `alloc` and `free` Are Sugar Over the In Scope Allocator

`alloc` and `free` are builtins, but they are not a fixed implementation. They lower to a call on the allocator that is in scope. Choosing the allocator type chooses the implementation that `alloc` resolves to. The default is the heap allocator.

A function that allocates must have an allocator in scope. You mark a parameter with `using` to designate it as the ambient allocator for that function body. Call sites stay clean. You write `alloc(...)`, not `allocator.alloc(...)`.

```text
func work(using allocator: Allocator) -> void {
    p: *Point = alloc(Point { x: 1.0, y: 2.0 })   // uses the passed allocator
    defer free(p)
}
```

This keeps allocation explicit at the boundary, since the signature shows the function needs an allocator, while keeping the body readable. Users never redefine `alloc`. They implement the `Allocator` interface and pass it in. No other builtin or function is overridable, and there is no function overloading.

`free` must run under the allocator that produced the pointer. A `using` scope routes `free` to the scope's allocator, so freeing a default heap block inside one hands it to the wrong allocator, the same caller matches rule C allocators follow.

Dispatch is static when the allocator's concrete type is known at that point, which is the common case and is zero cost. It falls back to a vtable call only when the allocator type is erased behind the interface.

The allocation size is inferred from the declared type on the left hand side. The programmer does not pass a byte size, which prevents size and type mismatch bugs.

```text
x: *int64 = alloc(100)     // 8 bytes, initialized to 100
y: *char  = alloc('c')     // 1 byte, initialized to 'c'
z: *int64 = alloc()        // 8 bytes, uninitialized
```

The uninitialized form requires the pointer annotation, since the annotation is what sizes the block. A bare `x := alloc()` is a compile error.

### Dereferencing

Heap allocated values are dereferenced explicitly with the `*` prefix operator. Implicit dereferencing is not allowed.

```text
x: *int64 = alloc(100)
y: int64 = 10 + *x         // dereference x to get 100
```

### Scope Cleanup with `defer`

Use `defer` to run cleanup when the enclosing function scope exits, in reverse order of registration, including on an early return.

```text
p: *int64 = alloc(100)
defer free(p)              // runs at scope exit, even on early return
y: int64 = *p + 1
```

`defer` makes deallocation deterministic and visible without any ownership tracking.

A `defer` sits at the top level of its function. Registration is lexical and every return replays the list, so a `defer` inside a conditional or a loop cannot be honored and is a compile error. Dynamic registration is planned.

### Arena Allocation

An arena frees all of its allocations at once. Per object `free` is a no op. Arenas are the ergonomic answer to threading an allocator through code, and they fit a compiler's allocation pattern well.

```text
@import std.memory.arena

func build(using a: Arena) -> Tree {
    // every alloc here comes from the arena
    // nothing needs individual free; the arena is reset or dropped as a whole
}
```

### Debug Allocator

In debug builds the standard allocator tracks live allocations and detects three faults.

- Leaks. Heap not freed by program or scope end.
- Double free. Freeing an already freed pointer.
- Use after free. Dereferencing a freed pointer. Freed memory is overwritten with a trapping poison value.

These are debug build diagnostics, not language guarantees. Release builds omit the tracking for speed.

### Safety

0.1.0 does no ownership tracking, so freeing is manual. `defer` and arenas keep cleanup deterministic, and the debug allocator catches mistakes in tests. Generational references for sound use after free and double free detection arrive in 0.2.0. A generation token rides inside each reference and is checked at dereference, so it survives copies.

### Garbage Collector

Garbage collection is deferred past 0.1.0. The `collector<T>` wrapper syntax is reserved. It ships first as a conservative collector, with a precise collector much later.

### The `main` Function

`main` is a special function with a flexible signature. All parameters are optional.

```text
func main() -> int32 { ... }
func main(argc: int32, argv: string[]) -> int32 { ... }
func main(argc: int32, argv: string[], using allocator: Allocator) -> int32 { ... }
```

`main` returns an `int32` exit code. `0` means success. If `main` declares a `using allocator` parameter, the program runs with that allocator as the ambient allocator. With no allocator parameter the default heap allocator is used.

The allocator form is planned. The compiler rejects it until the entry wrapper that constructs the ambient allocator lands, so a program never reads a garbage register where the allocator should be.

---

## Functions

### Declaration Syntax

Functions are declared with the `func` keyword.

```text
func name(param: Type, ...) -> ReturnType {
    // body
}
```

### Pass By Value, Always

All function parameters are passed by value. There are no reference types.

```text
func foo(x: int64) -> void {
    // x is a copy
}
```

When a pointer is passed, the pointer itself (the address value) is copied, not the heap data it points to. The callee can dereference the copy to read the heap value. The original allocation is still owned by the caller.

```text
func foo(p: *int64) -> void {
    y: int64 = *p + 1   // reads the heap value through the copied pointer
}

func main() -> int32 {
    x: *int64 = alloc(100)
    defer free(x)
    foo(x)              // passes a copy of the pointer, caller still owns the allocation
    return 0
}
```

For large heap allocated data, the caller passes a pointer to avoid copying.

### No Overloading

Two functions cannot share a name with different signatures. Generic functions are a different feature and are allowed.

```text
func id<T>(x: T) -> T { return x }   // one generic function, monomorphized per use
```

### Anonymous Functions (Lambdas)

A lambda is an anonymous function declared with the `lambda` keyword.

```text
double := lambda (n: int64) -> int64 { return n * 2 }
```

Lambdas are first class values and are the argument form for functional builtins.

```text
doubled := map(nums, lambda (n: int64) -> int64 { return n * 2 })
```

Capture rule. A lambda can read variables from outer scopes, captured by immutable copy. It cannot mutate them. This is the same rule that applies to nested function definitions. The copy is taken when the lambda is created. There is no capture by reference, which matches the absence of an address of operator and pass by value everywhere.

```text
factor := 3
triple := lambda (n: int64) -> int64 { return n * factor }   // reads factor by copy
```

---

## Object Oriented Concepts

Available when `@paradigm oop` is declared.

### Interfaces

The only OOP construct is the interface. There are no classes and no inheritance.

```text
interface DisplayName {
    getName() -> string;
    setName(name: string) -> void;
}
```

A few rules govern interfaces.

- An interface defines a contract of method signatures.
- A struct satisfies an interface by an explicit `impl` declaration.
- There is no inheritance. One interface cannot extend another.
- Composition is the only way to combine behaviors.

```text
impl DisplayName for Person {
    func getName() -> string { return self.name }
    func setName(name: string) -> void { self.name = name }
}
```

### Structs

Structs are plain data containers available across all paradigms, not gated by `@paradigm oop`. Methods can be associated with structs through `impl`. Structs use interfaces for polymorphism.

```text
struct Point {
    x: float64,
    y: float64,
}
```

### No Inheritance

There is no inheritance of any kind, not for structs and not for interfaces. Code reuse happens through composition only.

---

## Functional Concepts

Available when `@paradigm functional` is declared.

### Core Builtins

| Function | Description                                        |
| -------- | -------------------------------------------------- |
| map      | applies a function to each element of a collection |
| filter   | filters a collection by a predicate                |
| reduce   | reduces a collection to a single value             |
| fold     | fold left or right over a collection               |
| foreach  | iterates over a collection for side effects        |

These take lambdas, which capture outer variables by immutable copy (see Lambdas).

### Monads

The `monad` keyword declares a special interface type that enforces monadic structure. The compiler verifies that the required operations are present. Do notation is available when `@paradigm functional` is declared. The `monad` keyword belongs to the functional paradigm.

A monad must implement a unit operation that wraps a value and a `bind` operation that chains computations.

```text
monad Maybe<T> {
    some(value: T) -> Maybe<T>;
    none() -> Maybe<T>;
    bind(f: (T) -> Maybe<U>) -> Maybe<U>;
    unwrap() -> (T, error);
}
```

The standard library ships these monads through import.

| Monad        | Description                       |
| ------------ | --------------------------------- |
| Maybe<T>     | an optional value                 |
| Either<L, R> | one of two possible types         |

Do notation currently desugars against a monad whose `bind` has concrete types. A `bind` generic over the element type is not yet monomorphized through `do`, so ground monads work and fully generic ones wait on a later release.
| Result<T, E> | success or a typed failure        |
| IO<T>        | wraps side effecting computations |
| List<T>      | the list monad                    |

This program unwraps a `Maybe` and prints the value.

```text
@paradigm functional

@import std.functional.maybe
@import std.io

func main() -> int32 {
    m: Maybe<int32> = maybe.some(54)
    result, e := m.unwrap()
    e.check(lambda (err: error) -> void { std.io.printerr(err) })
    std.io.println(result)
    return 0
}
```

Do notation requires `@paradigm functional`.

```text
result: Maybe<int32> = do {
    x <- maybe_divide(10, 2)
    y <- maybe_divide(x, 0)
    z <- maybe_add(y, 1)
    return z
}
```

---

## Error Handling

### The `error` Builtin Type

`error` is a built in type. It is not imported from any library.

`error` carries a human readable message. It is a pointer to the NUL terminated message text, and the empty, non error value is a null pointer.

- `message: string`. A human readable description, read with `toString`.

A numeric code and a source location are not part of the current representation. They may return in a later release.

It has four methods.

- `exists() -> bool`. True if this is a real error, not an empty error.
- `toString() -> string`. Formats the error as a string.
- `check(handler: (error) -> void) -> void`. If the error exists, it calls `handler` with the error. If the error does not exist, it does nothing.
- `ignore() -> void`. Explicitly acknowledges and discards the error.

### Fallible Functions

Any function that can fail returns a tuple of `(T, error)`. There is no exception system and no panic. Errors are always values.

```text
func pop_back() -> (int32, error)
```

### Handling Errors

Errors are values, so ordinary code handles them. Two shapes are common.

First, control flow that propagates an error upward. A lambda cannot return from its caller, so this shape uses `exists`.

```text
y, e := x.pop_back()
if e.exists() {
    std.io.printerr(e)
    return 1
}
```

Second, side effecting handling that logs and continues, using `check`.

```text
y, e := x.pop_back()
e.check(lambda (err: error) -> void {
    std.io.printerr(err)
})
```

### Every Error Must Be Handled

The tuple return is destructured at the call site. Both values must be bound to named variables. The error binding must be used. Using an error means one of three things.

- inspecting it with `exists()` (usually followed by control flow),
- handling it with `check(...)`,
- or explicitly discarding it with `ignore()`.

```text
y, e := x.pop_back()
e.ignore()   // explicit, visible, greppable suppression
```

Unlike Go, there is no `_` suppression. `ignore()` replaces it. The difference is that `ignore()` is a visible, searchable acknowledgement in the source, while `_` hides the decision. An unhandled error binding is a compile error.

---

## Imports and Standard Library

See [Source Files](#source-files-directives-imports-exports) for import syntax. Imports are separate from paradigm directives. Importing a module does not grant any paradigm.

### Standard Library, Planned Modules

| Module                | Description                                           |
| --------------------- | ----------------------------------------------------- |
| std.io                | print, println, printerr, file I/O                    |
| std.logging           | structured logging with levels and output redirection |
| std.memory.arena      | arena allocator                                       |
| std.memory.collector  | garbage collector allocator wrapper (later)           |
| std.functional.maybe  | Maybe<T> monad                                        |
| std.functional.either | Either<L, R> monad                                    |
| std.functional.result | Result<T, E> monad                                    |
| std.functional.io     | IO<T> monad                                           |
| std.vector            | dynamic array                                         |
| std.map               | hash map                                              |
| std.string            | string manipulation utilities                         |

---

## Builtins

Builtins are always available regardless of paradigm directives unless noted.

### Always Available

| Builtin  | Signature             | Description                                  |
| -------- | --------------------- | -------------------------------------------- |
| alloc    | alloc(value?) -> \*T  | heap allocate through the in scope allocator |
| free     | free(p: \*T) -> void  | deallocate through the in scope allocator    |
| print    | print(...) -> void    | print to stdout, handles all primitive types |
| println  | println(...) -> void  | print to stdout with a newline               |
| printerr | printerr(...) -> void | println to stderr                            |
| sizeof   | sizeof(T) -> int64    | size of a type in bytes at compile time      |

`alloc` and `free` resolve to the in scope allocator. See [Memory Management](#memory-management).

### Functional Builtins (require `@paradigm functional`)

| Builtin | Description                        |
| ------- | ---------------------------------- |
| map     | applies a function to each element |
| filter  | filters a collection by predicate  |
| reduce  | reduces a collection to one value  |
| fold    | fold left or right                 |
| foreach | iterates for side effects          |

### Procedural Builtins (require `@paradigm procedural`)

| Builtin or Keyword | Description                 |
| ------------------ | --------------------------- |
| for                | for loop                    |
| while              | while loop                  |
| do while           | do while loop               |
| mut                | declares a mutable variable |

### Display Interface

Any type that implements the `Display` interface can be passed to `print` and `println`.

```text
interface Display {
    toString() -> string;
}
```

Passing a struct with no `Display` impl to a print builtin is a compile error, as is printing an enum, a slice, a tuple, or a pointer. Print never emits silence for a value it cannot render.
