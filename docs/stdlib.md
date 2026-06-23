# Standard Library

The dusk standard library lives under `lib/std` and is written in dusk. Import a module with a dotted path, then call its exported names.

```text
@import std.io
@import std.functional.maybe
```

Imported names are flat for now. After `@import std.io` you call `print_int` and `print_line` with no prefix. Enum constructors keep their type name, so you write `Maybe.Some(42)` and `Maybe.None`.

## std.io

Console output over the `println` builtin.

| Function                        | Description                     |
| ------------------------------- | ------------------------------- |
| `print_int(n: int64) -> void`   | Print an integer and a newline. |
| `print_line(s: string) -> void` | Print a string and a newline.   |

```text
@import std.io

print_int(42)
print_line("hello")
```

## std.string

Read only helpers over NUL terminated strings.

| Function                               | Description                                 |
| -------------------------------------- | ------------------------------------------- |
| `str_len(s: string) -> int64`          | Length up to the NUL terminator.            |
| `str_eq(a: string, b: string) -> bool` | True when both strings hold the same bytes. |

```text
@import std.string

n := str_len("hello")        // 5
same := str_eq("a", "a")     // true
```

## std.vector

A growable array, generic over its element type. The buffer lives on the heap and doubles when it fills. Pass the vector by pointer so growth persists across calls.

| Function                                   | Description                            |
| ------------------------------------------ | -------------------------------------- |
| `vec_new<T>() -> Vector<T>`                | A new empty vector.                    |
| `vec_push<T>(v: *Vector<T>, x: T) -> void` | Append one element, growing if needed. |
| `vec_get<T>(v: *Vector<T>, i: int64) -> T` | The element at index `i`.              |
| `vec_len<T>(v: *Vector<T>) -> int64`       | The element count.                     |
| `vec_free<T>(v: *Vector<T>) -> void`       | Free the backing buffer.               |

```text
@import std.vector

v: *Vector<int64> = alloc(vec_new())
mut i: int64 = 0
while i < 5 {
    vec_push(v, i * 10)
    i = i + 1
}
println(vec_len(v))      // 5
println(vec_get(v, 2))   // 20
vec_free(v)
free(v)
```

Capacity starts at 4 on the first push and doubles from there.

## std.memory.arena

A bump allocator over one backing buffer. Each allocation carves forward from the buffer. Individual frees do nothing, and you reset or destroy the whole arena at once. Pass the arena by pointer so the offset persists.

| Function                                       | Description                                       |
| ---------------------------------------------- | ------------------------------------------------- |
| `arena_new(cap: int64) -> Arena`               | An arena backed by a `cap` byte buffer.           |
| `arena_alloc(a: *Arena, size: int64) -> *void` | Carve `size` bytes and return the pointer.        |
| `arena_reset(a: *Arena) -> void`               | Roll the offset back to zero, keeping the buffer. |
| `arena_destroy(a: *Arena) -> void`             | Free the backing buffer.                          |

```text
@import std.memory.arena

a: *Arena = alloc(arena_new(1024))
p: *int64 = arena_alloc(a, 8)
*p = 7
arena_destroy(a)
free(a)
```

## std.functional.maybe

An optional value. It is `Some` with a payload or `None`.

```text
enum Maybe<T> {
    Some(v: T),
    None,
}
```

| Function                                      | Description                             |
| --------------------------------------------- | --------------------------------------- |
| `is_some<T>(m: Maybe<T>) -> bool`             | True when the value is `Some`.          |
| `unwrap_or<T>(m: Maybe<T>, fallback: T) -> T` | The payload, or `fallback` when `None`. |

```text
@import std.functional.maybe

m: Maybe<int64> = Maybe.Some(42)
println(unwrap_or(m, 0))        // 42

none: Maybe<int64> = Maybe.None
println(unwrap_or(none, 99))    // 99
```

## std.functional.either

A value of one of two types. `Left` is the error or first case by convention, `Right` is the success or second case.

```text
enum Either<L, R> {
    Left(l: L),
    Right(r: R),
}
```

| Function                                           | Description                                     |
| -------------------------------------------------- | ----------------------------------------------- |
| `is_left<L, R>(e: Either<L, R>) -> bool`           | True when the value is `Left`.                  |
| `left_or<L, R>(e: Either<L, R>, fallback: L) -> L` | The `Left` payload, or `fallback` when `Right`. |

```text
@import std.functional.either

e: Either<int64, int64> = Either.Left(-5)
println(left_or(e, 0))   // -5
```
