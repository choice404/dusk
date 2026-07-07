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

### Mutable strings

`StringBuilder` is a growable, heap backed string. Build it on the heap with `alloc(sb_new())` and pass it by pointer so growth persists, the same shape `std.vector` uses. The buffer keeps a NUL after the last character, so `sb_cstr` hands back a valid `string` view at no cost.

| Function                                           | Description                                 |
| -------------------------------------------------- | ------------------------------------------- |
| `sb_new() -> StringBuilder`                        | A fresh empty builder.                      |
| `sb_push_char(s: *StringBuilder, c: char) -> void` | Append one character.                       |
| `sb_push(s: *StringBuilder, t: string) -> void`    | Append every character of a string.         |
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

## std.functional.io

`IO<T>` as a monad over a plain struct, composing through generic `do` like any other monad. It is eager over its carried value: `bind` applies its continuation immediately and stores no closure, so it clears the escape check that a lazy, thunk storing `IO` would trip. `run` bridges the value onto the event loop through the pool offload idiom, so the loop and pool must both be up.

| Function                       | Description                                                             |
| ------------------------------ | ---------------------------------------------------------------------- |
| `io_pure<A>(x: A) -> IO<A>`     | Wrap a value in `IO`.                                                   |
| `bind`, `unit` (in `monad IO`)  | The monad pair a `do IO { ... }` block desugars against.               |
| `run<A>(io: IO<A>) -> A`        | Run the effect on the loop and return the value.                        |

```text
@paradigm functional

@import std.functional.io
@import std.async.loop
@import std.concurrent.pool

le := loop_init()
le.ignore()
pe := pool_start(2)
pe.ignore()
r := run(do IO {
    a <- io_pure(10)
    b <- io_pure(20)
    a + b
})
println(r)   // 30
pool_shutdown()
loop_free()
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
