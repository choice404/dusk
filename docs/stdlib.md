# Standard Library

The dusk standard library lives under `lib/std` and is written in dusk. Import a module with a dotted path, then call its exported names.

```text
@import std.io
@import std.functional.maybe
```

Imported names are flat. After `@import std.io` you call `print_int` and `print_line` with no prefix. You can also qualify a call through its module path, so `std.io.print_line("hi")` reaches the same function. Enum constructors keep their type name, so you write `Maybe.Some(42)` and `Maybe.None`.

## std.io

Console output over the `print` and `println` builtins, the `Writer` and `Reader` stream layer added in 1.17.0, and typed line input that reads a line and parses it.

### Console output

`print` and `println` are builtins, available everywhere without an import. `print` writes a value with no newline and `println` appends one, each handling a string, an integer of any width, signed or unsigned, a float, a bool, or a char. An unsigned value writes unsigned decimal, so a `uint64` at the top of its range prints `18446744073709551615` rather than `-1`, and a `{}` hole and an f-string hole render it the same bytes. Build a line piece by piece with `print`, then close it with `println`.

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

### Writers

Added in 1.17.0. A `Writer` is one value standing for a sink: standard output, standard error, any descriptor, a `StringBuilder`, a set of writers, or nothing at all. It is an enum, so it returns from a function, sits in a struct field or a `*Vector<Writer>`, crosses into an `async func`, and is captured by a `spawn`, the way any other value does.

```text
enum Writer {
    Fd(fd: int64),
    Str(b: *StringBuilder),
    Many(m: *MultiWriter),
    Discard,
}

struct MultiWriter {
    parts: *Vector<Writer>,
}
```

Build one through a constructor rather than naming a variant, and dispatch through the functions rather than matching a `Writer` yourself. A `match` on one compiles today, but the variant set belongs to `std.io` and a later release may add a sink to it, which would break that match and nothing else.

| Function                                                     | Description                                                     |
| ------------------------------------------------------------- | ------------------------------------------------------------------ |
| `stdout_writer() -> Writer`                                  | Descriptor 1, through the C stdout stream, so writes land in order with `println`. |
| `stderr_writer() -> Writer`                                  | Descriptor 2; each write flushes stdout first, the `printerr` rule. |
| `fd_writer(fd: int64) -> Writer`                             | Any descriptor, a file from `open_file` cast with `int64(fd)`, or a socket. |
| `sb_writer(b: *StringBuilder) -> Writer`                     | Appends into a builder the caller owns and frees.                |
| `discard_writer() -> Writer`                                 | Accepts every write, keeps nothing, never fails.                 |
| `multi_new() -> MultiWriter`                                 | An empty set of writers; build it with `alloc(multi_new())`.     |
| `multi_add(m: *MultiWriter, w: Writer) -> void`              | Appends a copy of `w`; a writer added twice receives every write twice. |
| `multi_writer(m: *MultiWriter) -> Writer`                    | Fans a write out to `m`'s parts in insertion order, stopping at the first error. |
| `multi_free(m: *MultiWriter) -> void`                        | Frees the part list; `free(m)` releases the block itself.        |
| `write_buf(w: Writer, buf: *void, n: int64) -> (int64, error)` | Writes all `n` bytes from `buf`; the count is `n` when the error is empty. |
| `write_string(w: Writer, s: string) -> error`                | Writes `s` up to its NUL; `s` is borrowed.                       |
| `write_line(w: Writer, s: string) -> error`                  | `s` and a newline, staged into one buffer and issued as one write. |
| `write_char(w: Writer, c: char) -> error`                    | One byte.                                                         |
| `flush(w: Writer) -> error`                                  | Flushes the C stream behind descriptor 1 or 2; every other writer is unbuffered already. |
| `writer_close(w: Writer) -> error`                           | Closes a descriptor above 2; 0, 1, and 2 are flushed and left open. |

```text
@import std.io
@import std.string

b: *StringBuilder = alloc(sb_new())
m: *MultiWriter = alloc(multi_new())
multi_add(m, stdout_writer())
multi_add(m, sb_writer(b))

e := write_line(multi_writer(m), "written once, kept twice")
e.ignore()

multi_free(m)
free(m)
sb_free(b)
free(b)
```

A descriptor is the whole bridge to `std.fs`, which `std.io` does not import: open the file there, wrap the descriptor here.

```text
fd, oe := open_file("/tmp/notes.txt", o_wronly() | o_creat() | o_trunc(), mode_0644())
oe.ignore()
w: Writer = fd_writer(int64(fd))
we := write_line(w, "one")
we.ignore()
ce := writer_close(w)
ce.ignore()
```

### Readers

A `Reader` is the same shape on the way in: standard input, any descriptor, or a cursor over a string already in memory.

```text
enum Reader {
    Fd(fd: int64),
    Str(c: *StringReader),
}

struct StringReader {
    s: string,
    pos: int64,
    n: int64,
}
```

| Function                                                     | Description                                                     |
| ------------------------------------------------------------- | ------------------------------------------------------------------ |
| `stdin_reader() -> Reader`                                   | Descriptor 0, read directly rather than through the C stdin buffer. |
| `fd_reader(fd: int64) -> Reader`                             | Any descriptor.                                                   |
| `string_reader_new(s: string) -> StringReader`               | A cursor at offset 0 over `s`, measured once; build it with `alloc(string_reader_new(s))`. |
| `string_reader(c: *StringReader) -> Reader`                  | Reads through the cursor, advancing it.                          |
| `read_buf(r: Reader, buf: *void, cap: int64) -> (int64, error)` | Reads up to `cap` bytes; a count of 0 with an empty error is end of stream. |
| `read_to_end(r: Reader) -> (string, error)`                  | Reads to end of stream into a fresh heap string the caller frees. |
| `reader_close(r: Reader) -> error`                           | Closes a descriptor above 2.                                     |
| `io_copy(dst: Writer, src: Reader) -> (int64, error)`        | Pumps `src` into `dst` through an 8192 byte scratch until end of stream. |

```text
@import std.io

c: *StringReader = alloc(string_reader_new("one\ntwo\n"))
text, e := read_to_end(string_reader(c))
e.ignore()
print(text)
free(text)
free(c)
```

`read_buf` reports a short count at the end of the data and then a count of 0 with an empty error, which is end of stream; a failure is a count of 0 with the error set. A NUL byte inside the data ends the string `read_to_end` hands back early, even though every byte read was copied in, since a string is a NUL terminated view; read through `read_buf` when the data is not text.

### Ownership and lifetimes

- A `Writer` is a value. Copy it freely; every copy names the same stream. It never owns heap memory: `sb_writer` borrows a `StringBuilder` the caller frees, `multi_writer` borrows a `MultiWriter` the caller releases with `multi_free` and then `free`, and an `Fd` writer holds the descriptor number and nothing else. A `MultiWriter` must not contain itself, directly or through another set: nothing checks for the cycle and a write through one recurses until the stack runs out.
- `writer_close` closes a descriptor above 2, and after it every copy of that `Writer` is stale: a later write returns an error, it never faults, unless a later open has reused the descriptor number, in which case the bytes land in that stream; close once and drop every copy. `stdout_writer()` and `stderr_writer()` are flushed and left open, never closed; close descriptor 1 through `std.fs` if you really mean it.
- Who closes a descriptor: whoever opened it. A descriptor from `open_file` wrapped with `fd_writer(int64(fd))` may be closed with either `writer_close` or `close_file`, once.
- A `Reader` is a value under the same rules. `string_reader` borrows the `StringReader` cursor, which borrows the string; free the cursor with `free`, then the string if you own it. `reader_close` closes a descriptor above 2 and does nothing else, and a `Reader` over a file neither frees nor closes it when it goes out of scope, since there is no drop.
- `read_to_end` returns a fresh heap string you free.
- Partial writes: `write_buf` loops until every byte is written or the OS fails, and reports the count written before the failure beside the error; `write_string`, `write_line`, and `write_char` hide the count and return the error alone. `EINTR` is retried inside the runtime and never surfaces.
- A `Writer` over descriptor 1 shares the C stdout stream with `println`, so the two interleave in program order; a `Writer` over any other descriptor writes straight to the kernel and `flush` on it does nothing.
- The stdin mixing rule: `stdin_reader()` reads descriptor 0 directly, while the `read_line` and `read_all` builtins and `read_int` and `read_float` read through the C `stdin` buffer. Bytes the C buffer has already pulled in are invisible to `stdin_reader`, so a program picks one of the two for its whole run.
- Blocking only: every `Reader` here blocks. A descriptor set non blocking with `fd_nonblock` returns a `would block` error from `read_buf`; use `std.async.io` for a non blocking stream.
- Threads: a `Writer` and a `Reader` cross a `spawn` as plain values, the pointer inside `Str` and `Many` becoming a borrow. Two threads writing one descriptor each issue one `write(2)` and interleave at that granularity, and a `StringBuilder` or a `MultiWriter` shared across threads is a data race like any other shared heap block.

### Console helpers

The four functions this module started as. They still go through the `print` and `read_line` builtins and are unchanged; reach for a `Writer` or a `Reader` for anything else.

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

Read until end of input by breaking out of the loop on the error.

```text
while true {
    line, e := read_line()
    if e.exists() {
        break
    }
    print_line(line)
}
```

Both builtins read through the C `stdin` buffer, and `stdin_reader()` reads descriptor 0 directly, so the two paths do not mix: pick one for the whole run. See the stdin mixing rule above.

## std.bufio

A buffered reader over any `std.io` `Reader`, so a program reads a stream one line or one byte at a time without paying for a `read(2)` per byte. A line arrives as a fresh heap string rather than as an offset into a buffer that is about to be refilled. Added in 1.17.0.

```text
struct BufReader {
    r: Reader,
    buf: *raw char,
    cap: int64,
    lo: int64,
    hi: int64,
    done: bool,
    err_msg: string,
}
```

| Function                                                        | Description                                                  |
| ----------------------------------------------------------------- | ------------------------------------------------------------------ |
| `bufreader_new(r: Reader, cap: int64) -> *BufReader`            | A heap reader over `r` with a `cap` byte buffer; a `cap` below 16 reads as 16. |
| `bufreader_read(b: *BufReader, buf: *void, cap: int64) -> (int64, error)` | Serves buffered bytes first, then one read straight into `buf`. |
| `bufreader_read_line(b: *BufReader) -> (string, bool)`          | The next line without its newline, paired with `true`; the empty string and `false` once there is none. |
| `bufreader_read_byte(b: *BufReader) -> (char, bool)`            | The next byte paired with `true`, or 0 and `false` at the end.  |
| `bufreader_err(b: *BufReader) -> error`                         | The read failure that ended the stream, or an empty error.      |
| `bufreader_free(b: *BufReader) -> void`                         | Frees the buffer; `free(b)` releases the block itself.          |

```text
@import std.io
@import std.bufio

b: *BufReader = bufreader_new(stdin_reader(), 4096)
while true {
    line, ok := bufreader_read_line(b)
    if ok == false {
        break
    }
    println(line)
    free(line)
}
e := bufreader_err(b)
e.ignore()
bufreader_free(b)
free(b)
```

A line keeps a carriage return that sits before its newline, so a stream with Windows line endings hands back a line ending in `\r` and a program that cares strips it. The last line of a stream that does not end in a newline is returned like any other, and an empty line between two newlines is a line of its own, so the flag rather than the text is what says the stream is done. A line longer than the buffer grows through a builder across as many refills as it takes, with no cap on its length.

`bufreader_read_line` and `bufreader_read_byte` both report `false` once the stream has ended or a read has failed, and `bufreader_err` is what tells the two apart: it is the empty error when the stream simply ran out and the failure itself when a read refused. A line a failure cut short comes back first, with `true`, so no byte already read is lost; the call after it is the one that answers `false`.

```text
line, ok := bufreader_read_line(b)
if ok {
    println(line)
    free(line)
} else {
    e := bufreader_err(b)
    if e.exists() {
        printerr(e.toString())
    }
    e.ignore()
}
```

A `BufReader` is not itself a `Reader` in 1.17.0. A buffered variant inside `std.io`'s enum would put the drain logic in `std.io` and the buffer in `std.bufio`, a cycle between the two files, so a program that wants `io_copy` from a buffered source pumps `bufreader_read` in a loop instead.

```text
scratch: *raw char = alloc_bytes(8192)
mut total: int64 = 0
mut going: bool = true
while going {
    got, e := bufreader_read(b, scratch, 8192)
    if e.exists() || got == 0 {
        going = false
    } else {
        wrote, we := write_buf(dst, scratch, got)
        we.ignore()
        total = total + wrote
    }
}
free(scratch)
```

### Ownership and lifetimes

- A `BufReader` owns its buffer and the copy of the `Reader` it was built over, not the descriptor or the cursor behind that `Reader`. `bufreader_free` releases the buffer, `free` releases the block, and closing the stream is a separate `reader_close`.
- A line from `bufreader_read_line` is a fresh heap string you free. The empty string that comes back with `false` is a literal, so free the string of a `true` result and leave the other alone.
- The message `bufreader_err` carries belongs to the error the source reported and lives as long as that error does, so it is neither copied into the reader nor freed with it.
- Do not mix a `BufReader` with a `read_buf` on the same underlying `Reader`. Bytes already in the buffer are invisible to one and gone from the other, which is the same rule the C `stdin` buffer and `stdin_reader()` follow.
- Reads block, the rule `std.io`'s `Reader` carries. A descriptor set non blocking with `fd_nonblock` lands a `would block` error in the reader, which ends the stream and comes back out of `bufreader_err`; use `std.async.io` for a non blocking stream.

## std.fmt

The verb formatter, added in 1.17.0. A format string carries directives, `%v`, `%s`, `%d`, `%q`, `%x`, `%.2f` and the rest, and each one renders the next argument beside it. dusk has no variadics for a dusk function and no way to ask a monomorphized body what type it was stamped for, so the type travels with the value: every argument is an `Arg`, and a format call takes a slice of them.

```text
enum Arg {
    Int(n: int64),
    Uint(n: uint64),
    Float(x: float64),
    Str(s: string),
    Bool(b: bool),
    Char(c: char),
}
```

| Function                                                     | Description                                             |
| ------------------------------------------------------------ | -------------------------------------------------------- |
| `arg_int(n: int64) -> Arg`                                   | A signed integer argument; a narrower width casts first, `arg_int(int64(n))`. |
| `arg_uint(n: uint64) -> Arg`                                 | An unsigned integer argument, rendered unsigned by `%v` and `%d`. |
| `arg_float(x: float64) -> Arg`                               | A float argument.                                       |
| `arg_str(s: string) -> Arg`                                  | A string argument, borrowed for the call.               |
| `arg_bool(b: bool) -> Arg`                                   | A bool argument.                                        |
| `arg_char(c: char) -> Arg`                                   | A char argument, one byte.                              |
| `arg_error(e: error) -> Arg`                                 | An error as its message, an empty error as the empty string. |
| `format(f: string, args: Arg[]) -> string`                   | The rendering as a fresh heap string the caller frees.  |
| `format_into(b: *StringBuilder, f: string, args: Arg[]) -> void` | Appends the rendering to a builder, the core the rest runs through. |
| `writef(w: Writer, f: string, args: Arg[]) -> error`         | Renders and writes the whole of it to a `Writer` as one write. |
| `print_fmt(f: string, args: Arg[]) -> void`                  | Renders to standard output, discarding the write error the way `print_line` does. |
| `errorf(f: string, args: Arg[]) -> error`                    | An error whose message is the rendering.                |
| `float_to_string(x: float64) -> string`                      | The shortest decimal that reads back as `x`, the `%r` layout, as a fresh heap string. |

```text
@paradigm procedural
@import std.fmt

print_fmt("%-8s %6.2f\n", [arg_str("total"), arg_float(12.5)])   // total     12.50

s: string = format("%s scored %d%%", [arg_str("ada"), arg_int(99)])
println(s)                                                       // ada scored 99%
free(s)
```

### Directives

A directive is a `%`, then any of the flags, then an optional width, then an optional point and precision, then the verb. Every byte outside a directive copies through, `%%` writes one `%` and consumes no argument, and arguments are consumed left to right, one per directive.

```text
directive := '%' flag* width? ('.' precision)? verb
flag      := '-' | '+' | ' ' | '0' | '#'
width     := digit+
precision := digit*
verb      := 'v' | 'r' | 's' | 'q' | 'd' | 'x' | 'X' | 'o' | 'b' | 'c' | 't'
           | 'f' | 'e' | 'E' | 'g' | 'G' | '%'
```

