#!/usr/bin/env python3
"""Summaries of the exploratory analyses (paper/REVISION.md S1-S8).
Development split only. No new forecasts.

    paper/.venv/bin/python -I paper/analysis/explore_summary.py S1|S2|S3|S4|S5|S6|S7 ...

Definitions are those of crates/ephemeris-eval/src/aggregate.rs:
- coverage: pooled over forecasts;
- skill against a reference, over the forecasts both made:
  - account mean: mean_a (1 - sum_a CRPS_m / sum_a CRPS_ref);
  - pooled: 1 - sum CRPS_m / sum CRPS_ref;
- intervals: 95% from 2000 bootstrap resamples of accounts (seeds on
  synthetic data), seed 6.
Every comparison is on the origins all its methods share; counts are
written with it. One-sided 90% misses split the 90% misses by PIT < 1/2.
"""
import csv
import pathlib
import sys
from collections import defaultdict

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[2]
B = ROOT / "paper/build"
OUT = ROOT / "paper/results/exploratory"
HS = ["7", "30", "90"]
RESAMPLES, SEED = 2000, 6
FROZEN_DEV = [B / "variants-dev/rows.csv", B / "berka-dev/rows.csv", B / "berka-dev/rows-lgbm.csv"]
FROZEN_HELDOUT = [B / "heldout/variants/rows.csv", B / "heldout/berka/rows.csv", B / "heldout/rows-lgbm.csv"]
# Switched by --heldout (the exploratory held-out runs, paper/PROGRESS.md step 1).
FROZEN = FROZEN_DEV
EXP = B / "explore"



def load(paths, methods=None, rename=None):
    """{(account, origin, h): {method: row}} for the given methods."""
    out = defaultdict(dict)
    for p in paths:
        with open(p) as f:
            for r in csv.DictReader(f):
                m = r["method"]
                if methods is not None and m not in methods:
                    continue
                if rename and m in rename:
                    m = rename[m]
                if True:
                    out[(r["account"], r["origin"], r["h"])][m] = r
    return out


def common(data, methods, h=None):
    return {k: v for k, v in data.items() if all(m in v for m in methods) and (h is None or k[2] == h)}


