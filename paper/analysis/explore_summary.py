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

# ----------------------------------------------------------------- S9 ----

def s9():
    """The detector-ablation diagnostic (REVISION.md S9, round 3 A1)."""
    d = EXP / "detector"
    out = OUT / "s9-detector"
    cfgs = ["V0", "V0-nodet", "V2", "V2-nodet"]
    data = load([d / "rows.csv"], set(cfgs))
    state = {(r["account"], r["origin"], r["config"]): r for r in csv.DictReader(open(d / "state.csv"))}

    # Identity checks: CRPS strings must equal the stored rows.
    ident = [["check", "compared", "identical", "max_abs_diff"]]

    def check(name, ref_paths, ref_method, mine):
        ref = load(ref_paths, {ref_method})
        n = same = 0
        worst = 0.0
        for k, v in ref.items():
            if k in data and mine in data[k]:
                n += 1
                a, b = data[k][mine]["crps"], v[ref_method]["crps"]
                same += a == b
                worst = max(worst, abs(float(a) - float(b)))
        ident.append([name, n, same, f"{worst:.3g}"])

    check("V2 = stored S6 V2", [EXP / "berka/rows.csv"], "V2", "V2")
    check("V2-nodet = stored S6 V2-nodetector", [EXP / "berka/rows.csv"], "V2-nodetector", "V2-nodet")
    check("V0 = frozen development V0", [B / "variants-dev/rows.csv"], "V0", "V0")
    check("V0-nodet = frozen development abl:no-detector", [B / "berka-dev/rows.csv"], "abl:no-detector",
          "V0-nodet")
    # Branch activation and the tracker shared by V0 and V2.
    keys = {(a, o) for (a, o, _) in state}
    branch_diff = sum(state[(a, o, "V2")]["sparse_used"] != state[(a, o, "V2-nodet")]["sparse_used"]
                      for a, o in keys)
    v0_sparse = sum(state[(a, o, c)]["sparse_used"] == "1" for a, o in keys for c in ("V0", "V0-nodet"))
    track_diff = sum(any(state[(a, o, "V0" + s)][c] != state[(a, o, "V2" + s)][c]
                         for c in ("b", "c", "m", "alarms_income", "alarms_balance"))
                     for a, o in keys for s in ("", "-nodet"))
    v2_sparse = sum(state[(a, o, "V2")]["sparse_used"] == "1" for a, o in keys)
    ident += [["origins", len(keys), "", ""],
              ["V2 sparse branch used (origins)", len(keys), v2_sparse, ""],
              ["sparse branch differs, V2 vs V2-nodet", len(keys), len(keys) - branch_diff, ""],
              ["V0 or V0-nodet used the sparse branch", 2 * len(keys), v0_sparse, ""],
              ["tracker state differs, V0 vs V2 (same detector setting)", 2 * len(keys), 2 * len(keys) - track_diff, ""]]
    write(out / "identity.csv", ident)
    show(ident)

    def st(k, c="V2"):
        return state[(k[0], k[1], c)]

    strata = {
        "": lambda k, v: True,
        "-inc-alarm": lambda k, v: st(k)["income_detected"] == "1" and int(st(k)["alarms_income"]) > 0,
        "-inc-noalarm": lambda k, v: st(k)["income_detected"] == "1" and int(st(k)["alarms_income"]) == 0,
        "-noinc": lambda k, v: st(k)["income_detected"] == "0",
        "-recent-alarm": lambda k, v: v["V2"]["alarm"] == "1",
        "-no-recent-alarm": lambda k, v: v["V2"]["alarm"] == "0",
    }
    pairs = [("V2-nodet", "V2"), ("V0-nodet", "V0")]
    for label, keep in strata.items():
        panel(data, cfgs, pairs, out, keep=keep, label=label)

    # Bias and paired absolute CRPS difference, and the state by stratum.
    rows = [["stratum", "h", "forecasts", "accounts"] + [f"crps_{c}" for c in cfgs]
            + [f"bias_{c}" for c in cfgs]]
    srows = [["stratum", "origins", "median_abs_dm", "median_rel_m_err_V2", "median_rel_m_err_V2_nodet",
              "median_gated_V2", "median_gated_V2_nodet", "mean_dc_V0", "corr_dc_dm"]]
    for label, keep in strata.items():
        name = label.lstrip("-") or "all"
        for h in HS:
            sub = {k: v for k, v in common(data, cfgs, h).items() if keep(k, v)}
            if not sub:
                continue
            mean = lambda f: float(np.mean([f(v) for v in sub.values()]))  # noqa: E731
            rows.append([name, h, len(sub), len({k[0] for k in sub})]
                        + [fmt(mean(lambda v, c=c: float(v[c]["crps"]))) for c in cfgs]
                        + [fmt(mean(lambda v, c=c: float(v[c]["median"]) - float(v[c]["truth"]))) for c in cfgs])
        ks = [(a, o) for a, o in keys if keep((a, o, "90"), data.get((a, o, "90"), {"V2": {"alarm": "0"}}))]
        if not ks:
            continue
        m = lambda c: np.array([float(state[(a, o, c)]["m"]) for a, o in ks])  # noqa: E731
        last = np.array([float(state[(a, o, "V2")]["last_pay"]) for a, o in ks])
        dm = m("V2-nodet") - m("V2")
        dc = np.array([float(state[(a, o, "V0-nodet")]["c"]) - float(state[(a, o, "V0")]["c"]) for a, o in ks])
        ok = np.isfinite(last) & (np.abs(last) > 0)
        rel = lambda c: np.abs(m(c)[ok] - last[ok]) / np.abs(last[ok])  # noqa: E731
        g = lambda c: np.array([int(state[(a, o, c)]["income_gated_365"]) for a, o in ks])  # noqa: E731
        corr = np.corrcoef(dc, dm)[0, 1] if np.std(dc) > 0 and np.std(dm) > 0 else float("nan")
        srows.append([name, len(ks), fmt(float(np.median(np.abs(dm))), 1),
                      fmt(float(np.median(rel("V2"))), 3) if ok.any() else "",
                      fmt(float(np.median(rel("V2-nodet"))), 3) if ok.any() else "",
                      fmt(float(np.median(g("V2"))), 1), fmt(float(np.median(g("V2-nodet"))), 1),
                      fmt(float(np.mean(dc)), 3), fmt(corr, 3)])
    write(out / "crps-bias.csv", rows)
    write(out / "state.csv", srows)
    show(rows)
    show(srows)

    # The mechanism, in the stratum where the effect sits: income detected
    # and an income alarm before the origin.
    ia = [(a, o) for a, o in keys if state[(a, o, "V2")]["income_detected"] == "1"
          and int(state[(a, o, "V2")]["alarms_income"]) > 0]
    g = lambda c, x: np.array([float(state[(a, o, c)][x] or "nan") for a, o in ia])  # noqa: E731
    last, pay1 = g("V2", "last_pay"), g("V2", "pay1")
    first_age = g("V2", "first_income_alarm_age")
    last_before = g("V2", "last_income_alarm_before_origin")
    m_det, m_nod = g("V2", "m"), g("V2-nodet", "m")
    # V0's lumps per 30 settled days, against the income V0-nodet's m̂ misses
    # per pay cycle (last paycheck minus m̂).
    lump_rate = lambda c: g(c, "lumps_sum") / np.maximum(g(c, "lump_days"), 1) * 30  # noqa: E731
    missing = last - m_nod
    q = lambda v: [fmt(x, 3) for x in np.nanquantile(v, [0.25, 0.5, 0.75])]  # noqa: E731
    mech = [["key", "measure", "q25", "median", "q75"],
            ["first-alarm-age", "first income alarm, days after the first day of history", *q(first_age)],
            ["last-alarm-before", "last income alarm, days before the origin", *q(last_before)],
            ["share-first-alarm-180", "share of origins whose first income alarm fired within 180 days of the first day", fmt(
                float(np.mean(first_age <= 180)) if len(ia) else float("nan"), 3), "", ""],
            ["pay1-over-last", "first tagged paycheck / last paycheck", *q(pay1 / last)],
            ["m-det-over-last", "m-hat with the detector / last paycheck", *q(m_det / last)],
            ["m-nodet-over-last", "m-hat without the detector / last paycheck", *q(m_nod / last)],
            ["m-nodet-over-pay1", "m-hat without the detector / first paycheck", *q(m_nod / pay1)],
            ["lumps-v0nodet", "V0 lumps per 30 days / missing income (V0-nodet)", *q(lump_rate("V0-nodet") / missing)],
            ["lumps-v0", "V0 lumps per 30 days / missing income (V0)", *q(lump_rate("V0") / missing)],
            ["lumps-n-v0nodet", "V0-nodet lumps, count", *q(g("V0-nodet", "lumps_n"))],
            ["lumps-n-v0", "V0 lumps, count", *q(g("V0", "lumps_n"))]]
    write(out / "mechanism.csv", mech)
    show(mech)

