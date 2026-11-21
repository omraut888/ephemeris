#!/usr/bin/env python3
"""Every number the paper quotes, as LaTeX, generated (paper/PLAN.md §6).

    paper/.venv/bin/python -I paper/analysis/macros.py

Reads the committed results (paper/results/**.csv) and the protocol's
constants from the Rust and Python sources, and writes:
- paper/results/numbers.tex: one value per key, read in the text as
  `\\R{key}`. An unknown key stops the build (see main.tex).
- paper/results/tables/*.tex: every table in the paper.
- paper/build/numbers-keys.txt: the key list, for writing (not committed).

Keys are `tag/row.../column`, built from each file's key columns (slugged:
lower case, spaces to `-`, `<` to `lt`, `>=` to `ge`). Formats by column:
coverage and shares in percent with one decimal; skills signed with two
decimals (also `@3` for three); differences of coverage in signed points;
CRPS and counts as integers with thousands separators.
"""
import csv
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[2]
RES = ROOT / "paper/results"
TAB = RES / "tables"
VALUES = {}


# ------------------------------------------------------------ formatting --

def minus(s):
    return s.replace("-", "\\textminus{}") if s.startswith("-") else s


def pct(x, d=1):
    return f"{100 * float(x):.{d}f}"


def signed(x, d=2):
    v = float(x)
    s = f"{v:+.{d}f}"
    if s in (f"+{0:.{d}f}", f"-{0:.{d}f}"):
        s = f"{0:.{d}f}"
    return minus(s)


def points(x, d=1):
    return signed(100 * float(x), d)


def integer(x):
    return f"{round(float(x)):,}".replace(",", "{,}")


def slug(s):
    if re.fullmatch(r"-\d+", s.strip()):
        return "m" + s.strip()[1:]
    s = s.replace(">=", " ge ").replace("<", " lt ").replace("≥", " ge ").replace(":", " ")
    s = re.sub(r"[^A-Za-z0-9.+]+", "-", s.strip()).strip("-")
    return s.lower() if not re.match(r"^[VB]", s) else s


def fmt(col, v):
    """The text form of a cell, chosen by its column name."""
    if v == "" or v is None:
        return None
    try:
        float(v)
    except ValueError:
        return v
    c = col
    if c.endswith("_diff") or c.endswith("_diff_lo") or c.endswith("_diff_hi"):
        return points(v)
    if c.startswith("miss") or c.startswith("prevalence") or c.startswith("share") or c == "values_moved_share":
        return pct(v)
    if c.startswith("brier"):
        return f"{float(v):.3f}"
    if c.startswith("bss"):
        return signed(v)
    if c.startswith("cov") or c.startswith("coverage") or c.endswith("share") \
            or c.startswith("recovered") or c in ("pit_outer_share",):
        return pct(v)
    if "skill" in c or c in ("mean_of_ratios", "pooled", "mean_of_ratios_trim10", "worst_account"):
        return signed(v)
    if "crps" in c or "width" in c or "mae" in c or c in ("score",):
        return integer(v)
    if c in ("forecasts", "accounts", "cases", "feeds_identical", "reports_identical",
             "inside", "forecasts_changed", "coverage_flips", "seeds"):
        return integer(v)
    return v


def put(key, value):
    if value is None:
        return
    VALUES[key] = value


def table(tag, path, keys):
    rows = list(csv.DictReader(open(path)))
    for r in rows:
        base = "/".join([tag] + [slug(r[k]) for k in keys])
        for c, v in r.items():
            if c in keys:
                continue
            put(f"{base}/{c}", fmt(c, v))
            if "skill" in c and v not in ("", None):
                put(f"{base}/{c}@3", signed(v, 3))
    return rows


def keyvalue(tag, path):
    out = {}
    for r in csv.DictReader(open(path)):
        out[r["key"]] = r["value"]
        v = r["value"]
        try:
            f = float(v)
            if f == int(f) and abs(f) >= 1000 and not r["key"].endswith(("year", "seed")):
                v = integer(f)
        except ValueError:
            pass
        put(f"{tag}/{r['key']}", v.replace("_", "\\_") if isinstance(v, str) else v)
    return out


# ------------------------------------------------------------- constants --

def rust_consts():
    out = {}
    for f in (ROOT / "crates/ephemeris-eval/src").glob("*.rs"):
        for m in re.finditer(r"pub const ([A-Z_0-9]+): [^=]+= ([^;]+);", f.read_text()):
            out[f"{f.stem}/{m.group(1)}"] = m.group(2).strip()
    split = (ROOT / "crates/ephemeris-eval/src/split.rs").read_text()
    out["split/DEV_PERCENT"] = re.search(r"% 100 < (\d+)", split).group(1)
    lgbm = (ROOT / "paper/baselines/lgbm.py").read_text()
    out["lgbm/ROUNDS"] = re.search(r"^ROUNDS = (\d+)", lgbm, re.M).group(1)
    return out


