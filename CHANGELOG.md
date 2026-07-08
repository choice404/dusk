# Changelog

Notable changes to the dusk compiler, the standard library, and the dawn package tool. Each entry matches a tagged release, newest first. Commit messages carry the highlights and this file carries the detail.

## 0.7.0

The parser in dusk. 0.6.0 gave dusk1 a lexer; 0.7.0 gives it a parser, a loader, and a desugar pass, the three stages that turn a token stream into the merged, monad expanded module the rest of the pipeline consumes. `dusk1 parse`, `dusk1 load`, and `dusk1 desugar` now stand beside `lex` and `scan` as full commands, and each one's dump agrees with stage0's byte for byte across the whole corpus. No language surface changes; this is the bootstrap line doing exactly what it says it will. Suite 458 unit, 554 golden, 13 parser termination, clippy clean.

The parser.

- `compiler/ast.dusk` lays out the AST as a set of parse order arenas rather than a tree of boxed nodes: `ExprNode`, `StmtNode`, `TypeNode`, and `PatternNode` are each a fixed slot tagged record, `kind` plus up to four `int64` payload fields, stored in their own `Vector`, so a node is an integer id into the right arena rather than a pointer. Every variable length list, a call's arguments, a struct's fields, a function's generics, an enum's variants, a block's statements, is an `(off, len)` slice into one shared `kids` vector of ids, so the arena never allocates a list of its own for a shape that already has a home in `kids`. `compiler/intern.dusk` interns every name once into an `Interner`, so two occurrences of the same identifier, `println` at two call sites or a field name repeated across variants, compare and store as one integer rather than two heap strings.
- `compiler/parse.dusk`, `parseexpr.dusk`, `parsestmt.dusk`, `parseitem.dusk`, `parsety.dusk`, and `parseops.dusk` port the complete grammar stage0's own parser accepts: every expression and statement form, `match` with its arm and pattern grammar, lambdas, and the three `do` classes, a plain source, a named monad, and an anonymous discard. `collector_mint_ahead` in `parseexpr.dusk` runs the same one token lookahead stage0 uses to tell a `collector<T>(...)` mint from an ordinary `collector < n` comparison, since `collector` is a contextual keyword and not a reserved one. `in_async_fn` gates `await` the same four ways stage0 does, `x := await f`, `x, e := await f`, a discarded `await f`, and `return await f`, rejecting it under a lambda, under `defer`, and mid expression by name rather than letting a fifth shape parse silently.
- `p_enter_nesting` counts every recursive descent into an expression, a statement block, or a type the same way stage0's own guard does, and refuses past a depth of 500 with a named diagnostic, `expression nesting is too deep`, `block nesting is too deep`, or `type nesting is too deep`, instead of overflowing dusk1's own stack. Every toll point stage0's guard covers, parenthesized expressions, nested blocks, and nested generic type arguments among them, has a matching guard here, so a malformed or adversarial input that stage0 rejects cleanly can't take dusk1 down with it.

The loader and privatize.

- `compiler/loader.dusk` ports the full three tier import search: a dotted import resolves beside the importing file first, then against the stdlib root beside the binary or the tree `DUSK_HOME` names, and a quoted git path resolves against the dawn cache, `$DAWN_CACHE` or `~/.dawn/cache`. Every resolved path is canonicalized through `cool_realpath` before it is recorded, so two different relative spellings of the same file merge once rather than twice.
- `compiler/privatize.dusk` renames every non exported top level item with the same per file suffix scheme stage0 uses, so a bare call can never reach another file's private helper and two files' same named private helpers never collide once merged. `compiler/loadfold.dusk` folds a qualified call like `std.io.println` down to the bare, possibly renamed global it names, once every imported namespace is known, and merges each file's `monad` blocks into the loaded module the same way stage0 does, keeping only the root file's own `Module.monads` record rather than importing an upstream file's monad metadata along with its functions.

Desugar.

- `compiler/desugar.dusk` rewrites every `do { x <- m; ... }` block into nested calls on that monad's `bind` and `unit`, the same expansion stage0's `desugar::run` performs ahead of resolve and typeck. `cont_type` inspects the target `bind`'s own signature to recover the continuation's parameter and return type, falling back to `Type::Infer` when `bind` is itself generic, so a `do` over a still unconstrained monad leaves the same inference holes for a later type pass to fill rather than guessing a concrete type too early. The anonymous discard form, `$do`, gets a synthesized bind name through `discard_name` the same way stage0 mints one.

Parity, extended.

- `tools/differential.sh` now diffs five pipeline stages, `lex`, `scan`, `parse`, `load`, and `desugar`, not two. `parse` agrees with stage0 over all 581 corpus files; `load` and `desugar` agree over 580 of them, the one exception being the single file whose divergence is that stage0's loader gates each imported file's own `@paradigm` and dusk1's does not yet, since that gate is sema's job and lands with the sema port rather than the loader itself.

Oracle tooling, extended.

- `dusk parse`'s dump switches from Rust's derived `Debug` output to a hand written canonical renderer, `parser::dump::render_module`, so a second, non Rust compiler has a format it can actually reproduce: a float prints as `Float(0x...)`, the sixteen hex digits of its IEEE 754 bits, and a string, char, or rune literal escapes every non printable scalar as `\u{hex}`, the same two rules the `lex` dump already used for the same reason. `dusk load` and `dusk desugar` are new commands on the stage0 side, printing the merged module and the merged and desugared module through the same renderer.
- `docs/dumps.md` is now a full contract over all five dump commands, exit codes included: `parse` always prints, since the parser recovers rather than aborting, and only its exit code reports a lex or parse error, while `load` and `desugar` can print a dump and still exit non zero, an unresolved import or an imported file's paradigm violation recorded as an error without stopping the merge, so a printed dump and a clean exit are independent facts for those two commands. The doc also writes down, as a permanent part of the contract rather than a bug to chase, the one existing asymmetry a merge produces: rebasing a multi file program's spans into one coordinate space walks into a function or method body but leaves an item's own span, and every span recorded in `Module.monads`, in that file's original, unrebased coordinates, so a second compiler's loader has to shift exactly the same nodes and leave exactly the same ones alone to agree.

stdlib and runtime growth.

- The runtime gains `cool_is_file` and `cool_realpath`, the file existence check and the canonicalizing path resolve the loader's import search and its realpath based dedup read through. `lib/std/vector.dusk` gains `vec_set`, an in place element write by index that the AST arenas use to patch a slot after it is first appended.

The audit, honestly recorded.

- `await` inside a `defer` block used to parse without complaint; a `defer` runs at completion and cannot suspend, so it is now rejected by name, `'await' cannot appear under defer; a defer runs at completion and cannot suspend`.
- `await` used mid expression, buried in an operand rather than named on its own statement, used to go unchecked; it is now rejected, `'await' cannot appear mid-expression; give the awaited value a name`.
- `collector<T>()` with no argument used to record its diagnostic at an empty, zero width span; it now points at the call itself, so the caret in a rendered diagnostic lands somewhere a reader can see.
- `cont_type` used to stop at the first function named `bind` it found and read its shape, even when that function's parameter list didn't match a continuation and a later, correctly shaped `bind` was still ahead in the file; it now keeps scanning past a malformed same named match instead of settling for the first one.

Fixture pinning.

- `tests/fixtures/{parse_p1,parse_p2,parse_p3,load_p4,desugar_p5}` adds 63 small, hand written programs, each isolating one edge of the parser or loader grammar, a `collector<T>()` empty argument, an else if chain, a pipe call, a monad merge across two files, a private name shadowed at two different scopes, for `tools/differential.sh` to pin stage0 and dusk1 against, on top of the corpus the two already agree over.

## 0.6.1

else if, written down. The first surface note recorded under the bootstrap freeze, and not a surface change: the `else if` chain, `if a { } else if b { } else { }`, was already accepted by the parser as an `else` branch whose whole body is a single nested `if`, so no program's meaning moves. 0.6.1 writes it into the spec and pins it with goldens rather than leaving it a shape the parser happened to reach with nothing documenting or testing it. The language holds still, exactly as the freeze promises. Suite 458 unit, 554 golden (up from 552, two new here), 13 parser termination, clippy clean.

- The parser has read `else if` as `else { if ... }` since before 0.5.0, and 0.5.0 gave the chain its own depth guard. Each link recurses `if_ -> if_` past both block guards, so a long chain grows the call stack one frame per link while the shared nesting depth stays flat, which once overflowed the stack. Counting the else-if descent feeds the shared ceiling, so a chain past it unwinds into `expression nesting is too deep; simplify the expression` at the link that crosses it rather than aborting the process. The two parser termination tests that pin both ends, a twenty thousand link chain and a fifty link one, already shipped with that guard; 0.6.1 adds only the running goldens and the spec note on top.
- `examples/elseif.dusk` runs both shapes end to end through codegen: a full chain that ends in a tail `else` and fires exactly one arm, and a chain with no tail `else` where a value matching no arm falls through and prints nothing. `examples/elseif_badcond.dusk` is its compile-fail twin: because the desugared inner `if` is type checked like any other, a non-bool `else if` condition is rejected at the condition, `if condition must be a bool`, with the caret on it.
- The change is a parser and documentation matter only. No new token, so the stage0 and dusk1 lexers still agree byte for byte and `tools/differential.sh` stays green over the corpus, now 581 files with the two new examples in it. Resolve, typeck, mono, and codegen see ordinary nested `if`s and are untouched.

## 0.6.0

The stage one spine. This is the first release of the 0.6.x through 0.9.x line the bootstrap freeze opened: the language stops changing and the compiler starts being rewritten in itself. 0.6.0 lands the first slice of that rewrite, `compiler/`, a dusk program that is dusk1, the self hosted stage1 compiler's front end scaffold, and the parity gate that holds it accountable to the stage0 compiler that builds it. Nothing here changes the language. Suite 458 unit, 552 golden (up from 545, seven of them new for this release), 13 parser termination, clippy clean.

dusk1, the stage1 scaffold.

- `compiler/` is a new tree of dusk source, compiled and run by stage0 the same way any other dusk program is, that carries the spine of a command line front end: `cli` dispatches `version`, `demo`, `lex`, and `scan`; `diag` and `source` give it a located source buffer and a diagnostic renderer; `home` and `driver` find the toolchain and shell out to `clang`, and `demo` proves the whole path by emitting the phase 0 IR spine, linking it, and running it, the same smoke test stage0's own `demo` command runs; and `prescan` and a full lexer, `lexcore`, `lex`, `lexlit`, and `lexsym`, port every literal form, the complete escape set, the `nl_before` flag, and span tracking. The scaffold is written entirely `@paradigm procedural`, with no collector, async, interface, or lambda in it anywhere; the front end needs none of them yet, and a stage1 that leans on a feature stage0 has to prove first would have the dependency backwards.
- This is a scaffold on purpose. There is no parser, no resolver, no type checker, and no codegen in `compiler/` yet; `dusk1 lex` and `dusk1 scan` dump what the front end has built so far, and `dusk1 version` and `dusk1 demo` prove the binary itself compiles, links, and runs. The rest of the pipeline lands in the releases behind it.

The lexer parity gate.

- `tools/differential.sh` runs `lex` and `scan` on both stage0 and the built `compiler/` scaffold and diffs stdout byte for byte plus the exit code, over every one of the 579 `.dusk` files in `examples/` and `lib/std`. Every file's token dump and prescan dump agree between the two compilers, and every file whose lexer rejects it, an unterminated string, a malformed escape, a bad numeric suffix, rejects at the same file and the same line in both. This is the bar the bootstrap holds itself to from here on: a stage cannot replace the stage under it until it reproduces that stage's output exactly, one pipeline phase at a time.

Oracle tooling.

- The dump formats stage0 prints for `lex` and `scan` are now a documented interchange contract, `docs/dumps.md`, not an incidental debug view. A float token dumps as `Float(0x...)`, the sixteen hex digits of its IEEE 754 bits, so two compilers agree on a value with no shortest decimal rounding to reproduce. A string, char, or rune token escapes every non printable ASCII scalar as `\u{hex}`, needing no Unicode property table on the reading side, only the code point. The lex dump gains an `nl_before` field alongside each token's span, the parser visible newline flag a second compiler's parser will need.
- `tools/differential.sh` compares two binaries at one pipeline stage, `lex`, `scan`, or `parse`, over a file or a directory tree. `tools/pyramid.sh` climbs the stage ladder itself: it builds stage1 from stage0, stage2 from stage1, and so on, checking each new stage against the one that built it until two consecutive stages agree, a fixpoint. The test harness reads a `DUSK_BIN` environment variable that overrides the cargo built binary under test, so the golden suite can run unchanged against a bootstrap stage instead of stage0.

stdlib and runtime growth. The frozen surface stays frozen; this is the standard library and the runtime beneath it growing to carry the bootstrap, exactly the kind of change the freeze still allows.

