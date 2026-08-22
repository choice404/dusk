# Testing

## About

`testrun` is a golden test runner for dusk, written in dusk. It reads a manifest
of test records, runs each one through a compiler you name, and checks the
program's stdout, stderr, and exit code against what the record expects. It is the
manifest driven successor to the golden suite that shipped with the retired Rust
implementation: that suite hard coded each case as a function, and `testrun`
reads the same kind of cases out of a flat text file so a case is data, not code.

The runner is itself a small dusk program under `tests/runner/`, so it doubles as
a working example of the language surface it exercises: records live in a growable
vector, values are compared with string equality and substring search, and the
whole command line is built with a string builder.

## Building and running

Build the runner with the compiler, the same way you build any dusk program. The
binary takes the stem of its root file, so it lands at `target/dusk-out/testrun`.

```sh
./target/dusk-out/dusk build tests/runner/testrun.dusk
```

Run it against a manifest. The compiler under test comes from `--bin` or the
`DUSK_BIN` environment variable, and there is no default, so every run names the
binary it exercised. Set `DUSK_HOME` so the compiler under test finds its stdlib,
and run from the repository root, since every path in a manifest is relative to
it.

```sh
DUSK_HOME=$PWD DUSK_BIN=target/dusk-out/dusk target/dusk-out/testrun tests/goldens.manifest
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
- The `dawn_*` family drives the dawn package tool against a project, a directory
  holding a `package.dawn` manifest. Each of these builds its own git fixture
  repositories and its own consuming project under `target/testrun/dawn_<name>/`,
  requires the fixtures through a `file://` URL so no check touches the network, and
  reads the result off disk. `DAWN_CACHE` is pointed at `dawncache` inside that same
  work directory, which is emptied before the check runs, so the tool's bare mirror
  cache never reads or writes the machine's own and every check starts cold. An
  unrecognized special name, `dawn_*` or not, fails rather than passing as a no op.

The tool under test is `DAWN_BIN`, or the file named `dawn` beside the compiler under
test when `DAWN_BIN` is unset, since the suite builds both into the same output
directory. A tool that is missing or not executable fails every dawn special by name,
so build one before a full run:

```sh
DUSK_HOME=$PWD target/dusk-out/dusk build compiler/dawn.dusk
```

| special | proves |
|---|---|
| `dawn_get_lock` | `dawn get` clones the required package, stamps the clone in `dawn_modules/<alias>/.dawn` with the source, the ref, and the commit, writes one `@lock <alias> <source> <ref> <commit>` line whose commit is the one the tag resolves to, and a second `get` rewrites nothing |
| `dawn_run_alias` | a root that imports a required package as `<alias>.<module>` builds into `target/dawn-out` inside the project and runs, and the value the required module returns reaches the program's output |
| `dawn_run_args` | the words after `dawn run` reach the program as its own argv, in order |
| `dawn_transitive` | a required package's own requires are fetched into the same flat `dawn_modules`, locked beside the direct one, and a root composed across both packages runs |
| `dawn_update_moves_pin` | with the manifest edited to a newer tag, building is refused while the lock still pins the old commit, and `dawn update <alias>` moves the pin so the program prints the newer package's line |
| `dawn_missing_lock_refused` | a manifest that requires a package with no lock beside it is refused by `dawn build` and by `dusk build` with no file argument, each naming `run dawn get` |
| `dawn_stale_stamp_refused` | a clone whose stamp no longer matches the lock is refused by alias, naming `dawn get`, rather than built |
| `dawn_add_edits_manifest` | `dawn add <alias> <source> <ref>` writes the require into the manifest and then fetches it, so the manifest, the lock, and the clone all name the new package |
| `dawn_init_layout` | `dawn init <name>` writes a manifest declaring `@package <name>`, a `.gitignore` holding `dawn_modules/`, and a root that runs and prints a line naming the package |
| `dawn_ir_parity` | one fetched project copied into two directories emits byte identical IR, the required module's fault locations are spelled under `dawn_modules/<alias>/` relative to the manifest rather than by absolute path, and `dusk build` with no file argument takes the manifest root |
| `dawn_alias_collision` | one alias claimed by two different sources, one of them through a transitive require, is refused by that alias instead of resolved to whichever arrived last |
| `dawn_cache_offline_get` | the first fetch of a source leaves a bare mirror under `DAWN_CACHE`, and with the fixture repository moved out of reach a second project requiring that same source still fetches, locks, and runs, which only the mirror can answer |
| `dawn_tree_prints_graph` | `dawn tree` prints the package and its version, then every requirement at one step of indentation per level with the source, the ref, and the first twelve of the locked commit, marking a deleted checkout `(not fetched)` and a requirement no lock line pins `(not locked)` |
| `dawn_tampered_checkout_refreshed` | a checkout edited under the tool, detached at another commit, or deleted outright is replaced by the next `get`, which says `refreshing <alias>` rather than reporting the tree as cached on the strength of its stamp |