def constants():
    c = rust_consts()
    num = lambda s: s.replace("_", "")  # noqa: E731
    put("c/bootstrap", integer(num(c["aggregate/BOOTSTRAP"])))
    put("c/bootstrap-seed", c["aggregate/BOOTSTRAP_SEED"])
    put("c/paths", integer(num(c["berka_run/PATHS"])))
    put("c/warmup", c["berka_run/WARM_UP"])
    put("c/origin-step", c["berka_run/ORIGIN_STEP"])
    put("c/min-records", c["berka_run/MIN_RECORDS_180"])
    put("c/sa-grid-step", c["berka_run/SA_GRID_STEP"])
    put("c/max-drift", integer(c["berka/MAX_DRIFT_CZK"]))
    put("c/reference-scale", f"{float(c['reference/REFERENCE_SCALE']):,.1f}".replace(",", "{,}"))
    put("c/aci-gamma", c["variants/ACI_GAMMA"])
    put("c/aci-max", c["variants/ACI_MAX_LEVEL"])
    put("c/aci-grid-start", c["variants/ACI_GRID_START"])
    put("c/aci-grid-step", c["variants/ACI_GRID_STEP"])
    put("c/aci-levels", str(len(c["variants/ACI_LEVELS"].strip("[]").split(","))))
    put("c/sparse-window", c["variants/SPARSE_WINDOW"])
    put("c/sparse-max", integer(c["variants/SPARSE_MAX_PER_MONTH"]))
    put("c/folds", c["variants/FOLDS"])
    put("c/dev-percent", c["split/DEV_PERCENT"])
    put("c/heldout-percent", str(100 - int(c["split/DEV_PERCENT"])))
    put("c/a1-tol-off", integer(100 * float(c["synthetic/A1_TOL_OFF"])))
    put("c/a1-tol-on", integer(100 * float(c["synthetic/A1_TOL_ON"])))
    put("c/a1-seeds", c["synthetic/A1_SEEDS"])
    put("c/panel-days", c["synthetic/PANEL_DAYS"])
    put("c/panel-seeds", c["synthetic/PANEL_SEEDS"])
    put("c/panel-first-origin", c["synthetic/PANEL_FIRST_ORIGIN"])
    put("c/panel-step", c["synthetic/PANEL_STEP"])
    put("c/regime-seeds", c["synthetic/REGIME_SEEDS"])
    put("c/regime-step-day", c["synthetic/STEP_OFFSET"])
    put("c/settle-days", c["synthetic/SETTLE_DAYS"])
    put("c/md-origin-step", c["moneydata/ORIGIN_STEP"])
    put("c/lgbm-rounds", c["lgbm/ROUNDS"])
    put("c/lookback", c["explore/L"])
    put("c/cal-half", c["explore/CAL_HALF_WIDTH"])
    rev = (ROOT / "paper/REVISION.md").read_text()
    put("c/prev-min", re.search(r"If a prevalence is below ([0-9.]+)%", rev).group(1))
    dist_src = (ROOT / "crates/ephemeris-eval/src/dist.rs").read_text()
    put("c/crps-grid", re.search(r"pub const CRPS_GRID: usize = (\d+);", dist_src).group(1))
    lv = re.search(r"^LEVELS = \[([0-9., ]+)\] \+ \[round\(0\.05 \* i, 2\) for i in range\(1, (\d+)\)\] \+ \[([0-9., ]+)\]",
                   (ROOT / "paper/baselines/lgbm.py").read_text(), re.M)
    head = [float(x) for x in lv.group(1).split(",")]
    tail = [float(x) for x in lv.group(3).split(",")]
    put("c/qf-levels", str(len(head) + int(lv.group(2)) - 1 + len(tail)))
    put("c/qf-lo", integer(100 * head[0]))
    put("c/qf-hi", integer(100 * tail[-1]))
    aci = c["variants/ACI_LEVELS"].strip("[]").split(",")
    put("c/v1-levels", str(2 * len(aci) + 1))
    put("c/train-seeds", str(eval(c["synthetic/TRAIN_SEEDS"].replace("..", ",").join(["len(range(", "))"]))))
    base = (ROOT / "crates/ephemeris-eval/src/baselines.rs").read_text()
    put("c/b30-window", re.search(r"bal\[t - (\d+)\.\.=t\]", base).group(1))
    put("c/blin-window", re.search(r"bal\[t \+ 1 - (\d+)\.\.=t\]", base).group(1))
    put("c/bsa-window", re.search(r"today - t\.day\(\) < (\d+)", base).group(1))
    agg = (ROOT / "crates/ephemeris-eval/src/aggregate.rs").read_text()
    lo, hi = re.search(r"percentile\(&draws, ([0-9.]+)\), percentile\(&draws, ([0-9.]+)\)", agg).groups()
    put("c/ci-level", integer(100 * (float(hi) - float(lo))))
    dist = (ROOT / "crates/ephemeris-eval/src/dist.rs").read_text()
    levels = re.search(r"pub const INTERVALS: \[f64; 4\] = \[([^\]]+)\]", dist).group(1).split(",")
    for i, lv in enumerate(levels, 1):
        put(f"c/l{i}", integer(100 * float(lv)))
    hz = c["berka_run/HORIZONS"].strip("[]").split(",")
    put("c/h1", hz[0].strip()); put("c/h2", hz[1].strip()); put("c/h3", hz[2].strip())
    # Pre-registered thresholds (PLAN §1, A2): ±5 points at 7 and 30 days, ±10 at 90.
    plan = (ROOT / "paper/PLAN.md").read_text()
    m = re.search(r"within ±(\d+) points of nominal\s+at 7 and 30 days, and within ±(\d+) points at 90 days", plan)
    put("c/a2-tol", m.group(1)); put("c/a2-tol-90", m.group(2))
    m = re.search(r"CRPS about (\d+)% \*better\* at 90 days", plan)
    put("c/d1-detector-pred", m.group(1))
    m = re.search(r"within ±(\d+) points of 90% at every horizon", plan)
    put("c/select-tol", m.group(1))