- `std.os` is new: `run` shells a command out through the C library `system` and decodes the raw wait status into a normal exit code or, for a child a signal killed, 128 plus the signal, the shell convention, so a killed child is never mistaken for a clean exit. `env` reads an environment variable, reading back the empty string for one that is unset rather than any kind of null. `quote` wraps an argument in single quotes so a POSIX shell reads it as one literal word.
- `std.string` gains `int_to_string` and `int_to_hex16` for signed integer formatting, `substring` for a clamped byte range slice, `starts_with`, `sb_push_int` to append an integer straight into a `StringBuilder` with no intermediate string, and `f64_to_ir_hex` and `f32_to_ir_hex`, which format a float value as the exact `0x` hex token stage0 emits for a float constant in its IR, the contract a dusk hosted compiler's codegen will need to match stage0 byte for byte.
- `std.map` gains `map_keys`, the keys of a map in the order they were first inserted, returned as a fresh vector the caller owns independently of the map. A key is recorded once, at its first insertion; an overwrite does not move it and a grow rehashes the table without disturbing the record.
- The runtime gains `cool_env`, `cool_f64_bits`, `cool_f32_bits`, and `cool_file_size`, the C shims `std.os`, `std.string`, and the bootstrap's own source loader read a file's true byte size through, rather than trusting a NUL to mark its end.

The audit, honestly recorded. Building the scaffold and its gate surfaced real bugs, in the stdlib growing to support it and in stage0 itself; every one is listed here, not folded quietly into the feature list above.

- `map_keys` first returned the map's own insertion order record directly rather than a copy, so freeing the returned vector and later freeing the map freed the same buffer twice. Fixed by copying the keys into a fresh vector the caller owns with no shared owner.
- `std.os`'s `run` first reported a signal killed child's exit code as the raw low byte of the wait status, which is zero for a child a signal killed rather than exited; a process the OS kills used to read back as a clean exit. Fixed by decoding the signal byte first and reporting 128 plus the signal when it is set, the shell convention.
- `int_to_string` and `sb_push_int` first negated an integer to format its magnitude, which overflows silently for `INT64_MIN`, the one value with no positive counterpart. Fixed by accumulating the digits in the non positive range instead, where the most negative value needs no negation.
- The bootstrap's source loader first read a file by scanning to its NUL terminator, silently truncating a source that carries an embedded NUL byte. Fixed by reading the file's true byte size through `cool_file_size` instead, the same full byte length stage0's own `read_to_string` covers.
- Lexing a large file was quadratic: every token's text came from `std.string`'s `substring`, which computes the source's full length with `str_len` on every single call. A 40,000 line file took 27 seconds to lex. Fixed with a bounded `substring_n` that takes the buffer's already known length, dropping the same file to 0.17 seconds.
- A source file that is not valid UTF-8 used to load without complaint. The loader now validates the byte range up front and rejects it, matching stage0's own `read_to_string`, which is fallible for the same reason.
- An empty char or rune literal, `''` or `r''`, used to lex without a fault. Both now report `empty char literal` and `empty rune literal` at the literal's start.
- The pre scan pass only treated ASCII space, tab, and newline as whitespace between directives, missing the wider set a real source can carry. It now recognizes the full Unicode whitespace set, matching Rust's own `char::is_whitespace`.
- The parity gate itself caught a bug in stage0: its pre scan advanced past each line by its content length plus one byte, which undercounts a line ending in `\r\n` by a byte and misplaces every directive after the first CRLF line in a file. dusk1's independent pre scan disagreed with stage0 on the offset, and stage0 was the one that was wrong; it is fixed there first, since dusk1 has to match a correct oracle, not inherit a bug from an incorrect one.

## 0.5.4

Polish and freeze. This release finishes up 0.5: it closes an audit of the spec against the compiler, sharpens the diagnostics a program actually reads, downgrades the last covariance panic to a named build error, and declares the bootstrap freeze in the spec. No new language surface lands. The changes are a batch of spec drift fixes, a handful of behavior corrections where the checker was quietly wrong, and a permanent ledger folding every deferral recorded from 0.5.0 through 0.5.3 into one accounted list. Suite 458 unit (up from 434), 545 golden (up from 511), 13 parser termination, clippy clean.

Caret diagnostics.

- A diagnostic now renders three lines, the header, the offending source line, and a caret run underneath it, instead of the bare header alone, so the reader sees where in the line the error sits without counting columns. The caret span is measured in Unicode scalar columns rather than bytes, so a caret under a line that holds `中` or `😀` lands on the character rather than drifting off by the extra encoded bytes, and a zero width span still draws a single `^` so every diagnostic points somewhere.

`e.message`, a behavior fix.

