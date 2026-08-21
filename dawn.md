# Dawn

Dawn is the package tool for the dusk language. A project is a directory holding a `package.dawn` manifest, a package is a git repository with one at its root, and a version is a git tag, so there is no registry to run or trust and the source you build against is the repository you can read. Dawn owns the network: it resolves each dependency's reference to a commit, writes the commit into `dawn.lock`, and checks the code out under `dawn_modules/`. The dusk compiler owns the build and never opens a socket, so once a project is fetched every build after it is offline.

## Usage

Dawn has seven commands. Each one runs from inside the project, the directory holding `package.dawn` or any directory under it.

```sh
dawn init [name]              # write package.dawn, a hello root, and a .gitignore line
dawn get                      # resolve every requirement, clone it, write dawn.lock
dawn add <alias> <src> <ref>  # add a requirement to the manifest, then fetch it
dawn update [alias]           # re-resolve one requirement, or all of them
dawn build                    # build the manifest's root with dusk
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

Build and run.

```sh
dawn build
dawn run input.json --verbose
```

Both take the root from the manifest's `@root`, so neither takes a file argument, and running either outside a project is `no package.dawn found from /tmp/x upward; run dawn init`. A build prints its artifact the way the compiler does, `[dawn] target/dawn-out/app`. `run` forwards its trailing arguments to the program's `argv`. Both are thin: they check the lock and the checkouts, then hand the root to the dusk compiler, which is the same pipeline `dusk build` and `dusk run` walk. Once a project is fetched you can skip dawn entirely and call `dusk build` yourself, since the compiler reads the manifest, the lock, and `dawn_modules/` on its own.

```sh
dawn version    # dawn 1.14.0
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
- `@dusk` is the oldest compiler the package builds with, major and minor compared numerically: an older compiler refuses the build by name, `package.dawn asks for dusk 1.99, this is 1.14.0`, rather than failing somewhere in the middle of it. The floor is a major and a minor and nothing else, so `@dusk 1.14.0` is `@dusk value '1.14.0' is not a version like 1.14`. Only the root project's floor is checked; a dependency's own is read and not enforced.
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

The `.dawn` stamp is how a build tells a checkout that matches the lock from one that does not, without running git to ask: a missing directory is `dependency 'maybe' is not fetched; run dawn get` and a stamp that disagrees with the lock is `dependency 'maybe' does not match dawn.lock; run dawn get`. The stamp is a contract, not a hint, so a checkout that is stamped but gutted underneath is taken at its word and fails on what is missing, `dependency 'util' has no root file; add @root to its package.dawn`.

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

## Offline, and where the network is

Dawn shells out to the system `git` and does nothing else over the network. There is no dawn server, no index, no API, and no HTTP client of its own. An SSH source is git's business, so an `ssh://` or `git@` requirement uses whatever keys and agent git already uses, and a `file://` source is a local repository, which is how the test suite exercises the whole fetch path with no network at all.

The order git is driven in is fixed. A reference resolves through `ls-remote`, a peeled tag first so an annotated tag lands on the commit it points at rather than on the tag object, then a plain tag, then a branch; a 40 hex reference is already a commit and skips the round trip. The fetch is `clone --depth 1 --branch` for a named reference and a full clone for a commit, then a detached checkout at the resolved commit, and a clone that fails is removed and retried as a full clone rather than left half fetched. A failure quotes git's own stderr under dawn's message rather than replacing it with a summary.

```text
dawn: bad source 'not a url' for 'maybe'
dawn: cannot resolve 'v9.9.9' for 'maybe' at https://github.com/choice404/dusk-maybe
dawn: cannot check out 'v0.3.0' of 'github.com/choice404/dusk-maybe' for 'maybe'
```

The compiler's side of the line is absolute: `dusk build`, `dusk run`, and `dusk check` read the manifest, the lock, and the checkouts, and never fetch. So a build machine with no network builds a project someone else fetched, and a fetch is a thing you do knowingly by running `dawn get`, `dawn add`, or `dawn update`.

One more property falls out of the layout. A dependency's files register under a manifest relative path, `dawn_modules/<alias>/<path>`, so the fault locations compiled into a program do not carry the absolute path of your checkout, and two machines with the same lock emit identical IR.

## The old cache import

Before 1.14.0 a dependency was named at the import itself, `@import "github.com/user/repo/module"`, resolved from a global clone cache with no reference and no lock. The compiler still resolves that form outside a project this release, unchanged, but nothing fills the cache any more: dawn fetches into `dawn_modules` now, so the form reaches only a cache placed by hand or left behind by an earlier release. Inside a project holding a `package.dawn` it is an error, `'example.com/user/repo/mod' is a url import; declare it with @require in package.dawn`, since the manifest is where a dependency and its reference are written down. It is removed in 1.15.0. Moving is two steps: write a `@require` for the repository with the tag you want, and spell the import through the alias.

## The tradeoffs

A version is a git tag, and dawn does not resolve version ranges. There is no `^1.2` and no solver picking a set that satisfies everybody, so what you get is exactly what a manifest wrote and what a lock recorded, and moving forward is a thing you do by running `dawn update` and reading what changed. The cost is real: a diamond in the graph, where two dependencies pin one package at different tags, is a conflict you resolve by hand with a pin in your own manifest, where a solver would have picked for you. The benefit is that a build is a function of text you can read, and nothing quietly selects a version on your behalf.

There is no registry. A package is a repository, so publishing is pushing a tag and there is nothing to sign up for and nothing to go down, and the same gaps come with it. There is no integrity check past the commit hash the lock pins, no vendor mode that copies dependencies into your tree, no namespace that stops two people from picking one name, and no way to yank a bad release. The commit pin carries most of that weight, since a repointed tag cannot change your build, and the rest is later work.

Exports are global in this release. Two packages that export the same name collide, and the loader refuses the program and names both files rather than picking one. Import scoped renaming, which lets two packages each keep a name, comes in a later release, and no program that builds today changes meaning when it lands.

## Building dawn

`compiler/dawn.dusk` is a second root over the compiler's own tree, importing the loader, the driver, and the toolchain search directly, so a package build walks the identical front end, sema, mono, and codegen pipeline `dusk` itself does. `dusk` builds it like any other dusk program.

```sh
DUSK_HOME=$PWD target/dusk-out/dusk build compiler/dawn.dusk
```

This produces `target/dusk-out/dawn`. A packaged install places `dawn` beside `dusk` in the same `bin` directory, so it resolves `lib/` and `runtime/` the same way `dusk` does, with no separate `DUSK_HOME` of its own.