| Verb        | Int              | Uint             | Float               | Str                   | Bool           | Char         |
| ----------- | ---------------- | ---------------- | ------------------- | --------------------- | -------------- | ------------ |
| `%v`        | decimal          | unsigned decimal | what `println` writes | the bytes           | `true`/`false` | the byte     |
| `%r`        | as `%v`          | as `%v`          | shortest round trip | as `%v`               | as `%v`        | as `%v`      |
| `%s`        | as `%v`          | as `%v`          | as `%v`             | the bytes, precision truncates | as `%v` | as `%v` |
| `%d`        | decimal          | unsigned decimal | marker              | marker                | marker         | the code     |
| `%x` `%X`   | `-` then hex magnitude | hex        | marker              | hex of each byte      | marker         | hex of the code |
| `%o` `%b`   | `-` then magnitude | octal, binary  | marker              | marker                | marker         | the code     |
| `%c`        | UTF-8 of the scalar | UTF-8 of the scalar | marker         | marker                | marker         | the byte     |
| `%q`        | marker           | marker           | marker              | double quoted, escaped | marker        | single quoted, escaped |
| `%t`        | marker           | marker           | marker              | marker                | `true`/`false` | marker       |
| `%f` `%e` `%E` `%g` `%G` | marker | marker      | the C conversions   | marker                | marker         | marker       |

`-` left justifies inside the width, padding on the right with spaces. `0` pads a numeric verb with zeros after the sign and after a `#` prefix; it is ignored with `-`, on a non numeric verb, on `inf` and `nan`, and on an integer verb carrying a precision, which is what C does. `+` writes a `+` before a non negative number and ` ` writes a space there, `+` winning when both are given. `#` writes `0x`, `0X`, `0o`, or `0b` before `%x`, `%X`, `%o`, and `%b`, and keeps `%g`'s trailing zeros and its point.

A width and a precision count bytes, not scalars, so padding a string of non ASCII text lines up by byte count; a scalar aware pad waits for a `std.string` pad family. Both are capped at 4096. Precision on `%s` truncates to that many bytes, on `%d`, `%x`, `%o`, and `%b` it is the minimum digit count, on `%f`, `%e`, and `%g` it is the C meaning, and on `%v` over a float it selects `%g` at that precision. A float verb with no precision runs at six, C's own default.

`%x` and `%o` and `%b` over a negative integer write a `-` and the magnitude rather than the two's complement word C writes, so `%x` of -42 is `-2a`, and the magnitude of the most negative `int64` is taken in the unsigned width so it does not overflow back to itself.

`%q` wraps a string in double quotes and a char in single quotes. In both, `\` becomes `\\`, a newline `\n`, a tab `\t`, a carriage return `\r`, every other byte below 0x20 and the byte 0x7F becomes `\xNN` in lowercase hex, and every byte at 0x80 and above passes through so UTF-8 stays readable. Each form escapes its own delimiter and leaves the other bare: a string writes `"` as `\"` and `'` as is, a char writes `'` as `\'` and `"` as is.

`inf`, `-inf`, `nan`, and `-nan` render under every float verb, glibc's own spelling and the one `println` already writes. A width applies to them, the `0` flag does not, and `+` and ` ` write their sign the way C does.

### Markers

A directive never fails and a format call has no error channel, so a mistake in the literal renders a marker into the output and the program keeps running. A wrong call shows up in what it printed, which is the right place for a mistake in a literal, and never as a fault or a condition the caller has to resolve.

| Mistake                         | Marker                        |
| ------------------------------- | ----------------------------- |
| no argument left for a directive | `%!d(MISSING)`               |
| a verb the type does not take   | `%!d(string=hello)`           |
| a verb nothing defines          | `%!z(int64=5)`                |
| a `%` at the end of the format  | `%!(NOVERB)`                  |
| arguments left over             | `%!(EXTRA int64=5, string=x)` |

A marker prints the verb, the argument's type name, and its `%v` rendering, and takes no width and no precision, so it arrives exactly as written above. The extra marker goes out once, at the end.

### Floats and println parity

`println`, `printerr`, a `{}` hole, and an f string hole all render a float through the C general conversion at precision 6. `%v` and a `%g` with no precision reproduce that byte for byte, through `std.fmtfloat`'s exact digit generation rather than through the C library, so `format("%v", [arg_float(x)])` and `f"{x}"` never disagree over the same value. The shortest form that reads back as the same double is the separate verb `%r`, and `float_to_string` is the same rendering under a name a program reaches for directly.

```text
x: float64 = 0.1
a: string = format("%v %r", [arg_float(x), arg_float(x)])
println(a)              // 0.1 0.1
free(a)

y: float64 = 1.0 / 3.0
b: string = format("%v %r", [arg_float(y), arg_float(y)])
println(b)              // 0.333333 0.3333333333333333
free(b)
```

### Ownership

Every argument is borrowed and nothing here frees one, so an `Arg.Str` never takes the string it carries. `format` returns a fresh heap string the caller frees, `format_into` allocates nothing beyond the builder's own growth, and `errorf`'s message is a heap string the error owns for as long as the error is alive, the convention every `error { message: errstr(code) }` in `std.fs` already follows. `writef` writes up to the first NUL in the rendering, the rule `write_string` follows, so a `%c` of the byte 0 ends what is written.
Two edges worth knowing. A `%c` of the scalar 0 writes a NUL byte, and since a string ends at its first NUL, `format` measures the result up to it while `format_into` keeps every byte in the builder; `writef` stops there too, the rule its own entry states. And `%#g` follows the C standard where glibc does not: when rounding carries into a new power of ten, `%#.2g` of 99.5 prints `1.0e+02` here (C99 and Python agree) where glibc prints `1.e+02`; without `#` the two never differ, and the parity goldens cover every other flag.

`std.fmtfloat` and `std.fmtbig` are the two files under the formatter: the first is the float printer (`float_to_string` and the fixed, exponent, and general layouts), the second is the exact integer arithmetic it runs on, 48 limbs of 32 bits. Both reach a program through `@import std.fmt`; neither is a module to import on its own.

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

f, fe := parse_float("2.5")  // 2.5, a builtin, no import needed
fe.ignore()

a: char[5] = "Hello"
s := str_from_chars(a[0..5])   // "Hello", a heap string
free(s)
```

`str_from_chars` is the bridge back to the dynamic string world: a `char[N]` slices down to a `char[]` and `str_from_chars` copies its bytes into a fresh heap allocated `string`, the same ownership `substring` hands back.

`cbuf` is the bridge the other way, out to a foreign call. A `string` is a fat view typed apart from a raw pointer, so a foreign signature, which takes only a scalar, a `*raw T`, or a `*void`, cannot read one directly; `cbuf` copies the bytes into a fresh, NUL terminated, heap allocated buffer the caller owns and frees once the call that reads it has returned. `std.os`'s `run` and `env` both cross this way.

`parse_int` takes a base 10 string, so a `0x`, `0o`, or `0b` prefix fails on the prefix letter. `parse_int_radix` takes the base and accepts the matching prefix, `0x` for 16, `0o` for 8, `0b` for 2, but never infers the base from the prefix. Each parser returns the value with an error you must handle.

Float parsing is not a `std.string` export. `parse_float(s: string) -> (float64, error)` is a builtin, available everywhere with no import and gated behind no paradigm, the same way `read_file` is, and it returns the value with an error on the same must handle rule the integer parsers follow. `std.io`'s `read_float` and `std.json`'s number parse both go through it.

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

A builder owns its buffer. `sb_free` releases the buffer and `free` releases the builder struct, so a heap builder is freed with both. `concat` returns a builder whose ownership moves to the caller. The `cstr` builtin underneath `sb_cstr` reinterprets a NUL terminated `*char` as a `string` at no runtime cost, and the view stays valid until the builder next grows or is freed. The view is a borrow of the builder's buffer, so `free(sb_cstr(b))` is refused at check; `sb_free` releases the bytes.

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
| `vec_set<T>(v: *Vector<T>, i: int64, x: T) -> void` | Overwrite the element at index `i` in place. |
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

`vec_set` writes over the element already at `i`, bounds checked exactly as `vec_get`, a negative index or one at or past the length printing `fatal: vector index out of bounds` and aborting. The prior element is overwritten rather than freed, so a managed pointer element the caller means to replace is freed before the call or its object leaks; `vec_take` is the shape that hands the old element back instead.

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

`vec_map` and `vec_filter`, added in 1.8.1, are the functional pair. Each returns a fresh heap vector the caller owns and frees, leaves the source vector and its elements untouched, and calls the closure exactly once per element in index order. `vec_map` applies `f` and collects the results, and the result element type `U` is inferred from the closure, so a `Vector<int64>` maps to a `Vector<string>` when `f` returns a string. `vec_filter` copies through the elements for which `p` answers true. Both take a lambda or a named function, exactly as `vec_sort` does, and neither is gated by a paradigm, since a standard library generic never reaches the bare `map` and `filter` builtins the functional paradigm gates. `vec_map` keeps what its closure returns, and the checker reads that result by the closure's declared result type: a number, a string, or a struct or enum of those holds no reference, so `vec_map(v, lambda (x: int64) -> int64 { return x + k })` over a local `k` or a parameter vector builds. A closure that captures a binding of the calling frame and declares a result that can hold a reference, a pointer or a slice, `vec_map(v, lambda (x: int64) -> *Node { return alloc(Node { v: x + k }) })`, is refused, `this call may store a closure that captures the current frame beyond it; put the captured backing on the heap`, or `argument 2's view is stored into argument 1 and may outlive this frame` when `v` is a parameter, while a lambda that captures nothing, a named function, or the loop written out, `for x in v { vec_push(out, alloc(Node { v: x + k })) }`, builds, and `vec_filter`, `vec_fold`, and `vec_sort` take a capturing lambda as before. `vec_sort` already takes its comparator as a closure, so there is no separate `vec_sort_by`.

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

A hash map generic over both its key type K and its value type V. A key is any hashable type: an integer of any width, signed or unsigned, a `char`, a `rune`, or a `string`. Keys hash through the `hash` builtin and compare with `==`, so a string key hashes and compares by its content and a scalar key by its value; a `Map<uint64, V>` keys across the whole unsigned width, since an unsigned key hashes by zero extension and compares by magnitude; a struct, pointer, or float key is rejected by name, `cannot hash <type>; a map key is an integer, char, rune, or string`. The map uses open addressing with linear probing over heap buffers that double and rehash once the table is half full, so the slot count is always a power of two and a slot comes from masking the hash rather than dividing by it. Pass the map by pointer so inserts and growth persist across calls.

| Function                                                | Description                              |
| ------------------------------------------------------- | ---------------------------------------- |
| `map_new<K, V>() -> Map<K, V>`                          | A new empty map.                         |
| `map_put<K, V>(m: *Map<K, V>, k: K, v: V) -> void`      | Insert the value, or overwrite the key.  |
| `map_get<K, V>(m: *Map<K, V>, k: K) -> Maybe<V>`        | The value for a key, or `None` when absent. |
| `map_has<K, V>(m: *Map<K, V>, k: K) -> bool`            | True when the key is present.            |
| `map_remove<K, V>(m: *Map<K, V>, k: K) -> bool`         | Remove a key, true when it was present.  |
| `map_take<K, V>(m: *Map<K, V>, k: K) -> V`              | Remove a key and hand its value back as the caller's own; fault on a miss. |
| `map_len<K, V>(m: *Map<K, V>) -> int64`                 | The entry count.                         |
| `map_key_at<K, V>(m: *Map<K, V>, i: int64) -> K`        | The key at insertion order position `i`, borrowed from the map. |
| `map_val_at<K, V>(m: *Map<K, V>, i: int64) -> V`        | The value stored for that key, borrowed from the map. |
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

`map_key_at` and `map_val_at` walk a map by position without allocating the copy `map_keys` returns. Position `i` runs from 0 to `map_len(m) - 1` and visits every live key exactly once in the order the keys first entered, the same order `map_keys` reports, and both are bounds checked through `vec_get` so an `i` outside the range faults rather than reading a stale slot. Both hand back a borrow the map still owns, so a string key read this way is not freed and a managed value is drained with `map_take` when the caller needs it as its own.

```text
@import std.map

mut i: int64 = 0
while i < map_len(m) {
    k := map_key_at(m, i)
    v := map_val_at(m, i)
    println("{} = {}", k, v)
    i = i + 1
}
```

The `for k, v in m` loop is the same walk written as a statement, lowered into exactly this index loop over the map's own accessors.

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
| `proc_read_line(p: *Proc) -> (string, bool)`    | The next line without its trailing newline; `false` once the stream is exhausted. |
| `proc_close(p: *Proc) -> (int64, error)`        | Closes the stream, reaps the child, and decodes its exit status.  |
| `run_capture(cmd: string) -> (string, int64, error)` | Runs `cmd` to completion and returns its whole output, its exit code, and any error. |

```text
@import std.process

out, code, e := run_capture("echo hello")
e.ignore()
println(out)   // "hello"
println(code)  // 0
```

`proc_read_line` and `proc_close` take the `Proc` by pointer, not by value, so the stream cannot be copied and advanced from two aliases: a `Proc` names one OS stream that a single owner reads and closes. `proc_open` returns the `Proc` by value, so the caller stages it on the heap, `pp: *Proc = alloc(p)`, and frees that allocation once after the last close.

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
| `sin(x: float64)`, `cos(x: float64)`, `tan(x: float64) -> float64` | The trigonometric functions, `x` in radians. |
| `asin(x: float64)`, `acos(x: float64)`, `atan(x: float64) -> float64` | Their inverses.                   |
| `atan2(y: float64, x: float64) -> float64`   | The angle of the point `(x, y)`.             |
| `exp(x: float64) -> float64`                 | `e` raised to `x`.                           |
| `log(x: float64)`, `log2(x: float64)`, `log10(x: float64) -> float64` | Natural, base 2, and base 10 logarithm. |
| `sqrt(x: float64)`, `cbrt(x: float64) -> float64` | Square root and cube root.              |
| `floor(x: float64)`, `ceil(x: float64)`, `round(x: float64)`, `trunc(x: float64) -> float64` | Round down, up, to nearest, and toward zero. |
| `fmod(x: float64, y: float64) -> float64`    | The floating point remainder of `x / y`.     |
| `fabs(x: float64) -> float64`                | The absolute value.                          |
| `hypot(x: float64, y: float64) -> float64`   | `sqrt(x*x + y*y)`, without the intermediate overflow. |
| `fmin(a: float64, b: float64)`, `fmax(a: float64, b: float64) -> float64` | The lesser and the greater of two values. |
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

`path_join`, `path_dirname`, `path_basename`, and `path_extension` each return a fresh heap string the caller frees, the fallback branches included: the `"."` a path with no separator yields and the empty extension are heap copies like every other result, so a caller frees what it got without asking which branch produced it.

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
| `format_imf_fixdate(c: Civil) -> string`       | Renders `c` as an RFC 9110 IMF fixdate, `"Sun, 06 Nov 1994 08:49:37 GMT"`. |

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

Added in 1.17.2, `format_imf_fixdate(c)` renders the IMF fixdate RFC 9110 gives an HTTP `Date` field: the three letter day name from `weekday`, a comma, the two digit day, the three letter month name, the year padded to four digits, the time as `HH:MM:SS`, and the literal `GMT`, since every `Civil` here is UTC. The value is normalized through `unix_from_civil` and `civil_from_unix` first, so an out of range field rolls forward exactly as `unix_from_civil` reads it and the day name, the date, and the time always name the same instant. The result is a fresh heap string you free. The format is defined for the years 0 through 9999, the four digits the IMF fixdate grammar gives a year; an instant outside them renders a year of more digits or a sign, which no HTTP recipient reads as a date.

```text
@import std.time

d := format_imf_fixdate(civil_from_unix(784111777))
println(d)                                 // Sun, 06 Nov 1994 08:49:37 GMT
free(d)
```

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

## std.memory.heap

Gauges for the generational heap, the heap every `alloc`, string, and container draws from when no allocator is named with `using`. The runtime keeps the counters under the lock it already takes, so a reading is cheap and one consistent moment of the heap across threads. Take a reading, run the code under suspicion, take another, and a delta that never returns to zero is a leak. The counters cover the runtime's own records of a future or a spawned thread too, which are live until awaited or joined. A `using` allocator's blocks, the collected heap, task frames, closure environments, and channel rings are outside them.

