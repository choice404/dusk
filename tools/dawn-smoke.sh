#!/usr/bin/env bash
# dawn-smoke.sh: proves dawn (compiler/dawn.dusk, built to target/dusk-out/dawn)
# on the paths a machine without a network can exercise: version, help, the
# cached branch of get, and build/run with its exit-code passthrough. The
# compiler that builds dawn is target/dusk-out/dusk, or set DUSK to point at
# another one.
#
# usage: tools/dawn-smoke.sh
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
cd "$repo_root"
export DUSK_HOME="$repo_root"

builder="${DUSK:-$repo_root/target/dusk-out/dusk}"
dawn_dusk="$repo_root/target/dusk-out/dawn"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

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
[[ -x "$dawn_dusk" ]] || {
    echo "dawn-smoke: build produced no $dawn_dusk" >&2
    exit 1
}

# --- Offline surface --------------------------------------------------------
echo "offline surface:"

expect_eq "version" "dawn 1.5.2
" "$("$dawn_dusk" version)
"

expect_eq "--version" "dawn 1.5.2
" "$("$dawn_dusk" --version)
"

help_text="dawn 1.5.2 - package tool for the dusk language

usage:
  dawn get <file.dusk>     clone the git packages a file imports
  dawn build <file.dusk>   fetch packages, then compile
  dawn run <file.dusk>     fetch packages, compile, and run
  dawn version             print version

imports are git paths, e.g. @import \"github.com/user/repo/module\"
cache: \$DAWN_CACHE or ~/.dawn/cache"

expect_eq "help (no command)" "$help_text" "$("$dawn_dusk")"
expect_eq "help (unknown command)" "$help_text" "$("$dawn_dusk" bogus)"

# get with no file argument is a usage error on stderr, exit 1.
set +e
usage_out=$("$dawn_dusk" get 2>&1 1>/dev/null)
usage_code=$?
set -e
expect_eq "get missing arg (stderr)" "usage: dawn get <file.dusk>" "$usage_out"
if [[ "$usage_code" -eq 1 ]]; then ok "get missing arg exit 1"; else bad "get missing arg exit $usage_code"; fi

# --- Offline cached get -----------------------------------------------------
# A hermetic cache: a directory standing in for a cloned repository at
# host/user/repo, plus a root file importing a module inside it. get prescans the
# import, finds the repo directory present, and prints the cached line with no
# network touched.
echo "offline cached get:"
cache="$work/cache"
mkdir -p "$cache/example.com/acme/widget"
cat > "$cache/example.com/acme/widget/tiny.dusk" <<'EOF'
@paradigm procedural

export func widget_value() -> int32 {
    return 7
}
EOF
cat > "$work/app.dusk" <<'EOF'
@paradigm procedural

@import "example.com/acme/widget/tiny"

func main() -> int32 {
    return widget_value()
}
EOF
get_out=$(DAWN_CACHE="$cache" "$dawn_dusk" get "$work/app.dusk")
expect_eq "get cached" "dawn: cached example.com/acme/widget
dawn: 1 package(s) ready" "$get_out"

# --- build / run passthrough ------------------------------------------------
# A slash-free program (no git packages) that exits non-zero. build prints the
# [dawn] line; run forwards the child's exit code untouched.
echo "build / run passthrough:"
cat > "$work/ret7.dusk" <<'EOF'
@paradigm procedural

@import std.io

func main() -> int32 {
    println("child ran")
    return 7
}
EOF
build_out=$("$dawn_dusk" build "$work/ret7.dusk")
expect_eq "build [dawn] line" "[dawn] target/dawn-out/ret7" "$build_out"

set +e
run_out=$("$dawn_dusk" run "$work/ret7.dusk")
run_code=$?
set -e
expect_eq "run stdout order" "[dawn] target/dawn-out/ret7
child ran" "$run_out"
if [[ "$run_code" -eq 7 ]]; then ok "run forwards exit 7"; else bad "run exit $run_code (want 7)"; fi

# --- Verdict ----------------------------------------------------------------
echo ""
echo "dawn-smoke: $pass passed, $fail failed"
if [[ "$fail" -ne 0 ]]; then
    exit 1
fi
