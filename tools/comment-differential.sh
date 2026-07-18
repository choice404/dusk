#!/usr/bin/env bash
# comment-differential.sh: prove block comments are IR-invisible.
#
# For every example whose baseline `dusk ir` succeeds, produce a rewritten
# copy where line comments become block comments without moving any token's
# line or column, then assert the emitted IR is byte-identical.
#
# Two rewrites per file:
#   A) per-line wrap: `// text` -> `/* text */` (trailing and full-line)
#   B) run merge: N consecutive full-line comments -> one N-line block
#      (first line's // becomes /*, last line gains a closing */)
#
# Lines are skipped conservatively when the rewrite could change meaning:
# comment text containing /* or */, or a // preceded by an odd number of
# quotes (likely inside a string literal). Skipped lines stay as they are;
# the file still participates with its remaining rewrites.
#
# Usage: tools/comment-differential.sh <dusk-binary> [examples-dir]
set -u

BIN=${1:?usage: comment-differential.sh <dusk-binary> [examples-dir]}
DIR=${2:-examples}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
BIN=$(realpath "$BIN")
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

rewrite_a() { # per-line wrap
  awk '
    {
      line = $0
      i = index_outside_strings(line)
      if (i > 0) {
        text = substr(line, i + 2)
        if (text !~ /\*\// && text !~ /\/\*/) {
          line = substr(line, 1, i - 1) "/*" text " */"
        }
      }
      print line
    }
    function index_outside_strings(s,   j, n, c, instr, esc) {
      n = length(s); instr = 0
      for (j = 1; j < n; j++) {
        c = substr(s, j, 1)
        if (instr) {
          if (c == "\\") { j++ } else if (c == "\"") { instr = 0 }
        } else {
          if (c == "\"") { instr = 1 }
          else if (c == "/" && substr(s, j + 1, 1) == "/") return j
        }
      }
      return 0
    }
  ' "$1"
}

rewrite_b() { # merge runs of full-line comments into one block
  awk '
    function flush(   k) {
      if (nrun == 0) return
      if (nrun == 1 || bad) { for (k = 1; k <= nrun; k++) print run[k] }
      else {
        first = run[1]; sub(/\/\//, "/*", first); print first
        for (k = 2; k < nrun; k++) print run[k]
        print run[nrun] " */"
      }
      nrun = 0; bad = 0
    }
    /^[ \t]*\/\// {
      nrun++; run[nrun] = $0
      # a comment opener or closer in the run text would nest or close the block
      if ($0 ~ /\*\//) bad = 1
      if ($0 ~ /\/\*/) bad = 1
      next
    }
    { flush(); print }
    END { flush() }
  ' "$1"
}

total=0; compared=0; skipped=0; fail=0
for f in "$DIR"/*.dusk; do
  total=$((total + 1))
  base="$WORK/base.ll"
  if ! DUSK_HOME="$ROOT" nice -n 19 timeout 60 "$BIN" ir "$f" > "$base" 2>/dev/null; then
    skipped=$((skipped + 1)); continue
  fi
  grep -q '//' "$f" || { skipped=$((skipped + 1)); continue; }
  stem=$(basename "$f")
  for mode in a b; do
    # Privatize suffixes and fault strings derive from the file name and the
    # path as spelled, so both variants must compile from the very same path:
    # original first, then the rewrite overwrites it in place.
    site="$WORK/site/$stem"
    mkdir -p "$WORK/site"
    if [ "$mode" = a ]; then rewrite_a "$f" > "$WORK/rw.dusk"; else rewrite_b "$f" > "$WORK/rw.dusk"; fi
    cmp -s "$f" "$WORK/rw.dusk" && continue
    ob="$WORK/ob.ll"; nb="$WORK/nb.ll"
    cp "$f" "$site"
    DUSK_HOME="$ROOT" nice -n 19 timeout 60 "$BIN" ir "$site" > "$ob" 2>/dev/null || continue
    cp "$WORK/rw.dusk" "$site"
    if ! DUSK_HOME="$ROOT" nice -n 19 timeout 60 "$BIN" ir "$site" > "$nb" 2>/dev/null; then
      echo "FAIL(compile) $f [$mode]: rewritten copy no longer compiles"
      fail=$((fail + 1)); continue
    fi
    if ! cmp -s "$ob" "$nb"; then
      echo "FAIL(ir) $f [$mode]: IR diverged"
      diff <(head -50 "$ob") <(head -50 "$nb") | head -20
      fail=$((fail + 1)); continue
    fi
    compared=$((compared + 1))
  done
done

echo "comment-differential: $total files, $compared comparisons byte-equal, $skipped skipped (no ir/no comments), $fail failures"
[ "$fail" -eq 0 ]
