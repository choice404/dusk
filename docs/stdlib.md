# Standard Library

The dusk standard library lives under `lib/std` and is written in dusk. Import a module with a dotted path, then call its exported names.

```text
@import std.io
@import std.functional.maybe
```

Imported names are flat. After `@import std.io` you call `print_int` and `print_line` with no prefix. You can also qualify a call through its module path, so `std.io.print_line("hi")` reaches the same function. Enum constructors keep their type name, so you write `Maybe.Some(42)` and `Maybe.None`.

## std.io

Console output over the `print` and `println` builtins, plus typed line input that reads a line and parses it.

`print` and `println` are builtins, available everywhere without an import. `print` writes a value with no newline and `println` appends one, each handling a string, an int, a float, a bool, or a char. Build a line piece by piece with `print`, then close it with `println`.

```text
print("score: ")
print(42)
println("")        // ends the line
```

With a value argument, the first argument is a format string whose `{}` holes the rest fill in order. Write `{{` or `}}` for a literal brace. The format string is a literal expanded at compile time, so each hole prints its value by type with no runtime format parser and no allocation, and a hole count that does not match the arguments is a compile error.

```text
println("hello {}", name)            // hello Ada
println("I am {} and I am {}", name, age)
print("no newline {}", 7)
println("{{braces}} and a hole {}", 99)   // {braces} and a hole 99
```

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

## std.logging

Level gated logging to stderr, so program output on stdout stays clean underneath it. Added in 0.5.3.

```text
enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}
```

| Function                            | Description                                                     |
| ------------------------------------ | ------------------------------------------------------------------ |
| `log_set_level(l: LogLevel) -> void` | Set the threshold; a call at or above it fires, anything below it is dropped. |
| `log_debug(msg: string) -> void`     | Log at `Debug`, tagged `[debug]`.                              |
| `log_info(msg: string) -> void`      | Log at `Info`, tagged `[info]`.                                 |
| `log_warn(msg: string) -> void`      | Log at `Warn`, tagged `[warn]`.                                 |
| `log_error(msg: string) -> void`     | Log at `Error`, tagged `[error]`.                               |

```text
@paradigm procedural
@import std.logging

log_info("starting up")      // [info] starting up
log_debug("skipped by default")

log_set_level(LogLevel.Debug)
log_debug("now shown")       // [debug] now shown
```

The order is `Debug < Info < Warn < Error`, and the default threshold is `Info`. The level lives in the C runtime as one atomic word shared by every thread, so `log_set_level` from any thread takes effect everywhere at that thread's next log call.

## std.string

Helpers over NUL terminated strings: length and comparison, searching, trimming, splitting, joining, replacing, ASCII case folding, signed integer parsing and formatting, substring slicing, a growable builder, and the float constant formatting the bootstrap compiler emits its IR through.

| Function                                              | Description                                 |
| ----------------------------------------------------- | ------------------------------------------- |
| `str_len(s: string) -> int64`                         | Length up to the NUL terminator.            |
| `str_eq(a: string, b: string) -> bool`                | True when both strings hold the same bytes. |
| `str_cmp(a: string, b: string) -> int32`              | Unsigned lexicographic compare: negative, zero, or positive, shaped to pass to `vec_sort`. |
| `starts_with(s: string, prefix: string) -> bool`      | True when `s` begins with `prefix`.         |
| `ends_with(s: string, suffix: string) -> bool`        | True when `s` ends with `suffix`, the tail mirror of `starts_with`. |
| `str_find(s: string, needle: string) -> int64`        | The byte offset of the first `needle` in `s`, or -1; the empty needle matches at 0. |
| `str_rfind(s: string, needle: string) -> int64`       | The byte offset of the last `needle` in `s`, or -1; the empty needle matches at `str_len(s)`. |
| `str_contains(s: string, needle: string) -> bool`     | True when `needle` occurs anywhere in `s`.  |
| `substring(s: string, lo: int64, hi: int64) -> string`| A fresh heap string of `s[lo, hi)`, clamped to the string. |
| `trim_start(s: string) -> string`                     | A fresh copy of `s` with leading whitespace removed: space, tab, CR, LF. |
| `trim_end(s: string) -> string`                       | A fresh copy of `s` with trailing whitespace removed: space, tab, CR, LF. |
| `trim(s: string) -> string`                           | A fresh copy of `s` with whitespace removed from both ends: space, tab, CR, LF. |
| `str_split(s: string, sep: string) -> *Vector<string>`| Split `s` on every `sep` into a fresh vector of fresh strings; an empty `sep` yields one copy of `s`. |
| `str_join(parts: *Vector<string>, sep: string) -> string` | Concatenate `parts` with `sep` between each pair into a fresh string. |
| `replace_all(s: string, old: string, new_s: string) -> string` | A fresh copy of `s` with every non overlapping `old` replaced by `new_s`; an empty `old` copies `s`. |
| `repeat(s: string, n: int64) -> string`               | A fresh string of `s` repeated `n` times; `n <= 0` yields the empty string. |
| `to_upper(s: string) -> string`                       | A fresh copy of `s` with ASCII letters uppercased, every other byte unchanged. |
| `to_lower(s: string) -> string`                       | A fresh copy of `s` with ASCII letters lowercased, every other byte unchanged. |
| `int_to_string(n: int64) -> string`                   | The base 10 text of a signed integer.       |
| `int_to_hex16(n: int64) -> string`                    | The `0x` prefixed, 16 digit, uppercase hex of `n` read as a 64 bit word. |
| `parse_int(s: string) -> (int64, error)`              | Parse a signed base 10 integer.             |
| `parse_int_radix(s: string, base: int64) -> (int64, error)` | Parse a signed integer in a base from 2 to 36. |
| `parse_float(s: string) -> (float64, error)`          | Parse a base 10 float.                      |
| `str_from_chars(cs: char[]) -> string`                | Copy a char slice into a fresh heap string the caller owns. |
| `cbuf(s: string) -> *raw char`                        | Copy `s` into a fresh NUL terminated raw buffer for a foreign call to read. |

```text
@import std.string

n := str_len("hello")        // 5
same := str_eq("a", "a")     // true

v, e := parse_int("42")      // 42
e.ignore()
h, he := parse_int_radix("0xFF", 16)   // 255
he.ignore()

a: char[5] = "Hello"
s := str_from_chars(a[0..5])   // "Hello", a heap string
free(s)
```

`str_from_chars` is the bridge back to the dynamic string world: a `char[N]` slices down to a `char[]` and `str_from_chars` copies its bytes into a fresh heap allocated `string`, the same ownership `substring` hands back.

`cbuf` is the bridge the other way, out to a foreign call. A `string` is a fat view typed apart from a raw pointer, so a foreign signature, which takes only a scalar, a `*raw T`, or a `*void`, cannot read one directly; `cbuf` copies the bytes into a fresh, NUL terminated, heap allocated buffer the caller owns and frees once the call that reads it has returned. `std.os`'s `run` and `env` both cross this way.

`parse_int` takes a base 10 string, so a `0x`, `0o`, or `0b` prefix fails on the prefix letter. `parse_int_radix` takes the base and accepts the matching prefix, `0x` for 16, `0o` for 8, `0b` for 2, but never infers the base from the prefix. Each parser returns the value with an error you must handle.

`str_split` and `str_join` are inverse shapes over a `*Vector<string>`, which is why `std.string` imports `std.vector`; the vector and every element it holds are fresh heap allocations the caller frees. The case folds and the trim family are ASCII only by design, a byte at 128 or above passes through untouched, so a multibyte UTF-8 scalar survives them intact and a full Unicode fold stays out of the string module, the same posture the unicode tables took.

### Mutable strings

`StringBuilder` is a growable, heap backed string. Build it on the heap with `alloc(sb_new())` and pass it by pointer so growth persists, the same shape `std.vector` uses. The buffer keeps a NUL after the last character, so `sb_cstr` hands back a valid `string` view at no cost.

| Function                                           | Description                                 |
| -------------------------------------------------- | ------------------------------------------- |
| `sb_new() -> StringBuilder`                        | A fresh empty builder.                      |
| `sb_push_char(s: *StringBuilder, c: char) -> void` | Append one character.                       |
| `sb_push(s: *StringBuilder, t: string) -> void`    | Append every character of a string.         |
| `sb_push_int(s: *StringBuilder, n: int64) -> void` | Append the base 10 text of `n`, no intermediate string. |
| `sb_size(s: *StringBuilder) -> int64`              | The number of characters built.             |
| `sb_cstr(s: *StringBuilder) -> string`             | View the built bytes as a string.           |
| `sb_free(s: *StringBuilder) -> void`               | Free the backing buffer.                    |
| `concat(a: string, b: string) -> *StringBuilder`   | Join two strings into a fresh heap builder. |