- Reading `e.message` used to lower to a silent zero. It now reads through the same null guarded lowering `e.toString()` uses: a real error yields its message string, and the empty error's null message pointer reads as `""` rather than a garbage word. `message` is read only, so `e.message = "..."` is rejected, `an error's message is read only; build a new error instead`, and any field name other than `message` on an error is rejected, `error has no field '<name>'; it carries only 'message'`. A `match` arm whose tail read an error's message once mistyped that arm; the arm tail typing is corrected alongside the read.

Slice covariance, a panic downgraded to a build error, a behavior change.

- A slice of a concrete struct and a slice of an interface share the `{ ptr, len }` shape, so reinterpreting one as the other reads every element as a boxed interface and corrupts memory. The sema covariance sink that rejects it, `cannot pass a slice of '<concrete>' as a slice of interface '<iface>'`, now fires in the positions it used to miss: a method argument and an interface receiver, a tuple element, an array element, and a function value argument, so the check no longer stops at a plain function call.
- Where a concrete element type was erased to the unknown type before the sink could see it, codegen used to reach an unreachable path and abort the compiler with a process panic, exit 101. The codegen backstop now records the same named build error and poisons the value with a `zeroinitializer` instead, so the module fails to link with a clean diagnostic rather than crashing the compiler. The permanent net is that any missed sink is a loud, named build error, never a miscompile.

The audit hardening batch.

Ten checks the surface audit surfaced, each a drift the spec claimed the compiler did not enforce or a hole the checker left open. Several are behavior changes, marked.

- **Constructor payload literal fit, a behavior change.** An enum constructor's payload literal is now ranged against the field's declared width the same way an annotation's right hand side is, so `Tag.V(300)` at an `int8` field is rejected, `literal 300 does not fit in 8 bits`, instead of silently truncating, and the signed bounds apply the same way.
- **String index assignment rejected, a behavior change.** `s[i] = c` on a string is now rejected, `a string is immutable; build a new one with a StringBuilder`, since the bytes live in read only storage; reading `s[i]` stays legal.
- **Phantom parameter type rejected, a behavior change.** A function parameter declared with an undeclared type name is now rejected, `unknown type '<name>'; no type of that name is declared or imported`, closing a path where an unused, undeclared type slipped straight through, a phantom `Collector` parameter among them.
- **Whole fallible tuple bind rejected, a behavior change.** Binding a fallible call's `(T, error)` result to a single name is now rejected, `a fallible result must be destructured; bind the value and the error`, so the error can never hide unread inside the pair.
- **impl and interface paradigm gate.** Both an `interface` declaration and an `impl` block now require `@paradigm oop`, gated the same way a functional builtin needs `@paradigm functional`. A struct stays ungated across every paradigm, since it is plain data.
- **Monad block validated.** A `monad` block missing either `bind` or `unit` is now rejected at parse, `a monad block must define both 'bind' and 'unit'`, and a block without `@paradigm functional` is rejected during gating, `monad block requires the functional paradigm`.
- **Enum method call rejected, a behavior change.** A method call on an enum value, `m.unwrap()`, is now rejected, `'<name>' is not defined; methods on the enum '<Enum>' are not supported, match on it instead`, since only struct receivers dispatch a method.
- **Functional builtin arity checked.** `fold` takes exactly three arguments and `map`, `filter`, `reduce`, and `foreach` take two; a stray extra argument is now rejected, `fold takes 3 argument(s)`, rather than ignored.
- **Unsigned integers removed, reserved, a behavior change.** The unsigned integer type names `uint8` through `uint64` and the `u` literal suffixes are reserved rather than usable; naming one is rejected, `unsigned integers are reserved; use the signed widths`. The signed widths cover the surface until after 1.0.0.
- **Async loop pumping rejected, a behavior change.** Calling the loop's blocking `await`, `await_timeout`, or `try_poll` primitives directly inside an async func is now a compile error, `'<name>' pumps the event loop and cannot be called inside an async func; use the await statement`, since pumping one by hand parks the only thread the loop cranks on. The reject is direct only; an indirect pump through a sync helper the checker cannot see into still falls to the runtime idle fatal.

The kqueue runbook.

- The BSD and macOS reactor backend `reactor_kqueue.c`, written against the poller seam but never compiled or run on this Linux host, gains a bring up runbook in `docs/kqueue.md` for the person who runs it on a kqueue platform. The honest status is unchanged: the backend reads clean and is statically reviewed, but it stays unverified until a BSD or macOS runner compiles and exercises the full reactor, net, and stress matrix and pins the one documented divergence, a close while armed then reused descriptor.

The bootstrap freeze.

- The spec's status chapter now declares the bootstrap freeze. The surface described there is frozen as of 0.5.4: the 0.6.x through 0.9.x line changes the compiler as dusk is rewritten in itself, not the language, so a program that compiles against this spec keeps compiling across the bootstrap with no source change. Three kinds of work stay live under the freeze, a diagnostic can improve, the standard library can grow, and a soundness fix can land, since none of the three change the surface a correct program relies on. New surface resumes only after 1.0.0. The one exception is a soundness hole that forces a surface change to close it, and when that happens the change is named in the changelog of the release that makes it.

The permanent ledger.

Every deferral the 0.5.0 through 0.5.3 line recorded, folded into one accounted list and sorted by where each one now stands.

Fixed in this release.

- `e.message` reading a silent zero, now the null guarded string read that `toString` already used.
- The slice covariance codegen panic, now a named build error and a poisoned value rather than a process abort.
- The ten audit items above, each a drift the spec claimed or a hole the checker left open, closed and pinned by a golden or a unit test.

Closed earlier, now pinned.

- A generic instantiated over an interface type argument, rejected outright since the 0.5.0 ledger rather than hanging the compiler, with the instantiation ceiling backing a bounded reject.
- A scalar typed array field, sized correctly in codegen so a field read no longer disagrees with the slot the frame reserved.
- A monad block missing `bind` or `unit`, or written without `@paradigm functional`, now a loud reject rather than a silent miss.

Permanent by design.

- The empty source `do` element defaults its width consistently over an underdetermined program, a deterministic default rather than a reject, since resolving it needs an analysis of the source carrier's own bind body.
- An async func cannot take or return a `Future<T>`, since a future belongs to the event loop thread and taking one by value would let it cross into a task frame.
- No `Collector` type implementing `Allocator` ships, withheld, since the allocator interface's untyped `*void` would erase the `collector<T>` tracking the checker relies on to keep a collected reference on its anchor thread.
- `Either` has no `monad Either { ... }` block, since a `unit` would have to pick a free `Left` and there is no canonical one; the plain helpers are the surface.
- The `IO` helpers yield `IO<bool>` rather than `IO<void>`, since `void` carries no value for `bind` to thread through a chain; hand constructing an `IO<void>` is not banned, `run` just forces it and yields nothing.
- A multi statement lambda in some argument positions still infers its return type weakly; an explicit return type annotation on the lambda resolves it.

Carried into the bootstrap.

- An alias buried inside an aggregate a call returns is not yet surfaced: `wrap(c)` returning `Store{c: c}` forms no edge from the binding back to the pointer argument, so a store through the returned field and a separate later use of the pointer can read clean when the two name the same view.
- A nested enum variant's payload is not yet alias linked to the binding that built it, safe today only because a locally constructed enum copies its payload rather than aliasing the argument; the two alias gaps close together the day enum payloads alias instead of copy.
- The kqueue backend is unverified on BSD or macOS until a runner exercises it.
- The enum method depth couples to local enum ground typing, so extending it waits on that path.
- Conservative collection over retains, so a live byte count read through `gc_live_bytes` is an upper bound, never an under count.
- The build passes no optimization flag to `clang`, a collector root scan dependency rather than a speed choice; adding one is a soundness change that must land with a precise root map.

## 0.5.3

The stdlib. This release rebuilds `IO<T>` as a true lazy monad, ships `std.functional.result` and `std.logging`, rounds out the `Maybe` and `Either` helper surfaces, and closes two soundness gaps the stdlib work exposed: a class of generic constructor calls that used to default silently or die inside `clang`, and a must handle launder through an `error` parameter that used to let an obligation end quietly. Suite 434 unit (up from 418), 511 golden (up from 477, 34 of them new for this release), 13 parser termination, clippy clean.

`IO<T>` becomes lazy, a behavior change.

- `std.functional.io` now defines `struct IO<T> { run: collector<() -> T> }`. `bind` and `unit` build a new collected thunk instead of running anything: `bind(m, k)` returns a thunk that, once forced, forces `m`, feeds its value to `k`, and forces the `IO` `k` returns; `unit(x)` returns a thunk that just yields `x`. A `do IO { ... }` block therefore builds a chain of nested thunks on the collected heap with no effect fired yet, and `run(io: IO<A>) -> A` is the one place that forces the outermost thunk and runs the whole chain, on the calling thread.
- The thunk and every step it captures live on the collected heap, so a chain outlives the frame that built it and survives a collection forced between build and force; `run` keeps the chain rooted through to completion. `IO<T>` inherits collector confinement outright: a value of it cannot cross a `spawn` or `submit` capture, a channel, or an interface box, since the suspended environment behind its thunk is only ever safe on the anchor thread. `IO<T>` still does not exist for `void`; an effect that returns nothing yields `bool`, as `io_print` and `io_println` already did.
- **Migration note.** The 0.4.3 `IO` was eager over its carried value and its `run` minted a future and offloaded onto the thread pool, so a program had to bring the event loop and the pool up with `loop_init` and `pool_start` before the first `run` and tear them down after the last one. `run` now just forces its thunk on the calling thread; that loop and pool ceremony around an `IO` chain is no longer needed, and a program that keeps it around for no other reason can drop it.
- `io_pure`, `run`, `io_map`, `io_and_then`, `io_print`, `io_println`, and a new `io_read_line() -> IO<Result<string, string>>`, which reads one line when forced and folds end of input or a read failure into `Err`, ship over the lazy carrier.

`std.functional.result`, new.

- `Result<T, E>` is `enum Result<T, E> { Ok(v: T), Err(e: E) }`. A `monad Result { ... }` block fixes `E` to `string`, the common case, since a generic `E` cannot flow through `do` inference the way `Maybe`'s single type parameter does; a caller needing a different error type uses the plain constructors and helpers instead of `do Result { ... }`. `do Result { ... }` threads `Ok` values and short circuits on the first `Err`.
- `result_ok`, `result_err`, `is_ok`, `is_err`, `result_unwrap_or`, `result_map`, `result_map_err`, `result_and_then`, and `result_or_else` round out the surface the same way `Maybe`'s helpers do. `result_from(v: T, e: error) -> Result<T, string>` bridges the `(value, error)` pair a fallible call returns: `Ok(v)` when `e` carries nothing, `Err(e.toString())` when it does, and handing `result_from` a bound error discharges the caller's must handle obligation the same way any other hand-off to an `error` parameter now does (see below).

`Maybe` and `Either`, rounded out.

- `Maybe` gains `is_none`, `maybe_map`, `maybe_and_then`, and `maybe_or_else`, alongside the existing `is_some` and `unwrap_or`.
- `Either` gains `right_or`, `either_map`, `either_map_left`, `either_and_then`, and `either_or_else`, alongside the existing `is_left` and `left_or`. `Either` still has no `monad Either { ... }` block; a `unit` for it would have to pick a free `Left`, and there is no canonical one, so `do Either { ... }` stays unsupported by design and the plain helpers are the surface.

`std.logging`, new.

- `LogLevel` is `Debug`, `Info`, `Warn`, or `Error`, ranked in that order. `log_set_level` sets a process wide threshold, and `log_debug`, `log_info`, `log_warn`, and `log_error` each fire to stderr, tagged `[debug]`, `[info]`, `[warn]`, or `[error]`, only when their own level is at or above the current threshold. The default threshold is `Info`. The level lives in the C runtime as a single atomic word shared by every thread, so a `log_set_level` call from any thread takes effect everywhere at that thread's next log call, and every message goes to stderr so a program's stdout output stays clean underneath it.

Generic inference, hardened against a silent default or a `clang` death.

- A bare lambda handed to a parameter typed `collector<(A) -> X<B>>`, the shape a lazy monad's `bind` takes its continuation as, now pins `A` and `B` the same way a bare lambda at a plain function typed parameter already did; this is what lets `do` notation compose over a lazy monad like the new `IO` or a user defined one shaped like it.
- An enum constructor for an empty variant, `Opt.None` on a generic `Opt<T>`, carries no payload to read `T` from. Sitting at a struct literal field, a call argument, an assignment's declared right hand side, or an array element, it now instantiates `T` from that position's grounded expected type instead of defaulting; a nested constructor threads the same way, so `Opt.Some(Opt.None)` at an annotated `Opt<Opt<float64>>` instantiates the inner `None` at `float64`.
- A constructor or a monad `do` element that still cannot be pinned anywhere is now a named compile error, `cannot infer the type parameter 'T' for 'Opt'; add an annotation that pins it`, instead of silently defaulting the parameter to `int64` and dying later inside `clang` on a width mismatch it never surfaced, or being silently relabeled as whatever type happened to be expected.
- A generic enum constructor's payload is now validated against the variant's declared field type in the ground, types only pass, the same recheck that already catches a width mismatch hiding inside a generic function body: a call that pins the element from one argument, `keep(0, Box.Has(true))` pinning `Box<T>` to `int64` through `seed: T`, now catches the mismatched `bool` payload instead of letting it relabel silently as an `int64`.

Every error must be handled, extended to `error` parameters, a behavior change.

- Handing a bound error straight to a parameter declared `error` now discharges the caller's must handle obligation, the same as a bare `return`, a `check`, or an `ignore` call. The obligation does not stop at the caller: an `error` parameter now carries the same must handle rule a let bound error does, so a function like `func swallow(err: error) -> void { }`, an empty body that receives an error and drops it, is rejected, `the error 'err' is never handled`. A callee discharges its own `error` parameter with `exists()`, `check(...)`, `ignore()`, a `return`, or a hand-off to another `error` parameter, the same menu a let bound error already had.
- The discharge is narrowed to a direct hand-off at the argument, not a whole expression scan: reading a bound error into a fresh value first, or laundering it through a generic passthrough call, still leaves the original binding unhandled. `sink(fst(e, e2))`, handing a passthrough helper's result to a clean `sink`, still reports both `e` and `e2` as never handled, since neither is handed to `sink` directly. The net effect closes a hole where an obligation could pass from a binding to a parameter to another parameter and end inside a body that never actually inspected it.

Known limitations.

- A monad `do` element bound from an empty source carrier, `a <- Maybe.None` or a user monad's equivalent, still defaults to `int64` when nothing pins its width and the source carrier's own bind body would call the continuation on that phantom element anyway; this is a deterministic default over an underdetermined program, not a reject, and closing it needs an analysis of the bind body itself, out of scope for this release. A `do` chain that returns its own empty source directly, leaving the whole result element undetermined rather than merely a phantom argument, is still reported by name rather than defaulted.
- A multi statement lambda passed as a helper call's argument still infers its return type weakly in some positions; an explicit return type annotation on the lambda resolves it.
- `IO<T>` still has no `void` instance; an effect returns `bool` instead.
- `Either` still has no `monad Either { ... }` block; use the plain helpers.

## 0.5.2

Unicode strings. This release adds a `rune` type for a single Unicode scalar value, the `\u{...}` escape, strict UTF-8 validation of string literals, and `std.unicode`, a pure dusk decode and encode layer over the string's existing byte view. A string's representation does not change: it stays the same NUL terminated UTF-8 byte view it always was, `s[i]` still reads a byte, and iterating scalar by scalar is a decoding walk, `decode_rune(s, i)`, not a new indexing form. A codegen fix lands alongside the surface work: a loop body binding no longer allocates a fresh stack slot every iteration. Suite 418 unit, 477 golden (up from 458, 19 of them new for this release), 13 parser termination, clippy clean.

`rune`, a 4 byte Unicode scalar.

- `rune` is a new primitive, 4 bytes, holding one Unicode scalar value with no encoding attached, wide enough for `中` or `😀` where `char`'s one byte only ever holds ASCII. A rune literal is `r'...'`: `r'a'`, `r'中'`, `r'\u{1F600}'`, every ordinary escape legal inside one plus `\u{...}`.
- `rune` and `int` interconvert both ways under the same rule `char` and `int` already follow: assignment widens freely, and a wide int silently truncates down to a rune the same way it does for `char`. `rune` and `char` refuse to mix in either direction, since a byte and a scalar are different things even riding the same integer register: `type annotation that does not match its value` at an annotation, `argument N has the wrong type` at a call.
- A rune carries no arithmetic of its own; codepoint arithmetic happens by binding the rune to an `int64`, computing there, and assigning the result back. Comparison between two runes, or a rune and an int literal, is allowed directly. `sizeof(rune)` is 4, and a `rune` crosses the foreign function boundary as a C `i32`. No user defined type may be named `rune`, the name is reserved.
- `println(rune)` prints the scalar's codepoint number, not a glyph, `println(r'中')` prints `20013`; printing the character itself goes through `std.unicode`'s `encode_rune`. A `match` pattern still does not bind a rune literal, the same restriction the existing pattern grammar already puts on a char or int literal; compare a rune scrutinee with an `if` chain instead.

The `\u{...}` escape.

- `\u{...}` names a Unicode scalar by 1 to 6 hex digits between the braces, legal inside a string literal and a rune literal for any scalar up to the Unicode maximum `0x10FFFF` excluding the surrogate range `0xD800..0xDFFF`, and legal inside a char literal only when the value fits one byte, `0x7F` and under.
- Five ways to get it wrong are each a named diagnostic: an empty or over 6 digit body, `\u escape needs 1 to 6 hex digits`; a missing closing brace, `unterminated \u escape; expected '}'`; a surrogate value, `\u escape is a surrogate code point, not a scalar value`; a value above the maximum, `\u escape is above 0x10FFFF, the Unicode maximum`; and a wide escape inside a one byte char literal, `a char is one byte; this escape does not fit, use a rune literal or a string`.

Strict UTF-8 string literals, a behavior change.

- A string literal with an invalid UTF-8 byte sequence used to lex silently, replacing the bad bytes with U+FFFD and moving on. It is now a loud compile time reject, `string literal is not valid UTF-8`. A program that carried a malformed byte sequence inside a string literal and relied on the silent replacement now fails to compile instead; fix the literal's encoding or, if the bytes are intentional, build the string at runtime through `std.unicode`'s `encode_rune` instead of a literal.

`std.unicode`, pure dusk, zero runtime C change.

- `decode_rune(s: string, i: int64) -> (rune, int64)` decodes one scalar at byte offset `i` and returns it paired with its encoded width. It is total: the NUL terminator decodes to `(0, 0)`, and any malformed byte, a stray continuation, an overlong lead, a truncated tail, a surrogate, or a scalar above the maximum, resyncs to exactly `(0xFFFD, 1)` so the caller always makes forward progress one byte at a time. Its precondition is that `i` lies in `[0, str_len(s)]`; a string is a raw NUL terminated view, so an out of range `i` is an unchecked read, exactly as `str_len` would give on the same view.
- `encode_rune(r: rune, buf: *raw char) -> int64` writes a scalar's 1 to 4 UTF-8 bytes into a caller sized buffer and returns the count; an invalid scalar writes the 3 byte U+FFFD encoding instead. `rune_len` reports the width `encode_rune` would use without writing. `rune_count` walks a string end to end counting scalars, each malformed byte counting as exactly one so the count never desyncs from `decode_rune`'s resync. `utf8_valid` runs the identical decode loop and is invalid only on the resync signature, width 1 paired with the U+FFFD scalar, so it cannot drift from what `decode_rune` itself accepts. `sb_push_rune` appends one scalar's encoded bytes to a `StringBuilder`.
- The decoder is strict throughout: an overlong encoding, a surrogate, and a scalar above `0x10FFFF` are all rejected the same as a truncated or malformed sequence, never silently accepted as some other valid scalar.

Codegen fix, an unbounded loop no longer overflows the stack.

- A binding introduced inside a loop body used to emit its `alloca` at the binding site, so every iteration allocated a fresh stack slot the loop never reclaimed; a decode loop over roughly 300 KB of input was enough to segfault on the default stack, since dusk's unoptimized build never ran `mem2reg` to fold the growth away. Every sync mode stack slot is now funneled into the function's entry block instead, one slot per binding reused across iterations, the shape a normal LLVM frontend already produces. A 500,000 character `rune_count` walk now returns clean on the default stack; the regression golden is `unicodebig`.

Known limitations.

- `decode_rune`'s `i` precondition is unchecked outside `[0, str_len(s)]`, the same honor system a raw string view already carries; a normal decode walk never leaves the range, only a caller supplying an arbitrary offset can.
- A `match` pattern still does not bind a rune literal; compare a rune scrutinee with an `if` chain.
- Case folding, normalization, and grapheme clustering are out of scope for this release and are not part of `std.unicode`.

## 0.5.1

The collector. This release activates the `collector<T>` syntax the 0.1.0 spec reserved and left dormant: a second, conservative mark and sweep heap sits beside the generational one, single mutator and pinned to the thread it anchors to, opted into per value and never ambient. Nothing else in the pipeline moves; the escape checker, the interprocedural summary, and the async keyword layer are unchanged from 0.5.0 except where a collected value now flows through them. Suite 458 golden (up from 405), 408 unit, 13 parser termination, clippy clean.

The collected heap and `collector<T>`.

- A collected block carries the exact sixteen byte header a generational block carries, an eight byte size word and an eight byte generation word ahead of the payload, so the same dereference check that faults a stale generational pointer reads a collected block's header unchanged. The two heaps differ only in retirement: a generational block is retired by an explicit `free`, a collected block only by a collection, a conservative mark and sweep over the roots the runtime can reach.
- `collector<T>` mints three kinds of value depending on `T`. A plain kind, a scalar, a managed `*T`, a string, or a struct of those, derefs exactly like an ordinary managed pointer. A closure kind, `collector<F>(lambda ...)`, builds the lambda's environment on the collected heap instead of the frame, so the closure survives the frame that wrote it. A slice kind, `collector<U[]>(e)`, deep copies the backing one level onto the collected heap, legal only when `U` is immortal safe, so a slice of slices, closures, or interfaces is rejected since the copy does not reach what an element points at in turn.
- The collector is single mutator, anchored the first time a collected block is minted or a collection is forced, in practice the thread running `main`; an allocation or collection asked for off that thread aborts by name, `fatal: the collector runs on the main thread only`. The root scan is conservative: the anchor thread's stack between a collection point and its high water mark, a register spill caught through a `setjmp` snapshot, every live entry in the generational heap's registry, and every region the async substrate registers for a task frame or a closure environment, each word tested as a possible pointer into a live collected block. A conservative scan only over retains, never under retains.

Escape neutral minting, and the capture rule.

- A collected value is not a frame view, so it returns cleanly, bare or embedded in a tuple, struct, or array. The mint itself is an outliving sink the same escape check already runs on `return`: an argument carrying a frame view, a slice into a local array, a closure over a frame local, or a managed pointer whose pointee a store already tainted, is rejected at the mint rather than copied onto a heap the view does not outlive.
- The closure kind carries a matching capture rule. Every capture in `collector<F>(lambda ...)` must be immortal safe, a scalar, a managed pointer, a string, a nested `collector<..>`, or an aggregate of those; a slice, a closure, or an interface capture fails outright, and a managed pointer capture whose pointee already stores a frame view is rejected too, since the pointer is immortal safe but the view behind it is not, `cannot collect a closure that captures '<name>': it may view a frame; collect '<name>' first or capture heap owned data`.
- A slice kind source is checked the same way one level down: a managed pointer buried in the copied elements that itself carries a tainted pointee is rejected, `a collected slice element holds a pointer to an object that stores a view of the current frame; the collected block outlives the frame, so heap own the pointee or collect it first`.

No `free`, no `move`, no `ref`.

- A collected value is never freed, moved, or borrowed with `ref`. All three are rejected at the checker: `a collected value is not freed; the collector reclaims it`, `a collected value is not owned; copy it directly`, and `a collected value is not borrowed with ref; copy it directly`. Passing or storing one copies it by value, the same rule a scalar or a managed pointer already follows; there is no explicit release to hand off, so there is no ownership to transfer.

Thread confinement, enforced at compile time.

- A `Channel<collector<T>>` is rejected, `a collected value stays on the main thread; it cannot cross through a channel to another thread`, since a same thread channel's ring buffer sits outside every root the collector scans. A `spawn` or `submit` capture of a collector value is rejected the same way, `<fn> cannot capture '<name>': a collected value stays on the main thread; it cannot cross to another thread`, since a worker thread's private environment is the same kind of unrooted store. Boxing a collector value into an interface is rejected, `a collected value cannot be boxed into an interface; it stays on the main thread`, since the boxed payload would need to travel wherever the interface value travels. A managed pointer whose pointee reaches a collector value is caught crossing any of these same three paths, so the ban does not stop at a bare collector argument.
- A `Future<collector<T>>` and an async func that returns a collector value are allowed: a future completes on the loop thread, and `async_run` is the anchor thread's own bridge into the loop, so a collector value crossing a suspension never leaves the thread it is confined to. A task frame is a registered root region, so a collector minted before an `await` and read after it survives a forced collection on either side of the suspension. A same thread container, `Vector<collector<T>>` among them, is allowed too: the container's own backing buffer is a generational block the registry already scans as a root, so a collector value pushed into one stays reachable across the vector's growth and any number of forced collections in between.

`collector` as a contextual reserved word.

- `collector<` opening a type or an expression position starts a `collector<T>` type or a `collector<T>(e)` mint. A named binding called `collector` compared against something else, `collector < n`, still parses as a plain identifier: the parser looks far enough ahead to tell a mint from a comparison before it commits to either reading, so a program can still name a variable `collector` outside that one shape.

Widening, one way only.

- A `collector<F>` value passes anywhere a plain `F` is expected, and a `collector<U[]>` value passes anywhere a plain `U[]` is expected, since the wrapper's representation is exactly the value it wraps. The reverse never happens implicitly. A bare lambda literal at a `collector<F>` parameter is accepted only at a direct top level call, where the compiler rewrites it into the equivalent mint; the same bare lambda at a method argument or through an indirect call is rejected, `a bare lambda cannot become a closure collector at a method argument; write the mint explicitly: collector<F>(lambda ...)`, since only the explicit mint runs the escape and capture checks that make the value immortal safe.

`std.memory.collector`.

- Four wrappers over the collector's control and gauges: `gc_collect` forces a collection now, `gc_live_blocks` and `gc_live_bytes` read the live set, and `gc_collections` counts collections run since the program started. No `Collector` type implementing `Allocator` ships alongside them; see the withheld item below.

Deferred and known limitations.

- **Collector as Allocator, withheld.** A `Collector` struct implementing the `Allocator` interface was drafted and pulled before shipping. The `Allocator` interface hands back an untyped `*void`, which erases the `collector<T>` tracking the checker relies on to keep a collected reference confined to its anchor thread, so a collected block routed through the allocator seam could cross a channel or a spawn boundary as a bare pointer with no diagnostic and be swept out from under a worker thread still holding it, a silent hole rather than a caught one. Closing it needs the checker to track whether a value is collected through the allocator seam itself, deferred to a later release. The typed `collector<T>` mint stays the one checked surface for collected memory.
- **Inherited alias residual.** The alias gap 0.5.0 recorded, an alias buried inside an aggregate a call returns, is unchanged and not specific to the collector: `wrap(p)` returning `Holder { p: p }` still forms no edge from the binding that receives the struct back to `p` itself, so a store through the returned struct's field and a separate later use of `p` can read clean when the two alias the same view. This is a language wide escape gap, not a collector one, and stays open pending per field pointer aliasing in the summary model.
- **Cosmetic diagnostic reuse.** An opaque call's tainted result rejected at a `collector<T>(e)` mint reuses the return escape wording, `this call may return a view of argument N, which views the current frame`, even though the value in question is being collected rather than returned. The reject fires correctly; only the phrasing borrows from the wrong sink.
- **Over retention.** The mark and sweep scan is conservative, so a stray stack word that merely resembles a pointer keeps a block alive one collection longer than it strictly needed to. This is the correctness direction a conservative collector is supposed to err in, never the reverse, but it means live byte counts read through `gc_live_bytes` are an upper bound, not an exact one.
- **No optimized build.** The collector's root scan depends on the frame layout dusk's unoptimized build already guarantees, a local kept in a stack home a register allocator could otherwise elide. `clang` is invoked with no optimization flag for exactly this reason; adding one is a collector soundness change, not a speed change, and must land with a precise root map alongside it.

## 0.5.0

The ledger. This release closes the debt the 0.4.x line recorded against itself, with no new language surface. Its center is escape analysis: the checker now catches a frame view laundered out of a function through a call, a store, a channel send, a closure, or a pointer alias, the whole class an intraprocedural check could not see by construction. Alongside it, the parser terminates on malformed or pathologically deep input, a generic instantiated over an interface type argument is rejected instead of hanging the compiler, a future minted by a direct async call finally behaves like any other value, a mutable tuple's fat member survives a reassignment, and a bare function value called in return position lowers correctly. Suite 826 (408 unit, 405 golden, 13 parser termination), clippy clean.

Interprocedural escape enforcement.

- Escape analysis was flow sensitive but intraprocedural: `func passthrough(s: int64[]) -> int64[] { return s }`, called on a view of a frame local array and returned again, handed the caller a dangling view with no diagnostic, since the checker only ever walked one function body at a time. Escape is now a summary based interprocedural analysis. Every function gets a summary computed to a fixed point over four relations: `returns_alias` names the parameters whose view may reach the return value, `reads_through` names the pointer parameters whose pointee the return value may expose, `flows_into` records that one parameter's view may be stored into a place another parameter reaches, and `sinks` names the parameters whose value or pointee is handed to `chan_send` or `chan_try_send`, directly or through a helper that itself sinks its argument. A lambda literal earns the same four relations over its own parameters, and a method's summary treats its by pointer `self` as parameter zero, so a method that stores a frame view through `self` or sends `self` into a channel is caught the same way an ordinary function is: `this call may return a view of argument 1, which views the current frame` and `argument 1's view is stored into argument 2 and may outlive this frame`.
- A callee the summary cannot see through, a closure value, a function parameter, or a lambda bound to a struct field, is opaque, and an opaque call now defaults to rejecting a polluted argument rather than accepting one: a managed pointer whose pointee a store edge has already touched, a bare frame slice, or a frame capturing closure, all refused at a call the checker cannot look inside. This is a deliberate over reject, the same posture the escape check has always taken when it cannot prove a value clean, and it is what makes the sink and store classes total instead of keyed to only the callees the summary happens to name.
- Enforcement runs on the surface pass only; the ground, types only pass monomorphization drives is unchanged from the soundness split 0.4.3 introduced.

