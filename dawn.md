# Dawn

Dawn is the package tool for the dusk language. A project is a directory holding a `package.dawn` manifest, a package is a git repository with one at its root, and a version is a git tag, so there is no registry to run or trust and the source you build against is the repository you can read. Dawn owns the network: it resolves each dependency's reference to a commit, writes the commit into `dawn.lock`, and checks the code out under `dawn_modules/`. The dusk compiler owns the build and never opens a socket, so once a project is fetched every build after it is offline.

## Usage

Dawn has eight commands. Each one runs from inside the project, the directory holding `package.dawn` or any directory under it.

```sh
dawn init [name]              # write package.dawn, a hello root, and a .gitignore line
dawn get                      # resolve every requirement, clone it, write dawn.lock
dawn add <alias> <src> <ref>  # add a requirement to the manifest, then fetch it
dawn update [alias]           # re-resolve one requirement, or all of them
dawn tree                     # print the dependency graph the lock describes
dawn build [--release]        # build the manifest's root with dusk
dawn run [args...]            # build it and run it, forwarding the args
dawn version                  # print the tool version
dawn help                     # the same usage, also --help and -h
```

Every one of them operates on a project, not on a file. The old file taking forms, `dawn get <file>` and `dawn build <file>`, are gone.

Start a project.

```sh
mkdir json && cd json
dawn init json
```

That writes a `package.dawn` naming the package, with `@dusk` set to the running toolchain's major and minor, a root source file, and a `.gitignore` carrying `dawn_modules/`. The lock is not written yet, since there is nothing to lock. Running it over a project that already has one is `package.dawn already exists in <dir>`.

Add a dependency. The alias is the name your imports spell, the source is the repository, and the reference is a tag, a branch, or a commit.

```sh
dawn add maybe github.com/choice404/dusk-maybe v0.3.0
```

`add` fetches first and writes the `@require` line only once the fetch has succeeded, so the manifest, the lock, and `dawn_modules/` move together and an add that fails leaves `package.dawn` and `dawn.lock` byte identical to what they were. Editing the manifest by hand and running `dawn get` does the same thing. Adding something already required is `'maybe' is already required in package.dawn; run dawn update maybe to move it`, which names the command that does move it.

Fetch what a manifest asks for, on a fresh clone of a project or after editing the manifest.

```sh
dawn get
```

`get` walks the whole graph. It reads each dependency's own `package.dawn`, resolves that package's requirements too, and lands every one of them flat in `dawn_modules/`. It resolves each reference to a commit, checks the repository out at that commit with the head detached, and writes one `@lock` line per requirement. It is idempotent, so a requirement already locked and already stamped costs no network at all.

```text
dawn: fetching maybe from https://github.com/choice404/dusk-maybe
dawn: cached json
dawn: 2 package(s) ready
dawn: wrote dawn.lock
```

Each checkout is stamped as it lands and the lock is written last, after the whole graph has. A fetch that fails partway leaves the lock exactly as it was, and everything already fetched stays where it is and reports `cached` on the next run, so `dawn get` after a failure resumes rather than starting over. Progress goes to stdout; every error goes to stderr under a `dawn: ` prefix and exits 1.

Move a dependency forward, which is the one command that changes a lock line on purpose.

```sh
dawn update maybe    # re-resolve one requirement
dawn update          # re-resolve all of them
```

Naming something the manifest does not require is `no package named 'nope' is required here`.

See what the project already has.

```sh
dawn tree
```

`tree` reads `package.dawn`, `dawn.lock`, and the stamp inside each checkout, and touches nothing else, so it answers the same on a machine with no connection. The root prints as its package and version, and every requirement under it as the alias, the source, the reference, and the first twelve characters of the commit the lock pinned, one step of indentation per level of the graph.

```text
app 0.1.0
  mid file:///tmp/mid.git v0.1.0 4c1b90e7a5f6
    leaf file:///tmp/leaf.git v0.1.0 8e14b7a03f95
  ghost github.com/a/ghost v9.9.9 (not locked)
```