```text
@import std.string

g := alloc(sb_new())
sb_push(g, "dusk")
sb_push_char(g, 32)          // a space
sb_push(g, "and dawn")
println(sb_cstr(g))          // dusk and dawn
sb_free(g)
free(g)

r := concat("hello, ", "world")
println(sb_cstr(r))          // hello, world
sb_free(r)
free(r)
```

A builder owns its buffer. `sb_free` releases the buffer and `free` releases the builder struct, so a heap builder is freed with both. `concat` returns a builder whose ownership moves to the caller. The `cstr` builtin underneath `sb_cstr` reinterprets a NUL terminated `*char` as a `string` at no runtime cost, and the view stays valid until the builder next grows or is freed.

### Float constant tokens

A dusk hosted compiler emits a float constant into its IR as the `0x` hex of the value's IEEE 754 bits. These helpers produce that token, so the bootstrap stage and the host stage write the same constant for the same value.

| Function                             | Description                                                        |
| ------------------------------------ | ----------------------------------------------------------------- |
| `f64_to_ir_hex(x: float64) -> string`| The IR constant token for a `float64`: `0x` and the 16 hex digits of `x`'s bits. |
| `f32_to_ir_hex(x: float64) -> string`| The same over the double a `float32` literal rounds to, `(double)(float)x`.       |

The host compiler lowers every float constant, `float64` and `float32` alike, as the `float64` bits of the parsed value and narrows a `float32` with a later `fptrunc`, so a byte for byte match of the emitted token uses `f64_to_ir_hex`. `f32_to_ir_hex` gives the post rounding value a `float32` literal actually holds, for a consumer that wants the double a `float32` constant equals.

## std.vector

A growable array, generic over its element type. The buffer lives on the heap and doubles when it fills. Pass the vector by pointer so growth persists across calls.

| Function                                   | Description                            |
| ------------------------------------------ | -------------------------------------- |
| `vec_new<T>() -> Vector<T>`                | A new empty vector.                    |
| `vec_push<T>(v: *Vector<T>, x: T) -> void` | Append one element, growing if needed. |
| `vec_get<T>(v: *Vector<T>, i: int64) -> T` | The element at index `i`.              |
| `vec_pop<T>(v: *Vector<T>) -> void`        | Drop the last element. A no op on an empty vector. |
| `vec_len<T>(v: *Vector<T>) -> int64`       | The element count.                     |
| `vec_free<T>(v: *Vector<T>) -> void`       | Free the backing buffer.               |
| `vec_sort<T>(v: *Vector<T>, cmp: (T, T) -> int32) -> void` | Sort `v` in place, stable, by `cmp`. |
| `vec_contains<T>(v: *Vector<T>, x: T) -> bool` | Whether `x` appears anywhere in `v`, by `==`. |
| `vec_index_of<T>(v: *Vector<T>, x: T) -> int64` | The index of the first `x` in `v` by `==`, or `-1`. |
| `vec_map<T, U>(v: *Vector<T>, f: (T) -> U) -> *Vector<U>` | A fresh owned vector of `f` applied to each element in order. |
| `vec_filter<T>(v: *Vector<T>, p: (T) -> bool) -> *Vector<T>` | A fresh owned vector of the elements for which `p` is true, in order. |
| `vec_fold<T, A>(v: *Vector<T>, init: A, f: (A, T) -> A) -> A` | Fold `f` left to right from `init` to one value. |
| `vec_take<T>(v: *Vector<T>, i: int64) -> T` | Remove the element at `i` and hand it back as the caller's own value. |

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

Added in 1.4.2, `vec_sort` takes an ordering closure, negative when its first argument sorts before its second, zero when the two are equal, positive when the first sorts after, and reorders `v`'s backing data in place. It runs a bottom up, iterative merge sort: passes double a merged run width from 1 up past `v`'s length, each pass merging adjacent runs into one scratch buffer sized to `v` and copying the whole buffer back before the next pass starts, so a pass always merges from data untouched by itself. The sort is stable, a tie always favors the earlier run, and deterministic, two calls over the same data and comparator always produce the same output order; a vector of fewer than two elements returns immediately with `cmp` uncalled.

```text
@import std.vector

struct Pair {
    key: int32,
    idx: int64,
}

func cmp_pair(a: Pair, b: Pair) -> int32 {
    return a.key - b.key
}

v: *Vector<Pair> = alloc(vec_new())
vec_push(v, Pair { key: 3, idx: 0 })
vec_push(v, Pair { key: 1, idx: 1 })
vec_push(v, Pair { key: 3, idx: 2 })
vec_sort(v, cmp_pair)
// v now reads key 1, key 3 idx 0, key 3 idx 2: the two key 3 pairs keep their
// original relative order.
vec_free(v)
free(v)
```

`vec_contains` and `vec_index_of`, also added in 1.4.2, are a linear scan by `==`, legal only for a `T` that `==` itself accepts, a scalar or a string, never a pointer, dusk's own comparison rule; a `Vector<T>` of a pointer typed `T` rejects both at the ground types the same way any other illegal `==` does.

`vec_map` and `vec_filter`, added in 1.8.1, are the functional pair. Each returns a fresh heap vector the caller owns and frees, leaves the source vector and its elements untouched, and calls the closure exactly once per element in index order. `vec_map` applies `f` and collects the results, and the result element type `U` is inferred from the closure, so a `Vector<int64>` maps to a `Vector<string>` when `f` returns a string. `vec_filter` copies through the elements for which `p` answers true. Both take a capturing lambda or a named function, exactly as `vec_sort` does, and neither is gated by a paradigm, since a standard library generic never reaches the bare `map` and `filter` builtins the functional paradigm gates. `vec_sort` already takes its comparator as a closure, so there is no separate `vec_sort_by`.

```text
@import std.vector
@import std.string

v: *Vector<int64> = alloc(vec_new())
vec_push(v, 1)
vec_push(v, 2)
vec_push(v, 3)

evens := vec_filter(v, lambda (x: int64) -> bool { return x % 2 == 0 })
println(vec_len(evens))     // 1

labels := vec_map(v, lambda (x: int64) -> string { return int_to_string(x) })
println(vec_get(labels, 2)) // "3"

vec_free(evens)
free(evens)
vec_free(labels)
free(labels)
vec_free(v)
free(v)
```

`vec_filter` copies element values, so when `T` is a managed pointer type the returned vector holds the same pointers as the source and both vectors alias the pointed to objects. Freeing an element reachable through both is a double free the ownership checker does not catch across `vec_filter`, and the runtime generational free check is the backstop that faults the stale read rather than corrupting silently. `vec_map`'s results belong to whatever the closure returned: an allocating closure hands the caller owners, which you drain with `vec_take` before you free the outer vector, while plain values and borrowed pointers need only the outer `vec_free` and `free`.

`vec_fold`, added in 1.9.1, reduces a vector to one value. It folds `f` left to right over the elements from `init`, passing the accumulator so far as the first argument and the current element as the second, and each call's result is the next accumulator with the final one returned. An empty vector returns `init` with `f` uncalled, and the source vector is neither mutated nor freed. The accumulator type `A` is independent of the element type `T`, so a `Vector<T>` folds into a value of another type, a running sum, a joined string, or a built structure.

`vec_take` and `map_take`, added in 1.8.0, remove an element and hand it back as the caller's own value: `vec_take(v, i)` removes the element at index `i`, shifting the survivors down to keep their order, and `map_take(m, k)` removes the entry for `k` and returns its value, faulting on a miss so probe with `map_has` first when a key may be absent. The returned value is the same pointer the container held, so a managed element's owner is now the caller and freeing it is legal where freeing a `vec_get` read is not. Both heads declare `owning`, which is what tells the checker the result is the caller's own value rather than a borrow into the container.

## std.map

A hash map generic over both its key type K and its value type V. A key is any hashable type: an integer of any width, a `char`, a `rune`, or a `string`. Keys hash through the `hash` builtin and compare with `==`, so a string key hashes and compares by its content and a scalar key by its value; a struct, pointer, or float key is rejected by name, `cannot hash <type>; a map key is an integer, char, rune, or string`. The map uses open addressing with linear probing over heap buffers that double and rehash once the table is half full. Pass the map by pointer so inserts and growth persist across calls.

