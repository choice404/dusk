#!/usr/bin/env bash
# ratchet.sh: prove the ratchet rule at a release gate. The previous release's
# dusk binary must build the current compiler source, and the binary it
# produces must pass the golden suite. Run this before cutting any release;
# success means a machine holding only the previous release can reach this one
# from source.
#
# usage: tools/ratchet.sh <previous-release-dusk-binary>
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: tools/ratchet.sh <previous-release-dusk-binary>" >&2
    exit 2
fi

original_pwd=$(pwd)
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)

case "$1" in
    /*) prev=$1 ;;
    *) prev=$original_pwd/$1 ;;
esac
[[ -x "$prev" ]] || { echo "ratchet: previous binary is not executable: $prev" >&2; exit 2; }

cd "$repo_root"

# The self build overwrites target/dusk-out, so a previous binary living
# there would be replaced by the very build it is supposed to prove, and the
# gate would prove nothing. Refuse it.
case "$(realpath "$prev")" in
    "$repo_root"/target/dusk-out/*)
        echo "ratchet: the previous release binary must live outside target/dusk-out; the self build overwrites that directory" >&2
        exit 2
        ;;
esac

# Every self build runs caged: 24GB address space, a CPU ceiling, lowest
# priority, and an outer wall clock timeout, so a runaway build cannot take
# the machine down.
caged_build() {
    local builder=$1
    local source=$2
    timeout 600 bash -c '
        ulimit -v 25165824
        ulimit -t 900
        exec nice -n 19 "$0" build "$1"
    ' "$builder" "$source"
}

# The source path is passed absolute: fault location strings embed the path
# as spelled, and the stage ladder builds absolute, so a relative spelling
# would emit IR that differs from the release's recorded hash.
echo "ratchet: previous release builds the compiler source"
DUSK_HOME="$repo_root" caged_build "$prev" "$repo_root/compiler/dusk.dusk" \
    || { echo "ratchet: FAIL: the previous release cannot build compiler/dusk.dusk" >&2; exit 1; }
new_dusk="$repo_root/target/dusk-out/dusk"
[[ -x "$new_dusk" ]] || { echo "ratchet: FAIL: build produced no $new_dusk" >&2; exit 1; }

echo "ratchet: previous release builds the test runner"
DUSK_HOME="$repo_root" caged_build "$prev" tests/runner/testrun.dusk \
    || { echo "ratchet: FAIL: the previous release cannot build the test runner" >&2; exit 1; }

echo "ratchet: golden suite against the freshly built compiler"
DUSK_HOME="$repo_root" DUSK_BIN="$new_dusk" timeout 3000 bash -c '
    ulimit -v 12582912
    ulimit -t 3600
    exec nice -n 19 target/dusk-out/testrun tests/goldens.manifest
' || { echo "ratchet: FAIL: golden suite failed under the new compiler" >&2; exit 1; }

echo "ratchet: PASS"
