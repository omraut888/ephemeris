#!/usr/bin/env bash
# Rebuild the paper's results from the pinned datasets (paper/PLAN.md §6).
#
#   scripts/paper.sh            every stage below, then the figures
#
# Stages:
#   - development split: Berka backtest, B-GLOBAL, the revision candidates
#     V0-V4 (PLAN §9), tables into paper/results/dev/;
#   - synthetic: A1, both generators, the regime sweep (paper-synthetic.sh);
#   - MoneyData: the one-account case study;
#   - bit identity on this Mac (paper-identity.sh);
#   - figure data and figures (paper/figures/).
# The held-out run is not here: it ran once, guarded and logged
# (scripts/paper-heldout.sh, PLAN §10-§11). Its tables are committed; the
# figure data reads its rows from paper/build/heldout/ when present.
# Still to come: the PDF.
#
# Needs: the Python environment (uv venv --python 3.11 paper/.venv, then
# uv pip install --python paper/.venv/bin/python --require-hashes
# -r paper/requirements.txt) and, on macOS, Homebrew's libomp for LightGBM.
set -euo pipefail
cd "$(dirname "$0")/.."

PY=paper/.venv/bin/python
BUILD=paper/build
RESULTS=paper/results

# 1. Data present and matching the pinned checksums (never re-fetched here).
"$PY" -I - <<'EOF'
import hashlib, pathlib, sys
root = pathlib.Path("data")
bad = []
for line in pathlib.Path("paper/data.sha256").read_text().splitlines():
    if not line.strip() or line.startswith("#"):
        continue
    want, name = line.split()
    p = root / name
    if not p.exists():
        bad.append(f"{name}: missing")
    elif hashlib.sha256(p.read_bytes()).hexdigest() != want:
        bad.append(f"{name}: checksum differs")
if bad:
    sys.exit("data check failed (run paper/.venv/bin/python -I scripts/fetch_paper_data.py):\n  "
             + "\n  ".join(bad))
print("data matches paper/data.sha256", file=sys.stderr)
EOF

cargo build --release -q -p ephemeris-eval
EVAL=target/release/ephemeris-eval

# 2. Berka, development split: every method, every eligible origin.
"$EVAL" berka --out "$BUILD/berka-dev"

# 3. B-GLOBAL, cross-fitted over development accounts, then scored.
"$PY" -I paper/baselines/lgbm.py crossfit \
  --features "$BUILD/berka-dev/features.csv" --out "$BUILD/berka-dev/lgbm-quantiles.csv"
"$EVAL" score-quantiles --features "$BUILD/berka-dev/features.csv" \
  --quantiles "$BUILD/berka-dev/lgbm-quantiles.csv" --method B-GLOBAL \
  --out "$BUILD/berka-dev/rows-lgbm.csv"

# 4. Tables (committed), plus the data summary.
"$EVAL" tables --rows "$BUILD/berka-dev/rows.csv" --rows "$BUILD/berka-dev/rows-lgbm.csv" \
  --out "$RESULTS/dev"
grep -v '^wall_seconds\|^threads' "$BUILD/berka-dev/data.csv" > "$RESULTS/dev/data.csv"

# 5. The revision candidates V0–V4 (PLAN §9) and the selection rule.
"$EVAL" variants --out "$BUILD/variants-dev"
"$EVAL" variant-tables --rows "$BUILD/variants-dev/rows.csv" \
  --rows "$BUILD/berka-dev/rows.csv" --rows "$BUILD/berka-dev/rows-lgbm.csv" \
  --out "$RESULTS/dev"
grep -v '^wall_seconds' "$BUILD/variants-dev/data.csv" > "$RESULTS/dev/variants-data.csv"
echo "development results in $RESULTS/dev" >&2

# 6. Synthetic evaluation.
scripts/paper-synthetic.sh

# 7. MoneyData, one account, descriptive.
"$EVAL" moneydata --out "$BUILD/moneydata" --results "$RESULTS/moneydata"

# 8. Bit identity on this Mac, with the current engine. The frozen engine's
#    result stays in paper/results/identity/ (PLAN §11) and is not rewritten.
scripts/paper-identity.sh paper/results/identity-postfix

# 9. Figures. Their data comes from committed tables, plus the held-out
#    rows for the coverage curve when this machine has them.
if [ -s "$BUILD/heldout/variants/rows.csv" ]; then
  "$EVAL" figure-data --heldout "$BUILD/heldout" --out "$RESULTS/figures"
else
  echo "no held-out rows here: keeping the committed figure data" >&2
fi
"$PY" -I paper/figures/src/make_figures.py
