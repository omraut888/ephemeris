#!/usr/bin/env bash
# The synthetic evaluation (paper/PLAN.md §3: claim A1, the T4 synthetic
# panel for B2, the T6 regime sweep), tables into paper/results/synthetic/.
#
#   scripts/paper-synthetic.sh             the full protocol
#   scripts/paper-synthetic.sh --seeds 4   a quick check (A1 always runs in full)
set -euo pipefail
cd "$(dirname "$0")/.."

PY=paper/.venv/bin/python
BUILD=paper/build/synthetic
RESULTS=paper/results/synthetic

cargo build --release -q -p ephemeris-eval
EVAL=target/release/ephemeris-eval

"$EVAL" synthetic --out "$BUILD" "$@"

# B-GLOBAL: trained on disjoint seeds of the same generator.
for gen in model-matched realistic; do
  "$PY" -I paper/baselines/lgbm.py fit-predict \
    --train "$BUILD/$gen/train-features.csv" --predict "$BUILD/$gen/features.csv" \
    --out "$BUILD/$gen/lgbm-quantiles.csv"
  "$EVAL" score-quantiles --features "$BUILD/$gen/features.csv" \
    --quantiles "$BUILD/$gen/lgbm-quantiles.csv" --method B-GLOBAL \
    --out "$BUILD/$gen/rows-lgbm.csv"
done

"$EVAL" synthetic-tables --build "$BUILD" --out "$RESULTS"
echo "synthetic results in $RESULTS" >&2