# ---------------------------------------------------------------- tables --

def write_table(name, body):
    TAB.mkdir(parents=True, exist_ok=True)
    (TAB / f"{name}.tex").write_text(body)


def rowmap(path, *keys):
    return {tuple(r[k] for k in keys): r for r in csv.DictReader(open(path))}


def ci(r, col, f=signed):
    return f"{f(r[col])} {{\\scriptsize[{f(r[col + '_lo'])}, {f(r[col + '_hi'])}]}}"


def tables():
    ho = rowmap(RES / "heldout/variants.csv", "method", "h")
    ho.update({k: v for k, v in rowmap(RES / "heldout/main.csv", "method", "h").items() if k not in ho})
    sel = rowmap(RES / "heldout/selection.csv", "method", "h")
    hs = ["7", "30", "90"]
    lines = []
    order = ["V0", "V1", "V2", "V3", "V4", "B-SA+", "B-EQ", "B-GLOBAL", "B-30", "B-LIN", "B-SA", "B-SEAS"]
    for m in order:
        cov = (lambda h, c: "--") if m == "B-SA" else (lambda h, c: pct(ho[(m, h)][c]))
        cells = [cov(h, "cov50") for h in hs] + [cov(h, "cov90") for h in hs] \
            + [signed(ho[(m, h)]["skill"]) for h in hs] \
            + [pct(sel[(m, h)]["pit_outer_share"]) if (m, h) in sel else "--" for h in ["30", "90"]]
        name = "\\textbf{" + m + "}" if m == "V2" else m
        lines.append(f"{name} & " + " & ".join(cells) + r" \\")
        if m == "V4":
            lines.append(r"\midrule")
    write_table("heldout-main", "\n".join([
        r"\begin{tabular}{l rrr rrr rrr rr}", r"\toprule",
        r" & \multicolumn{3}{c}{50\% coverage} & \multicolumn{3}{c}{90\% coverage} & \multicolumn{3}{c}{CRPS skill vs B-SA+} & \multicolumn{2}{c}{PIT outer}\\",
        r"\cmidrule(lr){2-4}\cmidrule(lr){5-7}\cmidrule(lr){8-10}\cmidrule(lr){11-12}",
        r"Method & 7 & 30 & 90 & 7 & 30 & 90 & 7 & 30 & 90 & 30 & 90\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    dev = rowmap(RES / "dev/variants.csv", "method", "h")
    dsel = rowmap(RES / "dev/selection.csv", "method", "h")
    lines = []
    for label, src, s in (("Development", dev, dsel), ("Held-out", ho, sel)):
        for metric, f in (("cov50", pct), ("cov90", pct), ("skill", signed)):
            name = {"cov50": "50\\% coverage", "cov90": "90\\% coverage", "skill": "skill vs B-SA+"}[metric]
            lines.append(f"{label} & {name} & " + " & ".join(f(src[('V2', h)][metric]) for h in hs) + r" \\")
        lines.append(f"{label} & mean CRPS (norm.) & " + " & ".join(integer(s[('V2', h)]['mean_crps_norm']) for h in hs) + r" \\")
        if label == "Development":
            lines.append(r"\midrule")
    write_table("devho-v2", "\n".join([
        r"\begin{tabular}{ll rrr}", r"\toprule", r"Split & Metric & 7 d & 30 d & 90 d\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    pair = rowmap(RES / "heldout-posthoc/pairwise-v2.csv", "reference", "h")
    lines = []
    for b in ["B-SA+", "B-EQ", "B-GLOBAL", "B-30", "B-LIN", "B-SA", "B-SEAS"]:
        for h in hs:
            r = pair[(b, h)]
            lines.append(f"{b if h == '7' else ''} & {h} & {integer(r['forecasts'])} & {integer(r['accounts'])} & "
                         f"{ci(r, 'skill')} & {ci(r, 'pooled_skill')} & {integer(r['mean_crps_norm'])} & "
                         f"{integer(r['mean_crps_norm_ref'])}" + r" \\")
        if b != "B-SEAS":
            lines.append(r"\addlinespace[2pt]")
    write_table("heldout-pairwise", "\n".join([
        r"\begin{tabular}{l r rr ll rr}", r"\toprule",
        r" & & & & \multicolumn{2}{c}{CRPS skill of V2 (positive: V2 lower CRPS)} & \multicolumn{2}{c}{mean CRPS (norm.)}\\",
        r"\cmidrule(lr){5-6}\cmidrule(lr){7-8}",
        r"V2 vs & $h$ & forecasts & accounts & account mean & pooled (origins) & V2 & baseline\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    # A2, cell by cell: pass (interval inside the band), fail (wholly
    # outside), inconclusive (straddles an edge).
    tol = {"7": float(VALUES["c/a2-tol"]), "30": float(VALUES["c/a2-tol"]), "90": float(VALUES["c/a2-tol-90"])}
    counts = {"pass": 0, "fail": 0, "inconclusive": 0}
    lines = []
    for lv, col in (("50", "cov50"), ("80", "cov80"), ("90", "cov90"), ("95", "cov95")):
        cells = []
        for h in hs:
            r = ho[("V2", h)]
            lo, hi, pt = (100 * float(r[col + x]) for x in ("_lo", "_hi", ""))
            blo, bhi = float(lv) - tol[h], min(100.0, float(lv) + tol[h])
            verdict = "pass" if blo <= lo and hi <= bhi else ("fail" if hi < blo or lo > bhi else "inconclusive")
            counts[verdict] += 1
            put(f"a2/{lv}/{h}/verdict", verdict)
            cells.append(f"{pt:.1f} [{lo:.1f}, {hi:.1f}] \\textit{{{verdict}}}")
        lines.append(f"{lv}\\% & " + " & ".join(cells) + r" \\")
    for k, v in counts.items():
        put(f"a2/count/{k}", str(v))
    write_table("a2-cells", "\n".join([
        r"\begin{tabular}{l lll}", r"\toprule",
        r"Interval & 7 d (band $\pm" + VALUES["c/a2-tol"] + r"$) & 30 d (band $\pm" + VALUES["c/a2-tol"]
        + r"$) & 90 d (band $\pm" + VALUES["c/a2-tol-90"] + r"$)\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    # 90% interval widths and one-sided misses (held-out).
    tails = rowmap(RES / "heldout-posthoc/tails.csv", "method", "h")
    lines = []
    for m in ["V0", "V1", "V2", "B-SA+", "B-EQ", "B-GLOBAL"]:
        w = [integer(ho[(m, h)]["median_width90_czk"]) for h in hs]
        lo_ = [f"{pct(tails[(m, h)]['miss_low'])}" for h in hs]
        hi_ = [f"{pct(tails[(m, h)]['miss_high'])}" for h in hs]
        name = "\\textbf{" + m + "}" if m == "V2" else m
        lines.append(f"{name} & " + " & ".join(w + lo_ + hi_) + r" \\")
    write_table("widths-tails", "\n".join([
        r"\begin{tabular}{l rrr rrr rrr}", r"\toprule",
        r" & \multicolumn{3}{c}{90\% width (CZK, median account)} & \multicolumn{3}{c}{miss below (\%)} & \multicolumn{3}{c}{miss above (\%)}\\",
        r"\cmidrule(lr){2-4}\cmidrule(lr){5-7}\cmidrule(lr){8-10}",
        r"Method & 7 & 30 & 90 & 7 & 30 & 90 & 7 & 30 & 90\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    st = rowmap(RES / "heldout/variants-strata.csv", "stratum", "method", "h")
    strata = ["all", "income detected", "no income detected", "history < 12 months",
              "history 12-23 months", "history >= 24 months", "regime alarm in last 28 days", "no recent alarm"]
    lines = []
    for s in strata:
        cells = []
        for m in ["V2", "B-EQ"]:
            cells += [pct(st[(s, m, h)]["cov90"]) for h in ["30", "90"]]
            cells += [signed(st[(s, m, h)]["skill"]) for h in ["30", "90"]]
        n = integer(st[(s, "V2", "30")]["forecasts"]) + " & " + integer(st[(s, "V2", "30")]["accounts"])
        label = s.replace("<", "$<$").replace(">=", "$\\geq$").replace("regime alarm in last 28 days", "alarm in last 28 days")
        lines.append(f"{label} & {n} & " + " & ".join(cells) + r" \\")
    write_table("heldout-strata", "\n".join([
        r"\begin{tabular}{lrr rrrr rrrr}", r"\toprule",
        r" & & & \multicolumn{4}{c}{V2} & \multicolumn{4}{c}{B-EQ}\\",
        r"\cmidrule(lr){4-7}\cmidrule(lr){8-11}",
        r" & & & \multicolumn{2}{c}{90\% cov.} & \multicolumn{2}{c}{skill} & \multicolumn{2}{c}{90\% cov.} & \multicolumn{2}{c}{skill}\\",
        r"Stratum & origins & accounts & 30 & 90 & 30 & 90 & 30 & 90 & 30 & 90\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    ab = rowmap(RES / "heldout-ablations/ablations-paired.csv", "stratum", "method", "h")
    lines = []
    for a in ["abl:fixed-noise", "abl:no-var-draw", "abl:no-lumps", "abl:no-detector", "abl:no-floor"]:
        cells = []
        for s in ["all", "regime alarm in last 28 days", "no recent alarm"]:
            cells += [signed(ab[(s, a, h)]["skill"], 3) for h in ["30", "90"]]
            cells += [points(ab[(s, a, h)]["cov90_diff"]) for h in ["30", "90"]]
        lines.append(f"{a.replace('abl:', '')} & " + " & ".join(cells) + r" \\")
    write_table("heldout-ablations", "\n".join([
        r"\begin{tabular}{l rrrr rrrr rrrr}", r"\toprule",
        r" & \multicolumn{4}{c}{all origins} & \multicolumn{4}{c}{alarm in last 28 days} & \multicolumn{4}{c}{no recent alarm}\\",
        r"\cmidrule(lr){2-5}\cmidrule(lr){6-9}\cmidrule(lr){10-13}",
        r" & \multicolumn{2}{c}{skill} & \multicolumn{2}{c}{$\Delta$cov90} & \multicolumn{2}{c}{skill} & \multicolumn{2}{c}{$\Delta$cov90} & \multicolumn{2}{c}{skill} & \multicolumn{2}{c}{$\Delta$cov90}\\",
        r"Ablation & 30 & 90 & 30 & 90 & 30 & 90 & 30 & 90 & 30 & 90 & 30 & 90\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    lines = []
    for gen, tag in (("model-matched", "Model-matched"), ("realistic", "Realistic")):
        sm = rowmap(RES / f"synthetic/{gen}-main.csv", "method", "h")
        for m in ["V0", "V1", "V4", "B-SA+", "B-EQ", "B-GLOBAL"]:
            name = "V0 $=$ V2" if m == "V0" else m
            cells = [pct(sm[(m, h)]["cov50"]) for h in hs] + [pct(sm[(m, h)]["cov90"]) for h in hs] \
                + [signed(sm[(m, h)]["skill"]) for h in hs]
            lines.append(f"{tag if m == 'V0' else ''} & {name} & " + " & ".join(cells) + r" \\")
        if gen == "model-matched":
            lines.append(r"\midrule")
    write_table("synthetic-main", "\n".join([
        r"\begin{tabular}{ll rrr rrr rrr}", r"\toprule",
        r" & & \multicolumn{3}{c}{50\% coverage} & \multicolumn{3}{c}{90\% coverage} & \multicolumn{3}{c}{skill vs B-SA+}\\",
        r"\cmidrule(lr){3-5}\cmidrule(lr){6-8}\cmidrule(lr){9-11}",
        r"Generator & Method & 7 & 30 & 90 & 7 & 30 & 90 & 7 & 30 & 90\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    a1 = rowmap(RES / "synthetic/a1.csv", "tracker", "h", "level")
    lines = []
    for tr, tol in (("no detector", "c/a1-tol-off"), ("default (detector on)", "c/a1-tol-on")):
        for h in hs:
            cells = [pct(a1[(tr, h, lv)]["coverage"]) for lv in ["0.5", "0.8", "0.9", "0.95"]]
            ok = all(a1[(tr, h, lv)]["pass"] == "1" for lv in ["0.5", "0.8", "0.9", "0.95"])
            pm = "$\\pm$" + VALUES[tol]
            lines.append(f"{tr if h == '7' else ''} & {pm} & {h} & " + " & ".join(cells) + f" & {'pass' if ok else 'FAIL'}" + r" \\")
    write_table("synthetic-a1", "\n".join([
        r"\begin{tabular}{llr rrrr l}", r"\toprule",
        r"Tracker & Tolerance & $h$ & 50\% & 80\% & 90\% & 95\% & \\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    det = list(csv.DictReader(open(RES / "synthetic/regime-detection.csv")))
    lines = []
    for r in det:
        if r["tracker"] != "detector":
            continue
        sc = r['scenario'].replace(' x', ' $\\times$')
        lines.append(f"{sc} & {pct(r['detected_share'], 0)} & {r['median_detect_days']} & "
                     f"{r['median_recovery_days']} & {pct(r['recovered_within_30'], 0)} & {float(r['false_alarms_per_year']):.2f}" + r" \\")
    write_table("synthetic-detection", "\n".join([
        r"\begin{tabular}{l rrrrr}", r"\toprule",
        r"Scenario & detected (\%) & median delay (d) & median recovery (d) & recovered $\leq$30 d (\%) & false alarms / yr\\",
        r"\midrule", *lines, r"\bottomrule", r"\end{tabular}", ""]))

    md = rowmap(RES / "moneydata/main.csv", "method", "h")
    lines = []
    for m in ["V0", "V1", "B-SA+", "B-EQ", "B-30", "B-LIN", "B-SA", "B-SEAS"]:
        name = "V0 $=$ V2" if m == "V0" else m
        cells = [pct(md[(m, h)]["cov90"]) for h in hs] + [signed(md[(m, h)]["skill"]) for h in hs]
        lines.append(f"{name} & " + " & ".join(cells) + r" \\")
    write_table("moneydata", "\n".join([
        r"\begin{tabular}{l rrr rrr}", r"\toprule",
        r" & \multicolumn{3}{c}{90\% coverage} & \multicolumn{3}{c}{pooled skill vs B-SA+}\\",
        r"\cmidrule(lr){2-4}\cmidrule(lr){5-7}", r"Method & 7 & 30 & 90 & 7 & 30 & 90\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    fr = rowmap(RES / "identity/identity.csv", "leg")
    po = rowmap(RES / "identity-postfix/identity.csv", "leg")
    lines = []
    for leg in fr:
        f, p = fr[leg], po[leg]
        lines.append(f"{leg[0]} & {f['golden_feed']} & {f['feeds_identical']}/{f['cases']} & {f['reports_identical']}/{f['cases']}"
                     f" & {p['golden_feed']} & {p['feeds_identical']}/{p['cases']} & {p['reports_identical']}/{p['cases']}" + r" \\")
    write_table("identity", "\n".join([
        r"\begin{tabular}{l lll lll}", r"\toprule",
        r" & \multicolumn{3}{c}{Frozen engine (pre-registered)} & \multicolumn{3}{c}{After the post-freeze fix}\\",
        r"\cmidrule(lr){2-4}\cmidrule(lr){5-7}",
        r"Leg (arm64 Mac) & golden & feeds & reports & golden & feeds & reports\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))

    tl = list(csv.DictReader(open(RES / "timeline.csv")))
    lines = []
    for r in tl:
        commit = "\\texttt{" + r['commit'] + "}"
        lines.append(f"{r['event']} & {commit} & {r['time_utc']} & {r['first_pushed_utc'] or '(run log)'}" + r" \\")
    write_table("timeline", "\n".join([
        r"\begin{tabular}{p{0.50\linewidth} l l l}", r"\toprule",
        r"Event & Commit & Committed / run (UTC) & First on GitHub (UTC)\\", r"\midrule",
        *lines, r"\bottomrule", r"\end{tabular}", ""]))
    for r in tl:
        key = r["commit"] if r["first_pushed_utc"] else ("run1" if "run 1" in r["event"] else "run2")
        put(f"tl/{key}/time", r["time_utc"])
        put(f"tl/{key}/pushed", r["first_pushed_utc"])


# --------------------------------------------------------------- derived --

def derived():
    dev = rowmap(RES / "dev/variants.csv", "method", "h")
    ho = rowmap(RES / "heldout/variants.csv", "method", "h")
    hs = ["7", "30", "90"]
    put("d/v2-devho-cov-maxdiff", f"{max(abs(100 * (float(dev[('V2', h)][c]) - float(ho[('V2', h)][c]))) for h in hs for c in ('cov50', 'cov80', 'cov90', 'cov95')):.1f}")
    put("d/v2-devho-skill-maxdiff", f"{max(abs(float(dev[('V2', h)]['skill']) - float(ho[('V2', h)]['skill'])) for h in hs):.2f}")
    sel = rowmap(RES / "heldout/selection.csv", "method", "h")
    put("d/ho-v2-worst90", f"{100 * float(sel[('V2', '7')]['worst_cov90_error']):.1f}")
    dsel = rowmap(RES / "dev/selection.csv", "method", "h")
    for m in ("V1", "V2", "V3", "V4"):
        put(f"d/dev-{m}-worst90", f"{100 * float(dsel[(m, '7')]['worst_cov90_error']):.1f}")
        put(f"d/dev-{m}-score", integer(dsel[(m, '7')]['score']))
    st = list(csv.DictReader(open(RES / "heldout/variants-strata.csv")))
    for h in ("30", "90"):
        v = [float(r["cov90"]) for r in st if r["method"] == "V2" and r["h"] == h and r["stratum"] != "all"]
        put(f"d/v2-strata-cov90-{h}-min", pct(min(v)))
        put(f"d/v2-strata-cov90-{h}-max", pct(max(v)))
        sk = [float(r["skill"]) for r in st if r["method"] == "V2" and r["h"] == h]
        put(f"d/v2-strata-skill-{h}-min", signed(min(sk)))
    pf = list(csv.DictReader(open(RES / "postfix/forecasts.csv")))
    put("d/pf-forecasts", integer(sum(int(r["forecasts"]) for r in pf)))
    put("d/pf-changed", integer(sum(int(r["forecasts_changed"]) for r in pf)))
    put("d/pf-flips", integer(sum(int(r["coverage_flips"]) for r in pf)))
    put("d/pf-origins", integer(int(pf[0]["forecasts"])))
    put("d/pf-accounts", rowmap(RES / "postfix/cells.csv", "method", "h")[("V0", "7")]["accounts"])
    put("d/syn-sparse", integer(sum(int(v) for k, v in keyvalue_raw(RES / "synthetic/data.csv").items()
                                    if k.endswith("origins_sparse_v2"))))
    put("d/syn-origins", integer(sum(int(v) for k, v in keyvalue_raw(RES / "synthetic/data.csv").items()
                                     if k.endswith("_origins"))))
    det = rowmap(RES / "synthetic/regime-detection.csv", "scenario", "tracker")
    put("d/fa-matched", f"{float(det[('matched: spend x2', 'detector')]['false_alarms_per_year']):.2f}")
    put("d/fa-realistic", f"{float(det[('realistic: spend x2', 'detector')]['false_alarms_per_year']):.2f}")
    ident = re.sub(r"\s+", " ", (RES / "identity/README.md").read_text())
    m = re.search(r"`(\d+\.\d+)` against `(\d+\.\d+)`", ident)
    put("d/c1-native", m.group(1)); put("d/c1-wasm", m.group(2))
    m = re.search(r"day (\d+), quantile index", ident)
    put("d/c1-day", m.group(1))
    m = re.search(r"about (\d+)% and (\d+)% of random inputs \(a (\d+) M-draw", ident)
    put("d/c1-ln-share", m.group(1)); put("d/c1-exp-share", m.group(2)); put("d/c1-draws", m.group(3))
    m = re.search(r"In case (\d+), one value", ident)
    put("d/c1-case", m.group(1))


def keyvalue_raw(path):
    return {r["key"]: r["value"] for r in csv.DictReader(open(path))}


def moneydata_flows():
    """Shares of gross flow by the dataset's category (for the text)."""
    path = ROOT / "data/moneydata/transactions.tsv"
    out = RES / "moneydata/flows.csv"
    if path.exists():
        rows = list(csv.DictReader(open(path), delimiter="\t"))
        amt = lambda r: (float(r["Credit Amount"]) if r["Credit Amount"] != "\\N" else 0.0) \
            - (float(r["Debit Amount"]) if r["Debit Amount"] != "\\N" else 0.0)  # noqa: E731
        by = {}
        for r in rows:
            by.setdefault(r["Category"], [0, 0.0, 0.0])
            b = by[r["Category"]]
            b[0] += 1
            b[1] += abs(amt(r))
            b[2] = max(b[2], abs(amt(r)))
        total = sum(v[1] for v in by.values())
        with open(out, "w", newline="") as f:
            w = csv.writer(f, lineterminator="\n")
            w.writerow(["category", "records", "share_of_gross", "largest"])
            for k, v in sorted(by.items(), key=lambda kv: -kv[1][1]):
                w.writerow([k, v[0], f"{v[1] / total:.4f}", f"{v[2]:.2f}"])
        iso = next(r["Transaction Date"] for r in rows if "-" in r["Transaction Date"])
        y, a, b = iso.split("-")
        (RES / "moneydata/date-example.csv").write_text(
            f"key,value\nraw,{iso}\nrepaired,{y}-{b}-{a}\n")
    ex = keyvalue_raw(RES / "moneydata/date-example.csv")
    put("d/md-date-raw", ex["raw"])
    put("d/md-date-repaired", ex["repaired"])
    flows = {r["category"]: r for r in csv.DictReader(open(out))}
    one_off = ["Supplementary Income", "Investment", "Account transfer"]
    put("d/md-irregular-share", f"{100 * sum(float(flows[c]['share_of_gross']) for c in one_off):.0f}")
    put("d/md-largest-inflow", integer(flows["Supplementary Income"]["largest"]))
    put("d/md-transfer", integer(flows["Account transfer"]["largest"]))
    put("d/md-paycheck-share", f"{100 * float(flows['Paycheck']['share_of_gross']):.0f}")


def berka_span():
    """First and last year of the Berka records (kept in a small CSV)."""
    out = RES / "berka-span.csv"
    path = ROOT / "data/berka/trans.tsv"
    if path.exists():
        years = set()
        with open(path) as f:
            next(f)
            for line in f:
                years.add(line.split("\t", 3)[2][:4])
        out.write_text(f"key,value\nfirst_year,{min(years)}\nlast_year,{max(years)}\n")
    keyvalue("berka", out)


DRIFT_THRESHOLDS = (50, 100, 1000)


def drift_recount():
    """Accounts excluded at drift thresholds of 50, 100 (applied) and 1,000
    crowns, per split, with the code's rule (audit A2)."""
    out = RES / "audit/drift-thresholds.csv"
    path = ROOT / "data/berka/trans.tsv"
    if path.exists():
        mask = (1 << 64) - 1

        def sm(x):
            z = (x + 0x9E3779B97F4A7C15) & mask
            z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & mask
            z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & mask
            return z ^ (z >> 31)
        salt = int(re.search(r"SPLIT_SALT: u64 = (0x[0-9a-f_]+)",
                             (ROOT / "crates/ephemeris-eval/src/split.rs").read_text()).group(1).replace("_", ""), 16)
        accs = {}
        with open(path) as f:
            for r in csv.DictReader(f, delimiter="\t"):
                sign = 1 if r["type"] == "PRIJEM" else -1
                accs.setdefault(int(r["account_id"]), []).append(
                    (r["date"], int(r["trans_id"]), sign * float(r["amount"]), float(r["balance"])))
        counts = {}
        for a, recs in accs.items():
            recs.sort()
            b, mx, i = recs[0][3] - recs[0][2], 0.0, 0
            while i < len(recs):
                d, rep_ = recs[i][0], []
                while i < len(recs) and recs[i][0] == d:
                    b += recs[i][2]
                    rep_.append(recs[i][3])
                    i += 1
                mx = max(mx, min(abs(x - b) for x in rep_))
            split = "development" if sm(salt ^ a) % 100 < int(VALUES["c/dev-percent"]) else "held-out"
            c = counts.setdefault(split, [0, 0, 0, 0])
            c[0] += 1
            for j, t in enumerate(DRIFT_THRESHOLDS, 1):
                c[j] += mx > t
        with open(out, "w", newline="") as f:
            w = csv.writer(f, lineterminator="\n")
            w.writerow(["split", "accounts", "drift_over_50", "drift_over_100", "drift_over_1000"])
            for k in ("development", "held-out"):
                w.writerow([k, *counts[k]])
    table("au/drift", out, ["split"])
    put("c/drift-low", integer(DRIFT_THRESHOLDS[0]))
    put("c/drift-high", integer(DRIFT_THRESHOLDS[2]))


def exploratory():
    """The exploratory results (development `xd/`, held-out `xh/`). A
    held-out file that does not exist yet is skipped."""
    import glob
    for tag, d in (("xd", RES / "exploratory"), ("xh", RES / "exploratory-heldout")):
        for sub, short in (("s3-reversal", "s3"), ("s6-v2-ablations", "s6"), ("s7-bglobal", "s7"),
                           ("s8-phase", "s8")):
            for f in sorted(glob.glob(str(d / sub / "*.csv"))):
                name = pathlib.Path(f).stem
                if name.startswith("compare"):
                    table(f"{tag}/{short}{name[len('compare'):]}", f, ["comparison", "h"])
                elif name.startswith("stats"):
                    table(f"{tag}/{short}/stats{name[len('stats'):]}", f, ["method", "h"])
                elif name in ("v2-identity", "data", "offset0-identity"):
                    keyvalue(f"{tag}/{short}/{name}", f)
        if (d / "s5-overdraft/brier.csv").exists():
            table(f"{tag}/s5", d / "s5-overdraft/brier.csv", ["threshold", "h"])
        if (d / "s4-grid/compare.csv").exists():
            table(f"{tag}/s4", d / "s4-grid/compare.csv", ["sparsity", "regularity", "comparison", "h"])
        if (d / "s1-eligibility/rules.csv").exists():
            keyvalue(f"{tag}/s1", d / "s1-eligibility/rules.csv")
            table(f"{tag}/s1/groups", d / "s1-eligibility/groups.csv", ["group", "measure"])
        if (d / "s2-small/v1-crossings.csv").exists():
            table(f"{tag}/s2/crossings", d / "s2-small/v1-crossings.csv", ["h"])
            table(f"{tag}/s2/selection", d / "s2-small/selection-account-equal.csv", ["method"])


def checksums():
    """The pinned SHA-256 of the raw files (paper/data.sha256)."""
    for line in (ROOT / "paper/data.sha256").read_text().splitlines():
        if line.strip() and not line.startswith("#"):
            h, name = line.split()
            put(f"sha/{name}", h)


def main():
    constants()
    checksums()
    berka_span()
    specs = [
        ("ho/var", "heldout/variants.csv", ["method", "h"]),
        ("ho/main", "heldout/main.csv", ["method", "h"]),
        ("ho/sel", "heldout/selection.csv", ["method", "h"]),
        ("ho/str", "heldout/variants-strata.csv", ["stratum", "method", "h"]),
        ("ho/pair", "heldout-posthoc/pairwise-v2.csv", ["reference", "h"]),
        ("ho/abl", "heldout-ablations/ablations.csv", ["method", "h"]),
        ("ho/tail", "heldout-posthoc/tails.csv", ["method", "h"]),
        ("ho/ablp", "heldout-ablations/ablations-paired.csv", ["stratum", "method", "h"]),
        ("dev/var", "dev/variants.csv", ["method", "h"]),
        ("dev/main", "dev/main.csv", ["method", "h"]),
        ("dev/sel", "dev/selection.csv", ["method", "h"]),
        ("dev/str", "dev/variants-strata.csv", ["stratum", "method", "h"]),
        ("syn/a1", "synthetic/a1.csv", ["tracker", "h", "level"]),
        ("syn/mm", "synthetic/model-matched-main.csv", ["method", "h"]),
        ("syn/re", "synthetic/realistic-main.csv", ["method", "h"]),
        ("syn/mmp", "synthetic/model-matched-pairwise.csv", ["method", "reference", "h"]),
        ("syn/rep", "synthetic/realistic-pairwise.csv", ["method", "reference", "h"]),
        ("syn/det", "synthetic/regime-detection.csv", ["scenario", "tracker"]),
        ("syn/cov", "synthetic/regime-coverage.csv", ["scenario", "offset", "method", "h"]),
        ("syn/rpair", "synthetic/regime-paired.csv", ["scenario", "offset", "h"]),
        ("md/main", "moneydata/main.csv", ["method", "h"]),
        ("id/frozen", "identity/identity.csv", ["leg"]),
        ("id/post", "identity-postfix/identity.csv", ["leg"]),
        ("pf/cells", "postfix/cells.csv", ["method", "h"]),
        ("pf/fc", "postfix/forecasts.csv", ["method", "h"]),
    ]
    for tag, path, keys in specs:
        table(tag, RES / path, keys)
    for tag, path in [("ho/data", "heldout/data.csv"), ("ho/vdata", "heldout/variants-data.csv"),
                      ("ho/abldata", "heldout-ablations/data.csv"), ("dev/data", "dev/data.csv"),
                      ("dev/vdata", "dev/variants-data.csv"), ("syn/data", "synthetic/data.csv"),
                      ("md/data", "moneydata/data.csv"), ("fig/ex", "figures/example.csv")]:
        keyvalue(tag, RES / path)
    table("au/est", RES / "audit/estimands-heldout.csv", ["comparison", "stratum", "h"])
    exploratory()
    keyvalue("au/prefix", RES / "audit/prefix-check-summary.csv")
    drift_recount()
    tables()
    derived()
    moneydata_flows()
    lines = ["% Generated by paper/analysis/macros.py from paper/results. Do not edit."]
    for k in sorted(VALUES):
        lines.append(f"\\expandafter\\def\\csname r@{k}\\endcsname{{{VALUES[k]}}}")
    (RES / "numbers.tex").write_text("\n".join(lines) + "\n")
    build = ROOT / "paper/build"
    build.mkdir(parents=True, exist_ok=True)
    (build / "numbers-keys.txt").write_text("\n".join(f"{k} = {VALUES[k]}" for k in sorted(VALUES)) + "\n")
    print(f"{len(VALUES)} values, {len(list(TAB.glob('*.tex')))} tables")


if __name__ == "__main__":
    main()