| Function                                                | Description                              |
| ------------------------------------------------------- | ---------------------------------------- |
| `map_new<K, V>() -> Map<K, V>`                          | A new empty map.                         |
| `map_put<K, V>(m: *Map<K, V>, k: K, v: V) -> void`      | Insert the value, or overwrite the key.  |
| `map_get<K, V>(m: *Map<K, V>, k: K) -> Maybe<V>`        | The value for a key, or `None` when absent. |
| `map_has<K, V>(m: *Map<K, V>, k: K) -> bool`            | True when the key is present.            |
| `map_remove<K, V>(m: *Map<K, V>, k: K) -> bool`         | Remove a key, true when it was present.  |
| `map_take<K, V>(m: *Map<K, V>, k: K) -> V`              | Remove a key and hand its value back as the caller's own; fault on a miss. |
| `map_len<K, V>(m: *Map<K, V>) -> int64`                 | The entry count.                         |
| `map_keys<K, V>(m: *Map<K, V>) -> *Vector<K>`           | The keys in insertion order, a fresh owned vector. |
| `map_free<K, V>(m: *Map<K, V>) -> void`                 | Free the backing buffers.                |
| `map_hash(s: string) -> int64`                          | The old string hash, now a thin wrapper over `hash`. |

```text
@import std.map
@import std.functional.maybe

m: *Map<string, int64> = alloc(map_new())
map_put(m, "two", 2)
map_put(m, "two", 22)
println(map_len(m))                       // 1
println(unwrap_or(map_get(m, "two"), 0))  // 22
map_free(m)
free(m)

byid: *Map<int64, string> = alloc(map_new())
map_put(byid, 42, "answer")
println(unwrap_or(map_get(byid, 42), "?"))  // answer
```

`map_get` returns a `Maybe<V>`, so import `std.functional.maybe` to unwrap it. Capacity starts at 8 and doubles each time the map fills to half. The map stores keys by value: a scalar key carries no lifetime, and a string key is the caller's pointer, which must outlive the map; `map_free` releases the buffers, never a key string. `map_keys` returns the keys in the order they were first inserted, so iteration is a pure function of the insert sequence rather than the hash layout. A key appears once, at its first insertion; an overwrite does not move it and a grow rehashes without disturbing it. The returned vector is a fresh copy the caller owns and frees with `vec_free` and `free`, independent of the map, so there is no shared owner. Import `std.vector` to walk it with `vec_len` and `vec_get`.

One inference note: a map read whose map argument is a struct field, nested directly inside another generic call such as `unwrap_or`, may fail to pin K, `cannot infer the type parameter 'K' for 'map_get'`. Rebind the field to a locally annotated variable first, `mm: *Map<string, V> = (*s).field`, then call through `mm`; a map held in a local or a parameter infers directly.

## std.set

Added in 1.8.1. An unordered set generic over its element type, a thin wrap over `std.map` keyed by the element with a `bool` value that is always true. The wrap is a real type rather than an alias, so a `*Set<string>` parameter cannot take a map by mistake, and it seals the value channel: membership is exactly key presence, and there is no way to hold an element that is present but false. Build a set on the heap with `alloc(set_new())` and pass it by pointer so inserts persist.

```text
struct Set<T> {
    m: *Map<T, bool>,
}
```

| Function                                    | Description                                                     |
| ------------------------------------------- | --------------------------------------------------------------- |
| `set_new<T>() -> Set<T>`                    | A new empty set.                                                |
| `set_add<T>(s: *Set<T>, x: T) -> bool`      | Add `x`; true when it was newly added, false when already present. |
| `set_has<T>(s: *Set<T>, x: T) -> bool`      | Whether `x` is a member.                                        |
| `set_remove<T>(s: *Set<T>, x: T) -> bool`   | Remove `x`; true when it was present.                          |
| `set_len<T>(s: *Set<T>) -> int64`           | The member count.                                              |
| `set_items<T>(s: *Set<T>) -> *Vector<T>`    | The members in first insertion order, a fresh owned vector.    |
| `set_free<T>(s: *Set<T>) -> void`           | Free the backing map's buffers and the map allocation.        |

```text
@import std.set
@import std.vector

s: *Set<string> = alloc(set_new())
println(set_add(s, "a"))    // true
println(set_add(s, "a"))    // false, already present
println(set_add(s, "b"))    // true
println(set_len(s))         // 2
println(set_has(s, "a"))    // true

items := set_items(s)       // a fresh vector, "a" then "b"
println(vec_len(items))     // 2
vec_free(items)
free(items)

set_free(s)
free(s)
```

`set_add` and `set_remove` report whether the call changed the set, so a repeat of an existing element neither grows the set nor disturbs its first insertion order. `set_items` returns the members in that insertion order as a fresh vector the caller owns and frees with `vec_free` and `free`, the `map_keys` contract verbatim, independent of the set. The element contract is the map key contract verbatim: an element is any hashable type compared with `==`, so a scalar or a string is legal and a pointer element is rejected through the backing map's own hash and `==` restriction. A string element is borrowed and must outlive the set. `set_free` releases the backing map's buffers and the map allocation only; the elements themselves are never freed, and the caller frees the `Set` allocation itself after it returns.

## std.os

A thin wrapper over the process environment, the command shell, and the C library's errno. Every string argument crosses the C boundary through `std.string`'s `cbuf`, since a string is typed apart from a raw pointer.

| Function                        | Description                                                       |
| ------------------------------- | ----------------------------------------------------------------- |
| `run(cmd: string) -> int64`     | Run `cmd` through the C library `system` and return the exit code.|
| `env(name: string) -> string`   | The value of an environment variable, or the empty string when unset. |
| `os_errno() -> int64`           | The C library's errno, read right after a foreign call that may have set it. |
| `errstr(code: int64) -> string` | The message `strerror` reports for an errno value.                |
| `quote(arg: string) -> string`  | Wrap `arg` in single quotes so a POSIX shell reads it as one word. |

```text
@import std.os
@import std.string

code := run("exit 7")            // 7
home := env("HOME")             // "" when unset, never a fault
safe := quote("it's a test")    // 'it'\''s a test'

msg := errstr(2)                // "No such file or directory" (wording varies by platform)
free(msg)
```

`run` returns the child's exit code, decoded from the wait status `system` reports. A normally terminated child reports its exit code. A child the OS kills reports 128 plus the signal, the shell convention, so a process killed by, say, the out of memory killer is never mistaken for a clean exit. `env` reads back the empty string for an unset variable, never a null, so test the result with `str_len` or `str_eq`. `quote` writes every embedded single quote as the four byte close quote, escaped quote, reopen quote sequence, so the quoted result is safe to splice into a command line passed to `run`.

Added in 1.4.0, `os_errno` and `errstr` are the read side of the C library's own error channel; the read carried the bare name `errno` through 1.5.x and 1.6.0 renames it so its symbol never collides with the C `errno` it reads on a target whose libc declares one. dusk never sets errno itself; a call to `os_errno()` always reports whatever the most recent foreign call, a libc function or a third party one, left behind, so read it immediately after the call whose failure it names, before anything else crosses the C boundary and overwrites it. `errstr` hands back `strerror`'s message for a code, `os_errno()`'s own result or a literal like `2` for `ENOENT`, copied off `strerror`'s static buffer into a fresh heap string the caller owns and frees, since that buffer is only good until the thread's next `strerror` call.

## std.process

Added in 1.4.2. Runs a shell command as a child process and reads its output back. The low level `foreign` block binds three C runtime shims, `cool_popen`, `cool_fgets`, and `cool_pclose`, rather than `popen`/`fgets`/`pclose` directly, since a `FILE*` stream cannot ride home as a `*void` a dusk wrapper could NULL test the way a C caller tests `popen`'s own return: `==` on any pointer type is rejected outright, the same constraint `std.fs`'s `Dir` carries for `DIR*`. `Proc` wraps its stream the identical way, one `int64` field holding the pointer's bit pattern, meaningful only to this module's own calls.

| Function                                        | Description                                                       |
| ------------------------------------------------ | ------------------------------------------------------------------- |
| `proc_open(cmd: string) -> (Proc, error)`       | Runs `cmd` through the platform shell and opens a readable pipe to its combined stdout. |
| `proc_read_line(p: Proc) -> (string, bool)`     | The next line without its trailing newline; `false` once the stream is exhausted. |
| `proc_close(p: Proc) -> (int64, error)`         | Closes the stream, reaps the child, and decodes its exit status.  |
| `run_capture(cmd: string) -> (string, int64, error)` | Runs `cmd` to completion and returns its whole output, its exit code, and any error. |

