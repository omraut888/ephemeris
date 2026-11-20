#!/usr/bin/env python3
"""B-GLOBAL without calendar look-ahead, on the development split
(exploratory; paper/REVISION.md S7).

    paper/.venv/bin/python -I paper/baselines/lgbm_cf.py \
        --features paper/build/berka-dev/features.csv --out DIR [--jobs 8]

It uses the frozen B-GLOBAL (paper/baselines/lgbm.py) unchanged: the same
features, quantile levels, hyperparameters and single-threaded LightGBM.
Only the training rows change.
- G-cf: for fold k and quarter [C_q, C_{q+1}), it trains on the other folds'
  rows whose outcome date (origin + h) is before C_q and predicts fold k's
  origins in the quarter. With fewer than MIN_TRAIN rows, it does not
  predict.
- G-cut: one model per fold and horizon, trained on the other folds' rows
  with outcome before CUTOFF. It predicts fold k's origins on or after
  CUTOFF.
Fits run in parallel processes. Each is deterministic, so the output does
not depend on the order.
"""
import argparse
import datetime
import importlib.util
import pathlib
import sys
from concurrent.futures import ProcessPoolExecutor

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("lgbm", HERE / "lgbm.py")
lgbm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lgbm)

FIRST_QUARTER = datetime.date(1994, 1, 1)
LAST_QUARTER = datetime.date(1998, 10, 1)
CUTOFF = datetime.date(1996, 7, 1)
MIN_TRAIN = 1000

STATE = {}


def init(path):
    names, X, y, h, acct, keys = lgbm.load(path)
    origin = np.array([datetime.date.fromisoformat(k[1]).toordinal() for k in keys])
    outcome = origin + h
    folds = np.array([lgbm.fold_of(int(a)) for a in acct])
    STATE.update(names=names, X=X, y=y, h=h, origin=origin, outcome=outcome, folds=folds)


def quarters():
    q, out = FIRST_QUARTER, []
    while q <= LAST_QUARTER:
        nxt = datetime.date(q.year + (q.month + 2) // 12, (q.month + 2) % 12 + 1, 1)
        out.append((q.toordinal(), nxt.toordinal()))
        q = nxt
    return out


def job(args):
    """One (variant, horizon, fold, window): (test indices, predictions)."""
    variant, hz, k, lo, hi = args
    s = STATE
    test = np.where((s["h"] == hz) & (s["folds"] == k) & (s["origin"] >= lo) & (s["origin"] < hi))[0]
    train = np.where((s["h"] == hz) & (s["folds"] != k) & (s["outcome"] < lo))[0]
    if len(test) == 0 or len(train) < MIN_TRAIN:
        return variant, test, None, len(train)
    return variant, test, lgbm.predict_block(train, test, s["X"], s["y"], s["names"]), len(train)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--features", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--jobs", type=int, default=8)
    a = ap.parse_args()
    init(a.features)
    jobs = []
    for hz in lgbm.HORIZONS:
        for k in range(lgbm.FOLDS):
            for lo, hi in quarters():
                jobs.append(("cf", hz, k, lo, hi))
            jobs.append(("cut", hz, k, CUTOFF.toordinal(), 10**9))
    n = len(STATE["y"])
    preds = {"cf": np.full((n, len(lgbm.LEVELS)), np.nan), "cut": np.full((n, len(lgbm.LEVELS)), np.nan)}
    skipped = {"cf": 0, "cut": 0}
    with ProcessPoolExecutor(max_workers=a.jobs, initializer=init, initargs=(a.features,)) as pool:
        for i, (variant, test, p, ntrain) in enumerate(pool.map(job, jobs)):
            if p is None:
                skipped[variant] += len(test)
            else:
                preds[variant][test] = p
            if i % 25 == 0:
                print(f"  {i + 1}/{len(jobs)} fits", file=sys.stderr)
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    _, _, _, _, _, keys = lgbm.load(a.features)
    for variant, p in preds.items():
        ok = ~np.isnan(p[:, 0])
        lgbm.write(out / f"quantiles-{variant}.csv", [k for k, o in zip(keys, ok) if o], p[ok])
        print(f"{variant}: predicted {int(ok.sum())} of {n}; without enough training rows {skipped[variant]}",
              file=sys.stderr)


if __name__ == "__main__":
    main()