def write(path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", newline="") as f:
        csv.writer(f, lineterminator="\n").writerows(rows)


def fmt(x, d=4):
    return "" if x is None or (isinstance(x, float) and np.isnan(x)) else f"{x:.{d}f}"


def boot_ratio(num, den, rng):
    """Account mean and pooled skill with bootstrap intervals, from
    per-account sums (arrays)."""
    n = len(num)
    idx = rng.integers(0, n, size=(RESAMPLES, n))
    ok = den > 0
    per = np.where(ok, 1 - num / np.where(ok, den, 1), np.nan)
    mean = np.nanmean(per)
    pooled = 1 - num.sum() / den.sum()
    pm = np.nanmean(per[idx], axis=1)
    pp = 1 - num[idx].sum(axis=1) / den[idx].sum(axis=1)
    q = lambda v: np.quantile(v, [0.025, 0.975])  # noqa: E731
    return mean, *q(pm), pooled, *q(pp)


def method_stats(sub, m):
    """Pooled coverage, misses, mean CRPS, median width for method m."""
    rows = [v[m] for v in sub.values()]
    n = len(rows)
    cov = [sum(r[f"in{l}"] == "1" for r in rows) / n for l in (50, 80, 90, 95)]
    low = sum(r["in90"] == "0" and float(r["pit"]) < 0.5 for r in rows) / n
    high = sum(r["in90"] == "0" and float(r["pit"]) >= 0.5 for r in rows) / n
    crps = sum(float(r["crps"]) for r in rows) / n
    acc = defaultdict(list)
    for r in rows:
        acc[r["account"]].append(float(r["width90"]) / float(r["k"]))
    width = float(np.median([np.mean(v) for v in acc.values()]))
    return [n, len(acc), *cov, low, high, crps, width]


def compare(sub, m, ref, rng):
    num, den = defaultdict(float), defaultdict(float)
    for k, v in sub.items():
        num[k[0]] += float(v[m]["crps"])
        den[k[0]] += float(v[ref]["crps"])
    a = sorted(num)
    return boot_ratio(np.array([num[x] for x in a]), np.array([den[x] for x in a]), rng)


STAT_HEAD = ["method", "h", "forecasts", "accounts", "cov50", "cov80", "cov90", "cov95",
             "miss_low90", "miss_high90", "mean_crps", "median_width90"]
CMP_HEAD = ["comparison", "h", "forecasts", "accounts", "skill_mean", "skill_mean_lo", "skill_mean_hi",
            "skill_pooled", "skill_pooled_lo", "skill_pooled_hi"]


def panel(data, methods, pairs, out, keep=lambda k, v: True, label=""):
    """Stats of every method and every pair, on common origins per horizon."""
    rng = np.random.default_rng(SEED)
    stats, cmps = [STAT_HEAD], [CMP_HEAD]
    for h in HS:
        sub = {k: v for k, v in common(data, methods, h).items() if keep(k, v)}
        if not sub:
            continue
        for m in methods:
            stats.append([m, h] + [fmt(x) if isinstance(x, float) else x for x in method_stats(sub, m)])
        for m, ref in pairs:
            r = compare(sub, m, ref, rng)
            accounts = len({k[0] for k in sub})
            cmps.append([f"{m} vs {ref}", h, len(sub), accounts] + [fmt(x) for x in r])
    write(out / f"stats{label}.csv", stats)
    write(out / f"compare{label}.csv", cmps)
    return stats, cmps


def show(rows):
    for r in rows:
        print(",".join(str(x) for x in r))


# ----------------------------------------------------------------- S1 ----

def s1():
    d = B / "explore/eligibility"
    out = OUT / "s1-eligibility"
    accts = list(csv.DictReader(open(d / "accounts.csv")))
    origins = list(csv.DictReader(open(d / "origins.csv")))
    f = sum(r["f"] == "1" for r in origins)
    p = sum(r["p"] == "1" for r in origins)
    both = sum(r["f"] == "1" and r["p"] == "1" for r in origins)
    excl = [r for r in accts if r["drift_excluded"] == "1"]
    min_drift = min(float(r["drift_to_origin"]) for r in origins
                    if r["account"] in {a["account"] for a in excl} and r["minrec_ok"] == "1")
    rows = [["key", "value"], ["accounts", len(accts)], ["origins_f", f], ["origins_p", p],
            ["origins_both", both], ["p_not_f", p - both], ["f_not_p", f - both],
            ["drift_excluded_accounts", len(excl)],
            ["min_drift_at_a_grid_origin_of_an_excluded_account", fmt(min_drift, 1)]]
    write(out / "rules.csv", rows)

    def q(vals):
        v = np.array([x for x in vals if not np.isnan(x)])
        return [len(v), *(fmt(x, 3) for x in np.quantile(v, [0.25, 0.5, 0.75]))] if len(v) else [0, "", "", ""]
    groups = [["group", "measure", "n", "q25", "median", "q75"]]
    kept = [r for r in accts if r["drift_excluded"] == "0"]
    g2a = [r for r in kept if int(r["eligible_f"]) > 0]
    g2b = [r for r in kept if int(r["eligible_f"]) == 0]
    for name, grp in (("G1 drift-excluded", excl), ("G1 kept", kept),
                      ("G2 kept, no eligible origin", g2b), ("G2 kept, with eligible origins", g2a)):
        for meas in ("records_per_month", "volatility", "income_share"):
            groups.append([name, meas] + q([float(r[meas]) for r in grp]))
    kept_ids = {r["account"] for r in kept}
    elig = [r for r in origins if r["f"] == "1"]
    fail = [r for r in origins if r["account"] in kept_ids and r["minrec_ok"] == "0" and r["scale_ok"] == "1"]
    fail_noscale = [r for r in origins if r["account"] in kept_ids and r["minrec_ok"] == "0"]
    for name, grp in (("G3 eligible origins", elig), ("G3 failing min-records (scale defined)", fail),
                      ("G3 failing min-records (all)", fail_noscale)):
        for meas in ("records_180", "daily_sd_over_s"):
            groups.append([name, meas] + q([float(r[meas]) for r in grp]))
    write(out / "groups.csv", groups)
    show(rows)
    show(groups)


# ----------------------------------------------------------------- S2 ----

def s2():
    out = OUT / "s2-small"
    c = list(csv.DictReader(open(B / "explore/crossings/crossings.csv")))
    n = len(c)
    nonmono = sum(r["monotone"] == "0" for r in c)
    moved = sum(int(r["values_moved"]) for r in c)
    vals = sum(int(r["values"]) for r in c)
    rows = [["h", "forecasts", "non_monotone", "share_non_monotone", "values_moved_share"]]
    for h in HS + ["all"]:
        s = [r for r in c if h == "all" or r["h"] == h]
        rows.append([h, len(s), sum(r["monotone"] == "0" for r in s),
                     fmt(sum(r["monotone"] == "0" for r in s) / len(s)),
                     fmt(sum(int(r["values_moved"]) for r in s) / sum(int(r["values"]) for r in s))])
    write(out / "v1-crossings.csv", rows)
    show(rows)
    print(f"total {n}, non-monotone {nonmono}, values moved {moved}/{vals}")
    # Selection sensitivity: account-equal score, qualification unchanged.
    sel = {(r["method"], r["h"]): r for r in csv.DictReader(open(ROOT / "paper/results/dev/selection.csv"))}
    data = load([B / "variants-dev/rows.csv"], {"V1", "V2", "V3", "V4"})
    acc = defaultdict(lambda: defaultdict(list))
    for k, v in data.items():
        for m, r in v.items():
            acc[(m, k[2])][k[0]].append(float(r["crps"]))
    res = [["method", "qualifies", "score_origin_equal", "score_account_equal"]]
    for m in ("V1", "V2", "V3", "V4"):
        ae = np.mean([np.mean([np.mean(x) for x in acc[(m, h)].values()]) for h in HS])
        res.append([m, sel[(m, "7")]["qualifies"], sel[(m, "7")]["score"], fmt(ae, 1)])
    write(out / "selection-account-equal.csv", res)
    show(res)


# ----------------------------------------------------------------- S3 ----

def s3():
    out = OUT / "s3-reversal"
    new = EXP / "berka/rows.csv"
    methods = ["V0", "V2", "B-SA+", "B-EQ", "B-EQc", "B-SR", "CP-noKF"]
    frozen = load(FROZEN, {"V0", "V2", "B-SA+", "B-EQ"})
    fresh = load([new], {"V2", "B-EQc", "B-SR", "CP-noKF"}, rename={"V2": "V2-check"})
    data = defaultdict(dict)
    for src in (frozen, fresh):
        for k, v in src.items():
            data[k].update(v)
    # Check: the exploratory run's V2 rows equal the frozen V2 rows.
    keys = [k for k, v in data.items() if "V2" in v and "V2-check" in v]
    same = sum(all(data[k]["V2"][c] == data[k]["V2-check"][c] for c in data[k]["V2"] if c != "method") for k in keys)
    write(out / "v2-identity.csv", [["key", "value"], ["compared", len(keys)], ["identical", same]])
    print(f"V2 identity: {same}/{len(keys)}")
    pairs = [(m, "B-SA+") for m in methods if m != "B-SA+"] + [
        ("B-EQc", "B-EQ"), ("B-SR", "B-EQ"), ("B-SR", "V2"), ("CP-noKF", "V2"), ("CP-noKF", "B-EQ"),
        ("V2", "B-EQ")]
    show(panel(data, methods, pairs, out)[1])
    # History length: V2 vs B-EQ by months of history (frozen rows).
    strata = {"lt12": lambda m: m < 12, "12to23": lambda m: 12 <= m < 24, "ge24": lambda m: m >= 24}
    for name, f in strata.items():
        panel(data, ["V2", "B-EQ", "B-SA+"], [("V2", "B-EQ"), ("B-EQ", "B-SA+"), ("V2", "B-SA+")], out,
              keep=lambda k, v, f=f: f(int(v["V2"]["months"])), label=f"-history-{name}")
    fb = {r["key"]: r["value"] for r in csv.DictReader(open(EXP / "berka/data.csv"))}
    write(out / "data.csv", [["key", "value"]] + [[k, v] for k, v in fb.items()])


# ----------------------------------------------------------------- S6 ----

def s6():
    out = OUT / "s6-v2-ablations"
    frozen = load(FROZEN, {"V0", "V2"})
    fresh = load([EXP / "berka/rows.csv"], {"V2-noadapt", "V2-nodetector", "V2-nofloor", "CP-noKF"})
    data = defaultdict(dict)
    for src in (frozen, fresh):
        for k, v in src.items():
            data[k].update(v)
    methods = ["V2", "V2-noadapt", "V2-nodetector", "V2-nofloor", "V0", "CP-noKF"]
    pairs = [(m, "V2") for m in methods if m != "V2"]
    show(panel(data, methods, pairs, out)[1])


# ----------------------------------------------------------------- S4 ----

def s4():
    out = OUT / "s4-grid"
    g = B / "explore/grid"
    summary = list(csv.DictReader(open(g / "data.csv")))
    methods = ["V0", "V2", "B-SA+", "B-EQ", "B-EQc", "B-SR", "CP-noKF"]
    pairs = [(m, "B-SA+") for m in methods if m != "B-SA+"] + [("V2", "B-EQ"), ("CP-noKF", "B-EQ"),
                                                              ("B-SR", "B-EQ")]
    allc = [["sparsity", "regularity", "mean_count", "sparse_fired_share"] + CMP_HEAD]
    alls = [["sparsity", "regularity"] + STAT_HEAD]
    for s in summary:
        data = load([g / f"rows-s{s['sparsity']}-r{s['regularity']}.csv"], set(methods))
        stats, cmps = panel(data, methods, pairs, out / "cells", label=f"-s{s['sparsity']}-r{s['regularity']}")
        share = int(s["sparse_fired"]) / int(s["origins"])
        for r in cmps[1:]:
            allc.append([s["sparsity"], s["regularity"], s["mean_count"], fmt(share, 3)] + r)
        for r in stats[1:]:
            alls.append([s["sparsity"], s["regularity"]] + r)
    write(out / "compare.csv", allc)
    write(out / "stats.csv", alls)
    for r in allc:
        if r[4] in ("V2 vs B-EQ", "comparison") and r[5] in ("30", "90", "h"):
            print(",".join(str(x) for x in r))


# ----------------------------------------------------------------- S5 ----

def s5():
    out = OUT / "s5-overdraft"
    ev = list(csv.DictReader(open(EXP / "berka/events.csv")))
    methods = ["V2", "CP-noKF", "B-EQp", "B-SRp", "CLIM"]
    rng = np.random.default_rng(SEED)
    head = ["threshold", "h", "forecasts", "accounts", "prevalence", "prevalence_account_mean"]
    for m in methods:
        head += [f"brier_{m}", f"brier_{m}_acct", f"bss_{m}", f"bss_{m}_lo", f"bss_{m}_hi"]
    res, rel = [head], [["threshold", "h", "method", "bin", "count", "mean_p", "observed"]]
    for u in ("u0", "u10"):
        for h in HS:
            s = [r for r in ev if r["threshold"] == u and r["h"] == h]
            y = np.array([float(r["event"]) for r in s])
            acct = np.array([r["account"] for r in s])
            ids, inv = np.unique(acct, return_inverse=True)
            cnt = np.bincount(inv)
            prev_acc = np.mean(np.bincount(inv, weights=y) / cnt)
            row = [u, h, len(s), len(ids), fmt(y.mean()), fmt(prev_acc)]
            clim = np.array([float(r["p_CLIM"]) for r in s])
            bc = np.bincount(inv, weights=(clim - y) ** 2)
            for m in methods:
                p = np.array([float(r[f"p_{m}"]) for r in s])
                b = (p - y) ** 2
                ba = np.bincount(inv, weights=b)
                acc_mean = np.mean(ba / cnt)
                idx = rng.integers(0, len(ids), size=(RESAMPLES, len(ids)))
                bss_draw = 1 - ba[idx].sum(axis=1) / bc[idx].sum(axis=1)
                bss = 1 - ba.sum() / bc.sum()
                row += [fmt(b.mean(), 5), fmt(acc_mean, 5), fmt(bss), *(fmt(x) for x in np.quantile(bss_draw, [0.025, 0.975]))]
                bins = np.minimum((p * 10).astype(int), 9)
                for k in range(10):
                    sel = bins == k
                    if sel.any():
                        rel.append([u, h, m, k, int(sel.sum()), fmt(p[sel].mean()), fmt(y[sel].mean())])
            res.append(row)
    write(out / "brier.csv", res)
    write(out / "reliability.csv", rel)
    show(res)
    plot_reliability(rel, out)


def s5c():
    """Calibration in the large of the overdraft-event probabilities
    (exploratory, added after the held-out S5 run): the mean forecast
    probability against the observed event frequency, pooled over origins,
    per method, threshold and horizon. Reads the stored S5 events only; the
    difference gets a 95% account-bootstrap interval."""
    out = OUT / "s5-overdraft"
    ev = list(csv.DictReader(open(EXP / "berka/events.csv")))
    methods = ["V2", "CP-noKF", "B-EQp", "B-SRp", "CLIM"]
    rng = np.random.default_rng(SEED)
    res = [["threshold", "h", "method", "forecasts", "accounts", "observed", "mean_p",
            "diff", "diff_lo", "diff_hi", "ratio"]]
    for u in ("u0", "u10"):
        for h in HS:
            s = [r for r in ev if r["threshold"] == u and r["h"] == h]
            y = np.array([float(r["event"]) for r in s])
            ids, inv = np.unique([r["account"] for r in s], return_inverse=True)
            n_a = np.bincount(inv).astype(float)
            y_a = np.bincount(inv, weights=y)
            idx = rng.integers(0, len(ids), size=(RESAMPLES, len(ids)))
            for m in methods:
                p = np.array([float(r[f"p_{m}"]) for r in s])
                p_a = np.bincount(inv, weights=p)
                draw = (p_a[idx].sum(axis=1) - y_a[idx].sum(axis=1)) / n_a[idx].sum(axis=1)
                lo, hi = np.quantile(draw, [0.025, 0.975])
                res.append([u, h, m, len(s), len(ids), fmt(y.mean(), 5), fmt(p.mean(), 5),
                            fmt(p.mean() - y.mean(), 5), fmt(lo, 5), fmt(hi, 5),
                            fmt(p.mean() / y.mean(), 2)])
    write(out / "citl.csv", res)
    show(res)


def plot_reliability(rel, out):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    colors = {"V2": "#1baf7a", "CP-noKF": "#2a78d6", "B-EQp": "#eda100", "B-SRp": "#eb6834", "CLIM": "#8a8985"}
    marks = {"V2": "D", "CP-noKF": "o", "B-EQp": "^", "B-SRp": "s", "CLIM": "v"}
    fig, axes = plt.subplots(2, 3, figsize=(7.2, 4.8), sharex=True, sharey=True)
    for i, u in enumerate(("u0", "u10")):
        for j, h in enumerate(HS):
            ax = axes[i, j]
            ax.plot([0, 1], [0, 1], color="#8a8985", lw=0.8, ls="--")
            for m in colors:
                pts = [(float(r[5]), float(r[6]), int(r[4])) for r in rel[1:] if r[0] == u and r[1] == h and r[2] == m and int(r[4]) >= 20]
                if pts:
                    ax.plot([p[0] for p in pts], [p[1] for p in pts], color=colors[m], marker=marks[m],
                            ms=3.5, lw=1.2, label=m)
            ax.set_title(f"{'below 0' if u == 'u0' else 'below own 10th pct'}, {h} d", fontsize=8)
            ax.grid(True, color="#e4e3df", lw=0.6)
            for side in ("top", "right"):
                ax.spines[side].set_visible(False)
            if i == 1:
                ax.set_xlabel("forecast probability", fontsize=8)
            if j == 0:
                ax.set_ylabel("observed frequency", fontsize=8)
    handles, labels = axes[0, 0].get_legend_handles_labels()
    fig.legend(handles, labels, loc="lower center", ncol=5, frameon=False, fontsize=8)
    fig.suptitle("Overdraft-event reliability, development Berka (bins with at least 20 forecasts)", fontsize=9)
    fig.tight_layout(rect=(0, 0.06, 1, 1))
    fig.savefig(out / "reliability.pdf", metadata={"CreationDate": None, "ModDate": None})
    fig.savefig(out / "reliability.png", dpi=150)


# ----------------------------------------------------------------- S7 ----

def s7():
    out = OUT / "s7-bglobal"
    frozen = load(FROZEN, {"V2", "B-SA+", "B-EQ", "B-GLOBAL"})
    fresh = load([EXP / "bglobal/rows-cf.csv", EXP / "bglobal/rows-cut.csv"],
                 {"B-GLOBAL-cf", "B-GLOBAL-cut"})
    data = defaultdict(dict)
    for src in (frozen, fresh):
        for k, v in src.items():
            data[k].update(v)
    for label, extra in (("-cf", "B-GLOBAL-cf"), ("-cut", "B-GLOBAL-cut")):
        methods = ["V2", "B-SA+", "B-EQ", "B-GLOBAL", extra]
        pairs = [(m, "B-SA+") for m in methods if m != "B-SA+"] + [("V2", extra), ("V2", "B-GLOBAL"),
                                                                    (extra, "B-GLOBAL"), (extra, "B-EQ")]
        show(panel(data, methods, pairs, out, label=label)[1])


def s8():
    out = OUT / "s8-phase"
    # Check: offset 0 reproduces the frozen development rows.
    fz = load(FROZEN_DEV, {"V2", "B-EQ", "B-SA+"})
    z = load([B / "explore/phase/rows-offset0.csv"])
    cols = lambda r: {c: r[c] for c in r if c != "method"}  # noqa: E731
    keys = [(k, m) for k, v in z.items() for m in v]
    same = sum(fz.get(k, {}).get(m) is not None and cols(fz[k][m]) == cols(z[k][m]) for k, m in keys)
    write(out / "offset0-identity.csv", [["key", "value"], ["compared", len(keys)], ["identical", same]])
    print(f"offset 0 identity: {same}/{len(keys)}")
    for off in (0, 10, 20):
        data = load([B / f"explore/phase/rows-offset{off}.csv"])
        show(panel(data, ["V2", "B-EQ", "B-SA+"], [("V2", "B-SA+"), ("B-EQ", "B-SA+"), ("V2", "B-EQ")], out,
                   label=f"-offset{off}")[1])


if __name__ == "__main__":
    args = sys.argv[1:]
    if args and args[0] == "--heldout":
        FROZEN, EXP, OUT = FROZEN_HELDOUT, B / "explore-heldout", ROOT / "paper/results/exploratory-heldout"
        args = args[1:]
        assert set(a.lower() for a in args) <= {"s3", "s5", "s5c", "s6", "s7"}, "held-out: S3, S5, S5c, S6, S7 only"
    for a in args:
        globals()[a.lower()]()