Alias aware escape flagging.

- An escape flag lives on a binding, not on an allocation, so it used to wash out the moment a value crossed an alias: `st := Store{c: c}`, then a frame view stored through `st.c`, raised `st`'s flag but left `c`'s clean, and returning `c` on its own slipped through dangling. Every binding introduction site, a plain `let`, a tuple or struct destructure, a match payload binder, a `for` loop variable, and a plain assignment, now funnels through one binding alias choke that links the new name into the alias group of every managed pointer, or pointer reaching value, its initializer touches. A raise on any member of a group now raises the whole group, so `st := Store{c: c}`, `p := st.c`, and a `for row in rows` loop variable each keep a later store through the alias linked back to the value it aliases.
- The link only fires for a type that can reach a managed pointer, a bare pointer, a struct or tuple with one buried inside, or a generic field erased to the unknown type; a slice or a scalar member links nothing, so an unrelated sibling field or a scalar read through the same binding does not falsely taint a clean pointer. This precision is what keeps the alias model from over rejecting a program that merely names a pointer twice without ever storing a frame view through either name.

The recorded 0.4.x debt.

- An interface bound as a generic type argument, `Box<Speaker>` where `Speaker` is an interface, is now rejected outright, `an interface cannot be a generic type argument; generics are monomorphized over concrete types`, at the annotation from the type checker and as a monomorphization backstop when the argument is inferred from a value rather than spelled. An interface has no single ground layout to expand a generic over, and binding one used to send monomorphization into an unbounded expansion instead of a diagnostic; the existing instantiation ceiling that already bounds any runaway generic now backs a clean, bounded reject instead of a multi minute hang.
- A malformed or pathologically deep input no longer hangs or aborts the parser. Every recovery loop, the loop that mops up leftover tokens after a malformed statement or expression, now routes through one shared progress invariant helper, so a loop making no forward progress on its current token is a bug caught in testing rather than an infinite loop in the field. A shared recursion depth counter bounds the expression, type, block, and `else if` chain recursions; crossing it unwinds with a named diagnostic instead of a stack overflow. Twenty thousand open parens, a deeply nested generic, and a long `else if` chain each used to run the parser out of stack; each now reports a depth diagnostic in bounded time, pinned by a termination test file kept separate from the golden suite.
- A future minted by a direct async func call now passes as an argument, carries an explicit `Future<T>` annotation, and stores as an element in a generic container, not only a bare name awaited on the spot. The ground, types only pass read `Future<T>` as the mangled struct monomorphization built for it, `Future$int64`, while the async call's own signature still carried the surface future type, so the two never agreed and every non trivial use rejected as a type mismatch. Monomorphization now records the mangled name of every future instantiation against its element type, and the ground pass restores the surface shape at every annotation, parameter, and container element before comparing types. `Vector<Future<int64>>`, ten tasks fanned out into one vector and drained by an awaiting loop, now type checks and runs. One rule stays, intentionally: an async func still cannot declare a `Future<T>` parameter, since a future belongs to the event loop thread and taking one by value would let it cross into a task frame.
- A mutable tuple bound with an array literal member, then reassigned to a slice bound to that same member, now builds. The binding's inferred storage carries a slice member, since a later reassignment stores one there, but the array literal initializer alone used to size the slot as a fixed array, so codegen and the reassignment disagreed on the member's shape and clang rejected the mismatched IR types. The reconciled storage type now threads from the type checker through monomorphization into codegen, so the slot is sized as a slice from the first `let` and the array literal adapts into it. The same fix closes a sibling gap on a bare slice binding: assigning an array literal to an already slice typed place now coerces the literal into the slice representation instead of failing the same way.
- A bare top level function bound to a local and called in return position now lowers correctly. `g := inc; return g(41)` used to drop the call at codegen and emit a bogus literal return that clang rejected; a function value with no captures now lowers to the same closure shape, an environment pointer paired with a forwarding thunk, that a capturing lambda already uses, and the call dispatches through the same indirect path.
- The checker now catches a value `self` used where a pointer is required, at a return, a direct call, or a method call, rather than leaving it to a backend type error. Methods on an enum type are rejected, since only struct receivers dispatch. `self` used as a pointer through an explicit dereference is a clear error.
- Enum values are constructed through the qualified `Enum.Variant` form the standard library already uses; a bare variant name is rejected with the qualified form named, and a constructor's argument count and payload types are now checked, closing a silent coercion the unqualified form and the qualified form both carried.

