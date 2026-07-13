#!/usr/bin/env bash
# dawn-smoke.sh: proves the dusk1 dawn (compiler/dawn.dusk, built to
# target/dusk-out/dawn) reproduces the stage0 dawn (src/bin/dawn.rs) byte for
# byte on the paths a machine without a network can exercise: version, help, the
# cached branch of get, and build/run with its exit-code passthrough. When cargo
# is on PATH the Rust dawn is built and its version, help, and cached-get output
# are diffed against dusk1 dawn line for line; without cargo that comparison is
# skipped rather than failed, so the script is useful offline.
#
# usage: tools/dawn-smoke.sh
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
cd "$repo_root"
export DUSK_HOME="$repo_root"

stage0="$repo_root/target/release/dusk"
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

# --- Build dusk1 dawn through the resource cage -----------------------------
# The whole compiler tree loads for a second root, so the build peaks high; the
# same limits the bootstrap suites run under bracket it.
if [[ ! -x "$stage0" ]]; then
    echo "dawn-smoke: stage0 binary not found at $stage0" >&2
    echo "dawn-smoke: run 'cargo build --release --bin dusk' first" >&2
    exit 2
fi

echo "building dusk1 dawn (compiler/dawn.dusk) ..."
timeout 400 bash -c '
    ulimit -v 25165824
    ulimit -t 900
    exec nice -n 19 "$0" build compiler/dawn.dusk
' "$stage0" >/dev/null 2>"$work/build.err" || {
    echo "dawn-smoke: dusk1 dawn failed to build" >&2
    cat "$work/build.err" >&2
    exit 1
}
[[ -x "$dawn_dusk" ]] || {
    echo "dawn-smoke: build produced no $dawn_dusk" >&2
    exit 1
}

# --- Offline surface --------------------------------------------------------
echo "offline surface:"

expect_eq "version" "dawn 1.3.0
" "$("$dawn_dusk" version)
"

expect_eq "--version" "dawn 1.3.0
" "$("$dawn_dusk" --version)
"

help_text="dawn 1.3.0 - package tool for the dusk language

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

# --- Rust dawn differential (guarded) ---------------------------------------
# When cargo is present the stage0 dawn is built and its output is diffed against
# dusk1 dawn's, the byte-for-byte oracle. Without cargo this section is skipped.
echo "rust dawn differential:"
if command -v cargo >/dev/null 2>&1; then
    if cargo build --release --bin dawn >"$work/cargo.log" 2>&1; then
        rust_dawn="$repo_root/target/release/dawn"
        differ=0
        diff <("$rust_dawn" version) <("$dawn_dusk" version) || differ=1
        diff <("$rust_dawn") <("$dawn_dusk") || differ=1
        diff <("$rust_dawn" bogus) <("$dawn_dusk" bogus) || differ=1
        diff <(DAWN_CACHE="$cache" "$rust_dawn" get "$work/app.dusk") \
             <(DAWN_CACHE="$cache" "$dawn_dusk" get "$work/app.dusk") || differ=1
        diff <("$rust_dawn" get 2>&1 1>/dev/null) \
             <("$dawn_dusk" get 2>&1 1>/dev/null) || differ=1
        if [[ "$differ" -eq 0 ]]; then
            ok "dusk1 dawn matches stage0 dawn (version, help, unknown, cached get, usage)"
        else
            bad "dusk1 dawn output diverges from stage0 dawn (see diff above)"
        fi
    else
        echo "  SKIP: cargo present but 'cargo build --bin dawn' failed" >&2
        cat "$work/cargo.log" >&2
    fi
else
    echo "  SKIP: cargo not on PATH, differential oracle unavailable"
fi

# --- Verdict ----------------------------------------------------------------
echo ""
echo "dawn-smoke: $pass passed, $fail failed"
if [[ "$fail" -ne 0 ]]; then
    exit 1
fi