```text
@import std.process

out, code, e := run_capture("echo hello")
e.ignore()
println(out)   // "hello"
println(code)  // 0
```

`proc_open` opens `cmd` through `popen(cmd, "r")`, POSIX shell semantics (`/bin/sh -c`); a failure to open reports `strerror`'s text through the returned `error` and hands back a zeroed `Proc` that is never valid to read from or close. `proc_read_line` reassembles a line across as many internal reads as it takes when a line outruns the 4096 byte read chunk, so no line length silently truncates; a hard read error reports the same `false` as a clean end of stream, since the function carries no error channel of its own. `proc_close` decodes `pclose`'s wait status the way `std.os`'s `run` decodes `system`'s: the low 7 bits name a terminating signal, reported as 128 plus the signal number, and a normal exit reports the shifted down exit code; `pclose` failing outright, a bad handle or a wait failure, reports through the returned `error` instead of a decoded status. `run_capture` is the convenience wrapper over all three: it opens, reads every line into one newline joined string, closes, and hands back the output alongside the decoded exit code, reporting an open failure immediately with a `-1` exit code and no output.

## std.flags

Added in 1.8.1. A small command line flag parser built as register then parse. A program declares its flags up front, then hands the whole `argv` to `flags_parse`, which fills the flag values and collects the leftover positional words in order. There are no short flags and no grouping: a flag is always its long name matched as `--` followed by the registered name, stored without the dashes. Build a `Flags` on the heap with `alloc(flags_new(...))` and pass it by pointer so registration and parsing persist.

| Function                                                        | Description                                                     |
| --------------------------------------------------------------- | --------------------------------------------------------------- |
| `flags_new(prog: string, about: string) -> Flags`              | A fresh parser with no flags registered.                        |
| `flag_bool(f: *Flags, name: string, help: string) -> void`     | Register a boolean flag; `--name` sets it true, `--name=x` is an error. |
| `flag_str(f: *Flags, name: string, def: string, help: string) -> void` | Register a string flag with a default.                  |
| `flag_int(f: *Flags, name: string, def: int64, help: string) -> void`  | Register a base 10 integer flag with a default.         |
| `flags_parse(f: *Flags, argv: string[], start: int64) -> error` | Parse `argv` from index `start`; the error names the first bad token. |
| `flag_get_bool(f: *Flags, name: string) -> bool`               | The bool flag's value, false when it never appeared.            |
| `flag_get_str(f: *Flags, name: string) -> string`             | The string flag's value, its default when it never appeared.    |
| `flag_get_int(f: *Flags, name: string) -> int64`              | The integer flag's value, its default when it never appeared.   |
| `flag_seen(f: *Flags, name: string) -> bool`                  | Whether the flag appeared on the command line at least once.    |
| `flags_pos_len(f: *Flags) -> int64`                           | The count of positional words collected.                        |
| `flags_pos_at(f: *Flags, i: int64) -> string`                 | The positional word at index `i`, in order.                     |
| `flags_usage(f: *Flags) -> string`                            | A fresh owned usage string, deterministic in the registered flags. |
| `flags_free(f: *Flags) -> void`                               | Free the parser's two vectors; the caller frees the `Flags`.    |

```text
@paradigm procedural
@import std.flags

func main(argv: string[]) -> int32 {
    f: *Flags = alloc(flags_new("greet", "print a greeting"))
    flag_bool(f, "loud", "shout the greeting")
    flag_str(f, "name", "world", "who to greet")
    flag_int(f, "times", 1, "how many times")

    e := flags_parse(f, argv, 1)
    if e.exists() {
        println(e.message)
        print(flags_usage(f))
        return 1
    }

    who := flag_get_str(f, "name")
    n := flag_get_int(f, "times")
    println("hello {} x{}, loud={}", who, n, flag_get_bool(f, "loud"))
    println(flags_pos_len(f))     // count of leftover positional words

    flags_free(f)
    free(f)
    return 0
}
```

The grammar reads `argv` from `start`, the index that skips the program name; a subcommand tool passes 2. A lone `--` ends flag parsing, so every token after it is positional even when it begins with a dash. A `--name=value` binds inline and a `--name value` binds the following token; a bool flag takes no value, so `--name` alone sets it true and `--name=x` is an error. Any token that does not begin with `--`, a single dash word or a negative number included, is a positional and keeps its order. A repeated flag is last wins, and `flag_seen` stays true across the repeat.

Bad input on the command line is an error value the caller must handle, not a fault, so the library never prints and never exits on a user's typo. `flags_parse` returns an `error` naming the first bad token, one of `unknown flag '--frob'`, `flag '--timeout' needs a value`, `flag '--timeout' needs an integer, got 'abc'`, or `flag '--verbose' takes no value`, and the caller prints the message beside `flags_usage` and exits itself. A misuse of the library itself is a fault that aborts with a `fatal: flags:` prefix and no source location, the same contract `vec_get`'s bounds fault follows: a duplicate registration, `fatal: flags: flag '--verbose' already registered`; a getter on an unregistered name or the wrong kind, `fatal: flags: no int flag named '--verbose'`; and a positional index out of range, `fatal: flags: positional index out of bounds`.

`flags_usage` returns a fresh heap string the caller owns and frees, the usage line, the about line, then one line per flag in registration order with a `<str>` or `<int>` marker and the flag's default. It is a pure function of the registered flags, so its output is deterministic and a golden pins it byte for byte. `Flags` stores only borrows, the caller's own literals and the `argv` strings, both of process lifetime, so `flags_free` releases the two vectors alone and the caller frees the `Flags` allocation itself.

## std.math

Added in 1.4.0. libm's scalar functions over `float64`, bound straight through the foreign boundary with no C shim of its own, since libm already ships beside libc and every dusk binary already links `-lm`. `pi` and `e` are the two constants libm keeps as macros rather than symbols, so they come back as plain dusk literals. `is_nan` and `is_inf` are the two `float64` predicates glibc exposes as macros too, each reproduced here as a pure dusk expression over IEEE 754's own algebra rather than a foreign call.

| Function                                     | Description                                  |
| --------------------------------------------- | --------------------------------------------- |
| `sin(x)`, `cos(x)`, `tan(x) -> float64`      | The trigonometric functions, `x` in radians. |
| `asin(x)`, `acos(x)`, `atan(x) -> float64`   | Their inverses.                              |
| `atan2(y, x) -> float64`                     | The angle of the point `(x, y)`.             |
| `exp(x) -> float64`                          | `e` raised to `x`.                           |
| `log(x)`, `log2(x)`, `log10(x) -> float64`   | Natural, base 2, and base 10 logarithm.      |
| `sqrt(x)`, `cbrt(x) -> float64`              | Square root and cube root.                   |
| `floor(x)`, `ceil(x)`, `round(x)`, `trunc(x) -> float64` | Round down, up, to nearest, and toward zero. |
| `fmod(x, y) -> float64`                      | The floating point remainder of `x / y`.     |
| `fabs(x) -> float64`                         | The absolute value.                          |
| `hypot(x, y) -> float64`                     | `sqrt(x*x + y*y)`, without the intermediate overflow. |
| `fmin(a, b)`, `fmax(a, b) -> float64`        | The lesser and the greater of two values.    |
| `pi() -> float64`                            | Archimedes' constant, to `float64` precision. |
| `e() -> float64`                             | Euler's number, to `float64` precision.      |
| `is_nan(x: float64) -> bool`                 | Whether `x` is NaN.                          |
| `is_inf(x: float64) -> bool`                 | Whether `x` is positive or negative infinity. |

```text
@import std.math

println(sqrt(9.0) == 3.0)          // true
println(hypot(3.0, 4.0) == 5.0)    // true
println(is_nan(sqrt(-1.0)))        // true
println(is_inf(1.0 / 0.0))         // true
```

`is_nan` is built from `!(x == x)` rather than dusk's own `!=`, since NaN is IEEE 754's only `float64` value that compares unequal to itself; `x != x` answers the same thing today, but the definition here does not lean on it. `is_inf` reads `x == x && x + x == x && x != 0.0`: a finite value only ever satisfies `x + x == x` at `0.0`, which the last clause excludes by name, so what remains is exactly the two infinities, and `x == x` rules out NaN up front.

## std.rand

Added in 1.4.0. xoshiro256**, D. Blackman and S. Vigna's generator, over a heap allocated `Rng` seeded through splitmix64 so every seed, zero included, lands the state away from the all zero fixed point a thin seed would otherwise take many draws to mix away.

