#!/usr/bin/env python3
"""B-GLOBAL: a global LightGBM quantile model (paper/PLAN.md §2, §8 decision 3).

    paper/.venv/bin/python -I paper/baselines/lgbm.py crossfit \
        --features paper/build/berka-dev/features.csv --out Q.csv
    paper/.venv/bin/python -I paper/baselines/lgbm.py fit-predict \
        --train DEV/features.csv --predict HELDOUT/features.csv --out Q.csv

It predicts quantiles of the h-day balance change (truth − b0), in each
account's normalized units, from the features `ephemeris-eval berka`
writes, with one model per horizon and quantile level.

- `crossfit` (development): accounts are split into 5 folds by a fixed hash
  of the account id; each fold is predicted by models trained on the other
  four, so no account is scored by a model that saw it.
- `fit-predict` (held-out): trained on all development accounts, applied to
  the held-out ones.

The hyperparameters are fixed here, at common defaults, and were not tuned
on any outcome. Training is single-threaded and seeded, so the output is
reproducible. `ephemeris-eval score-quantiles` turns the output into rows.
"""

import argparse
import csv
import sys

import lightgbm as lgb
import numpy as np

LEVELS = [0.01, 0.025] + [round(0.05 * i, 2) for i in range(1, 20)] + [0.975, 0.99]
HORIZONS = [7, 30, 90]
FOLDS = 5
FOLD_SALT = 0x5048415345362D33  # "PHASE6-3"
# Not features: identifiers, the outcome, and the unit factor.
NOT_FEATURES = {"account", "origin", "h", "k", "truth"}
PARAMS = dict(
    objective="quantile",
    learning_rate=0.05,
    num_leaves=31,
    min_data_in_leaf=50,
    feature_fraction=0.9,
    bagging_fraction=0.8,
    bagging_freq=1,
    lambda_l2=1.0,
    seed=0,
    deterministic=True,
    force_col_wise=True,
    num_threads=1,
    verbose=-1,
)
ROUNDS = 300
MASK = (1 << 64) - 1


def splitmix64(x):
    z = (x + 0x9E3779B97F4A7C15) & MASK
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK
    return z ^ (z >> 31)


def fold_of(account):
    return splitmix64(FOLD_SALT ^ account) % FOLDS


def load(path):
    with open(path, newline="") as f:
        rows = list(csv.DictReader(f))
    if not rows:
        sys.exit(f"{path}: no rows")
    names = [c for c in rows[0] if c not in NOT_FEATURES]
    X = np.array([[float(r[c]) for c in names] for r in rows])
    y = np.array([float(r["truth"]) - float(r["b0"]) for r in rows])
    h = np.array([int(r["h"]) for r in rows])
    acct = np.array([int(r["account"]) for r in rows])
    keys = [(r["account"], r["origin"], r["h"]) for r in rows]
    return names, X, y, h, acct, keys


def fit(X, y, alpha, names):
    data = lgb.Dataset(X, label=y, feature_name=names, free_raw_data=False)
    return lgb.train(dict(PARAMS, alpha=alpha), data, num_boost_round=ROUNDS)


def predict_block(train_idx, test_idx, X, y, names):
    out = np.zeros((len(test_idx), len(LEVELS)))
    for j, a in enumerate(LEVELS):
        out[:, j] = fit(X[train_idx], y[train_idx], a, names).predict(X[test_idx])
    return out


def write(path, keys, preds):
    with open(path, "w", newline="\n") as f:
        f.write("account,origin,h," + ",".join(f"q{a}" for a in LEVELS) + "\n")
        for k, p in sorted(zip(keys, preds), key=lambda kp: (int(kp[0][0]), kp[0][1], int(kp[0][2]))):
            f.write(",".join(k) + "," + ",".join(repr(float(v)) for v in p) + "\n")


def crossfit(args):
    names, X, y, h, acct, keys = load(args.features)
    folds = np.array([fold_of(int(a)) for a in acct])
    preds = np.zeros((len(y), len(LEVELS)))
    for hz in HORIZONS:
        for k in range(FOLDS):
            test = np.where((h == hz) & (folds == k))[0]
            train = np.where((h == hz) & (folds != k))[0]
            if len(test) == 0:
                continue
            preds[test] = predict_block(train, test, X, y, names)
            print(f"h={hz} fold={k}: train {len(train)}, predict {len(test)}", file=sys.stderr)
    write(args.out, keys, preds)


def fit_predict(args):
    names, X, y, h, _, _ = load(args.train)
    names2, X2, _, h2, _, keys2 = load(args.predict)
    if names2 != names:
        sys.exit("feature columns differ between train and predict files")
    preds = np.zeros((len(keys2), len(LEVELS)))
    for hz in HORIZONS:
        train = np.where(h == hz)[0]
        test = np.where(h2 == hz)[0]
        for j, a in enumerate(LEVELS):
            preds[test, j] = fit(X[train], y[train], a, names).predict(X2[test])
        print(f"h={hz}: train {len(train)}, predict {len(test)}", file=sys.stderr)
    write(args.out, keys2, preds)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("crossfit")
    c.add_argument("--features", required=True)
    c.add_argument("--out", required=True)
    p = sub.add_parser("fit-predict")
    p.add_argument("--train", required=True)
    p.add_argument("--predict", required=True)
    p.add_argument("--out", required=True)
    args = ap.parse_args()
    {"crossfit": crossfit, "fit-predict": fit_predict}[args.cmd](args)


if __name__ == "__main__":
    main()
