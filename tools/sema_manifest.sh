#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "usage: tools/sema_manifest.sh <dusk-binary>" >&2
}

if [[ $# -ne 1 ]]; then
    usage
    exit 2
fi

binary=$1
if [[ "$binary" != /* ]]; then
    binary=$PWD/$binary
fi

if [[ ! -x "$binary" ]]; then
    echo "sema_manifest: dusk-binary is not executable: $binary" >&2
    exit 2
fi

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
cd "$repo_root"

corpus_dir=tests/sema_corpus
manifest=$corpus_dir/manifest.tsv

if [[ ! -d "$corpus_dir" ]]; then
    echo "sema_manifest: missing corpus directory: $corpus_dir" >&2
    exit 2
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/dusk-sema-manifest.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

first_diag_header() {
    local stderr_path=$1

    sed -nE 's/^(.+): ([0-9]+):[0-9]+: error:.*/\1: \2:/p' "$stderr_path" | head -n 1
}

tmp_manifest=$tmp/manifest.tsv
: >"$tmp_manifest"

while IFS= read -r -d '' file; do
    out=$tmp/out
    err=$tmp/err

    set +e
    "$binary" check "$file" >"$out" 2>"$err"
    status=$?
    set -e

    header=$(first_diag_header "$err")
    printf '%s\t%s\t%s\n' "$file" "$status" "$header" >>"$tmp_manifest"
done < <(find "$corpus_dir" -type f -name '*.dusk' -print0 | LC_ALL=C sort -z)

mv "$tmp_manifest" "$manifest"