# ---------------------------------------------------------------- S11 ----

def s11():
    """The schedule-replacement contrast (REVISION.md S11, round 3 B1)."""
    d = EXP / "schedule"
    out = OUT / "s11-schedule"
    ms = ["F0", "F0.5", "F1", "O"]
    data = load([d / "rows.csv"], set(ms))
    st = {(r["account"], r["origin"]): r for r in csv.DictReader(open(d / "state.csv"))}
    ident = [["check", "compared", "identical", "max_abs_diff"]]

    def check(name, paths, ref, mine):
        refd = load(paths, {ref})
        n = same = 0
        worst = 0.0
        for k, v in refd.items():
            if k in data and mine in data[k]:
                n += 1
                a, b = data[k][mine]["crps"], v[ref]["crps"]
                same += a == b
                worst = max(worst, abs(float(a) - float(b)))
        ident.append([name, n, same, f"{worst:.3g}"])

    check("F0 = frozen development B-EQ", [B / "berka-dev/rows.csv"], "B-EQ", "F0")
    check("F1 = stored S3 B-SR", [EXP / "berka/rows.csv"], "B-SR", "F1")
    write(out / "identity.csv", ident)
    show(ident)

    # Predictability and strata.
    def amount_err(r):
        return float(r["abs_err"]) / float(r["abs_real"]) if float(r["abs_real"]) > 0 else float("nan")

    def group(r):
        if r["income"] == "0" and r["bills"] == "0":
            return "no-schedule"
        if int(r["checkpoints"]) < 3 or not np.isfinite(amount_err(r)):
            return "short-history"
        return "rated"

    rated = np.array([amount_err(r) for r in st.values() if group(r) == "rated"])
    cut = np.quantile(rated, [1 / 3, 2 / 3])

    def stratum(r):
        g = group(r)
        if g != "rated":
            return g
        a = amount_err(r)
        return "predictable" if a <= cut[0] else ("middle" if a <= cut[1] else "unpredictable")

    lab = {k: stratum(r) for k, r in st.items()}
    write(out / "cutpoints.csv", [["quantile", "amount_error"], ["1/3", fmt(cut[0])], ["2/3", fmt(cut[1])]])
    desc = [["stratum", "origins", "accounts", "median_amount_error", "mean_date_error_days",
             "missed_rate", "extra_rate"]]
    for s in ("predictable", "middle", "unpredictable", "short-history", "no-schedule"):
        rs = [r for k, r in st.items() if lab[k] == s]
        if not rs:
            continue
        tot = lambda c: sum(float(r[c]) for r in rs)  # noqa: E731
        ae = np.array([amount_err(r) for r in rs])
        desc.append([s, len(rs), len({r["account"] for r in rs}),
                     fmt(float(np.nanmedian(ae)), 3) if np.isfinite(ae).any() else "",
                     fmt(tot("date_err_sum") / tot("matched"), 2) if tot("matched") else "",
                     fmt(tot("missed") / tot("projected"), 3) if tot("projected") else "",
                     fmt(tot("extra") / tot("realized"), 3) if tot("realized") else ""])
    write(out / "strata.csv", desc)
    show(desc)
    pairs = [("F1", "F0"), ("F0.5", "F0"), ("O", "F0"), ("O", "F1")]
    panel(data, ms, pairs, out)
    for s in ("predictable", "middle", "unpredictable", "short-history", "no-schedule"):
        _, c = panel(data, ms, pairs, out, keep=lambda k, v, s=s: lab.get((k[0], k[1])) == s, label=f"-{s}")
        print(s)
        show(c)