| Function                                        | Description                                    |
| ------------------------------------------------ | ----------------------------------------------- |
| `rng_new(seed: int64) -> *Rng`                  | A fresh generator seeded from `seed`.          |
| `rng_next(r: *Rng) -> int64`                    | The next raw 64 bit word, advancing `r`'s state. |
| `rng_range(r: *Rng, lo: int64, hi: int64) -> int64` | An integer drawn uniformly from `[lo, hi)`.   |
| `rng_float(r: *Rng) -> float64`                 | A float drawn uniformly from `[0, 1)`.         |
| `shuffle(r: *Rng, xs: int64[]) -> void`         | Fisher-Yates shuffle, in place.                |
| `rng_seed_os() -> int64`                        | A seed folded from eight bytes of kernel entropy through `getrandom`. |

```text
@import std.rand

r: *Rng = rng_new(42)
println(rng_next(r))
println(rng_range(r, 0, 100))
println(rng_float(r))

xs: int64[5] = [1, 2, 3, 4, 5]
shuffle(r, xs[0..5])
free(r)
```

`rng_new` owns the returned pointer; free it with `free` like any other heap value. `rng_range` halves the raw draw before reducing it, which costs one bit of entropy and is not itself uniform at spans near `2**63`, but keeps a plain `%` from carrying the wrong sign out of a negative draw. `rng_seed_os` is not itself a source of randomness wired into the generator; it is a seed for a caller that wants `rng_new` started from the OS rather than a fixed value, and a short read from `getrandom` is not retried, so a caller after a hardened seed can call it again.

## std.fs

Added in 1.4.1. Files, directories, and paths. The low level `foreign` block binds `open`, `close`, `read`, `write`, `lseek`, `mkdir`, `rmdir`, `unlink`, and `rename` straight from libc; every wrapper below that can fail returns its value alongside an `error` built from `std.os`'s `os_errno()`/`errstr`, read immediately after the call that may have set it.

| Function                                                    | Description                                                       |
| ------------------------------------------------------------ | ------------------------------------------------------------------ |
| `open_file(path: string, flags: int32, mode: int32) -> (int32, error)` | Opens `path`; `mode` applies only when `flags` includes `o_creat()`. |
| `close_file(fd: int32) -> error`                            | Closes a descriptor.                                               |
| `read_bytes(fd: int32, buf: *void, cap: int64) -> (int64, error)` | Reads up to `cap` bytes into `buf`; 0 with no error is end of file. |
| `write_bytes(fd: int32, buf: *void, n: int64) -> (int64, error)` | Writes `n` bytes from `buf`; the count returned can be less than `n` on a short write. |
| `seek(fd: int32, offset: int64, whence: int32) -> (int64, error)` | Repositions `fd`'s offset and returns the resulting absolute offset. |
| `make_dir(path: string, mode: int32) -> error`              | Creates a directory.                                                |
| `remove_dir(path: string) -> error`                         | Removes an empty directory.                                        |
| `remove_file(path: string) -> error`                        | Removes a file, or a symlink itself, never its target.             |
| `move_file(old_path: string, new_path: string) -> error`    | Renames or moves `old_path` to `new_path`.                         |
| `file_stat(path: string) -> (FileStat, error)`              | `size`, `mode`, and `mtime` (seconds since the epoch, UTC).         |
| `dir_open(path: string) -> (Dir, error)`                    | Opens `path` for directory iteration.                              |
| `dir_next(d: Dir) -> (string, bool)`                        | The next entry's bare name, skipping `.` and `..`; `false` once the stream is exhausted. |
| `dir_close(d: Dir) -> error`                                | Closes a directory stream.                                         |
| `path_join(dir: string, name: string) -> string`            | Joins with a single `/`, collapsing any trailing separators on `dir`. |
| `path_dirname(p: string) -> string`                         | Everything up to the last `/`, or `"."` when there is none.        |
| `path_basename(p: string) -> string`                        | Everything after the last `/`, or the whole string.                |
| `path_extension(p: string) -> string`                       | The final component's extension, without its leading `.`, or `""`. |

`o_rdonly()`, `o_wronly()`, `o_rdwr()`, `o_creat()`, `o_excl()`, `o_trunc()`, and `o_append()` are `open`'s Linux/glibc flag values, bitwise ORed together; `seek_set()`, `seek_cur()`, and `seek_end()` are `lseek`'s whence values; `mode_0644()` and `mode_0755()` are the common create permission modes, spelled out so a caller never hand converts octal to decimal.

```text
@import std.fs

fd, err := open_file("out.txt", o_wronly() | o_creat() | o_trunc(), mode_0644())
buf: *raw char = cbuf("hello")
write_bytes(fd, buf, 5)
free(buf)
close_file(fd)

d, derr := dir_open(".")
mut going: bool = true
while going {
    name, found := dir_next(d)
    if !found { going = false } else { println(name) }
}
dir_close(d)
```

`file_stat` and directory iteration are C runtime shims rather than a direct binding, since `struct stat` and `DIR` are C layouts dusk never reads; `FileStat` carries `size`, `mode`, and `mtime` as plain `int64` fields, never the raw `struct stat` layout. `Dir.h` is an opaque token, the directory stream's own pointer as a bit pattern `int64`, meaningful only to `dir_next` and `dir_close`; it is an `int64` rather than a `*void` because `==` on any pointer type is rejected outright, so a wrapper holding a `*void` would have no way to test a failed open against NULL the way a C caller tests `opendir`'s own return, and every call here that can fail reports it through a separate status value instead. `path_dirname`, `path_basename`, and `file_stat` carry the `path_`/`file_` prefix, unlike the rest of this module's wrappers, because dusk links every exported top level function under its bare, unmangled name, and libc already owns `stat(2)`, `dirname(3)`, and `basename(3)`; a bare `stat` export would resolve a call meant for libc's own `stat` to this module's definition instead.

## std.time

Added in 1.4.1. Wall clock reads paired with a pure dusk proleptic Gregorian civil calendar, UTC only; there is no time zone support, and a caller wanting local time converts outside this module.

| Function                                       | Description                                                  |
| ----------------------------------------------- | -------------------------------------------------------------- |
| `now_ns() -> int64`                            | Nanoseconds since the Unix epoch, UTC.                        |
| `now_ms() -> int64`                            | Milliseconds since the Unix epoch, UTC.                       |
| `now_unix() -> int64`                          | Seconds since the Unix epoch, UTC.                            |
| `civil_from_unix(secs: int64) -> Civil`        | The UTC calendar reading of a Unix timestamp.                 |
| `unix_from_civil(c: Civil) -> int64`           | The Unix timestamp of a UTC calendar reading.                 |
| `format_iso8601(c: Civil) -> string`           | Renders `c` as `"YYYY-MM-DDTHH:MM:SSZ"`.                       |
| `weekday(c: Civil) -> int64`                   | The day of week, 0 Sunday through 6 Saturday.                  |
| `parse_iso8601(s: string) -> (Civil, error)`   | Parse an ISO 8601 UTC string, the strict inverse of `format_iso8601`. |

`Civil` is a plain struct, one `int64` field per component: `year` (proleptic Gregorian, so a year before 1 is zero or negative), `month` in `[1, 12]`, `day` in `[1, the month's length]`, and `hour`, `minute`, `second` in their ordinary ranges. There is no sub-second field; `now_ns` and `now_ms` carry finer precision on their own.

```text
@import std.time

secs := now_unix()
c: Civil = civil_from_unix(secs)
text: string = format_iso8601(c)
println(text)                              // an ISO 8601 string, e.g. "2024-03-02T15:04:05Z"
println(unix_from_civil(c) == secs)        // true
```

`now_ns` is the only foreign call in the module, a shim over `clock_gettime(CLOCK_REALTIME)`; `now_ms` and `now_unix` are ordinary arithmetic on top of it. `civil_from_unix` and `unix_from_civil` are Howard Hinnant's `days_from_civil`/`civil_from_days` calendar arithmetic ported to dusk, correct proleptically over the whole `int64` range with no lookup table. `unix_from_civil` does not reject an out of range field, a month past 12 or a day past its month's length; it rolls forward the same way the day count arithmetic always does, the way most civil calendar libraries treat an out of range field as a relative offset rather than a fault.

Added in 1.8.1, `weekday` and `parse_iso8601` extend the calendar. `weekday(c)` returns the day of week for a `Civil` value, 0 for Sunday through 6 for Saturday, the `tm_wday` convention. It shares `days_from_civil` with `unix_from_civil` and does no validation of its own, so an out of range month or day rolls forward through the same arithmetic; `1970-01-01` counts zero days and returns 4, Thursday, and a date before the epoch resolves correctly through the flooring modulo.

