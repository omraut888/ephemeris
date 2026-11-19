#!/usr/bin/env python3
"""The paper's figures, from committed result files only (paper/PLAN.md §3).

    paper/.venv/bin/python -I paper/figures/src/make_figures.py [--png DIR]

Writes one PDF per figure into paper/figures/. With --png, also writes PNG
copies into DIR, for looking at. The PDFs carry no creation date, so a rerun
on the same inputs gives the same bytes.

Colors follow a fixed role per method, the same in every figure: V0 blue,
V1 orange, V2 aqua, B-EQ yellow, B-GLOBAL magenta. They are the first five
slots of the validated categorical palette, and every line also has its own
marker or dash and a direct label (the contrast relief rule).
"""
import argparse
import csv
import pathlib

import matplotlib

matplotlib.use("Agg")
import matplotlib.patches  # noqa: E402
import matplotlib.pyplot as plt  # noqa: E402
import matplotlib.ticker  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
RESULTS = ROOT / "results"
OUT = ROOT / "figures"

INK = "#0b0b0b"
INK2 = "#52514e"
MUTED = "#8a8985"
GRID = "#e4e3df"
SURFACE = "#fcfcfb"
COLOR = {
    "V0": "#2a78d6",
    "V1": "#eb6834",
    "V2": "#1baf7a",
    "B-EQ": "#eda100",
    "B-GLOBAL": "#e87ba4",
}
STYLE = {
    "V0": dict(marker="o", linestyle="-"),
    "V1": dict(marker="s", linestyle="--"),
    "V2": dict(marker="D", linestyle="-"),
    "B-EQ": dict(marker="^", linestyle=":"),
    "B-GLOBAL": dict(marker="v", linestyle="-."),
}
HORIZONS = [7, 30, 90]

plt.rcParams.update({
    "font.family": "DejaVu Sans",
    "font.size": 8,
    "axes.titlesize": 8.5,
    "axes.labelsize": 8,
    "axes.edgecolor": MUTED,
    "axes.labelcolor": INK2,
    "axes.linewidth": 0.6,
    "axes.facecolor": SURFACE,
    "figure.facecolor": SURFACE,
    "xtick.color": INK2,
    "ytick.color": INK2,
    "xtick.major.width": 0.6,
    "ytick.major.width": 0.6,
    "grid.color": GRID,
    "grid.linewidth": 0.6,
    "legend.frameon": False,
    "pdf.fonttype": 42,
    "lines.linewidth": 1.6,
    "lines.markersize": 4.5,
})


def read(path):
    with open(path, newline="") as f:
        return list(csv.DictReader(f))


def tidy(ax, grid_axis="y"):
    for side in ("top", "right"):
        ax.spines[side].set_visible(False)
    ax.grid(True, axis=grid_axis)
    ax.set_axisbelow(True)


def save(fig, name, png):
    fig.savefig(OUT / f"{name}.pdf", metadata={"CreationDate": None, "ModDate": None})
    if png:
        fig.savefig(png / f"{name}.png", dpi=170)
    plt.close(fig)


def pit(png):
    """PIT histograms of V0, V1 and V2 at 30 and 90 days, held-out Berka."""
    rows = {(r["method"], int(r["h"])): r for r in read(RESULTS / "heldout/variants-pit.csv")}
    methods = ["V0", "V1", "V2"]
    fig, axes = plt.subplots(2, 3, figsize=(6.6, 3.6), sharex=True, sharey=True)
    for i, h in enumerate([30, 90]):
        for j, m in enumerate(methods):
            ax = axes[i, j]
            counts = [int(rows[(m, h)][f"b{k}"]) for k in range(10)]
            n = sum(counts)
            dens = [10 * c / n for c in counts]
            ax.bar([k / 10 + 0.05 for k in range(10)], dens, width=0.1 - 0.012,
                   color=COLOR[m], edgecolor=SURFACE, linewidth=0.8)
            ax.axhline(1.0, color=INK2, linewidth=0.8, linestyle="--")
            ax.set_xlim(0, 1)
            ax.set_ylim(0, 3.6)
            tidy(ax)
            if i == 0:
                ax.set_title(m, color=INK)
            if j == 0:
                ax.set_ylabel(f"{h} days\ndensity")
            if i == 1:
                ax.set_xlabel("PIT")
    axes[0, 2].text(0.98, 1.05, "uniform", transform=axes[0, 2].get_yaxis_transform(),
                    ha="right", va="bottom", color=INK2, fontsize=7)
    fig.suptitle("PIT histograms, held-out Berka (3,514 accounts)", color=INK, fontsize=9)
    fig.tight_layout()
    save(fig, "pit-heldout", png)


