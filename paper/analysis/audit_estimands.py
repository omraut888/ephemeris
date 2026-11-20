#!/usr/bin/env python3
"""Audit of the CRPS skill estimands (review response, Tier 1 A1).

    paper/.venv/bin/python -I paper/analysis/audit_estimands.py --split heldout

Reads the stored per-forecast rows of a split (no new forecasts) and writes
paper/results/audit/estimands-<split>.csv and a short report on stdout:

- the forecasts and accounts each method has per horizon (origin sets);
- for each comparison and horizon, three aggregations of CRPS skill:
  * mean of per-account ratios, `mean_a (1 − Σ_a CRPS_m / Σ_a CRPS_ref)`
    (what aggregate.rs reports as `skill`), over the forecasts both made;
  * ratio of pooled sums, `1 − Σ CRPS_m / Σ CRPS_ref` (`pooled_skill`);
  * the same mean of ratios after dropping the accounts in the lowest
    decile of mean reference CRPS (sensitivity to tiny denominators);
- the mean CRPS of each method on the common forecasts (normalized units);
- for the strata, the account counts per stratum and the pooled skill.
"""
import argparse
import collections
import csv
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
HS = ["7", "30", "90"]


def load(split):
    base = ROOT / "paper/build" / ("heldout" if split == "heldout" else "berka-dev")
    paths = ([base / "variants/rows.csv", base / "berka/rows.csv", base / "rows-lgbm.csv"]
             if split == "heldout" else
             [ROOT / "paper/build/variants-dev/rows.csv", base / "rows.csv", base / "rows-lgbm.csv"])
    crps = {}
    strata = {}
    for p in paths:
        with open(p) as f:
            for r in csv.DictReader(f):
                k = (r["account"], r["origin"], r["h"])
                crps[(r["method"],) + k] = float(r["crps"])
                if r["method"] == "V2":
                    strata[k] = (r["income"] == "1", int(r["months"]), r["alarm"] == "1")
    return crps, strata


def compare(crps, m, ref, h, keep=lambda k: True):
    per = collections.defaultdict(lambda: [0.0, 0.0, 0])
    for key, v in crps.items():
        if key[0] != m or key[3] != h:
            continue
        k = key[1:]
        if not keep(k):
            continue
        rv = crps.get((ref,) + k)
        if rv is None:
            continue
        a = per[k[0]]
        a[0] += v
        a[1] += rv
        a[2] += 1
    accs = [a for a in per.values() if a[1] > 0]
    n = sum(a[2] for a in accs)
    mean_ratio = sum(1 - a[0] / a[1] for a in accs) / len(accs)
    pooled = 1 - sum(a[0] for a in accs) / sum(a[1] for a in accs)
    # Sensitivity: drop the lowest decile of mean reference CRPS.
    by_ref = sorted(accs, key=lambda a: a[1] / a[2])
    cut = by_ref[len(by_ref) // 10:]
    trimmed = sum(1 - a[0] / a[1] for a in cut) / len(cut)
    worst = min(1 - a[0] / a[1] for a in accs)
    return dict(forecasts=n, accounts=len(accs), mean_of_ratios=mean_ratio, pooled=pooled,
                mean_of_ratios_trim10=trimmed, worst_account=worst,
                mean_crps_m=sum(a[0] for a in accs) / n, mean_crps_ref=sum(a[1] for a in accs) / n)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", choices=["heldout", "dev"], default="heldout")
    a = ap.parse_args()
    crps, strata = load(a.split)
    out = ROOT / "paper/results/audit"
    out.mkdir(parents=True, exist_ok=True)
    counts = collections.Counter((k[0], k[3]) for k in crps)
    accs = collections.defaultdict(set)
    for k in crps:
        accs[(k[0], k[3])].add(k[1])
    rows = [["comparison", "stratum", "h", "forecasts", "accounts", "mean_of_ratios", "pooled",
             "mean_of_ratios_trim10", "worst_account", "mean_crps_m", "mean_crps_ref"]]
    pairs = [("V2", "B-SA+"), ("B-EQ", "B-SA+"), ("V2", "B-EQ"), ("V2", "B-GLOBAL"), ("B-GLOBAL", "B-SA+"),
             ("V0", "B-SA+"), ("V1", "B-SA+")]
    strat = {
        "all": lambda k: True,
        "income detected": lambda k: strata.get(k, (None,))[0] is True,
        "no income detected": lambda k: strata.get(k, (None,))[0] is False,
    }
    for m, ref in pairs:
        for sname, f in strat.items():
            if sname != "all" and (m, ref) != ("V2", "B-SA+"):
                continue
            for h in HS:
                c = compare(crps, m, ref, h, f)
                rows.append([f"{m} vs {ref}", sname, h, c["forecasts"], c["accounts"]]
                            + [f"{c[x]:.4f}" for x in ("mean_of_ratios", "pooled", "mean_of_ratios_trim10",
                                                       "worst_account", "mean_crps_m", "mean_crps_ref")])
    with open(out / f"estimands-{a.split}.csv", "w", newline="") as fh:
        csv.writer(fh, lineterminator="\n").writerows(rows)
    with open(out / f"origin-sets-{a.split}.csv", "w", newline="") as fh:
        w = csv.writer(fh, lineterminator="\n")
        w.writerow(["method", "h", "forecasts", "accounts"])
        for (m, h) in sorted(counts):
            w.writerow([m, h, counts[(m, h)], len(accs[(m, h)])])
    for r in rows:
        print(",".join(str(x) for x in r))


if __name__ == "__main__":
    main()