Depth comes from reading each fetched package's own manifest, so what a dependency requires prints under it even though every checkout is a sibling in the flat `dawn_modules`, and an alias already printed with its requirements under it is not expanded a second time, so a diamond prints once at depth and a cycle cannot run away. A requirement no lock line pins prints `(not locked)` where the commit would be, and one the lock pins with nothing on disk holding it prints `(not fetched)` after it. Both are states a build refuses, and each names the fetch that clears it, so `tree` reports them rather than failing on them and exits 0 whatever the project turns out to look like.

Build and run.

```sh
dawn build
dawn build --release
dawn run input.json --verbose
```

Both take the root from the manifest's `@root`, so neither takes a file argument, and running either outside a project is `no package.dawn found from /tmp/x upward; run dawn init`. A build prints its artifact the way the compiler does, `[dawn] target/dawn-out/app`. `run` forwards its trailing arguments to the program's `argv`. Both are thin: they check the lock and the checkouts, then hand the root to the dusk compiler, which is the same pipeline `dusk build` and `dusk run` walk. Once a project is fetched you can skip dawn entirely and call `dusk build` yourself, since the compiler reads the manifest, the lock, and `dawn_modules/` on its own.

Since 1.15.1 `dawn build --release` hands the compiler its `--release` flag, which compiles the IR, the dusk runtime, and every `@csource` at `-O2` instead of the `-O0` a plain build takes. The flag is the compiler's and dawn passes it through unchanged, so a package built this way is the same program built by a `clang` that was told to optimize it, and nothing about resolving, locking, or checking out moves. The plain `dawn build` is what it always was, which is the build you want while you are working. `dawn run` takes no flag of its own, since every word after it belongs to the program, so an optimized run is `dawn build --release` and then the binary it names.

```sh
dawn version    # dawn 1.15.1
```

Dawn's version is the toolchain's version. `dusk version` and `dawn version` print the same number from this release forward, since the two tools are built from one tree and a mismatch would mean nothing good.

## package.dawn

The manifest is a directive file. Every line is blank, a comment, or one `@directive` with its arguments, whitespace separates arguments, a double quote opens a literal run and the next one closes it with both stripped, and there are no escapes, so a value cannot contain a quote. A `//` opens a comment only at an argument boundary, at the start of a line or right after whitespace, which is what lets a URL go unquoted: `@require gh https://github.com/a/b v1` is three arguments and no comment. Any other line is `expected a directive`, with the file and the line number on it.

```text
@package json
@version 1.2.0
@dusk 1.14
@root src/json.dusk
@require maybe github.com/choice404/dusk-maybe v0.3.0
@link "m"
@csource "vendor/fast.c"
```

- `@package` names the package. It is what `dusk doc` prints, what dawn reports, and the header stem `dusk build --lib` writes, so it is letters, digits, `_`, and `-` and nothing else: `@package value '../evil' is not a package name; use letters, digits, _ and -`. It is not what a dependent spells at an import; that is the alias, and the alias belongs to whoever depends on you.
- `@version` is informational. Dawn suggests it as the tag when you cut a release. Nothing parses it as semver, because nothing resolves against it.
- `@dusk` is the oldest compiler the package builds with, major and minor compared numerically: an older compiler refuses the build by name, `package.dawn asks for dusk 1.99, this is 1.14.1`, rather than failing somewhere in the middle of it. The floor is a major and a minor and nothing else, so `@dusk 1.14.0` is `@dusk value '1.14.0' is not a version like 1.14`. A dependency's own floor is checked too since 1.14.1, when its alias is first reached and under its own manifest, `dawn_modules/greet/package.dawn: package.dawn asks for dusk 9.9, this is 1.14.1`, since a package pins the language surface it was written against and reading it as if it were this one's is the worse answer.
- `@root` is the entry file for a program, or the file whose exports the package presents to a dependent, resolved against the manifest's directory. Leave it out and the root is `main.dusk`, then `src/main.dusk`; with none of the three there it is `no root file; add @root, or put the entry at main.dusk or src/main.dusk`, and a path that names nothing is `@root names 'src/gone.dusk', which is not a file`.
- `@require` declares one dependency per line: an alias, a source, and a reference. The alias is an identifier, `'9bad' is not a valid alias; an alias is an identifier like my_pkg`, `std` and `dawn_modules` are reserved, and one alias twice in one manifest is `duplicate alias 'x'`. The source is `host/owner/repo`, where https is assumed, or a full URL beginning `https://`, `git@`, `ssh://`, or `file://`. The reference is a tag, a branch, or a commit.
- `@link` and `@csource` are the package wide form of the two source file directives. They fold into the build's one deduplicated link line in manifest order, after the runtime's own C sources, and a `@csource` path resolves against the manifest that declares it, so a dependency's C source is found wherever its checkout landed. One C source named twice, by a manifest and by a source file, compiles once, deduplicated on the real path.