| Function                       | Description                                                                                                     |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| `heap_live_blocks() -> int64`  | How many blocks are allocated and not yet freed.                                                                 |
| `heap_live_bytes() -> int64`   | The bytes the live blocks asked for, a string's terminator included, so `"abc" + "def"` is 7.                   |
| `heap_held_bytes() -> int64`   | The capacity of every block the heap holds, live or parked for reuse. Never shrinks, except under `--sanitize=address`, where a freed block leaves the heap. |
| `heap_report() -> void`        | Print the live totals to stderr after flushing stdout, checked against a walk of the heap.                      |

```text
@import std.memory.heap

before := heap_live_blocks()
s := "abc" + "def"
println(heap_live_blocks() - before)
free(s)
println(heap_live_blocks() - before)
```

This prints `1` and then `0`. `heap_report()` recomputes the totals from the heap itself and prints a drift line if they disagree with the counters. In a build with `--heap-sites` it also groups the live blocks by the line of the program that allocated each and the calls that led there, and prints the same report at exit whenever a block is still live:

```text
dusk heap: 5 blocks, 86 bytes live, 3 sites
      1 block        32 bytes  examples/sites_basic.dusk:14
      3 blocks       30 bytes  examples/sites_basic.dusk:10
      1 block        24 bytes  examples/sites_basic.dusk:13
```

Without the flag the report prints the totals and `dusk heap: build with --heap-sites to see allocation sites`. The spec's Heap Sites section covers the report's environment variables, `DUSK_HEAP_EXIT`, `DUSK_HEAP_DEPTH`, and `DUSK_HEAP_SIGNAL`.

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

## std.concurrent.thread

Helpers around the `spawn` and `join` builtins. The builtins themselves need no import: `spawn f(args)` starts `f` on a fresh OS thread and yields a typed handle, and `join` on that handle blocks until the thread returns and hands back its value, so a thread carries any return type across the join. This module holds what rides beside them, which today is sleep.

| Function                    | Description                                                        |
| ---------------------------- | ------------------------------------------------------------------- |
| `sleep_ms(ms: int64) -> void` | Block the calling thread for `ms` milliseconds against the wall clock. |

```text
@import std.concurrent.thread

worker := spawn compute(input)
sleep_ms(10)
result := join worker
```

`sleep_ms` parks a real OS thread, so a task on the event loop uses `std.async.time`'s `sleep_async` instead, which yields the loop while it waits. A non positive duration returns at once.

## std.concurrent.sync

A mutex and a condition variable over pthread, the sanctioned way to mutate shared memory between threads. The blessed shape is a `*raw` buffer guarded by one `Mutex`: lock, touch the buffer, unlock, and inside a function body prefer `lock(m)` followed by `defer unlock(m)`. Both handles are one word and copy freely, so every copy names the same lock or variable.

```text
struct Mutex {
    h: *void,
}

struct Condvar {
    h: *void,
}
```

| Function                                  | Description                                                       |
| ------------------------------------------ | ------------------------------------------------------------------- |
| `mutex_new() -> Mutex`                    | A fresh mutex.                                                    |
| `lock(m: Mutex) -> void`                  | Acquire, blocking until the mutex is free.                        |
| `unlock(m: Mutex) -> void`                | Release.                                                          |
| `mutex_free(m: Mutex) -> void`            | Free the mutex once no thread holds or contends it.               |
| `cond_new() -> Condvar`                   | A fresh condition variable.                                       |
| `cond_wait(cv: Condvar, m: Mutex) -> void`| Release `m`, sleep until a signal or broadcast, reacquire `m`.    |
| `cond_signal(cv: Condvar) -> void`        | Wake one waiter, if any thread is waiting.                        |
| `cond_broadcast(cv: Condvar) -> void`     | Wake every waiter.                                                |
| `cond_free(cv: Condvar) -> void`          | Free the variable once no thread waits on it.                     |

```text
@import std.concurrent.sync

m := mutex_new()
lock(m)
defer unlock(m)
balance[0] = balance[0] + 1
```

The mutex is the error checking kind, so relocking by the holder, unlocking by a non holder, and freeing a held mutex all fault by name instead of hanging or corrupting. An `unlock` happens before the next `lock` of the same mutex, which is the ordering that makes the guarded memory safe to touch. `cond_wait` requires the caller to hold the mutex and wakeups can be spurious, so always wait inside a loop that rechecks the predicate.

```text
lock(m)
while ready[0] == 0 {
    cond_wait(cv, m)
}
unlock(m)
```

## std.concurrent.atomic

A sequentially consistent `int64` atomic over a heap word, the way to share a mutable counter between threads without a mutex. The handle copies by value and every copy names the same word, so a spawned thread that receives a copy increments the shared counter.

```text
struct AtomicInt {
    p: *raw int64,
}
```

| Function                                                        | Description                                          |
| ---------------------------------------------------------------- | ------------------------------------------------------ |
| `atomic_new(v: int64) -> AtomicInt`                             | Allocate a counter initialized to `v`.               |
| `atomic_load(a: AtomicInt) -> int64`                            | The value observed at this point.                    |
| `atomic_store(a: AtomicInt, v: int64) -> void`                  | Overwrite the value.                                 |
| `atomic_add(a: AtomicInt, d: int64) -> int64`                   | Add `d`, which may be negative, and return the new value. |
| `atomic_cas(a: AtomicInt, expect: int64, desired: int64) -> bool` | Compare and swap; true when the counter held `expect`. |
| `atomic_free(a: AtomicInt) -> void`                             | Free the counter's word.                             |

```text
@import std.concurrent.atomic

counter := atomic_new(0)
worker := spawn increment_many(counter)
atomic_add(counter, 1)
join worker
total := atomic_load(counter)
atomic_free(counter)
```

The atomics order the accesses they mediate, so a counter touched only through this surface is race free. Exhaustion at `atomic_new` is fatal, the allocator's contract, and freeing the word while any thread still touches it is fatal too, so free after the last join.

## std.concurrent.channel

A bounded, thread safe queue for handing values between threads. A send blocks while the channel is full, a receive blocks while it is empty, and closing wakes everyone. The handle is one word and copies freely, including into a spawned lambda's captures, and every copy names the same channel. The sanctioned shutdown order is close, then join every thread that touches the channel, then free.

```text
struct Channel<T> {
    h: *void,
}
```

| Function                                                    | Description                                                     |
| ------------------------------------------------------------ | ----------------------------------------------------------------- |
| `chan_new<T>(cap: int64) -> Channel<T>`                     | A channel holding at most `cap` elements, `cap` at least one.   |
| `chan_send<T>(c: Channel<T>, x: T) -> error`                | Send, blocking while full; the error exists on a closed channel. |
| `chan_recv<T>(c: Channel<T>) -> (T, error)`                 | Receive the oldest value, blocking while empty; the error exists once closed and drained. |
| `chan_try_send<T>(c: Channel<T>, x: T) -> error`            | Send only if there is room right now, never blocking.           |
| `chan_try_recv<T>(c: Channel<T>) -> (T, error)`             | Receive only if a value is ready right now, never blocking.     |
| `chan_recv_timeout<T>(c: Channel<T>, ms: int64) -> (T, error)` | Receive, blocking at most `ms` milliseconds against a monotonic clock. |
| `chan_recv_async<T>(c: Channel<T>) -> Future<T>`            | A loop future completing with what `chan_recv` would return, without parking the loop thread. |
| `chan_close<T>(c: Channel<T>) -> void`                      | Close and wake every blocked sender and receiver. Idempotent.   |
| `chan_free<T>(c: Channel<T>) -> void`                       | Free the channel after every touching thread has joined.        |

```text
@import std.concurrent.channel

jobs: Channel<int64> = chan_new(8)
producer := spawn worker(jobs)
e := chan_send(jobs, 42)
e.ignore()
chan_close(jobs)
join producer
chan_free(jobs)
```

`chan_new` sizes the element from the binding annotation, the same rule `alloc` uses, so a bare `jobs := chan_new(8)` cannot pin `T` and is a compile error. A capacity below one or exhausted memory is fatal rather than an error. The refusals read differently by wording: `send on a closed channel`, `channel is full`, `receive on a closed, drained channel`, `channel is empty`, and `receive timed out`, and the value beside any receive error is the zero pattern for `T`. A value crosses by copy, so hand a managed pointer over with `chan_send(c, move(p))` and let the receiver own it; a moved value refused by a closed channel never entered the ring and its record leaks, so finish sending before anyone closes. Managed pointers still buffered at `chan_free` are dropped as raw bytes and leak, so drain before freeing when elements own heap records. An element type containing a slice, a closure, or an interface value is rejected at compile time, since each may view the sending frame.

`chan_recv_async` is the bridge to the event loop: a detached helper blocks in the same receive off thread and completes a loop future when a value or the drained close arrives, so `await`ing it yields exactly the pair `chan_recv` would return. Import `std.async.future` for the `Future<T>` it hands back, and pin the element with the binding annotation, `f: Future<int64> = chan_recv_async(c)`.

## std.concurrent.pool

The global thread pool behind the `submit` builtin. The pool is a process singleton with a fixed worker count and an unbounded queue, so a submission never blocks the submitter. Start it once, submit work with `submit`, and shut it down before `main` returns; the shutdown drains everything already queued. After a shutdown the pool stays down for the rest of the process.

| Function                            | Description                                                       |
| ------------------------------------ | ------------------------------------------------------------------- |
| `pool_start(workers: int64) -> error` | Start the pool with a fixed worker count of at least one.        |
| `pool_shutdown() -> void`           | Stop accepting submissions, run everything queued, join the workers. |
| `ncpu() -> int64`                   | The number of processors online, never below one.                 |

```text
@import std.concurrent.pool

e := pool_start(ncpu())
e.ignore()
submit render_tile(0)
submit render_tile(1)
pool_shutdown()
```

`pool_start`'s error exists when the count is below one, the pool is already running, it was already shut down, or the operating system refuses a worker thread; a refused start leaves the pool pristine, so a later attempt can succeed. `pool_shutdown` is idempotent and a no op before the pool starts, and when callers race, every one of them returns only after the drain completes. Calling it from inside a pool task is fatal by name.

## std.async.loop

The event loop's lifecycle. The loop is a process singleton like the thread pool, owned by the thread that initialized it. Futures are minted, awaited, polled, and freed on that thread only, while completion is legal from any thread; touching the loop off its thread faults by name. Initialize the loop before the first future and free it after the last completer has finished, the close, join, free discipline.

| Function                | Description                                                       |
| ------------------------ | ------------------------------------------------------------------- |
| `loop_init() -> error`  | Start the loop on the calling thread, which becomes its owner.    |
| `loop_free() -> void`   | Free the loop.                                                    |

```text
@import std.async.loop
@import std.async.time
@import std.async.future

e := loop_init()
e.ignore()
v, te := await(sleep_async(5))
te.ignore()
println(v)
loop_free()
```

`loop_init`'s error exists when the loop is already running or the operating system refuses the resources. Pending timer futures leak their records at `loop_free`, so consume or free futures first and shut the pool down before it. After the free a completer that fires anyway faults on the not running loop instead of corrupting memory.

## std.async.future

The one shot completion future over the event loop. A future is minted pending on the loop thread, completed exactly once from any thread, and consumed exactly once on the loop thread by `await`, a timed await, or a ready poll. The record lives in the generational heap, so consuming it retires it and a second consume faults by name, the double join contract. The handle is a plain pair of words and copies freely; every copy names the same future.

```text
struct Future<T> {
    h: *void,
    gen: int64,
}
```

| Function                                                              | Description                                                     |
| ----------------------------------------------------------------------- | ----------------------------------------------------------------- |
| `future_new<T>() -> Future<T>`                                        | Mint a pending future; the binding annotation pins `T`.         |
| `future_wrap<T>(h: *void) -> Future<T>`                               | Wrap a live runtime completion handle into a typed future.      |
| `complete<T>(f: Future<T>, v: T, e: error) -> error`                   | Complete with a value and an error, from any thread.            |
| `complete_raw<T>(h: *void, gen: int64, v: T, e: error) -> error`       | The same completion addressed by the two raw handle words.      |
| `await<T>(f: Future<T>) -> (T, error)`                                | Wait until the future completes, then consume it.               |
| `await_timeout<T>(f: Future<T>, ms: int64) -> (T, error)`             | Wait at most `ms` milliseconds; the future stays live on timeout. |
| `try_poll<T>(f: Future<T>) -> (T, error)`                             | Poll without parking; the future stays live while pending.      |
| `future_free<T>(f: Future<T>) -> void`                                | Release a future that will never be consumed.                   |

```text
@import std.async.future

f: Future<int64> = future_new()
h := f.h
gen := f.gen
w := spawn produce(h, gen)
v, e := await(f)
e.ignore()
println(v)
join w
```

Exactly one completion wins and a late loser is refused and dropped, whether it arrives before or after the awaiter consumes the future, so racing completers never need to outrun the awaiter. The error a completer passes is payload for the awaiter rather than a failure at the completion site. `complete_raw` is the completer surface a thread carries when the typed future belongs to the loop thread: `h` and `gen` are the struct's own fields lifted out before the spawn, so the completion crosses without capturing a future, which is rejected wherever it would cross a thread boundary.

`await_timeout` reports `await timed out` and `try_poll` reports `future is pending`, and in both cases the value is the zero pattern for `T` and the future stays live, so the caller retries, polls, or frees it. A blocking `await` with nothing left that could complete the future, no timer pending, no spawned thread alive, no pool task in flight, is a deadlock and faults by name instead of hanging. A consumed future needs no free and freeing it faults, the double free family.

The three pumping primitives, `await`, `await_timeout`, and `try_poll`, crank the loop themselves, so calling one directly inside an `async func` is a compile error; the `await` statement is the suspension inside a task. `future_new` refuses an element that could view a frame, a slice, a closure, or an interface value, the same ban a channel element carries.

## std.async.time

Timers over the event loop. A timer is an ordinary future the loop's heap completes at its deadline, measured on the monotonic clock, so awaiting one parks without holding a thread hostage the way `sleep_ms` does. Timers sharing a deadline complete in creation order.

| Function                                | Description                                                    |
| ---------------------------------------- | ---------------------------------------------------------------- |
| `sleep_async(ms: int64) -> Future<int64>` | A future that completes with 0 after `ms` milliseconds.       |

```text
@import std.async.time
@import std.async.future

async func tick() -> int64 {
    v, e := await sleep_async(50)
    e.ignore()
    return v
}
```

A non positive wait completes on the next await or poll, where all timers fire. Timers are minted on the loop thread only. Await one, poll it, or race it against another future with `await_timeout`. Outside an async func the `await` keyword is not available, so a synchronous caller waits through the plain call, `v, e := await(sleep_async(50))`, the same surface `std.async.future` exports.

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

## std.net

Added in 1.17.1. Blocking sockets: TCP over IPv4 and IPv6 and unix domain stream sockets, all addressed through one value type, `Addr`. The address type lives in its own file, `std.net.addr`, and `@import std.net`, `@import std.net.udp`, and `@import std.async.net` each bring it, so a program names `Addr` and the `addr_` functions with no import of its own.

### Addresses

`Addr` is an enum with one variant per socket family the net modules bind:

```text
export enum Addr {
    Ip4(ip: int64, port: int64),
    Ip6(hi: uint64, lo: uint64, port: int64),
    Unix(path: string),
}
```