```text
@import std.time

c: Civil = Civil { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 0 }
println(weekday(c))                        // 4, Thursday

p, e := parse_iso8601("2000-02-29T12:30:00Z")
e.ignore()
println(p.month)                           // 2
println(p.day)                             // 29, the leap day
println(format_iso8601(p))                 // 2000-02-29T12:30:00Z, the same text back

bad, be := parse_iso8601("2100-02-29T00:00:00Z")
println(be.exists())                       // true, 2100 is not a leap year
```

`parse_iso8601(s)` returns `(Civil, error)` and is the strict inverse of `format_iso8601`. It reads exactly `[-]Y{4,}-MM-DDTHH:MM:SSZ`, at least four ASCII digits of year with an optional leading `-`, two digits for every other field, an uppercase `T` and `Z`, and nothing after the `Z`. It accepts no fractional seconds, no timezone offset, and no lowercase spelling, since those are shapes `format_iso8601` never emits. As an input boundary it validates: the month is 1 to 12, the day is checked by round tripping through `civil_from_days(days_from_civil(y, m, d))` so a leap day is exact with no month length table, the hour is 0 to 23, and the minute and second are 0 to 59, so a literal leap second `:60` is rejected. A structural mismatch returns `iso8601: malformed timestamp` and an out of range field returns `iso8601: field out of range`. The law is `parse_iso8601(format_iso8601(c)) == c` for every representable `Civil`, negative and wider than four digit years included.

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

## std.memory.collector

Control and gauges for the conservative collected heap. The collected heap is not an allocator you pass with `using`. You reach it only by minting a `collector<T>` value, which the checker confines to the main thread and reclaims by a conservative mark and sweep. This module exposes the collection trigger and four read only counters over that heap, so no pointer crosses the boundary and the wrappers are safe to call anywhere on the main thread. A `Collector` allocator over the raw `alloc` builtin does not ship, because a bare `*void` would erase the type the checker confines through.

| Function                       | Description                                                        |
| ------------------------------ | ----------------------------------------------------------------- |
| `gc_collect() -> void`         | Force one full mark and sweep now. Main thread only.              |
| `gc_live_blocks() -> int64`    | How many collected blocks are live.                               |
| `gc_live_bytes() -> int64`     | Total live collected payload bytes.                               |
| `gc_collections() -> int64`    | How many collections have run since start, monotonic.            |

```text
@import std.memory.collector

t := collector<() -> int64>(lambda () -> int64 { return 41 })
gc_collect()
println(t() + gc_collections())
```

## std.unicode

UTF-8 decode, encode, and validation over the string's existing byte view. A string's representation is unchanged: it is still a NUL terminated byte buffer, `s[i]` still reads one byte, and this module is what walks that buffer scalar by scalar. Every function is total, no fault and no unbounded read; a malformed byte or an invalid scalar resyncs to the standard replacement character U+FFFD rather than stopping the walk. Case folding, normalization, and grapheme clustering sit outside this layer.

| Function                                            | Description                                                       |
| ---------------------------------------------------- | ------------------------------------------------------------------ |
| `decode_rune(s: string, i: int64) -> (rune, int64)` | Decode one scalar at byte offset `i`, returning it with its width. |
| `encode_rune(r: rune, buf: *raw char) -> int64`     | Encode a scalar as 1 to 4 UTF-8 bytes into `buf`, sized to at least 4. |
| `rune_len(r: rune) -> int64`                        | The encoded width of a scalar, 1 to 4, with no write.              |
| `rune_count(s: string) -> int64`                    | The scalar count of a NUL terminated string.                       |
| `utf8_valid(s: string) -> bool`                     | Whether `s` is strict, well formed UTF-8.                          |
| `sb_push_rune(sb: *StringBuilder, r: rune) -> void` | Append one scalar's encoded bytes to a builder.                    |

```text
@import std.unicode

mut i: int64 = 0
while s[i] != 0 {
    r, w := decode_rune(s, i)
    println(r)     // the scalar's codepoint number
    i = i + w
}
```

`decode_rune`'s only precondition is that `i` lies in `[0, str_len(s)]`; a normal decode walk that steps by the width it gets back never leaves that range. Every rejection is strict: an overlong encoding, a surrogate, and a scalar above `0x10FFFF` are all invalid, the same as a truncated or malformed sequence, and `utf8_valid` runs the identical decode loop `decode_rune` does so the two can never disagree.

## std.json

A JSON parser and emitter over a recursive `Json` value, pure dusk but for the one runtime shim that formats a number as `%.17g`. `json_parse` reads the full grammar into a heap tree and returns an error value on any malformed input; `json_emit` writes a tree back to compact text with no insignificant whitespace. A parsed number round-trips through emit and back to the same `float64`, since `%.17g` is a faithful decimal form for a double.

The value is a tagged union. `JArr` owns its element vector and `JObj` owns its field map, and both hold child pointers, so a nested document is a tree of `alloc`'d nodes.

```text
enum Json {
    JNull,
    JBool(b: bool),
    JNum(n: float64),
    JStr(s: string),
    JArr(items: *Vector<*Json>),
    JObj(fields: *Map<string, *Json>),
}
```

| Function                                      | Description                                              |
| --------------------------------------------- | -------------------------------------------------------- |
| `json_parse(s: string) -> (*Json, error)`     | Parse one JSON document into a heap tree, or an error.   |
| `json_emit(j: *Json) -> string`               | Emit a tree as compact JSON, a fresh owned string.       |
| `json_free(root: *Json) -> void`              | Deep free a heap tree: every node, payload, key, and buffer. |

```text
@import std.json
@import std.string

v, e := json_parse("{\"xs\":[1,2,3],\"ok\":true}")
if e.exists() {
    println(e.message)
} else {
    out := json_emit(v)    // {"xs":[1,2,3],"ok":true}
    println(out)
    free(out)
}
```

`json_parse` reads the whole grammar: `null`, `true`, `false`; a number with an optional sign, an integer part, an optional fraction, and an optional exponent; a string with the escapes `\" \\ \/ \b \f \n \r \t` and `\uXXXX`, where a `\uD800..\uDBFF` high surrogate pairs with a following `\uDC00..\uDFFF` low surrogate into one scalar above the basic plane; arrays; objects; arbitrary nesting; and insignificant whitespace between tokens. On any malformed input, an unterminated string, a number out of grammar, an unbalanced bracket, a bad literal, or trailing content after the value, it returns an error whose message names the fault, and the returned pointer is a throwaway the caller must not read, so test the error first. On success the error does not exist and the pointer roots a tree the caller owns. A repeated object key keeps the later value, and the earlier value's subtree and the repeat key's bytes are reclaimed during the parse, so `json_free` of the result frees every allocation the document produced.

`json_emit` re-escapes the quote, the backslash, and the control bytes below `0x20`, formats numbers through `%.17g`, and writes object keys in the field map's insertion order, so the same tree always emits the same bytes. A byte at or above `0x20` passes through unchanged, so a multibyte UTF-8 scalar survives a parse and emit round trip intact.

Build a tree by hand with `alloc` and the qualified constructors, `alloc(Json.JNum(3.5))` for a leaf and `alloc(Json.JArr(items))` for a branch. A `match` arm that binds a `JArr` or `JObj` payload is typed from the variant since 1.8.0, so `vec_len`, `vec_get`, and the `map_*` calls infer directly on the binder with no rebinding.

```text
match *j {
    JArr(items) => {
        mut i: int64 = 0
        while i < vec_len(items) {
            emit_value(sb, vec_get(items, i))
            i = i + 1
        }
    }
    // ...
}
```

`json_parse` is safe on adversarial input. It bounds array and object nesting at a fixed depth, well above any real document, and returns a `nesting is too deep` error past it rather than recursing until the stack overflows. It refuses a number whose magnitude overflows a `float64`, `1e400` among them, with a `number out of range` error rather than parsing it to an infinity that would emit as `inf`, which is not JSON and would not reparse.

A parsed tree is a set of managed heap allocations, and `json_free`, added in 1.8.1, reclaims one. It consumes `root`, freeing every node, every string payload, every object key, and every backing buffer, so no pointer into the tree is valid after it returns and a later dereference of any freed block faults named through the generational check. It requires a fully heap allocated tree, which every `json_parse` result is; a hand built tree carrying a literal string payload frees that payload into undefined behavior at its `free`, since a literal is not a heap allocation. A subtree reachable from two parents double frees, and the second free faults named rather than corrupting silently.

