# Dawn

Dawn is the package tool for the dusk language. It fetches external code from git repositories the way Go does, so there is no central registry, and a package is just a repository you can read. This document covers how dawn works and what it does today.

## The model

A dusk program pulls in the standard library and local modules with a dotted path.

```text
@import std.io
@import std.functional.maybe
```

An external package uses its git path in quotes.

```text
@import "github.com/user/repo/module"
```

The first three segments, `host/user/repo`, are the repository. The rest, `module`, is a file inside it. The import string carries no version yet. dawn fetches the repository, and the dusk compiler resolves the module from the local cache.

This approach is inspired by Go. There is no registry to run or trust, the source is the package, and the import path tells you where the code lives. The same gaps come with it, and later work will close them. Those gaps are version pinning, a lock file for repeatable builds, and a way to vendor code you do not want to refetch.

## Commands

```sh
dawn get <file.dusk>     # clone the git packages a file imports into the cache
dawn build <file.dusk>   # fetch packages, then compile
dawn run <file.dusk>     # fetch packages, compile, and run
dawn version
```

`build` and `run` are wrappers that fetch every imported package, then hand the program to the dusk compiler. You can also fetch with `dawn get` and then call `dusk` directly.

## The cache

Dawn clones each repository into a content cache, and the dusk loader resolves git imports from there. The cache root is `$DAWN_CACHE`, or `~/.dawn/cache` when that is unset. The layout mirrors the import path.

```
~/.dawn/cache/
  github.com/user/repo/        a shallow clone of the repository
    module.dusk                a module inside it, resolved by an import
```

An import `github.com/user/repo/module` resolves to `~/.dawn/cache/github.com/user/repo/module.dusk`, or falls back to `repo.dusk` as a leaf, the same way the stdlib resolves a dotted path. The `$DAWN_CACHE` override points a build at a clean cache, which is also how the tests check resolution without touching a real home directory.

## Current features

- The compiler parses a quoted git import, so `@import "github.com/user/repo/module"` is valid syntax.
- The loader resolves a slash bearing import against the cache and merges the modules it finds, the same as a stdlib import.
- The `dawn` binary has `get`, `build`, and `run`. `get` shallow clones each imported repository into the cache with the system `git` and skips repositories already present.

This path runs end to end. It is minimal on purpose. There is no version selection, no lock file, no fetch past the root file's direct imports, and no integrity check. It is the working seed the rest grows from.

## Roadmap

1. Version selection. Pin a reference per package, a git tag or commit, chosen by a minimal version selection rule like Go and recorded so builds repeat.
2. A lock file. A manifest at the project root that lists every package and its resolved version, checked in, so a fresh machine builds the same bytes.
3. Graph fetch. Walk the imports of fetched packages, not the root file alone, and resolve the whole graph before a build.
4. Integrity. A hash per fetched module, verified on use, to catch a moved or rewritten tag.
5. Quality of life. A vendor mode that copies dependencies into the tree, offline builds from the cache, and private repositories with authentication.
