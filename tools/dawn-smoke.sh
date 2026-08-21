#!/usr/bin/env bash
# dawn-smoke.sh: proves dawn (compiler/dawn.dusk, built to target/dusk-out/dawn) on
# the paths a machine with no network can exercise. A package is a directory holding
# package.dawn, so every check here builds one under a throwaway work directory,
# requires its dependencies from local git repositories through file:// URLs, and
# reads back the two files the tool is judged by, dawn.lock and the per module stamp.
# Nothing reaches the network and nothing is left behind.
#
# The compiler that builds dawn is target/dusk-out/dusk, or set DUSK to point at
# another one.
#
# The bare mirror cache the tool keeps is pointed inside the work directory too, so a
# check never reads or writes the machine's own ~/.dawn/cache and every fetch here
# starts from a cold cache.
#
# usage: tools/dawn-smoke.sh
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
cd "$repo_root"
export DUSK_HOME="$repo_root"

builder="${DUSK:-$repo_root/target/dusk-out/dusk}"
dawn_bin="$repo_root/target/dusk-out/dawn"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export DAWN_CACHE="$work/cache"

pass=0
fail=0

ok() {
    echo "  ok: $1"
    pass=$((pass + 1))
}

bad() {
    echo "  FAIL: $1" >&2
    fail=$((fail + 1))
}

# Compares captured output against an expected string, naming the case.
expect_eq() {
    local label=$1
    local expected=$2
    local actual=$3
    if [[ "$expected" == "$actual" ]]; then
        ok "$label"
    else
        bad "$label"
        diff <(printf '%s' "$expected") <(printf '%s' "$actual") >&2 || true
    fi
}

# Checks that text carries a fragment.
expect_has() {
    local label=$1
    local needle=$2
    local haystack=$3
    if [[ "$haystack" == *"$needle"* ]]; then
        ok "$label"
    else
        bad "$label: no '$needle' in '$haystack'"
    fi
}

expect_code() {
    local label=$1
    local want=$2
    local got=$3
    if [[ "$want" -eq "$got" ]]; then
        ok "$label"
    else
        bad "$label: exit $got, want $want"
    fi
}

expect_file() {
    local label=$1
    local path=$2
    if [[ -f "$path" ]]; then
        ok "$label"
    else
        bad "$label: no file at $path"
    fi
}

# Runs dawn inside a project directory, capturing both streams and the exit code
# into the three globals the checks read.
run_dawn() {
    local dir=$1
    shift
    set +e
    out=$(cd "$dir" && "$dawn_bin" "$@" 2>"$work/err.txt")
    code=$?
    set -e
    err=$(cat "$work/err.txt")
}

# --- git fixtures ------------------------------------------------------------
# The committer identity and the default branch are pinned on the command line so a
# fixture never depends on the machine's git configuration.
git_fixture() {
    git -c user.email=dawn@example.invalid -c user.name='dawn smoke' \
        -c commit.gpgsign=false "$@"
}

# Builds a git repository holding a dawn package: a manifest, a root, and one module
# whose line names the version, so a moved pin shows up in a program's own output.
# $1 name, $2 module line, $3 tag, $4 extra manifest lines.
make_repo() {
    local name=$1 line=$2 tag=$3 extra=${4:-}
    local p="$work/$name.git"
    rm -rf "$p"
    mkdir -p "$p/src"
    git -c init.defaultBranch=main init -q "$p"
    {
        echo "@package $name"
        echo "@version 0.1.0"
        echo "@dusk 1.14"
        echo "@root src/main.dusk"
        [[ -n "$extra" ]] && echo "$extra"
    } > "$p/package.dawn"
    cat > "$p/src/main.dusk" <<'EOF'
@paradigm procedural

func main() -> int32 {
    return 0
}
EOF
    cat > "$p/src/greet.dusk" <<EOF
@paradigm procedural

export func greet_line() -> string {
    return "$line"
}
EOF
    git_fixture -C "$p" add -A >/dev/null
    git_fixture -C "$p" commit -q -m "fixture $name"
    git -C "$p" tag "$tag"
}