# ---------------------------------------------------------------- S12 ----

M64 = (1 << 64) - 1


def splitmix64(x):
    z = (x + 0x9E3779B97F4A7C15) & M64
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & M64
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & M64
    return z ^ (z >> 31)


def logit(p):
    p = np.clip(p, 1e-4, 1 - 1e-4)
    return np.log(p / (1 - p))


def platt_fit(p, y, iters=50):
    """Two-parameter logistic recalibration of logit(p), by Newton steps."""
    x = logit(p)
    a, b = 0.0, 1.0
    for _ in range(iters):
        q = 1 / (1 + np.exp(-np.clip(a + b * x, -500, 500)))
        w = q * (1 - q) + 1e-12
        g = np.array([np.sum(q - y), np.sum((q - y) * x)])
        hm = np.array([[np.sum(w), np.sum(w * x)], [np.sum(w * x), np.sum(w * x * x)]])
        step = np.linalg.solve(hm + 1e-9 * np.eye(2), g)
        a, b = a - step[0], b - step[1]
        if np.max(np.abs(step)) < 1e-10:
            break
    return a, b


def platt_apply(ab, p):
    return 1 / (1 + np.exp(-np.clip(ab[0] + ab[1] * logit(p), -500, 500)))


def isotonic_fit(p, y):
    """Pool-adjacent-violators on p sorted ascending: (thresholds, values)."""
    o = np.argsort(p, kind="mergesort")
    xs, ys = p[o], y[o].astype(float)
    vals, wts, ends = [], [], []
    for x, v in zip(xs, ys):
        vals.append(v); wts.append(1.0); ends.append(x)
        while len(vals) > 1 and vals[-2] > vals[-1]:
            w = wts[-2] + wts[-1]
            v2 = (vals[-2] * wts[-2] + vals[-1] * wts[-1]) / w
            vals[-2:] = [v2]; wts[-2:] = [w]; ends[-2:] = [ends[-1]]
    return np.array(ends), np.array(vals)


def isotonic_apply(fit, p):
    ends, vals = fit
    i = np.searchsorted(ends, p, side="left")
    return vals[np.clip(i, 0, len(vals) - 1)]


def murphy(p, y, bins=10):
    """Brier = REL - RES + UNC + W, with equal-width bins."""
    n = len(p)
    bs = float(np.mean((p - y) ** 2))
    ob = float(np.mean(y))
    k = np.minimum((p * bins).astype(int), bins - 1)
    rel = res = 0.0
    for j in range(bins):
        s = k == j
        if s.any():
            rel += s.sum() * (p[s].mean() - y[s].mean()) ** 2
            res += s.sum() * (y[s].mean() - ob) ** 2
    rel, res, unc = rel / n, res / n, ob * (1 - ob)
    return bs, rel, res, unc, bs - (rel - res + unc)


def bss_boot(pm, pr, y, acct, rng):
    """Brier skill of pm against pr, pooled, with an account bootstrap."""
    ids, inv = np.unique(acct, return_inverse=True)
    a = np.bincount(inv, weights=(pm - y) ** 2, minlength=len(ids))
    r = np.bincount(inv, weights=(pr - y) ** 2, minlength=len(ids))
    idx = rng.integers(0, len(ids), size=(RESAMPLES, len(ids)))
    den = r[idx].sum(axis=1)
    draw = np.where(den > 0, 1 - a[idx].sum(axis=1) / np.where(den > 0, den, 1), np.nan)
    lo, hi = np.nanquantile(draw, [0.025, 0.975])
    return (1 - a.sum() / r.sum()) if r.sum() > 0 else float("nan"), lo, hi


