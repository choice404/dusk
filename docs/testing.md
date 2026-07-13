# Testing

## About

`testrun` is a golden test runner for dusk, written in dusk. It reads a manifest
of test records, runs each one through a compiler you name, and checks the
program's stdout, stderr, and exit code against what the record expects. It is the
manifest driven counterpart to the Rust golden suite in `tests/examples.rs`: the
Rust suite hard codes each case as a function, and `testrun` reads the same kind
of cases out of a flat text file so a case is data, not code.

The runner is itself a small dusk program under `tests/runner/`, so it doubles as
a working example of the language surface it exercises: records live in a growable
vector, values are compared with string equality and substring search, and the
whole command line is built with a string builder.

## Building and running

Build the runner with the compiler, the same way you build any dusk program. The
binary takes the stem of its root file, so it lands at `target/dusk-out/testrun`.

```sh
./target/release/dusk build tests/runner/testrun.dusk
```

Run it against a manifest. The compiler under test comes from `--bin` or the
`DUSK_BIN` environment variable, and there is no default, so every run names the
binary it exercised. Set `DUSK_HOME` so the compiler under test finds its stdlib,
and run from the repository root, since every path in a manifest is relative to
it.

```sh
DUSK_HOME=$PWD DUSK_BIN=target/release/dusk target/dusk-out/testrun tests/goldens.manifest
```

The command line is:

```
testrun <manifest> [--filter substr] [--timeout secs] [--bin path] [--limit-kb n]
```

- `--filter substr` runs only the records whose name contains `substr`.
- `--timeout secs` caps each command with `timeout`, defaulting to 60 seconds. A
  command that runs past the cap reports 124 and fails, naming the timeout.
- `--bin path` names the compiler under test, taking precedence over `DUSK_BIN`.
- `--limit-kb n` caps the child's virtual memory with `ulimit -v` in kilobytes, a
  guard against a runaway compile. It is off by default; set it deliberately,
  since a real compile and link can want a lot of memory.

The runner prints the selected count first, then one line per failure to stderr,
then a tally to stdout:

```
testrun: selected 596 tests
testrun: 596 tests, 593 passed, 3 failed
```

The exit code is 0 when nothing failed and 1 when anything did. A malformed
manifest exits 1 after printing its errors, and a bad command line or an unreadable
manifest exits 2.

## Manifest format

A manifest is a flat text file of records. A line of exactly three dashes, `---`,
separates one record from the next. A line beginning with `#` is a comment, and a
blank line is ignored, so a record is the run of field lines between two
separators with comments and blanks skipped.

Each field line is a key, a single space, and a value. The value is the rest of
the line verbatim, so a value keeps its own leading and trailing spaces. A value
carries three escapes and no others:

- `\n` for a newline.
- `\t` for a tab.
- `\\` for a backslash.

Any other escape, or a backslash at the end of a value, is an error. A raw NUL
byte anywhere in the file is rejected, since a NUL would truncate the read and
hide the rest of the file.

The parser collects every error it can find and reports them all before the runner
executes a single command, so a malformed manifest fails loudly rather than
passing as an empty run. An unknown field, an unknown mode, a missing required
field, a bad escape, a duplicate scalar field, a duplicate test name, and a record
that does not begin with a `test` field are each named with the line they sit on.

### Fields

| field | meaning |
|---|---|
| `test` | the record name, required and first in the record |
| `mode` | the verdict mode, one of the seven below, required |
| `file` | the source file to compile, relative to the repository root |
| `args` | extra argv words appended after the file, split on spaces |
| `stdin` | text fed to the program on standard input |
| `out` | the exact expected stdout |
| `ok` | `true` or `false`, the expected clean exit for `run_raw` |
| `err_has` | a substring stderr must contain; repeatable, checked in order |
| `err_absent` | a substring stderr must not contain; repeatable, used by `check_fail`, `run_raw`, and `build_fail` |
| `err_eq` | the exact expected stderr |
| `argv` | the words passed to the compiler in `tool` mode |
| `code` | the expected exit code in `tool` mode |
| `special` | the name of a builtin check for `special` mode |

