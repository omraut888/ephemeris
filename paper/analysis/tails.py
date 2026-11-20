#!/usr/bin/env python3
"""One-sided misses of the central 90% interval, held-out Berka (post hoc).

    paper/.venv/bin/python -I paper/analysis/tails.py

Computed after the held-out results were known (review response, Tier 1
item 8), from the stored per-forecast rows of the first held-out run; no
new forecasts. A forecast misses low when the truth lies below the 90%
interval (`in90 = 0` and PIT < 1/2) and high when above it (`in90 = 0`
and PIT >= 1/2); the two add up exactly to 1 - coverage. Nominal: 5% each.
The low tail is the one overdraft risk depends on.

Writes paper/results/heldout-posthoc/tails.csv: pooled rates with 95%
intervals from 2000 bootstrap resamples of whole accounts (seed 6).
"""
import csv
import pathlib
from collections import defaultdict

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[2]
B = ROOT / "paper/build/heldout"
METHODS = ["V0", "V1", "V2", "B-SA+", "B-EQ", "B-GLOBAL"]
HS = ["7", "30", "90"]
RESAMPLES, SEED = 2000, 6


def main():
    # per (method, h): account -> [n, low, high]
    acc = defaultdict(lambda: defaultdict(lambda: [0, 0, 0]))
    for p in [B / "variants/rows.csv", B / "berka/rows.csv", B / "rows-lgbm.csv"]:
        with open(p) as f:
            for r in csv.DictReader(f):
                if r["method"] not in METHODS:
                    continue
                a = acc[(r["method"], r["h"])][r["account"]]
                a[0] += 1
                if r["in90"] == "0":
                    if float(r["pit"]) < 0.5:
                        a[1] += 1
                    else:
                        a[2] += 1
    rng = np.random.default_rng(SEED)
    out = [["method", "h", "forecasts", "accounts", "miss_low", "miss_low_lo", "miss_low_hi",
            "miss_high", "miss_high_lo", "miss_high_hi"]]
    for m in METHODS:
        for h in HS:
            arr = np.array(list(acc[(m, h)].values()), dtype=float)
            n = len(arr)
            idx = rng.integers(0, n, size=(RESAMPLES, n))
            s = arr[idx].sum(axis=1)
            low, high = s[:, 1] / s[:, 0], s[:, 2] / s[:, 0]
            tot = arr.sum(axis=0)
            q = lambda v: np.quantile(v, [0.025, 0.975])  # noqa: E731
            out.append([m, h, int(tot[0]), n, f"{tot[1] / tot[0]:.4f}", *(f"{x:.4f}" for x in q(low)),
                        f"{tot[2] / tot[0]:.4f}", *(f"{x:.4f}" for x in q(high))])
    path = ROOT / "paper/results/heldout-posthoc/tails.csv"
    with open(path, "w", newline="") as f:
        csv.writer(f, lineterminator="\n").writerows(out)
    print(path.read_text())


if __name__ == "__main__":
    main()