def s12():
    """The shortfall audit (REVISION.md S12, round 3 B2)."""
    d = EXP / "shortfall"
    out = OUT / "s12-shortfall"
    ev = list(csv.DictReader(open(d / "events.csv")))
    methods = ["V2", "CP-noKF", "B-EQp", "B-SRp", "CLIM", "CLIM-D"]
    # Identity: u0 and u10 equal the stored S5 events.
    old = {(r["account"], r["origin"], r["h"], r["threshold"]): r
           for r in csv.DictReader(open(EXP / "berka/events.csv"))}
    n = same = 0
    worst = 0.0
    for r in ev:
        o = old.get((r["account"], r["origin"], r["h"], r["threshold"]))
        if o is None:
            continue
        n += 1
        diffs = [abs(float(r[f"p_{m}"]) - float(o[f"p_{m}"])) for m in methods[:5]]
        diffs.append(abs(float(r["event"]) - float(o["event"])))
        worst = max(worst, max(diffs))
        same += max(diffs) == 0.0
    ident = [["check", "compared", "identical", "max_abs_diff"],
             ["u0 and u10 = stored S5 events", n, same, f"{worst:.3g}"]]
    write(out / "identity.csv", ident)
    show(ident)
    rng = np.random.default_rng(SEED)

    def arr(rows, c):
        return np.array([float(r[c]) for r in rows])

    def score_block(rows, label, subset):
        y = arr(rows, "event")
        acct = np.array([r["account"] for r in rows])
        out_rows = []
        for m in methods + [x for x in ("V2-platt", "V2-iso") if f"p_{x}" in rows[0]]:
            p = arr(rows, f"p_{m}")
            bss, lo, hi = bss_boot(p, arr(rows, "p_CLIM"), y, acct, rng)
            bs, rel, res, unc, w = murphy(p, y)
            out_rows.append([label, subset, m, len(rows), len(set(acct)), fmt(y.mean(), 5), fmt(p.mean(), 5),
                             fmt(bs, 5), fmt(bss), fmt(lo), fmt(hi), fmt(rel, 5), fmt(res, 5),
                             fmt(unc, 5), fmt(w, 5)])
        return out_rows

    head = ["block", "subset", "method", "origins", "accounts", "prevalence", "mean_p", "brier",
            "bss_vs_clim", "bss_lo", "bss_hi", "rel", "res", "unc", "within"]

    # Item 6: recalibrate V2 (u0 and u10), cross-fitted over 5 account folds.
    fold = {a: splitmix64(int(a)) % 5 for a in {r["account"] for r in ev}}
    fits = [["threshold", "h", "platt_a", "platt_b"]]
    for u in ("u0", "u10"):
        for h in HS:
            rows = [r for r in ev if r["threshold"] == u and r["h"] == h]
            p, y = arr(rows, "p_V2"), arr(rows, "event")
            f = np.array([fold[r["account"]] for r in rows])
            pp, pi = np.zeros_like(p), np.zeros_like(p)
            for k in range(5):
                tr, te = f != k, f == k
                pp[te] = platt_apply(platt_fit(p[tr], y[tr]), p[te])
                pi[te] = isotonic_apply(isotonic_fit(p[tr], y[tr]), p[te])
            for r, a, b in zip(rows, pp, pi):
                r["p_V2-platt"], r["p_V2-iso"] = str(a), str(b)
            ab = platt_fit(p, y)
            fits.append([u, h, fmt(ab[0]), fmt(ab[1])])
    write(out / "recalibration-fit.csv", fits)

    res = [head]
    counts = [["threshold", "h", "origins", "start_below", "events", "events_day0", "events_reported",
               "flips_0to1", "flips_1to0", "events_start_at_or_above"]]
    for u in ("u0", "u10", "m100", "p100", "m1000", "p1000"):
        for h in HS:
            rows = [r for r in ev if r["threshold"] == u and r["h"] == h]
            e, e0, er = arr(rows, "event"), arr(rows, "event_day0"), arr(rows, "event_reported")
            sb = arr(rows, "start_below")
            counts.append([u, h, len(rows), int(sb.sum()), int(e.sum()), int(e0.sum()), int(er.sum()),
                           int(((e == 0) & (er == 1)).sum()), int(((e == 1) & (er == 0)).sum()),
                           int(e[sb == 0].sum())])
            if u in ("u0", "u10"):
                res += score_block(rows, f"{u}/{h}", "all")
                above = [r for r in rows if r["start_below"] == "0"]
                below = [r for r in rows if r["start_below"] == "1"]
                res += score_block(above, f"{u}/{h}", "start at or above")
                if len(below) >= 20 and len({r["event"] for r in below}) > 1:
                    res += score_block(below, f"{u}/{h}", "start below")
            elif u in ("m100", "p100", "m1000", "p1000"):
                res += score_block(rows, f"{u}/{h}", "all")
    write(out / "counts.csv", counts)
    show(counts)

    # Item 3: shallow events at u0.
    by = {(r["account"], r["origin"], r["h"], r["threshold"]): r for r in ev}
    shallow = [["h", "delta_czk", "u0_events", "share_min_above_minus_delta"]]
    for h in HS:
        u0 = [r for r in ev if r["threshold"] == "u0" and r["h"] == h and r["event"] == "1"]
        for dn, dl in (("m100", 100), ("m1000", 1000)):
            sh = sum(float(r["min_path"]) >= float(by[(r["account"], r["origin"], h, dn)]["u"]) for r in u0)
            shallow.append([h, dl, len(u0), fmt(sh / len(u0), 3) if u0 else ""])
    write(out / "shallow.csv", shallow)
    show(shallow)

    # Item 4: concentration.
    accts = list(csv.DictReader(open(d / "accounts.csv")))
    ever = sum(int(a["episodes"]) > 0 for a in accts)
    u0_90 = [r for r in ev if r["threshold"] == "u0" and r["h"] == "90"]
    per = defaultdict(int)
    for r in u0_90:
        per[r["account"]] += int(r["event"])
    ranked = sorted(per.items(), key=lambda kv: -kv[1])
    tot = sum(per.values())
    top5 = {a for a, _ in ranked[:5]}
    top1 = {a for a, _ in ranked[:max(1, int(np.ceil(0.01 * len(ranked))))]}
    conc = [["key", "value"], ["accounts", len(accts)], ["episodes", sum(int(a["episodes"]) for a in accts)],
            ["accounts_ever_below_zero", ever],
            ["accounts_with_a_u0_event_at_90", sum(v > 0 for v in per.values())],
            ["u0_events_at_90", tot], ["share_from_top5", fmt(sum(per[a] for a in top5) / tot, 3) if tot else ""],
            ["top1pct_accounts", len(top1)],
            ["share_from_top1pct", fmt(sum(per[a] for a in top1) / tot, 3) if tot else ""]]
    write(out / "concentration.csv", conc)
    show(conc)
    for name, drop in (("without top 5", top5), ("without top 1%", top1)):
        for h in HS:
            rows = [r for r in ev if r["threshold"] == "u0" and r["h"] == h and r["account"] not in drop]
            res += score_block(rows, f"u0/{h}", name)
    write(out / "scores.csv", res)
    show([r for r in res if r[2] in ("V2", "V2-platt", "B-EQp", "CLIM", "CLIM-D")][:60])

