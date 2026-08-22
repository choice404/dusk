#!/usr/bin/env bash
# runner-differential.sh: prove a rewrite of the test runner changed nothing the
# suite can see. The previous release's runner source is taken from git at the
# tag given, built with the previous release's compiler, and run over the
# current manifest beside the runner built from the current tree (each in its
# own scratch tree, so nothing under target/ moves); both reports must be byte
# identical and both must pass.
#
# Usage: tools/runner-differential.sh <prev-tag> <prev-dusk> <dusk-under-test> [dawn-under-test]
set -u
TAG=${1:?usage: runner-differential.sh <prev-tag> <prev-dusk> <dusk-under-test> [dawn-under-test]}
PREV=${2:?}; BIN=${3:?}; DAWN=${4:-$(dirname "$3")/dawn}
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT"
PREV=$(realpath "$PREV"); BIN=$(realpath "$BIN"); DAWN=$(realpath "$DAWN")
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/old/tests/runner" "$WORK/old/target/dusk-out"
for f in $(git ls-tree --name-only "$TAG" tests/runner/); do git show "$TAG:$f" > "$WORK/old/$f"; done
cp -r lib "$WORK/old/lib"; cp -r runtime "$WORK/old/runtime"
( cd "$WORK/old" && DUSK_HOME="$WORK/old" timeout 600 bash -c 'ulimit -v 25165824; ulimit -t 900; exec nice -n 19 "$0" build tests/runner/testrun.dusk' "$PREV" ) || { echo "runner-differential: FAIL: the previous runner does not build"; exit 1; }
OLDRUN="$WORK/old/target/dusk-out/testrun"
mkdir -p "$WORK/new/tests" "$WORK/new/target/dusk-out"
cp -r tests/runner "$WORK/new/tests/runner"
( cd "$WORK/new" && DUSK_HOME="$ROOT" timeout 600 bash -c 'ulimit -v 25165824; ulimit -t 900; exec nice -n 19 "$0" build tests/runner/testrun.dusk' "$BIN" ) || { echo "runner-differential: FAIL: the new runner does not build"; exit 1; }
NEWRUN="$WORK/new/target/dusk-out/testrun"
run_suite() { DUSK_HOME="$ROOT" DUSK_BIN="$BIN" DAWN_BIN="$DAWN" timeout 3000 bash -c 'ulimit -v 12582912; ulimit -t 3600; exec nice -n 19 "$0" tests/goldens.manifest' "$1" > "$2" 2>&1; echo "exit=$?" >> "$2"; }
run_suite "$OLDRUN" "$WORK/old.report"
run_suite "$NEWRUN" "$WORK/new.report"
tail -2 "$WORK/old.report"; tail -2 "$WORK/new.report"
if cmp -s "$WORK/old.report" "$WORK/new.report" && [[ "$(tail -1 "$WORK/new.report")" == "exit=0" ]]; then echo "runner-differential: PASS"; else diff "$WORK/old.report" "$WORK/new.report" | head -20; echo "runner-differential: FAIL"; exit 1; fi