def coverage(png):
    """Empirical against nominal central-interval coverage, every level."""
    rows = read(RESULTS / "figures/coverage-curve-heldout.csv")
    methods = ["V0", "V1", "V2", "B-EQ", "B-GLOBAL"]
    fig, axes = plt.subplots(1, 3, figsize=(6.8, 2.7), sharey=True)
    for ax, h in zip(axes, HORIZONS):
        ax.plot([0, 1], [0, 1], color=MUTED, linewidth=0.8, linestyle="--")
        for m in methods:
            pts = [(float(r["level"]), float(r["coverage_pit"]))
                   for r in rows if r["method"] == m and int(r["h"]) == h]
            xs, ys = zip(*pts)
            ax.plot(xs, ys, color=COLOR[m], markevery=[3, 9, 17, 19], **STYLE[m], label=m)
        ax.set_xlim(0, 1)
        ax.set_ylim(0, 1.05)
        ax.set_title(f"{h} days", color=INK)
        ax.set_xlabel("nominal coverage")
        tidy(ax, "both")
    axes[0].set_ylabel("empirical coverage")
    handles, labels = axes[0].get_legend_handles_labels()
    fig.legend(handles, labels, loc="lower center", ncol=5, bbox_to_anchor=(0.5, -0.02))
    fig.suptitle("Central-interval coverage at every level, held-out Berka", color=INK, fontsize=9)
    fig.tight_layout(rect=(0, 0.08, 1, 1))
    save(fig, "coverage-heldout", png)


def skill(png):
    """V2's CRPS skill against each baseline by horizon, 95% intervals."""
    rows = read(RESULTS / "heldout-posthoc/pairwise-v2.csv")
    baselines = ["B-SA+", "B-EQ", "B-GLOBAL", "B-30", "B-LIN", "B-SA", "B-SEAS"]
    fig, axes = plt.subplots(1, 7, figsize=(7.0, 2.4), sharey=True)
    for ax, b in zip(axes, baselines):
        pts = sorted((int(r["h"]), float(r["skill"]), float(r["skill_lo"]), float(r["skill_hi"]))
                     for r in rows if r["reference"] == b)
        hs = range(len(pts))
        ys = [p[1] for p in pts]
        err = [[p[1] - p[2] for p in pts], [p[3] - p[1] for p in pts]]
        ax.axhline(0, color=INK2, linewidth=0.8)
        ax.errorbar(hs, ys, yerr=err, color=COLOR["V2"], marker="D", linestyle="-",
                    capsize=2.5, elinewidth=1.0)
        for x, y in zip(hs, ys):
            ax.text(x, y + (0.09 if y >= 0 else -0.09), f"{y:+.2f}", ha="center",
                    va="bottom" if y >= 0 else "top", fontsize=6, color=INK2)
        ax.set_xticks(list(hs), [str(p[0]) for p in pts])
        ax.set_xlim(-0.5, len(pts) - 0.5)
        ax.set_title(f"vs {b}", color=INK)
        ax.set_xlabel("days")
        tidy(ax)
    axes[0].set_ylabel("CRPS skill of V2\n(> 0: V2 better)")
    axes[0].set_ylim(-1.3, 0.8)
    fig.suptitle("V2 against each baseline, held-out Berka (post-hoc paired intervals)",
                 color=INK, fontsize=9)
    fig.tight_layout()
    save(fig, "skill-v2-heldout", png)