`Ip4.ip` is the 32 bit address as one host order integer, so `127.0.0.1` is `2130706433`. `Ip6.hi` and `Ip6.lo` are the 128 bits as two big endian halves, so `::1` is `hi` 0 and `lo` 1; the IPv6 scope id is not carried. `Unix.path` is a borrowed path of at most 107 bytes. Matching an `Addr` is supported and stable: the three variants are the three families this release binds, and a fourth would arrive as a release note, not a silent addition.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `addr_ip4(a: int64, b: int64, c: int64, d: int64, port: int64) -> Addr` | `a.b.c.d:port`. An octet outside `0..255` or a port outside `0..65535` is a misuse fault. |
| `addr_ip6(hi: uint64, lo: uint64, port: int64) -> Addr`         | An IPv6 address from its two halves; the same port rule.       |
| `addr_unix(path: string) -> Addr`                               | A unix socket path, borrowed, never copied or freed.           |
| `addr_loopback(port: int64) -> Addr`                            | `127.0.0.1:port`.                                              |
| `addr_loopback6(port: int64) -> Addr`                           | `[::1]:port`.                                                  |
| `addr_parse(text: string, port: int64) -> (Addr, error)`        | A dotted quad or a bare IPv6 literal through `inet_pton`; anything else is the error `not an IP address: <text>`. |
| `addr_port(a: Addr) -> int64`                                   | The port, or 0 for a unix address.                             |
| `addr_with_port(a: Addr, port: int64) -> Addr`                  | A copy with the port replaced; a unix address comes back unchanged. |
| `addr_string(a: Addr) -> string`                                | `127.0.0.1:8080`, `[::1]:443`, or the unix path, as a fresh heap string. |
| `addr_eq(a: Addr, b: Addr) -> bool`                             | Same family and same fields; unix paths compare by bytes.      |

`addr_parse` never looks a name up. It takes a literal address written without brackets or a zone and nothing else, so `localhost` is an error here exactly as `1.2.3` is; `resolve` is the name lookup. A port outside `0..65535` is the error `port out of range`, since this call has an error channel. `addr_ip4`, `addr_ip6`, and `addr_with_port` fault instead, printing a `fatal: net:` line and aborting the way `std.flags` does, because the octets and the port they take are a programmer's constants and a bad one is a bug, not input. `addr_string` formats through `inet_ntop`, so an IPv6 address prints in its canonical compressed form, `2001:db8::1` rather than the eight groups.

```text
@import std.net

a, e := addr_parse("2001:db8::1", 443)
if e.exists() {
    return e
}
s := addr_string(a)
println(s)
free(s)

local := addr_loopback(0)
println(addr_eq(addr_with_port(local, 8080), addr_ip4(127, 0, 0, 1, 8080)))
println(addr_port(addr_unix("/tmp/app.sock")))
```

An `Addr` is a value and never owns heap memory, so it returns from a function, sits in a `Vector`, crosses a `spawn` and a channel, and enters or leaves an `async func` with nothing to free. `Addr.Unix` borrows the string it was built from, so keep that string alive while the address is in use; an `Addr.Unix` that comes back from an accept or an address query is the unnamed address with the empty string. `addr_string` is the one call here that hands back memory: free its result.

`AddrRec` and the three helpers `addr_pack`, `addr_unpack`, and `addr_path_cbuf` are the C boundary the net modules share. `AddrRec` is a 32 byte record, `family` 4, 6, or 1 for unix, a host order `port`, and 16 `ip` bytes in network order, laid out to match the runtime's own record; a unix path crosses beside it as a NUL terminated buffer. They are exported only because the sibling modules must name them, and a program never builds a record itself.

### Blocking sockets

Every function below blocks the thread that calls it, and a socket it hands back is an `int64` descriptor, the same number `fd_writer`, `fd_reader`, `Writer.Fd`, and `std.async.net` use. Each error is the OS's own `strerror` text read right after the call, `Connection refused`, `Address already in use`, `Too many open files`.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `net_dial(a: Addr) -> (int64, error)`                           | Connect a blocking, close on exec stream socket to `a`; `-1` and the OS failure when nothing listens or the unix path is missing. An interrupted connect is retried. |
| `net_listen(a: Addr, backlog: int64) -> (int64, error)`         | Bind and listen; `SO_REUSEADDR` on an IP socket, `IPV6_V6ONLY` on IPv6, `backlog` of 0 or below reads as 128, port 0 asks the OS for one. Never unlinks a unix path. |
| `net_accept(lfd: int64) -> (int64, Addr, error)`                | Block for one connection; the client descriptor and its peer address, `Addr.Unix("")` over a unix listener. On failure `-1` and `Addr.Ip4(0, 0)`. |
| `net_close(fd: int64) -> error`                                 | `close(2)`, the same call `fd_close`, `writer_close`, and `reader_close` make. |
| `net_shutdown_write(fd: int64) -> error`                        | The half close: the peer reads end of stream once it drains what was sent, and this side still reads. |
| `net_shutdown_read(fd: int64) -> error`                         | This side's reads return end of stream; the peer is not told.  |
| `net_local_addr(fd: int64) -> (Addr, error)`                    | `getsockname`; the port a listener bound to 0 was given.       |
| `net_peer_addr(fd: int64) -> (Addr, error)`                     | `getpeername`; `Transport endpoint is not connected` on a listener, an unconnected socket, or a connection both sides have shut down. |
| `net_set_nodelay(fd: int64, on: bool) -> error`                 | `TCP_NODELAY`; an error on a socket that is not TCP.           |
| `net_set_read_timeout(fd: int64, ms: int64) -> error`           | `SO_RCVTIMEO` in milliseconds; a read that waits longer is the `read_buf` error `would block`. 0 clears it. |
| `net_set_write_timeout(fd: int64, ms: int64) -> error`          | `SO_SNDTIMEO` in milliseconds; `write_buf` reports the count sent before the timeout beside `Resource temporarily unavailable`. 0 clears it. |
| `resolve(host: string, port: int64) -> (*Vector<Addr>, error)`  | Blocking `getaddrinfo`; every address for `host` with `port` set, in the resolver's order, at most 16, as a fresh vector. |

A connected socket is a stream the moment `std.io` wraps it, so there is no socket writer or reader of its own: `fd_writer(fd)` writes to it with `write_string`, `write_line`, and `write_buf`, `fd_reader(fd)` reads from it with `read_buf` and `read_to_end`, and `std.bufio` buffers either side. The server below is one thread per connection, the shape a small blocking server takes; the client writes a request, half closes, and reads the whole reply.

```text
@paradigm procedural
@import std.net
@import std.io

func serve(lfd: int64) -> void {
    cfd, peer, ae := net_accept(lfd)
    if ae.exists() {
        printerr(ae)
        return
    }
    request, re := read_to_end(fd_reader(cfd))
    re.ignore()
    we := write_string(fd_writer(cfd), request)
    we.ignore()
    free(request)
    ce := net_close(cfd)
    ce.ignore()
}

func main() -> int32 {
    lfd, le := net_listen(addr_loopback(0), 0)
    if le.exists() {
        printerr(le)
        return 1
    }
    bound, be := net_local_addr(lfd)
    be.ignore()
    t, te := spawn(lambda () -> void {
        serve(lfd)
    })
    te.ignore()
    fd, de := net_dial(bound)
    if de.exists() {
        printerr(de)
        return 1
    }
    e := write_line(fd_writer(fd), "hello")
    e.ignore()
    se := net_shutdown_write(fd)
    se.ignore()
    reply, rre := read_to_end(fd_reader(fd))
    rre.ignore()
    print(reply)
    free(reply)
    ce := net_close(fd)
    ce.ignore()
    je := join(t)
    je.ignore()
    lc := net_close(lfd)
    lc.ignore()
    return 0
}
```

The half close is how a request and reply protocol ends its request without a length prefix or a delimiter. `net_shutdown_write(fd)` sends the peer a FIN: the peer's `read_to_end` returns once it has drained what was already sent, the peer writes its reply and closes, and this side's `read_to_end` returns in turn. The descriptor is still open after the shutdown and is still closed with `net_close`; a write through it after the shutdown returns `Broken pipe`, never a fault, since the runtime ignores `SIGPIPE` process wide. `net_shutdown_read` is the other direction, this side's reads return end of stream from now on and the peer is not told; it is rarer, and a server that wants no more of a request after it has read the header uses it.

The listener is a descriptor too, so one `net_close` covers every socket a program opens, and closing a listener does not close the connections accepted from it. A unix listener leaves its socket file behind: `net_close` closes the descriptor and never unlinks, and `net_listen` over a path that exists fails with `Address already in use`, so remove a stale file with `remove_file` before the listen and remove the one this run created after the close.

```text
@import std.net
@import std.fs

path: string = "/tmp/app.sock"
stale := remove_file(path)
stale.ignore()
lfd, le := net_listen(addr_unix(path), 0)
...
lc := net_close(lfd)
lc.ignore()
gone := remove_file(path)
gone.ignore()
```

### Ownership and lifetime

An `Addr` is a value and never owns heap memory. `Addr.Unix` borrows the path string it was built from, so keep that string alive while the address is in use; an `Addr.Unix` that comes back from `net_accept`, `net_local_addr`, or `net_peer_addr` is the unnamed address with the empty string. `addr_string` returns a fresh heap string you free.

`resolve` returns a vector you own: `vec_free(v)` then `free(v)`. The addresses inside are plain values, so copying one out before the free is fine, and reading the vector after the free is the runtime's `use of a freed or stale pointer` fault.

Whoever opened a descriptor closes it, once. `net_dial`, `net_listen`, `net_accept`, and `udp_bind` each hand you a descriptor; `net_close` and `fd_close` are `close(2)` on any descriptor, and `writer_close` and `reader_close` are the same call for a descriptor above 2 (they flush and keep the standard three open), so a socket wrapped in both a `Writer` and a `Reader` is closed through one of them, not both, and a socket that landed on descriptor 0 to 2 because the program closed its standard stream is closed through `net_close`. Closing a listener does not close the connections accepted from it. Closing a listener does not wake a thread blocked in `net_accept` on it either: the blocked call keeps the old socket, the descriptor number is reused by the next open, and a `join` on that thread waits forever. To stop a blocking server, call `net_shutdown_read(lfd)` from the other thread, which makes the blocked `net_accept` return `Invalid argument`, then `join`, then `net_close`.

Half close: `net_shutdown_write(fd)` tells the peer you are done sending, its `read_to_end` returns, and you still read the reply. A `Writer` over a socket shut down for writing, or over a socket the peer closed, returns `Broken pipe` from the next write and never faults; `SIGPIPE` is ignored process wide.

A `Writer` or `Reader` over a closed socket returns an error, `Bad file descriptor`, never a fault, unless a later open has reused the number, in which case the bytes land in that stream; close once and drop every copy.

A unix listener leaves its socket file behind; `net_close` closes the descriptor and never unlinks, so `remove_file(path)` after the close, and before a `net_listen` that reuses the path.

A server thread is a `spawn` that captures the listener fd and joins at the end, the shape above; a datagram or stream socket crosses a `spawn` as its `int64`, and an `Addr` crosses as a value.

Blocking only: every function in `std.net` and `std.net.udp` blocks the calling thread, and calling one inside an `async func` stalls every task on the loop; the checker cannot see it. `std.async.net` is the surface for the loop, and `resolve_async` is the loop's name lookup.

### Name resolution

`resolve(host, port)` is blocking `getaddrinfo(host, NULL, AF_UNSPEC, SOCK_STREAM)` with no flags, so a literal address returns itself, `localhost` returns what `/etc/hosts` says in its order, `::1` first on a box that lists it first, and a name goes to the resolver the OS configured. Every entry is `Ip4` or `Ip6` with `port` set, in the resolver's order, at most 16. On failure the vector is empty and the error carries the resolver's own sentence, `Name or service not known` for an unknown host among others, so test `exists` rather than matching the text.

```text
@import std.net
@import std.vector

v, e := resolve("localhost", 80)
if e.exists() {
    return e
}
fd, de := net_dial(vec_get(v, 0))
vec_free(v)
free(v)
if de.exists() {
    return de
}
```

A lookup against `localhost` answers in microseconds; a real name against a slow resolver takes seconds, and during that time the calling thread does nothing else. Inside an `async func` that thread is the loop, so every timer, accept, and read waits with it, and the checker cannot see the stall. An async program has three ways out, named in the order to reach for them: call `resolve` before the loop starts, right for a client with one host and free; call `std.async.net`'s `resolve_async(host, port)`, which resolves the first address on a helper thread the runtime owns while the loop keeps cranking, right for a server that resolves per request; or, when the loop needs every address and not just the first, `spawn` a thread that calls `resolve` and hands the vector back through a channel read with `chan_recv_async`.

## std.net.udp

Added in 1.17.1. Blocking datagram sockets over IPv4 and IPv6, addressed through the same `Addr` as `std.net`. The module imports `std.net`, so `@import std.net.udp` brings the address functions and the socket calls that apply to a datagram socket too: `net_close`, `net_local_addr`, `net_set_read_timeout`, and `net_set_write_timeout`. Every call here blocks the calling thread; `std.async.net`'s `udp_send_async` and `udp_recv_async` are the loop's forms, over a socket from `udp_bind` set nonblocking with `fd_nonblock`.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `udp_bind(a: Addr) -> (int64, error)`                           | Open a blocking, close on exec datagram socket of `a`'s family and bind it; `IPV6_V6ONLY` on IPv6, port 0 asks the OS for one. A unix address is the error `udp_bind takes an IP address` before any syscall. On failure `-1`. |
| `udp_send_to(fd: int64, buf: *void, n: int64, to: Addr) -> (int64, error)` | One `sendto` of the `n` bytes at `buf` to `to`; a datagram goes whole or not at all, so the count is `n`, or 0 with the OS failure: `Message too long` past the datagram limit, `Resource temporarily unavailable` on a full nonblocking buffer. An interrupted send is retried. |
| `udp_recv_from(fd: int64, buf: *void, cap: int64) -> (int64, Addr, error)` | One `recvfrom` of at most `cap` bytes into `buf`, the sender's address beside the count. A longer datagram is truncated to `cap` silently. A receive timeout or a nonblocking socket with nothing queued is the error `would block`, the `read_buf` spelling. On failure 0 and `Addr.Ip4(0, 0)`. |

A datagram socket is a descriptor like a stream socket, and the two sockets below trade one datagram each way in a single thread: both bind port 0 on loopback, each reads its port back with `net_local_addr`, and the receiver answers to the address `udp_recv_from` handed it. A client that only sends and awaits replies binds `addr_loopback(0)` or `addr_ip4(0, 0, 0, 0, 0)`.

```text
@import std.net.udp
@import std.string

a, ae := udp_bind(addr_loopback(0))
if ae.exists() {
    return ae
}
b, be := udp_bind(addr_loopback(0))
if be.exists() {
    return be
}
b_addr, le := net_local_addr(b)
le.ignore()

ping: *raw char = cbuf("ping")
sent, se := udp_send_to(a, ping, 4, b_addr)
free(ping)
if se.exists() {
    return se
}

buf: *raw char = alloc_bytes(512)
n, from, re := udp_recv_from(b, buf, 512)
if re.exists() {
    free(buf)
    return re
}
back, pe := udp_send_to(b, buf, n, from)
pe.ignore()
free(buf)

c1 := net_close(a)
c1.ignore()
c2 := net_close(b)
c2.ignore()
```

`udp_recv_from` fills `buf` up to `cap` and drops whatever did not fit, with no error and no second read that recovers the tail, so size the buffer for the largest datagram you expect; 65507 bytes is the most an IPv4 datagram carries. `net_set_read_timeout(fd, ms)` bounds the wait, and a receive that times out is the error `would block`, so a server loop that polls a socket between other work tests that text, or `exists` alone when any failure ends the loop.

Unix datagram sockets and connected UDP are not in this release: `udp_bind` refuses an `Addr.Unix` before any syscall, and `udp_send_to` over a unix address is `Address family not supported by protocol` from the OS. A datagram socket is closed with `net_close`, once, like any other descriptor, and it crosses a `spawn` as its `int64`.

## std.url

Added in 1.17.2. A parsed URL and the percent coding of RFC 3986. A URL is not an HTTP idea, so the module carries no socket and imports only `std.string`; the http client is its first consumer, and any program that reads the same grammar, a redirect target among them, uses it the same way.

```text
export struct Url {
    scheme: string,
    host: string,
    port: int64,
    path: string,
    query: string,
    fragment: string,
}
```