A manifest you depend on gets less rope than your own. A dependency's `@root` and `@csource` paths must land inside its checkout once normalized, so an absolute path or a `..` climb out of `dawn_modules/<alias>/` is refused, `dependency 'greet' names a root outside its checkout; fix @root in its package.dawn` and the same for a C source, and a dependency's `@link` may name a library and nothing else, `dependency 'greet' names a link path; a dependency may only link a library by name`, since a path there would reach a file on your machine that the lock says nothing about. Your own manifest keeps the full reading of both. A fault in a dependency's `package.dawn` or `dawn.lock` reports at that file, `dawn_modules/greet/package.dawn:3: expected a directive`, and a dependency whose manifest does not read does not resolve, so an import through its alias says so.

Arities are fixed, three arguments for `@require`, four for `@lock`, and one for everything else, with the count named on a miss, `@require takes 3 arguments (alias source ref), got 2`. A repeated single valued directive is `duplicate @package` rather than a quiet override, and an unknown directive is `unknown directive '@nope'`, so a manifest written for a later dawn fails loudly here instead of building with half of what it asked for. The manifest and the lock hold different directive sets and each says so, `@lock belongs in dawn.lock, not package.dawn` and `dawn.lock holds @lock lines only`.

Imports through a dependency spell the alias first and then a dotted path under that package's root directory, exactly like a local or stdlib import.

```text
@import std.io
@import maybe.maybe             // module: maybe.unwrap(x)
@import maybe.maybe.unwrap      // symbol: unwrap(x)
@import maybe.unwrap            // symbol the package's root file exports
```

The last form is the short one: an alias and one name reads that name out of the dependency's `@root` file, so a package that presents its surface from a single file is imported without spelling that file. A name the root does not export is refused where it is written, `import 'greet.no_such' names 'no_such', but 'greet' exports no such symbol`.

## dawn.lock

The lock is generated and committed. It holds one line per requirement, in the manifest's declaration order, carrying the alias, the source and reference as the manifest wrote them, and the 40 hex commit the reference resolved to, checked as it is read, `@lock commit must be a 40 character git hash`.

```text
@lock maybe github.com/choice404/dusk-maybe v0.3.0 3f2ad4c1b90e7a5f6c2d8e14b7a03f95d6e1c2b8
```

The lock is what makes a build repeat. A tag can be moved, a branch moves by definition, and a commit is the only reference that means one thing forever, so the reference is what you write and the commit is what you build. `dawn get` writes the lock and `dawn update` re-resolves it; nothing else moves a line. A requirement with no lock line stops a build before any source is read, in `dawn build` and in `dusk build` alike, `'maybe' is not locked; run dawn get`, since the compiler has no network and so has no other answer to give. A lock that will not parse is `dawn.lock does not read; fix it or delete it, then run dawn get`. The lock mirrors the manifest only when dawn rewrites it, so deleting a `@require` by hand leaves its lock line behind and that line still asks for its checkout, `dependency 'f' is not fetched; run dawn get`, which the same run clears.

Commit `package.dawn` and `dawn.lock`. Do not commit `dawn_modules/`; `dawn init` writes it into `.gitignore`, because the lock is what reproduces it.

## Layout

Every dependency in the graph lands beside the manifest, flat, under the alias that named it.