def detector(png):
    """Detector on and off around a known change (synthetic, 30-day horizon)."""
    cov = read(RESULTS / "synthetic/regime-coverage.csv")
    pair = read(RESULTS / "synthetic/regime-paired.csv")
    scenarios = []
    for r in cov:
        if r["scenario"] not in scenarios:
            scenarios.append(r["scenario"])
    on, off = "#2a78d6", "#eb6834"
    fig, axes = plt.subplots(2, len(scenarios), figsize=(7.2, 3.8), sharex=True, sharey="row")
    for j, sc in enumerate(scenarios):
        top, bot = axes[0, j], axes[1, j]
        for ax in (top, bot):
            ax.axvspan(-30, 0, color=GRID, alpha=0.6, linewidth=0)
            ax.axvline(0, color=INK2, linewidth=0.8)
            tidy(ax)
        for method, color, style in (("detector", on, "-"), ("no detector", off, "--")):
            pts = sorted((int(r["offset"]), float(r["cov90"]), float(r["cov90_lo"]), float(r["cov90_hi"]))
                         for r in cov if r["scenario"] == sc and r["method"] == method and r["h"] == "30")
            xs = [p[0] for p in pts]
            top.errorbar(xs, [p[1] for p in pts],
                         yerr=[[p[1] - p[2] for p in pts], [p[3] - p[1] for p in pts]],
                         color=color, linestyle=style, marker="o" if method == "detector" else "s",
                         capsize=1.5, elinewidth=0.8, markersize=3, label=method)
        top.axhline(0.9, color=MUTED, linewidth=0.7, linestyle=":")
        top.set_ylim(-0.03, 1.03)
        top.set_title(sc.replace(": ", "\n"), color=INK, fontsize=7.5)
        pts = sorted((int(r["offset"]), float(r["skill"]), float(r["skill_lo"]), float(r["skill_hi"]))
                     for r in pair if r["scenario"] == sc and r["h"] == "30")
        bot.axhline(0, color=INK2, linewidth=0.8)
        bot.errorbar([p[0] for p in pts], [p[1] for p in pts],
                     yerr=[[p[1] - p[2] for p in pts], [p[3] - p[1] for p in pts]],
                     color=on, marker="o", capsize=1.5, elinewidth=0.8, markersize=3)
    axes[0, 0].set_ylabel("90% coverage\n(30-day forecast)")
    axes[1, 0].set_ylabel("CRPS skill of detector\nvs none (> 0: better)")
    fig.supxlabel("forecast origin minus change day (days)", fontsize=8, color=INK2, y=0.07)
    handles, labels = axes[0, 0].get_legend_handles_labels()
    handles.append(matplotlib.patches.Patch(color=GRID, alpha=0.6))
    labels.append("30-day window spans the change")
    fig.legend(handles, labels, loc="lower center", ncol=3, bbox_to_anchor=(0.5, -0.01))
    fig.suptitle("Regime changes with known dates (synthetic, 100 seeds per scenario)", color=INK, fontsize=9)
    fig.tight_layout(rect=(0, 0.09, 1, 1))
    save(fig, "detector-synthetic", png)


def cone(png):
    """One development account's 90-day cone: V2 bands, V0's 90% band, truth."""
    rows = read(RESULTS / "figures/example-cone.csv")
    meta = {r["key"]: r["value"] for r in read(RESULTS / "figures/example.csv")}
    hist = [(int(r["day"]), float(r["balance"])) for r in rows if r["model"] == "history"]
    fig, ax = plt.subplots(figsize=(6.4, 2.9))
    past = [p for p in hist if p[0] <= 0]
    after = [p for p in hist if p[0] >= 0]
    v2 = [r for r in rows if r["model"] == "V2"]
    v0 = [r for r in rows if r["model"] == "V0"]
    d = [int(r["day"]) for r in v2]
    q = lambda rs, k: [float(r[k]) for r in rs]  # noqa: E731
    ax.fill_between(d, q(v2, "q05"), q(v2, "q95"), color=COLOR["V2"], alpha=0.22, linewidth=0,
                    label="V2 90% interval")
    ax.fill_between(d, q(v2, "q25"), q(v2, "q75"), color=COLOR["V2"], alpha=0.45, linewidth=0,
                    label="V2 50% interval")
    ax.plot(d, q(v2, "q50"), color=COLOR["V2"], linewidth=1.4, label="V2 median")
    ax.plot(d, q(v0, "q05"), color=COLOR["V0"], linewidth=1.0, linestyle="--", label="V0 90% interval")
    ax.plot(d, q(v0, "q95"), color=COLOR["V0"], linewidth=1.0, linestyle="--")
    ax.plot(*zip(*past), color=INK, linewidth=1.0, label="balance")
    ax.plot(*zip(*after), color=INK, linewidth=1.0, linestyle=(0, (1, 1.2)), label="balance (realized)")
    ax.axvline(0, color=INK2, linewidth=0.8)
    ax.text(1, ax.get_ylim()[1], " origin", color=INK2, fontsize=7, va="top")
    ax.set_xlabel("days from the origin")
    ax.set_ylabel("balance (CZK)")
    ax.yaxis.set_major_formatter(matplotlib.ticker.FuncFormatter(lambda v, _: f"{v:,.0f}"))
    tidy(ax)
    ax.legend(loc="upper left", ncol=2, fontsize=6.5)
    ax.set_title(f"Example forecast: development account {meta['account']}, origin {meta['origin']}",
                 color=INK)
    fig.tight_layout()
    save(fig, "cone-example", png)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--png", type=pathlib.Path)
    a = ap.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    if a.png:
        a.png.mkdir(parents=True, exist_ok=True)
    for f in (pit, coverage, skill, detector, cone):
        f(a.png)
        print(f"{f.__name__}: done")


if __name__ == "__main__":
    main()
