# dusk Language Specification

## Status, the 1.4.0 surface

This is the language reference for dusk. It describes the language as of 1.8.1: the paradigm system and the type system, immutability by default with `mut` to opt in, explicit memory with `alloc`, `free`, `defer`, and pointers, a generational heap that checks every managed dereference and faults on a use after free or a double free, an opt in collected heap through `collector<T>`, errors as values with a must handle rule, threads with channels, mutexes, and a thread pool, an async line with futures, an event loop, a readiness reactor, and TCP, `do` notation over any generic monad, Unicode strings with the `rune` primitive, and a foreign boundary that now crosses in both directions, reaching variadic C functions, third party libraries, and structs by value, taking a dusk function as a C callback, and compiling a dusk module into a C library any C ABI language can link. The spec is kept current with each release, so where it describes a rule the rule is the one the compiler enforces today, not an earlier core.

### The bootstrap freeze

The surface described in this spec was frozen as of 0.5.4 for the bootstrap. The releases from 0.6.x through 0.9.4 changed the compiler, not the language, as dusk was rewritten in itself, so a program that compiled against this spec at 0.5.4 kept compiling across that whole line without a source change. Three kinds of work stayed live during the freeze: diagnostics could improve, the standard library could grow, and a soundness fix could land, since none of those change the surface a correct program relies on.