```text
@import std.json

v, e := json_parse("{\"xs\":[1,2,3],\"ok\":true}")
if e.exists() {
    println(e.message)
} else {
    out := json_emit(v)
    println(out)
    free(out)
    json_free(v)     // the whole tree, node by node
}
```

`json_free` is a worklist walk rather than a recursion, because the recursive spelling is inexpressible: a `*Json` parameter borrows, so a payload pointer freed through a recursive call rejects as a borrowed free. It pushes `root` onto a work vector, then repeatedly takes a node out with `vec_take`, which hands back the removed element as its owner, frees the node's own payload, and pushes each child onto the work vector by taking it out of its array vector or field map with `vec_take` and `map_take`. This is the first standard library code to call the owning takes 1.8.0 introduced, so a program that parses many documents across one long run can now free each as it finishes instead of holding them until the process exits. A tree you would rather not track at all can still simply never free, which leaks nothing the process end does not reclaim.

## std.functional.maybe

An optional value. It is `Some` with a payload or `None`.

```text
enum Maybe<T> {
    Some(v: T),
    None,
}
```

| Function                                                            | Description                                             |
| -------------------------------------------------------------------- | -------------------------------------------------------- |
| `is_some<T>(m: Maybe<T>) -> bool`                                   | True when the value is `Some`.                          |
| `is_none<T>(m: Maybe<T>) -> bool`                                   | True when the value is `None`.                           |
| `unwrap_or<T>(m: Maybe<T>, fallback: T) -> T`                       | The payload, or `fallback` when `None`.                 |
| `maybe_map<A, B>(m: Maybe<A>, f: (A) -> B) -> Maybe<B>`             | Applies `f` to a `Some` payload, passes `None` through. |
| `maybe_and_then<A, B>(m: Maybe<A>, f: (A) -> Maybe<B>) -> Maybe<B>` | Chains a `Maybe` returning step onto a `Some` payload.  |
| `maybe_or_else<A>(m: Maybe<A>, f: () -> Maybe<A>) -> Maybe<A>`      | Runs `f` for a fallback `Maybe` when the value is `None`. |

```text
@import std.functional.maybe

m: Maybe<int64> = Maybe.Some(42)
println(unwrap_or(m, 0))        // 42

none: Maybe<int64> = Maybe.None
println(unwrap_or(none, 99))    // 99
println(is_none(none))          // 1

doubled := maybe_map(m, lambda (x: int64) -> int64 { return x * 2 })
println(unwrap_or(doubled, 0))  // 84
```

`Maybe` also ships a `monad Maybe { ... }` block, so `do Maybe { ... }` threads `Some` values and short circuits on the first `None`.

## std.functional.either

A value of one of two types. `Left` is the error or first case by convention, `Right` is the success or second case.

```text
enum Either<L, R> {
    Left(l: L),
    Right(r: R),
}
```

| Function                                                                     | Description                                              |
| ------------------------------------------------------------------------------ | ----------------------------------------------------------- |
| `is_left<L, R>(e: Either<L, R>) -> bool`                                     | True when the value is `Left`.                            |
| `left_or<L, R>(e: Either<L, R>, fallback: L) -> L`                           | The `Left` payload, or `fallback` when `Right`.           |
| `right_or<L, R>(e: Either<L, R>, fallback: R) -> R`                          | The `Right` payload, or `fallback` when `Left`.           |
| `either_map<L, R, B>(e: Either<L, R>, f: (R) -> B) -> Either<L, B>`          | Applies `f` to a `Right` payload, passes `Left` through.  |
| `either_map_left<L, R, B>(e: Either<L, R>, f: (L) -> B) -> Either<B, R>`     | Applies `f` to a `Left` payload, passes `Right` through.  |
| `either_and_then<L, R, B>(e: Either<L, R>, f: (R) -> Either<L, B>) -> Either<L, B>` | Chains an `Either` returning step onto a `Right` payload. |
| `either_or_else<L, R>(e: Either<L, R>, f: (L) -> Either<L, R>) -> Either<L, R>` | Runs `f` for a fallback `Either` when the value is `Left`. |

```text
@import std.functional.either

e: Either<int64, int64> = Either.Left(-5)
println(left_or(e, 0))   // -5

r: Either<int64, int64> = Either.Right(6)
doubled := either_map(r, lambda (x: int64) -> int64 { return x * 2 })
println(right_or(doubled, 0))   // 12
```

`Either` ships no `monad Either { ... }` block and so has no `do Either { ... }` form: a `unit` for it would have to pick a free `Left`, and there is no canonical one, so the plain helpers above are the whole surface.

## std.functional.io

`IO<T>` is `struct IO<T> { run: collector<() -> T> }`, a true lazy monad composing through generic `do` like any other monad. Added in 0.5.3, `bind` and `unit` build a new collected thunk instead of running anything, so a `do IO { ... }` chain is a suspended computation the moment it is built and nothing fires until `run` forces it, on the calling thread. The thunk and every step it captures live on the collected heap, so a chain outlives the frame that built it and survives a collection forced between build and force. Building or running a chain touches neither the event loop nor the thread pool.

| Function                                                        | Description                                              |
| ------------------------------------------------------------------ | ----------------------------------------------------------- |
| `io_pure<A>(x: A) -> IO<A>`                                     | Wrap a value in a lazy `IO`.                             |
| `bind`, `unit` (in `monad IO`)                                  | The monad pair a `do IO { ... }` block desugars against. |
| `run<A>(io: IO<A>) -> A`                                        | Force the thunk on the calling thread and return the value. |
| `io_map<A, B>(m: IO<A>, f: collector<(A) -> B>) -> IO<B>`       | Map a pure function over the value once forced.          |
| `io_and_then<A, B>(m: IO<A>, f: collector<(A) -> IO<B>>) -> IO<B>` | Sequence an effectful step after `m`, without `do`.       |
| `io_print(msg: string) -> IO<bool>`                             | Print `msg` with no newline when forced, yields `true`.   |
| `io_println(msg: string) -> IO<bool>`                           | Print `msg` with a newline when forced, yields `true`.    |
| `io_read_line() -> IO<Result<string, string>>`                  | Read one line when forced; `Err` at end of input or on a read error. |

```text
@paradigm functional

@import std.functional.io

func main() -> int32 {
    r := run(do IO {
        a <- io_pure(10)
        b <- io_pure(20)
        a + b
    })
    println(r)   // 30
    return 0
}
```

`IO<T>` does not exist for `void`; an effect that returns nothing yields `bool` instead, as `io_print` and `io_println` do above. As a collected value, an `IO<T>` is confined to the thread that built it: it cannot cross a `spawn` or `submit` capture, a channel, or an interface box.

Before 0.5.3, `run` minted a future and offloaded the carried value to a pool worker, so a program had to bring the event loop and the pool up first and tear them down after. That contract is gone: `run` forces its thunk directly, with no loop or pool required.

## std.functional.result

`Result<T, E>` is `enum Result<T, E> { Ok(v: T), Err(e: E) }`, success or a typed failure. Added in 0.5.3.

```text
enum Result<T, E> {
    Ok(v: T),
    Err(e: E),
}
```

| Function                                                          | Description                                                |
| ---------------------------------------------------------------------- | -------------------------------------------------------------- |
| `bind`, `unit` (in `monad Result`, `E` fixed to `string`)         | The monad pair a `do Result { ... }` block desugars against. |
| `result_ok<T>(v: T) -> Result<T, string>`                        | Wrap a value in `Ok`.                                      |
| `result_err<T>(msg: string) -> Result<T, string>`                | Wrap a message in `Err`.                                   |
| `result_from<T>(v: T, e: error) -> Result<T, string>`             | Bridge a `(value, error)` pair into a `Result`.            |
| `is_ok<T, E>(r: Result<T, E>) -> bool`                            | True when the value is `Ok`.                               |
| `is_err<T, E>(r: Result<T, E>) -> bool`                           | True when the value is `Err`.                              |
| `result_unwrap_or<T, E>(r: Result<T, E>, fallback: T) -> T`       | The payload, or `fallback` when `Err`.                     |
| `result_map<T, E, U>(r: Result<T, E>, f: (T) -> U) -> Result<U, E>` | Applies `f` to an `Ok` payload, passes `Err` through.      |
| `result_map_err<T, E, F>(r: Result<T, E>, f: (E) -> F) -> Result<T, F>` | Applies `f` to an `Err` payload, passes `Ok` through.      |
| `result_and_then<T, E, U>(r: Result<T, E>, f: (T) -> Result<U, E>) -> Result<U, E>` | Chains a `Result` returning step onto an `Ok` payload.     |
| `result_or_else<T, E, F>(r: Result<T, E>, f: (E) -> Result<T, F>) -> Result<T, F>` | Runs `f` for a fallback `Result` when the value is `Err`.  |