# ---------------------------------------------------------------- S13 ----

def s13():
    """The paycheck channel without the detector (REVISION.md S13)."""
    d = EXP / "income-trace"
    out = OUT / "s13-income"
    res = [["engine", "group", "config", "accounts", "pay_first_over_last", "gated_first12", "never_close_share",
            "median_k_last12", "final_s_over_pay2", "final_m_over_pay"]]
    for engine, fname in (("before fix", "trace-prefix.csv"), ("after fix", "trace.csv")):
        p = d / fname
        if not p.exists():
            continue
        by = defaultdict(dict)
        for r in csv.DictReader(open(p)):
            by[r["account"]][r["config"]] = r
        groups = {"income alarm": [], "no income alarm": []}
        for v in by.values():
            if "det" in v and "nodet" in v:
                groups["income alarm" if int(v["det"]["alarms_income"]) > 0 else "no income alarm"].append(v)
        for g, L in groups.items():
            for c in ("det", "nodet"):
                f = lambda x: np.array([float(v[c][x]) if v[c][x] else np.nan for v in L])  # noqa: E731
                res.append([engine, g, c, len(L), fmt(float(np.nanmedian(f("pay_first") / f("pay_last"))), 3),
                            fmt(float(np.nanmedian(f("gated_first12"))), 1),
                            fmt(float(np.mean(np.isnan(f("first_close")))), 3),
                            fmt(float(np.nanmedian(f("median_k_last12"))), 4),
                            fmt(float(np.nanmedian(f("final_s_over_pay2"))), 3),
                            fmt(float(np.nanmedian(f("final_m_over_pay"))), 3)])
    write(out / "trace.csv", res)
    show(res)
    # V0's smaller bias: the change the detector's removal makes, by model.
    cb = {(r["stratum"], r["h"]): r for r in csv.DictReader(open(OUT / "s9-detector/crps-bias.csv"))}
    r = cb[("inc-alarm", "90")]
    dv0 = float(r["bias_V0-nodet"]) - float(r["bias_V0"])
    dv2 = float(r["bias_V2-nodet"]) - float(r["bias_V2"])
    v0 = [["key", "value"], ["bias_V0", r["bias_V0"]], ["bias_V0_nodet", r["bias_V0-nodet"]],
          ["bias_V2", r["bias_V2"]], ["bias_V2_nodet", r["bias_V2-nodet"]],
          ["shift_V0", fmt(dv0, 1)], ["shift_V2", fmt(dv2, 1)]]
    write(out / "v0-bias.csv", v0)
    show(v0)


# ---------------------------------------------------------------- S14 ----

def s14():
    """Within-window dependence (REVISION.md S14)."""
    d = EXP / "schedule"
    out = OUT / "s14-dependence"
    ms = ["F0", "F1", "O", "EQ-shuf", "EQ-ident", "SR-match", "O-match"]
    data = load([d / "rows.csv"], set(ms))
    # The F and O rows must equal S11's (the previous run, kept aside).
    prev = load([EXP / "schedule-s11/rows.csv"], {"F0", "F0.5", "F1", "O"})
    n = same = 0
    for k, v in prev.items():
        for m in v:
            if k in data and m in data[k]:
                n += 1
                same += data[k][m]["crps"] == v[m]["crps"]
    worst = max((abs(float(v["EQ-ident"]["crps"]) - float(v["F0"]["crps"])) for v in data.values()
                 if "EQ-ident" in v and "F0" in v), default=float("nan"))
    ident = [["check", "compared", "identical", "max_abs_diff"],
             ["F and O rows = S11 run", n, same, ""],
             ["EQ-ident (b) = F0 (B-EQ)", len(data), "", f"{worst:.3g}"]]
    write(out / "identity.csv", ident)
    show(ident)
    pairs = [("EQ-shuf", "F0"), ("SR-match", "F1"), ("SR-match", "F0"), ("O-match", "O"), ("O-match", "F0"),
             ("F1", "F0"), ("O", "F0")]
    _, c = panel(data, ms, pairs, out)
    show(c)
    st = {(r["account"], r["origin"]): r for r in csv.DictReader(open(d / "state.csv"))}
    cut = [float(r["amount_error"]) for r in csv.DictReader(open(OUT / "s11-schedule/cutpoints.csv"))]

    def tert(k):
        r = st[(k[0], k[1])]
        a = float(r["abs_err"]) / float(r["abs_real"]) if float(r["abs_real"]) > 0 else float("nan")
        if not np.isfinite(a) or int(r["checkpoints"]) < 3:
            return "other"
        return "predictable" if a <= cut[0] else ("middle" if a <= cut[1] else "unpredictable")

    for s in ("predictable", "middle", "unpredictable"):
        panel(data, ms, pairs, out, keep=lambda k, v, s=s: tert(k) == s, label=f"-{s}")

