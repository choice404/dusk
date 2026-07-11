#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "usage: tools/pyramid.sh <stage0-binary> <compiler-root>" >&2
}

fail() {
    echo "pyramid: $*" >&2
    exit 1
}

if [[ $# -ne 2 ]]; then
    usage
    exit 2
fi

original_pwd=$(pwd)
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)

abs_path() {
    case "$1" in
        /*) echo "$1" ;;
        *) echo "$original_pwd/$1" ;;
    esac
}

stage0=$(abs_path "$1")
compiler_root=$(abs_path "$2")

if [[ ! -x "$stage0" ]]; then
    fail "stage0-binary is not executable: $stage0"
fi

if [[ ! -f "$compiler_root" ]]; then
    fail "stage ladder cannot run yet because the compiler root does not exist: $compiler_root"
fi

cd "$repo_root"

work="$repo_root/target/bootstrap-pyramid"
mkdir -p "$work/bin" "$work/ll" "$work/logs" "$work/examples/stage1" "$work/examples/stage2"

stage1="$work/bin/stage1"
stage2="$work/bin/stage2"
stage3="$work/bin/stage3"
stage1_ll="$work/ll/stage1-from-stage0.ll"
stage2_ll="$work/ll/stage2-from-stage1.ll"
stage3_ll="$work/ll/stage3-from-stage2.ll"

stem_of() {
    local path=$1
    local base
    base=$(basename "$path")
    echo "${base%.dusk}"
}

first_diff_line() {
    local left=$1
    local right=$2
    local left_label=$3
    local right_label=$4
    local line

    set +e
    line=$(
        diff -u --label "$left_label" --label "$right_label" "$left" "$right" |
            awk '
                substr($0, 1, 3) == "---" { next }
                substr($0, 1, 3) == "+++" { next }
                substr($0, 1, 2) == "@@" { next }
                substr($0, 1, 1) == "-" || substr($0, 1, 1) == "+" { print; exit }
            '
    )
    set -e

    if [[ -n "$line" ]]; then
        echo "$line"
    else
        echo "files differ"
    fi
}

build_compiler_stage() {
    local builder=$1
    local output_bin=$2
    local output_ll=$3
    local label=$4
    local stem
    local emitted_bin
    local emitted_ll

    stem=$(stem_of "$compiler_root")
    emitted_bin="$repo_root/target/dusk-out/$stem"
    emitted_ll="$repo_root/target/dusk-out/$stem.ll"

    echo "$label"
    if ! DUSK_HOME="$repo_root" "$builder" build "$compiler_root"; then
        fail "$label failed while building $compiler_root"
    fi

    [[ -x "$emitted_bin" ]] || fail "$label did not produce executable $emitted_bin"
    [[ -f "$emitted_ll" ]] || fail "$label did not produce LLVM IR $emitted_ll"

    cp "$emitted_bin" "$output_bin"
    cp "$emitted_ll" "$output_ll"
    chmod +x "$output_bin"
}

print_stage_sha256() {
    local label=$1
    local binary=$2
    local ll=$3

    echo "$label binary sha256: $(sha256sum "$binary" | cut -d' ' -f1)"
    echo "$label LLVM IR sha256: $(sha256sum "$ll" | cut -d' ' -f1)"
}

stage_supports_build() {
    local builder=$1
    local stdout_path="$work/logs/stage1-build-probe.stdout"
    local stderr_path="$work/logs/stage1-build-probe.stderr"
    local combined_path="$work/logs/stage1-build-probe.combined"
    local status

    set +e
    DUSK_HOME="$repo_root" "$builder" build /dev/null >"$stdout_path" 2>"$stderr_path"
    status=$?
    set -e

    cat "$stdout_path" "$stderr_path" >"$combined_path"

    if [[ "$status" -ne 0 ]] && grep -Fq "dusk1: unknown command" "$combined_path"; then
        return 1
    fi

    return 0
}

build_example_ll() {
    local builder=$1
    local file=$2
    local output_ll=$3
    local label=$4
    local safe=$5
    local stem
    local emitted_ll
    local status

    stem=$(stem_of "$file")
    emitted_ll="$repo_root/target/dusk-out/$stem.ll"

    set +e
    DUSK_HOME="$repo_root" "$builder" build "$file" \
        >"$work/logs/$label-$safe.stdout" \
        2>"$work/logs/$label-$safe.stderr"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        [[ -f "$emitted_ll" ]] || fail "$label succeeded for $file but did not emit $emitted_ll"
        cp "$emitted_ll" "$output_ll"
    fi

    return "$status"
}

compare_example_ir() {
    local file=$1
    local rel
    local safe
    local ll1
    local ll2
    local status1
    local status2

    rel=${file#"$repo_root/"}
    safe=${rel//\//__}
    safe=${safe%.dusk}
    ll1="$work/examples/stage1/$safe.ll"
    ll2="$work/examples/stage2/$safe.ll"

    if build_example_ll "$stage1" "$file" "$ll1" "stage1" "$safe"; then
        status1=0
    else
        status1=$?
    fi

    if build_example_ll "$stage2" "$file" "$ll2" "stage2" "$safe"; then
        status2=0
    else
        status2=$?
    fi

    if [[ "$status1" -ne "$status2" ]]; then
        fail "stage1 and stage2 disagree on build status for $rel: $status1 vs $status2"
    fi

    if [[ "$status1" -ne 0 ]]; then
        rejected_examples=$((rejected_examples + 1))
        return
    fi

    if ! cmp -s "$ll1" "$ll2"; then
        echo "pyramid: LLVM IR mismatch for $rel" >&2
        echo "first diff: $(first_diff_line "$ll1" "$ll2" "stage1 $rel" "stage2 $rel")" >&2
        exit 1
    fi

    compared_examples=$((compared_examples + 1))
}

# Stage 1 proves the dusk compiler source can be translated by the trusted
# stage0 compiler. The resulting stage1 is trustworthy only through stage0.
build_compiler_stage "$stage0" "$stage1" "$stage1_ll" "stage 1: stage0 builds the dusk compiler source"
print_stage_sha256 "stage1" "$stage1" "$stage1_ll"

if ! stage_supports_build "$stage1"; then
    echo "stage 1 check: SKIP (dusk1 has no build command yet)"
    echo "stage 2: SKIP (dusk1 has no build command yet)"
    echo "stage 3: SKIP (dusk1 has no build command yet)"
    echo "stage 2 check: SKIP (dusk1 has no build command yet)"
    exit 0
fi

# The existing golden suite must pass when the compiler under test is stage1.
# tests/examples.rs honors DUSK_BIN, so no test harness edit is needed here.
echo "stage 1 check: run golden suite against stage1"
if ! DUSK_HOME="$repo_root" DUSK_BIN="$stage1" cargo test --test examples; then
    fail "golden suite failed against stage1"
fi

# TODO(bootstrap): dusk1 needs a working build command with codegen ported to compiler/*.dusk.
# Stage 2 is built by the now validated stage1. Matching behavior here shows
# that the dusk compiler source is self consistent under its own front end.
build_compiler_stage "$stage1" "$stage2" "$stage2_ll" "stage 2: stage1 builds the dusk compiler source"
print_stage_sha256 "stage2" "$stage2" "$stage2_ll"

# TODO(bootstrap): dusk1 needs a working build command with codegen ported to compiler/*.dusk.
# Stage1 and stage2 should compile every golden example to the same LLVM IR.
# Examples that both compilers reject are counted separately; a status mismatch
# is still a bootstrap failure.
echo "stage 2 check: compare stage1 and stage2 emitted LLVM IR for examples"
compared_examples=0
rejected_examples=0
while IFS= read -r -d '' example; do
    compare_example_ir "$example"
done < <(find "$repo_root/examples" -type f -name '*.dusk' -print0 | sort -z)
echo "stage 2 check: matched $compared_examples compiled examples, $rejected_examples rejected by both"

# TODO(bootstrap): dusk1 needs a working build command with codegen ported to compiler/*.dusk.
# Stage 3 is built by stage2. Once the bootstrap has converged, the compiler IR
# that produced stage2 and the compiler IR that produced stage3 are identical.
build_compiler_stage "$stage2" "$stage3" "$stage3_ll" "stage 3: stage2 builds the dusk compiler source"
print_stage_sha256 "stage3" "$stage3" "$stage3_ll"

# The fixpoint check compares two consecutive self builds of the same source.
if ! cmp -s "$stage2_ll" "$stage3_ll"; then
    echo "pyramid: stage2 and stage3 compiler LLVM IR differ" >&2
    echo "first diff: $(first_diff_line "$stage2_ll" "$stage3_ll" "stage2 compiler IR" "stage3 compiler IR")" >&2
    exit 1
fi

echo "pyramid complete: stage1, stage2, and stage3 matched the bootstrap checks"
