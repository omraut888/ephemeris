#!/usr/bin/env python3
"""Checks on the stored held-out rows (round 3, A2; post hoc, no new
forecasts).

    paper/.venv/bin/python -I paper/analysis/rows_checks.py

Reads the per-forecast rows of the first held-out run (paper/build/heldout)
and writes paper/results/exploratory-heldout/s10-rows-checks/:

- coverage-tails.csv: per method and horizon, the forecasts, and the counts
  inside the central 90% interval, below it and above it, all from the same
  rows, so inside + below + above = forecasts exactly. Below: in90 = 0 and
  PIT < 1/2; above: in90 = 0 and PIT >= 1/2 (as tails.py).
- zero-reference.csv: per method and horizon, the accounts whose summed
  CRPS is 0 (they drop out of an account-mean skill that uses the method as
  the reference).
- pit-edges.csv: per method and horizon, the share of PIT values that are
  exactly 0 or 1 (the outcome lies outside the outermost stored quantile),
  and the share on a flat stretch of the quantile function (a PIT equal to
  the midpoint of two stored levels), which the histogram shows at that
  midpoint.

The frozen tables in paper/results/heldout* are not touched.
"""
import csv
import pathlib
from collections import defaultdict

ROOT = pathlib.Path(__file__).resolve().parents[2]
B = ROOT / "paper/build/heldout"
OUT = ROOT / "paper/results/exploratory-heldout/s10-rows-checks"
METHODS = ["V0", "V1", "V2", "B-SA+", "B-EQ", "B-GLOBAL", "B-SA", "B-30", "B-LIN", "B-SEAS"]
HS = ["7", "30", "90"]
SOURCES = [B / "variants/rows.csv", B / "berka/rows.csv", B / "rows-lgbm.csv"]


def main():
    counts = defaultdict(lambda: [0, 0, 0, 0])  # n, inside, below, above
    crps = defaultdict(float)  # (method, h, account) -> sum
    pit0 = defaultdict(int)
    pit1 = defaultdict(int)
    seen = set()
    for p in SOURCES:
        with open(p) as f:
            for r in csv.DictReader(f):
                m, h = r["method"], r["h"]
                if m not in METHODS:
                    continue
                key = (m, h, r["account"], r["origin"])
                if key in seen:  # V0 appears in two files; count it once
                    continue
                seen.add(key)
                c = counts[(m, h)]
                c[0] += 1
                pit = float(r["pit"])
                if r["in90"] == "1":
                    c[1] += 1
                elif pit < 0.5:
                    c[2] += 1
                else:
                    c[3] += 1
                crps[(m, h, r["account"])] += float(r["crps"])
                pit0[(m, h)] += pit == 0.0
                pit1[(m, h)] += pit == 1.0
    OUT.mkdir(parents=True, exist_ok=True)
    with open(OUT / "coverage-tails.csv", "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        w.writerow(["method", "h", "forecasts", "inside", "below", "above",
                    "cov90", "miss_low", "miss_high"])
        for m in METHODS:
            for h in HS:
                n, i, lo, hi = counts[(m, h)]
                if n:
                    assert i + lo + hi == n
                    w.writerow([m, h, n, i, lo, hi, f"{i / n:.4f}", f"{lo / n:.4f}", f"{hi / n:.4f}"])
    zero = defaultdict(int)
    accounts = defaultdict(int)
    for (m, h, _), v in crps.items():
        accounts[(m, h)] += 1
        zero[(m, h)] += v == 0.0
    with open(OUT / "zero-reference.csv", "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        w.writerow(["method", "h", "accounts", "accounts_zero"])
        for m in METHODS:
            for h in HS:
                if accounts[(m, h)]:
                    w.writerow([m, h, accounts[(m, h)], zero[(m, h)]])
    with open(OUT / "pit-edges.csv", "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        w.writerow(["method", "h", "forecasts", "share_pit0", "share_pit1"])
        for m in METHODS:
            for h in HS:
                n = counts[(m, h)][0]
                if n:
                    w.writerow([m, h, n, f"{pit0[(m, h)] / n:.6f}", f"{pit1[(m, h)] / n:.6f}"])
    print((OUT / "coverage-tails.csv").read_text())
    print((OUT / "pit-edges.csv").read_text())


if __name__ == "__main__":
    main()