# Adds a second commit to a fixture repository under a new tag, so a check can prove
# a pin moves.
revise_repo() {
    local name=$1 line=$2 tag=$3
    local p="$work/$name.git"
    cat > "$p/src/greet.dusk" <<EOF
@paradigm procedural

export func greet_line() -> string {
    return "$line"
}
EOF
    git_fixture -C "$p" add -A >/dev/null
    git_fixture -C "$p" commit -q -m "fixture $name revised"
    git -C "$p" tag "$tag"
}

# Writes a consuming project: a manifest carrying the given requires and a root with
# the given source. A project is a directory holding a manifest and nothing more, so
# this is not a git repository.
make_project() {
    local dir=$1 requires=$2 root_src=$3
    rm -rf "$dir"
    mkdir -p "$dir/src"
    {
        echo "@package app"
        echo "@version 0.1.0"
        echo "@dusk 1.14"
        echo "@root src/main.dusk"
        [[ -n "$requires" ]] && printf '%s\n' "$requires"
    } > "$dir/package.dawn"
    printf '%s' "$root_src" > "$dir/src/main.dusk"
}

plain_root='@paradigm procedural

func main() -> int32 {
    println("app ran")
    return 0
}
'

args_root='@paradigm procedural

func main(argc: int32, argv: string[]) -> int32 {
    mut i: int64 = 1
    while i < argv.len {
        println(argv[i])
        i = i + 1
    }
    return 7
}
'

# --- Build dawn through the resource cage ------------------------------------
# The whole compiler tree loads for a second root, so the build peaks high; the
# same limits the bootstrap suites run under bracket it.
if [[ ! -x "$builder" ]]; then
    echo "dawn-smoke: no compiler at $builder" >&2
    echo "dawn-smoke: build one first (see tools/bootstrap.sh) or set DUSK" >&2
    exit 2
fi

echo "building dawn (compiler/dawn.dusk) ..."
timeout 400 bash -c '
    ulimit -v 25165824
    ulimit -t 900
    exec nice -n 19 "$0" build compiler/dawn.dusk
' "$builder" >/dev/null 2>"$work/build.err" || {
    echo "dawn-smoke: dawn failed to build" >&2
    cat "$work/build.err" >&2
    exit 1
}
[[ -x "$dawn_bin" ]] || {
    echo "dawn-smoke: build produced no $dawn_bin" >&2
    exit 1
}

version_line=$("$dawn_bin" version)

# --- Offline surface ---------------------------------------------------------
echo "offline surface:"

expect_has "version names dawn" "dawn " "$version_line"
expect_eq "--version" "$version_line" "$("$dawn_bin" --version)"
expect_eq "-V" "$version_line" "$("$dawn_bin" -V)"

help_text=$("$dawn_bin")
expect_eq "help opens with the version" "$version_line - package tool for the dusk language" "$(printf '%s' "$help_text" | head -1)"
expect_has "help lists init" "dawn init" "$help_text"
expect_has "help lists get" "dawn get" "$help_text"
expect_has "help lists add" "dawn add" "$help_text"
expect_has "help lists update" "dawn update" "$help_text"
expect_has "help lists build" "dawn build" "$help_text"
expect_has "help lists run" "dawn run" "$help_text"
expect_has "help lists tree" "dawn tree" "$help_text"

run_dawn "$work" bogus
expect_code "unknown command exits 1" 1 "$code"
expect_has "unknown command names itself" "unknown command 'bogus'" "$err"

mkdir -p "$work/nowhere"
run_dawn "$work/nowhere" get
expect_code "get with no package.dawn exits 1" 1 "$code"
expect_has "get with no package.dawn names init" "run dawn init" "$err"

# --- init --------------------------------------------------------------------
echo "init:"

