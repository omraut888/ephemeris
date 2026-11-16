#!/usr/bin/env bash
# The held-out evaluation (paper/PLAN.md §10), exactly once.
#
#   scripts/paper-heldout.sh                        the first and only run
#   scripts/paper-heldout.sh --rerun-reason "..."   only with the user's approval
#
# `ephemeris-eval heldout` refuses to run from a dirty working tree, logs
# the run to paper/results/heldout-runs.log, and refuses a second run without
# --rerun-reason. It runs every method and the variants V0–V4 on the held-out
# accounts, with V3's priors from all development accounts. B-GLOBAL is
# trained on all development accounts (lgbm.py fit-predict), so the
# development features from scripts/paper.sh must exist.
set -euo pipefail
cd "$(dirname "$0")/.."

PY=paper/.venv/bin/python
DEV=paper/build/berka-dev
OUT=paper/build/heldout
RESULTS=paper/results/heldout

test -s "$DEV/features.csv" || { echo "run scripts/paper.sh first (development features)" >&2; exit 1; }

cargo build --release -q -p ephemeris-eval
EVAL=target/release/ephemeris-eval

# 1. Guarded, logged: the backtest of every method, then V0–V4.
"$EVAL" heldout --out "$OUT" "$@"

# 2. B-GLOBAL: trained on all development accounts, applied to held-out.
"$PY" -I paper/baselines/lgbm.py fit-predict \
  --train "$DEV/features.csv" --predict "$OUT/berka/features.csv" \
  --out "$OUT/lgbm-quantiles.csv"
"$EVAL" score-quantiles --features "$OUT/berka/features.csv" \
  --quantiles "$OUT/lgbm-quantiles.csv" --method B-GLOBAL --out "$OUT/rows-lgbm.csv"

# 3. The same tables as development.
"$EVAL" tables --rows "$OUT/berka/rows.csv" --rows "$OUT/rows-lgbm.csv" --out "$RESULTS"
grep -v '^wall_seconds\|^threads' "$OUT/berka/data.csv" > "$RESULTS/data.csv"
"$EVAL" variant-tables --rows "$OUT/variants/rows.csv" --rows "$OUT/berka/rows.csv" \
  --rows "$OUT/rows-lgbm.csv" --out "$RESULTS"
grep -v '^wall_seconds' "$OUT/variants/data.csv" > "$RESULTS/variants-data.csv"
echo "held-out results in $RESULTS" >&2