# ------------------------------------------------------------ S5 split ----

def s5split():
    """Round 4 item 4: the S5 events split by start state (stored events and
    start balances only; no new forecasts)."""
    out = OUT / "s5-overdraft"
    ev = list(csv.DictReader(open(EXP / "berka/events.csv")))
    b0 = {(r["account"], r["origin"]): float(r["b0"]) for r in csv.DictReader(open(EXP / "startbal/startbal.csv"))}
    methods = ["V2", "CP-noKF", "B-EQp", "B-SRp", "CLIM"]
    rng = np.random.default_rng(SEED)
    head = ["threshold", "h", "subset", "method", "origins", "accounts", "events", "prevalence", "mean_p",
            "citl_diff", "citl_lo", "citl_hi", "brier", "bss_vs_clim", "bss_lo", "bss_hi"]
    res = [head]
    missing = 0
    for u in ("u0", "u10"):
        for h in HS:
            rows = [r for r in ev if r["threshold"] == u and r["h"] == h]
            for r in rows:
                k = (r["account"], r["origin"])
                if k not in b0:
                    missing += 1
                r["start_below"] = "1" if b0.get(k, float("inf")) < float(r["u"]) else "0"
            for subset, keep in (("all", lambda r: True), ("new crossings", lambda r: r["start_below"] == "0"),
                                 ("existing negatives", lambda r: r["start_below"] == "1")):
                sub = [r for r in rows if keep(r)]
                if not sub:
                    continue
                y = np.array([float(r["event"]) for r in sub])
                acct = np.array([r["account"] for r in sub])
                ids, inv = np.unique(acct, return_inverse=True)
                idx = rng.integers(0, len(ids), size=(RESAMPLES, len(ids)))
                n_a = np.bincount(inv).astype(float)
                y_a = np.bincount(inv, weights=y)
                pc = np.array([float(r["p_CLIM"]) for r in sub])
                for m in methods:
                    p = np.array([float(r[f"p_{m}"]) for r in sub])
                    p_a = np.bincount(inv, weights=p)
                    draw = (p_a[idx].sum(axis=1) - y_a[idx].sum(axis=1)) / n_a[idx].sum(axis=1)
                    clo, chi = np.quantile(draw, [0.025, 0.975])
                    bss, lo, hi = bss_boot(p, pc, y, acct, rng)
                    res.append([u, h, subset, m, len(sub), len(ids), int(y.sum()), fmt(y.mean(), 5), fmt(p.mean(), 5),
                                fmt(p.mean() - y.mean(), 5), fmt(clo, 5), fmt(chi, 5),
                                fmt(float(np.mean((p - y) ** 2)), 5), fmt(bss), fmt(lo), fmt(hi)])
    write(out / "split.csv", res)
    print("origins without a start balance:", missing)
    show([r for r in res if r[3] in ("V2", "B-EQp", "CLIM") and r[0] == "u0"])

# ---------------------------------------------------------------- S15 ----