mkdir -p "$work/hello"
run_dawn "$work/hello" init hello
expect_code "init exits 0" 0 "$code"
expect_file "init wrote package.dawn" "$work/hello/package.dawn"
expect_file "init wrote main.dusk" "$work/hello/main.dusk"
expect_file "init wrote .gitignore" "$work/hello/.gitignore"
expect_has "the manifest declares the package" "@package hello" "$(cat "$work/hello/package.dawn")"
expect_has "the manifest names a root" "@root main.dusk" "$(cat "$work/hello/package.dawn")"
expect_has "the gitignore keeps fetched packages out" "dawn_modules/" "$(cat "$work/hello/.gitignore")"

run_dawn "$work/hello" run
expect_code "the fresh package runs" 0 "$code"
expect_has "the fresh package names itself" "hello" "$out"

run_dawn "$work/hello" init hello
expect_code "init refuses a second package here" 1 "$code"
expect_has "init says what is in the way" "package.dawn already exists" "$err"

# --- get, lock, stamp --------------------------------------------------------
echo "get:"

make_repo dep "dep v1" v0.1.0
dep_url="file://$work/dep.git"
app="$work/app"
make_project "$app" "@require dep $dep_url v0.1.0" "$plain_root"

run_dawn "$app" get
expect_code "get exits 0" 0 "$code"
expect_file "get wrote a lock" "$app/dawn.lock"
expect_file "get stamped the clone" "$app/dawn_modules/dep/.dawn"

lock_line=$(cat "$app/dawn.lock")
dep_head=$(git -C "$work/dep.git" rev-parse HEAD)
expect_eq "the lock names the require and its commit" "@lock dep $dep_url v0.1.0 $dep_head" "$lock_line"
expect_eq "the stamp agrees with the lock" "$dep_url v0.1.0 $dep_head" "$(cat "$app/dawn_modules/dep/.dawn")"

cp "$app/dawn.lock" "$work/lock.before"
run_dawn "$app" get
expect_code "a second get exits 0" 0 "$code"
expect_has "a second get takes the checkout as it stands" "cached dep" "$out"
if cmp -s "$app/dawn.lock" "$work/lock.before"; then
    ok "a second get rewrites nothing"
else
    bad "a second get changed dawn.lock"
fi

# --- build and run -----------------------------------------------------------
echo "build and run:"

run_dawn "$app" build
expect_code "build exits 0" 0 "$code"
expect_eq "build names the artifact" "[dawn] target/dawn-out/main" "$out"

make_project "$work/argsapp" "" "$args_root"
run_dawn "$work/argsapp" run alpha beta
expect_code "run forwards the program's exit code" 7 "$code"
expect_eq "run forwards the trailing arguments" "[dawn] target/dawn-out/main
alpha
beta" "$out"

alias_root='@paradigm procedural

@import dep.greet

func main() -> int32 {
    println(greet_line())
    return 0
}
'
make_project "$work/aliasapp" "@require dep $dep_url v0.1.0" "$alias_root"
run_dawn "$work/aliasapp" get
expect_code "get for the alias project exits 0" 0 "$code"
run_dawn "$work/aliasapp" run
expect_code "a root importing through an alias runs" 0 "$code"
expect_has "the required module's value reaches the output" "dep v1" "$out"

# --- refusals ----------------------------------------------------------------
echo "refusals:"

make_project "$work/nolock" "@require dep $dep_url v0.1.0" "$plain_root"
run_dawn "$work/nolock" build
expect_code "a require with no lock is refused" 1 "$code"
expect_has "the refusal names the fetch" "run dawn get" "$err"

printf '%s v0.1.0 0000000000000000000000000000000000000000\n' "$dep_url" > "$app/dawn_modules/dep/.dawn"
run_dawn "$app" build
expect_code "a clone that left the lock is refused" 1 "$code"
expect_has "the stale clone refusal names the alias" "'dep'" "$err"
expect_has "the stale clone refusal names the fetch" "dawn get" "$err"
run_dawn "$app" get
expect_code "get repairs the stale clone" 0 "$code"

# --- update ------------------------------------------------------------------
echo "update:"

first_commit=$(awk '{print $5}' "$app/dawn.lock")
revise_repo dep "dep v2" v0.2.0
{
    echo "@package app"
    echo "@version 0.1.0"
    echo "@dusk 1.14"
    echo "@root src/main.dusk"
    echo "@require dep $dep_url v0.2.0"
} > "$app/package.dawn"