Examples and goldens: close to a hundred pin the escape and alias work, covering every binding introduction site's alias link and its clean accept twin, the store into argument and channel sink relations threaded through a plain call, a method receiver, and a lambda, the opaque callee reject and its heap backed accept twin, and one residual gap kept as a check only marker rather than a run golden, an alias buried in a call returned aggregate. The match binder probe over a locally constructed enum, once that residual's twin, now runs: the local enum construct and match codegen path landed, so `aliasmatch_ok` builds `Opt.Some(c)`, matches it, and reads the mutation back through the copied pointer. Six cover the future container fixes, the annotation, argument, and container forms plus the drop, spawn capture, and frame view guard rails that had to keep firing. Five cover the mutable tuple and slice assignment coercions, including the escape check still firing across the reshaped storage. Four more cover the value `self` in a pointer position, a return, a method call argument, and an enum shaped impl, each rejected, plus the value returning twin that still runs clean, and four cover the qualified enum constructor, a rejected bare variant name, a wrong arity, and a mistyped payload, plus a three shape running golden that builds, matches, and passes the qualified form as a by value argument. Thirteen parser termination tests, kept in their own file rather than the golden suite, drive the recursion ceiling and the recovery loop invariant against a deep nesting and flood corpus.

Deferred. An alias buried inside an aggregate a call returns is not yet caught: `wrap(c)` returning `Store{c: c}` forms no edge from the binding that receives the struct back to `c`, so a store through the returned struct's field and a later use of `c` on its own reads clean when it should not. A nested enum variant's payload is not yet alias linked either, though this stays safe today since a locally constructed enum copies its payload rather than aliasing it; the day enum payloads alias, the two gaps close together. Name resolution stays out, literal IPv4 addresses only.

## 0.4.4

The second platform and the hardening pass. The reactor's poller splits behind a six function seam so a second backend can sit beside the first: epoll stays the Linux path, byte for byte unchanged, and a kqueue backend for the BSDs and macOS is written against the same seam. The syscall surface hardens against the three things a networked program meets in the wild, a peer that hangs up mid write, a signal that interrupts a blocking call, and a process that runs out of file descriptors. Four async stress goldens pin the runtime under load. 325 unit tests and 268 golden integration tests pass, the reactor seam and the four stress goldens ThreadSanitizer clean.

The poller seam and the second backend.

- The reactor splits into a portable core and a poller backend. The core keeps the watch registry, the armed gauge, the arm, fire, and drop path, and the start and stop lifecycle; the backend is six functions, `create`, `destroy`, `arm`, `disarm`, `wait`, and `wake`, over a normalized readiness mask, chosen at compile time by a platform guard. `reactor_epoll.c` is the Linux backend, the existing epoll fd, eventfd sentinel, and `EPOLLONESHOT` path lifted verbatim, so every reactor and net golden and the seam under ThreadSanitizer are unchanged. The split moves no lock boundary and no gauge raise or drop, proven by re-running the goldens and diffing the emitted IR empty.
- `reactor_kqueue.c` is the BSD and macOS backend over kqueue and kevent, an `EVFILT_USER` event as the wake sentinel in place of the eventfd and `EV_ADD | EV_ONESHOT` for the one shot arm. It is written and reads clean but is not compiled or run on this machine, which is Linux with no kqueue header; a BSD or macOS runner is what compiles and exercises it. One behavior diverges and is documented rather than smoothed over: a close while armed then reused file descriptor re-arms clean on epoll, whose registration the close already dropped, but faults on kqueue, since `EV_ADD` cannot fail on a duplicate the way `EPOLL_CTL_ADD` returns `EEXIST`, so the kqueue backend reproduces the already armed fault by probing the registry first. Both backends reject a readiness watch on a regular file, epoll through `EPERM` and kqueue through an `fstat`.

SIGPIPE, EINTR, and fd exhaustion.

- `SIGPIPE` is ignored process wide by a load time constructor, so a write to a closed peer, whether a pipe or a socket, returns an error value instead of killing the process. The non blocking write classifies the broken pipe distinctly, `broken pipe`, from a generic `the write failed`; a peer reset, which is `ECONNRESET` rather than `EPIPE`, falls to the generic one.
- Every blocking syscall retries on `EINTR`: the reactor wait, the stop sentinel read and write, the non blocking byte calls, `accept`, and `connect`. `close` treats `EINTR` as success rather than retrying, since the descriptor is already gone on return and a retry could close a reused one. The fast, register only calls that do not return `EINTR` are audited and left alone.
- File descriptor exhaustion, `EMFILE` from the process limit or `ENFILE` from the system, surfaces at every socket and pipe mint as a handled `too many open files` error, never a crash. The half open descriptor is closed before the error returns, so nothing leaks, and the reactor stays usable, so a program recovers once the limit lifts. On accept the exhausted return is terminal, not a would block, so the accept loop cannot spin on a listener that stays ready.

Async stress goldens.

- Four goldens pin the runtime under load, each exact or commutative so its output is deterministic regardless of interleave: 2000 zero delay timers minted then awaited, 1000 tasks ten in flight across a hundred batches, 100 TCP connections against one accept loop with the echoed byte sum fixed at 4950, and 10000 pool tasks each writing its index to one channel that a single loop thread drains to an exact fold. The timer count is pinned at 2000 rather than higher because the timer cancellation is a linear scan, an honest cap for wall clock speed with every timer still minted and awaited. The pool saturation and accept storm goldens are ThreadSanitizer clean.

The spec's async chapter.

- The reference gains the reactor's portability, the poller seam and its two backends and the one documented divergence, and the hardening contract for `SIGPIPE`, `EINTR`, and fd exhaustion, and states plainly that the async state machine lowering landed in line rather than waiting for a later release.

Deferred. The kqueue backend waits on a BSD or macOS runner to compile and exercise the full reactor, net, and stress matrix and to pin the documented divergence. A checker gap the stress work surfaced and confirmed waits for 0.5.0: a bare future from a direct async func call can only be named and awaited, not passed as an argument, annotated, or stored in a container, since the type check reads the call's raw return outside an await; it is a loud, safe reject, not a miscompile. Name resolution stays out, literal IPv4 addresses only.

## 0.4.3

Phase four of the async line: the awaitable channel, TCP, and the generic monad, plus a soundness hardening of the generic `do` path that turned out to sit under all of it. `do` notation grows up from a single concrete monad to any generic one, `chan_recv_async` gives a blocking channel receive a home on the event loop, `std.functional.io` ships an `IO<T>` monad, and `std.async.net` puts TCP over the reactor. 325 unit tests and 260 golden integration tests pass, the channel bridge seam and the accept loop ThreadSanitizer clean.

Generic `bind` through `do`.

- `do` notation composes over any generic monad, not only one whose `bind` is already ground to concrete types. `Maybe`, an `Either` shaped monad, and any user `monad Name { ... }` block generic over its element all thread through `do` the same way. The desugar emits the continuation chain over an open type hole, and monomorphization resolves and instantiates the `bind` and `unit` pair fresh at each `do` site: an argument pass, an expected type or annotation pass, and a lambda body pass, with the first pass to pin a type winning. This is the one compiler investment of the async line that pays outside async.
- A `do` over a type with no `monad` block is rejected at the names its desugar calls, `undefined name '<Name>.bind'` and `undefined name '<Name>.unit'`, and a `bind` whose signature drops the continuation parameter is an arity mismatch on the desugared call.

The soundness hardening under it.

- The open type hole the continuation carries lowers to the permissive unknown type, which is compatible with everything, so the continuation body escaped the width and type checks and an int32 and int64 mix inside a generic `do` continuation silently truncated. The fix runs a second, types only, pass of the real type checker over the monomorphized program, where every type is concrete, recovering the width and agreement checks with no duplicated logic while leaving the ownership, escape, and must handle checks to the first pass. The mix is now caught, `arithmetic mixes int32 and int64; match the widths`, and an inferred element type clashing with an explicit annotation is caught the same way, `return type does not match the function's return type`. The fix is general: the same recheck catches a width mismatch buried in an ordinary generic function body too. Pinned by the genericwidth and genericpin goldens.

The awaitable channel.

- `chan_recv_async(c: Channel<T>) -> Future<T>` makes a receive awaitable on the loop instead of blocking the caller, since a blocking `chan_recv` on the loop thread stalls every task. It mints a future and hands the blocking receive to a detached helper thread that completes the future off the loop thread, the live threads gauge raised before the helper starts and dropped strictly after the completion so the deadlock detector never false fires. A closed and drained channel completes with `receive on a closed, drained channel`. Because the helper is detached and cannot be joined, the drain discipline is close and settle, not close then join.

std.functional.io.

- `IO<T>` is a `monad IO { ... }` block over a plain struct, composing through the generic `do`. `run(io: IO<A>) -> A` is the one effect boundary, minting a future, offloading the carried value to a pool worker that completes it, and awaiting the result on the loop thread. The `IO` is eager over its value: `bind` applies its continuation immediately and stores no closure. A lazy `IO` that stores its continuation as a thunk is not expressible yet, since the escape check rejects a struct field holding a closure that captures a local and is returned, `a closure that captures a local escapes its frame; it cannot be returned`, a deferred item rather than an oversight.

std.async.net.

- TCP over the reactor's readiness futures. `tcp_listen`, `tcp_local_port`, and `tcp_close` are synchronous; `tcp_accept`, `tcp_connect`, `tcp_read`, and `tcp_write` are async funcs that await `readable` or `writable` and retry, so a server accept loop and its clients run as tasks under `async_run` and never pump the loop from inside a task. `tcp_connect` completes the non blocking connect handshake by awaiting writability then reading the socket error, so a refusal surfaces as a clean error not a broken descriptor. `tcp_write` sends every byte, looping until the buffer is gone, so a short write never drops the tail. Literal IPv4 addresses only, no name resolution. Awaiting a networking future outside an async func is rejected, `'await' is only legal inside an async func`.

Deferred. A lazy `IO` and the broader escape work it needs wait for 0.5.0. Name resolution and the second reactor platform wait for later in the line.

## 0.4.2

The largest release of the 0.4.x line, two tracks landing together plus a soundness hardening underneath both. The complete operator set closes out arithmetic and control expressions the language had leaned on a small subset of since 0.1.0, bitwise, compound assignment, increment and decrement, exponent, pipe, and an inclusive range. `async func`, `await`, and `async_run` then land the keyword layer the whole async line has been building toward: an async func compiles to a state machine over a heap frame, `await` is a statement level suspension inside one, and `async_run` is the only bridge a synchronous `main` uses to crank the loop. Underneath both, the escape check and the interface boxing codegen it rides on are completed end to end. 325 unit tests and 247 golden integration tests pass, the async transform included.

The complete operator set.