def s15():
    """The forward-period confirmatory test (REVISION.md S15). Held-out only:
    run with --heldout after the single guarded run."""
    d = B / "confirmatory-s15"
    out = ROOT / "paper/results/confirmatory-s15"
    cut = (0.1428, 0.4127)
    ms = ["F0", "F1", "O", "EQ-shuf"]
    data = load([d / "rows.csv"], set(ms))
    st = {}
    for r in csv.DictReader(open(d / "state.csv")):
        a = float(r["abs_err"]) / float(r["abs_real"]) if float(r["abs_real"]) > 0 else float("nan")
        ok = np.isfinite(a) and int(r["checkpoints"]) >= 3
        st[(r["account"], r["origin"])] = ("P" if a <= cut[0] else ("M" if a <= cut[1] else "U")) if ok else "X"
    rng = np.random.default_rng(SEED)
    res = [["prediction", "comparison", "h", "stratum", "origins", "accounts", "skill_mean", "skill_mean_lo",
            "skill_mean_hi", "skill_pooled", "skill_pooled_lo", "skill_pooled_hi", "predicted_sign", "verdict"]]

    def sums(h, m, ref, keep):
        num, den = defaultdict(float), defaultdict(float)
        n = 0
        for k, v in data.items():
            if k[2] == h and m in v and ref in v and keep(k):
                num[k[0]] += float(v[m]["crps"])
                den[k[0]] += float(v[ref]["crps"])
                n += 1
        return num, den, n

    def row(pred, m, ref, h, label, keep, sign):
        num, den, n = sums(h, m, ref, keep)
        a = sorted(num)
        r = boot_ratio(np.array([num[x] for x in a]), np.array([den[x] for x in a]), rng)
        lo, hi = r[1], r[2]
        ok = hi < 0 if sign < 0 else lo > 0
        res.append([pred, f"{m} vs {ref}", h, label, n, len(a), *(fmt(x) for x in r),
                    "<0" if sign < 0 else ">0", "confirmed" if ok else "not confirmed"])

    row("P2", "O", "F0", "90", "all", lambda k: True, -1)
    for h in ("30", "90"):
        row("P3", "EQ-shuf", "F0", h, "all", lambda k: True, -1)
    # P3b: s_shuf - s_SR/4 at 90 days, accounts resampled jointly.
    ns, ds_, _ = sums("90", "EQ-shuf", "F0", lambda k: True)
    nr, dr, n90 = sums("90", "F1", "F0", lambda k: True)
    accts = sorted(set(ds_) & set(dr))
    a_s = np.array([ns[a] for a in accts]); b_s = np.array([ds_[a] for a in accts])
    a_r = np.array([nr[a] for a in accts]); b_r = np.array([dr[a] for a in accts])
    idx = rng.integers(0, len(accts), size=(RESAMPLES, len(accts)))
    def stat(sel, pooled):
        if pooled:
            ss = 1 - a_s[sel].sum(axis=-1) / b_s[sel].sum(axis=-1)
            sr = 1 - a_r[sel].sum(axis=-1) / b_r[sel].sum(axis=-1)
        else:
            ss = (1 - a_s[sel] / b_s[sel]).mean(axis=-1)
            sr = (1 - a_r[sel] / b_r[sel]).mean(axis=-1)
        return ss - sr / 4, sr
    all_ = np.arange(len(accts))
    pm, srm = stat(all_, False)
    pp, _ = stat(all_, True)
    qm = np.quantile(stat(idx, False)[0], [0.025, 0.975])
    qp = np.quantile(stat(idx, True)[0], [0.025, 0.975])
    ok = qm[0] > 0 and srm < 0
    res.append(["P3b", "EQ-shuf vs F0 minus 1/4 of F1 vs F0", "90", "all", n90, len(accts), fmt(pm), fmt(qm[0]),
                fmt(qm[1]), fmt(pp), fmt(qp[0]), fmt(qp[1]), ">0", "confirmed" if ok else "not confirmed"])
    # P1: the difference between tertiles, accounts resampled jointly.
    for h in ("30", "90"):
        tert = {}
        for t in "PU":
            num, den, n = sums(h, "F1", "F0", lambda k, t=t: st.get((k[0], k[1])) == t)
            tert[t] = (num, den, n)
        accts = sorted(set(tert["P"][0]) | set(tert["U"][0]))
        idx = rng.integers(0, len(accts), size=(RESAMPLES, len(accts)))
        est = {}
        for t in "PU":
            num = np.array([tert[t][0].get(a, 0.0) for a in accts])
            den = np.array([tert[t][1].get(a, 0.0) for a in accts])
            has = den > 0
            per = np.where(has, 1 - num / np.where(has, den, 1), np.nan)
            est[t] = (np.nanmean(per), 1 - num.sum() / den.sum(), np.nanmean(per[idx], axis=1),
                      1 - num[idx].sum(axis=1) / den[idx].sum(axis=1))
        dm = est["P"][0] - est["U"][0]
        dp = est["P"][1] - est["U"][1]
        qm = np.nanquantile(est["P"][2] - est["U"][2], [0.025, 0.975])
        qp = np.nanquantile(est["P"][3] - est["U"][3], [0.025, 0.975])
        res.append(["P1 (secondary)", "F1 vs F0, predictable minus unpredictable", h, "P minus U",
                    tert["P"][2] + tert["U"][2], len(accts), fmt(dm), fmt(qm[0]), fmt(qm[1]), fmt(dp), fmt(qp[0]),
                    fmt(qp[1]), ">0", "confirmed" if qm[0] > 0 else "not confirmed"])
    write(out / "predictions.csv", res)
    show(res)
    panel(data, ms, [("F1", "F0"), ("O", "F0"), ("EQ-shuf", "F0")], out)

# ---------------------------------------------------------------- S16 ----

def s16():
    """Does subtracting S_hist widen the sampled changes? (REVISION.md S16)"""
    out = OUT / "s16-dispersion"
    w = defaultdict(dict)
    for r in csv.DictReader(open(EXP / "schedule-s11/rows.csv")):
        if r["method"] in ("F0", "F1") and r["h"] in ("30", "90"):
            w[(r["account"], r["origin"], r["h"])][r["method"]] = float(r["width90"])
    res = [["h", "origins", "accounts", "share_origins_wider", "share_accounts_wider",
            "ratio_q25", "ratio_median", "ratio_q75"]]
    for h in ("30", "90"):
        per = defaultdict(list)
        wider = n = 0
        for (a, o, hh), v in w.items():
            if hh != h or "F0" not in v or "F1" not in v or v["F0"] <= 0:
                continue
            ratio = v["F1"] / v["F0"]
            per[a].append(ratio)
            wider += ratio > 1
            n += 1
        med = np.array([np.median(x) for x in per.values()])
        q = np.quantile(med, [0.25, 0.5, 0.75])
        res.append([h, n, len(med), fmt(wider / n), fmt(float(np.mean(med > 1))), *(fmt(x, 3) for x in q)])
    write(out / "dispersion.csv", res)
    show(res)

# ---------------------------------------------------------------- S17 ----