run_dawn "$app" build
expect_code "a moved require with the old pin is refused" 1 "$code"
expect_has "the moved require refusal names the fetch" "dawn get" "$err"

run_dawn "$app" update dep
expect_code "update exits 0" 0 "$code"
second_commit=$(awk '{print $5}' "$app/dawn.lock")
if [[ "$first_commit" != "$second_commit" ]]; then
    ok "update moved the pin"
else
    bad "update left the pin on $first_commit"
fi
expect_eq "the lock records the new ref" "@lock dep $dep_url v0.2.0 $second_commit" "$(cat "$app/dawn.lock")"
expect_eq "the stamp follows the new pin" "$dep_url v0.2.0 $second_commit" "$(cat "$app/dawn_modules/dep/.dawn")"

run_dawn "$app" update nosuch
expect_code "update of an alias nothing requires is refused" 1 "$code"
expect_has "that refusal names the alias" "'nosuch'" "$err"

# --- add ---------------------------------------------------------------------
echo "add:"

make_repo extra "extra v1" v0.1.0
extra_url="file://$work/extra.git"
run_dawn "$app" add extra "$extra_url" v0.1.0
expect_code "add exits 0" 0 "$code"
expect_has "add wrote the require" "@require extra $extra_url v0.1.0" "$(cat "$app/package.dawn")"
expect_has "add locked what it wrote" "@lock extra $extra_url v0.1.0" "$(cat "$app/dawn.lock")"
expect_file "add fetched what it wrote" "$app/dawn_modules/extra/.dawn"

run_dawn "$app" add extra "$extra_url" v0.1.0
expect_code "add refuses an alias already required" 1 "$code"
expect_has "that refusal names update" "dawn update extra" "$err"

run_dawn "$app" add std "$extra_url" v0.1.0
expect_code "add refuses a reserved alias" 1 "$code"
expect_has "the reserved alias refusal names it" "'std' is reserved" "$err"

run_dawn "$app" add nope "not a source" v0.1.0
expect_code "add refuses a source that is no source" 1 "$code"
expect_has "the bad source refusal names the alias" "for 'nope'" "$err"

# A refused add must leave the project exactly as it found it. The fetch runs before
# the require is written, so a ref that resolves to nothing edits neither file and the
# next get and build are not left refusing a require nobody could satisfy.
cp "$app/package.dawn" "$work/manifest.untouched"
cp "$app/dawn.lock" "$work/lock.untouched"
run_dawn "$app" add ghost "$extra_url" v9.9.9
expect_code "add with a ref that resolves to nothing is refused" 1 "$code"
expect_has "that refusal names the ref" "cannot resolve 'v9.9.9'" "$err"
if cmp -s "$app/package.dawn" "$work/manifest.untouched"; then
    ok "the refused add left package.dawn byte for byte"
else
    bad "the refused add edited package.dawn"
    diff "$work/manifest.untouched" "$app/package.dawn" >&2 || true
fi
if cmp -s "$app/dawn.lock" "$work/lock.untouched"; then
    ok "the refused add left dawn.lock byte for byte"
else
    bad "the refused add wrote dawn.lock"
fi
run_dawn "$app" build
expect_code "the project still builds after the refused add" 0 "$code"

# --- transitive and collisions ----------------------------------------------
echo "graph:"

make_repo leaf "leaf" v0.1.0
make_repo mid "mid" v0.1.0 "@require leaf file://$work/leaf.git v0.1.0"
make_project "$work/deep" "@require mid file://$work/mid.git v0.1.0" "$plain_root"
run_dawn "$work/deep" get
expect_code "a package's own requires are fetched too" 0 "$code"
expect_eq "the lock holds the direct require and the deep one" "2" "$(wc -l < "$work/deep/dawn.lock")"
expect_file "the deep require landed in the same dawn_modules" "$work/deep/dawn_modules/leaf/.dawn"

