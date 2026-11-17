#!/usr/bin/env bash
# The pre-registered ablations (D1) on every held-out account: a second,
# separately logged held-out run (paper/PLAN.md §11), made after the first
# run's results were known. It does not touch the first run's outputs.
#
#   scripts/paper-heldout-ablations.sh --rerun-reason "..."
#
# Check: the run recomputes every method for these accounts; its
# non-ablation rows must equal the first run's byte for byte.
set -euo pipefail
cd "$(dirname "$0")/.."

FIRST=paper/build/heldout/berka
OUT=paper/build/heldout-ablations
RESULTS=paper/results/heldout-ablations

cargo build --release -q -p ephemeris-eval
EVAL=target/release/ephemeris-eval

"$EVAL" heldout-ablations --out "$OUT" "$@"

if grep -v ',abl:' "$OUT/rows.csv" | cmp -s - "$FIRST/rows.csv"; then
  echo "identity: non-ablation rows equal the first held-out run's" >&2
else
  echo "identity FAILED: non-ablation rows differ from the first run's" >&2
  exit 1
fi

"$EVAL" ablation-tables --rows "$OUT/rows.csv" --out "$RESULTS"
grep -v '^wall_seconds\|^threads' "$OUT/data.csv" > "$RESULTS/data.csv"
echo "held-out ablations in $RESULTS" >&2
