#!/usr/bin/env bash
# bootstrap.sh: stand up a dusk compiler on a machine that has none. Feed it a
# release artifact, either the dusk binary itself or the compiler's own LLVM IR
# (dusk.ll, optionally xz compressed), and it produces target/dusk-out/dusk
# built from this tree's source. The .ll path trusts only text plus clang: the
# IR is linked against the C runtime into a seed, and the seed rebuilds the
# compiler from source.
#
# usage: tools/bootstrap.sh <dusk-binary | dusk.ll | dusk.ll.xz>
#
# The artifacts are attached to each release tag; the sha256sums file beside
# them carries the hash the release's stage ladder printed, so the .ll you
# link is the .ll the release proved. The IR pins x86_64 linux; on another
# platform take the audit path printed at the end instead.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: tools/bootstrap.sh <dusk-binary | dusk.ll | dusk.ll.xz>" >&2
    exit 2
fi

original_pwd=$(pwd)
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)

case "$1" in
    /*) artifact=$1 ;;
    *) artifact=$original_pwd/$1 ;;
esac
[[ -e "$artifact" ]] || { echo "bootstrap: no such artifact: $artifact" >&2; exit 2; }

cd "$repo_root"

# The self build overwrites target/dusk-out, so a seed living there would be
# replaced mid boot by its own output. Refuse it.
case "$(realpath "$artifact")" in
    "$repo_root"/target/dusk-out/*)
        echo "bootstrap: the artifact must live outside target/dusk-out; the self build overwrites that directory" >&2
        exit 2
        ;;
esac

work="$repo_root/target/bootstrap-seed"
mkdir -p "$work"

seed=""
case "$artifact" in
    *.ll.xz)
        echo "bootstrap: decompressing the compiler IR"
        xz -dkc "$artifact" >"$work/seed.ll"
        echo "bootstrap: linking the compiler IR against the C runtime"
        clang "$work/seed.ll" runtime/*.c -pthread -lm -o "$work/seed"
        seed="$work/seed"
        ;;
    *.ll)
        echo "bootstrap: linking the compiler IR against the C runtime"
        clang "$artifact" runtime/*.c -pthread -lm -o "$work/seed"
        seed="$work/seed"
        ;;
    *)
        [[ -x "$artifact" ]] || { echo "bootstrap: artifact is neither .ll nor executable: $artifact" >&2; exit 2; }
        seed="$artifact"
        ;;
esac

echo "bootstrap: seed version: $("$seed" version)"

# The self build runs caged: 24GB address space, a CPU ceiling, lowest
# priority, an outer wall clock timeout. The source path is passed absolute
# because fault location strings embed the path as spelled, and the stage
# ladder builds absolute; a relative spelling here would emit different IR.
echo "bootstrap: seed builds the compiler source"
DUSK_HOME="$repo_root" timeout 600 bash -c '
    ulimit -v 25165824
    ulimit -t 900
    exec nice -n 19 "$0" build "$1"
' "$seed" "$repo_root/compiler/dusk.dusk" || { echo "bootstrap: FAIL: the seed cannot build compiler/dusk.dusk" >&2; exit 1; }

echo "bootstrap: done. the compiler is at target/dusk-out/dusk"
echo
echo "verify it:"
echo "  target/dusk-out/dusk version"
echo "  DUSK_HOME=$repo_root $seed build tests/runner/testrun.dusk"
echo "  DUSK_HOME=$repo_root DUSK_BIN=target/dusk-out/dusk target/dusk-out/testrun tests/goldens.manifest"
echo
echo "audit path (no artifact trusted at all): clone github.com/choice404/dusk-rust,"
echo "cargo build --release the Rust compiler it froze at v1.3.0, build"
echo "compiler/main.dusk there with it, then hand that binary to tools/ratchet.sh"
echo "here. Every archive tag carries the full Rust tree, so any release"
echo "re-derives from its own tag's Rust seed."