make_repo left "left" v0.1.0
make_repo right "right" v0.1.0
make_repo claimer "claimer" v0.1.0 "@require shared file://$work/right.git v0.1.0"
make_project "$work/clash" "@require shared file://$work/left.git v0.1.0
@require claimer file://$work/claimer.git v0.1.0" "$plain_root"
run_dawn "$work/clash" get
expect_code "one alias claimed by two sources is refused" 1 "$code"
expect_has "the clash names the alias" "alias 'shared'" "$err"
if [[ -f "$work/clash/dawn.lock" ]]; then
    bad "the refused fetch left a lock behind"
else
    ok "the refused fetch wrote no lock"
fi

# --- the mirror cache --------------------------------------------------------
# A source is mirrored once per machine, and the mirror is what every fetch after the
# first clones out of. Taking the repository out of reach is what proves it: with
# nothing left for a second project to reach, a fetch that still lands the package came
# from the cache alone, which is the offline path a machine with no network walks.
echo "cache:"

make_repo mir "mir v1" v0.1.0
mir_url="file://$work/mir.git"
make_project "$work/mirone" "@require mir $mir_url v0.1.0" "$plain_root"
run_dawn "$work/mirone" get
expect_code "get for a mirrored source exits 0" 0 "$code"
if [[ -n "$(find "$DAWN_CACHE" -type d -name 'mir-*.git' -print -quit 2>/dev/null)" ]]; then
    ok "the fetch left a bare mirror under the cache"
else
    bad "no mirror named mir-<hash>.git under $DAWN_CACHE"
fi

mv "$work/mir.git" "$work/mir.gone"
make_project "$work/mirtwo" "@require mir $mir_url v0.1.0" "$plain_root"
run_dawn "$work/mirtwo" get
expect_code "a second project fetches with the source out of reach" 0 "$code"
expect_file "the offline fetch stamped the clone" "$work/mirtwo/dawn_modules/mir/.dawn"
expect_eq "the offline fetch locked what the first one did" "$(cat "$work/mirone/dawn.lock")" "$(cat "$work/mirtwo/dawn.lock")"
expect_eq "the checkout names the source and not the mirror" "$mir_url" "$(git -C "$work/mirtwo/dawn_modules/mir" config --get remote.origin.url)"
mv "$work/mir.gone" "$work/mir.git"

# Two sources whose paths differ only in a byte the mirror path folds to an underscore
# are two sources, and each has to get its own repository back. The hash of the source's
# identity is what keeps them apart, since the readable half of the path is the same for
# both.
make_repo keyplus "key plus" v0.1.0
make_repo keyunder "key under" v0.1.0
mv "$work/keyplus.git" "$work/a+b.git"
mv "$work/keyunder.git" "$work/a_b.git"
make_project "$work/keyone" "@require k file://$work/a+b.git v0.1.0" "$plain_root"
make_project "$work/keytwo" "@require k file://$work/a_b.git v0.1.0" "$plain_root"
run_dawn "$work/keyone" get
expect_code "get for the first of two folded sources exits 0" 0 "$code"
run_dawn "$work/keytwo" get
expect_code "get for the second of two folded sources exits 0" 0 "$code"
expect_eq "two sources one path spelling folds together keep two mirrors" "2" "$(find "$DAWN_CACHE" -type d -name 'a*b-*.git' | wc -l)"
expect_eq "the first locked its own commit" "$(git -C "$work/a+b.git" rev-parse HEAD)" "$(awk '{print $5}' "$work/keyone/dawn.lock")"
expect_eq "the second locked its own commit" "$(git -C "$work/a_b.git" rev-parse HEAD)" "$(awk '{print $5}' "$work/keytwo/dawn.lock")"
expect_has "the first checked out its own tree" "key plus" "$(cat "$work/keyone/dawn_modules/k/src/greet.dusk")"
expect_has "the second checked out its own tree" "key under" "$(cat "$work/keytwo/dawn_modules/k/src/greet.dusk")"

