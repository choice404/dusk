# Dump Commands

The `dusk` binary has five front end dump commands used as interchange points
for bootstrap checks. `lex`, `scan`, and `parse` each read exactly one source
file. `load` and `desugar` read the root file plus every file it transitively
imports, since both run the loader that resolves `@import` directives and
merges the result into one module. Every one of the five writes its dump to
stdout, writes diagnostics to stderr, and exits non zero when that stage
reports errors.

The dump contract is stdout plus process status. A consumer that compares two
compilers should compare stdout byte for byte and compare the exit code. Stderr
is diagnostic text for a human and is not part of this contract.

The exit contract is the same across all five commands, but it is worth stating
precisely for `load` and `desugar`, where it is easy to assume a printed dump
means a clean run: a dump prints whenever this stage produced a module, and the
exit code is non zero whenever this stage recorded any diagnostic, and those two
facts are independent. `parse` always has a module to print, since the parser
recovers rather than aborting on an error, so its dump prints unconditionally
and only the exit code reports whether lexing or parsing found anything wrong.
`load` and `desugar` only have a module to print once the root file itself
lexes, parses, and passes its own paradigm check; past that point an unresolved
import, a private name reached through a module namespace, or a paradigm
violation in an imported file is recorded as an error and still lets the merge
finish and the module print, so a `load` or `desugar` dump can appear on stdout
in the same run that exits non zero. A paradigm violation in the root file
refuses the module outright, and nothing prints.

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

`dusk parse <file>` runs the lexer and parser and prints the parsed module
through a canonical renderer, not through Rust's derived `Debug`.

```text
{render_module(&module)}
```

The renderer is hand written, and it mirrors the shape Rust's own pretty
`Debug` would produce for `parser::ast::Module`: the same struct and tuple
variant names, four space indentation, a trailing comma after every field and
every list element, an empty vector printed as `[]`, and a present `Option`
value printed as a multi line `Some(...)` the same way the derive breaks it
across lines. A boxed node dumps as the value inside it, with no `Box` wrapper
ever appearing in the text, and a `Span` always prints as `Span { lo: N, hi: N
}`, its two stored byte offsets.

Three leaf forms replace what the derive would print, because the derive's own
formatting is not something a second, non Rust compiler can reproduce byte for
byte:

- A float literal prints as `Float(0x{16 hex digits})`, the uppercase hex of
  the value's IEEE 754 bits, the same rule the lex dump uses for the same
  reason: equal values print equal text with no shortest decimal rounding to
  match, and the literal's own type suffix, already a separate field next to
  it, stays out of this form.
- A string literal escapes to `"..."` and a char or rune literal escapes to
  `'...'`, through the same escaper the lex dump uses: a scalar in the
  printable ASCII range, `0x20` to `0x7e`, passes through unchanged, and
  everything else becomes `\u{hex}` with the lowercase, minimal width code
  point.
- An identifier is plain ASCII in this language, so it prints unchanged as a
  quoted string; there is no separate identifier form to reconcile against the
  string form above.

`dusk load <file>` resolves the root file's `@import` directives, transitively,
parses every file that reaches, merges all of their items into one module, and
prints that merged module through the same renderer `parse` uses.

```text
{render_module(&merged_module)}
```

Loading needs a real path on disk rather than stdin, since resolving an import
walks the importing file's own directory first and then the stdlib root beside
the compiler binary, or the tree `DUSK_HOME` points at when it is set, looking
for the imported file. The merge folds a qualified call like `std.io.println`
down to the bare global it names once every imported namespace is known, and
that fold, along with import resolution and each file's own paradigm check,
runs before the module is handed back for printing; see the exit contract above
for what happens when one of those steps fails partway through.

`dusk desugar <file>` loads the file exactly the way `load` does, then runs the
`do` block rewrite over the merged module before printing it.

```text
{render_module(&desugared_module)}
```

The rewrite is the same one `desugar::run` performs ahead of resolve and
typeck: every `do { x <- m; ... }` monadic block becomes nested calls to that
monad's `bind` and `unit` builtins. The dump shows that expanded shape, the one
a differential oracle at this stage actually has to match, not the surface
`ExprKind::Do` node the parser produced; no `Do` node reaches the printed tree,
every one is rewritten before the dump runs.

## Merging and Item Spans

Merging rebases spans so that every file's tokens land at a distinct offset in
the merged program's coordinate space, one file after another with a one byte
gap between them, but the rebase does not walk every span in a merged file. It
only walks into a function body: the statements and expressions inside a
`Func`'s body move, and so do the ones inside each method body of an `Impl`,
but the `Func`, `Impl`, and `Foreign` item's own `span` field, the one that
marks the whole item for a diagnostic about it as a block, is left exactly as
the parser first wrote it, still in that file's own, unrebased coordinates.
Every span recorded in `Module.monads` is left the same way.

A `load` or `desugar` dump on any program with more than one file therefore
mixes two coordinate spaces in the same tree on purpose: a span inside a
function or method body carries the merged, program wide offset, and an item
level span, or a monad declaration's span, still carries the single file
offset it had before the merge. This is not an oversight to quiet by rebasing
the rest, and it is not a bug the parse dump would show, since `parse` only
ever sees one file and never rebases anything. It is part of the dump contract
itself: a second compiler's loader has to shift exactly the same nodes and
leave exactly the same ones alone, or its `load` and `desugar` dumps disagree
with this compiler's even on a program whose parsed AST shape actually
matches, file by file.

## Stability

For the same binary version and the same input bytes, each dump is byte
identical across runs. The printed structures are ordered vectors and scalar
fields. The dump path does not print hash collection iteration, pointer
addresses, thread ids, wall clock values, process ids, or random values.

Each dump is a pure function of the source bytes it reads, plus, for `load` and
`desugar`, `DUSK_HOME`. `lex`, `scan`, and `parse` read exactly one file's
bytes and nothing else, so the file's bytes alone determine the dump. `load`
and `desugar` additionally resolve every `@import` against the stdlib root
beside the compiler binary, or the tree `DUSK_HOME` names instead when it is
set, so the same root file can merge a different set of files, and print a
different dump, under a different `DUSK_HOME`. Hold the source bytes at every
path the loader touches and the value of `DUSK_HOME` fixed, and a `load` or
`desugar` dump is exactly as deterministic as `lex`, `scan`, or `parse`.

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

Run the same comparison with `scan`, `parse`, `load`, and `desugar` when
isolating a difference. `lex` localizes tokenization and newline flag
differences, `scan` localizes directive handling, `parse` localizes AST shape
differences within a single file, `load` localizes import resolution, the
merge itself, and the span rebasing described above, before any one file's own
parsed shape is the suspect, and `desugar` localizes the monadic `do` rewrite
on top of an already agreed merge, before semantic analysis or code generation
can hide the source.