```text
myproject/
  package.dawn
  dawn.lock
  .gitignore
  src/
    main.dusk
  dawn_modules/
    maybe/
      .dawn                 source, ref, and commit, one line
      package.dawn
      src/maybe.dusk
```

The `.dawn` stamp is how a build tells a checkout that matches the lock from one that does not, without running git to ask: a missing directory is `dependency 'maybe' is not fetched; run dawn get` and a stamp that disagrees with the lock is `dependency 'maybe' does not match dawn.lock; run dawn get`. The stamp is a contract, not a hint, so a checkout that is stamped but gutted underneath is taken at its word by a build and fails on what is missing, `dependency 'util' has no root file; add @root to its package.dawn`.

A fetch asks more than the stamp. Since 1.14.1 a `get` that finds a stamp naming the commit it wants puts two local questions to the tree itself, `rev-parse HEAD` for the commit it holds and `status --porcelain --ignored=matching` for whether anything under it changed, the one line naming the stamp as untracked excepted since the stamp belongs to no commit of the package's own. An ignored file counts as a change, since a file a package's own `.gitignore` matches is still a file the build would compile, and a tamper hiding behind an ignore rule is the one most worth catching. A tree detached at another commit, edited by hand, gutted, or no repository at all fails that pair and is replaced, and the line says which happened.

```text
dawn: fetching maybe from https://github.com/choice404/dusk-maybe
dawn: refreshing util
dawn: cached leaf
```

So a checkout you edited to try something out is not silently compiled into the next build, and `dawn get` is the command that puts it back. Both questions are local, so asking them of every dependency on every `get` costs no network.

Flat means a dependency of a dependency is a sibling, not a nesting, and that has consequences worth stating. A source and reference pair two dependents share is fetched once. The other three cases are errors that name both manifests that asked, so you can see who wants what.

```text
dawn: alias 'p' is required from 'github.com/a/p' by package.dawn and from
      'github.com/b/p' by dawn_modules/util/package.dawn; one alias names one package
dawn: alias 'p' is required at 'v1.0.0' by dawn_modules/a/package.dawn and at 'v2.0.0'
      by dawn_modules/b/package.dawn; add a @require for it in package.dawn to settle
      the version
dawn: 'github.com/a/p' at 'v1.0.0' is required under two aliases, 'p' by package.dawn
      and 'q' by dawn_modules/util/package.dawn; one package has one alias
```

The middle one is the version conflict, and the fix in it is the whole policy: a `@require` in the root manifest wins over what a dependency asked for, so you settle a diamond by pinning it yourself. The alias namespace is one namespace across the whole graph in this release.

## The mirror cache

A source is cloned from the network once per machine, not once per project. Since 1.14.1 the first fetch of a repository leaves a bare mirror of it under the cache root, and every fetch after that, in this project or in any other, clones its working tree out of that mirror. Two projects requiring one package cost one trip to the remote, and a machine that has already seen a repository can fetch it again with no network at all.

The cache root is `$DAWN_CACHE` when that names one and `~/.dawn/cache` otherwise, and each source takes one directory under it named after the source it came from.

```text
~/.dawn/cache/
  github.com/choice404/dusk-maybe-6b1f4c02.git
```

One source takes one path, and the path is derived rather than trusted. What a source is, apart from how it was spelled, is the host and the path under it: the scheme comes off, a user or a token written before the host is dropped rather than filed away, and the `.git` a clone URL may carry is taken off so the one dawn appends is the only one. That identity is what `https://github.com/a/b`, `https://github.com/a/b.git`, and `git@github.com:a/b` all reduce to, so the three share one mirror. The directories under the root are the sanitized components of it, each keeping its letters, digits, and the three marks a repository name carries while every other byte, a `:port` among them, becomes an underscore, and a component that is nothing but dots goes the same way, so no derived path can climb out of the cache root or mean anything to a shell. Eight hex digits of that identity's hash close the last component, which is what keeps two genuinely different sources the sanitizer would fold onto one name in two mirrors. A `file://` source is a local repository and goes through the same reduction.