The `monad Result { ... }` block fixes `E` to `string`, the common case, since a generic `E` cannot flow through `do` inference; a caller needing a different error type uses the plain constructors and helpers above instead of `do Result { ... }`.

```text
@paradigm functional

@import std.functional.result

func main() -> int32 {
    r := do Result {
        a <- Result.Ok(1)
        b <- Result.Ok(20)
        a + b
    }
    match r {
        Ok(v) => println("ok {}", v),
        Err(e) => println("err {}", e),
    }
    return 0
}
```

`result_from` bridges a fallible call's `(value, error)` return into a `Result`, folding an existing error into `Err(e.toString())` and an absent one into `Ok(v)`. Handing `result_from` a bound error discharges the caller's must handle obligation, the same as handing it to any other parameter declared `error`.

```text
n, e := read_int()
r := result_from(n, e)
match r {
    Ok(v) => println("got {}", v),
    Err(msg) => println("failed: {}", msg),
}
```

## std.async.io

The readiness reactor and the non blocking byte surface it watches. The reactor is one C thread that turns file descriptor readiness into a one shot `Future<int64>` on the event loop; it runs no user code and touches no user memory. Pipes are the deterministic rig to exercise it. Start the loop, then the reactor, before arming any watch, and stop the reactor before freeing the loop.

| Function                                                     | Description                                            |
| -------------------------------------------------------------| ------------------------------------------------------|
| `reactor_start() -> error`                                   | Start the reactor thread; errors on a double start, an OS refusal, or a start while a stop is still in flight. |
| `reactor_stop() -> void`                                     | Stop the thread; faults if a watch is still armed. Restartable after. |
| `readable(fd: int64) -> Future<int64>`                       | Arm a one shot watch for readability; completes with the readiness mask. |
| `writable(fd: int64) -> Future<int64>`                       | Arm a one shot watch for writability; same mask and rules as `readable`. |
| `pipe_new() -> (Pipe, error)`                                 | Create a close on exec, blocking by default pipe.      |
| `fd_nonblock(fd: int64) -> error`                             | Set a descriptor non blocking.                         |
| `fd_close(fd: int64) -> error`                                | Close a descriptor.                                    |
| `read_nb(fd: int64, buf: *void, cap: int64) -> (int64, error)` | Non blocking read into a staged buffer; "would block" or a count, 0 is end of stream. |
| `write_nb(fd: int64, buf: *void, n: int64) -> (int64, error)`  | Non blocking write from a staged buffer; "would block" or a count. |

```text
@import std.async.io
@import std.async.future
@import std.async.loop

le := loop_init()
le.ignore()
se := reactor_start()
se.ignore()

p, pe := pipe_new()
pe.ignore()
ne := fd_nonblock(p.r)
ne.ignore()

w := readable(p.r)

buf: *raw int64 = alloc_bytes(sizeof(int64))
buf[0] = 7
n, we := write_nb(p.w, buf, sizeof(int64))
we.ignore()
println(n)           // 8, bytes written

m, me := await(w)
me.ignore()
println(m)           // 1, readable

v, ve := read_nb(p.r, buf, sizeof(int64))
ve.ignore()
println(v)           // 8, bytes read
println(buf[0])       // 7

free(buf)
ce := fd_close(p.r)
ce.ignore()
cwe := fd_close(p.w)
cwe.ignore()
reactor_stop()
loop_free()
```

The readiness mask is 1 for readable, 2 for writable, 4 for hangup, and 8 for error, ORed together. Only one watch may be armed on a file descriptor at a time; a second watch on an already armed fd is a fault. `future_free` on a readiness future does not disarm its watch. Stage `read_nb` and `write_nb` buffers through `alloc_bytes`, the same idiom `Pipe`'s own two fds use internally. Writing to a pipe whose read end is closed delivers `SIGPIPE` and kills the process; do not write to a pipe with no reader.

## std.async.net

TCP over the readiness reactor. Sockets are non blocking file descriptors the reactor watches, so this module is a thin layer over `std.async.io`. The connecting and accepting calls are `async func`s that await `readable` or `writable` and retry; the setup and teardown calls are synchronous. Literal IPv4 addresses only, no name resolution.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `tcp_listen(port: int64, backlog: int64) -> (int64, error)`      | Bind and listen on loopback; port 0 lets the OS assign one.    |
| `tcp_local_port(fd: int64) -> (int64, error)`                    | The ephemeral port a listener was assigned.                    |
| `tcp_accept(fd: int64) -> (int64, error)`                        | Await a connection and return the client descriptor. Async.    |
| `tcp_connect(host: string, port: int64) -> (int64, error)`       | Connect to a literal IPv4 address, completing the handshake and surfacing a refusal. Async. |
| `tcp_read(fd: int64, buf: *void, cap: int64) -> (int64, error)`  | Await readability and read once; 0 is end of stream. Async.    |
| `tcp_write(fd: int64, buf: *void, n: int64) -> (int64, error)`   | Write every byte, awaiting writability as needed. Async.       |
| `tcp_close(fd: int64) -> error`                                  | Close a descriptor.                                            |

Awaiting any of the async calls is legal only inside an `async func`; a server accept loop and its clients run as tasks under `async_run`.

## Async keywords

Added in 0.4.2. `async func`, `await`, and `async_run` are keywords and a builtin, not a stdlib module, but they ride the same `std.async.future` and `std.async.loop` machinery every other entry in this file describes, so they belong beside it. See the language reference's async chapter for the full signature and statement rules, the fault family, and the cost table.

| Form                          | Description                                            |
| ------------------------------| ------------------------------------------------------|
| `async func f(...) -> T`      | Compiles to a poll function over a heap frame; calling it mints a task and a `Future<T>` and runs nothing until the loop cranks it. No type parameters, no future, slice, closure, or interface value as a parameter or return. |
| `x := await f`                | Suspends until `f` completes; binds the value, discards the completer's error. |
| `x, e := await f`             | Suspends until `f` completes; binds the value and the completer's error. |
| `await f`                     | Suspends until `f` completes; discards the value, legal only when `f`'s element is void. |
| `return await f`              | Suspends until `f` completes, then propagates the value and error to the caller. |
| `async_run(f(args)) -> T`     | Cranks the event loop until a direct call of an async func's future completes, then yields its value. The only sync to async bridge; illegal inside an async func. |

```text
@import std.async.future
@import std.async.loop

async func fetch(n: int64) -> int64 {
    return n * 2
}

async func amain() -> int32 {
    a := await fetch(10)
    b := await fetch(20)
    println(a + b)
    return 0
}

func main() -> int32 {
    le := loop_init()
    le.ignore()
    rc := async_run(amain())
    loop_free()
    return rc
}
```

`await` is legal only in the four statement shapes above, never mid expression, and only directly inside an async func body, never inside a lambda literal created there or under `defer`. A future, a slice, a closure, and an interface value all share one reason for being barred from an async func's signature and from a spawned or submitted lambda's captures: each may view a frame or a thread the task outlives, so a completer on another thread carries a future's raw handle and generation through `complete_raw` instead of the typed value.

## Operators

Added in 0.4.2, the complete operator set, on a thirteen level precedence ladder from loosest to tightest: range, pipe, or, and, comparison, bitwise or, bitwise xor, bitwise and, shift, additive, multiplicative, exponent, then unary and postfix. See the language reference's Expressions and Operators chapter for the full table and every family's rules.

| Operator                                   | Description                                            |
| --------------------------------------------| ------------------------------------------------------|
| `& \| ^ ~`                                  | Bitwise and, or, xor, and unary not, on integers only. |
| `<< >>`                                     | Shift left, arithmetic shift right; a dynamic out of range amount faults, a constant one is a compile error. |
| `+= -= *= /= %= &= \|= ^= <<= >>=`          | Compound assignment; the place is computed once.       |
| `++ --`                                     | Postfix increment and decrement, statement only, no value. |
| `**`                                        | Exponent, right associative, tighter than `* / %`.     |
| `\|>`                                       | Pipe: `x \|> f(a)` rewrites to `f(x, a)`.               |
| `..=`                                       | Inclusive range in a slice index: `a..=b` is `a..b+1`. |

```text
@paradigm procedural

func double(n: int64) -> int64 {
    return n * 2
}

func main() -> int32 {
    mut x: int64 = 10
    x += 5
    x <<= 1
    println(x & 15)
    println(2 ** 10 |> double)
    return 0
}
```