def s17():
    """Round 6 analyses from stored outputs (REVISION.md S17)."""
    held = EXP.name == "explore-heldout"
    out = OUT / "s17-stored"
    rng = np.random.default_rng(SEED)
    cut = (0.1428, 0.4127)
    sdir = (B / "confirmatory-s15") if held else (EXP / "schedule-s11")
    data = load([sdir / "rows.csv"], {"F0", "F1", "O"})
    st = {(r["account"], r["origin"]): r for r in csv.DictReader(open(sdir / "state.csv"))}

    def tert(r):
        a = float(r["abs_err"]) / float(r["abs_real"]) if float(r["abs_real"]) > 0 else float("nan")
        if not np.isfinite(a) or int(r["checkpoints"]) < 3:
            return "unrated"
        return "predictable" if a <= cut[0] else ("middle" if a <= cut[1] else "unpredictable")

    lab = {k: tert(r) for k, r in st.items()}
    comp = [["tertile", "origins", "no_schedule", "share_income_detected"]]
    for s in ("predictable", "middle", "unpredictable", "unrated"):
        ks = [k for k in st if lab[k] == s]
        if ks:
            comp.append([s, len(ks), sum(st[k]["income"] == "0" and st[k]["bills"] == "0" for k in ks),
                         fmt(float(np.mean([st[k]["income"] == "1" for k in ks])))])
    write(out / "tertile-composition.csv", comp)
    show(comp)
    res = [["comparison", "subset", "h"] + CMP_HEAD[2:]]
    subsets = [("all", lambda k: True)] + [
        (f"{s}", lambda k, s=s: lab.get((k[0], k[1])) == s) for s in ("predictable", "middle", "unpredictable")] + [
        (f"{s}, income detected", lambda k, s=s: lab.get((k[0], k[1])) == s and st[(k[0], k[1])]["income"] == "1")
        for s in ("predictable", "unpredictable")]
    for (m, ref) in (("F1", "F0"), ("O", "F1"), ("O", "F0")):
        for name, keep in subsets:
            if (m, ref) != ("F1", "F0") and name != "all":
                continue
            for h in ("30", "90"):
                sub = {k: v for k, v in data.items() if k[2] == h and m in v and ref in v and keep(k)}
                if not sub:
                    continue
                r = compare(sub, m, ref, rng)
                res.append([f"{m} vs {ref}", name, h, len(sub), len({k[0] for k in sub})] + [fmt(x) for x in r])
    write(out / "contrasts.csv", res)
    show(res)

    if held:
        # New crossings: reliability bins, and the accounts and crossing
        # clusters behind the positive 90-day windows.
        ev = list(csv.DictReader(open(EXP / "berka/events.csv")))
        b0 = {(r["account"], r["origin"]): float(r["b0"]) for r in csv.DictReader(open(EXP / "startbal/startbal.csv"))}
        nc = [r for r in ev if r["threshold"] == "u0" and b0[(r["account"], r["origin"])] >= 0.0]
        rel = [["h", "method", "bin", "count", "mean_p", "observed"]]
        for h in HS:
            rows = [r for r in nc if r["h"] == h]
            y = np.array([float(r["event"]) for r in rows])
            for m in ("V2", "CP-noKF", "B-EQp", "B-SRp", "CLIM"):
                p = np.array([float(r[f"p_{m}"]) for r in rows])
                bins = np.minimum((p * 10).astype(int), 9)
                for k in range(10):
                    s = bins == k
                    if s.any():
                        rel.append([h, m, k, int(s.sum()), fmt(float(p[s].mean())), fmt(float(y[s].mean()))])
        write(out / "newcross-reliability.csv", rel)
        pos = defaultdict(list)
        for r in nc:
            if r["h"] == "90" and r["event"] == "1":
                pos[r["account"]].append(r["origin"])
        clusters = 0
        import datetime as dt
        for a, os_ in pos.items():
            ds = sorted(dt.date.fromisoformat(o) for o in os_)
            clusters += 1 + sum((ds[i] - ds[i - 1]).days > 30 for i in range(1, len(ds)))
        conc = [["key", "value"], ["positive_windows_90", sum(len(v) for v in pos.values())],
                ["accounts_90", len(pos)], ["crossing_clusters_90", clusters]]
        write(out / "newcross-concentration.csv", conc)
        show(conc)
    else:
        # Identity of the rerun shortfall events with S12, and new crossings
        # scored with CLIM-D and CLIM+.
        new = list(csv.DictReader(open(EXP / "shortfall/events.csv")))
        old = {(r["account"], r["origin"], r["h"], r["threshold"]): r
               for r in csv.DictReader(open(EXP / "shortfall-s12/events.csv"))}
        cols = [c for c in old[next(iter(old))] if c not in ("account", "origin", "h", "threshold")]
        same = sum(all(r[c] == old[(r["account"], r["origin"], r["h"], r["threshold"])][c] for c in cols) for r in new)
        write(out / "identity.csv", [["check", "compared", "identical"],
                                     ["shortfall rerun = S12 events (all S12 columns)", len(new), same]])
        print("identity", same, "of", len(new))
        sc = [["h", "method", "origins", "accounts", "prevalence", "mean_p", "brier", "bss_vs_clim", "bss_lo", "bss_hi"]]
        for h in HS:
            rows = [r for r in new if r["threshold"] == "u0" and r["h"] == h and r["start_below"] == "0"]
            y = np.array([float(r["event"]) for r in rows])
            acct = np.array([r["account"] for r in rows])
            pc = np.array([float(r["p_CLIM"]) for r in rows])
            for m in ("V2", "B-EQp", "CLIM", "CLIM-D", "CLIM+"):
                p = np.array([float(r[f"p_{m}"]) for r in rows])
                bss, lo, hi = bss_boot(p, pc, y, acct, rng)
                sc.append([h, m, len(rows), len(set(acct)), fmt(y.mean(), 5), fmt(p.mean(), 5),
                           fmt(float(np.mean((p - y) ** 2)), 5), fmt(bss), fmt(lo), fmt(hi)])
        write(out / "newcross-climatologies.csv", sc)
        show(sc)


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
        assert set(a.lower() for a in args) <= {"s3", "s5", "s5c", "s5split", "s6", "s7", "s15", "s17"}, "held-out: S3, S5, S5c, S6, S7 only"
    for a in args:
        globals()[a.lower()]()
