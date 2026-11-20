#!/usr/bin/env python3
"""The post-freeze fix's effect on results (paper/PLAN.md §11).

    paper/.venv/bin/python -I paper/analysis/postfix_delta.py \
        --frozen paper/build/variants-dev/rows.csv \
        --fixed paper/build/postfix-variants/rows.csv \
        --out paper/results/postfix

The frozen development rows came from the frozen engine, whose rand_distr
used std math. The fixed rows come from a rerun of the same development
accounts with the fixed engine. Only V0 and V2 are compared, over the
(account, origin, horizon) forecasts present in both files.

Writes:
- cells.csv: per method and horizon, coverage at 50/80/90/95% and mean
  normalized CRPS, frozen against fixed, with the differences;
- forecasts.csv: per method and horizon, how many forecasts changed at
  all, how many changed an interval's coverage, and the largest absolute
  and relative CRPS change of a single forecast.
"""
import argparse
import csv
import pathlib

METHODS = ["V0", "V2"]
HORIZONS = ["7", "30", "90"]
LEVELS = ["in50", "in80", "in90", "in95"]


def load(path, accounts=None):
    out = {}
    with open(path, newline="") as f:
        for r in csv.DictReader(f):
            if r["method"] not in METHODS:
                continue
            if accounts is not None and r["account"] not in accounts:
                continue
            out[(r["method"], r["h"], r["account"], r["origin"])] = r
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--frozen", type=pathlib.Path, required=True)
    ap.add_argument("--fixed", type=pathlib.Path, required=True)
    ap.add_argument("--out", type=pathlib.Path, required=True)
    a = ap.parse_args()
    fixed = load(a.fixed)
    accounts = {k[2] for k in fixed}
    frozen = load(a.frozen, accounts)
    keys = sorted(set(frozen) & set(fixed))
    if len(keys) != len(fixed) or len(keys) != len(frozen):
        raise SystemExit(f"rows do not pair up: frozen {len(frozen)}, fixed {len(fixed)}, both {len(keys)}")
    a.out.mkdir(parents=True, exist_ok=True)
    cells = [["method", "h", "forecasts", "accounts"]
             + [f"{c}_{w}" for c in ["cov50", "cov80", "cov90", "cov95", "mean_crps_norm"]
                for w in ["frozen", "fixed", "diff"]]]
    fc = [["method", "h", "forecasts", "forecasts_changed", "coverage_flips",
           "max_abs_crps_change", "max_rel_crps_change"]]
    for m in METHODS:
        for h in HORIZONS:
            ks = [k for k in keys if k[0] == m and k[1] == h]
            n = len(ks)
            row = [m, h, n, len({k[2] for k in ks})]
            for lv in LEVELS:
                f0 = sum(frozen[k][lv] == "1" for k in ks) / n
                f1 = sum(fixed[k][lv] == "1" for k in ks) / n
                row += [f"{f0:.6f}", f"{f1:.6f}", f"{f1 - f0:+.6f}"]
            c0 = sum(float(frozen[k]["crps"]) for k in ks) / n
            c1 = sum(float(fixed[k]["crps"]) for k in ks) / n
            row += [f"{c0:.6f}", f"{c1:.6f}", f"{c1 - c0:+.3e}"]
            cells.append(row)
            changed = [k for k in ks if frozen[k] != fixed[k]]
            flips = sum(frozen[k][lv] != fixed[k][lv] for k in ks for lv in LEVELS)
            d_abs = [abs(float(fixed[k]["crps"]) - float(frozen[k]["crps"])) for k in ks]
            d_rel = [abs(float(fixed[k]["crps"]) - float(frozen[k]["crps"])) / float(frozen[k]["crps"])
                     for k in ks if float(frozen[k]["crps"]) > 0]
            fc.append([m, h, n, len(changed), flips, f"{max(d_abs):.3e}", f"{max(d_rel):.3e}"])
    for name, rows in (("cells.csv", cells), ("forecasts.csv", fc)):
        with open(a.out / name, "w", newline="") as f:
            csv.writer(f, lineterminator="\n").writerows(rows)
        print((a.out / name).read_text())


if __name__ == "__main__":
    main()