A `get` puts its questions to the mirror before it reaches the network, and it asks the mirror exactly what it would ask the remote. A reference is looked for as a ref of that name and nothing else, `show-ref --verify` on `refs/tags/<ref>` and then on `refs/heads/<ref>`, and whatever that names is peeled to a commit, which is the answer `ls-remote`'s peeled line gives. The restriction is the point rather than an optimization of it. A mirror is a local repository and git's revision grammar reads far past a ref inside one, so an unqualified name there would also resolve `HEAD`, `main~1`, `v1.0.0^0`, and an abbreviated object id, none of which `ls-remote` will answer, and taking one of those would pin a lock line to whatever this machine happened to hold while the same manifest was refused on a machine with no cache. A mirror may be stricter than the remote, since a miss falls through to asking the remote, and it must never be looser, so what a lock records never depends on the state of a cache. A miss updates the mirror with `fetch --prune` and asks again before the remote gets a question of its own. The checkout is a clone out of the mirror with its `origin` pointed back at the source afterward, so running git inside a fetched package talks to the repository the manifest names rather than to this machine's cache. `dawn update` always fetches first, since moving a pin is the one thing a stale mirror must not decide.

The cache is shared by every project on the machine, so a mirror is locked while it is being written. The lock is a directory beside it, `<mirror>.lock`, since making a directory either succeeds or fails and no filesystem lets two processes make one twice, and it is held across a whole cache call rather than around each git command inside one. A process that cannot make it waits and tries again. A lock nothing has touched in two minutes belonged to a process that died holding it and is taken rather than waited on forever, and a process that waits out those two minutes without taking or making one gives up and fetches from the remote, which is the answer whenever the cache cannot serve. Two terminals, or two jobs of a build system, cannot write one mirror between them.

The cache is an optimization and never a gate. A cache root that cannot be written, a mirror that will not clone or update, and a clone out of one that fails are each reported and then stepped around, the fetch falling back to asking the remote directly, which is what dawn did before there was a cache.

```text
dawn: cannot update the cache for 'maybe' at https://github.com/choice404/dusk-maybe
```

That line is a fault the command survives: it goes to stderr like any other, the fetch carries on, and the exit code is whatever the fetch itself earned. A broken cache costs a slower fetch and never a failed one, and a machine with no `HOME` and no `DAWN_CACHE` has nowhere to keep mirrors and simply fetches every time.

One thing is traded for the offline path. A reference the mirror already resolves is not put to the remote, so a tag moved since this machine last fetched it resolves to what the mirror holds. `dawn update` fetches first and is the only command that moves a pin, so the stale answer never reaches a lock line that was not already being rewritten, and a reference this machine has never seen goes to the network like any other.

## Offline, and where the network is

Dawn shells out to the system `git` and does nothing else over the network. There is no dawn server, no index, no API, and no HTTP client of its own. An SSH source is git's business, so an `ssh://` or `git@` requirement uses whatever keys and agent git already uses, and a `file://` source is a local repository, which is how the test suite exercises the whole fetch path with no network at all.

The order git is driven in is fixed, and what follows is the remote path, the one a fetch walks for what the mirror above could not answer. A reference resolves through `ls-remote`, a peeled tag first so an annotated tag lands on the commit it points at rather than on the tag object, then a plain tag, then a branch; a 40 hex reference is already a commit and skips the round trip. The fetch is `clone --depth 1 --branch` for a named reference and a full clone for a commit, then a detached checkout at the resolved commit, and a clone that fails is removed and retried as a full clone rather than left half fetched. A failure quotes git's own stderr under dawn's message rather than replacing it with a summary.

```text
dawn: bad source 'not a url' for 'maybe'
dawn: cannot resolve 'v9.9.9' for 'maybe' at https://github.com/choice404/dusk-maybe
dawn: cannot check out 'v0.3.0' of 'github.com/choice404/dusk-maybe' for 'maybe'
```

The compiler's side of the line is absolute: `dusk build`, `dusk run`, and `dusk check` read the manifest, the lock, and the checkouts, and never fetch. `dawn tree` reads the same three files, so the graph is printable on the same machine. A build machine with no network builds a project someone else fetched, and a fetch is a thing you do knowingly by running `dawn get`, `dawn add`, or `dawn update`.

