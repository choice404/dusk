# Standard Library

The dusk standard library lives under `lib/std` and is written in dusk. Import a module with a dotted path, then call its exported names.

```text
@import std.io
@import std.functional.maybe
```

Imported names are flat. After `@import std.io` you call `print_int` and `print_line` with no prefix. You can also qualify a call through its module path, so `std.io.print_line("hi")` reaches the same function. Enum constructors keep their type name, so you write `Maybe.Some(42)` and `Maybe.None`.

## std.io

Console output over the `println` builtin, plus typed line input that reads a line and parses it.

| Function                            | Description                                  |
| ----------------------------------- | -------------------------------------------- |
| `print_int(n: int64) -> void`       | Print an integer and a newline.              |
| `print_line(s: string) -> void`     | Print a string and a newline.                |
| `read_int() -> (int64, error)`      | Read one line and parse it as a base 10 int. |
| `read_float() -> (float64, error)`  | Read one line and parse it as a float.       |

```text
@import std.io

print_int(42)
print_line("hello")

n, e := read_int()
if e.exists() {
    return 1
}
print_int(n * 2)
```

`read_int` and `read_float` read one line through the `read_line` builtin and parse it, so the error exists at end of input or when the line is not a number.

### File I/O

`read_file` and `write_file` are builtins, so they are available everywhere without an import, the same way `print` is. `read_file` returns a `(string, error)` pair and `write_file` returns an `error`, so the must handle rule applies and a caller resolves the failure through `exists`, `check`, or `ignore`.

| Builtin                                               | Description                              |
| ----------------------------------------------------- | ---------------------------------------- |
| `read_file(path: string) -> (string, error)`          | Read the whole file into a heap string.  |
| `write_file(path: string, contents: string) -> error` | Write the string to the file, truncating it. |

```text
werr := write_file("/tmp/note.txt", "persisted")
werr.ignore()
s, rerr := read_file("/tmp/note.txt")
rerr.ignore()
print_line(s)
```

A failed read hands back the empty string and an error that exists. The string `read_file` returns lives on the heap, so free it with `free` once you are done with it.

### Console input

`read_line` and `read_all` are builtins that read from stdin. `read_line` reads one line, `read_all` reads the whole stream. Each returns a `(string, error)` pair. For `read_line` the error exists at end of input, so a read loop stops when it fires, while `read_all` errors only on an allocation failure, since the whole of an empty stream is the empty string. A line keeps no trailing newline, and an empty line is the empty string, which is distinct from end of input. Both read from the terminal when stdin is not redirected, and from a pipe or a file when it is.

| Builtin                          | Description                                        |
| -------------------------------- | -------------------------------------------------- |
| `read_line() -> (string, error)` | Read one line from stdin, the error marking end of input. |
| `read_all() -> (string, error)`  | Read all of stdin into one string.                 |

```text
print_line("what is your name?")
name, err := read_line()
if err.exists() {
    return 0
}
print_line(name)
```

dusk has no `break`, so read until end of input with a done flag.

```text
mut done: bool = false
while !done {
    line, e := read_line()
    if e.exists() {
        done = true
    } else {
        print_line(line)
    }
}
```

## std.string

Read only helpers over NUL terminated strings.

| Function                                              | Description                                 |
| ----------------------------------------------------- | ------------------------------------------- |
| `str_len(s: string) -> int64`                         | Length up to the NUL terminator.            |
| `str_eq(a: string, b: string) -> bool`                | True when both strings hold the same bytes. |
| `parse_int(s: string) -> (int64, error)`              | Parse a signed base 10 integer.             |
| `parse_int_radix(s: string, base: int64) -> (int64, error)` | Parse a signed integer in a base from 2 to 36. |
| `parse_float(s: string) -> (float64, error)`          | Parse a base 10 float.                      |

```text
@import std.string

n := str_len("hello")        // 5
same := str_eq("a", "a")     // true

v, e := parse_int("42")      // 42
e.ignore()
h, he := parse_int_radix("0xFF", 16)   // 255
he.ignore()
```

`parse_int` takes a base 10 string, so a `0x`, `0o`, or `0b` prefix fails on the prefix letter. `parse_int_radix` takes the base and accepts the matching prefix, `0x` for 16, `0o` for 8, `0b` for 2, but never infers the base from the prefix. Each parser returns the value with an error you must handle.

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

## std.map

A hash map from string keys to values, generic over the value type. It uses open addressing with linear probing over heap buffers that double and rehash once the table is half full. Pass the map by pointer so inserts and growth persist across calls. Keys are compared by their bytes.

| Function                                          | Description                              |
| ------------------------------------------------- | ---------------------------------------- |
| `map_new<V>() -> Map<V>`                          | A new empty map.                         |
| `map_put<V>(m: *Map<V>, k: string, v: V) -> void` | Insert the value, or overwrite the key.  |
| `map_get<V>(m: *Map<V>, k: string) -> Maybe<V>`   | The value for a key, or `None` when absent. |
| `map_has<V>(m: *Map<V>, k: string) -> bool`       | True when the key is present.            |
| `map_len<V>(m: *Map<V>) -> int64`                 | The entry count.                         |
| `map_free<V>(m: *Map<V>) -> void`                 | Free the backing buffers.                |
| `map_hash(s: string) -> int64`                    | The key hash, exposed for reuse.         |

```text
@import std.map
@import std.functional.maybe

m: *Map<int64> = alloc(map_new())
map_put(m, "two", 2)
map_put(m, "two", 22)
println(map_len(m))                       // 1
println(unwrap_or(map_get(m, "two"), 0))  // 22
map_free(m)
free(m)
```

`map_get` returns a `Maybe<V>`, so import `std.functional.maybe` to unwrap it. Capacity starts at 8 and doubles each time the map fills to half. `map_free` releases the buffers, not the key strings, which the caller still owns.

## std.memory.allocator

The `Allocator` interface and two allocators that implement it. A function that allocates takes an allocator marked `using`, and the builtins `alloc` and `free` dispatch to it. Choosing the allocator type chooses the implementation. A stateful allocator advances in place across calls, since a method takes its receiver by pointer.

| Item                                                   | Description                                          |
| ------------------------------------------------------ | ---------------------------------------------------- |
| `interface Allocator`                                  | `alloc(size, align) -> *void` and `free(p) -> void`. |
| `Heap`                                                 | The libc backed allocator, and the default.          |
| `heap() -> Heap`                                       | A heap allocator value to pass through `using`.      |
| `FixedBuffer`                                          | A bump allocator over a caller buffer, no heap.      |
| `fixed_buffer(base: *int8, cap: int64) -> FixedBuffer` | A fixed buffer allocator over `base`.                |
| `Debug`                                                | A debug allocator that reports leaks and catches a double free, and poisons freed memory. |
| `debug() -> Debug`                                     | A debug allocator value to pass through `using`.     |
| `debug_leaks() -> int64`                               | How many allocations are not yet freed.              |
| `debug_double_frees() -> int64`                        | How many double or invalid frees were seen.          |

```text
@import std.memory.allocator

func fill(using a: FixedBuffer) -> int64 {
    p: *int64 = alloc(8)
    *p = 1
    return a.used
}
```

With no allocator in scope, `alloc` and `free` use the heap.

## std.memory.arena

A bump allocator over one backing buffer. Each allocation carves forward from the buffer. Individual frees do nothing, and you reset or destroy the whole arena at once. Pass the arena by pointer so the offset persists. Arena also implements `Allocator`, so you can pass it with `using` and let the `alloc` builtin dispatch to it.

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