The `test` field is required and must be the first field of a record. Depending on
the mode, `file`, `argv`, or `special` is also required. A field whose empty value
still means something, `out`, `stdin`, and `err_eq`, is distinguished from an
absent field: an empty `out` expects empty stdout, which is not the same as no
`out` field at all.

### Exact matches

An exact match, `out`, `err_eq`, and the `out` of a `tool` record, checks two
things: the bytes agree, and the true byte count agrees. The byte count is
measured with `wc -c`, not read off the captured string, so a stream that ends in
a NUL followed by more bytes never reads equal to a shorter expected value the way
a plain C string compare would. A value the manifest expects never holds a NUL, so
its own length is exact.

A substring match, `err_has`, runs through the standard library's substring
search. The list is checked in order, and a failure names the first fragment that
did not appear.

A forbidden match, `err_absent`, is its negative counterpart: stderr must not
contain any of its fragments. A single fragment that appears is a failure, and the
report names which one. It reads the same way a Rust golden pins a diagnostic that
must not mention a wrong cause, for instance a collect reject that must name the
collect and must not mention a channel. `err_absent` applies to `check_fail`,
`run_raw`, and `build_fail`, the modes that inspect stderr, and is repeatable.

## Modes

Each mode checks exactly what its contract promises and nothing more. A run mode
never inspects stderr, and a check mode never inspects stdout.

| mode | command | passes when |
|---|---|---|
| `run` | `<bin> run <file> <args>` | exit is 0 and `out` matches when present |
| `run_raw` | `<bin> run <file> <args>` | the clean bit matches `ok`, `out` matches when present, every `err_has` is present, no `err_absent` appears, and `err_eq` matches when present |
| `check_fail` | `<bin> check <file>` | exit is non zero, every `err_has` is present, and no `err_absent` appears |
| `check_ok` | `<bin> check <file>` | exit is 0 |
| `build_fail` | `<bin> build <file>` | exit is non zero, every `err_has` is present, and no `err_absent` appears |
| `tool` | `<bin> <argv>` | exit matches `code` when present and `out` matches when present |
| `special` | a builtin | the builtin reports success |

`run` expects a clean exit, so a timeout, a signal death, or any non zero code
fails and the report names the reason. `run_raw` is for a program that faults on
purpose, so its `ok` field says whether a clean exit is expected: a use after free
or an out of range slice aborts with a non zero code, which `ok false` accepts,
and its fault message is then matched with `err_has`.

A `stdin` field writes its value to a file and redirects it into the command, for
any mode. An `args` field appends its words to a `run` command as the program's own
argv, after the file.

## Specials

A special is a check that needs more than one command and so cannot be written as a
single run line. The special's name selects its handler.

- `installed_layout` builds a throwaway install prefix, copies the compiler under
  test into its `bin`, and copies the stdlib and runtime under
  `share/dusk-lang`, exactly the packaged layout. It then compiles and runs a
  hello program from inside the prefix with `DUSK_HOME` unset, so the only way the
  compiler finds its assets is the walk from its own executable up to the share
  directory. This mirrors the Rust golden of the same name.
- The `dawn_*` family is named but not yet implemented, since the dawn package tool
  defines the interface those checks will drive. Each fails loudly rather than
  passing as a no op, and an unrecognized special name fails the same way.

## The selftest

`tools/testrun-selftest.sh` proves the runner itself. It builds the runner and
runs it against `tests/selftest.manifest`, which carries one passing record and one
deliberately failing record for every mode, plus a pair for the `err_absent`
assertion. The script asserts that the failing set is exactly the deliberate one
and that the tally is eight passed and eight failed, so a green run means the
runner accepts a correct expectation and rejects a wrong one across every mode.

```sh
tools/testrun-selftest.sh
```