A refusal is read on stderr, where dawn and the compiler both put their error lines,
and its exit code must be non zero. Two of the legs above are the compiler's side of
the contract rather than the tool's: a `dusk` invocation with no file argument builds
the manifest root, and a required package's files are registered by their manifest
relative path, so two machines with the same lock emit the same IR.

## Package fixtures

`tests/dawn/<case>/` holds a static fixture project, the compiler's side of the
package contract written as ordinary records. Each case carries a `package.dawn`, a
`dawn.lock` whose commits are invented 40 hex strings, a `src/main.dusk`, and, where
the case needs a dependency, a `dawn_modules/<alias>/` placed in the tree by hand
with a `.dawn` stamp whose commit matches the lock line. There is no `.git` anywhere
under `tests/dawn/` and nothing is ever fetched, so a case is just a project that is
already in the state a fetch would have left it in.

That makes the record ordinary. A `check_fail` or a `run` record names
`tests/dawn/<case>/src/main.dusk` as its `file`, and the compiler discovers the
manifest by walking up from that file the way it does anywhere else.

```text
test dawn_std_shadow
mode check_fail
file tests/dawn/std_shadow/src/main.dusk
err_has 'std' is reserved for the standard library
```

The two families split along the tool boundary. A static fixture proves what the
compiler does with a manifest, a lock, and a checkout it is handed, so every reading
rule, alias resolution, and refusal lives here and costs one process to check. A
`dawn_*` special proves what the tool does, builds real git repositories at run time,
and covers fetching, locking, updating, and stamping, which a static tree cannot
show.

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

`tools/runner-differential.sh` proves the other direction, that a change to the
runner itself changed nothing the suite can see. It takes the previous
release's runner source out of git at the tag you name, builds it with the
previous release's compiler in a tree of its own, and runs it over the current
manifest beside the runner built from this tree. Both reports must be byte
identical and both must pass, so a reworded line or a comparison that drifted
shows up as a difference rather than as a suite that still says green.

```sh
tools/runner-differential.sh v1.14.1 /path/to/v1.14.1/dusk target/dusk-out/dusk
```

## The dump differential

`tools/dump-differential.sh` proves the other half of a change: not that the
suite still passes, but that two compilers cannot be told apart. It runs both
over one source set and requires every dump the pipeline can print to be byte
identical, with identical exit codes.

```sh
tools/dump-differential.sh <old-dusk> <new-dusk> [allow-file] [source-list]
```

Thirteen commands run per file: `lex`, `scan`, `parse`, `load`, `desugar`,
`check`, `check --json`, `mono`, `esc`, `ir`, `ir --target=wasm32`, `doc`, and
`doc --json`. Each run's stdout, stderr, and exit code are compared as one byte
stream, so a compiler that reaches the same verdict through a different fault
line, a different diagnostic order, or a different exit code still fails. Both
binaries read this tree through `DUSK_HOME`, which is what makes the comparison
fair: two correct compilers over one standard library and one runtime must agree
on every dump, the compiler's own roots included.

The default source set is every `.dusk` under `examples/`, each static package
fixture's root under `tests/dawn/`, every standard library module,
`tests/runner/testrun.dusk`, and both compiler roots, `compiler/dusk.dusk` and
`compiler/dawn.dusk`. Pass a file of paths, one per line, as the fourth argument
to run a narrower set while chasing one difference.

```sh
tools/dump-differential.sh /path/to/v1.14.1/dusk target/dusk-out/dusk
```

The script prints a `DIFF <file> [<command>]` line and the first lines of the
diff for every disagreement, then a tally, and exits non zero when anything
differed:

```
dump-differential: same=16559 allowed=16 differ=0
```

Use it where a change is supposed to be invisible: a refactor, a file split, an
idiom sweep, a rename that should not reach the emitted symbol. A change that is
meant to alter behavior fails this gate by design, and the golden suite, not
this script, is where that change is pinned.

The third argument is an allow list, one `<stem> <command>` line per tolerated
difference, with the stem being the source path with `/` replaced by `_`. It
exists so a deliberate exception can be written down and reviewed rather than
the gate being skipped around it, and it holds only what a release can name.
Across a version bump that is the version number itself: the two package
fixtures declaring a `@dusk` floor no compiler meets print the running version
in their refusal, so `tools/dump-allow-1.15.0.txt` carries those pairs and
nothing else. An allow list that is growing past that is the sign that a change
is not the invisible one it was described as.

Each command runs caged, under an address space limit, a timeout, and low
priority, and the whole script runs serially. It compiles the compiler's own
roots many times over, so never start one beside a self build.
