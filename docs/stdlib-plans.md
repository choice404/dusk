# Standard Library Plans

What the standard library grows into after 0.1.0. These modules are not in the tree yet. This is the shape the library is headed toward, not a schedule.

## std.map

A hash map, generic over key and value. It rounds out the core collections next to `std.vector` and is one of the pieces the compiler needs to host itself.

## std.functional.result

`Result<T, E>`, a success value or a typed error. It pairs with the `error` builtin for code that wants a typed failure channel instead of the `(T, error)` tuple.

## std.functional.io

`IO<T>`, a monad that wraps side effecting work so it composes with do notation.

## std.logging

Structured logging with levels and output redirection, built on `std.io`.

## std.memory.collector

A garbage collector exposed as an allocator, the `collector<T>` wrapper. It ships first as a conservative collector. A precise collector comes much later.

## Strings

Mutable strings, concatenation, and Unicode aware operations. 0.1.0 strings are immutable, so these land in a later release.

## More monads

`List<T>` and a wider set of helpers across `Maybe`, `Either`, and `Result`, so do notation reaches more shapes.
