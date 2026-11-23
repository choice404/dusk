#!/usr/bin/env bash
# dump-differential.sh: prove a compiler change is invisible in every observable
# output. Two compilers are run over the same source set and every dump the
# pipeline can print must be byte identical, with identical exit codes:
# lex, scan, parse, load, desugar, check, check --json, mono, esc, ir,
# ir --target=wasm32, doc, and doc --json.
#
# Both compilers read the same tree through DUSK_HOME, so two correct compilers
# must agree on every dump, the compiler roots included; the allow list (one
# "<stem> <command>" per line, the stem being the path with every "/" turned
# into "_" and the command with its spaces turned into "_", for example
# "tests_dawn_dusktoonew_src_main.dusk check_--json") exists for a deliberate,
# named exception and holds only what a release can name, such as the two
# fixtures whose refusal prints the compiler's own version. A comparison where
# both compilers fail counts as agreement only when their output agrees byte for
# byte, since compile-fail programs are part of the set; the tally also prints
# how many runs succeeded on the new side so a double failure cannot hide.
#
# A release line whose IR change touches nearly every module names a normalizer
# instead of thousands of allow rows: when DUSK_DUMP_NORM names an executable
# filter, both sides of the ir and ir --target=wasm32 commands pass through it
# before the compare, so the tally counts only what the filter does not explain.
#
# Usage: [DUSK_DUMP_NORM=filter] tools/dump-differential.sh <old-dusk> <new-dusk> [allow-file] [source-list]
#   source-list defaults to every .dusk under examples/, tests/dawn/*/src/main.dusk,
#   lib/std/*.dusk, tests/runner/testrun.dusk, compiler/dusk.dusk, compiler/dawn.dusk.
# Runs serially and caged; never start one beside a self build.
set -u

OLD=${1:?usage: dump-differential.sh <old-dusk> <new-dusk> [allow-file] [source-list]}
NEW=${2:?usage: dump-differential.sh <old-dusk> <new-dusk> [allow-file] [source-list]}
ALLOW=${3:-}
LIST=${4:-}
NORM=${DUSK_DUMP_NORM:-}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
OLD=$(realpath "$OLD"); NEW=$(realpath "$NEW")
if [[ -n "$NORM" ]]; then
  NORM=$(realpath "$NORM")
  [[ -x "$NORM" ]] || { echo "dump-differential: DUSK_DUMP_NORM is not executable: $NORM" >&2; exit 2; }
fi
export DUSK_HOME="$ROOT"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

if [[ -z "$LIST" ]]; then
  LIST="$WORK/list"
  { find examples -name "*.dusk" | sort; ls tests/dawn/*/src/main.dusk 2>/dev/null; ls lib/std/*.dusk; echo tests/runner/testrun.dusk; echo compiler/dusk.dusk; echo compiler/dawn.dusk; } > "$LIST"
fi

allowed() { [[ -n "$ALLOW" ]] && grep -Fqx -- "$1 $2" "$ALLOW"; }

normalize() { # <file> <cmd>: rewrite an ir capture in place through the normalizer
  [[ -n "$NORM" ]] || return 0
  case "$2" in ir|"ir --target=wasm32") ;; *) return 0 ;; esac
  "$NORM" < "$1" > "$1.norm" && mv "$1.norm" "$1"
}

run_one() { # <bin> <cmd...> -> stdout+stderr file, exit code appended
  local bin=$1; shift
  timeout 120 bash -c 'ulimit -v 12582912; ulimit -t 300; exec nice -n 19 "$0" "$@"' "$bin" "$@" > "$WORK/out" 2>&1 < /dev/null
  echo "exit=$?" >> "$WORK/out"
}

same=0; differ=0; allowedn=0; okruns=0
while read -r f; do
  [[ -f "$f" ]] || continue
  stem=$(echo "$f" | tr '/' '_')
  for cmd in lex scan parse load desugar check "check --json" mono esc ir "ir --target=wasm32" doc "doc --json"; do
    # shellcheck disable=SC2086
    run_one "$OLD" $cmd "$f"; mv "$WORK/out" "$WORK/old"
    # shellcheck disable=SC2086
    run_one "$NEW" $cmd "$f"; mv "$WORK/out" "$WORK/new"
    grep -qx 'exit=0' "$WORK/new" && okruns=$((okruns+1))
    normalize "$WORK/old" "$cmd"; normalize "$WORK/new" "$cmd"
    if cmp -s "$WORK/old" "$WORK/new"; then
      same=$((same+1))
    elif allowed "$stem" "${cmd// /_}"; then
      allowedn=$((allowedn+1))
    else
      differ=$((differ+1))
      echo "DIFF $f [$cmd]"
      diff "$WORK/old" "$WORK/new" | head -6
    fi
  done
done < "$LIST"

echo "dump-differential: same=$same allowed=$allowedn differ=$differ (new side succeeded in $okruns runs)"
[[ $differ -eq 0 ]]