Every string field is a fresh heap copy the `Url` owns, and an absent part is the empty heap string, so you read any field without testing it first. `scheme` is lowercased. `host` keeps its case and is the bare host, an IPv6 literal without its brackets. `port` is the explicit port, or the scheme's default, 80 for `http` and 443 for `https`, or 0 for a scheme with no default. `path` is the path as written with its percent encoding untouched, empty when the URL has none. `query` and `fragment` are the text after `?` and `#` without the marks.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `url_parse(text: string) -> (*Url, error)`                      | Parses `scheme://host[:port][/path][?query][#fragment]` into a fresh `Url`. |
| `url_free(u: *Url) -> void`                                     | Frees the five strings; free the block with `free` after.      |
| `url_default_port(scheme: string) -> int64`                     | 80 for `http`, 443 for `https`, 0 otherwise, ASCII case folded. |
| `url_host_header(u: *Url) -> string`                            | The `Host` field value: the host, in brackets when it holds a `:`, then `:port` when the port is not the scheme's default. |
| `url_target(u: *Url) -> string`                                 | The request target in origin form: the path, `/` when it is empty, then `?` and the query when there is one; never the fragment. |
| `url_encode(s: string) -> string`                               | Every byte outside the unreserved set, letters, digits, `-`, `.`, `_`, and `~`, as `%XX` in uppercase hex. |
| `url_decode(s: string) -> (string, error)`                      | Every `%XX` back to its byte; `+` is left alone.               |

The scheme is an ASCII letter followed by letters, digits, `+`, `-`, and `.`. The query runs from the first `?` after the authority to the first `#` after it, and the fragment from that `#` to the end, so a `?` inside a query or a `#` inside a fragment is kept as text. An empty port after the colon, `http://h:/`, reads as the scheme's default, the way RFC 3986 lets it. `url_parse` fails with one of a closed set of sentences, each one a value a program may match:

| Error                                    | Cause                                                          |
| ---------------------------------------- | -------------------------------------------------------------- |
| `url: control character`                 | a byte below 0x21, a space or a control character, anywhere in the text |
| `url: no scheme`                         | the text does not open with a scheme and `://`                 |
| `url: user information is not supported` | an `@` in the authority                                        |
| `url: empty host`                        | nothing between `://` and the port, path, query, or fragment, or `[]` |
| `url: bad host`                          | an unclosed `[`, a byte after `]` other than `:`, a bracket inside a plain host, or a bracketed host that is not hex digits, colons, and dots with at least one colon |
| `url: bad port`                          | a port byte that is not a digit, or a port above 65535         |

User information is refused rather than dropped: RFC 9110 deprecates it in an `http` URL, and silently discarding a credential is the worse surprise. Brackets hold an IPv6 literal and nothing else, so `http://[127.0.0.1]/` and `http://[localhost]/` are `url: bad host` rather than a host whose brackets `url_host_header` would drop. On failure the `Url` beside the error is real, every field the empty heap string and the port 0, so every call pairs with `url_free` and `free` on either path.

```text
@import std.url

u, e := url_parse("http://[::1]:8080/a/b?x=1#top")
if e.exists() {
    println(e)
} else {
    host := url_host_header(u)
    target := url_target(u)
    println(host)                          // [::1]:8080
    println(target)                        // /a/b?x=1
    free(host)
    free(target)
}
url_free(u)
free(u)
```

The path and the query keep their percent encoding exactly as written, because a decoded path would encode back differently and a request target goes out as the program wrote it. `url_decode` undoes the encoding on request. A `%` not followed by two hex digits is `url: bad percent escape`, and so is `%00`, since a decoded NUL would end the string early and drop what followed it with no sign. `url_decode` never turns `+` into a space: that is the form encoding rule, which belongs to a form parser. Both coding functions return a fresh heap string you free, and the one `url_decode` hands back beside an error is the empty heap string, freed the same way.

```text
q := url_encode("a b/c~d%")
println(q)                                 // a%20b%2Fc~d%25
back, de := url_decode(q)
if de.exists() {
    println(de)
} else {
    println(back)                          // a b/c~d%
}
free(back)
free(q)
```

`url_host_header`, `url_target`, and `url_encode` each return a fresh heap string, and `url_default_port` and every field read allocate nothing. The parser is byte wise and bounds every index by the measured length of the text, so a `Url` never reads past its input whatever the bytes are.

## std.net.http

Added in 1.17.2. An HTTP/1.1 client and a small server in pure dusk over the blocking sockets of `std.net` and the stream layer of `std.io` and `std.bufio`, with no change to the compiler or the runtime. The wire is RFC 9110 and RFC 9112: a request line and a status line, header fields, `Content-Length` bodies, chunked transfer coding in both directions, `Connection: close` and keep alive, `Host` required on a request, and a 1.0 response tolerated on read.

The layer is four modules. The wire imports the message types, the client and the server each import the wire, and the client alone imports `std.url` and dials. Imports are transitive, so a program names the highest one it needs:

| Module                 | Contents                                                       |
| ---------------------- | -------------------------------------------------------------- |
| `std.net.http`         | The message types, `Header`, `Request`, and `Response`, the header list functions, and `status_text`. |
| `std.net.http.wire`    | The parsers, the framing rule, the chunked coding, and the head writers, over a `*BufReader` and a `Writer` and never a descriptor. |
| `std.net.http.client`  | A `Conn` over one connection, `http_get`, `http_post`, and `http_send`. |
| `std.net.http.server`  | A sequential accept loop with a typed context and a named handler. |

`@import std.net.http.client` brings `Url`, the message types, the header functions, and `std.net`'s `Addr` with it, and `@import std.net.http.server` brings the same set plus the server. A program that only builds and reads messages imports `std.net.http`. The client and the server never import each other, so a client program links no accept loop and a server program links no resolver call.

### Messages and headers

```text
export struct Header { name: string, value: string }

export struct Request {
    method: string,
    path: string,
    query: string,
    minor: int64,
    headers: *Vector<Header>,
    body: *raw char,
    body_len: int64,
    chunked: bool,
}

export struct Response {
    status: int64,
    reason: string,
    minor: int64,
    headers: *Vector<Header>,
    body: *raw char,
    body_len: int64,
    chunked: bool,
}
```

`method`, `path`, `query`, `reason`, and every header name and value are fresh heap strings the message owns. A request's target is held split at its first `?`, `path` before it and `query` after it without the mark, so a target with no query has the empty `query`. `minor` is the HTTP/1.x minor version, `1` or `0`. `body` is a heap buffer of `body_len` bytes with a NUL appended after them, never null and one byte when the body is empty. `chunked` asks a writer to send the body in chunked coding, and a parser sets it to say the body arrived that way.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `headers_new() -> *Vector<Header>`                              | An empty header list on the heap.                              |
| `headers_add(h: *Vector<Header>, name: string, value: string) -> void` | Appends a copy of both strings; a repeated name is kept in order. |
| `headers_set(h: *Vector<Header>, name: string, value: string) -> void` | Removes every field named `name`, then appends one copy.       |
| `headers_get(h: *Vector<Header>, name: string) -> (string, bool)` | The value of the first field named `name`, borrowed from the list; the empty literal and `false` when absent. |
| `headers_has(h: *Vector<Header>, name: string) -> bool`         | Whether a field of that name is present.                       |
| `headers_remove(h: *Vector<Header>, name: string) -> int64`     | Removes and frees every field of that name; how many went.     |
| `headers_free(h: *Vector<Header>) -> void`                      | Frees every name and value and the backing buffer; free the block with `free` after. |
| `header_name_eq(a: string, b: string) -> bool`                  | ASCII case insensitive byte equality, no allocation.           |
| `request_new(method: string, target: string) -> *Request`       | `method` copied, `target` split at its first `?`, `minor` 1, empty headers and body, `chunked` false. |
| `request_text(r: *Request) -> string`                           | The body bytes up to the first NUL, a borrowed view.           |
| `request_set_body(r: *Request, text: string) -> void`           | Replaces the body with a copy of `text`; `body_len` is its byte count. |
| `request_set_body_bytes(r: *Request, buf: *raw char, n: int64) -> void` | Replaces the body with a copy of `n` raw bytes, a NUL among them kept. |
| `request_free(r: *Request) -> void`                             | Frees the method, path, query, headers and their list block, and body; free the block with `free` after. |
| `response_new(status: int64) -> *Response`                      | `status`, `reason` a copy of `status_text(status)`, `minor` 1, empty headers and body, `chunked` false. |
| `response_text(r: *Response) -> string`                         | `request_text`'s twin.                                         |
| `response_set_body(r: *Response, text: string) -> void`         | `request_set_body`'s twin.                                     |
| `response_set_body_bytes(r: *Response, buf: *raw char, n: int64) -> void` | `request_set_body_bytes`'s twin.                           |
| `response_free(r: *Response) -> void`                           | Frees the reason, headers and their list block, and body; free the block with `free` after. |
| `status_text(code: int64) -> string`                            | The reason phrase for a status code, a literal; the empty literal for a code it does not know. |

The header section is an ordered `*Vector<Header>` rather than a `Map`, since header fields are a list that may repeat a name, `Set-Cookie` and `Via` among them, and a field name compares case insensitively. Every lookup is a linear scan through `header_name_eq`, which folds ASCII case byte by byte and allocates nothing, cheaper than a hash over the dozen fields a message carries. `headers_get` answers with the first match, the rule RFC 9110 permits for every field but `Set-Cookie`; a program that wants every value of a repeated field walks the list with `vec_len`, `vec_get`, and `header_name_eq`. `headers_set` is the call to reach for when a field must appear exactly once, `Host` or `Content-Length`, since it removes every field of the name before it appends. `headers_set` copies the name and the value, and `headers_remove` copies the name, before either frees a field, so both take a borrow from the same list, the value `headers_get` returned or the name of a field read with `vec_get`, and `headers_set(h, "Host", v)` with `v` from `headers_get(h, "Host")` keeps the value.

```text
@import std.net.http

h := headers_new()
headers_add(h, "Set-Cookie", "a=1")
headers_add(h, "Set-Cookie", "b=2")
headers_set(h, "Content-Type", "text/plain")
first, ok := headers_get(h, "set-cookie")
if ok {
    println(first)                         // a=1
}
for i in 0..vec_len(h) {
    f := vec_get(h, i)
    if header_name_eq(f.name, "Set-Cookie") {
        println(f.value)                   // a=1, then b=2
    }
}
println(headers_remove(h, "SET-COOKIE"))   // 2
headers_free(h)
free(h)
```

A body is a raw buffer plus a length rather than a string, because a string ends at its first NUL: an image or a protobuf body carried as one would be cut there with no sign. `request_text` and `response_text` are zero copy views of the bytes up to the first NUL, the right read for a text body, while `body_len` stays the true byte count, so a binary body reads as a prefix here and arrives whole through `(*r).body` and `(*r).body_len`. `request_set_body` copies a string, `request_set_body_bytes` copies raw bytes, and each makes its copy before it frees the old body, so the new text may be a view of the current one.

```text
resp := response_new(404)
println((*resp).reason)                    // Not Found
headers_set((*resp).headers, "Content-Type", "text/plain")
response_set_body(resp, "no such page")
println(response_text(resp))               // no such page
println((*resp).body_len)                  // 12
response_free(resp)
free(resp)
```

`status_text` knows the reason phrase of every code RFC 9110 defines, `200 OK`, `404 Not Found`, `413 Content Too Large`, `431 Request Header Fields Too Large`, and the other codes the IANA registry assigns from 1xx through 5xx. A code the table does not know, `299` or the reserved `418` among them, is the empty string, which is a legal empty reason on the wire. The result is a literal, so read it or copy it and never free it; `response_new` copies it into the `reason` the response owns.

A message owns everything inside it. `request_free` and `response_free` release the strings, every header and the header list block, and the body, and the block itself is yours to `free` after, the convention `vec_free` and `bufreader_free` follow. `headers_add` and `headers_set` copy what you pass, so the strings you built stay yours to free. A header value from `headers_get` and a text view from `request_text` or `response_text` are borrows into the message, valid until that header is removed, the body is replaced, or the message is freed; copy one with `substring` to keep it, and never free it. Since 1.17.3 the checker records both as borrows, so `free(v)` on a `headers_get` value is refused at check with `cannot free a borrowed string; only its owner frees it`, and `free(request_text(r))` with the call result sentence, `cannot free a call result that may alias its arguments`. A borrow the checker cannot see, a string passed through a closure value, still reaches the runtime: the message's own free of the block is the `fatal: double free` fault while the block stays parked, and once another allocation has taken the block in between, that free releases the other allocation's memory without a fault, since a string carries no generation. A header list you made with `headers_new` and never handed to a message is released with `headers_free` then `free`. Reading a message after its free is the runtime's `use of a freed or stale pointer` fault.

### The wire

`std.net.http.wire` is the HTTP/1.1 wire of RFC 9112: the capped line read, the request line, status line, and header field parsers, the framing rule, the three body readers, chunked coding in both directions, and the two head writers. Every reader takes a `*BufReader` and nothing else and every writer takes a `Writer` or a head builder, so the same code reads a socket through `fd_reader`, a string through `string_reader`, and writes into a `StringBuilder` through `sb_writer`; the module never names a descriptor. The functions live in two files, the byte half in `std.net.http.wirebody`, which `std.net.http.wire` imports, so `@import std.net.http.wire` brings all of them, and the client and the server import it for you.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `read_line_capped(b: *BufReader, max: int64) -> (string, error)` | One line without its CR LF or bare LF ending, as a fresh heap string; a line of `max` bytes or more is refused. |
| `parse_request_line(line: string) -> (*Request, error)`         | `method SP target SP HTTP/1.minor` into a fresh `Request` with empty headers and body. |
| `parse_status_line(line: string) -> (*Response, error)`         | `HTTP/1.minor SP 3DIGIT [SP reason]` into a fresh `Response` with empty headers and body. |
| `parse_header_line(line: string, into: *Vector<Header>) -> error` | `name ":" OWS value OWS`, appended to `into`.                 |
| `read_headers(b: *BufReader, into: *Vector<Header>, max_bytes: int64, max_count: int64) -> error` | Header field lines until the empty line, each parsed into `into`. |
| `body_framing(h: *Vector<Header>, is_request: bool) -> (int64, int64, error)` | How the body ends: kind `0` none, `1` `Content-Length` with the length, `2` chunked, `3` to the close. |
| `read_body_fixed(b: *BufReader, n: int64, max: int64) -> (*raw char, error)` | Exactly `n` bytes, with a NUL after them.                      |
| `read_body_chunked(b: *BufReader, max: int64) -> (*raw char, int64, error)` | A chunked body decoded, its trailer section read and dropped; the bytes and their count. |
| `read_body_to_close(b: *BufReader, max: int64) -> (*raw char, int64, error)` | Every byte to the end of the stream; the bytes and their count. |
| `write_chunked(w: Writer, body: *raw char, n: int64, chunk: int64) -> error` | `n` bytes as chunks of at most `chunk` bytes, then the last chunk; `chunk` below 1 reads as 4096. |
| `write_message(w: Writer, head: *StringBuilder, body: *raw char, n: int64) -> error` | The head and the body staged as one buffer and written in one write. |
| `write_request_head(head: *StringBuilder, r: *Request) -> void` | Appends the request line, every header, and the empty line.    |
| `write_response_head(head: *StringBuilder, r: *Response) -> void` | Appends the status line, every header, and the empty line.    |
| `token_list_has(value: string, token: string) -> bool`          | Whether a comma separated list field holds `token`, case folded, with the whitespace around each element ignored. |
| `http_date_now() -> string`                                     | The current time as an IMF fixdate, the `Date` field value.    |

Every reader failure is one sentence from a closed set, a value a program may match and the server maps to a status. A read failure keeps the reader's own text after its prefix, so `http: read failed: Connection reset by peer` is one example of that row.

| Error                                              | Cause                                                          |
| -------------------------------------------------- | -------------------------------------------------------------- |
| `http: line too long`                              | a line reached the cap before its LF                           |
| `http: bare carriage return`                       | a CR not followed by an LF                                     |
| `http: control character`                          | a NUL byte in a line                                           |
| `http: connection closed`                          | the stream ended before the first byte of a line               |
| `http: truncated line`                             | the stream ended inside a line                                 |
| `http: timed out`                                  | the reader reported `would block`, a socket's read timeout     |
| `http: read failed: ` and the reader's text        | any other read failure                                         |
| `http: bad request line`                           | a missing, doubled, or third space, or a version that is not `HTTP/` at all |
| `http: bad method`                                 | a method byte outside the token set                            |
| `http: bad target`                                 | a target byte below 0x21 or above 0x7E, a `#`, the asterisk form on any method but `OPTIONS`, or a target of no known form |
| `http: version not supported`                      | an `HTTP/` version other than 1.0 and 1.1 on a request, or a major version other than 1 on a response |
| `http: bad status line`                            | any other malformed status line, a code below 100 or a control byte in the reason among them |
| `http: obsolete line folding`                      | a header line opening with SP or HTAB                          |
| `http: bad header name`                            | an empty name, a name byte outside the token set, or whitespace before the colon |
| `http: bad header value`                           | a control byte other than HTAB, or DEL, in a value             |
| `http: header section too large`                   | the header section passed `max_bytes`                          |
| `http: too many headers`                           | the header section passed `max_count`, or a trailer section passed 100 fields |
| `http: both Content-Length and Transfer-Encoding`  | a message carrying both framing fields                         |
| `http: bad Content-Length`                         | a length that is not 1 to 18 ASCII digits                      |
| `http: conflicting Content-Length`                 | two lengths that disagree                                      |
| `http: unsupported Transfer-Encoding`              | a coding list other than the single element `chunked`          |
| `http: body too large`                             | a body that would pass its `max`                               |
| `http: truncated body`                             | the stream ended inside a body                                 |
| `http: bad chunk size`                             | a chunk size that is not 1 to 16 hex digits, or a size line ended by a bare LF |
| `http: bad chunk terminator`                       | a chunk's data not followed by CR LF, or a trailer line ended by a bare LF |

