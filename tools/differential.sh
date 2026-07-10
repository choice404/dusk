#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "usage: tools/differential.sh <binary-a> <binary-b> <lex|scan|parse|load|desugar|check|mono|esc>" >&2
}

if [[ $# -ne 3 ]]; then
    usage
    exit 2
fi

binary_a=$1
binary_b=$2
cmd=$3

if [[ ! -x "$binary_a" ]]; then
    echo "differential: binary-a is not executable: $binary_a" >&2
    exit 2
fi

if [[ ! -x "$binary_b" ]]; then
    echo "differential: binary-b is not executable: $binary_b" >&2
    exit 2
fi

case "$cmd" in
    lex | scan | parse | load | desugar | check | mono | esc) ;;
    *)
        usage
        echo "differential: command must be lex, scan, parse, load, desugar, check, mono, or esc" >&2
        exit 2
        ;;
esac

tmp=$(mktemp -d "${TMPDIR:-/tmp}/dusk-differential.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

run_dump() {
    local binary=$1
    local file=$2
    local stdout_path=$3
    local stderr_path=$4
    local status

    set +e
    "$binary" "$cmd" "$file" >"$stdout_path" 2>"$stderr_path"
    status=$?
    set -e

    return "$status"
}

first_diff_line() {
    local left=$1
    local right=$2
    local line

    set +e
    line=$(
        diff -u --label "$binary_a stdout" --label "$binary_b stdout" "$left" "$right" |
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
        echo "stdout differs"
    fi
}

first_diag_header() {
    local stderr_path=$1

    sed -nE 's/^(.+): ([0-9]+):[0-9]+: error:.*/\1: \2:/p' "$stderr_path" | head -n 1
}

diag_header_multiset() {
    local stderr_path=$1
    local out_path=$2

    sed -nE 's/^(.+): ([0-9]+):[0-9]+: error:.*/\1: \2:/p' "$stderr_path" | sort >"$out_path"
}

count=0
skipped=0
while IFS= read -r -d '' file; do
    # check, mono, and esc still exclude the known paradigm-gated fixture until dusk1
    # carries the full sema verdicts those modes compare.
    if [[ "$cmd" == "check" || "$cmd" == "mono" || "$cmd" == "esc" ]]; then
        case "$file" in
            examples/implgate_fail.dusk)
                skipped=$((skipped + 1))
                continue
                ;;
        esac
    fi

    out_a="$tmp/a.out"
    out_b="$tmp/b.out"
    err_a="$tmp/a.err"
    err_b="$tmp/b.err"

    if run_dump "$binary_a" "$file" "$out_a" "$err_a"; then
        status_a=0
    else
        status_a=$?
    fi

    if run_dump "$binary_b" "$file" "$out_b" "$err_b"; then
        status_b=0
    else
        status_b=$?
    fi

    if [[ "$status_a" -ne "$status_b" ]]; then
        echo "divergence: $file" >&2
        echo "exit code: $binary_a returned $status_a, $binary_b returned $status_b" >&2
        exit 1
    fi

    if [[ "$cmd" == "check" ]]; then
        if [[ "$status_a" -eq 0 ]]; then
            if ! cmp -s "$out_a" "$out_b"; then
                echo "divergence: $file" >&2
                echo "first diff: $(first_diff_line "$out_a" "$out_b")" >&2
                exit 1
            fi
        else
            primary_a=$(first_diag_header "$err_a")
            primary_b=$(first_diag_header "$err_b")
            if [[ "$primary_a" != "$primary_b" ]]; then
                echo "divergence: $file" >&2
                echo "primary diagnostic: $binary_a emitted '$primary_a', $binary_b emitted '$primary_b'" >&2
                exit 1
            fi
            headers_a="$tmp/a.headers"
            headers_b="$tmp/b.headers"
            diag_header_multiset "$err_a" "$headers_a"
            diag_header_multiset "$err_b" "$headers_b"
            if ! cmp -s "$headers_a" "$headers_b"; then
                echo "advisory: header multiset differs: $file" >&2
            fi
        fi
    else
        if ! cmp -s "$out_a" "$out_b"; then
            echo "divergence: $file" >&2
            echo "first diff: $(first_diff_line "$out_a" "$out_b")" >&2
            exit 1
        fi
    fi

    count=$((count + 1))
done < <(find examples lib/std -type f -name '*.dusk' -print0 | sort -z)

if [[ "$skipped" -gt 0 ]]; then
    echo "compared $count files, skipped $skipped paradigm-gated"
else
    echo "compared $count files"
fi