The freeze closed with the bootstrap itself: 0.9.4 reached the fixpoint, the compiler written in dusk building itself to a byte identical result three stages deep, and 1.0.0 declares that compiler canonical with no language surface change of its own. The 0.5.4 surface 1.0.0 carried forward is unchanged start to finish across the whole line that held the freeze. 1.1.0 is the first release to add to the surface since: a `char`, a `char[N]`, and a `char[]` are now printable text, a string literal initializes a `char[N]` directly, and a `for` loop and a range slice both treat a string's bytes with the same discipline described in [Strings](#strings), [Arrays and Slices](#arrays-and-slices), and the [Builtins](#builtins) chapter. 1.2.0 keeps adding to it: `&&` and `||` now short circuit instead of evaluating both operands unconditionally, `==` and `!=` on a string compare content instead of pointer identity while comparison closes on a named list of comparable types, `+` concatenates two strings, a width cast converts an integer explicitly, `break` and `continue` are new statement keywords, and a runtime fault now names the source line that raised it. Each is a genuine change to what a program can write and what it means, recorded in full in [Expressions and Operators](#expressions-and-operators), [Strings](#strings), the [Builtins](#builtins) chapter, and [Memory Management](#memory-management). 1.4.0 opens the foreign boundary further: a `foreign` block may declare a variadic C function, a `@link` directive names a library for the linker and a `@csource` directive compiles a C file in alongside the runtime, and `!=` between two floats is corrected to the unordered comparison IEEE 754 defines. 1.4.1 through 1.4.3 open it the rest of the way: a C plain struct crosses by value as a `foreign` parameter or return, a capture free lambda or a named function crosses as a C callback, and `export "C"` with `dusk build --lib` compiles a dusk module into a static archive and a generated header any C ABI language links. Recorded in full in [Foreign Functions](#foreign-functions), [Linking and C Sources](#linking-and-c-sources), and [Comparison](#comparison). 1.5.0 widens the cast: `rune`, `float32`, and `float64` join the cast builtins and a cast now crosses between the integer and float families, a float to an integer saturating rather than leaving an out of range input undefined, recorded in [Numeric Casts](#numeric-casts). 1.5.1 adds the `hash` builtin, a deterministic 64-bit hash of a hashable scalar or string, the groundwork for a generic keyed map, recorded in [Hash](#hash). 1.5.2 delivers that map: `std.map` is generic over its key, `Map<K, V>`, K any hashable type. 1.5.3 rounds out `std.string` with the everyday manipulation set, a library change with no new surface. 1.6.0 adds the block comment: `/* */` nests, evaporates before parsing, and a comment containing a newline terminates a statement the way the newline itself does, recorded in [Comments](#comments), which also writes down the line comment the language has carried since the beginning. The same release teaches `dusk ir` a second target: `ir --target=wasm32 <file>` cross-emits the module's LLVM IR for wasm32-wasip1, the form a wasi toolchain links, with the native triple staying the default everywhere else; alongside it `std.os`'s errno read is renamed `os_errno`, recorded in [The errno convention](#the-errno-convention). 1.6.1 adds the doc comment and the `doc` command, recorded in [Doc Comments](#doc-comments). 1.7.0 changes no surface a correct program relies on: it closes an escape hole where a frame view buried in a call's returned aggregate crossed the frame boundary unseen, turns a misplaced type declaration into one recovering error instead of a cascade, lets `@param` name a generic type parameter and a doc comment bind to an `impl` head, and writes down rules long enforced, the interface-as-generic-argument rejection, the top level declaration rule, and the ownership boundary that sends a reclaiming tree to the collected heap. 1.7.1 gives the compiler a machine face, `check --json` reporting diagnostics as one deterministic JSON document with file local byte spans, and span keys joining the doc model, recorded in [The Machine Face](#the-machine-face). 1.8.0 gives dusk owning removal from a container: `vec_take` and `map_take` take an element out and hand the caller the owner, the checker's borrow net around containers settles into one model across every spelling, and a program can now hand write the deep free of a heap tree, recorded in [Memory Management](#memory-management). 1.8.1 is a standard library release with no new surface: `std.flags` parses a command line as a register then parse builder, `std.set` is a set over the generic map, `std.vector` gains `vec_map` and `vec_filter`, `std.time` gains `weekday` and a strict `parse_iso8601` that inverts `format_iso8601`, and `std.json` ships the hand written deep free of a parse tree as `json_free`, the first in-tree caller of the owning takes 1.8.0 introduced, recorded in [Imports and Standard Library](#imports-and-standard-library) and [Memory Management](#memory-management).

The one exception the freeze carried was a soundness hole. A hole found during the bootstrap could force a surface change to close it, and when that happened the change was named in the changelog of the release that made it.

A shape the compiler already accepts can also be written into this spec for the first time without breaking the freeze, since a program's meaning does not change when the reference catches up to what the parser has always done. The `else if` chain is the first such note, recorded in 0.6.1: `if a { } else if b { } else { }` has always parsed as an `else` branch whose whole body is a single nested `if`, so a chain of any length is ordinary nested `if`s carrying no new node, no new keyword, and no new rule. Each `else if` condition is a bool and is checked like any `if` condition, and a chain longer than the parser's nesting ceiling is refused with the same too deep diagnostic every deep nesting meets rather than overflowing the stack.

---

## Table of Contents

1. [Core Philosophy](#core-philosophy)
2. [Source Files, Directives, Imports, Exports](#source-files-directives-imports-exports)
3. [Paradigm System](#paradigm-system)
4. [Type System](#type-system)
5. [Expressions and Operators](#expressions-and-operators)
6. [Memory Management](#memory-management)
7. [Functions](#functions)
8. [Object Oriented Concepts](#object-oriented-concepts)
9. [Functional Concepts](#functional-concepts)
10. [Error Handling](#error-handling)
11. [Threads and the Memory Model](#threads-and-the-memory-model)
12. [Builtins](#builtins)

---

## Core Philosophy

- Immutability by default. All values are immutable unless explicitly declared mutable. (I don't like mutability in languages ¯\\_(ツ)_/¯)
- Explicit over implicit. Allocations, dereferences, paradigm usage, and error handling are never hidden.
- Multiple paradigms with enforced discipline. Paradigms are opt in per file through directives. Undeclared paradigm features are compile errors in that file.
- Systems level control. Manual memory management by default. A collected heap exists, but only by explicit opt in through the `collector<T>` type; nothing is collected unless a program names `collector` itself.
- All declared variables must be used. An unused variable is a compile error. This is never suppressible.
- All errors must be handled. Ignoring an error return is a compile error.

---

## Source Files, Directives, Imports, Exports

A source file has two kinds of top of file syntax. Directives start with `@` and configure the file. Declarations define types, functions, and values, and can carry modifier keywords like `export` and `mut`. A type declaration is a top level item: an `enum`, `struct`, `interface`, or `impl` inside a function body is rejected with a single error naming the rule, and the parser skips the misplaced block so the rest of the body still checks.

### Comments

dusk has two comment forms. A line comment starts with `//` and runs to the end of the line. A block comment starts with `/*`, ends with `*/`, and nests: each `/*` inside an open block comment opens another level, and the comment ends only when every level has closed, so a block comment can comment out code that itself contains block comments. Both forms are stripped before parsing and produce no tokens.

```dusk
// a line comment
/* a block comment */
/* nests: /* inner */ still inside, now closed */
x: int64 = 1 /* a comment can sit between tokens */ + 2
```

Three rules give comments their exact meaning.

- A comment opener inside a string, char, or rune literal is text, not a comment: `"/*"` is a two byte string. Symmetrically, inside a block comment only `/*` and `*/` are significant, so a quote or a `//` inside one neither opens a string nor extends the comment.
- A block comment that contains a newline terminates a statement exactly the way the newline itself would, and a block comment that opens and closes on one line does not, so wrapping a comment around code never silently glues two statements together.
- An unterminated block comment is a lexical error reported at its opening `/*`.

Comments interleave freely with the directive prologue, and a directive inside a block comment is dead in every consumer: the loader does not load a commented `@import`, and the linker does not see a commented `@link` or `@csource`.

### Doc Comments

A block comment that opens with `/**`, with the next byte after the second star neither `*` nor `/`, is a doc comment: `/**/` is an empty plain comment, and `/***/` or a `/****` banner stays plain. In every other way it is an ordinary block comment, nesting and terminating by the same rules and producing no token, so a doc comment never changes what `check`, `build`, or `ir` does. What it adds is a binding: the compiler remembers the block and attaches it to the declaration it precedes.

```dusk
/** Scales every element in place.
 * @param v the vector to scale
 * @param k the factor applied to each element
 * @return the new element count
 */
export func scale(v: *Vector<int64>, k: int64) -> int64 {
```

Binding follows the token gap: a doc block binds to the item whose first token is the next token after it, including a modifier like `export`, and blank lines or plain comments between the doc and the item do not break the binding, since neither adds a token. A block on the same line as the item's first token binds the same way. A doc block before the directive prologue is the module's doc; in a file with no directives a leading doc block binds to the first item instead, since there is no prologue to document. The bindable targets are a function, a struct, an enum, an interface, an interface method signature, an impl method, and a monad method. A doc block anywhere else, inside a function body, before an `impl` or `foreign` or `monad` block head, or trailing at the end of the file, binds nothing and is dangling.

The body carries prose, not facts: the compiler already knows every name, type, and signature from the declaration itself, so the doc never restates them and cannot drift from them. The first paragraph is the summary and later paragraphs are the body. Three tags are recognized at the start of a line: `@param <name> <description>` describes one parameter, `@return <description>` describes the return value, and `@example` opens a verbatim block that runs to the next tag or the end of the comment: an unknown `@` word inside example code is content, while a line that starts with one of the three tag words ends the example. A leading gutter of aligned `*` columns is stripped; without one, the common indentation is.

`dusk doc <file>` renders the module as markdown: every function, struct, enum, interface, and impl in source order, each with its signature built from the declaration and its prose from the doc, an undocumented item listed with its signature alone. Prose is copied verbatim. The command reads the one file, without loading imports or type checking, so an imported name renders as written. `dusk doc --json <file>` emits the same model as JSON with a stable shape, the form a language server or a doc pipeline consumes. Both forms refuse to emit from a broken module: a dangling doc block, an unknown tag outside an example, an `@param` that names no parameter of its item or repeats one, an `@return` on a function returning `void`, and an `@param` or `@return` on anything without parameters are each reported with the doc's own source position, and the command exits nonzero with nothing on stdout. The checks live in `dusk doc` alone; `check` and `build` accept a program regardless of what its doc comments say.

### The Machine Face

Two commands emit machine readable output under a stable contract, the form a language server or an editor tool consumes. The shared rule for both: a consumer must ignore keys it does not know, which is what lets a later release add a key without breaking the contract, and the key order, indentation, and escaping are fixed, so the same input bytes always produce the same output bytes.

`dusk check --json <file>` runs the same pipeline as `check` and reports the diagnostics as data. stdout carries exactly one JSON document plus one trailing newline in every outcome, clean or broken, and the exit code mirrors the human command, 0 clean and 1 on any diagnostic, a read error on the root included, reported inside the envelope. The envelope is `{"file": <the path as given>, "ok": <bool>, "diagnostics": [...]}`, each diagnostic `{"file": <the path of the file the span lands in>, "severity": "error", "message": <the message verbatim>, "span": {...} | null}`. A span carries `lo` and `hi`, byte offsets local to the named file and clamped to UTF-8 boundaries, plus `line`, `col`, `end_line`, and `end_col`, 1 based with columns counted in Unicode scalars, `line`/`col` at `lo` and the `end` pair at the true `hi`. Byte offsets are the primitive a tool converts from; the line and column pairs serve a consumer that wants them precomputed. A diagnostic with no source position, an unresolvable import for one, carries `span: null`. `severity` is always `"error"` today; the key exists so a warning can arrive later without a schema break. Diagnostics appear in the order the human command prints them.

`dusk doc --json <file>` is described in [Doc Comments](#doc-comments); since 1.7.1 every item object carries a `span` key holding the item's name token position, `{"lo", "hi", "line", "col"}` in the same file local, scalar counted convention, the impl object carrying its `impl` keyword position, and every doc object opens with the `span` of its whole doc block, delimiters included. `dusk doc` reads a single file, so its offsets are file local by construction.

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

### Linking and C Sources

Added in 1.4.0. Two more top of file directives reach the link line a build's `clang` invocation runs, alongside a `foreign` block's own bound symbols.

```text
@link "m"
@csource "adder.c"
```

- `@link <value>` names something for the linker. A value containing a path separator, or ending in `.a`, `.o`, or `.so`, is passed to `clang` verbatim as a file argument; every other value is a bare library name, turned into a `-l` flag, so `@link "m"` becomes `-lm` and `@link "curl"` becomes `-lcurl`. There is no third form: a value is always read as a library name or a path, never spliced in as an arbitrary flag.
- `@csource "<path>"` names a C source file, resolved against the directory of the file that declares it, that `clang` compiles and links in beside the dusk runtime's own C sources. A `foreign` block in the same file, or any file, then binds against whatever that source defines.

Both directives are collected the same way `@import` is, at the top of a file before any declaration, and both fold into one module wide, deduplicated, order preserving list: a `@link` or `@csource` value already seen in an earlier file is not repeated on the command line, and first appearance, in the loader's own file walk order, wins. The clang invocation itself is a pure function of that list, every runtime source first, then each `@csource` file, then each `@link` value as a file argument or a `-l` flag, then the fixed `-pthread -lm` and the output path; it carries no flags any directive did not name, and no directive can inject one, since a value is always read as a bare library name or a bare path, never interpreted as an arbitrary argument.

```text
@csource "adder.c"

foreign "C" {
    func add(a: int64, b: int64) -> int64
}
```

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
| float32 | 4 bytes | 32 bit floating point              |
| float64 | 8 bytes | 64 bit floating point              |
| bool    | 1 byte  | true or false                      |
| char    | 1 byte  | single ASCII character             |
| rune    | 4 bytes | a Unicode scalar value             |
| string  | one ptr | built in string type (see Strings) |
| error   | builtin | built in error type (see Errors)   |

The unsigned integer type names, `uint8`, `uint16`, `uint32`, and `uint64`, and the `u` literal suffixes are reserved, not yet part of the surface. The signed widths cover it until after 1.0.0, and naming an unsigned type is rejected at check, `unsigned integers are reserved; use the signed widths`.

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
- For other types such as `float32`, use a literal suffix or an annotation.

Numeric widths never mix silently. Arithmetic, comparison, assignment, and argument passing take operands of one width, so an `int32` next to an `int64` is a compile error rather than a truncation. A bare literal adapts to the width beside it, and a literal that cannot fit its annotated width is rejected.

Literal suffixes select a non default type without an annotation.

```text
a := 3.14f32    // float32
b := 5i8        // int8
c := 200i16     // int16
```

### Strings

A string is a pointer to a NUL terminated buffer of `char`, a read only view that costs one machine word. String literals do not heap allocate, since the literal bytes live in static storage.

```text
s: string = "hello"   // a pointer to the NUL terminated bytes
```

- A string value is immutable. Indexing a string, `s[i]`, is a read and stays legal; an index assignment, `s[i] = c`, is rejected at check, `a string is immutable; build a new one with a StringBuilder`, since the bytes live in read only storage. The growable `StringBuilder` in `std.string`, added in 0.2.0, builds and concatenates strings on the heap.
- A string's length is found by scanning to the NUL, which `std.string`'s `str_len` does. The NUL keeps a string view compatible with C and the foreign interface.
- The `cstr` builtin reinterprets a NUL terminated `*char` buffer as a string at no runtime cost.

Added in 1.2.0: `==` and `!=` compare two strings by content, a byte for byte comparison of the bytes up to each string's own NUL, not by the pointer each one happens to hold. A null operand, an error's empty `message` among them, reads as the empty string rather than crashing. `<`, `<=`, `>`, and `>=` between two strings reject, since a string carries no ordering; see [Comparison](#comparison). `+` and `+=` concatenate: `a + b` allocates a fresh heap string holding both operands' bytes back to back with one NUL at the end, the same allocation `substring` already returns, so the result frees with an ordinary `free` and a `mut string` grows across repeated `+=` calls. `+` is the only arithmetic operator a string accepts; mixing a string with a non string operand on either side of any operator is still a type mismatch.

```text
a := "hi" + "!"          // a fresh heap string, "hi!"
mut s: string = "x"
s += "y"                 // s is now "xy", a fresh heap string
free(a)
free(s)
println("ab" == "ab")    // true, by content, not by address
println("ab" < "cd")     // rejected: strings compare with == and != only
```

A string's representation never changed to add Unicode support. It stays the same NUL terminated byte view it always was, UTF-8 by convention rather than by any different layout. `s[i]` reads one byte, exactly as it always has, not one character and not one scalar. Iterating scalar by scalar is a decoding walk over that byte view, not an indexing operation: `std.unicode`'s `decode_rune(s, i)` reads the bytes starting at `i` and returns the decoded scalar paired with its width in bytes, so a caller steps forward by the width it gets back.

```text
mut i: int64 = 0
while s[i] != 0 {
    r, w := decode_rune(s, i)
    // r is the scalar at this position, w is how many bytes it took
    i = i + w
}
```

A string literal is checked at compile time and rejected if it is not valid UTF-8, `string literal is not valid UTF-8`; a source file with a malformed byte sequence inside a string literal no longer compiles silently with the bad bytes replaced.

A string literal also initializes a `char[N]` directly, in `let` and assignment position, when its byte length is exactly `N`.

```text
s: char[5] = "Hello"   // ok, 5 bytes into a 5 element array, no NUL, no padding
mut m: char[3] = "abc"
m = "xyz"              // reassignment through the same rule
m[0] = 'q'             // an element write after the copy is an ordinary place store
```

No NUL is appended and no padding happens; the array holds exactly the literal's bytes. A byte count that does not match `N` is rejected and names both counts, `the string literal has 6 byte(s); the annotation says char[5]`. Only a literal converts this way; a `string` typed value never does, since the rule reads the literal's own bytes at the site that holds the expression rather than living in the general type compatibility relation, so the conversion cannot be laundered through a binding: `t := "x"; s: char[1] = t` is `'s' has a type annotation that does not match its value`. The conversion applies at a `let` binding and an assignment, a struct field or an index place among them, and nowhere else; a call argument, a return, a struct literal field, and a tuple member all keep the ordinary type mismatch a `string` and a `char[N]` otherwise have. An embedded `\u{0}` inside the literal is an ordinary byte the array keeps; the same escape inside a plain `string` literal is likewise an ordinary byte, but a `string`'s own NUL terminated reading, `str_len` among it, stops at the first zero regardless of where it came from.

A `char`, a `char[N]`, and a `char[]` print as the text they are, not as numbers. `print`, `println`, and `printerr` write a `char`'s single byte, a `char[N]`'s `N` bytes, or a `char[]`'s bytes straight to the stream, exactly as given, so `s := "hi"; println(s[0])` prints `h` and a `char[5]` holding `['H','e','l','l','o']` prints `Hello`. A format hole, `{}`, prints the same way when the value behind it is one of these three types. The bytes are written whole, not scanned for a NUL and not decoded, so an embedded NUL prints through and a multibyte UTF-8 sequence prints its glyph intact rather than one raw byte's worth of garbled text. Reading the numeric value behind a `char` is one annotated binding away, `b: int64 = c`, unchanged from before. `rune` does not follow this rule; see [Runes and Unicode](#runes-and-unicode) for why `println(rune)` still prints a codepoint number.

### Runes and Unicode

`rune` is a 4 byte primitive holding one Unicode scalar value, the codepoint alone with no encoding attached. Where `char` is one byte and stands for a single ASCII byte in a string, `rune` is wide enough to name any character in Unicode, `中`, `😀`, or an ASCII letter alike.

A rune literal is written `r'...'`: `r'a'`, `r'中'`, or with an escape, `r'\u{1F600}'`. Every ordinary char escape works inside one, and `\u{...}` besides.

`rune` and `int` interconvert both ways under the same rule a `char` and an `int` already follow: a rune assigns to or from any integer width, with a wide integer silently truncating the way it does for `char`. `rune` and `char` do not mix in either direction, a byte and a scalar are different things even though both eventually ride an integer register: assigning a `char` to a `rune`, or a `rune` to a `char`, is `type annotation that does not match its value` at the annotation and `argument N has the wrong type` at a call.

```text
c: char = 'A'
b: int64 = c          // ok, char widens to int
x := r'中'
v: int64 = x           // ok, rune widens to int
y: rune = v + 1        // ok, arithmetic happens on the int, then narrows back
// bad: y2: rune = c   // char does not assign to rune
```

A rune carries no arithmetic of its own. Adding, subtracting, or otherwise computing on codepoints happens by binding the rune to an `int64`, doing the arithmetic there, and assigning the result back to a `rune`. Comparison is allowed directly between two runes, and between a rune and an int literal, the same as `char`. `sizeof(rune)` is 4. Across the foreign function boundary a `rune` passes as a C `i32`. No user defined type may be named `rune`; the name is reserved for the primitive.

`println(rune)` prints the scalar's codepoint number, not a glyph: `println(r'中')` prints `20013`. Printing the character itself goes through `std.unicode`'s `encode_rune`, which writes the scalar's UTF-8 bytes into a buffer for display. This is the opposite of what `println` does for a `char`, a `char[N]`, or a `char[]`, which print their bytes as text directly: a `char` is one byte of a string's own text and a `rune` is a 4 byte scalar with no text form of its own, so the two types print by different rules even though both ride an integer register underneath.

A `match` pattern does not bind a rune literal, a char literal, or an int literal; the pattern grammar covers a wildcard, a bound name, and an enum variant only. Comparing a rune scrutinee against a literal is written as an `if` chain, not a `match` arm.

The `\u{...}` escape names a Unicode scalar by its hex codepoint, 1 to 6 hex digits between the braces, `\u{9}`, `\u{4E2D}`, `\u{1F600}`. It is legal inside a string literal and a rune literal, where it may name any scalar up to the Unicode maximum `0x10FFFF`, excluding the surrogate range `0xD800..0xDFFF`. Inside a char literal it is legal only for a value that fits one byte, `0x7F` and under; a wider `\u{...}` inside a char literal is rejected, `a char is one byte; this escape does not fit, use a rune literal or a string`. A malformed `\u{...}` is rejected at each of its failure points: an empty or over 6 digit body (`\u escape needs 1 to 6 hex digits`), a missing closing brace (`unterminated \u escape; expected '}'`), a surrogate value (`\u escape is a surrogate code point, not a scalar value`), and a value above the maximum (`\u escape is above 0x10FFFF, the Unicode maximum`).

`std.unicode` carries the decode and encode layer over the byte view: `decode_rune`, `encode_rune`, `rune_len`, `rune_count`, `utf8_valid`, and `sb_push_rune`. See the standard library reference for the full signatures. Case folding, normalization, and grapheme clustering sit outside this layer and are not part of it.

### Arrays and Slices

Two aggregate forms hold a sequence of a single element type `T`.

- Fixed array `T[N]`. `N` elements stored inline. The size is known at compile time. Stack allocated like any value, passed by value as a copy.
- Slice `T[]`. A fat pointer `{ ptr: *T, len: int64 }` that views a contiguous run of elements without owning them. A `string` is conceptually a `char[]`, though it keeps the leaner one word, NUL terminated form the Strings section describes rather than this two word shape.

```text
xs: int32[4] = [1, 2, 3, 4]   // fixed array, 16 bytes inline
s:  int32[]  = xs[1..3]       // slice viewing xs[1], xs[2], length 2
argv: string[]                // slice of strings, as passed to main
```

- Slice length is always known. No scanning, no null terminator.
- Every array and slice index is bounds checked and traps when it misses, negatives included, naming its own file and line since 1.2.0; see [Runtime Faults](#runtime-faults).
- A range slice validates `lo <= hi <= len` against its base, so a slice can never claim a length past its backing. A `string`'s own range slice, `s[lo..hi]`, validates the same way against its length as read by a NUL scan, so `"abc"[1..9]` faults, `index out of bounds`, rather than minting a slice over whatever bytes sit past the terminator. A chained range, `s[0..4][1..3]`, and every base shape a range can sit on, a bound name, a literal, and a call result, all validate the same way.
- A raw pointer, `*raw T`, and `*void` have no length to validate a range against, so the range form on either is rejected at check, `cannot take a range slice of a raw pointer; it has no length to check the range against`. An ordinary index read through either stays legal; only `p[lo..hi]` is refused.
- A dynamic array is provided in the standard library as `std.vector`, a heap backed generic type.

### Immutability and Mutability

All variables are immutable by default. Mutability is declared with `mut`.

```text
x: int32 = 5       // immutable, cannot be reassigned
mut y: int32 = 5   // mutable, can be reassigned
```

Function scope restriction on mutability.

A mutable variable is only mutable within the function it was declared in. A lambda created inside that function can read it but cannot mutate it. The language has lambdas, not nested function declarations, so there is no inner `func` to reach back and write the outer local.

Immutability covers projections. An element or field store, `xs[i] = v` or `p.x = v`, needs its root binding declared `mut`, the same as the bare `xs = v` form. A store through a pointer dereference or through a slice writes the buffer the binding views, not the binding, so it is governed by the pointee's rules instead.

```text
func outer() -> void {
    mut x: int32 = 5
    x = 10             // allowed, same function

    bump := lambda () -> void {
        x = 15         // COMPILE ERROR, x not mutable inside a lambda
        y := x + 1     // allowed, reading x is fine
    }
}
```

Scope here means the declaring function body. Ordinary blocks in the same function, such as loop bodies and `if` branches, can mutate the variable. Only a lambda body loses mutation rights. So `mut x = 0` followed by a `for` loop that runs `x = x + 1` is allowed, while mutating `x` from inside a lambda is not. This forces explicit data passing into inner scopes and prevents hidden state mutation through closures.

### Pointers

Pointers are immutable. Once a pointer is assigned it cannot be reassigned to a different address. Pointers exist only as the result of an explicit heap allocation through `alloc`. There is no address of operator for stack variables. Stack variables are passed by value.

```text
p: *int64 = alloc(100)   // p points to a heap int64 initialized to 100
```

After `free(p)`, the binding `p` is consumed. Using it again is a compile error where statically determinable, and a trapping poison value in debug builds.

### Foreign Functions

Added in 0.2.4. A `foreign` block declares functions that live outside dusk, so dusk code can call into C. The functions have no body. Each binds at link to a C symbol of the same name in anything the binary links, which is libc and the dusk runtime today. The standard library uses this to bind the runtime's `cool_*` shims.

```text
foreign "C" {
    func abs(n: int32) -> int32
    func write(fd: int32, buf: *raw int8, count: int64) -> int64
}
```

The boundary is the raw pointer layer only. A parameter or return type is a scalar, a `*raw T`, or a `*void`. A managed `*T` is rejected, since it is a fat value carrying a generation that C cannot read, so a buffer crosses as `*raw T` and an opaque pointer as `*void`. Once declared, a foreign function is called like any other function. A `*raw T` passes anywhere `*void` is expected, both are the same bare word. The reverse binding is rejected, since a `*void` that could become a typed `*raw T` would let a managed pointer launder through `*void` into a dereferenceable alias the generation check cannot see. A managed `*T` that round trips through `*void` back to a managed annotation comes back untracked, with no generation for the check to read, so everything through it afterward is the raw layer's honor system. Keep managed pointers on the managed layer. A named struct also crosses by value under a narrower rule; see [Struct by value](#struct-by-value) below. A function type crosses too, as a callback under a narrower rule of its own; see [Callbacks](#callbacks) below.

- Only the `"C"` calling convention is supported.
- A struct passed by value, and a dusk function handed to C as a callback, now both cross on the import side, a `foreign` declaration's own parameter and return types. Exporting a dusk function so arbitrary C code outside the boundary calls it directly now crosses on the export side too; see [Exporting to C](#exporting-to-c). A struct by value on an export, parameter or return, is the one boundary shape still deferred, since the classification the import side runs does not yet drive the export define side.

#### Struct by value

Added in 1.4.1. A named struct may be declared as a `foreign` function's fixed parameter type or return type when every one of its fields is C plain: an integer of any width, `char`, `rune`, `float64`, a `*raw T`, a `*void`, a nested struct that is itself C plain, or a fixed array of any of those. A field of any other type is rejected by name at the struct, `field '<name>' is <kind>; <advice>`:

- `bool`, `field '<name>' is bool; use char or int8 at the boundary`, since an `i1` field has no settled one byte storage rule at the boundary.
- `float32`, `field '<name>' is float32; use float64 or pass by pointer`, since coercing an eightbyte that packs two `float32` fields is not classified.
- a managed pointer, `field '<name>' is a managed pointer; use *raw T or *void, since a managed *T carries a generation C cannot read`.
- a string, a slice, a tuple, a closure, a collected value, a future, an error value, a thread handle, an interface, or an enum, each named outright, `field '<name>' is <kind>; the C boundary takes scalars, raw pointers, and nested plain structs`.
- a generic struct, `field '<name>' is a generic struct; instantiate it or pass a pointer to it`.

A variadic foreign function never takes a struct by value at all, fixed parameter or vararg tail, `foreign function '<name>' is variadic and cannot take a struct by value; a variadic foreign function takes only scalars and raw pointers`, since C reads a vararg positionally with no type information of its own to classify a struct against.

A C plain struct crosses the way clang's own classification places it, not as a flat memory copy. The struct is split into eight byte pieces; a piece holding only floating point data is an SSE register, anything else is an INTEGER register, and each register is coerced to the narrowest type that covers the data inside it, the same coercion clang emits at `-O0`. A struct of two eight bytes or fewer rides in registers, up to a pair of them; a struct larger than sixteen bytes falls back to memory entirely, passed by a hidden pointer, `byval` on a parameter and `sret` on a return. This is byte exact against clang, so the `.ll` dusk emits for a call or a `declare` links cleanly against an object a C compiler produced, from either side of the boundary.

The classification above is the System V x86_64 ABI, and `x86_64-pc-linux-gnu` is the only target triple dusk emits today. Struct by value across a different target's own calling convention is future work, on the same footing every other target already stands on.

#### Callbacks

Added in 1.4.2. A `foreign` function's parameter may be declared with a function type, meaning a bare C function pointer, one LLVM word carrying no environment, the shape a C API's own callback parameter, `qsort`'s comparator among them, already expects. This is narrower than a dusk closure, which carries a code pointer and an environment pointer together as a two word `{ ptr, ptr }` value; a callback drops the second word entirely, since C has nowhere to put it.

```text
foreign "C" {
    func qsort(base: *raw int64, n: int64, w: int64, cmp: (*raw int64, *raw int64) -> int32) -> void
}
```

A callback's own parameters and return type must themselves be C legal, the boundary's ordinary scalar, `*raw T`, or `*void` set. A struct by value inside a callback signature is refused the same as everywhere else the boundary classifies a type, since a struct is not one of the accepted shapes; a function type nested inside a callback's own signature is refused outright, `foreign function '<name>': a C callback's own parameters and return must be scalars or raw pointers; a nested function type cannot cross`, since C carries a callback as one function pointer word with no room underneath it for a second, nested closure word. A foreign function cannot itself return a function pointer, `foreign function '<name>' cannot return a function pointer; a returned C function pointer has no dusk value to call, only an argument callback is supported`; a callback only ever crosses as an argument, the one direction dusk can actually supply a code pointer for. A variadic foreign function cannot take a callback parameter either, `foreign function '<name>' is variadic and cannot take a function pointer parameter; a C callback and a varargs tail cannot be combined`, since a callback rides the direct call's exact signature check, a path the variadic call never runs.

At the call site a callback argument takes exactly one of two forms: a capture-free lambda literal, or the bare name of a top level, non-generic, non-foreign dusk function, each checked against the callback's declared parameter type by strict equality, no integer width widening and no wildcarding, since C reads the two mismatched widths as different registers. A lambda that captures a local is refused, naming the capture, `this callback captures '<var>'; C has no environment for it, pass state through the *void user data argument the API carries`. A signature mismatch names both sides, `callback argument N: expected <sig>, found <sig>`. A generic function name is refused before it has one concrete symbol to point a bare code pointer at, `callback argument N cannot be the generic function '<name>'; a C callback needs one concrete function, and a generic function has none until it is instantiated`. Anything else in that position, a closure value, a local, an arbitrary expression, is refused as the wrong shape entirely, `callback argument N must be a capture-free lambda literal or the name of a top-level function; a closure value has no bare C function pointer`.

A foreign function that takes a callback parameter cannot itself be taken as a value; it can only be called directly, `'<name>' is a foreign function that takes a C callback; call it directly, it cannot be taken as a value`, the same rule a variadic foreign function and a struct passing foreign function already carry. Taken as a value the call would route through a funcval thunk shaped for an ordinary closure, dropping the bare function pointer ABI a direct call preserves and silently miscompiling the call.

Codegen lowers both accepted forms with no trampoline. A capture-free lambda literal is lifted to a fresh top level function with no environment parameter at all, its LLVM signature exactly the callback's own, and the argument becomes that function's bare address. A named top level function already has that signature, so it crosses as its own bare address with no intermediate thunk either. Since C carries no closure environment, the sanctioned way to thread state through a callback is the same `*void` user data argument a C API's own callback registration typically carries alongside it, passed through untouched from the call site to every invocation:

```text
@csource "callback_fixture.c"

foreign "C" {
    func call_n(n: int64, user: *void, fn: (int64, *raw int64) -> int64) -> int64
}

func scale(i: int64, user: *raw int64) -> int64 {
    return i * user[0]
}
```

**Safety posture.** A callback body is ordinary dusk, checked and compiled exactly like any other function, and every runtime guarantee inside it, the generational dereference check, bounds checking, a named fault on abort, holds exactly as it does anywhere else. What changes is the thread: a callback runs on whichever thread the C code holding its pointer calls it on, and for a library that keeps its own worker threads that is not necessarily the thread that made the foreign call at all. Off the anchor thread a callback is governed by the same rules as a spawned thread's body: the generational heap stays thread safe, but the collector does not, and a `collector<T>` mint or a forced collection reached from a callback running off the anchor thread aborts by name, `fatal: the collector runs on the main thread only`, the identical fault a spawned thread hits. A callback body is never itself an async func, so it may call the loop's blocking primitives, `await`, `await_timeout`, and `try_poll`, directly; the rule that refuses them inside an async func body governs an async func specifically, not a plain synchronous callback the foreign boundary calls back into. The mechanism reaches a third party C library; it is not a way to install a dusk function into dusk's own reactor, which is C machinery the async substrate alone drives and exposes no callback shaped foreign entry point of its own.

#### Opaque handles

Added in 1.4.1. A C library commonly hands back an opaque pointer, a database handle or a stream, whose layout its own header does not expose. The convention at the boundary is to carry it as a `*void`: the library's own open call returns it, every later call the library exposes takes it back, and the boundary already keeps this safe end to end, since a `*void` can never be dereferenced and never launders into a typed pointer.

`std.fs`'s `Dir` follows the same convention with one substitution: its `h` field is an `int64` carrying the directory stream's underlying pointer as a bit pattern, not a `*void`. The substitution is not a style choice; it follows from a rule the language itself enforces. `==` on any pointer type is rejected outright, `pointers do not compare; compare the values they point to`, so a wrapper holding a `*void` would have no way to test a failed open against NULL the way a C caller tests `opendir`'s own return. Every call that can fail on such a handle reports failure through a separate status value instead, and the handle itself is read only by the same library's own next call. Read the `*void` form above as the convention proper and the `int64` form here as its equivalent under a constraint the pointer comparison rule imposes; neither exposes an unsafe read, since a bit pattern `int64` is exactly as opaque to dusk as the `*void` it stands in for.

#### Variadic foreign functions

Added in 1.4.0. A `foreign` block's parameter list may end in a bare `...`, marking the function variadic in the C sense.

```text
foreign "C" {
    func printf(fmt: *raw char, ...) -> int32
}
```

`...` is legal only as the last parameter; writing it anywhere else is rejected, `'...' must be the last parameter of a variadic foreign function`. Only a `foreign` declaration can take `...` at all, an ordinary dusk function cannot be variadic. A call must supply at least the fixed parameters; falling short of that count is rejected by name, `expected at least N argument(s), found M`, before a single argument is checked against a type.

Every fixed argument checks against its declared parameter type exactly as an ordinary call does. An argument beyond the fixed parameters, the vararg tail itself, is checked against the same admitted set the boundary already enforces: a scalar (an integer of any width, a `float32` or `float64`, `bool`, `char`, `rune`), a `*raw T`, or a `*void` rides across; everything else is rejected by name at the argument that carries it:

- a managed pointer: `argument N to a variadic foreign function is a managed pointer; pass a *raw T or *void, since a managed *T carries a generation C cannot read`
- a string: `argument N to a variadic foreign function is a string; pass a *raw char, a string view cannot cross a C vararg directly`
- a slice or an array: `pass its backing as a *raw T`
- a tuple: `pass its elements individually`
- a struct: `pass a pointer to it`
- a closure, a collected value, a future, an interface value, or an error: each named outright, none of them may ride a C vararg

C reads a vararg positionally with no type information of its own to check against, so a fat or generation carrying value handed across would read as garbage or corrupt the read; the reject list keeps every value on the wire a bare word C's own calling convention already knows how to place.

A value beyond the fixed parameters also takes the C default argument promotions before it is passed, the same widening a C compiler applies to its own variadic call sites:

| Argument type            | Crosses as                    |
| ------------------------- | ------------------------------ |
| `int8`, `int16`          | `int32`, sign extended         |
| `char`, `bool`           | `int32`, zero extended         |
| `float32`                | `float64`                      |
| `int32`, `int64`, `rune`, a raw pointer, `*void` | unchanged |

A fixed parameter never promotes; only a value riding the `...` tail does, since a fixed parameter's own declared type already tells C exactly what to expect at that position.

#### The errno convention

Added in 1.4.0. dusk never sets the C library's `errno` itself; a read only ever reports whatever the most recent foreign call, a libc function or a third party one, left behind. `std.os` exposes it directly: `os_errno() -> int64` reads the current value, and `errstr(code: int64) -> string` returns `strerror`'s message for a code, copied off `strerror`'s own static buffer into a fresh heap string the caller owns. `errno` is thread local under the pthreads runtime dusk links, so a read never races another thread's foreign call, only the calling thread's own next one. Read it immediately after the call whose failure it reports: any foreign call in between, including a call `std.os` or another stdlib wrapper makes internally, is free to overwrite it, so an `os_errno()` read after such a call has gone stale and reports someone else's failure instead. The wrapper carried the bare name `errno` through 1.5.x; 1.6.0 renames it to `os_errno` so the wrapper's own symbol never collides with the C `errno` it reads when a dusk module is emitted for a target whose libc declares `errno` as a symbol of its own.

#### Exporting to C

Added in 1.4.3. Everything above carries C into dusk. This carries dusk out: an `export "C"` function is a dusk function a C caller reaches directly, by its own bare symbol, with no callback pointer handed across first. It is the reverse of a callback, and the two round out the boundary in both directions.

```text
export "C" func mylib_add(a: int64, b: int64) -> int64 {
    return a + b
}
```

The `"C"` after `export` is a calling convention, the same string a `foreign` block carries, and `"C"` is the only one supported. A plain `export` still means only that a name is visible to other dusk files; adding the `"C"` marks the function a C ABI entry point besides, and only that stronger form imposes the rules that follow. Only a function can carry it: a struct, an enum, or an interface after `export "C"` is rejected, and an `async func` cannot carry it at all, since an async function is a state machine with no plain C signature to export.

An exported function's signature must be C legal in the same sense the boundary already means: every parameter and the return is a scalar (an integer of any width, `float32`, `float64`, `bool`, `char`, or `rune`), a `*raw T`, or a `*void`, and the return may also be `void`. A struct by value on an export is deferred within this line and rejected by name; the classification machinery that crosses a struct on the import side does not yet run on the define side, and a `@csource` adapter covers the shape meanwhile. A string, a slice, a managed `*T`, a closure, an interface value, a collected value, a future, and an error never cross, the boundary's standing rule extended verbatim to the export position.

Three name rules keep an export from colliding with the machinery around it. The function may not be generic, since C has no monomorphization and a generic has no single symbol to point at. It may not be named `main`, which the program entry point owns. It may not begin with `cool_`, the prefix the runtime reserves for its own shims. Beyond these the name is emitted exactly as written: privatization, which renames a file's private top level names so a bare call cannot reach another file's helper, skips it, and monomorphization roots its worklist at it, so an export reached from no dusk call at all still survives to the emitted module. The one collision the compiler cannot see is a symbol the host itself links under the same name; that one is the exporter's to avoid, the same as any C project sharing a link namespace.

##### Building a library

`dusk build --lib <file>` compiles a module to a linkable C library instead of an executable. The module may omit `main` entirely; monomorphization roots every `export "C"` function, so the exports and everything they reach compile even with no `main` to call them. The build writes two files beside each other in `target/dusk-out`:

- `lib<stem>.a`, a static archive bundling the module's own object and every dusk runtime object, so the archive is self contained: a host links it and needs nothing of dusk's besides. It is built with `llvm-ar` in its deterministic mode, so the same source yields the same archive bytes. The module object and every `@csource` object merge into one object whose only global symbols are the exports; a private dusk helper, a lifted lambda, and a `@csource`'s own functions are all made local, so none can interpose a host's like named symbol at the static link, and a host cannot call a `@csource` function directly, since a `@csource` is the library's implementation, not its interface.
- `<stem>.h`, a generated C header: `#include <stdint.h>`, one prototype per export inside an `extern "C"` guard so a C++ host may include it too, and a comment naming the link line. Each prototype's C types come from the same lowering the emitted symbol does, `int64_t` for an `int64`, `uint8_t` for a `char`, `int32_t` for a `rune`, `double` for a `float64`, `void*` for a `*raw T` or a `*void`, so the header can never drift from the archive.

A host compiles against the two together, for example `clang host.c -I target/dusk-out -L target/dusk-out -l<stem> -pthread -lm`. Any toolchain that statically links a C archive reaches the same symbols: C, C++ through the header's `extern "C"` guard, Zig, and Rust among them. A loader that opens a shared object at run time, Python's `ctypes` and Ruby's `Fiddle` among them, needs a position independent `.so` this line does not emit yet: the archive's objects are not position independent and the runtime's thread local storage takes the local exec model a shared object cannot, so a `.so` build waits for a later release. An `@link` a library file names is not folded into the archive, since it is an external dependency the host resolves; the header's link line names it so the host adds it. A `@csource` file is compiled into the archive, since it is the module's own C.

Every exported function runs `cool_lib_init` as its first act, an idempotent hook the runtime guards with `pthread_once`, so it runs exactly once no matter which entry point a host calls first or from how many threads. It has nothing to do today, since the runtime's own load time setup already fires through a constructor, and it exists so a future initialization need never changes the exported ABI.

Library mode meets the collector and the event loop by the same doctrine a spawned thread does, stated plainly rather than enforced by a new check. A library has no dusk `main`, so the collector's anchor, which the emitted `main` records, is never set; an exported function that mints a `collector<T>` or forces a collection therefore aborts by name, `fatal: the collector runs on the main thread only`, the same fault a spawned thread hits, so keep an export over plain values and raw pointers. The event loop and the async surface run on whichever host thread calls an exported function that drives them, one such thread at a time; a host that calls exported functions from several threads at once is in the memory model's documented race territory, no different from dusk's own threads sharing data.

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

A variant is constructed through the enum qualified form, `Shape.Circle(2.0)`. A `match` pattern reads the other way: an arm names the variant bare, `Circle(r)`, not qualified, since the scrutinee's own type already fixes which enum the arm belongs to. So construction is qualified and a pattern is bare, and the two never share a spelling. The bare form in construction position, `Circle(2.0)`, is not a constructor and is rejected, `use the qualified form 'Shape.Circle' to construct an enum value; the unqualified variant name is not a constructor`, naming the fix rather than resolving the variant by its global name and risking a collision with a like named function or a stale local of the same name still in scope. A constructor's argument count and each payload's type are checked against the variant's declaration: `Shape.Circle()` with no argument, and `Shape.Rect(1.0, true)` against a `float64` second field, are each rejected at the constructor site rather than left to surface later as an unrelated mismatch once the value is read back. A literal payload must fit the field's declared width the same way an annotation's right hand side must: a constructor handing an `int8` field a literal too large for eight bits is rejected at the constructor, `literal <n> does not fit in 8 bits`, the identical bounds rule `x: int8 = 300` faces.

A generic enum's empty variant, `Opt.None` on `enum Opt<T> { Some(v: T), None }`, carries no payload to read its type parameter from, so something around it has to pin `T` instead. The surrounding expected type does the pinning: a struct literal field, a call argument at a non generic parameter, an assignment's declared type, and an array element each thread their own grounded type down as the constructor's expected, instantiating `T` there rather than falling back to any default. A `Opt.None` sitting nowhere an expected type reaches it, an unannotated `:=` binding among them, is rejected by name, `cannot infer the type parameter 'T' for 'Opt'; add an annotation that pins it`, instead of silently defaulting the parameter to `int64` and later dying inside `clang` on a width it never actually had.

---

## Expressions and Operators

Added in 0.4.2. Every binary and unary operator sits on one precedence ladder, thirteen levels from loosest to tightest, each level left associative unless noted. Parentheses group as usual, and only the comparison level rejects chaining outright: `1 < 2 < 3` is a compile error, not a silently wrong bool.

| Level (loosest to tightest) | Operators                                 | Notes                                     |
| --------------------------- | ----------------------------------------- | ----------------------------------------- |
| 1. Range                    | `..` `..=`                                | only legal inside a slice index           |
| 2. Pipe                     | `\|>`                                     | a parse time rewrite to a call, see below |
| 3. Or                       | `\|\|`                                    |                                           |
| 4. And                      | `&&`                                      |                                           |
| 5. Comparison               | `== != < <= > >=`                         | not chainable                             |
| 6. Bitwise or               | `\|`                                      |                                           |
| 7. Bitwise xor              | `^`                                       |                                           |
| 8. Bitwise and              | `&`                                       |                                           |
| 9. Shift                    | `<< >>`                                   |                                           |
| 10. Additive                | `+ -`                                     |                                           |
| 11. Multiplicative          | `* / %`                                   |                                           |
| 12. Exponent                | `**`                                      | right associative                         |
| 13. Unary, then postfix     | prefix `- ! ~ *`, then call, index, field | tightest, unary binds tighter than `**`   |

Shifts sit between `&` and `+`, and the bitwise trio nests `|` loosest, `^` in the middle, `&` tightest, so `4 | 2 ^ 3 & 1` groups as `4 | (2 ^ (3 & 1))` and `1 + 2 << 3` groups as `(1 + 2) << 3`. `**` binds tighter than the multiplicatives and right associates, so `2 ** 3 ** 2` is `2 ** (3 ** 2)`, while unary minus binds tighter than `**` at the call site, so `-2 ** 2` is `(-2) ** 2`, which is `4`.

### Logical operators

Added in 1.2.0. `&&` and `||` both take a `bool` on each side; a non `bool` operand rejects, `logical operators need bool operands`, on whichever side has one. Both short circuit: `&&` evaluates its right operand only when the left is `true`, and `||` only when the left is `false`, so a guard like `i < n && a[i] == x` never reaches the array read once the length check has already failed, and a right side with a side effect, a call that prints or a call that faults, does not run when the left side has already settled the answer.

```text
i: int64 = 5
n: int64 = 3
if i < n && a[i] == x { ... }   // a[i] never evaluates; i < n is false
```

Before 1.2.0 both operands evaluated unconditionally, so a right side call always ran. This is a change to evaluation order, not to typing: both sides still type check regardless of which one runs at a given call, so a non `bool` operand behind a condition that would skip it is still a compile error.

### Comparison

`== != < <= > >=` compare two operands of the same type. An integer, a float, `bool`, `char`, `rune`, and a string all support the family, though a string supports only `==` and `!=`; a `<`, `<=`, `>`, or `>=` between two strings rejects, `strings compare with == and !=; they have no ordering`, since a string has no meaningful order to compare by. A string compares by content, through a runtime byte compare, not by the pointer identity every other value compares by; see [Strings](#strings).

A managed pointer, a raw pointer, an `error`, an array, a slice, a tuple, a struct, an enum, an interface, a future, a collector, and a thread handle have no meaningful `==`. Each rejects at check instead:

```text
p == q   // pointers do not compare; compare the values they point to
xs == ys // cannot compare an array; compare its parts instead (also a slice, tuple, struct, enum, interface, future, collector, thread)
e == f   // an error does not compare; test it with exists()
```

Compare a pointer by dereferencing both sides, an aggregate by comparing its parts, and an `error` with `exists()`. A generic type parameter stays permissive on the surface pass, since its concrete shape is not yet known, and is checked again once monomorphization makes it ground: a generic function comparing a type parameter that turns out to be one of the rejected kinds is caught on the ground pass rather than accepted forever.

A float comparison follows IEEE 754: `==`, `<`, `<=`, `>`, and `>=` are ordered, so any comparison against NaN, on either side, answers `false`. `!=` is the one unordered operator: `x != y` is the negation of `x == y` at every input, so `NaN != x` answers `true` for every `x`, NaN included, the same fact `is_nan` in `std.math` is built on, `!(x == x)`, true only when `x` is NaN. A comparison between two ordinary, non NaN floats reads the same under either convention.

### Bitwise operators

`&`, `|`, and `^` are binary and `~` is unary, all on integer operands only, two's complement throughout. `~0` is `-1`, `-1 & 255` masks down to the low byte, and each width truncates the way ordinary arithmetic does, so an `int8` operand keeps the mask honest at eight bits.

`<<` and `>>` shift by an integer amount. `<<` is a plain logical shift; `>>` is always an arithmetic shift, sign extending the top bit, because dusk does not track signedness separately from the type at the point a shift lowers. A constant shift amount outside `[0, width)` is a compile error, a negative constant included. A dynamic amount is checked at the shift itself and a miss aborts with the named fault `fatal: shift amount out of range`, at the file and line of the shift itself since 1.2.0, never a silently masked or poison result. See [Runtime Faults](#runtime-faults).

### Assignment targets

The left side of an assignment, plain or compound, must name storage the write can land in: a binding, a field chain, a scalar index, or a dereference. Anything else is rejected at check, `the left side of an assignment must be a place: a name, a field, an index, or a dereference`. A range index in particular is not a place: `xs[0..2]` mints an rvalue slice, so `xs[0..2] = [7, 8]` rejects; write the elements through scalar indexes instead.

### Compound assignment

`+= -= *= /= %= &= |= ^= <<= >>=` rewrite a place through a load, the operator, and a single store. The place, including any index expression, is evaluated exactly once: `xs[pick()] += 5` calls `pick()` once even though it names the index. A compound assignment on an immutable binding is rejected, the same rule the plain `=` form follows, and mixing widths on the right is the same error the binary operator gives.

### Increment and decrement

`++` and `--` are statement only, postfix only, and produce no value; there is no prefix form and neither can appear inside an expression. Each desugars to a compound assignment with the literal `1`, so `i++` is `i += 1` and an `int8` place wraps exactly the way `+ 1` does.

### Exponent

`**` is right associative. An integer base and exponent lower to `cool_pow_i64`, repeated squaring in `uint64_t` so the wraparound matches the plain `mul` codegen already emits; `0 ** 0` is `1`, the same convention Rust's `pow` uses. A negative integer exponent is meaningless for an integer result. A constant negative exponent is rejected at check, `'**' on integers needs a nonnegative exponent`, before any code is emitted; a dynamic exponent that turns out negative at runtime keeps the named fault `fatal: negative exponent in integer '**'` rather than returning a wrong value. A float base or exponent lowers to the LLVM `pow` intrinsic at the operand's width.

### Pipe

`x |> f(a)` rewrites at parse time to `f(x, a)`, prepending the left side as the call's first argument; `x |> f` with a bare name becomes `f(x)`. It is left associative and the loosest operator, so `1 + 2 |> double` pipes the whole sum, not just the `2`. The rewrite adds no capability, only a call spelling, so it is ungated by paradigm; a piped functional builtin still faces the ordinary paradigm gate on the call it rewrites to. The right side must be a function name or a call; anything else is a compile error naming the rule.

### Inclusive range

`a..=b` in a slice index is `a..b+1`: the endpoint moves before the ordinary `lo <= hi <= base.len` bounds check runs, so `xs[2..=1]` is the empty slice rather than a trap, and `xs[0..=n-1]` covers the whole backing.

### Operators dusk does not have

A few operators common elsewhere are deliberately absent. The ternary `?:`, optional chaining `?.`, and null coalescing `??` have no place in a language with no null: a managed pointer is single owner and every dereference is checked, a missing value is `Maybe.None` or an `error`, `if` already covers selection, and `?` is reserved. Spread `...` has no varargs to spread into. A concatenation operator, `<>` or a reused `++`, is also absent: `StringBuilder` owns string building explicitly, and a slice concatenation needs an allocator, which an operator has nowhere to name.

---

## Memory Management

### Philosophy

Manual memory management is the default. There is no ambient garbage collector, and no allocator backed by one: choosing an allocator never opts a program into collection. A collected heap sits beside the generational one instead, reached only through the `collector<T>` wrapper type, so a value is collected because a program wrote `collector<T>(e)` on it, never because of which allocator happens to be in scope. See the collected heap section below.

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

### Ownership, Borrows, and Freeing a Tree

`free` follows ownership, and ownership follows the binding that received the allocation. A parameter borrows: the callee may read and write through it, but `free` on a parameter is refused with `cannot free a borrowed pointer; only its owner frees it`, and `move` on one is refused the same way, so a callee can never end a lifetime its caller still holds. A value read out of a container borrows too: `vec_get` hands back the stored pointer as a view of the container's backing, so freeing or moving that result is refused. The refusal is one model across every spelling. Freeing a container read directly, `free(vec_get(v, 0))`, is rejected with `cannot free a call result that may alias its arguments; vec_take or map_take removes an element as its owner`, the `move` form with the matching `cannot move a call result that may alias its arguments; vec_take or map_take removes an element as its owner`, and the method spelling `free(box.get(0))` rejects the same way, since a method's element read aliases its receiver exactly as a bare call aliases its argument. Binding the read to a name first and freeing the name is refused as a borrow all the same.

Owning removal is the sanctioned way out. `vec_take(v, i)` removes the element at index `i` and returns it to the caller as an owned value, and `map_take(m, k)` removes the entry for `k` and returns its value the same way. The returned value is the identical fat pointer the container held, reading the same block header at the same generation, so a taken managed pointer is now the caller's to `free`. Removal preserves order: `vec_take` shifts every survivor after `i` down one slot with a plain copy loop, so the surviving elements keep their sequence. Taking the last index shifts nothing, so `while vec_len(v) > 0 { x := vec_take(v, vec_len(v) - 1) }` drains a vector in `O(1)` per step, and this is the idiom the language teaches, since a forward walk over a vector being taken from skips the successor that shifted into the vacated slot. `vec_take` bounds checks exactly as `vec_get`, a negative index and one at or past the length both aborting with `fatal: vector index out of bounds`; `map_take` of a key the map does not hold aborts with `fatal: map_take of a missing key`, so probe with `map_has` first when a key may be absent.

`map_take` reclaims nothing. It removes the entry from the table and forgets it from the insertion order, and it hands back the stored value, but it frees no bytes, not the value's block and not the key's. A string key's heap bytes belong to whoever else holds the string, the copy `map_keys` returns or the caller's own binding, so removal and reclamation stay separate acts and the caller frees the value and the key on its own terms.

A hand written deep free of a linked tree is written with a worklist and owning takes, not with recursion. The recursive spelling stays inexpressible by design: a parameter borrows, so a node reached through a recursive call arrives as a borrow and cannot be freed there. The worklist idiom pushes the root into a `*Vector<*Json>`, then repeatedly takes a node off the back as an owner, matches it to free its payloads, pushes each managed child onto the list through another take, and frees the node last. `examples/jsonfree.dusk` is the whole shape: it parses a json document, emits it, and frees every node, string payload, object key, and backing buffer through this one loop.

Take asserts ownership rather than proving it. Pushing a borrowed pointer into a container and taking it back out forges an owner with no provenance, and the deep free deliberately does this to its own root, which is a borrow the caller handed in. The static single owner discipline does not flow through a container, which is an escape hatch the checker cannot see across, so `vec_take` and `map_take` mint an owner on the caller's word. This is stated plainly because it is the honest boundary: every misuse of it, a node taken from two containers and freed twice, a payload freed under two matches, a borrow laundered through a take and freed while its real owner still lives, faults named at runtime through the generational check, `fatal: use of a freed or stale pointer at path:line`, rather than corrupting silently. The forged owner is a checked capability backed by the generation, not a proof the checker verified.

The tree free contract is heap strings only. `free` of a string that is a literal rather than a heap allocation is undefined: the runtime hands the pointer to the allocator undetected, so a deep free is safe only over a tree whose every string is heap allocated, which every `json_parse` result is. A hand built tree carrying a literal string payload frees that payload into undefined behavior at the `free`, and the contract names the requirement rather than checking it.

The standard library ships this deep free as `std.json`'s `json_free(root)`, the worklist walk moved verbatim, the first standard library code to call the owning takes. It consumes `root`, freeing every node, string payload, object key, and backing buffer, so no pointer into the tree is valid afterward and a later dereference of any freed block faults named through the generational check. It requires a fully heap allocated tree, which every `json_parse` result is, and a hand built tree carrying a literal string payload frees that payload into undefined behavior at its `free`, the same heap strings only contract above. A subtree reachable from two parents double frees, and the second free faults named rather than corrupting silently.

Match payload ownership tracks the subject. A `match` over a value whose binder is a managed pointer classifies that binder by how the subject was reached: under a deref of an owned pointer, `match *t` where `t` owns its allocation, a managed payload binder is an owner and may be freed or moved; under a borrowed subject, every binder borrows and freeing one is refused with `cannot free a borrowed pointer; only its owner frees it`. The raw binder and its annotated rebind agree with one voice now, so no spelling frees a payload binder under a borrowed subject or refuses one under an owned subject. A payload binder is also typed from the subject's enum, so `vec_len(items)` on a `*Vector<*Json>` payload infers its type parameter with no rebinding, and a binder whose declared type is an owner correctly demands `move` or direct use rather than a silent copy. A variant pattern that binds the wrong number of payload fields is a check error, `variant pattern '<V>' binds <got> of <want> payload field(s)`.

Some gaps remain, and they are named rather than hidden. An unannotated bind of a container read, `q := vec_get(v, 0); free(q)`, passes the check and faults at runtime: a bare generic return is not substituted on the surface pass where ownership runs, so the read is never classified as managed and the later `free` is not caught until the generation check aborts it. A container read through an opaque callee, a lambda value or an interface method dispatched at runtime, passes the check the same way, since the checker cannot see across the call to know the result aliases an argument. A user's own exported function named `vec_take` or `map_take` whose shape matches the standard library's is blessed as owning by name and shape; the shape guard narrows this but does not close it, and a declared owning marker is the intended replacement. Freeing the result of a wrapper that returns a fresh box directly is rejected conservatively, where binding it to a name and freeing the name is accepted. Extracting an owned payload through a `match` used as an expression is limited today, the copy spelling caught by the backstop and the move spelling not yet buildable, so the take idiom is the sanctioned extraction. Every one of these is a leak or a named runtime fault, never a silent corruption.

For a tree that would rather not track ownership at all, the collected heap still applies: a `collector<T>` owns its cells and reclaims unreachable ones without any `free`, which is exactly the shape a parser result or a long lived graph wants. Data that lives for the whole process can simply never free, which leaks nothing the process end does not reclaim.

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
- Use after free. Freed memory is overwritten with a poison pattern so a stale read is visibly wrong rather than plausibly valid.

The leak and double free counters report at exit. These are debug build diagnostics, not language guarantees, and the poison pattern only makes a stale read conspicuous; it does not trap. The sound use after free and double free guarantee is the generational heap's, the default allocator, whose generation check faults at the dereference itself, not the debug allocator's poison. Release builds omit the tracking for speed.

### Safety

0.1.0 does no ownership tracking, so freeing is manual. `defer` and arenas keep cleanup deterministic, and the debug allocator catches mistakes in tests. Generational references for sound use after free and double free detection arrive in 0.2.0. A generation token rides inside each reference and is checked at dereference, so it survives copies.

### Runtime Faults

Four runtime checks abort the process by name rather than corrupting memory or returning a wrong value: an out of bounds array, slice, or string index; a null pointer dereference; a stale or freed pointer dereference, the generational check; and a dynamic shift amount outside its operand's width. Since 1.2.0 each one names the source location of the statement that raised it.

| Check                         | Message                                                |
| ------------------------------ | ------------------------------------------------------ |
| Bounds                        | `fatal: index out of bounds at path:line`              |
| Null dereference               | `fatal: dereference of a null pointer at path:line`     |
| Generational (use after free)  | `fatal: use of a freed or stale pointer at path:line`   |
| Shift                          | `fatal: shift amount out of range at path:line`         |

```text
fatal: index out of bounds at examples/x.dusk:7
```

Codegen interns one `"path:line"` constant per fault site at compile time, computed from the merged source's own line table, and passes it to the runtime function that raises the fault. The location names the statement that raised it, the same one however deep the expression underneath it that actually triggers the check, and holds the same way inside an async body, a lambda, or a nested loop as it does at a function's top level. Every other named fault, the async and task family included, still aborts with a bare message and no location.

### Escaping Value Lifetimes

Added in 0.2.3, completed in 0.4.2, made interprocedural in 0.5.0. Two shapes of value hold a view into the returning frame rather than owning what they view, and returning either lets that view dangle the moment the frame is reclaimed. A slice into a frame local fixed array, an array literal or a range slice of one, dangles once its backing array is gone: `a slice into a local array escapes its frame; put the backing on the heap`. A closure that captures a frame local keeps that local alive only as long as its environment does, and returning the closure returns an environment about to be reclaimed: `a closure that captures a local escapes its frame; it cannot be returned`. A closure with no captures is a plain function pointer and returns fine, and a slice backed by a heap allocation or a slice parameter, whose backing the caller already owns, returns fine too.

The check is flow sensitive and driven by the declared return shape, not only the returned expression's own syntax, so it follows a value through a binding, an alias, or a match before it reaches the return, and it sees through every carrier a fat value can ride in: a bare return, a tuple returned by literal or by name, a struct or enum returned by literal or by name, a fixed array of a reference shaped element, and any of those nested inside a generic field, at any nesting depth. `return xs[0..3]` from a `T[3]` local is caught directly; so is `return (row, 1)` when `row` is that same slice, `return Wrapper { items: row }`, `return [row, row]`, and a generic struct field that resolves to the same slice type once monomorphized. A managed pointer escape is not part of this check: dusk has no address of operator, every pointer is heap allocated, and the runtime generation check already catches a stale one at the dereference that follows. A frame view stored through a `*raw T` is not part of it either: the raw pointer layer is the same FFI boundary honor system a foreign pointer rides on, outside the escape walk by design, so keeping a view backed while a `*raw T` holds it is the caller's responsibility.

The 0.4.2 check above is intraprocedural: it walks one function body and stops at a call boundary, so a view laundered out through a call it cannot see into used to dangle uncaught. `func passthrough(s: int64[]) -> int64[] { return s }`, called on a slice into a frame local array and returned again by its caller, handed back a dangling view with no diagnostic. Since 0.5.0 escape enforcement is interprocedural, driven by a summary computed for every function and lambda literal: which parameters a return value may alias, which pointer parameters' pointees the return value may expose, which parameter's view may be stored into a place another parameter reaches, and which parameters are handed to `chan_send` or `chan_try_send`, directly or through a helper that itself does the same. A method's summary treats its by pointer `self` as the first parameter, so a method that stashes a frame view through `self` or sends `self` into a channel is caught the same way a plain function is. A callee the summary cannot see through, a closure value, a function parameter, or a lambda bound to a struct field, is opaque, and an opaque call defaults to rejecting a polluted argument, a managed pointer whose pointee a store has already touched, a bare frame slice, or a frame capturing closure, rather than accepting one it cannot prove clean. Enforcement runs on the surface pass; the ground, types only pass monomorphization drives never repeats it.

Escape flags now travel with an alias, not only with the binding a view was first assigned to. Every binding introduction site, a `let`, a tuple or struct destructure, a match payload binder, a `for` loop variable, and a plain assignment, links the new name into the alias group of every managed pointer, or pointer reaching value, its initializer touches; storing a frame view through any member of the group raises the whole group, so `st := Store{c: c}`, `p := st.c`, and a loop variable bound the same way all keep a later escape of `c` linked back to `st` or `p`. The link only forms for a type that can reach a managed pointer, a bare pointer, an aggregate with one buried inside, or a generic field erased to the unknown type; a slice or a scalar member links nothing, so a clean sibling field or a scalar read through the same binding is never falsely tainted.

Two residuals stay open past 0.5.0. A frame view stored through a `*raw T` is still outside the escape walk entirely, the same honor system boundary the FFI layer has always carried: the raw pointer layer speaks no generation and the escape summary makes no attempt to trace a view once a raw pointer holds it, so keeping the backing alive while a `*raw T` names it is the caller's responsibility. And an alias buried inside an aggregate a call returns is not yet surfaced: `wrap(c)` returning `Store{c: c}` forms no edge from the binding that receives the struct back to the pointer argument `c` itself, so a store through the returned struct's field and a separate, later use of `c` on its own can read clean when the two in fact name the same escaped view. Closing it needs the summary to expose a per field alias inside a returned aggregate, not only a whole parameter relation, and is left to later work. A nested enum variant's payload carries the same latent gap, not yet alias linked to the binding that built it, though this stays safe today since a locally constructed enum copies its payload into the enum's own storage rather than aliasing the argument; the two gaps close together the day enum payloads alias instead of copy.

An interface value is a fat pointer, a data pointer paired with a vtable pointer, and boxing a concrete struct into one works correctly when the interface value sits inside a struct field, an enum variant's payload, or an array element: the struct literal, the enum constructor, and the array literal each box the concrete value into the field, payload, or element's fat pointer as they build it, so a later method call dispatches through the stored interface exactly as it would through a bare interface binding. Boxing a struct to an interface does not work at every position, though. Returning an interface value by value is rejected outright, since the boxed payload would sit in a frame slot that dangles the moment the function returns: `returning an interface value is not supported; return the concrete type or a pointer to it`. An interface value inside a tuple, whether returned or passed as an argument, is rejected the same way in both positions rather than accepted at one and miscompiled at the other: `an interface value inside a tuple is not supported; return or pass the concrete type, or box it outside the tuple`.

A slice of a concrete struct type and a slice of an interface type share the same two word shape at the machine level, `{ ptr, len }`, which makes reinterpreting one as the other compile clean and read every element as a boxed interface at runtime, silently corrupting memory. Passing, assigning, or storing an existing slice value where a slice of an interface is expected is rejected as this covariance: `cannot pass a slice of '<concrete>' as a slice of interface '<iface>'; a slice of concrete values cannot be reinterpreted as a slice of interfaces`. An array literal of concrete structs coerced to a slice of an interface is exempt, since it boxes each element as it coerces rather than reinterpreting an existing buffer, and a slice of an interface passed where a slice of that same interface is expected is exempt for the same reason: it is not a reinterpretation at all.

### The Collected Heap and `collector<T>`

dusk ships a second managed heap beside the generational one: a conservative, mark and sweep collected heap, opted into per value through the `collector<T>` wrapper type and its minting expression, `collector<T>(e)`. Nothing lands on the collected heap by default; a value is collected only because a program wrote `collector<T>(e)` naming it, so the ambient allocator, `alloc`, and the generational heap's dereference checking are unaffected by a program that never mentions `collector`.

A collected block carries the same sixteen byte header a generational block carries, an eight byte size word followed by an eight byte generation word ahead of the payload, so the generational dereference check that faults on a stale generation reads a collected block's header exactly as it reads a generational one's. The two heaps differ only in how a block is retired: an explicit `free` retires a generational block by bumping its generation and parking it, while a collected block is retired only by a collection, which scans the roots, marks what a root can still reach, and bumps the generation of everything left unmarked.

`collector<T>` mints one of three kinds of collected value, chosen by the element type `T`.

- **Plain.** `T` a scalar, a managed `*T`, a string, or a struct built only of those. The block holds the value the way a managed `*T`'s block holds its pointee, and `*c` or a field read on `c` derefs through it exactly as an ordinary managed pointer would, the same generation check firing on every dereference.
- **Closure.** `collector<F>(lambda ...)`, `F` a function type. The lambda's environment is built on the collected heap instead of the frame or the generational heap a plain closure would use, so the closure keeps working after the frame that wrote it has returned.
- **Slice.** `collector<U[]>(e)`. The backing is deep copied onto the collected heap, one level, so a slice into a frame local array becomes a legal source: the copy severs the view from the frame that built it. This kind is legal only when `U` is immortal safe (below); a slice of slices, a slice of closures, or a slice of interfaces is rejected, since the one level copy immortalizes the outer buffer and nothing an element of it points at in turn.

**Minting is escape neutral.** A collected value is not a frame view: its block sits on a heap that outlives every frame, so a `collector<T>` value returns cleanly, bare or embedded in a tuple, struct, or array, exactly like any other clean value. The mint itself, though, is an outliving sink the same escape check already runs on a `return`: an argument to `collector<T>(e)` that carries a frame view, a closure over a frame local or a managed pointer whose pointee a store has already tainted, is rejected at the mint, since collecting it would copy that view onto a heap the view's own backing does not outlive. The one exception is a slice source: `collector<U[]>(e)` deep copies the backing onto the collected heap, so a slice into a frame local array is a legal argument there, the copy severing the view from the frame that built it, exactly as the slice kind above describes. The closure kind carries the matching capture rule: every capture in a `collector<F>(lambda ...)` must itself be immortal safe, a scalar, a managed pointer, a string, a nested `collector<..>`, or an aggregate of those, and a managed pointer capture whose pointee already stores a frame view is rejected even though the pointer itself is immortal safe, since the view it points at is not.

**No `free`, no `move`, no `ref`.** A collected value is never freed, moved, or borrowed with `ref`; all three are compile errors, `a collected value is not freed; the collector reclaims it` among them. Passing or storing a collected value copies it by value, the same rule an ordinary managed pointer or scalar follows: there is no ownership to transfer, because there is no explicit release to hand off. Reclamation happens only when a collection finds no root reaching the block.

**Thread confinement.** The collector is single mutator: it runs only on the one thread it anchors to the first time a collected block is minted or a collection is forced, in practice the thread that runs `main`, since the collector's root scan walks that thread's stack and no other. A collected value is only sound to hold on that same thread, and the checker enforces the confinement at compile time rather than leaving it to an off thread runtime abort. Rejected outright: a `Channel<collector<T>>`, since a same thread channel's ring buffer sits outside every root the collector scans; a `spawn` or `submit` capture of a collector value, since a worker thread's private environment is the same kind of unrooted store; boxing a collector value into an interface, since the boxed payload would need to travel wherever the interface value travels; and a managed pointer whose pointee reaches a collector value across any of those same crossings. Allowed: a `Future<collector<T>>` and an async func that returns a collector value, since a future completes on the loop thread and `async_run` is that same anchor thread's own bridge into the loop; and a same thread container, `Vector<collector<T>>` among them, since the container's backing buffer is itself a generational block the collector's registry already scans as a root.

The confinement checks a value that is directly a `collector<T>`, not a struct that merely carries one as a field. Boxing a struct with a collected field into an interface is allowed: an interface value is itself barred from crossing a `spawn`, a `submit`, or a channel, so the collected field behind it can never reach another thread, and confinement holds transitively without a separate check on the field. The direct case, boxing a bare `collector<T>` value into an interface, stays rejected, `a collected value cannot be boxed into an interface; it stays on the main thread`. That reject is a deferred boxing path rather than a confinement rule: a bare collected payload has no stable home in an interface's fat pointer yet, and lifting the limit is later work, not a hole in the thread rule.

**`collector` is a contextual reserved word.** `collector<` starting a type or an expression position is read as the start of a `collector<T>` type or a `collector<T>(e)` mint. A named binding called `collector` compared against something else, `collector < n`, still parses as an identifier: the parser looks far enough ahead to tell the two shapes apart before it commits to either reading, so naming a variable `collector` stays legal everywhere outside that one ambiguous shape.

**Widening is one way.** A `collector<F>` value passes anywhere a plain `F` is expected, and a `collector<U[]>` value passes anywhere a plain `U[]` is expected, since a collected value's representation is exactly the value it wraps and no conversion runs. The reverse direction does not hold: a plain `F` or `U[]` never becomes a `collector<F>` or `collector<U[]>` implicitly, and a bare lambda literal handed where a `collector<F>` parameter is expected is only accepted at a direct top level call, where the compiler rewrites it into the equivalent mint; at a method argument or through an indirect call the same bare lambda is rejected, with the explicit `collector<F>(lambda ...)` form named as the fix. Only the explicit mint runs the escape and capture checks that make a wrapped value immortal safe.

**Cost and collection.** A mint is one allocation on the collected heap. Collection is amortized: it runs automatically once the byte debt since the last collection crosses a threshold that doubles with the live set, at whichever mint trips it, and a program can force one directly through `gc_collect`. The scan is conservative: every word on the anchor thread's stack between a collection point and the thread's high water mark, every collected reference reachable through the generational heap's own live registry, and every root region the async substrate registers for a task frame or a closure environment, is read as a possible pointer and, where it lands inside a live collected block, keeps that block and everything it in turn reaches. A conservative scan only over retains: a stray word that merely looks like a pointer keeps a block alive one collection longer than it needed to, never the reverse. This is mark and sweep, not moving: a collected block never relocates, so a raw address into one stays valid across a collection for as long as the block itself stays live. A precise, moving collector is not this one. dusk's build passes no optimization flag to `clang`, and the collector depends on that: its root scan brackets the anchor thread's stack under the frame layout the unoptimized build guarantees, where a local variable keeps a stack home a register allocator could otherwise remove.

`std.memory.collector` wraps the collector's control and gauges: `gc_collect` forces a collection now, and `gc_live_blocks`, `gc_live_bytes`, and `gc_collections` read its counters. It does not offer a `Collector` type implementing `Allocator`. The `Allocator` interface hands back an untyped `*void`, which would erase the `collector<T>` tracking the checker relies on to keep a collected reference confined to its anchor thread, so a collected block routed through the allocator seam could cross a channel or a spawn boundary as a bare pointer with no diagnostic and be swept while a worker thread still held it. Closing that hole needs the checker to track whether a value is collected through the allocator seam itself, and is left for later work; the typed `collector<T>` mint stays the one checked surface for collected memory. A function parameter declared with an undeclared type name is itself rejected at check, `unknown type '<name>'; no type of that name is declared or imported`, so a phantom `Collector` parameter written to probe for a collector allocator type is a compile error rather than a silently accepted unknown.

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

A generic instantiates over concrete types only. An interface cannot be a generic type argument: `Box<Speaker>` is rejected with `an interface cannot be a generic type argument; generics are monomorphized over concrete types`, since monomorphization stamps one copy per ground type and an interface names a set of types, not one. A generic that should range over an interface's implementors takes the interface as a plain parameter or field type instead, where the value carries its vtable.

### Anonymous Functions (Lambdas)

A lambda is an anonymous function declared with the `lambda` keyword.

```text
double := lambda (n: int64) -> int64 { return n * 2 }
```

Lambdas are first class values and are the argument form for functional builtins.

```text
doubled := map(nums, lambda (n: int64) -> int64 { return n * 2 })
```

Capture rule. A lambda can read variables from outer scopes, captured by immutable copy. It cannot mutate them. The copy is taken when the lambda is created. There is no capture by reference, which matches the absence of an address of operator and pass by value everywhere.

```text
factor := 3
triple := lambda (n: int64) -> int64 { return n * factor }   // reads factor by copy
```

---

## Object Oriented Concepts

Available when `@paradigm oop` is declared. Both an `interface` declaration and an `impl` block require the directive: a file that declares either without `@paradigm oop` is rejected during paradigm gating, the same way a functional builtin is rejected without `@paradigm functional`. Structs are the one exception here, ungated and available in every paradigm, since a struct is plain data rather than an OOP construct.

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

`self` inside a method names the receiver's value, of the concrete struct type the impl names, not a pointer to it, even though the receiver is passed by pointer underneath and `self.field` writes back through it by design. A whole value use of `self` where a pointer is required, `return self` against a `*T` return, `self` handed into a `*T` parameter at a direct call or a method call, or an explicit `*self`, is a value where a pointer is required and is rejected at the source rather than surfacing as a stray backend type error; returning `self` where the return type is the plain struct stays legal, since that is exactly the value `self` names. `impl` targets a struct receiver only. Codegen dispatches a method call on a struct, so an enum named as an impl's receiver type is rejected outright rather than compiling into a call that never fires and a `match self` inside it that silently falls to the wrong arm.

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
| fold     | fold left over a collection                        |
| foreach  | iterates over a collection for side effects        |

`fold` is a left fold only, `fold(xs, init, lambda (acc, x) -> ...)`, threading the accumulator from the initial value forward through the collection; there is no right folding form. Each builtin's argument count is checked: `fold` takes exactly three arguments and `map`, `filter`, `reduce`, and `foreach` take two, so a stray extra argument is rejected, `fold takes 3 argument(s)`, rather than silently ignored.

These take lambdas, which capture outer variables by immutable copy (see Lambdas).

### Monads

The `monad` keyword declares a special interface type that enforces monadic structure. The compiler verifies that the required operations are present. Do notation is available when `@paradigm functional` is declared. The `monad` keyword belongs to the functional paradigm.

A monad block must define both a `unit` operation that wraps a value and a `bind` operation that chains computations. A block missing either is rejected at parse, `a monad block must define both 'bind' and 'unit'`. The type parameters live on those two functions, not on the block header, so the header names the monad bare, `monad Maybe`, and `bind` and `unit` carry their own generics.

```text
monad Maybe {
    export func unit<T>(x: T) -> Maybe<T> {
        return Maybe.Some(x)
    }
    export func bind<T, U>(m: Maybe<T>, f: (T) -> Maybe<U>) -> Maybe<U> {
        match m {
            Some(a) => return f(a),
            None    => return Maybe.None,
        }
    }
}
```

The standard library ships these monads through import.

| Monad        | Description                                             |
| ------------ | --------------------------------------------------------- |
| Maybe<T>     | an optional value                                        |
| Either<L, R> | one of two possible types                                |
| IO<T>        | wraps a side effecting computation, lazy over its thunk  |
| Result<T, E> | success or a typed failure                               |
| List<T>      | the list monad (planned, not yet in the tree)            |

This program builds a `Maybe` and prints the value it carries.

```text
@paradigm functional

@import std.functional.maybe
@import std.io

func main() -> int32 {
    m: Maybe<int32> = Maybe.Some(54)
    result := unwrap_or(m, 0)
    std.io.println(result)
    return 0
}
```

A `Maybe` is constructed through the qualified `Maybe.Some` and `Maybe.None` forms, the same as any other enum, and read back with `match` or a helper like `unwrap_or`. A method call on an enum value, `m.unwrap()`, is rejected, `'unwrap' is not defined; methods on the enum 'Maybe' are not supported, match on it instead`, since only struct receivers dispatch a method; the monad's `bind` and `unit` are plain functions the `do` desugar calls, not methods on the value.

Do notation requires `@paradigm functional`. A `do Name { ... }` block names the monad it desugars against, so several monads coexist in one file; a bare `do { ... }` desugars against the plain top level names `bind` and `unit` instead of a namespaced pair, a shape none of the shipped monads export, so name the monad in practice. A do block is a sequence of `name <- expr` binds followed by one final expression, evaluated top to bottom, with no `return` inside it.

```text
result: Maybe<int32> = do Maybe {
    x <- maybe_divide(10, 2)
    y <- maybe_divide(x, 0)
    z <- maybe_add(y, 1)
    z
}
```

Added in 0.4.3, `do` desugars against a generic `bind` and `unit`, not only a pair already ground to concrete types. Before this release a `do` block only worked when its target monad's `bind` had no type parameters of its own; now `Maybe`, a hand rolled monad shaped like `Either`, and any user `monad Name { ... }` block generic over its element type all compose through `do` the same way.

```text
struct Box<T> {
    v: T,
}

monad Box {
    export func bind<A, B>(m: Box<A>, f: (A) -> Box<B>) -> Box<B> {
        return f(m.v)
    }
    export func unit<A>(x: A) -> Box<A> {
        return Box { v: x }
    }
}

func main() -> int32 {
    r := do Box {
        a <- Box { v: 3 }
        b <- Box { v: 4 }
        c <- Box { v: 5 }
        a * b + c
    }
    println(r.v)   // 19
    return 0
}
```

The desugar emits a chain of generic bind continuations over an open type hole, one bind for the value between each pair of steps, and monomorphization resolves and instantiates the `bind` and `unit` pair fresh at each `do` site rather than once for the whole program: an argument pass reading the types actually bound, an expected type or annotation pass, a lambda body pass, and first binding wins once one of those pins a concrete type. A `do` over a type with no `monad Name { ... }` block is rejected at the names its desugar calls, `undefined name '<Name>.bind'` and `undefined name '<Name>.unit'`, and a `bind` whose signature drops the continuation parameter is rejected as an arity mismatch on the desugared call, such as `expected 1 argument(s), found 2`.

Because the continuation the desugar builds carries an open type hole until monomorphization closes it, a second, types only pass re-runs the real type checker over the whole module once every type in it is concrete, recovering the width and type checks the open hole would otherwise let the continuation's body skip. Before this pass landed, an int32 and int64 mix inside a generic `do` continuation's body silently truncated instead of being rejected; it is now caught exactly as the same expression is in plain code, `arithmetic mixes int32 and int64; match the widths`, and a `do` block's inferred element type clashing with an explicit annotation on its binding is caught the same way, `return type does not match the function's return type`. The fix is general, not a special case for `do`: the same recheck also catches a width mismatch hiding inside an ordinary generic function body.

`std.functional.io` ships `IO<T>` as a `monad IO { ... }` block over `struct IO<T> { run: collector<() -> T> }`, composing through the generic `do` above like any other monad. Added in 0.5.3, `IO<T>` is a true lazy monad: `bind` and `unit` never run anything, they build a new collected thunk that captures the source and the continuation, so a whole `do IO { ... }` chain is a suspended computation sitting on the collected heap the moment it is built. `run(io: IO<A>) -> A` is the one effect boundary; it forces the thunk on the calling thread and returns the value the chain produces. Nothing about `IO<T>` touches the event loop or the thread pool: building a chain performs no effect, and `run` needs no `loop_init` or `pool_start` beforehand, unlike the earlier eager form this replaces.

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

Because the thunk and every step it captures live on the collected heap, a chain outlives the frame that built it and survives a collection forced between build and force; the escape check treats the mint the same as any other collected value, so a chain built from steps that only capture immortal safe data (a scalar, a managed pointer, a string, a nested collector) is accepted, while a step that would capture a frame local slice or an uncollected closure is rejected at the mint, naming the capture. `IO<T>` inherits collector confinement: a value of it cannot cross a `spawn` or `submit` capture, a channel, or an interface box, since the suspended environment behind its thunk is only ever rooted on the anchor thread. The shipped `IO` helpers yield `IO<bool>` rather than `IO<void>`, `io_print` and `io_println` among them, because `void` carries no value for `bind` to thread through a chain. Hand constructing an `IO<void>` is not banned at the language level, since `IO<T>` is an ordinary generic struct; `run` merely forces its thunk and yields nothing. The helpers pick `bool` so an effect still returns a value a `do IO { ... }` chain can carry.

**Migration note.** Before 0.5.3, `run` minted a future and offloaded the carried value to a pool worker, so a program using it had to bring the loop and the pool up first and tear them down after the last `run`. That contract is gone: `run` now forces its thunk directly on the calling thread, and a program that still calls `loop_init` or `pool_start` around an `IO` chain for no other reason no longer needs to.

`std.functional.result` ships `Result<T, E>` as `enum Result<T, E> { Ok(v: T), Err(e: E) }`, with a `monad Result { ... }` block fixed to `E = string`, the common case, since a generic `E` cannot flow through `do` inference. `do Result { ... }` threads `Ok` values and short circuits on the first `Err`, and `result_from(v: T, e: error) -> Result<T, string>` bridges the `(value, error)` pair a fallible call returns into a `Result`, folding an existing error into `Err(e.toString())` and an absent one into `Ok(v)`.

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

The tuple return is destructured at the call site. Both values must be bound to named variables. Binding the whole pair to one name, `r := x.pop_back()`, is rejected, `a fallible result must be destructured; bind the value and the error`, naming `v, e := f()` as the fix, so the error can never hide unread inside an aggregate. This is one case of the general tuple destructuring form, `a, b := (x, y)`, and a binder's own type annotation there, when given, is checked against the tuple member it binds the same way an ordinary `let` annotation is: `a: char[2], b: int64 := ("hi", 1)` is `'a' has a type annotation that does not match its value` rather than a mismatch that reaches codegen and stores the member as the wrong type. The error binding must be used. Using an error means one of three things.

- inspecting it with `exists()` (usually followed by control flow),
- handling it with `check(...)`,
- or explicitly discarding it with `ignore()`.

```text
y, e := x.pop_back()
e.ignore()   // explicit, visible, greppable suppression
```

Unlike Go, there is no `_` suppression. `ignore()` replaces it. The difference is that `ignore()` is a visible, searchable acknowledgement in the source, while `_` hides the decision. An unhandled error binding is a compile error.

Added in 0.5.3, a fourth way to use a bound error is to hand it to a parameter declared `error`.

```text
func sink(err: error) -> void {
    err.ignore()
}

y, e := x.pop_back()
sink(e)   // discharges e; sink's own err is now its obligation
```

Handing `e` straight to `sink`'s `err` parameter discharges the caller's obligation the same way a bare `return e` or a call to `check` or `ignore` does; a value handed to a plain, non `error` parameter does not discharge anything, `take(v)` next to an unread `e` is still rejected. The obligation does not stop at the caller: an `error` parameter carries the same must handle rule a let bound error does, so `func swallow(err: error) -> void { }`, a callee that receives an error and drops it with an empty body, is rejected, `the error 'err' is never handled`, the same message an unread let binding gets. A callee discharges its own `error` parameter by inspecting it with `exists()`, resolving it with `check(...)`, discarding it with `ignore()`, returning it, or handing it off again to another `error` parameter, the identical menu a let bound error has. The obligation is narrowed to a direct hand-off: reading the error into a fresh value first, or passing it through a generic passthrough call, does not count, so `sink(fst(e, e2))` still leaves both `e` and `e2` unhandled even though `sink` itself is clean. The net effect is a chain with no silent end: an error can move from a `:=` binding to a parameter to another parameter, but it cannot vanish into a body that never looks at it.

---

## Threads and the Memory Model

Added in 0.3.0. A thread is an OS thread. `spawn` starts one and `join` waits for it, both always available builtins like `alloc` and `read_file`, gated behind no paradigm.

```text
t, e := spawn(lambda () -> void {
    println("worker")
})
if e.exists() {
    printerr(e)
    return 1
}
je := join(t)
je.ignore()
```

`spawn(f: () -> void) -> (thread, error)` takes a lambda literal written at the call site, of type `() -> void`. The error fires when the operating system refuses the thread, and the must handle rule makes the caller face it. A closure variable cannot be spawned, since only the literal site knows the environment layout the runtime copies; wrap the call in a literal instead.

`join(t: thread) -> error` blocks until the body returns. `thread` is an opaque builtin type like `error`. The handle is a record in the generational heap and `join` retires it, so a second `join` of the same handle faults through the same check a use after free hits. Join what you spawn: a thread still running when `main` returns dies mid work.

### What crosses a spawn

A spawned lambda captures outer variables by immutable copy, like every lambda, and the copies live in a private heap block the runtime frees when the body returns. A thread therefore never reads another thread's stack and never mutates another thread's locals.

Scalars, strings, fixed arrays, structs, enums, tuples, raw pointers, and handle structs such as `AtomicInt` cross freely as captures. Capturing a slice, a closure, or an interface value is a compile error, wherever it sits, including buried in a struct or enum field, since each may view the spawning frame. A captured managed `*T` becomes a borrow inside the thread: the thread reads through it, and freeing or moving the binding there is a compile error. The ownership pass tracks direct bindings only, so a pointer laundered through an aggregate falls to the runtime generation backstop, the division of labor the ownership rules already document.

### Channels

Added in 0.3.1. A channel is a bounded, thread safe queue in `std.concurrent.channel`, an ordinary generic struct over runtime shims, not a compiler type. `Channel<T>` holds at most the capacity given at construction, always at least one.

```text
@import std.concurrent.channel

jobs: Channel<int64> = chan_new(8)
e := chan_send(jobs, 42)
e.ignore()
v, re := chan_recv(jobs)
re.ignore()
println(v)
chan_close(jobs)
chan_free(jobs)
```

`chan_new<T>(cap: int64) -> Channel<T>` sizes the element from the binding annotation, the same rule `alloc` uses, so a bare `jobs := chan_new(8)` cannot pin `T` and is a compile error. A capacity below one or exhausted memory is fatal rather than an error, the allocator's contract.

`chan_send(c, x) -> error` copies the value in and blocks while the channel is full. Its error exists when the channel is closed, whether it was closed before the call or while the sender waited. `chan_recv(c) -> (T, error)` copies the oldest value out and blocks while the channel is empty. Its error exists only once the channel is closed and drained, so a loop breaking on `e.exists()` consumes everything that was sent. The value beside that error is the zero pattern for `T` and means nothing. When `T` is a managed pointer that zero is null, and dereferencing it faults by name as a null dereference. `chan_close(c)` is idempotent, wakes every blocked sender and receiver, and discards nothing already buffered.

A channel element must be safe to carry to another thread, the same rule spawn captures follow: an element type containing a slice, a closure, or an interface value, wherever it sits, is a compile error at the instantiation, since each may view the sending frame and the ring would deliver a dangling view. Send heap owned data instead.

The handle is one word and copies freely, including into a spawned lambda's captures, and every copy names the same channel. It is deliberately exempt from the single owner rule because it is not a managed pointer: a channel is a sharing point, and aliasing it is its purpose.

Ownership crosses a thread boundary by moving a managed pointer through a channel. `chan_send(c, move(p))` kills the sender's name at compile time through the ordinary argument position move, and the receiver's `q, e := chan_recv(c)` binds a fresh owner through the ordinary call returns ownership rule. Sending without `move` leaves the sender holding a live name, so the sender and receiver then share the record with no order between them. The generation check backstops a free racing a use, best effort as the memory model section says.

A moved send that the channel refuses loses the record. When `chan_send(c, move(p))` returns the closed error, the value never entered the ring, the sender's name is already dead, and no name anywhere reaches the allocation again, so it leaks. The same applies to managed pointers still buffered when `chan_free` runs, since the ring holds raw bytes and frees none of them. Neither is corruption, and neither happens in the sanctioned protocol where senders finish before the close, but a design that closes under active movers pays in leaked records, not faults.

Added in 0.3.3, three operations refuse instead of parking. `chan_try_send(c, x) -> error` reports "channel is full" without waiting for room, `chan_try_recv(c) -> (T, error)` reports "channel is empty" without waiting for a value, and `chan_recv_timeout(c, ms) -> (T, error)` parks at most `ms` milliseconds against a monotonic clock and reports "receive timed out", so a wall clock step cannot stretch or shrink the wait. Each still reports the closed message its blocking twin uses, and the value beside any of these errors is the zero pattern for `T`. A tick loop parks on `chan_recv_timeout`, does a round of work, and loops back in, which is the event loop shape the async release builds on.

Added in 0.4.3, `chan_recv_async(c: Channel<T>) -> Future<T>` makes a receive awaitable on the event loop instead of blocking the caller. A blocking `chan_recv` on the loop thread stalls every task, so this is the sanctioned answer: it mints a future and hands the blocking receive to a detached helper thread, which completes the future off the loop thread when a value arrives or the channel closes and drains, the closed case completing with the `receive on a closed, drained channel` error its blocking twin uses. The loop awaits that future like any other, and the helper raises the live thread gauge before it starts and drops it strictly after the completion, so the deadlock detector keeps the awaiter parked while the receive is outstanding rather than declaring the loop idle. Because the helper is detached and cannot be joined, the drain discipline is close and settle, not the blocking channel's close then join: closing the channel releases the helper with the closed error, and the completion settles before the channel is freed. The future element obeys the same ban as `future_new` and `future_wrap`, so a slice, closure, or interface element is rejected where the future is minted.

Shutdown follows one order: close the channel, join every thread that touches it, then `chan_free` it. Freeing a channel while a thread is blocked inside a send or receive is fatal with a named message, caught best effort. Using a channel after `chan_free` is undefined, the raw layer's honor system, since the one word handle carries no generation.

### Mutexes and Condition Variables

Added in 0.3.2. `std.concurrent.sync` carries `Mutex` and `Condvar`, ordinary structs over runtime shims like the channel. The blessed shape for shared mutable state is a `*raw` buffer guarded by one mutex: lock, touch the buffer, unlock.

```text
@import std.concurrent.sync

m := mutex_new()
counter: *raw int64 = alloc_bytes(8)
counter[0] = 0
lock(m)
counter[0] = counter[0] + 1
unlock(m)
mutex_free(m)
free(counter)
```

`lock(m)` blocks until the mutex is free and `unlock(m)` releases it. An unlock happens before the lock that next acquires the same mutex, which is the ordering that makes the guarded memory safe to touch. Inside a function body the idiom is `lock(m)` followed by `defer unlock(m)`, so every return path releases. The handle is one word and copies freely, including into a spawned lambda's captures, and every copy names the same lock.

The mutex is the error checking kind, so relocking a mutex the thread already holds and unlocking a mutex the thread does not hold, both undefined in the default pthread flavor, fault by name. The runtime adds the rest: a trylock probe makes freeing a held mutex fatal, an operation on a mutex already freed faults as an invalid mutex rather than a misleading holder message, and a waiter count makes freeing a condition variable a thread waits on fatal instead of the silent forever hang the bare destroy gives.

`cond_wait(cv, m)` releases the mutex, sleeps until `cond_signal(cv)` wakes one waiter or `cond_broadcast(cv)` wakes all, and reacquires the mutex before returning. The caller must hold the mutex, every concurrent wait on one condition variable must name the same mutex, and wakeups can be spurious, so a wait always sits in a loop that rechecks its predicate under the lock.

```text
lock(m)
while buf[5] == 0 {
    cond_wait(notempty, m)
}
// consume under the lock, then
unlock(m)
```

Free a condition variable only after every waiter has left it. Freeing one a thread still waits on is fatal by name. A condition variable wait has no timeout, so a predicate nothing ever makes true is a deadlock. A channel receive is the wait that can time out, through `chan_recv_timeout`.

### The Thread Pool

Added in 0.3.3. The pool is a process singleton of OS threads that runs fire and forget tasks, the substrate the async release schedules onto. `submit` is an always available builtin like `spawn` and shares its whole argument rule: one lambda literal of type `() -> void`, captures copied to a private heap block, the same slice, closure, and interface capture ban, and a captured managed pointer borrowed, not owned. It returns only an error, because the pool owns the task and results flow through a channel.

```text
@import std.concurrent.channel
@import std.concurrent.pool

pe := pool_start(ncpu())
pe.ignore()
done: Channel<int64> = chan_new(8)
se := submit(lambda () -> void {
    we := chan_send(done, 42)
    we.ignore()
})
se.ignore()
v, re := chan_recv(done)
re.ignore()
println(v)
pool_shutdown()
chan_free(done)
```

`pool_start(workers) -> error` in `std.concurrent.pool` starts the singleton with a fixed worker count, `ncpu() -> int64` being the natural count. The error exists when the count is below one, the pool is already running, it was already shut down, or the operating system refuses a worker thread. A refused start leaves the pool startable again, but a successful start is the only one the process gets, and after a shutdown the pool stays down. A `submit` never blocks the submitter, whatever the queue holds, and its error exists only when the pool is not running, in which case the task body never runs. `pool_shutdown()` stops new submissions, runs everything already queued to completion, joins the workers, and is idempotent. When two threads race into it, the loser waits for the winner, so every caller returns holding the drain guarantee. A task still running at shutdown finishes normally, and a submission it makes after the flag flips is refused like any other, but a pool task calling `pool_shutdown` itself is fatal by name, since the worker would otherwise join itself or wait forever on its own completion.

Submission order is queue order, but tasks run on many workers at once, so nothing about completion order is promised. Queuing a task happens before its body runs, and everything a body did is visible to whoever receives its completion through a channel, the ordering the channel edge already provides. Shut the pool down before `main` returns for the same reason threads are joined: a worker mid task when the process exits dies mid write.

### Futures and the Event Loop

Added in 0.4.0, the first phase of the async line. A `Future<T>` from `std.async.future` is a one shot completion slot: minted pending, completed exactly once from any thread, and consumed exactly once by the thread that owns the event loop. The loop is a process singleton like the pool, started by `loop_init() -> error` in `std.async.loop` on the thread that will consume futures, and freed by `loop_free()` after the last completer has finished. Unlike the pool, a freed loop may be initialized again, on any thread, which then becomes the owner; futures from the earlier loop stay consumable, but their pending timers are gone. Everything except completion is a loop touch and faults by name off the owner thread.

```text
@import std.concurrent.pool
@import std.async.future
@import std.async.loop

le := loop_init()
le.ignore()
pe := pool_start(2)
pe.ignore()
f: Future<int64> = future_new()
se := submit(lambda () -> void {
    n, ne := compute()
    ne.ignore()
    ce := complete(f, n, ne)
    ce.ignore()
})
se.ignore()
v, e := await(f)
e.ignore()
println(v)
pool_shutdown()
loop_free()
```

`future_new() -> Future<T>` mints a pending future, the element type pinned by the binding annotation like `chan_new` and `alloc`. The channel element ban applies at the minting site: an element type containing a slice, a closure, or an interface value is rejected at compile time, since a view of the completing thread's frame would dangle in the awaiter. The handle is a plain pair of words and copies freely; every copy names the same future, which is how a pool lambda captures it. `complete(f, v, e) -> error` stores the value and the error together from any thread and wakes the loop, so an offloaded body hands its own failure through unchanged and the awaiter reads exactly the pair the completer supplied. The second completion is refused with `future already completed` and its value is dropped, whether the loser arrives before or after the awaiter consumes the future, so racing completers never need to outrun the awaiter. Passing an error into `complete` does not discharge it; the completer still inspects or ignores its own binding.

Consuming reads the pair and retires the record in the generational heap, so a future is awaited once the way a thread is joined once, and the second consume faults with `use of a dead future`. `await(f) -> (T, error)` parks until completion. `await_timeout(f, ms) -> (T, error)` parks at most ms milliseconds against the monotonic clock and comes back with `await timed out`, the zero value, and the future still live, the recoverable escape hatch. `try_poll(f) -> (T, error)` never parks, reporting `future is pending` while unresolved and consuming the future once it is ready. `future_free(f)` releases a future that will never be consumed; do not free one a completer may still touch, the channel free discipline.

`sleep_async(ms) -> Future<int64>` in `std.async.time` mints a future the loop's timer heap completes with 0 at its deadline. Timers fire while any await or poll runs, deadlines measure on the monotonic clock, and two timers sharing a deadline complete in creation order, so awaiting a long timer lets shorter ones fire in passing.

An await that provably cannot finish is a deadlock, not a hang. When no timer is pending, no spawned thread is alive, and no pool task is in flight, nothing in the process can complete the future, and the wait aborts with `the event loop is idle but work is still pending`. The gauges drop only after their bodies finish, and every drop wakes the loop, so the gate never fires against a completion still in flight; a live thread parked forever still parks the await, since the loop cannot prove it will never complete. The fault family, each named: consuming a dead future, touching the loop off its owner thread, touching it before `loop_init`, and the idle deadlock.

Two honest leaks, stated like the channel's refused moved send: a future never consumed leaks its record, and a pending timer still queued at `loop_free` leaks its record. Both are rule breaking shutdowns paid in leaked records, never corruption. The costs are not hidden either: a future is one generational record, a completion and a consume each stage the element and the error through scratch allocations exactly as a channel operation does, and an await is a park on the loop's monitor, not a spin.

### The Reactor and Readiness Futures

Added in 0.4.1, the second phase of the async line. The reactor is one C thread that turns file descriptor readiness into one shot readiness futures on the event loop, behind `std.async.io`. It runs no user code and touches no user memory: it trades only in file descriptors, its own watch records, and the future and loop entry points every other completer already shares. Zero compiler changes; the release is a runtime file, a standard library module, and the one link line that pulls the file in.

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
w := readable(p.r)
m, me := await(w)
me.ignore()
println(m)
reactor_stop()
loop_free()
```

`reactor_start() -> error` starts the thread. Its error fires on a double start, an operating system refusal setting up the epoll and event descriptors, or a start landing while a concurrent stop is still in flight, each `the reactor could not start`, the `loop_init` shape. `reactor_stop() -> void` flips the reactor to stopped, signals the thread, which finishes delivering everything already ready before it exits, then joins; a stop racing a concurrent stop waits for the winner to finish instead of returning early. A stopped reactor restarts clean, a fresh epoll descriptor and event descriptor on each start, mirroring the loop it serves; it has no hard dependency on the loop at start time. The sanctioned order is `loop_init`, then `reactor_start`, every watch armed and fired, `reactor_stop`, then `loop_free`.

The reactor's kernel wait sits behind a small poller seam of six functions, create, arm, disarm, wait, wake, and destroy, so the thread above it, the watch registry, the armed gauge, and the fire path that completes a future stay one portable core with no platform split. Two backends satisfy the seam: epoll on Linux, and kqueue on the BSDs and macOS. The epoll backend is the one this project builds and runs; the kqueue backend targets BSD and macOS, compiles to nothing on Linux where its platform guard is false, and its behavior below is read from its own source rather than from a run this project has had no BSD or macOS machine to perform.

One divergence between the two backends is intentional and named rather than smoothed over. Closing a file descriptor while its watch is still armed is already a misuse no sanctioned program commits, but if the operating system reuses that same descriptor number before the stale watch's event is ever handled, the two backends disagree on what the reused number's fresh arm sees. epoll drops its own registration the instant the descriptor closes, so the reused number arms clean, silently overwriting the stale entry with no fault. kqueue's arm can never fail on a duplicate registration the way epoll's does, so the kqueue backend reproduces the ordinary second watch fault by checking its own registry before arming, and that check cannot tell a reused descriptor's first watch from a genuine second one on a still open descriptor: where epoll overwrites and moves on, kqueue faults, `the file descriptor already has an armed watch`. Both backends refuse a watch on a regular file alike, `a regular file cannot report readiness`, epoll for free from `EPOLL_CTL_ADD`'s own `EPERM`, kqueue from an `fstat` its arm performs up front, since kqueue would otherwise accept the registration and report the file permanently ready, a silent divergence the check exists to close.

`readable(fd: int64) -> Future<int64>` and `writable(fd: int64) -> Future<int64>` in `std.async.io` arm a one shot watch on a file descriptor and return a future that completes with the readiness mask: 1 for readable, 2 for writable, 4 for hangup, 8 for error, ORed together into one `int64`. The watch fires exactly once by construction, and the reactor drops it the moment it fires. Only one armed watch is allowed per file descriptor at a time; arming a second watch on an fd that already carries one is a fault, not an error, since the signature carries no error channel. `future_free` on a readiness future does not disarm the watch: a freed future's watch stays armed until it later fires, at which point the completion lands on a dead record and is discarded like any other losing completer.

An armed watch is a possible completer, so arming one raises a gauge into the deadlock gate the same way a live thread or an in flight pool task already does: while any watch is armed, an otherwise idle await keeps parking instead of aborting, and the count drops only after the completion it produced is visible under the loop's lock, so the idle fatal never fires against a completion still in flight.

The non blocking byte surface sits beside the watches. `pipe_new() -> (Pipe, error)` makes a close on exec, blocking by default pipe with `r` and `w` fields, refusing with `the pipe could not be created`, or with `too many open files` when the descriptor table is exhausted; call `fd_nonblock(fd: int64) -> error` on an end before arming a watch on it, refusing with `the file descriptor could not be set non-blocking`. `read_nb(fd, buf, cap) -> (int64, error)` and `write_nb(fd, buf, n) -> (int64, error)` move bytes through a caller staged buffer, the channel element idiom, and never block: each refuses with `would block` when the operating system has nothing to give or take, the one canonical recoverable string in both directions, or with `the read failed` and `the write failed` on a harder refusal. A `read_nb` returning a count of zero with no error is end of stream, every writer closed. Writing to a pipe whose read end is fully closed, or to a socket shut down for writing, would ordinarily deliver `SIGPIPE` and kill the process; the runtime ignores `SIGPIPE` process wide before `main` ever runs and before any other thread is spawned, so that write instead returns `broken pipe`, a value the caller inspects like any other error, and no sanctioned program dies from writing into a closed pipe. A socket reset by its peer is a distinct case, `ECONNRESET` rather than `EPIPE`, and surfaces as the plain `the write failed`, not `broken pipe`. `fd_close(fd: int64) -> error` closes a descriptor, refusing with `the file descriptor could not be closed`.

Three hardening guarantees hold across this whole non blocking surface and the TCP surface built on top of it, not only the pipe path above. The `SIGPIPE` suppression just described is process wide, not pipe specific, so a socket shut down for writing behaves the same way a closed pipe does; nothing in the runtime installs or restores the signal handler anywhere else. Every blocking system call the reactor and its shims make, the poller's own wait, a `read`, a `write`, an `accept`, retries in place on `EINTR` rather than surfacing a spurious signal interruption as a failure the caller must handle. A file descriptor mint, a pipe, a socket, or an accepted connection, that finds the process or the system out of descriptors, `EMFILE` or `ENFILE`, surfaces as the named `too many open files` error rather than a crash or a silent retry; the mint that hits it is atomic, so it opens nothing and leaks nothing, and the reactor, the loop, and every future already armed keep working once the caller has handled the error.

The fault family, each named: arming a watch while the reactor is not running or after it has stopped, `the reactor is not running`; a second watch on an fd that already has one, `the file descriptor already has an armed watch`; a watch armed on a closed or nonexistent descriptor, `a readiness watch was armed on an invalid file descriptor`; a watch armed on a regular file, which epoll cannot poll, `a regular file cannot report readiness`; stopping the reactor while a watch is still armed, `the reactor stopped while a watch is still armed`, since the alternative is either a parked awaiter stranded forever or a dropped gauge lying to the deadlock gate later; the reactor's own wait failing for a reason other than a signal interruption, `the reactor could not wait for readiness`; the eventfd write that signals a stop persistently failing, `the reactor could not be signalled to stop`; and watch record exhaustion, `out of memory`, the same message every allocation failure already uses.

Bytes written to a pipe before the readiness event that reports it are visible to the read that follows the `await`: the kernel's own pipe ordering composes with the complete happens before consume edge the memory model already gives every future, so the two together order the whole path from write to read.

### `async func`, `await`, and `async_run`

Added in 0.4.2, the third phase of the async line and its keyword layer, on top of the futures and the event loop 0.4.0 and 0.4.1 built. Where those releases completed futures by hand, `async func` compiles a function to a single poll function over a heap allocated task frame. Calling it writes its arguments into a fresh frame, mints the task's result `Future<T>`, and runs nothing until the loop cranks it. `await` is the statement level suspension inside an async body, and `async_run` is the only bridge from synchronous code into the loop.

```text
async func amain() -> int32 {
    println("in")
    return 7
}

func main() -> int32 {
    le := loop_init()
    le.ignore()
    rc := async_run(amain())
    loop_free()
    println(rc)
    return 0
}
```

Calling `amain()` mints a task and a future and does no work. `async_run` cranks the event loop until that future completes, then yields its value; it is the sync to async bridge, `main`'s job, and illegal anywhere else a task frame already exists.

#### The signature rules

An async func's task frame and future are laid out at one declared shape, so it takes no type parameters: `an async func cannot take type parameters`. A method cannot be async, `a method cannot be async`, since a method call cannot suspend across the receiver's borrow. `main` cannot be async, `main cannot be async; call an async func with async_run instead`, since it is the C entry point the runtime calls directly, with no task frame around it yet.

A parameter or return type may not be a future, since a future belongs to the event loop thread and the caller should await it instead: `an async func cannot take '<name>': a future belongs to the event loop thread; await it in the caller instead`, and symmetrically for a return, `an async func cannot return a future; a future belongs to the event loop thread, so await it in the caller instead`. A parameter or return may not be a slice, a closure, or an interface value either, since the task frame outlives the call that made it and any of the three may view the caller's stack: `an async func cannot take '<name>': a slice, closure, or interface value may view the caller's frame, which the task outlives`, and for a return, `an async func cannot return a slice, closure, or interface value; the value would outlive the task frame it views`. Both walks see through a struct or tuple parameter or return type, so a future or a view buried in a field is still caught.

An async func's name is only a callable; it cannot be stored or passed as a plain value: `'<name>' is async; call it with await or start it with async_run`. A bare call that mints a future and drops it before it is ever awaited or released is rejected for the same reason a leak is rejected everywhere else in the language: `the future from '<name>' is never awaited; bind it so it can be awaited or released`. A future that is bound then follows the ordinary unused variable rule beyond that.

#### `await`, in exactly four statement positions

`await` is not an operator. It never appears mid expression: `'await' cannot appear mid-expression; give the awaited value a name, as in v, e := await f`. It is legal in exactly four statement shapes, and nowhere else, each a keystone that keeps every value live across a suspension named and stored in the frame rather than sitting in an SSA register a resume cannot see.

```text
v := await f          // single bind: the value; the completer's error is discarded
v, e := await f        // destructure: the value and the completer's pending error
await f                 // void discard, legal only when f's element is void
return await f          // propagation: forwards the awaited tuple whole
```

The void discard form is rejected when the awaited element is not void: `'await f' discards a value; bind it, as in v, e := await f`. When the awaited future's element is itself a tuple, such as the `(int64, error)` a fallible async func returns, a matching arity destructure binds each member directly instead of the value plus error pair, and a mismatched name count is rejected: `await destructures this future into {n} values, but {m} names are bound`. The error word of a two bind await is a pending error like any other and falls under the ordinary must handle rule: left unhandled it is `the error '<name>' is never handled; inspect it with exists, handle it with check, or discard it with ignore`.

`await` only suspends inside an async func body, and only directly inside it, never inside a lambda literal created there: a lambda has no task frame of its own to suspend. Outside an async context a leading `await` not written as the plain call `await(f)` is rejected, `'await' is only legal inside an async func`. Inside a lambda created within an async body the reject names the lambda directly, `a lambda cannot await; only the enclosing async func can suspend`, since only the enclosing async func has a task frame to suspend and the lambda has none of its own. Under `defer`, which runs at completion and can never suspend, a leading `await` is rejected the same way: `'await' cannot appear under defer; a defer runs at completion and cannot suspend`. `await` composes with every statement shape a value can sit in: inside a `while`, an `if`, a `for` over a named fixed array, and a `match` arm reading its payload after the await, each survives the resume because the loop counter, the array's data pointer, length, and index, and the match payload are all frame slots, reloaded on the resume edge rather than kept in a register the suspension bypassed.

Ordinary rules keep applying underneath the keyword: `move(p)` into an awaited call still kills the mover's name at compile time, so touching `p` after `v := await consume(move(p))` is `use of a moved pointer` exactly as it would be with no await in the way.

#### `async_run`

`async_run(g(args))` takes a direct call of an async func, written at the call site, never a stored future: a future does not carry which async func minted it, so `async_run takes a direct call of an async func, written at the call site` is rejected even when the stored future genuinely came from one. It cannot be called from inside an async func, since the enclosing task frame can simply await the call instead: `async_run cannot be called inside an async func; await the call instead`. Calling it from a synchronous helper the loop invokes while already cranking, an async body reaching a sync function that itself calls `async_run`, is not a compile error, since the checker cannot see through an arbitrary call graph, but the loop refuses the re-entry by name at runtime, `fatal: async_run re-entered the event loop`.

#### The frame and the state machine

Below sema, an async func lowers to `define void @async.<name>.poll(ptr %frame)` plus an `@async.<name>.framesize` constant the call site reads. The frame is a heap block: a fixed 48 byte C task header the runtime owns, immediately followed by the dusk visible frame the poll addresses, state word first, then the pending future's data pointer and generation the last await wrote, then the result region, then every parameter in declaration order, then every local that must survive an await in emission order, each aligned to its own requirement and the whole frame rounded up to 16 bytes.

The poll's entry block GEPs every one of those slots once, so every frame pointer is born in the entry block and dominates every resume edge, then loads the state word and switches on it: state 0 enters at the body's start, and each await site registers its own state and its own resume label. An await stores the state that names its resume label, records the pending future's data pointer and generation, suspends by returning from the poll, and the loop's crank later calls the poll again at that state. A resume reloads whatever it needs from its frame slots rather than trusting an SSA value, since nothing survives a suspension except what a frame slot holds; a return, including the implicit one at the end of a function that falls off the end, replays every registered `defer` in reverse order exactly once, then completes the task with its result bytes and retires it. A state the switch does not recognize is impossible by construction and traps rather than guessing: `fatal: a task resumed in an invalid state`.

Both of the compiler's alloca funnels for a local that must persist route to a frame slot when they are lowering an async body. A closure created inside an async body is the one exception: its environment is not a frame slot, since a closure can outlive a single poll turn and a multi capture environment is wider than a slot reserves for scalar use; it allocates from a per task environment arena instead, one block per closure execution, freed in one pass when the task completes. The same per execution allocation covers a slice backed by an array literal and an interface value boxed inside the frame, so a loop that builds a fresh closure, slice, or boxed interface on each iteration and stores it for later keeps every iteration's value distinct rather than aliasing the last one through a reused slot.

#### Determinism

The whole async substrate runs on one loop thread with a FIFO ready queue: a task that becomes runnable, because its await found the future already complete or a completer enqueued it, joins the tail of that queue, and the crank runs one task to its next suspension or return before picking up the next. An await always costs exactly one scheduler turn, even against an already complete future, and never resumes inline, so two tasks each printing a line before yielding interleave in exact, reproducible program order: two `worker` tasks each printing a label and a counter around `await tick()` produce `a0 b0 a1 b1 a2 b2`, not a race. Anything that crosses the pool, a spawned thread, or the reactor funnels back through one future completion and one enqueue, so the loop thread's own ordering is never in question; only the moment a pool worker or a spawned thread finishes work is externally timed.

#### Run to completion, no cancellation

A task runs to completion once started; there is no mechanism to cancel one mid flight. This is what makes the `defer` replay at true completion sound: a suspension is never a premature exit, so a resource acquired before an await and deferred for release is guaranteed to see that release, in reverse registration order, exactly once, whenever the task actually returns, never at a suspension partway through.

#### Errors as values, monadic bind

There is no rejection channel. A completer hands its value and its error through together, and the awaited tuple destructures through the same must handle machinery every other fallible result uses; `return await f` propagates the pair whole. `await` is monadic bind performed by the compiler: it sequences a suspending computation, threads its result into the frame that continues, and forwards its error alongside the value rather than short circuiting through an exception.

#### The fault family

Every abort under the async keyword layer and the substrate beneath it is named and pinned by a golden.

| Message                                                   | Fires when                                                                                                   |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `fatal: use of a dead future`                             | a future or task result is awaited, polled, or freed a second time                                           |
| `fatal: two tasks await one future`                       | a second task parks on a future that already carries a waiter                                                |
| `fatal: the event loop is not running`                    | a loop touch, an await, a completion, or a task start, runs before `loop_init` or after the owning loop is freed |
| `fatal: async_run re-entered the event loop`              | `async_run` is called while the loop is already cranking                                                     |
| `fatal: the event loop is idle but work is still pending` | an await parks with no timer, no live thread, no in flight pool task, and no armed watch left to complete it |
| `fatal: a task resumed in an invalid state`               | a poll's entry switch sees a state its own emission never produced                                           |
| `fatal: a task resumed on a pending future`               | a resumed poll tries to take a future still in flight, an internal invariant, not a user reachable path      |
| `fatal: out of memory`                                    | a task, its frame, or a closure environment block cannot be allocated                                        |

#### The cost table

An async call is one frame allocation, the task header plus the dusk visible frame in a single block, and one future record in the generational heap; nothing runs until the loop schedules it. An await is one enqueue, when its future is already complete, or one waiter registration followed by one enqueue from whichever completion reaches it, and one scheduler turn either way; it never resumes inline. A leaf future, the kind `future_new` or a timer mints, is one generational record. Nothing here differs from the cost 0.4.0 already names for a hand rolled future; the keyword layer changes how the frame is built, not what completing one costs.

#### The completer doctrine

A future belongs to the event loop thread. A completer running on another thread, a pool worker or a spawned thread, never captures the typed `Future<T>` handle: capturing one is rejected wherever it would cross, a spawned lambda's captures and a submitted lambda's captures alike, since a heap copied environment would carry the typed handle off the thread that owns it. Instead the completer carries the future's two raw words, its handle and its generation, lifted out before the spawn or the submit, and completes through `complete_raw`, the completer surface built for exactly this crossing. `complete` and `complete_raw` are otherwise identical: exactly one completion wins and a late loser is refused and dropped, whether it arrives before or after the awaiter consumes the future.

#### The pumping rule

Inside an async body the only way to wait on a future is `await`. Calling the loop's blocking `await`, `await_timeout`, or `try_poll` primitives directly inside an async func is a compile error, `'<name>' pumps the event loop and cannot be called inside an async func; use the await statement`, since pumping one by hand would park the one thread the whole loop cranks on, and every other task, timer, and completion behind it, on a wait the keyword layer already has a name for. The reject is direct only: a sync helper that pumps, reached through an arbitrary call graph the checker cannot see into, is left to the runtime, where a pumped await that stalls the only crank thread does not hang silently but converts the stuck task into the same named idle fatal an ordinary deadlocked await produces, since from the loop's own gauges the thread is simply gone.

#### Migration from 0.4.0 and 0.4.1

The six async examples 0.4.0 and 0.4.1 built by hand around a completer lambda now complete through `complete_raw` instead of a raw runtime call, with their goldens unchanged; `complete_raw` is the same completer surface a task's own pool offload uses. The stdlib `await` function, the one `std.async.future` already exported, keeps working for sync code that awaits a future outside any async body: the keyword is context sensitive and only absorbs the name `await` as a suspension inside an async func body, so `await(f)` stays a plain call everywhere else.

### TCP networking

Added in 0.4.3. `std.async.net` puts TCP over the reactor's readiness futures. A socket is an ordinary file descriptor the reactor already knows how to watch, so the networking surface is a thin standard library layer over the non blocking socket calls and the `readable` and `writable` watches, with no new event machinery and no compiler change.

`tcp_listen`, `tcp_local_port`, and `tcp_close` are synchronous. `tcp_accept`, `tcp_connect`, `tcp_read`, and `tcp_write` are async funcs: each tries its non blocking socket call, and when the call would block it awaits `readable` or `writable` on the descriptor and retries, so a server that accepts many connections and a client that connects both suspend and resume as tasks under `async_run`, never pumping the loop from inside a task. `tcp_connect` finishes the non blocking connect handshake by awaiting writability and then reading the socket error, so a connection refused after the handshake began surfaces as a clean error rather than a descriptor that fails on first use. `tcp_write` sends every byte, looping over writability and the non blocking write until the whole buffer is gone, so a short write never silently drops the tail. Addresses are literal IPv4 dotted quads; there is no name resolution yet. A listener bound to port 0 is assigned an ephemeral port the caller reads back with `tcp_local_port`.

Awaiting a networking future is subject to the same rule as any other await: it is legal only inside an `async func`, and awaiting `tcp_accept` or `tcp_connect` from a synchronous function is rejected, `'await' is only legal inside an async func`.

```text
@import std.async.net

async func serve(lfd: int64) -> int64 {
    cfd, ae := tcp_accept(lfd)
    ae.ignore()
    buf: *raw int64 = alloc_bytes(64)
    n, re := tcp_read(cfd, buf, 64)
    re.ignore()
    w, we := tcp_write(cfd, buf, n)
    we.ignore()
    ce := tcp_close(cfd)
    ce.ignore()
    return w
}
```

### The memory model

dusk does not detect data races. When two threads touch the same memory, at least one writes, and no sanctioned path orders the accesses, the program has a data race and its behavior is undefined, exactly as in the C the runtime compiles down to. The sanctioned paths provide the ordering they name: capture at `spawn` copies values into the thread's private environment, the sequentially consistent atomics in `std.concurrent.atomic` order the accesses they mediate, a `chan_recv` happens after the `chan_send` that delivered the value, a `complete` happens before the `await`, timed await, or poll that consumes the future it completed, an `unlock` happens before the next `lock` of the same mutex, and `join` orders everything the thread did before everything the joiner does after. Sharing built by hand out of `*raw T` buffers is on the raw layer's honor system across threads, exactly as it is within one, unless a mutex guards every touch.

The generational heap is thread safe, so `alloc` and `free` from any thread are defined, and the dereference check stays armed on every thread. In a program whose frees and uses are ordered by a sanctioned path, the check keeps its guarantee: a use after free, a double free, or a double `join` faults deterministically instead of corrupting memory. In a program that races, the check degrades to a best effort backstop. Checking and using are two steps, so a dereference racing the free of the same allocation can pass the check and then touch retired memory, and a fat pointer overwritten while another thread reads its sixteen bytes can tear into a mismatched pair. Freed blocks stay parked in the runtime's free list rather than returning to the operating system, which bounds the blast radius, but none of this makes a race defined. Code confined to the event loop's thread gets the stronger story for free: one thread orders every free against every use, so the check there is the deterministic single threaded guarantee, never the degraded mode.

---

## Imports and Standard Library

See [Source Files](#source-files-directives-imports-exports) for import syntax. Imports are separate from paradigm directives. Importing a module does not grant any paradigm.

### Standard Library Modules, Shipped and Planned

| Module                 | Description                                                     |
| ---------------------- | --------------------------------------------------------------- |
| std.io                 | print, println, printerr, file I/O                              |
| std.logging            | level gated logging to stderr, Debug through Error               |
| std.memory.arena       | arena allocator                                                 |
| std.memory.collector   | control and gauges for the collected heap behind `collector<T>` |
| std.functional.maybe   | Maybe<T> monad                                                  |
| std.functional.either  | Either<L, R> monad                                              |
| std.functional.result  | Result<T, E> monad                                              |
| std.functional.io      | IO<T> monad                                                     |
| std.vector             | dynamic array                                                   |
| std.map                | hash map                                                        |
| std.set                | a set over the generic map                                      |
| std.string             | string manipulation utilities                                   |
| std.flags              | register then parse command line flag parser                    |
| std.json               | JSON parse, emit, and deep free                                 |
| std.time               | civil and unix time, ISO 8601 format and parse, weekday         |
| std.unicode            | UTF-8 decode, encode, and validation over the byte view string  |
| std.math               | libm's scalar functions over float64, pi, e, is_nan, is_inf     |
| std.rand               | xoshiro256** pseudorandom generator, seeded through splitmix64  |
| std.concurrent.atomic  | sequentially consistent int64 atomics                           |
| std.concurrent.channel | bounded thread safe queue between threads                       |
| std.concurrent.pool    | the global thread pool behind the submit builtin                |
| std.concurrent.sync    | mutex and condition variable                                    |
| std.concurrent.thread  | sleep_ms beside the spawn and join builtins                     |
| std.async.future       | one shot futures: mint, complete, await, poll                   |
| std.async.loop         | the event loop's lifecycle                                      |
| std.async.time         | timers as futures the loop completes                            |
| std.async.io           | the readiness reactor, pipes, and non blocking read/write       |
| std.async.net          | TCP over the readiness reactor, non blocking connect and accept |

### Command Line Parsing with `std.flags`

`std.flags` parses a command line as a register then parse builder. A program builds a `Flags` on the heap with `flags_new(prog, about)`, registers each flag up front with `flag_bool`, `flag_str`, or `flag_int`, then hands the whole `argv` to `flags_parse(f, argv, start)`, which fills the registered values and collects the leftover positional words in order. There are no short flags and no grouping: a flag is always its long name matched as `--` followed by the registered name, which is stored without the dashes. Values read back through `flag_get_bool`, `flag_get_str`, and `flag_get_int`, whether a flag appeared through `flag_seen`, and the positionals through `flags_pos_len` and `flags_pos_at`.

The grammar reads `argv` from `start`, the index that skips the program name. A lone `--` ends flag parsing, so every token after it is positional even when it begins with a dash. A `--name=value` binds inline and a `--name value` binds the following token; a bool flag takes no value, so `--name` alone sets it true and `--name=x` is an error. Any token that does not begin with `--`, a single dash word or a negative number included, is a positional and keeps its order. A repeated flag is last wins, and `flag_seen` stays true across the repeat.

Bad input on the command line is an error value the caller must handle, not a fault. `flags_parse` returns an `error` naming the first bad token, `unknown flag '--frob'`, `flag '--timeout' needs a value`, `flag '--timeout' needs an integer, got 'abc'`, or `flag '--verbose' takes no value`, and the caller prints the message beside `flags_usage` and exits itself; the library never prints and never exits on a user's typo. A misuse of the library itself is a fault that aborts with a `fatal: flags:` prefix and no source location, the same contract `vec_get`'s bounds fault follows: a duplicate registration, `fatal: flags: flag '--verbose' already registered`; a getter on an unregistered name or the wrong kind, `fatal: flags: no int flag named '--verbose'`; and a positional index out of range, `fatal: flags: positional index out of bounds`.

`flags_usage(f)` returns a fresh heap string the caller owns, the usage line, the about line, then one line per flag in registration order with a `<str>` or `<int>` marker and the flag's default. It is a pure function of the registered flags, so its output is deterministic and a golden pins it byte for byte. `Flags` stores only borrows, the caller's own literals and the `argv` strings, both of process lifetime, so `flags_free` releases the two vectors alone and the caller frees the `Flags` allocation itself.

### Sets with `std.set`

`std.set` is `Set<T>`, an unordered set generic over its element type, a thin wrap over `std.map` keyed by the element with a `bool` value that is always true. The wrap is a real type rather than an alias, so a `*Set<string>` parameter cannot take a map by mistake, and it seals the value channel: membership is exactly key presence, and there is no way to hold an element that is present but false. `set_new`, `set_add`, `set_has`, `set_remove`, `set_len`, and `set_items` are the surface; `set_add` and `set_remove` return whether the call changed the set, and `set_items` returns the members in first insertion order as a fresh vector the caller owns, the `map_keys` contract verbatim.

The element contract is the map key contract verbatim: an element is any hashable type compared with `==`, and a string element is borrowed and must outlive the set. `set_free` releases the backing map's buffers and the map allocation only; the elements themselves are never freed, and the caller frees the `Set`.

### Mapping and Filtering a Vector

`std.vector` carries `vec_map(v, f)` and `vec_filter(v, p)`, both returning a fresh heap vector the caller owns while leaving the source untouched, the closure called exactly once per element in index order. `vec_map` applies `f` to each element and collects the results, whose type the compiler infers from the closure; `vec_filter` copies through the elements for which `p` answers true. Each takes a capturing lambda or a named function, exactly as `vec_sort` does, and neither is gated by a paradigm, since a standard library generic never touches the bare `map` and `filter` builtins the functional paradigm gates. `vec_sort` already takes its comparator as a closure, so there is no separate `vec_sort_by`.

`vec_filter` copies element values, so when `T` is a managed pointer type the returned vector holds the same pointers as the source and both vectors alias the pointed to objects; freeing an element reachable through both is a double free the ownership checker does not catch across `vec_filter`, and the runtime generational free check is the backstop that faults the stale read. `vec_map`'s results belong to whatever the closure returned: an allocating closure hands the caller owners, drained with `vec_take` before the outer vector is freed, while plain values and borrowed pointers need only the outer `vec_free` and `free`.

### Weekday and ISO 8601 Parsing in `std.time`

`weekday(c)` returns the day of week for a `Civil` value, 0 for Sunday through 6 for Saturday, the `tm_wday` convention. It shares `days_from_civil` with `unix_from_civil` and performs no validation of its own, so an out of range month or day field rolls forward through the same arithmetic; `1970-01-01` counts zero days and returns 4, Thursday, and a date before the epoch resolves correctly through the flooring modulo.

`parse_iso8601(s)` returns `(Civil, error)` and is the strict inverse of `format_iso8601`. It reads exactly `[-]Y{4,}-MM-DDTHH:MM:SSZ`, at least four ASCII digits of year with an optional leading `-`, two digits for every other field, an uppercase `T` and `Z`, and nothing after the `Z`. It accepts no fractional seconds, no timezone offset, and no lowercase spelling, since those are shapes `format_iso8601` never emits. As an input boundary it validates: the month is 1 to 12, the day is checked by round tripping through `civil_from_days(days_from_civil(y, m, d))` so a leap day is exact with no month length table, the hour is 0 to 23, and the minute and second are 0 to 59, so a literal leap second `:60` is rejected. A structural mismatch returns `iso8601: malformed timestamp` and an out of range field returns `iso8601: field out of range`. The law the roundtrip golden pins is `parse_iso8601(format_iso8601(c)) == c` for every representable `Civil`, negative and wider than four digit years included.

---

## Builtins

Builtins are always available regardless of paradigm directives unless noted.

### Always Available

| Builtin   | Signature                               | Description                                                       |
| --------- | --------------------------------------- | ----------------------------------------------------------------- |
| alloc     | alloc(value?) -> \*T                    | heap allocate through the in scope allocator                      |
| free      | free(p: \*T) -> void                    | deallocate through the in scope allocator                         |
| print     | print(...) -> void                      | print to stdout, handles all primitive types                      |
| println   | println(...) -> void                    | print to stdout with a newline                                    |
| printerr  | printerr(...) -> void                   | println to stderr                                                 |
| sizeof    | sizeof(T) -> int64                      | size of a type in bytes at compile time                           |
| hash      | hash(v) -> int64                        | a deterministic 64-bit hash of a hashable scalar or string        |
| int8      | int8(v) -> int8                         | numeric cast to int8                                              |
| int16     | int16(v) -> int16                       | numeric cast to int16                                             |
| int32     | int32(v) -> int32                       | numeric cast to int32                                             |
| int64     | int64(v) -> int64                       | numeric cast to int64                                             |
| char      | char(v) -> char                         | numeric cast to char                                              |
| rune      | rune(v) -> rune                         | numeric cast to rune                                              |
| float32   | float32(v) -> float32                   | numeric cast to float32                                           |
| float64   | float64(v) -> float64                   | numeric cast to float64                                           |
| spawn     | spawn(f: () -> void) -> (thread, error) | start an OS thread running a lambda literal                       |
| join      | join(t: thread) -> error                | wait for a thread; retires the handle                             |
| submit    | submit(f: () -> void) -> error          | queue a lambda literal on the global thread pool                  |
| async_run | async_run(g(args)) -> T                 | crank the event loop until a direct async call's future completes |

`alloc` and `free` resolve to the in scope allocator. See [Memory Management](#memory-management).

`async func` and `await` are keywords, not builtins, added in 0.4.2 and gated behind no paradigm, the same as `spawn` and `submit`. `async func` marks a function's task frame and state machine transform; `await` suspends inside one, in exactly the four statement positions [the async chapter](#threads-and-the-memory-model) names. `async_run` is a builtin like `alloc`, callable from any file regardless of paradigm, but only outside an async body; see the async chapter for its rules and the whole keyword layer.

### Numeric Casts

Added in 1.2.0 as integer width casts, widened in 1.5.0 to the whole numeric set. `int8(v)`, `int16(v)`, `int32(v)`, `int64(v)`, `char(v)`, `rune(v)`, `float32(v)`, and `float64(v)` convert a scalar value explicitly to the named type. The source may be any integer family value, an `int` of any width, a `char`, a `rune`, or a `bool`, or a `float32` or `float64`. The conversion follows the operand and target kinds:

- integer to integer: two's complement truncation going down, sign or zero extension going up, `char` and `bool` extending by magnitude rather than sign, the same coercion an annotated widening or narrowing assignment uses.
- integer to float: the integer's value as a float, exactly when it fits the significand, `char` and `bool` read as a magnitude.
- float to integer: truncation toward zero, so `3.9` casts to `3` and `-2.9` to `-2`. A magnitude beyond the target integer's range saturates to its nearest bound rather than wrapping or leaving the result undefined, and a NaN casts to zero, so a misused cast stays deterministic instead of undefined the way C's own conversion is.
- float to float: `float32` widens to `float64` exactly, `float64` narrows to `float32` with rounding.

```text
int32(300)      // 300, fits
int8(300)       // 44, truncated: 300 mod 256 read as signed
char(101)       // 'e'
int64(x)        // widens an int8 x, sign extending if x is negative
float64(7)      // 7.0
int64(3.9)      // 3, truncated toward zero
rune(65)        // the rune U+0041, the codepoint 65
float32(2.5)    // the double 2.5 as a single
```

A pointer, a string, a struct, or any other non scalar keeps its reject, `a numeric cast takes an integer, char, rune, or float value; <type> does not cast`. Each cast takes exactly one argument; any other count is `<name>(v) takes exactly one value`. A cast is an unchecked numeric conversion: `rune(v)` in particular accepts any 32 bit value, including a negative or a value above U+10FFFF that no Unicode scalar carries, the same way an assignment already could; nothing validates the result as a scalar, and a consumer that needs one checks it itself.

The eight names are reserved as builtin call forms: a function cannot be declared with one of them, `'int32' is a primitive type name; a function cannot take it`, since a call to the name would otherwise be ambiguous between the cast and the function. A variable, a struct field, or any other binding may still use one of the names; only a function declaration collides.

### Hash

Added in 1.5.1. `hash(v)` returns a deterministic 64-bit hash of a hashable value, the key hash a generic hash map builds on. The operand must be hashable: an integer of any width, a `char`, a `rune`, or a `string`. A float is refused, since a NaN is never equal to itself and would break the coherence between a hash and the equality a map compares keys by; a struct, a pointer, a slice, and every other non scalar is refused too, `cannot hash <type>; a map key is an integer, char, rune, or string`. In a generic function `hash(k)` over a type parameter passes the surface pass and a non hashable instantiation is rejected once its ground type is known, the same two pass shape a comparison over a generic type takes.

A `string` hashes by its content, not its pointer, so two strings with the same bytes hash equal however they were built, matching the way `==` compares strings by content. An integer, char, or rune hashes to its own value widened, so equal values hash equal. The exact hash value is unspecified and may change between releases; a program depends only on the guarantee that equal values hash equal and that the hash is stable within one run. `hash` reserves its name from a function declaration the same way the cast builtins do.

### Functional Builtins (require `@paradigm functional`)

| Builtin | Description                        |
| ------- | ---------------------------------- |
| map     | applies a function to each element |
| filter  | filters a collection by predicate  |
| reduce  | reduces a collection to one value  |
| fold    | fold left or right                 |
| foreach | iterates for side effects          |

### Procedural Builtins (require `@paradigm procedural`)

| Builtin or Keyword | Description                       |
| ------------------ | ---------------------------------- |
| for                | for loop                           |
| while              | while loop                         |
| do while           | do while loop                      |
| mut                | declares a mutable variable        |
| break              | jumps past the innermost loop      |
| continue           | jumps to the innermost loop's next iteration |

`for x in xs` takes an array, a slice, or a string. Over a string it iterates the bytes as `char`, front to back, with the string's length read once by a NUL scan at loop entry rather than rechecked each iteration. A source with no element type is rejected at check, `cannot iterate <type>; a for loop takes an array, a slice, or a string`, rather than being accepted and failing to link.

`break` and `continue`, added in 1.2.0, are reserved keywords rather than plain identifiers, gated to `@paradigm procedural` the same as the loop forms themselves. Each binds to the innermost enclosing loop, a `while` or a `for`: `break` exits it immediately, and `continue` skips the rest of the current body and jumps straight to the next iteration. In a `for` loop, `continue` jumps to the loop's own index increment rather than back to the top of the body, so the index still advances on a skipped iteration instead of the loop stalling on the same element. A lambda body is its own function boundary and lends neither statement a target, even when the lambda executes from inside an enclosing loop: a `break` or `continue` written inside a lambda's own body is checked against that lambda's nesting, not the loop the lambda happens to run under. Used outside any loop, each is a compile error naming the rule: `break is only legal inside a loop`, `continue is only legal inside a loop`.

```text
mut i: int64 = 0
while true {
    if i == 3 { break }
    i = i + 1
}

for x in xs {
    if x < 0 { continue }   // skip, but the loop's own index still advances
    if x > 100 { break }
    sum = sum + x
}
```

### Display Interface

Any type that implements the `Display` interface can be passed to `print` and `println`.

```text
interface Display {
    toString() -> string;
}
```

Passing a struct with no `Display` impl to a print builtin is a compile error, as is printing an enum, a tuple, or a pointer. Print never emits silence for a value it cannot render. A slice is not printable, with one exception: a `char[]`, like a `char[N]` and a `char` themselves, prints its bytes as text rather than being rejected; see [Strings](#strings) for the rule and the exact bytes each of the three writes.
