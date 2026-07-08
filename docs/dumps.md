# Dump Commands

The `dusk` binary has three front end dump commands used as interchange points
for bootstrap checks. Each command reads one source file, writes its dump to
stdout, writes diagnostics to stderr, and exits non zero when that stage reports
errors.

The dump contract is stdout plus process status. A consumer that compares two
compilers should compare stdout byte for byte and compare the exit code. Stderr
is diagnostic text for a human and is not part of this contract.

## Formats

`dusk lex <file>` runs the lexer and prints one line per token, including the
EOF token.

```text
{span.lo:>4}..{span.hi:<4} nl_before={true|false} {kind}
```

`span.lo` and `span.hi` are byte offsets in the input. `nl_before` is the parser
visible newline flag carried by the token. `{kind}` is the token kind in the dump
form described below.

Most kinds print their Rust `Debug` form, which is already a plain scalar or an
ordered list a second compiler reproduces from the same data. Three kinds carry a
value that a textual second compiler cannot reproduce from Rust's own `Debug`, so
they take a canonical form built only from the value itself:

- A float prints as `Float(0x{16 hex digits})`, the uppercase hex of the value's
  IEEE 754 bits. Equal values print equal text, so no shortest decimal rounding
  has to be matched, and the literal's type suffix is not part of this form.
- A string, char, or rune escapes every scalar that is not printable ASCII, the
  range `0x20` to `0x7e`, as `\u{hex}` with the lowercase, minimal width code
  point, and passes a printable ASCII scalar through unchanged. `Str`, `Char`, and
  `Rune` share this rule, so the escaping needs no Unicode property tables, only
  the code point.

`dusk scan <file>` runs the pre scan pass and prints the effective paradigm set
and imports.

```text
paradigms: {effective_paradigms:?}
imports:
  {import}
  {import}
```

The paradigm list is the `Debug` format of `Prescan::effective()`. A file with
no `@paradigm` directive prints `[Procedural]`. Import lines are printed in
source order, one import per line, with two leading spaces. If there are no
imports, the `imports:` header is followed by no import lines.

`dusk parse <file>` runs the lexer and parser and prints the parsed module.

```text
{module:#?}
```

This is the pretty `Debug` format for `parser::ast::Module`. All AST node types
reachable from `Module` derive `Debug`, so every stored field in the parsed AST
is included. Float literals use Rust's `Debug` formatting for `f64`, which is
locale independent and round trip safe for the stored value.

## Stability

For the same binary version and the same input bytes, each dump is byte
identical across runs. The printed structures are ordered vectors and scalar
fields. The dump path does not print hash collection iteration, pointer
addresses, thread ids, wall clock values, process ids, or random values.

The contract does not promise that future dusk versions keep the same text. A
future version may add fields, add enum variants, change field order, or change
spacing when the compiler data model changes. Consumers should treat the dump
format as versioned with the compiler binary they are comparing.

## Differential Use

A bootstrap oracle compares two compiler binaries on the same input at the same
pipeline stage. For example, a stage-N compiler and the stage-(N-1) compiler
that built it should produce identical dumps for each source file.

```sh
stage_a=target/release/dusk
stage_b=target/bootstrap/stage1/dusk

"$stage_a" lex examples/app.dusk > /tmp/a.lex
"$stage_b" lex examples/app.dusk > /tmp/b.lex
diff -u /tmp/a.lex /tmp/b.lex
```

Run the same comparison with `scan` and `parse` when isolating a difference.
`lex` localizes tokenization and newline flag differences, `scan` localizes
directive handling, and `parse` localizes AST shape differences before semantic
analysis or code generation can hide the source.