One more property falls out of the layout. A dependency's files register under a manifest relative path, `dawn_modules/<alias>/<path>`, so the fault locations compiled into a program do not carry the absolute path of your checkout, and two machines with the same lock emit identical IR.

## The old cache import

Before 1.14.0 a dependency was named at the import itself, `@import "github.com/user/repo/module"`, resolved from a global clone cache with no reference and no lock behind it. 1.14.0 refused the form inside a project and left it resolving outside one for a release, 1.15.0 carried it one release further since that release changed nothing at all, and 1.15.1 removes it. An import whose value carries a `/` is refused wherever it is written, in a project and outside one alike, `'example.com/user/repo/mod' is a url import; declare it with @require in package.dawn`, and the compiler no longer looks in a cache root for an import of any shape.

The cache the old form read is not the mirror cache above, which stays exactly as it is. `DAWN_CACHE` names one root and two different things once used it: dawn's bare mirrors, a fetch time optimization that never decides what an import means, and the old import resolution, which did. Only the second is gone.

Moving is two steps: write a `@require` for the repository with the tag you want, and spell the import through the alias.

## The tradeoffs

A version is a git tag, and dawn does not resolve version ranges. There is no `^1.2` and no solver picking a set that satisfies everybody, so what you get is exactly what a manifest wrote and what a lock recorded, and moving forward is a thing you do by running `dawn update` and reading what changed. The cost is real: a diamond in the graph, where two dependencies pin one package at different tags, is a conflict you resolve by hand with a pin in your own manifest, where a solver would have picked for you. The benefit is that a build is a function of text you can read, and nothing quietly selects a version on your behalf.

There is no registry. A package is a repository, so publishing is pushing a tag and there is nothing to sign up for and nothing to go down, and the same gaps come with it. There is no integrity check past the commit hash the lock pins, no vendor mode that copies dependencies into your tree, no namespace that stops two people from picking one name, and no way to yank a bad release. The commit pin carries most of that weight, since a repointed tag cannot change your build, and the rest is later work.

An export belongs to the package that declares it since 1.14.1. A dependency's exported names are renamed `<name>__<alias>` inside its own files, so two packages that both export `parse` coexist, and what a bare name means is decided by the file that writes it: a file spells its own declarations, every other file of the project, the standard library the project reached, and the exports of each dependency it imported, while `maybe.unwrap(x)` reaches a function anywhere with no import at all. The project wins where two of those answer one name, so adding a `@require` cannot change what a name already in your code means. A bare name a file did not import is `'parse' is exported by package 'maybe'; import it`, or `'leaf_tag' is exported by package 'leaf', which this package does not require; add a @require for it, then import it` when the package is in the build only because something else pulled it in. A name two imported packages both export is `'parse' is exported by both 'a' and 'b'; qualify it`, a qualified prefix two packages both answer to in a file that imported neither is `'util' names two modules; import the one this file means`, and a dependency reaching a name only the project declares is `'app_helper' is declared by the project, not by this package; a package cannot reach it`. Some names keep their spelling, since a rename would break what they stand for: an `export "C"` function and a `foreign` declaration, which name C symbols the linker resolves, a struct field, a method, an enum variant, a monad's `bind` and `unit`, and every builtin. Two packages colliding on one of those is still refused with both files on the line.

## Building dawn

`compiler/dawn.dusk` is a second root over the compiler's own tree, importing the loader, the driver, and the toolchain search directly, so a package build walks the identical front end, sema, mono, and codegen pipeline `dusk` itself does. `dusk` builds it like any other dusk program.

```sh
DUSK_HOME=$PWD target/dusk-out/dusk build compiler/dawn.dusk
```

This produces `target/dusk-out/dawn`. A packaged install places `dawn` beside `dusk` in the same `bin` directory, so it resolves `lib/` and `runtime/` the same way `dusk` does, with no separate `DUSK_HOME` of its own.