- Bitwise `&`, `|`, `^`, and unary `~` on integer operands, two's complement throughout, plus `<<` and `>>`. `>>` is always an arithmetic shift, sign extending, since dusk does not track signedness separately from the type at the point a shift lowers. A constant shift amount outside `[0, width)` is a compile error, and a dynamic one out of range is a named runtime fault, `fatal: shift amount out of range`, never a silently masked result.
- The full compound assignment family, `+= -= *= /= %= &= |= ^= <<= >>=`, lowered through a single load, operate, store on the place, with the place's own address, including an index expression, computed exactly once: `xs[pick()] += 5` calls `pick()` once. Postfix `++` and `--` are statement only and produce no value, each desugared to a compound assignment with the literal `1`.
- Exponent `**`, right associative, `2 ** 3 ** 2` grouping as `2 ** (3 ** 2)` and unary minus binding tighter so `-2 ** 2` is `4`. An integer base lowers through `cool_pow_i64`, repeated squaring in `uint64_t` matching the wraparound plain multiplication already has, `0 ** 0` is `1`, and a negative integer exponent faults by name, `fatal: negative exponent in integer '**'`, rather than returning a wrong value. A float base or exponent lowers to the LLVM `pow` intrinsic.
- The pipe `|>`, a parse time rewrite of `x |> f(a)` into `f(x, a)`, left associative and the loosest operator so it grabs the whole expression to its left before piping; the right side must be a function name or a call.
- The inclusive range `..=` in a slice index, `a..=b` as `a..b+1`, the endpoint moving before the ordinary bounds check runs.
- A full thirteen level precedence ladder, loosest to tightest: range, pipe, or, and, comparison (not chainable), bitwise or, bitwise xor, bitwise and, shift, additive, multiplicative, exponent, then unary and postfix, documented in the reference alongside every family above.
- Three operators considered and left out, with the reasoning kept alongside the ones that shipped: the ternary `?:`, optional chaining `?.`, and null coalescing `??` have no place in a language with no null, where `if` already covers selection and `?` is reserved; spread `...` has no varargs to spread into; and a concatenation operator is redundant with `StringBuilder` and the allocator a slice concatenation would need, which an operator has nowhere to name.

`async func`, `await`, and `async_run`, the keyword layer of the async line.

- `async func f(...) -> T` compiles to one poll function, `@async.f.poll`, over a heap allocated task frame, entry block first: every frame slot is GEPed once there, so every pointer dominates every resume edge, then a switch on the state word dispatches to `start` or to whichever await registered a resume label. Calling an async func mints the task and its `Future<T>` and runs nothing until the loop cranks it.
- `await` is a statement level suspension, not an operator, legal in exactly four shapes: `x := await f`, `x, e := await f`, a void discarding `await f` only when the awaited element is void, and `return await f`. It never appears mid expression, `'await' cannot appear mid-expression; give the awaited value a name, as in v, e := await f`, and it composes with `while`, `if`, `for` over a named fixed array, and a `match` arm reading its payload after the await, each surviving the resume because the loop's counter, the array's pointer, length, and index, and the match payload are frame slots, reloaded on resume rather than kept in a register the suspension bypasses.
- A wide compile fail surface pins the signature rules: no type parameters, no future, slice, closure, or interface value as a parameter or return, `main` and a method cannot be async, an async func's name cannot be stored as a value only called, a bare call that drops its future unawaited is rejected, `async_run` takes a direct call written at the call site never a stored future, and it cannot be called from inside an async func. Ordinary rules keep applying underneath: `move(p)` into an awaited call still kills the mover's name past the await, and the await's own error word falls under the ordinary must handle rule.
- Determinism: one loop thread, a FIFO ready queue, and one scheduler turn per await, even against an already complete future, make a loop only program's interleaving exact and reproducible; `roundrobin` asserts two workers printing `a0 b0 a1 b1 a2 b2`, not a race. Anything crossing the pool or a spawned thread funnels back through one future completion and one enqueue.
- Run to completion, no cancellation: a task always reaches its return, which is what makes a `defer` registered before an await sound, replayed in reverse order exactly once at true completion, never at a suspension. `await` is monadic bind performed by the compiler, sequencing a suspending computation and threading its error alongside its value with no rejection channel anywhere.
- The fault family, each a named abort pinned by a golden: `fatal: use of a dead future` on a second await, poll, or free; `fatal: two tasks await one future`; `fatal: async_run re-entered the event loop`; `fatal: the event loop is idle but work is still pending` when nothing, no timer, no live thread, no in flight pool task, no armed watch, can complete a parked await; and `fatal: a task resumed in an invalid state` for a poll's entry switch seeing a state its own emission never produced.
- The cost table, stated plainly in the reference: an async call is one frame allocation plus one future record, an await is one enqueue and one scheduler turn, and a leaf future is one generational record, no different from what 0.4.0 already costs by hand.
- The completer doctrine: a future belongs to the event loop thread, so a spawned or submitted lambda never captures the typed handle, only its raw words, and completes through `complete_raw`, the same completer surface a task's own pool offload rides. The pumping doctrine: inside an async body the only wait is `await`; a manually pumped wait on another future parks the one crank thread and turns a stuck task into the same named idle fatal a genuine deadlock produces.
- The six 0.4.0 and 0.4.1 async examples built by hand around a completer lambda now complete through `complete_raw`, goldens unchanged, and the stdlib `await` function keeps working for sync code, since the keyword only absorbs the name `await` as a suspension inside an async func body.

Hardening the escape check and its codegen forced.

- The escape check now rejects a frame local view, a slice into a local array or a closure capturing a local, escaping through any by value carrier at a return: bare, tuple, struct, enum, fixed array, and a generic field at any nesting depth, flow sensitively through a binding, an alias, or a match arm rather than only the returned expression's own syntax. The messages are unchanged, `a slice into a local array escapes its frame; put the backing on the heap` and `a closure that captures a local escapes its frame; it cannot be returned`, now firing everywhere the value can actually hide.
- An interface value buried in a struct field, an enum payload, or an array element now boxes correctly, so method dispatch through those carriers works instead of miscompiling. Returning an interface value by value is rejected outright, `returning an interface value is not supported; return the concrete type or a pointer to it`, since the boxed payload would sit in a dangling frame slot, and an interface value inside a tuple is rejected consistently at both a return and an argument, `an interface value inside a tuple is not supported; return or pass the concrete type, or box it outside the tuple`.
- Slice covariance, passing a slice of a concrete struct where a slice of an interface is expected, is rejected: the two share a machine shape but reinterpreting one as the other reads every element as a boxed interface and corrupts memory, `cannot pass a slice of '<concrete>' as a slice of interface '<iface>'; a slice of concrete values cannot be reinterpreted as a slice of interfaces`. An array literal coercing element by element, and a slice of an interface passed where that interface is already expected, are both exempt, since neither reinterprets an existing buffer.
- Two codegen gaps surfaced and closed along the way: a float32 print reaching the f64 sink now goes through the missing `fpext`, and a fixed array's `.len` lowers to its compile time constant instead of misreading a slice's runtime field.

Examples and goldens: sixteen for the operator set, covering the bitwise family, all ten compound forms chained on one binding, the single evaluation of a compound target's address, postfix increment and decrement including an int8 wrap, right associative exponent with its float twin, the pipe rewrite left associative and through a paradigm gated builtin, the inclusive range's off by one and empty cases, a narrow integer tuple member's per position width adapt, and the constant and dynamic shift fault pairs. Close to fifty for the async keyword layer, split three ways: a skeleton run through `async_run` with no awaits, arguments landing at the frame offsets the poll reads, a chain of awaits across three tasks, two tasks fanned in, timers and a pool offload awaited through `complete_raw`, shadowing across awaits, defers spanning awaits and an early return, closures and boxed interfaces captured before and read after a suspension including per iteration distinctness inside a loop, async recursion, and the FIFO round robin; the whole compile fail surface for the signature and statement rules above; and the runtime fault twins, a double await, two tasks on one future, and an off thread touch. Thirteen more cover the escape and interface hardening: a struct field, an enum payload, a plain array, a nested array, and an array of slices, each boxing an interface correctly, the tuple position rejected the same at a return and an argument, an interface returned by value rejected, and the covariance family, an assignment, an enum payload, a struct field, a call argument, and the one legal same type pass through that must not over reject.

The 0.4.x line continues with networking, an awaitable channel, and monadic async sugar in 0.4.3.

## 0.4.1

The reactor, the second phase of the async line. File descriptor readiness becomes a one shot future on the event loop, one C thread that turns `epoll` readiness into a completion behind a new `std.async.io`, with pipes as the deterministic rig every golden runs it through. Zero compiler changes and one Rust line, the new runtime file joining the link; everything else is the runtime file, one standard library module, and the reference.

The reactor thread and its lifecycle.

- `reactor_start() -> error` starts the thread; the error fires on a double start, an operating system refusal setting up its epoll descriptor and its stop sentinel, or a start landing while a concurrent stop is still in flight. `reactor_stop() -> void` flips the reactor stopped, signals the thread through an eventfd, waits for it to finish delivering everything already ready, then joins.
- The reactor is restartable: a fresh epoll descriptor and eventfd on every start, mirroring the loop it serves, since none of its state survives a stop. The sanctioned order is `loop_init`, `reactor_start`, every watch armed and fired, `reactor_stop`, `loop_free`.
- The reactor thread never raises the live thread gauge; it is spawned with a bare `pthread_create`, not the tracked spawn path, or the deadlock gate could never fire while the reactor idles. Its own mutex guards only lifecycle fields and is never held at the same time as the loop's, the lock ordering rule stated in the runtime file's header comment.

The watches and the gauge.

- `readable(fd) -> Future<int64>` and `writable(fd) -> Future<int64>` in `std.async.io` arm a one shot watch and return a future completed with the readiness mask, 1 readable, 2 writable, 4 hangup, 8 error, ORed together. The watch fires exactly once by construction and the reactor drops it the instant it fires.
- Only one armed watch is allowed per file descriptor at a time; a second watch on an fd that already carries one faults rather than errors, since the signatures carry no error channel. `future_free` does not disarm a watch: a freed future's watch stays armed until it later fires against a dead record and loses like any other refused completer.
- Arming a watch raises a third gauge into the deadlock gate beside the live thread count and the pool's in flight count: an armed watch is a possible completer, so an otherwise idle await keeps parking instead of aborting. The gauge is raised before the watch is registered and dropped strictly after its completion is visible under the loop's lock, so the idle fatal can never fire against a completion still in flight, and every drop still kicks the loop even when the completion it followed was refused.

The byte surface.

- `pipe_new() -> (Pipe, error)` makes a close on exec, blocking by default pipe; `fd_nonblock(fd) -> error` sets a descriptor non blocking; `fd_close(fd) -> error` closes one. `read_nb` and `write_nb` move bytes through a caller staged buffer, the channel element idiom, and never block.
- Both refuse with "would block" when the operating system has nothing to give or take, the one canonical recoverable string in both directions. A `read_nb` returning a count of zero with no error is end of stream, every writer closed.
- Writing to a pipe whose read end is closed delivers SIGPIPE and kills the process. Signal hardening is deferred to a later phase; the honesty note lands in the reference and no golden writes to a pipe with no reader.

The fault family.

- Five new fatals, each named: "the reactor is not running" arming before a start or after a stop, "the file descriptor already has an armed watch" for a second watch on an armed fd, "a readiness watch was armed on an invalid file descriptor" for a closed or nonexistent fd, "a regular file cannot report readiness" for a descriptor epoll cannot poll, and "the reactor stopped while a watch is still armed", the watchleak rule: stopping with a watch not yet fired would otherwise strand a parked awaiter forever or drop the gauge and lie to the deadlock gate later, so the violation gets a name instead. Two narrower fatals guard the reactor's own internals, "the reactor could not wait for readiness" and "the reactor could not be signalled to stop", and watch record exhaustion reuses "out of memory".

Hardening the adversarial soundness review forced.

- Consuming or releasing a future now retires its record before the loop mutex unlocks, not after, closing a window where a completion racing the retire could write its element into a block already handed back to a new allocation. The generation bump and the completer's own gen and state checks now run under the same lock, so a completion that already passed those checks has written before the retire can free the block, and one that has not yet checked finds the bumped generation and is refused.
- The reactor keys each armed watch in a registry from fd to its current watch record, guarded by the reactor mutex, and deletes the kernel registration only when the firing watch still owns the entry. Without it, a close while armed misuse could let the fd number be reused and armed again before the stale watch's event was handled, and the stale fire's `EPOLL_CTL_DEL` would tear down the innocent successor's registration by fd number alone and hang its awaiter; an arm on a reused fd now overwrites the registry entry, so a stale fire finds it already gone and skips the delete.
- Reactor lifecycle races get the pool's discipline: a `reactor_stop` racing a concurrent one waits on a condition variable until the winner finishes draining, joining, and closing, so every caller returns holding the same stop guarantee instead of one racing ahead of the teardown. A `reactor_start` landing while a stop is still in flight is refused rather than building a fresh epoll and thread the resuming stop would then close out from under it.