`read_line_capped` reads one byte at a time through `bufreader_read_byte` and never past the byte that settles the line. A CR LF ending and a bare LF ending are both accepted, as RFC 9112 lets a recipient do, and neither is ever part of the line, so no parser here strips a trailing `\r`; a CR followed by anything else is refused. The bare LF tolerance belongs to the head alone, the request line, the status line, and the header fields, which every reader here takes through `read_line_capped`. The lines inside a chunked body do not get it: `read_body_chunked` requires CR LF after every chunk size line and every trailer line, so a body cannot be framed one way here and another way by a stricter reader in front. A line that reaches `max` bytes is refused before another byte is read, so the longest line accepted is `max - 1` bytes and a peer sending without end costs `max` bytes and no more. The string beside an error is a real heap string holding the bytes read before the failure, freed like a line: the empty one beside `http: timed out` says the stream went quiet before a byte arrived, the idle connection a server closes cleanly, while a non empty one says a line stalled partway.

`parse_request_line` takes a target in origin form, `/path?query`, in asterisk form, `*`, on an `OPTIONS` request and no other, or in absolute form, `http://authority/path?query`, which it reduces to the path and the query after the authority, `/` when there is no path; the authority, a user information part included, is dropped unread. Dropping it is this module's choice rather than a rule of the RFC: the `Host` field names the origin, so a handler behind a proxy that routes on the absolute target sees `Host` instead. A `#` anywhere in a target is `http: bad target`, since a fragment is never sent and a handler comparing paths would otherwise see one. `parse_status_line` reads a later `HTTP/1.x` minor version as 1, the rule RFC 9110 gives a recipient. `parse_header_line` keeps the name as written and the value with its surrounding SP and HTAB removed, and appends nothing on an error. `read_headers` charges each line its length plus 2 against `max_bytes`, the ending empty line included, and caps each line read at what the section has left, so a section, or a single field, that would pass the budget is refused after reading no more than `max_bytes` bytes. A header line has no cap of its own, only the section budget, and a line that reaches what the section has left is `http: header section too large` rather than `http: line too long`; a field past `max_count` is refused before it is parsed. The parse functions hand back an empty `Request` or `response_new(0)` beside an error, freed like any other.

`body_framing` is where a request smuggling shape would slip through, so it refuses rather than resolves. A message with both a `Content-Length` and a `Transfer-Encoding` field is refused outright. Every `Transfer-Encoding` field read together as one comma separated list must be exactly the one element `chunked`, case folded, so `gzip, chunked`, two `chunked` fields, and `chunked,` are all refused. Every `Content-Length` field, and every comma separated element inside one, must be 1 to 18 ASCII digits with no sign and no space, which keeps the value inside an `int64` with no overflow test, and all of them must name the same length. A message with neither field is a request with no body or a response that runs to the close; the caller applies the status and method rules first, since a `HEAD` response, a 1xx, a 204, and a 304 carry no body whatever the fields say.

Each body reader returns a fresh heap buffer with a NUL after the body bytes, the shape a `Request` or `Response` body holds, and beside an error a one byte empty buffer, so every call pairs with one `free`. The buffer grows only as bytes arrive, so a length a peer claims and never sends costs nothing beyond what did arrive. `read_body_fixed` refuses an `n` above `max` before reading a byte. `read_body_chunked` requires CR LF at the end of every chunk size line and trailer line, a bare LF being `http: bad chunk size` on a size line and `http: bad chunk terminator` on a trailer line, ignores chunk extensions after a `;` without checking their grammar, judges each chunk's size against what the budget has left before reading a byte of it, a size past what an `int64` holds included, caps a chunk size line at 256 bytes and a trailer line at 8192 bytes, and reads and discards at most 100 trailer fields, so a peer streaming trailer lines without end is refused after a bounded read. `read_body_to_close` refuses a body past `max` after reading at most `max + 1` bytes. The chunk size line cap is small on purpose: a chunk extension has only what is left of 256 bytes after the size, so a peer cannot pad each chunk with kilobytes the reader must consume, and a line past the cap is `http: line too long`.

`write_request_head` and `write_response_head` add no field of their own; the client and the server decide `Host`, `Content-Length`, `Transfer-Encoding`, `Date`, and `Connection` before calling them. The request target is rebuilt from the path and the query, `/` for an empty path, and the version written is always `HTTP/1.1`. A control byte other than HTAB, or DEL, anywhere in the text is written as SP, the replacement RFC 9110 names for a CR, LF, or NUL in a field, so a value a program copied from its input can never end a line early and start a header or a message of its own. `write_message` sends the head and the body as one buffer, so a small message leaves in one segment; a chunked body goes out as `write_message` with `n` 0 and then `write_chunked`. A writer failure is the writer's own error, and a writer that reports fewer bytes than it was handed with no error, which no `std.io` writer does, is `http: short write` rather than a silent success.

```text
@import std.net.http.wire

// Read a request head and a Content-Length body from any stream
func read_request(b: *BufReader) -> (*Request, error) {
    line, le := read_line_capped(b, 8192)
    if le.exists() {
        free(line)
        return (request_new("", ""), le)
    }
    req, pe := parse_request_line(line)
    free(line)
    if pe.exists() {
        return (req, pe)
    }
    he := read_headers(b, (*req).headers, 65536, 100)
    if he.exists() {
        return (req, he)
    }
    kind, n, fe := body_framing((*req).headers, true)
    if fe.exists() {
        return (req, fe)
    }
    if kind == 1 {
        body, be := read_body_fixed(b, n, 1048576)
        free((*req).body)
        (*req).body = body
        (*req).body_len = n
        if be.exists() {
            (*req).body_len = 0
            return (req, be)
        }
    }
    return (req, error {})
}

text := "POST /submit?x=1 HTTP/1.1\r\nHost: example.com\r\nContent-Length: 5\r\n\r\nhello"
c: *StringReader = alloc(string_reader_new(text))
b := bufreader_new(string_reader(c), 4096)
req, e := read_request(b)
if e.exists() {
    println(e)
} else {
    println((*req).path)                   // /submit
    println(request_text(req))             // hello
}
request_free(req)
free(req)
bufreader_free(b)
free(b)
free(c)

resp := response_new(200)
response_set_body(resp, "ok")
headers_set((*resp).headers, "Content-Length", "2")
head: *StringBuilder = alloc(sb_new())
write_response_head(head, resp)
out: *StringBuilder = alloc(sb_new())
we := write_message(sb_writer(out), head, (*resp).body, (*resp).body_len)
we.ignore()
// out now holds HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok
sb_free(out)
free(out)
sb_free(head)
free(head)
response_free(resp)
free(resp)
```

### The client

`std.net.http.client` is the HTTP/1.1 client over `std.net`'s blocking sockets. `http_dial` opens a `Conn` to the origin a `Url` names, `conn_send` writes one `Request` on it and reads the `Response` back, and the `Conn` stays open for the next request until either side asks to close. `http_get`, `http_post`, and `http_send` are the one shot forms: each dials, sends with `Connection: close`, and tears the connection down before it returns, so a program that makes one request frees one response and nothing else. Every call blocks the calling thread, bounded by the socket timeouts, and none belongs inside an `async func`. A `Conn` carries one exchange at a time, so two threads that share one take turns through a mutex or each dial their own.

```text
export struct Conn {
    fd: int64,
    r: Reader,
    w: Writer,
    rd: *BufReader,
    host: string,
    is_open: bool,
    max_body: int64,
    timeout_ms: int64,
}
```

`fd` is the connected socket, `-1` for a `Conn` that never connected or that `conn_close` closed. `r` and `w` are the two stream halves over it, and `rd` is the buffered reader over `r` that every read goes through, owned by the `Conn`. `host` is the `Host` field value for the connection, `url_host_header`'s text as a fresh heap string the `Conn` owns. `is_open` says whether another request may be sent. `max_body` is the response body ceiling, 8388608 bytes by default, and `timeout_ms` the read and write timeout on the socket, 30000 by default. Read the fields; change them through `conn_set_timeout` and `conn_set_max_body`.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `http_dial(u: *Url) -> (*Conn, error)`                          | Resolve the host, dial each address in order until one connects, set `TCP_NODELAY` and the default timeouts. `https` and any other scheme are refused before any lookup. |
| `conn_send(c: *Conn, req: *Request) -> (*Response, error)`      | Write `req` with the client's `Host` and framing fields set, read the response, and mark the connection closed when the exchange says so. |
| `conn_close(c: *Conn) -> error`                                 | Close the socket, set `fd` to `-1`, and free the buffered reader and the host; free the block with `free` after. |
| `conn_set_timeout(c: *Conn, ms: int64) -> error`                | Set both socket timeouts to `ms` milliseconds and record it; 0 clears them. |
| `conn_set_max_body(c: *Conn, n: int64) -> void`                 | Set the response body ceiling.                                 |
| `http_get(url: string) -> (*Response, error)`                   | A `GET` of `url` through `http_send`.                          |
| `http_post(url: string, content_type: string, body: string) -> (*Response, error)` | A `POST` of `body` with `Content-Type` set, through `http_send`. |
| `http_send(url: string, req: *Request) -> (*Response, error)`   | Parse `url`, dial it, send `req` to it with `Connection: close`, and close the connection. |
| `http_read_response(b: *BufReader, method: string, max_body: int64) -> (*Response, error)` | The response reader `conn_send` uses, over any buffered reader. |

Every failure is a value. The wire's sentences come back as they are, `http: timed out`, `http: body too large`, `http: both Content-Length and Transfer-Encoding` and the rest of the closed set, a lookup failure keeps the resolver's text, a dial failure is the last address's OS error, `Connection refused` when nothing listens, and a write failure is the writer's own, `Broken pipe` among them. The client adds six sentences of its own.

| Error                                                                | Cause                                                          |
| -------------------------------------------------------------------- | -------------------------------------------------------------- |
| `http: the https scheme needs TLS, which this release does not ship` | a URL with the `https` scheme; no lookup is made and no socket opened |
| `http: unsupported scheme`                                           | a URL with any scheme but `http` and `https`                   |
| `http: connection is closed`                                         | a `conn_send` on a connection already marked closed; nothing is written |
| `http: too many interim responses`                                   | more than 8 interim 1xx responses in a row before the final one |
| `http: unexpected data on the connection`                            | bytes the server sent past the last response wait in the buffered reader when a send begins; the connection is marked closed and nothing is written |
| `http: CONNECT is not supported`                                     | a request with the method `CONNECT`, matched case sensitively; nothing is written |

`conn_send` also refuses a request it could not write as one message, with the wire's own sentences and before a byte is written: a method that is not a token is `http: bad method`, a path or query byte below 0x21 or above 0x7E is `http: bad target`, so a program percent encodes such text with `url_encode` first, and a header name that is empty or not a token is `http: bad header name`. Nothing reached the socket, so the connection stays as it was, open when it was open, and the next request may go out on it; `CONNECT` is refused the same way.

Every response comes back as a fresh heap `Response`, and on an error too: beside an error it is a real `response_new(0)`, so every call pairs with `response_free` and `free` whatever happened. A `Conn` is the same, a `Conn` that never connected comes back beside a dial error, closed from the start and holding no socket, so every `http_dial` pairs with one `conn_close` and one `free`. `conn_close` is the one place the socket is closed. `conn_send` marks a connection closed when the server says `Connection: close` and leaves the socket to `conn_close`, and `conn_close` marks it closed too, so a `conn_send` that reaches it before the `free` is refused rather than reading freed memory.

`conn_send` decides the fields that frame the request before it writes the head, with `headers_set`, so sending the same request a second time repeats them rather than doubling them. `Host` is set to the connection's host when the request carries none; a `Host` the request already has is sent as it is, so a program that sends one request on to a second origin removes it first with `headers_remove`. The framing fields belong to the client outright: with `chunked` true the request goes out with `Transfer-Encoding: chunked`, any `Content-Length` removed, and the body follows the head in chunks of 4096 bytes; otherwise any `Transfer-Encoding` is removed and `Content-Length` is set from `body_len` when the body is not empty or the method is `POST`, `PUT`, or `PATCH`, which state an empty body as `Content-Length: 0`, and removed when neither holds. A whole body leaves with the head in one write. An empty query is never written, so a request built from the target `/a?` goes out as `/a`. The request is borrowed and never freed, so it keeps those fields and is yours to free after the send, the same way after `http_send`.

The response is read through `http_read_response` with the connection's `max_body`. Interim 1xx responses before the final one, `100 Continue` and `103 Early Hints` among them, are read whole and dropped, at most 8 in a row; a `101` is final, since it switches the protocol. A response to `HEAD`, a 1xx, a `204`, and a `304` carry no body whatever their fields say. Any other body is framed by `body_framing`: a `Content-Length` body is refused before a byte of it is read when it passes `max_body`, a chunked one arrives decoded with `chunked` set to say so, and a response with neither field, an HTTP/1.0 response among them, runs to the close. The status line is capped at 8192 bytes and the header section at 65536 bytes and 100 fields. `http_read_response` is exported so a program can parse a canned response from a `string_reader` the same way.

The connection is marked closed after a response that says `Connection: close`, after a request that said it, after an HTTP/1.0 response that did not say `keep-alive`, after an HTTP/1.0 response that carried `Transfer-Encoding`, which RFC 9112 has a recipient close after since a 1.0 hop may have framed it differently, after a body read to the close, after a `101`, and after every error, the write path included, so no exchange ever continues on a stream whose framing is unknown. Before it writes, `conn_send` also checks that nothing is waiting in the buffered reader: a server that answered one request with two responses leaves the second there, and the send is `http: unexpected data on the connection` with the connection marked closed, rather than the stray response handed back as the answer. `http_send` sets `Connection: close` on the request, so the server knows the connection ends with the exchange.

A kept connection the server closed while it sat idle, past its own idle timeout, fails the next send with `http: connection closed`, and the connection is marked closed; the program dials again and resends. `conn_close` closes the socket and sets `fd` to `-1`. It is called once per `Conn`: a second `conn_close` on the same `Conn` is the runtime's `use of a freed or stale pointer` fault, since the buffered reader it frees is already gone.

`http_send` takes the destination from the URL and the message from the request: the URL's path and query replace the request's, and the method, headers, and body go out as the request holds them. A `3xx` comes back as it is rather than being followed, since following needs a cap, a rule for the method a `303` rewrites, and a rule for the fields that must not cross to another origin, which are the program's to decide. `Location` may hold a path rather than a whole URL, so the worked client at the end of this section joins one to the origin it came from before it follows.

The read and write timeouts are the socket's, 30 seconds by default, and a read that waits past one is `http: timed out`. They bound each read, not the whole exchange, so a body that keeps arriving never trips them; `conn_set_timeout(c, 0)` waits forever. They start once the dial has connected, so neither the name lookup nor the connect has a timeout of its own, and a resolver or a host that never answers holds `http_dial` and the one shot calls as long as the OS waits. A program that wants a deadline across a whole exchange, the dial included, runs the call on a thread and waits for it with `chan_recv_timeout`.

```text
@import std.net.http.client

resp, e := http_get("http://127.0.0.1:8080/hello")
if e.exists() {
    println(e)
} else {
    println(f"{(*resp).status} {response_text(resp)}")
}
response_free(resp)
free(resp)

// One request on a connection of its own
u, pe := url_parse("http://127.0.0.1:8080/")
pe.ignore()
c, de := http_dial(u)
url_free(u)
free(u)
if de.exists() {
    println(de)
} else {
    req := request_new("GET", "/hello")
    reply, se := conn_send(c, req)
    if se.exists() {
        println(se)
    } else {
        println(response_text(reply))
    }
    response_free(reply)
    free(reply)
    request_free(req)
    free(req)
}
ce := conn_close(c)
ce.ignore()
free(c)
```