# --- tree --------------------------------------------------------------------
# tree reads the manifest, the lock, and the checkouts, and prints what they say. The
# deep project above is the graph it is asked about, so a requirement of a requirement
# prints one step further in.
echo "tree:"

mid_short=$(awk '$2 == "mid" { print substr($5, 1, 12) }' "$work/deep/dawn.lock")
leaf_short=$(awk '$2 == "leaf" { print substr($5, 1, 12) }' "$work/deep/dawn.lock")
run_dawn "$work/deep" tree
expect_code "tree exits 0" 0 "$code"
expect_eq "tree prints the root and the graph under it" "app 0.1.0
  mid file://$work/mid.git v0.1.0 $mid_short
    leaf file://$work/leaf.git v0.1.0 $leaf_short" "$out"

printf '@require ghost file://%s/ghost.git v9.9.9\n' "$work" >> "$work/deep/package.dawn"
run_dawn "$work/deep" tree
expect_code "tree over a requirement nothing has locked still exits 0" 0 "$code"
expect_has "tree marks the requirement nothing has locked" "ghost file://$work/ghost.git v9.9.9 (not locked)" "$out"

# --- refresh -----------------------------------------------------------------
# A checkout is vouched for by what is in it, not by the stamp beside it, so a fetch
# asks the tree itself which commit it holds and whether anything under it changed.
echo "refresh:"

make_repo tamp "tamp v1" v0.1.0
tamp_url="file://$work/tamp.git"
make_project "$work/tampapp" "@require tamp $tamp_url v0.1.0" "$plain_root"
run_dawn "$work/tampapp" get
expect_code "get for the refresh project exits 0" 0 "$code"

module="$work/tampapp/dawn_modules/tamp/src/greet.dusk"
cp "$module" "$work/module.before"
echo "// edited behind the tool's back" > "$module"
run_dawn "$work/tampapp" get
expect_code "get over an edited checkout exits 0" 0 "$code"
expect_has "get says it is refreshing the edited checkout" "refreshing tamp" "$out"
if cmp -s "$module" "$work/module.before"; then
    ok "the refreshed checkout carries the package's own module again"
else
    bad "the refreshed checkout does not carry the package's own module"
fi

# A file the dependency's own .gitignore covers is invisible to a plain status, so a
# checkout with one dropped into it would read as untouched. It is a change to the tree
# like any other and the fetch has to see it.
printf 'junk/\n' > "$work/tamp.git/.gitignore"
git_fixture -C "$work/tamp.git" add -A >/dev/null
git_fixture -C "$work/tamp.git" commit -q -m "fixture tamp ignores junk"
git -C "$work/tamp.git" tag v0.2.0
make_project "$work/ignapp" "@require tamp $tamp_url v0.2.0" "$plain_root"
run_dawn "$work/ignapp" get
expect_code "get for the ignored file project exits 0" 0 "$code"
mkdir -p "$work/ignapp/dawn_modules/tamp/junk"
echo "evil" > "$work/ignapp/dawn_modules/tamp/junk/evil.dusk"
run_dawn "$work/ignapp" get
expect_code "get over a checkout carrying an ignored file exits 0" 0 "$code"
expect_has "an ignored file left in a checkout is a refresh" "refreshing tamp" "$out"
if [[ -e "$work/ignapp/dawn_modules/tamp/junk/evil.dusk" ]]; then
    bad "the ignored file survived the refresh"
else
    ok "the refresh cleared the ignored file"
fi
run_dawn "$work/ignapp" get
expect_has "a checkout with nothing left in it reads as cached" "cached tamp" "$out"

rm -rf "$work/tampapp/dawn_modules/tamp"
run_dawn "$work/tampapp" get
expect_code "get with the checkout deleted exits 0" 0 "$code"
expect_has "get fetches a checkout that is not there" "fetching tamp" "$out"
expect_file "the deleted checkout is stamped again" "$work/tampapp/dawn_modules/tamp/.dawn"

# --- Verdict ----------------------------------------------------------------
echo ""
echo "dawn-smoke: $pass passed, $fail failed"
if [[ "$fail" -ne 0 ]]; then
    exit 1
fi