Examples and goldens: seven clean, the reactor's lifecycle standing alone with no loop, the non blocking byte surface round tripping a value through a pipe end to end including EOF, a readiness watch completing a parked await with no other completer in the picture, a spawned thread waking a parked await through the reactor with the thread exit gauge dropping first, timers and a readiness watch sharing one loop with awaits returning in exact program order, four pool workers writing through four watches funnelling to one exact sum, and a writability watch completing on an already writable pipe end with mask 2; three fault, stopping the reactor with a watch still armed, a second watch colliding on one fd, and a watch armed on an invalid fd. All named `reactorlife`, `wouldblock`, `readywait`, `pipewake`, `timerinterleave`, `reactorsum`, `writewatch`, `watchleak`, `doublewatch`, and `badfdwatch`.

The release also lands a documented local ThreadSanitizer recipe, `docs/tsan.md`: rebuild a golden's emitted IR alongside the four runtime files under `clang -fsanitize=thread`, then run it in a loop. It was run against `reactorsum`, `pipewake`, and `racingcomplete` before this release, the arm and fire path, a cross thread wake racing the reactor's own gauge drop, and the racing completer path the reactor's fire step reuses.

The 0.4.x line continues with the async and await keywords in 0.4.2.

## 0.4.0

Futures and the event loop, the first phase of the async line. A one shot completion future replaces the hand rolled channel and counter of the 0.3.3 offload shape, the loop parks instead of polling, and an await that can never finish aborts by name instead of hanging. One compiler change only, the channel element ban extended to the future's minting sites; everything else is a runtime file, three standard library modules, examples, and the reference, riding the pool and monitor machinery the 0.3.x line built.

The future.

- `std.async.future` ships `Future<T>`, a one shot completion slot minted pending with `future_new`, the element type pinned by the binding annotation like `chan_new`. The handle is a plain pair of words and copies freely, every copy naming the same future, which is how a pool lambda captures it.
- `complete(f, v, e)` stores the value and the error together, from any thread, and wakes the loop. The awaiter reads exactly the pair the completer supplied, so an offloaded body hands its own failure through unchanged and there is no rejection channel anywhere, the errors as values rule extended to completion. The second completion is refused with "future already completed" and its value is dropped, whether the loser lands before or after the awaiter consumes the future, so racing completers stay visible instead of silently last writer wins and never need to outrun the awaiter. The adversarial review forced that last clause: the first cut faulted a loser that arrived after the consume, which an interleaving probe caught in one run out of eight.
- The channel element ban applies at `future_new` and `future_wrap`: an element containing a slice, closure, or interface value is rejected at compile time, since a view of the completing thread's frame would dangle in the awaiter. The one compiler change of the release, five lines in monomorphization beside the channel arm it mirrors.
- The record lives in the generational heap and consuming it retires it, so a future is awaited once the way a thread is joined once, and the second consume faults with "use of a dead future", the double join machinery on a future.
- `await` parks until completion, `await_timeout` parks at most the given milliseconds against the monotonic clock and leaves the future live on "await timed out", the recoverable escape hatch, and `try_poll` never parks, reporting "future is pending" until it consumes a ready future. `future_free` releases a future that will never be consumed.

The loop.

- The loop is a process singleton like the pool, started with `loop_init` in `std.async.loop` on the thread that consumes futures. Completion is legal from any thread; every other touch asserts the owner and faults by name off it, so the single threaded discipline is mechanical, not documentary.
- `std.async.time` ships `sleep_async`, a future the loop's timer heap completes with 0 at its deadline. Timers fire while any await or poll runs, and two timers sharing a deadline complete in creation order, the heap keyed by deadline then sequence.
- An await that provably cannot finish is a deadlock, not a hang: with no timer pending, no spawned thread alive, and no pool task in flight, nothing can complete the future, and the wait aborts with "the event loop is idle but work is still pending". The gauges drop only after their bodies finish and every drop wakes the loop, so the gate never fires against a completion still in flight.
- The fault family is named end to end: consuming a dead future, touching the loop off its owner thread, touching it before `loop_init`, and the idle deadlock. The reference gains the futures section, the completion edge in the memory model, the two honest leak notes, and the cost paragraph.

Examples and goldens: the offload flagship rewritten on futures, three awaited reads folding to the same sum with the tick loop and counter gone, a plain spawned thread as the completer, two completers racing to one exact number in every interleaving, three timers awaited out of creation order, a parse failure crossing a future intact, the refused second completion, the pending then consuming poll, the timed await against a future completed later, and five named fault goldens, the dead future, the off thread touch, the loop never started, and the idle deadlock proven both immediately and after the last completer exits.

The 0.4.x line continues with the epoll reactor in 0.4.1 and the async and await keywords in 0.4.2.

## 0.3.3

The thread pool and the async substrate, closing the 0.3.x concurrency line. The non blocking and timed channel operations land, a global worker pool runs fire and forget tasks behind a new `submit` builtin, and the flagship example rehearses the exact park, wake, and offload shape the 0.4.0 event loop lowers onto, proven in user code before the async design starts.

The operations that refuse instead of parking.

- `chan_try_send` reports "channel is full" without waiting for room, `chan_try_recv` reports "channel is empty" without waiting for a value, and both still report the closed message their blocking twins use. The runtime side never sleeps: one lock, one check, one copy.
- `chan_recv_timeout(c, ms)` parks at most ms milliseconds against the monotonic clock the condvars were built on in 0.3.1, so a wall clock step cannot stretch or shrink the wait. Its error distinguishes "receive timed out" from the closed message, and the loop rechecks the ring after every wakeup, so a spurious wake cannot fabricate a timeout while a value sits ready.
- The value beside any refusing error is the zero pattern for `T`, the drained receive's contract.

The pool and the submit builtin.

- The pool is a process singleton in the runtime, a fixed worker count over an unbounded FIFO queue, deliberately below the language: a dusk level channel of closures would copy environments that point at the sender's stack, so the sound task queue sits in C. It starts once per process and stays down after a shutdown.
- `submit` is an always available builtin sharing spawn's whole argument rule, the lambda literal, the nullary void shape, the view free capture ban, and the borrowed captured pointers, through the same checker path and the same codegen env handoff. It returns only an error: the pool owns the task and results flow through a channel.
- A submission never blocks the submitter, the contract the 0.4.0 event loop needs for its offload path. The error exists only when the pool is not running, and on that path the runtime frees the environment itself, so a refused task leaks nothing.
- `pool_start(workers)`, `pool_shutdown()`, and `ncpu()` live in the new `std.concurrent.pool`. The shutdown stops new submissions, drains everything already queued, and joins the workers, so a fold over ten thousand submitted increments prints the exact count with no wait loop in sight.
- The shutdown guarantee holds for every caller, not just one: when two threads race into `pool_shutdown`, the loser waits until the winner's drain and join complete instead of returning early with tasks still queued. A pool task calling `pool_shutdown` itself is fatal by name, since the worker would otherwise join its own thread, undefined in POSIX, or wait forever on its own completion. A start the operating system refuses unwinds to pristine, so a transient thread limit does not poison the pool for the rest of the process.

Examples and goldens: one hundred tasks on four workers folding to the arithmetic sum, the ten thousand submission stress proving the drain, both refusal windows around the pool's lifetime, a try_recv polling loop against a slow producer, the three outcomes of a timed receive, and the offload flagship, a main loop ticking on `chan_recv_timeout` while pool workers run blocking file reads and completions drain through a channel.

The 0.3.x concurrency line is complete: threads and atomics in 0.3.0, channels in 0.3.1, mutexes and condition variables in 0.3.2, and the pool substrate here. Next is the 0.4.x async line on top of it.

## 0.3.2

Mutexes and condition variables, the third phase of the concurrency line. Shared mutable state gains its sanctioned shape, a raw buffer guarded by a lock, with every classic pthread misuse turned into a named fault. No compiler changes at all: the whole release is runtime shims, a standard library module, examples, and the reference.

The primitives.

- `std.concurrent.sync` ships `Mutex` and `Condvar` as one word handles over runtime shims, the channel's pattern. `lock` blocks until the mutex is free, `unlock` releases it, `cond_wait` releases the mutex around its sleep and reacquires it before returning, `cond_signal` wakes one waiter, `cond_broadcast` wakes all.
- The mutex is the error checking pthread kind, so relocking a mutex the thread already holds and unlocking one it does not hold, both undefined in the default flavor, fault by name. The runtime adds what the kind cannot: a trylock probe makes freeing a held mutex fatal, and the fault paths branch on the actual error code, so operating on a mutex already freed reports an invalid mutex instead of blaming a holder that does not exist.
- The condvar record carries a waiter count beside the pthread object, raised before the wait releases the mutex, so freeing a condition variable a thread waits on faults by name. The bare destroy would hang forever on glibc, quiescing for a waiter no signal will ever release, the worst failure shape in the toolbox.
- Condition variables run on a CLOCK_MONOTONIC clock like the channel's, ready for the timed waits arriving in 0.3.3. Wakeups can be spurious, and the reference states the rule: a wait always sits in a loop that rechecks its predicate under the lock, and every concurrent wait on one condvar names the same mutex.
- The blessed idiom inside a function body is `lock(m)` then `defer unlock(m)`, so every return path releases, verified working end to end.

The memory model.

- The mutex edge joins the sanctioned list: an `unlock` happens before the next `lock` of the same mutex. Hand built `*raw` sharing stays on the honor system unless a mutex guards every touch, and the reference now says exactly that.
- Blocking waits still have no timeout until 0.3.3, so the reference names the deadlock hazard plainly.

Examples and goldens: four threads driving one counter to exactly ten thousand under a mutex, a two account transfer loop whose invariant holds to the digit, a bounded buffer hand built from one mutex and two condition variables proving the primitives express what channels provide natively, a condition variable ping pong whose six lines alternate deterministically, and three named fault goldens, freeing a held mutex, unlocking an unheld one, and freeing a condition variable with a parked waiter, the last made deterministic by the waiter count rising before the wait releases the mutex.

Planned next in the line: the timed and non blocking channel operations, the thread pool, and the async substrate rehearsal in 0.3.3.

## 0.3.1

Channels, the second phase of the concurrency line. A bounded, thread safe queue moves values and ownership between threads, built as a standard library generic over a textbook monitor in the runtime, with no new syntax. One new rule guards the crossing: a channel element must be free of frame views, the same ban spawn captures enforce.

The channel.

- `std.concurrent.channel` ships `Channel<T>` with free functions in the `Vector` pattern. `chan_new(cap)` builds a bounded queue whose element type the binding annotation pins, the sizing rule `alloc` already uses, so a bare `jobs := chan_new(8)` is a compile error. `chan_send` blocks while the channel is full, `chan_recv` blocks while it is empty, `chan_close` idempotently wakes every blocked party and discards nothing buffered, and `chan_free` releases the monitor.
- `chan_send`'s error exists when the channel is closed. `chan_recv`'s error exists once the channel is closed and drained, so a loop breaking on `e.exists()` consumes everything senders delivered. The value beside that error is the zero pattern for `T` and means nothing.
- The runtime monitor is one mutex, two condition variables on a CLOCK_MONOTONIC clock so the timed receive planned for 0.3.3 cannot be confused by a wall clock step, a ring buffer, a closed flag, and a count of blocked waiters. Construction aborts on a capacity below one or exhaustion, the allocator's contract, since a channel that cannot exist has no error path a fresh program could act on.
- Freeing a channel while a thread is blocked inside a send or receive is fatal with a named message, caught best effort under the monitor lock. The sanctioned shutdown order is close, then join, then free, and the language reference documents it.

Ownership and types.

- Ownership crosses threads by moving a managed pointer through a channel: `chan_send(c, move(p))` kills the sender's name through the ordinary argument position move, and the receiver binds a fresh owner through the ordinary call returns ownership rule. The compile fail twin proves the sender cannot touch the record again.
- A channel element containing a slice, a closure, or an interface value, wherever it sits, including buried in struct or enum fields, is a compile error at the instantiation, since each may view the sending frame and the ring would deliver a dangling view to another thread. The walk is the mono side twin of the spawn capture check.
- A `*raw T` now passes anywhere `*void` is expected, the direction the channel's element staging needs. Codegen always lowered `*void` to the same bare word, so the gap was the checker's alone. The reverse direction stays rejected, because a `*void` that could become a typed `*raw T` would let a managed pointer launder through `*void` into a dereferenceable alias the generation check cannot see.
- Dereferencing a null managed pointer now faults by name instead of dying by raw signal: the untracked generation zero path tests for null and calls the new null fault, which flushes stdout first like every fault. The drained receive's zero pattern for a pointer element is exactly this null, so the natural consumer mistake gets a named message.
- A moved send refused by a closed channel loses its record, and managed pointers still buffered at `chan_free` leak as raw bytes. Both are documented in the reference and the module, neither is corruption, and neither happens in the sanctioned close, join, free order where senders finish first.
- The memory model gains the channel edge: a `chan_recv` happens after the `chan_send` that delivered its value.
- The foreign function section now states that a symbol resolves against anything the binary links, libc and the dusk runtime today, the loosening the concurrent modules already rely on.

