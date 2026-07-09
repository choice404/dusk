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

## The Mono and Escape Dumps

`dusk mono <file>` runs the full front end, load, desugar, and semantic
analysis, and on a clean result prints the ground module, the one
monomorphization produced, through the same renderer `parse` and `load` use.

```text
{render_module(&analysis.ground_module())}
```

Ground here means every generic is gone: mono has already expanded each
distinct type argument set to its own concrete copy and given that copy a
mangled name, so a call the source spells `id<int64>` prints as a call to
whatever mangled name mono picked for that instantiation, and the mangled name
is the name, with no unmangling step anywhere in the dump. `mono` diverges
from `parse`, `load`, and `desugar` in one respect worth stating plainly: it
only has a module to print when the front end is clean. A diagnostic from
resolve, typeck, or monomorphization writes to stderr the same way `check`'s
does, but stdout stays empty and the process exits non zero; there is no
ground module to show for a program the checker rejected, since monomorphizing
a rejected module never happens.

The mono dump is the handoff point between semantic analysis and code
generation. Two compilers that already agree on `check`'s verdict agree only
that they accept or reject the same programs. A byte identical `mono` dump
goes further: it proves that what codegen would actually receive as input, the
concrete, fully monomorphized tree with every mangled name resolved, is one
and the same, before either compiler's code generator is trusted at all.

`dusk esc <file>` loads and desugars a file the way `desugar` does, then runs
the interprocedural escape summary pass over the desugared module and prints
its result. Escape summaries are compute only data, an oracle for the flow
across call boundaries that typeck's ownership and escape enforcement
consults at every call site, not a stage that reports its own diagnostics, so
`esc` prints unconditionally once loading and desugaring succeed and fails the
same way `load` and `desugar` fail when a source file itself is unclean.

```text
{render_escape_info(&summary::compute(&desugared_module))}
```

The dump is one line per fact, four kinds of line: a `fn` line for the
summary computed for every free function, a `method` line for the summary
computed for every impl method, a `lambda` line for a fact keyed by a lambda
literal's own span, and a `store` line for one direct, frame local store the
walk found.

```text
fn {name} returns_alias={mask} reads_through={mask} sinks={mask} collect_sinks={mask} flows=[{(i,j)},...]
method {recv}#{name} returns_alias={mask} reads_through={mask} sinks={mask} collect_sinks={mask} flows=[{(i,j)},...]
lambda {lo}:{hi} {table}={value}
store {lo}:{hi} {param_index}
```

Every kind sorts before printing, so a hash map's iteration order never
reaches the page: `fn` lines sort by function name, `method` lines by
`(receiver, name)`, the four lambda tables interleave into one list and sort
by `(span.lo, span.hi, table name)`, and `store` lines come out of the walk
that builds them already sorted by `(span.lo, span.hi, param index)`, so
render adds no sort of its own there.

A `{mask}` is a `ParamSet`, a bitmask over parameter indices backed by a
`u64`, printed with lowercase hex and a `0x` prefix, `0x0` for the empty set.
A function with more than 64 parameters is beyond any real program the
analysis is built for, so an index past that bound saturates the set to its
top rather than silently dropping it: the mask overstates the relation
instead of understating it, which is the direction that keeps the analysis
sound. A saturated mask prints as `0xffffffffffffffff`, every bit set. A
`flows` field is a list of `(i,j)` pairs, parameter `i`'s view flowing into
parameter `j`'s place, sorted and deduplicated before printing. The four
lambda table names, `lambda_returns`, `lambda_sinks`, `lambda_collect_sinks`,
and `lambda_capture_flows`, are the literal name of the table a lambda's fact
belongs to; a `lambda_capture_flows` value is a list of `(param_index,
capture_name)` pairs in place of a mask, since a capture flow is keyed by the
outer variable's own name rather than by a parameter index alone.

## The `check` Differential Contract

`check` produces no dump: a clean run prints `ok: {path}` to stdout and
nothing else, and a rejected program prints nothing to stdout and its
diagnostics to stderr. `mono` shares that same empty stdout on a rejected
program, but `tools/differential.sh` still runs a plain byte for byte stdout
comparison for `mono`, the same rule every other dump command gets; on a
rejected program that comparison is trivially satisfied, since both
compilers' stdout is empty, and the only thing actually gated on a `mono`
divergence is the shared exit code check every command in the sweep runs
first. `check` gets a contract of its own, narrower than the plain byte for
byte rule, built specifically around the fact that its only output, once a
program is rejected, lives in stderr rather than on stdout.

The verdict is the hard gate: both compilers must exit zero, or both must
exit non zero, on the same input, every time. A verdict mismatch is a
divergence and stops the sweep outright.

When both compilers accept a program, stdout is compared byte for byte the
same way a dump's stdout is, `ok: {path}` against `ok: {path}`. When both
compilers reject a program, the two stderr streams are not compared byte for
byte, because diagnostic wording is not frozen: a message can be reworded,
reordered, or improved in either compiler without that change meaning
anything went wrong. What has to agree is where the checker stopped and on
what kind of problem, not how it phrased the sentence about it. The sweep
extracts a `{path}: {line}:` prefix from the first `error:` line in each
compiler's stderr and requires the two prefixes to match; a mismatch here is
a divergence and stops the sweep the same as a verdict mismatch. The full
`(path, line)` multiset across every diagnostic in each stderr is also
compared, but a mismatch there prints only an `advisory:` line and does not
fail the sweep, since a diagnostic one compiler reports past the first
already agreed error, and the other does not, is a difference in how far each
checker kept going after the first failure, not a difference in what it found
wrong first.

The reasoning matches the reasoning behind never diffing diagnostic text
elsewhere in this project: a diagnostic's location and its pass or fail
verdict are semantic, part of what the language actually promises a program
means, while its exact wording is not, and freezing the wording would punish
making an error message clearer later. Gating on the location prefix and the
verdict, and only advising on the rest, lets two compilers go on disagreeing
about phrasing forever while still proving they agree on the only two facts
that matter: whether a program is accepted, and if not, the first place it
goes wrong.

## The Sema Corpus

`tests/sema_corpus/` is a fixed corpus of `.dusk` programs, one file per
case, split into subdirectories, `summary/`, `typeck/`, and `mono/` at the
time of writing, each aimed at one part of semantic analysis, alongside a
single `manifest.tsv` recording what `dusk check` does to every file in the
corpus.

The manifest is three tab separated columns, one row per file, in the
corpus's own sorted path order:

```text
{path}\t{exit}\t{first_diag_prefix}
```

`{path}` is the file's path relative to the repository root. `{exit}` is the
exit code `dusk check` produced for that file, `0` or `1`. `{first_diag_prefix}`
is the same `{path}: {line}:` prefix the `check` differential contract gates
on above, taken from the first `error:` line in that run's stderr; a file
that exits `0` has no such line, so its third column is empty.

The manifest is generated, never hand edited: `tools/sema_manifest.sh
<binary>` runs `<binary> check` over every file in the corpus and rewrites
`manifest.tsv` from scratch. Adding a case, or changing an existing case's
expected outcome on purpose, goes through the script, not a manual edit to
the tsv; nothing reads a hand edited row as authoritative, and the next run
of the script overwrites it anyway, so a hand edit never actually holds.
