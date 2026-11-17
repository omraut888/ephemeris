#!/usr/bin/env bash
# Post-hoc, after the held-out run (paper/PLAN.md §11): paired bootstrap
# intervals for V2 against each baseline at 7/30/90 days, from the scored
# rows the first held-out run wrote. No new forecasts; not a held-out run.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=paper/build/heldout
RESULTS=paper/results/heldout-posthoc

cargo build --release -q -p ephemeris-eval
target/release/ephemeris-eval pairwise \
  --rows "$OUT/variants/rows.csv" --rows "$OUT/berka/rows.csv" --rows "$OUT/rows-lgbm.csv" \
  --pair V2/B-SA+ --pair V2/B-EQ --pair V2/B-GLOBAL --pair V2/B-30 \
  --pair V2/B-LIN --pair V2/B-SA --pair V2/B-SEAS \
  --out "$RESULTS/pairwise-v2.csv"