Examples and goldens: a three stage pipeline folding to one sum, a four worker fan in, the receive until closed idiom, one hundred sends through a capacity one channel to force the blocking path, the ownership handoff plus its compile fail twin, the rejected slice element buried in a struct field, and the named null fault on a drained receive's placeholder.

Planned next in the line: mutex and condvar in 0.3.2, the timed and non blocking channel operations and the thread pool in 0.3.3.

## 0.3.0

Threads, the first phase of the concurrency line. OS threads with zero new syntax, a thread safe runtime underneath them, and atomics so parallel programs can prove themselves deterministically.

The thread primitive.

- `spawn(f: () -> void) -> (thread, error)` starts an OS pthread running a lambda literal, and `join(t: thread) -> error` waits for it. Both are always available builtins, paradigm agnostic like `alloc` and the error machinery, and both failures ride the must handle rule.
- `thread` is an opaque builtin type. The handle is a record in the generational heap and `join` retires it, so a double `join` faults through the same generation check a use after free hits.
- The spawned lambda's captured environment is copied into a private heap block the runtime frees after the body returns, so a thread never reads another thread's stack. A nullary void lambda already compiles to the pthread start shape, so the trampoline is direct.
- `spawn` requires the lambda literal at the call site, since only the literal knows the environment layout. Spawning a closure variable is a compile error naming the wrap in a literal fix.

Safety across threads.

- Captures cross by immutable copy, the rule lambdas always had. Capturing a slice, a closure, or an interface value is rejected wherever it sits, including buried in a struct or enum field, since each may view the spawning frame.
- A captured managed pointer is a borrow inside the thread: reading through it works, freeing or moving it there is a compile error, and a moved away pointer keeps its moved state, so capturing it stays the error a plain lambda gets. An owner freeing while a thread still holds the borrow is caught by the generation check at the thread's next dereference.
- The generational heap is thread safe: one mutex guards the free list and the debug tables, the generation word is bumped and read atomically on both sides of the check, and the dereference check stays armed on every thread.
- join's generation check and the handle's retirement run in one heap critical section, so a double join faults deterministically even when two threads race on copies of the same handle. The spawn environment allocation aborts on exhaustion rather than writing captures through a null.
- The language reference gains a Threads and the Memory Model section that states the data race stance honestly: races are undefined, sanctioned paths (spawn copies, atomics, join, and the coming channels and mutexes) provide the ordering they name, and the generation check degrades from a guarantee to a best effort backstop under a true race.

Standard library and tooling.

- `std.concurrent.atomic` ships `AtomicInt` with sequentially consistent `atomic_load`, `atomic_store`, `atomic_add`, and `atomic_cas` over a heap word, the sanctioned shared counter before mutexes land.
- `std.concurrent.thread` ships `sleep_ms`.
- The runtime grows `runtime/thread.c` with the spawn trampoline, join, sleep, and the atomic shims, and the driver links it with `-pthread` for older toolchains.
- Examples and goldens cover spawn and join ordering, a two thread atomic counter, per iteration capture copies, a deterministic cross thread use after free fault, a double join fault, and the compile time rejections.

Planned next in the line: channels in 0.3.1, mutex and condvar in 0.3.2, the timed channel operations and the thread pool the async release builds on in 0.3.3.

## 0.2.6

Hardens the whole 0.2.x line one level deeper. Where 0.2.5 closed exact reproductions, this release closes each rule's family, the range as well as the index, the binding site as well as the call site, the bare call as well as the qualified one, and lands the rules deferred along the way.

Memory and bounds.

- `alloc()` with no value sizes the block from the declared pointer annotation, so `p: *Big = alloc()` allocates all of `Big` instead of an 8 byte default that corrupted the heap. The unannotated form `x := alloc()` is a compile error.
- Returning an array literal where a slice is expected is caught by the escape check, closing a dangling stack slice the range form already rejected.
- A range slice validates `lo <= hi <= base.len` and traps on a miss, so a slice can no longer fabricate a length that launders out of bounds reads past the index check.
- `FixedBuffer` and `Arena` check capacity and honor alignment, aborting on exhaustion instead of handing out memory past the buffer. `vec_get` is bounds checked. `parse_int_radix` rejects overflow instead of wrapping.

Types.

- Integer and float widths are tracked in the checker. `int32 + int64` is a compile error instead of a silent truncation, in arithmetic, comparison, assignment, argument passing, and returns.
- A bare literal adapts to the width beside it, an unannotated literal binding hardens to the default width, and a literal that cannot fit its annotated or suffixed width is rejected.
- Immutability covers projections. `xs[i] = v` and `s.f = v` need a `mut` root binding, while a store through a pointer dereference or a slice stays governed by the pointee.
- A field store on an undereferenced pointer, which previously compiled and did nothing, is an error that names the `(*p).field` fix.

Semantic analysis.

- Binding or returning a struct where an interface is expected requires the impl, the same conformance rule call sites gained in 0.2.5, so a missing impl is a checker error instead of an undefined vtable in clang.
- `match` requires an enum scrutinee. A non enum previously executed every arm in sequence.
- A `defer` inside a conditional or loop is rejected, since registration is lexical and every return replays the list, which cannot honor a conditional registration.
- The binding level must handle rule lands. A bound error must reach `exists`, `check`, or `ignore`, or be returned to the caller, and printing it does not count.
- A non void function must return on every path, where falling off the end silently produced a zeroed value.
- `main`'s signature is validated. The allocator form is rejected until its entry wrapper exists, so it cannot read garbage registers, and any other unsupported shape is named.

Monomorphization.

- A destructured binding takes its tuple element's type, so valid generic code over a destructure no longer dies with a type mismatch in clang.
- A type parameter no call site pins down fails `dusk check` at the source line instead of silently defaulting to `int64` and reinterpreting values.
- An impl block on a generic type is diagnosed instead of silently dropped, and a duplicate `impl I for T` is a checker error instead of a duplicate symbol at link.
- Builtin results such as `read_file`'s `(string, error)` pair type their bindings for generic inference.

Modules.

- The loader renames each imported module's private top level items with a per file suffix before the merge and rewrites the module's own references to match. A bare call can no longer reach another file's private helper, and two modules may each keep a private `helper` without colliding. Exported names and foreign functions never change.

Printing.

- A struct prints through its `Display` impl's `toString`, and a struct without one is a compile error. Printing an enum, a slice, a tuple, or a pointer is an error instead of silence.
- `printerr` lands as a stderr println, flushing stdout first so program output precedes the message even when the program aborts right after.
- A string literal first argument is a format string at any arity, so `println("{}")` with no value is an error and `println("{{}}")` prints `{}` consistently.
- `toString()` on the empty error is the empty string, and the runtime printers guard a null pointer, closing a crash in `puts`.
- A bare `println()` prints a newline.

Tooling and internals.

- `dusk run` forwards trailing arguments to the program, so an argc and argv main sees them.
- The `monad` block is gated to the functional paradigm, matching `do` notation.
- Diagnostics for foreign blocks, impl completeness, and whole function errors carry real source spans instead of pointing at the file's first character.
- Identical string literals intern to one IR global, nominal type lookups in codegen go through a map instead of scanning item lists, and clippy is clean across all targets.
- The language reference documents every rule above. The suite grows to 195 unit and 49 golden tests.

## 0.2.5

Closes the gaps a specification review found, where a construct parsed and partly checked but leaned on permissive typing or late runtime behavior.

- `free` of a managed pointer runs the generation check, so freeing a stale pointer to a reused block faults at the free instead of corrupting the live owner.
- `for` loops lower to codegen over an array or a slice, where they were silently dropped.
- A bare statement that drops a fallible call's error is rejected, the first enforcement of the must handle rule.
- Reassigning an owning pointer is rejected as a leak, while a borrowing cursor still advances.
- Array and slice indexing is bounds checked and traps out of range, negatives included.
- `main(argc, argv)` gains a C ABI entry wrapper that builds the string slice, so `argv.len` matches argc.
- Passing a struct where an interface is expected requires an impl with every method, and an incomplete impl is rejected, both in the checker.
- Struct literals validate field names, duplicates, and completeness.
- A qualified module call to a private name is rejected.
- The language reference's string and error representations match the implementation.

## 0.2.4

The minimal foreign function interface, riding the raw pointer layer.

- A `foreign "C"` block declares external functions with dusk types and no body. Each binds to a C symbol of the same name at link, and a call type checks and lowers like any other.
- The boundary is the raw pointer layer only. Parameters and returns are scalars, `*raw T`, or `*void`. A managed `*T`, an aggregate by value, and an abi other than `"C"` are rejected.
- libc is the reachable library, since the toolchain already links it.

## 0.2.3

Escaping value lifetimes, the last memory safety phase.

- Returning a slice that views a frame local fixed array is a compile error, since the array is reclaimed with the frame. A heap backed slice or a slice parameter still returns fine.
- Returning a closure that captures a frame local is a compile error, while a capture free closure is a plain function pointer and may be returned.
- Pointer escapes were already covered at runtime by the generation check, since every pointer is heap allocated.

## 0.2.2

Single owner pointers, the static half of the ownership story.

- The checker tracks each managed pointer binding as an owner or a borrow. A plain copy of an owner is rejected and points at `ref` to alias or `move` to transfer.
- `move(x)` transfers ownership and invalidates the source, so a later use is rejected.
- A `ref` binding is a non owning alias, and a pointer parameter borrows. Freeing or moving a borrow is rejected, since only the owner does either.
- The raw layer, `*void` and `*raw T`, is exempt, and the runtime generation check backstops what the single block static pass cannot see.

## 0.2.1

Generational references, the runtime foundation of the memory safety line.

- A managed `*T` is a fat pointer, the data pointer paired with a remembered generation. The default heap writes a live generation in a header before each block, and `free` bumps it and parks the block on a size matched free list.
- Every managed dereference compares the remembered generation against the header and faults on a use after free, a double free, or a stale pointer to a reused block, in every build.
- The thin layer lands alongside. `*raw T` and `*void` are one word pointers with no generation, carrying strings, slice data, receivers, and collection buffers, with `ptr_add` for byte arithmetic.
- A generation of zero is the untracked sentinel, so a `using` allocator hands back unchecked memory and custom allocators keep working.

## 0.2.0

Mutable strings and concatenation.

- `std.string` ships `StringBuilder`, a growable heap buffer that keeps a NUL after the last character so a string view costs nothing.
- `concat` joins two strings into a fresh builder the caller owns, and the `cstr` builtin reinterprets a NUL terminated `*char` as a `string` at no cost.

## 0.1.5

Formatted printing.

- `print` writes with no newline and `println` appends one, where both previously appended it.
- `print` and `println` take a format string whose `{}` holes the value arguments fill in order, with `{{` and `}}` for a literal brace. The literal expands at compile time into typed prints, no runtime parser, no allocation, and a mismatched hole count is a compile error.

## 0.1.4

Console input and parsing.

- `read_line` reads one line and `read_all` the whole stream, each a `(string, error)` pair.
- `parse_int` and `parse_int_radix` parse signed integers, base 2 to 36, and `parse_float` parses a float through the runtime, each returning a value with an error.
- `read_int` and `read_float` in `std.io` compose the readers with the parsers.

## 0.1.3

Completeness of the planned core.

- Qualified call syntax. `std.io.print_line(x)` folds to the merged global, while method calls and enum constructors keep their meaning.
- `std.map`, a string keyed `Map<V>` with open addressing, doubling, and a `Maybe<V>` get, written in dusk.
- File I/O. `read_file` returns a `(string, error)` pair and `write_file` an `error`, both global builtins.

## 0.1.2

Methods and allocators.

- Every method takes its receiver by pointer, so a method mutates the receiver in place and a stateful allocator's bump offset persists across calls.
- The `using` `Allocator` interface works end to end, with `Heap`, `FixedBuffer`, `Arena`, and `Debug` in the stdlib, dispatched statically on a concrete type and through the vtable when erased.

## 0.1.1

Correctness and diagnostics.

- `char` is the unsigned range and zero extends, errors as values lower end to end with `exists`, `toString`, `check`, and `ignore`, and `reduce` returns `(T, error)` and guards the empty slice.
- Per file source tracking renders a merged diagnostic against the file its span falls in, an import's leaf segment is validated against the module's exports, and several monads coexist through `monad Name` blocks.

## 0.1.0

The core language, end to end. Paradigm directives gating procedural, functional, and oop features per file, structs, methods, enums with exhaustive `match`, interfaces with vtables, closures, monomorphized generics, functional builtins, `do` notation, errors as values under the must handle rule, explicit memory with `alloc`, `free`, and `defer`, a module system with a stdlib seed, and a golden test suite compiling and running every example.
