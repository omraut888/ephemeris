#!/usr/bin/env python3
"""The commit timeline of the paper's protocol (paper/PLAN.md §8-§11).

    paper/.venv/bin/python -I paper/analysis/timeline.py

For each protocol event it writes paper/results/timeline.csv with:
- the commit and its committer time (UTC), from git;
- the first time GitHub received a push containing the commit, from the
  repository's activity API (`gh api repos/OWNER/REPO/activity`).

The second is evidence the author cannot backdate: a committer time is set
by the author's machine, a push time by GitHub. The two held-out runs carry
the times their guard logged (paper/results/heldout-runs.log).

It needs `gh`, logged in, and network access. paper.sh runs it only when
`gh` works, and otherwise keeps the committed CSV.
"""
import csv
import datetime
import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[2]
REPO = "omraut888/ephemeris"
OUT = ROOT / "paper/results/timeline.csv"

# (commit, what it records). Commits are identified by hash; each is
# checked to exist below.
EVENTS = [
    ("8c40faf", "Plan: claims and thresholds pre-registered"),
    ("e4d9e96", "Decisions approved"),
    ("4e7c066", "Berka adapter; two adapter deviations recorded"),
    ("b8b90e9", "First real-data results (development split)"),
    ("aead9a4", "Revision rule: V1-V4 and selection rule, before revision code"),
    ("69feb54", "Revision code: sparse spending in the core (opt-in)"),
    ("26ed672", "Revision code: V1-V4 in the evaluation"),
    ("f0aa40b", "Development results of V0-V4; rule selects V2"),
    ("70a7bca", "Freeze: V2 primary, configuration fixed"),
    ("a5cbc9f", "Held-out plumbing: guarded single-run command"),
    ("RUN1", "Held-out run 1 (logged by the guard)"),
    ("cdf070d", "Held-out results committed"),
    ("5817d1a", "Post-hoc: paired intervals for V2 vs each baseline"),
    ("ae6b74a", "Ablation tooling for the second held-out run"),
    ("RUN2", "Held-out run 2: the omitted ablations (logged)"),
    ("336bc66", "Held-out ablation results committed"),
    ("1a3a219", "Synthetic results"),
    ("7e979be", "MoneyData case study"),
    ("dfa0056", "Bit identity: C1 falsified for the frozen engine"),
    ("f98fed5", "Post-freeze fix of the sampling math"),
    ("1bec1e0", "Bit identity after the fix"),
    ("129e222", "x86_64 workflow extended to the 100 identity cases"),
    ("CI44827652848", "x86_64 workflow run #4 on the fixed engine: passed"),
    ("9ec075f", "Exploratory specifications S1-S8 fixed (after the held-out results)"),
    ("c6f577b", "Exploratory results S1-S6, development split"),
    ("RUN3", "Exploratory held-out run: S3, S5, S6 (logged)"),
    ("RUN4", "Exploratory held-out run: S7, killed before any output (logged)"),
    ("RUN5", "Exploratory held-out run: S7 rerun (logged)"),
    ("7f9e8c5", "Exploratory held-out results S3, S5, S6 committed"),
    ("5f30843", "Exploratory held-out results S7 committed"),
    ("bde5b60", "Calibration in the large, from the stored events"),
    ("fe14e1d", "Confirmatory specification S15 committed"),
    ("RUN6", "Held-out start balances for the overdraft split, no forecasts (logged)"),
    ("44afcf4", "S15 amended (P3b) before the run"),
    ("RUN7", "Confirmatory forward-period run S15 (logged)"),
    ("7102b72", "S15 results committed"),
]


def git(*args):
    return subprocess.run(["git", "-C", str(ROOT), *args], check=True,
                          capture_output=True, text=True).stdout.strip()


def utc(iso):
    t = datetime.datetime.fromisoformat(iso.replace("Z", "+00:00"))
    return t.astimezone(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M")


def is_ancestor(a, b):
    return subprocess.run(["git", "-C", str(ROOT), "merge-base", "--is-ancestor", a, b],
                          capture_output=True).returncode == 0


def main():
    pushes = json.loads(subprocess.run(
        ["gh", "api", f"repos/{REPO}/activity?per_page=100&activity_type=push"],
        check=True, capture_output=True, text=True).stdout)
    pushes = sorted(
        [(p["timestamp"], p["before"], p["after"]) for p in pushes
         if p["ref"] == "refs/heads/main"])
    runs = [l.split("\t") for l in (ROOT / "paper/results/heldout-runs.log").read_text().splitlines()
            if l and not l.startswith("#")]
    rows = []
    for key, what in EVENTS:
        if key.startswith("CI"):
            # A GitHub Actions run: its creation time and commit come from
            # GitHub, not from this machine.
            r = json.loads(subprocess.run(
                ["gh", "run", "view", key[2:], "-R", REPO, "--json", "number,headSha,createdAt,conclusion"],
                check=True, capture_output=True, text=True).stdout)
            assert r["conclusion"] == "success", r
            rows.append([what, r["headSha"][:7], utc(r["createdAt"]), "", "GitHub Actions", f"ci{r['number']}"])
            continue
        if key.startswith("RUN"):
            ts, commit = runs[int(key[3:]) - 1][:2]
            rows.append([what, commit[:7], utc(ts), "", "guard log", key.lower()])
            continue
        full = git("rev-parse", key)
        committed = utc(git("show", "-s", "--format=%cI", full))
        first = ""
        for ts, before, after in pushes:
            known = all(c != "0" for c in before) and is_ancestor(full, before)
            if is_ancestor(full, after) and not known:
                first = utc(ts)
                break
        rows.append([what, key, committed, first, "git; GitHub activity", key])
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with open(OUT, "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        w.writerow(["event", "commit", "time_utc", "first_pushed_utc", "source", "key"])
        w.writerows(rows)
    print(OUT.read_text())


if __name__ == "__main__":
    main()