### The server

`std.net.http.server` is a small HTTP/1.1 server over the blocking sockets of `std.net`. The core is sequential: `http_serve` accepts a connection, serves every request on it, and only then accepts the next, and a program composes parallelism outside the call. A `Server` holds plain settings and owns nothing, so `free` alone releases one.

```text
export struct Server {
    lfd: int64,
    max_body: int64,
    max_header_bytes: int64,
    max_header_count: int64,
    max_line: int64,
    max_requests: int64,
    read_timeout_ms: int64,
    send_date: bool,
    keep_alive: bool,
}
```

`lfd` is the listener from `net_listen`, which stays the program's to stop and close; a server used only through `http_serve_stream` needs none, and `-1` says so. `max_body` is the request body ceiling, 1048576 bytes by default. `max_header_bytes`, 65536, and `max_header_count`, 100, bound the header section. `max_line`, 8192, caps the request line, so the longest line served is 8191 bytes. `max_requests`, 1000, is how many requests one connection carries: the response to the last of them says `Connection: close` and the stream ends cleanly after it, so one client cannot hold a sequential acceptor forever with a stream of quick requests, and 0 means no limit. `read_timeout_ms`, 10000, is how long each read of a request and each write of a response may wait, which bounds a connection sitting idle between requests and a peer that stops reading its response alike; 0 waits forever. `send_date`, true by default, adds a `Date` field to every response, and `keep_alive`, true by default, lets one connection carry more than one request. Write a field to change it before serving.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `http_server_new(lfd: int64) -> *Server`                        | The defaults above over a listener.                            |
| `http_serve<C>(s: *Server, ctx: C, handler: (*Request, C) -> *Response) -> error` | The accept loop: accept, serve the connection to its end, accept again, until an accept failure says the listener is stopped or gone; that error. |
| `http_serve_one<C>(s: *Server, ctx: C, handler: (*Request, C) -> *Response) -> error` | One accept and one connection served to its end; the accept's error or the connection's. |
| `http_serve_conn<C>(s: *Server, cfd: int64, ctx: C, handler: (*Request, C) -> *Response) -> error` | One accepted descriptor served to its end with `TCP_NODELAY` and both timeouts set, then closed. |
| `http_serve_stream<C>(s: *Server, r: Reader, w: Writer, ctx: C, handler: (*Request, C) -> *Response) -> (int64, error)` | The request loop over any stream; the count of requests the handler answered and the error that ended it. Closes nothing. |
| `http_read_request(b: *BufReader, s: *Server) -> (*Request, int64, error)` | One request read under the limits of `s`; the request, the status a failure maps to, and the wire error. |

The handler is a parameter of every serve function and never a field: a named top level function, or a lambda with no captures, of the shape `(*Request, C) -> *Response`. `handler(req, ctx)` borrows `req` for the length of the call and the server frees it afterwards, so nothing from it survives the return except what the handler copied. It returns a fresh `Response` built with `response_new`, which the server finishes, writes, and frees, so the handler never frees it and never keeps it. State lives behind `ctx`, the same value on every call: a scalar, or a managed pointer the program owns and frees once the serve has returned. A stateless handler passes `0`. A closure value cannot cross a `spawn`, while a named function written inside a spawned lambda is a plain reference, so the handler arrives by name and the context carries the state, and the capture rule that refuses `h := handle` captured into a spawned lambda stays in force at the one place a program would reach for it.

After the handler returns, the server writes the fields it owns. `Content-Length` comes from `body_len`; a response the handler marked `chunked` gets `Transfer-Encoding: chunked` and its body in chunks instead, when the client speaks 1.1, while a 1.0 client gets the length. A framing field the handler wrote itself is replaced, so a response never carries both. A 1xx and a 204 carry neither field and no body, and a 304 carries no body and keeps whatever length the handler set. `Date` is added when `send_date` holds and the handler set none. `Connection: close` is set when the stream ends after the response, and a 1.0 client the stream stays open for gets `Connection: keep-alive`, since it assumes a close otherwise. A `HEAD` request runs the handler like a `GET` and gets the head alone, its `Content-Length` kept. A response whose status is below 200 or above 999 is one the wire cannot carry as a final answer, a 0 left unset or an interim 1xx among them, so the server never sends it: it frees the handler's response, answers a 500 with `Connection: close` and the body `http: bad handler status` and an LF, and ends the stream with that error.

A connection stays open after a response while `keep_alive` holds, the request's `Connection` field does not list `close`, a 1.0 request asked for `keep-alive` and carries no `Transfer-Encoding`, the rule RFC 9112 gives for a message a 1.0 hop may have framed differently, the request is not number `max_requests` on the connection, and the handler's response does not say `Connection: close` itself. Requests on a kept connection are read in order through one buffer, so a pipelining client is answered in turn. One empty line before a request line is skipped, the tolerance RFC 9112 asks of a server for the stray CR LF some clients send after a body, and no more than one. A 1.1 request with `Expect: 100-continue` and a body still to come gets `HTTP/1.1 100 Continue` before a byte of the body is read; a `Content-Length` past `max_body` is refused with 413 first, so a client told to go on is never refused for the size it announced, and a 1.0 expectation is ignored. `http_read_request` writes nothing, so it never answers one.

A request that fails is refused by the server itself. Each wire error maps to a status:

| Status                                             | Wire error                                                     |
| -------------------------------------------------- | -------------------------------------------------------------- |
| `400`                                              | every sentence not below, among them `http: bad request line`, `http: bad header name`, both framings, the two `Content-Length` errors, the chunk errors, `http: missing Host` on a 1.1 request without `Host`, `http: duplicate Host` on any request with two, and `http: bad Host` on a `Host` value that is not host or host:port |
| `408`                                              | `http: timed out` after the first byte of a request            |
| `413`                                              | `http: body too large`                                         |
| `414`                                              | `http: line too long` on the request line                      |
| `431`                                              | `http: header section too large`, `http: too many headers`     |
| `501`                                              | `http: unsupported Transfer-Encoding`                          |
| `505`                                              | `http: version not supported`                                  |
| none                                               | `http: connection closed` or `http: timed out` before the first byte of a request, and `http: truncated body`, a peer that left inside the body |

The `Host` value of a request is read the way a handler that routes on it would read it, so it must be a host or a host and a port: no SP, HTAB, `@`, or `/` anywhere, a bracketed IPv6 literal of hex digits, colons, and dots that closes its bracket, no bracket in a plain host, and a port of digits alone after the colon. The empty value RFC 9112 names for a target with no authority passes. The rule holds on a 1.0 request that sends a `Host` too, since the handler reads it the same way.

A refusal is the status, a `text/plain` body of the wire sentence and an LF, the fields above with `Connection: close`, and the end of the stream. Nothing past the byte that failed is parsed, so a second request hidden behind a bad frame, a body that carries both `Content-Length` and `Transfer-Encoding` among them, is never seen and never reaches the handler. `http_serve_stream` returns the wire error, `http_serve_conn` and `http_serve_one` pass it up, and `http_serve` drops it and accepts the next connection, since one client's garbage ends its own connection and never the server. The stream closing, or going quiet for `read_timeout_ms`, before the first byte of a request is no failure at all: an idle kept alive connection is closed with no response and an empty error, while a request that stalls partway is a 408. `http_read_request` answers 0 for the cases that owe no response. A `Content-Length` past `max_body` is refused before a byte of the body is read and the connection closed, so a client still writing a large upload may see its own write fail, `Broken pipe` or `Connection reset by peer`, before it ever reads the 413.

An accept failure ends `http_serve` only when it says the listener can never accept again, and `http_serve` returns it: `Invalid argument`, the stop or a socket that is not listening, `Bad file descriptor`, a listener already closed, and `Socket operation on non-socket` or `Operation not supported`, a descriptor that is no stream socket. Every other failure passes, `Too many open files` and the rest of a descriptor or buffer shortage, or a connection the peer aborted before the accept took it: the loop waits 50 milliseconds and accepts again, so a burst of clients that exhausts the descriptor limit slows the server and never stops it. `http_serve_one` returns the accept's error whatever it is.

The stop is the `std.net` recipe unchanged: `net_shutdown_read(lfd)` from another thread makes the pending accept fail with `Invalid argument`, `http_serve` returns that error, then `join` the thread, then `net_close` the listener. A connection being served when the stop lands finishes first, bounded by `read_timeout_ms`; `http_serve_one` needs no stop at all. Two shapes add parallelism. N threads each in `http_serve` over the same listener serve N clients at once, the kernel waking one blocked accept per connection, with any state behind `ctx` made thread safe. A thread per connection is the program's own accept loop spawning `http_serve_conn(s, cfd, ctx, handle)` per client, which captures `s` and `cfd` and names `handle`. Either way a slow client holds only its own connection, and the timeout bounds each read and each write rather than a whole request, so a client that trickles one byte inside every timeout is never cut off.

```text
@import std.net.http.server
@import std.concurrent.thread

struct Hits {
    n: int64,
}

func hello(req: *Request, hits: *Hits) -> *Response {
    (*hits).n = (*hits).n + 1
    resp := response_new(200)
    headers_set((*resp).headers, "Content-Type", "text/plain")
    response_set_body(resp, "hello")
    return resp
}

func main() -> int32 {
    lfd, le := net_listen(addr_loopback(8080), 0)
    if le.exists() {
        printerr(le)
        return 1
    }
    s := http_server_new(lfd)
    hits: *Hits = alloc(Hits { n: 0 })
    t, te := spawn(lambda () -> void {
        e := http_serve(s, hits, hello)
        if e.exists() {
            println(e)                     // Invalid argument, once stopped
        }
    })
    if te.exists() {
        printerr(te)
        return 1
    }
    sleep_ms(60000)                        // serve for a minute
    se := net_shutdown_read(lfd)
    se.ignore()
    je := join(t)
    je.ignore()
    lc := net_close(lfd)
    lc.ignore()
    println((*hits).n)
    free(hits)
    free(s)
    return 0
}
```

`http_serve_stream` runs the same loop over any `Reader` and `Writer`, so a test drives a server with no socket at all, and `http_serve_conn` is the one function that turns a descriptor into a stream and the one that closes it, the place a later TLS stream wraps the accepted descriptor and calls the same loop.

```text
func echo_path(req: *Request, ctx: int64) -> *Response {
    resp := response_new(200)
    response_set_body(resp, (*req).path)
    return resp
}

text := "GET /a HTTP/1.1\r\nHost: h\r\n\r\nGET /b HTTP/1.1\r\nHost: h\r\n\r\n"
c: *StringReader = alloc(string_reader_new(text))
out: *StringBuilder = alloc(sb_new())
srv := http_server_new(-1)
(*srv).send_date = false
count, e := http_serve_stream(srv, string_reader(c), sb_writer(out), 0, echo_path)
println(count)                             // 2
println(e.exists())                        // false, the string ended between requests
sb_free(out)
free(out)
free(c)
free(srv)
```

### Ownership and lifetime

The rules are the ones the rest of the standard library follows. `free` on a parameter is refused, so every `_free` function releases the parts of a value and the block itself is yours to `free` after, and the checker tracks a pointer you bound but never a `string`. Each value this layer hands you has one owner and one way out:

| Value                                                            | Released by                                                    |
| ---------------------------------------------------------------- | -------------------------------------------------------------- |
| a `Url` from `url_parse`                                         | `url_free(u)` then `free(u)`, on the error path too            |
| a `Request` from `request_new`, `parse_request_line`, or `http_read_request` | `request_free(r)` then `free(r)`                       |
| a `Response` from `response_new`, `http_get`, `http_post`, `http_send`, `conn_send`, `http_read_response`, or `parse_status_line` | `response_free(resp)` then `free(resp)`, on the error path too |
| a `Conn` from `http_dial`                                        | `conn_close(c)` then `free(c)`, once per dial, on the error path too |
| a header list from `headers_new` that no message holds          | `headers_free(h)` then `free(h)`                               |
| a string from `url_host_header`, `url_target`, `url_encode`, `url_decode`, `read_line_capped`, or `http_date_now`, and a body buffer from a body reader | `free`                                      |
| a `Server`                                                       | `free(s)`, since it owns nothing                               |
| the listener a `Server` names                                    | `net_close` after the stop, the program's own                  |

A pointer beside an error is real. A call that fails still returns a `Response`, a `Url`, a `Request`, or a `Conn` you release the same way, so no error path needs a branch around the free. A `Conn` from a failed `http_dial` is closed from the start and holds no socket, and it still takes its one `conn_close` and its `free`, whether the dial failed or not, because `conn_close` is where the buffered reader and the host string are freed. One is exactly one: a second `conn_close` on the same `Conn` frees the buffered reader again and is the runtime's `use of a freed or stale pointer` fault. `conn_send` and `conn_close` mark the connection closed, so a `conn_send` that follows either is `http: connection is closed` rather than a read through freed memory.

A request you built is yours through every send. `conn_send` and `http_send` borrow it, set the client's own `Host` and framing fields on it with `headers_set`, and never free it, so one request may go out again on the same connection or another.

Every borrow comes with a lifetime. A value from `headers_get`, a view from `request_text` or `response_text`, and the `body` pointer of a message stay valid until that header is removed, the body is replaced, or the message is freed; `substring` makes a copy that outlives them. The checker records a `headers_get` value and a text view as borrows, so `free(loc)` on a `Location` value from `headers_get` is refused at check, `cannot free a borrowed string; only its owner frees it`, where through 1.17.2 it passed and the message's own free of that block afterwards was the runtime's `fatal: double free`. The same holds for the request your handler is handed: it is a borrow for the length of the call, the server frees it after the return, and anything the handler wants to keep is copied before it returns.

The response a handler returns belongs to the server from the return on, which writes it and frees it, so the handler neither frees it nor keeps it. A handler never returns a response it also holds anywhere else. The context is yours: it is passed by value on every call, a managed pointer in it is a borrow inside the handler, and the owner outside `http_serve` frees it once the serve has returned and every thread that served has been joined.

### A worked client

A complete program against the server below: a one shot `GET` that follows up to five redirects, a `POST`, and two requests on one kept alive connection. Every response is freed whatever the call returned, the connection takes its one `conn_close`, and the redirect loop copies the next URL out of the response before it frees the response the `Location` value was borrowed from.

```text
@paradigm procedural
@import std.net.http.client
@import std.fmt

// The absolute URL a Location value names, joined to the URL it came from when it is a path
func follow_target(from: string, loc: string) -> string {
    if !starts_with(loc, "/") {
        return substring(loc, 0, str_len(loc))
    }
    u, e := url_parse(from)
    e.ignore()
    host := url_host_header(u)
    out := format("%s://%s%s", [arg_str((*u).scheme), arg_str(host), arg_str(loc)])
    free(host)
    url_free(u)
    free(u)
    return out
}

// A GET that follows up to hops redirects and returns the last response
func get_following(url: string, hops: int64) -> (*Response, error) {
    resp, e := http_get(url)
    if e.exists() || hops == 0 || (*resp).status / 100 != 3 {
        return (resp, e)
    }
    loc, ok := headers_get((*resp).headers, "Location")
    if !ok {
        return (resp, e)
    }
    // The Location value is a borrow of resp, so the next URL is built before resp is freed
    next := follow_target(url, loc)
    response_free(resp)
    free(resp)
    out, oe := get_following(next, hops - 1)
    free(next)
    return (out, oe)
}

// Two requests on one kept alive connection, each response freed whatever happened
func keep_alive(base: string) -> error {
    u, pe := url_parse(base)
    if pe.exists() {
        url_free(u)
        free(u)
        return pe
    }
    c, de := http_dial(u)
    url_free(u)
    free(u)
    if de.exists() {
        // A failed dial still hands back a Conn, closed and holding no socket, to close and free
        dc := conn_close(c)
        dc.ignore()
        free(c)
        return de
    }
    mut result: error = error {}
    for path in ["/hello", "/count"] {
        req := request_new("GET", path)
        resp, e := conn_send(c, req)
        if e.exists() {
            result = e
        } else {
            println(f"{path} -> {(*resp).status} {response_text(resp)}")
        }
        response_free(resp)
        free(resp)
        request_free(req)
        free(req)
    }
    // Every http_dial pairs with one conn_close, once the requests are done
    ce := conn_close(c)
    ce.ignore()
    free(c)
    return result
}

func main() -> int32 {
    resp, e := get_following("http://127.0.0.1:8080/old", 5)
    if e.exists() {
        println(e)
    } else {
        println(f"{(*resp).status} {(*resp).reason}")
        server, has := headers_get((*resp).headers, "Content-Type")
        if has {
            println(server)
        }
        println(response_text(resp))
    }
    response_free(resp)
    free(resp)

    posted, pe := http_post("http://127.0.0.1:8080/echo", "text/plain", "ping")
    if pe.exists() {
        println(pe)
    } else {
        println(response_text(posted))
    }
    response_free(posted)
    free(posted)

    ke := keep_alive("http://127.0.0.1:8080/")
    if ke.exists() {
        println(ke)
        return 1
    }
    return 0
}
```

