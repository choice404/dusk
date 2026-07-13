#!/bin/sh
# testrun-selftest.sh: prove the dusk test runner itself. Builds the runner, runs
# it against tests/selftest.manifest, and asserts that the set of failing records
# is exactly the deliberate _bad set and that the tally is eight passed, eight
# failed. A green run here means the runner accepts a correct expectation and
# rejects a wrong one across every mode.
#
# Run from anywhere; the script places itself at the repository root, which the
# runner requires since manifest paths are relative to it.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

DUSK="${DUSK:-$ROOT/target/dusk-out/dusk}"
if [ ! -x "$DUSK" ]; then
    echo "selftest: no compiler at $DUSK; build one first (see tools/bootstrap.sh) or set DUSK" >&2
    exit 1
fi

echo "selftest: building the runner"
DUSK_HOME="$ROOT" "$DUSK" build tests/runner/testrun.dusk >/dev/null

OUT=$(mktemp)
trap 'rm -f "$OUT"' EXIT

set +e
DUSK_HOME="$ROOT" DUSK_BIN="$DUSK" target/dusk-out/testrun tests/selftest.manifest >"$OUT" 2>&1
CODE=$?
set -e

cat "$OUT"

FAILS=1

if [ "$CODE" -eq 0 ]; then
    echo "selftest: FAIL: the runner exited 0, but the manifest has deliberate failures" >&2
    FAILS=0
fi

EXPECTED="buildfail_bad checkfail_bad checkok_bad errabsent_bad run_bad runraw_bad special_bad tool_bad"
GOT=$(grep '^FAIL ' "$OUT" | awk '{print $2}' | sort | tr '\n' ' ' | sed 's/ *$//')
if [ "$GOT" != "$EXPECTED" ]; then
    echo "selftest: FAIL: failing set mismatch" >&2
    echo "  expected: $EXPECTED" >&2
    echo "  got:      $GOT" >&2
    FAILS=0
fi

if ! grep -q '^testrun: 16 tests, 8 passed, 8 failed$' "$OUT"; then
    echo "selftest: FAIL: summary line is not '16 tests, 8 passed, 8 failed'" >&2
    FAILS=0
fi

if [ "$FAILS" -eq 1 ]; then
    echo "selftest: PASS"
    exit 0
fi
echo "selftest: FAIL"
exit 1