`follow_target` joins a path `Location` to the scheme and the `Host` value of the URL it came from, which is the whole of what the loop needs; a program that follows redirects across origins decides there which request fields travel with it, `Authorization` first, and whether a `303` turns a `POST` into a `GET`, since the client leaves those rules to the program. Against the worked server below the output is

```text
200 OK
text/plain
hello
ping
/hello -> 200 hello
/count -> 200 5 requests
```

### A worked server

A complete program: a handler that routes on the method and path, a counter shared through the context, and four threads accepting on one listener. The handler is a named top level function of the shape `(*Request, C) -> *Response`, and the state is a typed context, an `AtomicInt` here, whose handle every thread copies and every copy names one counter. A stateless handler takes an `int64` it never reads and is passed `0` for the context. A lambda with no captures stands where a name would, so `http_serve_stream(s, r, w, 0, lambda (req: *Request, ctx: int64) -> *Response { ... })` is a legal call. A lambda that captures is a closure value, which a spawn refuses to carry, so a handler that will run on a thread of its own is named, and the thread's lambda names it.

```text
@paradigm procedural
@import std.net.http.server
@import std.concurrent.thread
@import std.concurrent.atomic

// The handler: a named top level function whose state arrives through the typed context
func route(req: *Request, hits: AtomicInt) -> *Response {
    n := atomic_add(hits, 1)
    is_get := (*req).method == "GET"
    if is_get && (*req).path == "/old" {
        moved := response_new(301)
        headers_set((*moved).headers, "Location", "/hello")
        return moved
    }
    if is_get && ((*req).path == "/hello" || (*req).path == "/count") {
        resp := response_new(200)
        headers_set((*resp).headers, "Content-Type", "text/plain")
        if (*req).path == "/hello" {
            response_set_body(resp, "hello")
        } else {
            response_set_body(resp, f"{n} requests")
        }
        return resp
    }
    if (*req).method == "POST" && (*req).path == "/echo" {
        echo := response_new(200)
        // The request body is a borrow, so the response takes a copy of the bytes
        response_set_body_bytes(echo, (*req).body, (*req).body_len)
        return echo
    }
    missing := response_new(404)
    response_set_body(missing, "not found")
    return missing
}

func main() -> int32 {
    lfd, le := net_listen(addr_loopback(8080), 0)
    if le.exists() {
        printerr(le)
        return 1
    }
    s := http_server_new(lfd)
    hits := atomic_new(0)
    // Four threads accept on one listener, so four clients are served at once
    workers: *Vector<thread> = alloc(vec_new())
    for i in 0..4 {
        t, te := spawn(lambda () -> void {
            e := http_serve(s, hits, route)
            if e.exists() {
                println(e)
            }
        })
        if te.exists() {
            printerr(te)
            return 1
        }
        vec_push(workers, t)
    }
    sleep_ms(60000)
    // One shutdown wakes every blocked accept, then each thread is joined and the listener closed
    se := net_shutdown_read(lfd)
    se.ignore()
    for i in 0..vec_len(workers) {
        je := join(vec_get(workers, i))
        je.ignore()
    }
    lc := net_close(lfd)
    lc.ignore()
    println(atomic_load(hits))
    vec_free(workers)
    free(workers)
    atomic_free(hits)
    free(s)
    return 0
}
```

The four `http_serve` calls block in `net_accept` on the same descriptor, and the kernel wakes one of them per connection. `net_shutdown_read(lfd)` makes every blocked accept fail with `Invalid argument`, each `http_serve` returns that error and its thread ends, the loop joins all four, and the listener is closed last. A connection being served when the stop lands finishes first, bounded by `read_timeout_ms`.

The other shape is a thread per connection, the program's own accept loop spawning `http_serve_conn` for each client. The lambda captures `s`, `cfd`, and `hits` by copy and names `route`, the shape the capture rule allows, and the loop joins every thread it spawned once the listener is gone. The loop follows the rule `http_serve` does: an accept failure that says the listener is stopped or gone ends it, and any other, `Too many open files` among them, is waited out and the accept tried again, so a burst of clients slows the server rather than stopping it. `accept_each` runs on a thread of its own and stops the way the four acceptors do, `net_shutdown_read`, then `join`, then `net_close`, and every client gets a thread, so a slow client holds that thread and never delays another. Every live connection holds a thread and a descriptor until its client leaves or times out, so a program that serves clients it does not trust caps the live connections, counting the threads it has not joined and waiting for one to finish before it accepts past the cap; without a cap a client that opens connections without end exhausts threads and descriptors alike. Keeping one handle for every connection grows without bound on a long running server, so a program that serves for long joins a connection's thread as it finishes, an arrangement it makes for itself.

```text
// The accept failures that mean the listener is gone, the rule http_serve follows
func accept_is_final(text: string) -> bool {
    if text == "Invalid argument" || text == "Bad file descriptor" {
        return true
    }
    return text == "Socket operation on non-socket" || text == "Operation not supported"
}

// A thread per connection: the program's own accept loop spawns http_serve_conn
func accept_each(s: *Server, hits: AtomicInt) -> void {
    conns: *Vector<thread> = alloc(vec_new())
    while true {
        cfd, peer, ae := net_accept((*s).lfd)
        if ae.exists() {
            if accept_is_final(ae.toString()) {
                println(ae)                // Invalid argument, once stopped
                break
            }
            // A shortage passes: wait a moment and accept again
            ae.ignore()
            sleep_ms(50)
            continue
        }
        who := addr_string(peer)
        println(who)
        free(who)
        // The lambda captures s, cfd, and hits by copy and names route
        t, te := spawn(lambda () -> void {
            ce := http_serve_conn(s, cfd, hits, route)
            ce.ignore()
        })
        if te.exists() {
            printerr(te)
            nc := net_close(cfd)
            nc.ignore()
            continue
        }
        vec_push(conns, t)
    }
    for i in 0..vec_len(conns) {
        je := join(vec_get(conns, i))
        je.ignore()
    }
    vec_free(conns)
    free(conns)
}
```

## std.async.net

TCP and UDP over the readiness reactor. Sockets are non blocking file descriptors the reactor watches, so this module is a thin layer over `std.async.io`. The connecting, accepting, sending, and receiving calls are `async func`s that await `readable` or `writable` and retry; the setup and teardown calls are synchronous. The seven `tcp_` functions from 0.4.x stay as they were, loopback IPv4 by literal and no name resolution; the five functions added in 1.17.1 take and return the `Addr` value `std.net.addr` defines, so `@import std.async.net` names `Addr` and the `addr_` functions with no import of its own, and `resolve_async` is the loop's name lookup.

| Function                                                        | Description                                                    |
| --------------------------------------------------------------- | -------------------------------------------------------------- |
| `tcp_listen(port: int64, backlog: int64) -> (int64, error)`      | Bind and listen on loopback; port 0 lets the OS assign one.    |
| `tcp_local_port(fd: int64) -> (int64, error)`                    | The ephemeral port a listener was assigned.                    |
| `async func tcp_accept(fd: int64) -> (int64, error)`             | Await a connection and return the client descriptor.           |
| `async func tcp_connect(host: string, port: int64) -> (int64, error)` | Connect to a literal IPv4 address, completing the handshake and surfacing a refusal. |
| `async func tcp_read(fd: int64, buf: *void, cap: int64) -> (int64, error)` | Await readability and read once; 0 is end of stream. |
| `async func tcp_write(fd: int64, buf: *void, n: int64) -> (int64, error)` | Write every byte, awaiting writability as needed.     |
| `tcp_close(fd: int64) -> error`                                  | Close a descriptor.                                            |
| `async func net_dial_async(a: Addr) -> (int64, error)`           | The `tcp_connect` handshake over any `Addr`: TCP on IPv4 or IPv6, or a unix stream socket. A pending connect awaits `writable` then reads `SO_ERROR`; a refusal is an error value. |
| `async func net_accept_async(lfd: int64) -> (int64, Addr, error)` | `tcp_accept` with the peer address beside the client descriptor, `Addr.Unix("")` over a unix listener. |
| `async func udp_send_async(fd: int64, buf: *void, n: int64, to: Addr) -> (int64, error)` | One datagram to `to`; a full send buffer awaits `writable` and retries. `n` on success, 0 beside the error otherwise. |
| `async func udp_recv_async(fd: int64, buf: *void, cap: int64) -> (int64, Addr, error)` | Await `readable`, then one `recvfrom`; the count and the sender's address. A datagram longer than `cap` is truncated to `cap`. |
| `async func resolve_async(host: string, port: int64) -> (Addr, error)` | The first address `getaddrinfo` returns for `host` with `port` set, resolved on a helper thread while the loop keeps cranking. |

The nine `async func` heads are shown as declared: a call to one mints a task and a future rather than running the body, so it is `await`ed inside another `async func`, and awaiting it is legal nowhere else. A server accept loop and its clients run as tasks under `async_run`.

```text
@paradigm procedural
@import std.async.net

async func serve(lfd: int64) -> int64 {
    cfd, ae := await tcp_accept(lfd)
    ae.ignore()
    buf: *raw char = alloc_bytes(64)
    n, re := await tcp_read(cfd, buf, 64)
    re.ignore()
    w, we := await tcp_write(cfd, buf, n)
    we.ignore()
    free(buf)
    ce := tcp_close(cfd)
    ce.ignore()
    return w
}
```

### Sockets over Addr

`net_dial_async` is `tcp_connect` over the address value: `cool_net_connect_nb` mints a nonblocking, close on exec stream socket of `a`'s family and starts the connect, a pending handshake parks on `writable(fd)`, and the deferred `SO_ERROR` decides the result, 0 for connected and a positive errno for a refusal carried as its `strerror` text, `Connection refused` for a closed port. An immediate refusal is `the connection was refused`, a full unix backlog included, since the kernel reports that as `EAGAIN`; a hard failure before any watch is armed carries the OS text, `Invalid argument` for an empty unix path and `File name too long` past 107 bytes; an exhausted descriptor table is `too many open files`. The descriptor it hands back is nonblocking and ready for `tcp_read` and `tcp_write`, and `tcp_close`, `net_close`, and `fd_close` are the same `close(2)`.

`net_accept_async` is `tcp_accept` with the peer: the client descriptor is nonblocking and close on exec, and the address beside it is the peer's, `Addr.Unix("")` over a unix listener since a connecting unix socket has no path of its own. The listener may come from `tcp_listen`, which is already nonblocking, or from `std.net`'s `net_listen`, which is blocking and must go through `fd_nonblock(lfd)` before the first accept; a blocking listener would otherwise hold the loop inside `accept` until a client arrives. The same rule applies to `tcp_accept` over a `net_listen` descriptor. The accept loop is the one `tcp_accept` runs: a would block parks on `readable(lfd)` and retries, a readiness error bit surfaces as an error rather than a spin, and `too many open files` is terminal, never re armed against a listener that stays ready with a connection the descriptor table cannot receive.

```text
@paradigm procedural
@import std.async.net
@import std.net

async func serve(lfd: int64) -> (int64, error) {
    cfd, peer, ae := await net_accept_async(lfd)
    if ae.exists() {
        return (-1, ae)
    }
    cc := tcp_close(cfd)
    cc.ignore()
    return (addr_port(peer), error {})
}

async func amain() -> int32 {
    lfd, le := net_listen(addr_loopback6(0), 0)
    le.ignore()
    ne := fd_nonblock(lfd)
    ne.ignore()
    bound, be := net_local_addr(lfd)
    be.ignore()
    sf := serve(lfd)
    fd, de := await net_dial_async(bound)
    de.ignore()
    ...
}
```

### Datagrams on the loop

A datagram socket for the loop comes from `std.net.udp`'s `udp_bind` followed by `fd_nonblock(fd)`; the socket stays an `int64`, so `net_local_addr`, `net_close`, and the timeouts from `std.net` apply to it unchanged. `udp_send_async` is one `sendto` per turn: a full send buffer, `EAGAIN`, parks on `writable(fd)` and retries, and any other failure carries the OS text, `Message too long` past the datagram limit. A datagram is sent whole or not at all, so the count is `n` on success and 0 beside the error. `udp_recv_async` awaits `readable(fd)`, checks the readiness error bit, then does one `recvfrom`; an `EAGAIN` after the wake, a spurious readiness, re parks instead of returning, so a receive task minted before the sender runs sits on the armed socket and wakes when the datagram lands. A datagram longer than `cap` is truncated to `cap` silently, the socket API's rule, and the sender's address comes back beside the count as a value.

```text
async func receiver(fd: int64) -> (int64, error) {
    buf: *raw char = alloc_bytes(16)
    n, from, e := await udp_recv_async(fd, buf, 16)
    free(buf)
    if e.exists() {
        return (0, e)
    }
    s := addr_string(from)
    println(s)
    free(s)
    return (n, error {})
}
```

### The loop's name lookup

`resolve_async(host, port)` is the one lookup that never stalls the loop. `std.net`'s `resolve` is blocking `getaddrinfo` on the calling thread, and inside an `async func` that thread is the loop, so every timer, accept, and read waits for the resolver with it and the checker cannot see the stall. `resolve_async` instead hands the name to a detached helper thread the runtime owns, which runs the same `getaddrinfo` and completes a future the task awaits; the helper raises the live thread gauge before it starts and drops it after the completion, so an await whose only pending work is this lookup is never declared idle. The result is the first address the resolver returned, with `port` set, in either family: `localhost` is whichever loopback `/etc/hosts` lists first, a literal returns itself. On failure the address is `Addr.Ip4(0, 0)` and the error carries the resolver's own sentence, `Name or service not known` for an unknown host, `no address found` for a name with no IP entry, so test `exists` rather than matching the text. `host` is borrowed and copied for the helper before the call returns. An async body runs when the loop first cranks its task, so `host` is read then, not at the call; keep it alive and unchanged until the await returns. And a helper is a thread the loop owns until it completes, so do not `loop_free` while a `resolve_async` may still be running: a completion into a freed loop is a fault.

One address, not the list. A future's element is a fixed size copy the helper writes, so it carries one `AddrRec` and never a vector; one address is what a client dials. A program on the loop that needs every address for a name has two ways, named in the order to reach for them: call `resolve` before the loop starts, free for a client with one host, or `spawn` a thread that calls `resolve` and hands the vector back through a channel read with `chan_recv_async`.

```text
async func client(port: int64) -> (int64, error) {
    a, ae := await resolve_async("127.0.0.1", port)
    if ae.exists() {
        return (1, ae)
    }
    fd, ce := await net_dial_async(a)
    if ce.exists() {
        return (2, ce)
    }
    ...
}
```

### Ownership on the loop

Every address that enters or leaves these functions is a value: an `Addr` never owns heap memory, so the peer from `net_accept_async`, the sender from `udp_recv_async`, and the result of `resolve_async` are kept, compared with `addr_eq`, or formatted with `addr_string` with nothing to free but `addr_string`'s text. An `Addr.Unix` passed to `net_dial_async` borrows its path only for the duration of the call. Whoever opened a descriptor closes it, once: `net_dial_async` and `net_accept_async` each hand you one, `tcp_close`, `net_close`, and `fd_close` are the same `close(2)`, and a descriptor is closed only after every watch on it has fired or been disarmed, the `std.async.io` rule. The buffers `udp_send_async`, `udp_recv_async`, `tcp_read`, and `tcp_write` take are yours, staged through `alloc_bytes` and freed after the await returns.

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
