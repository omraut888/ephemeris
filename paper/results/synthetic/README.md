# Synthetic evaluation

Built by `scripts/paper-synthetic.sh`; the code is
`crates/ephemeris-eval/src/synthetic.rs`. A seed plays the part of an
account, so intervals resample whole seeds (2000 resamples, seed 6).
Units are the generator's dollars. The `_czk` column names are kept so
the tables have the same format as Berka's.

- **`a1.csv`: claim A1.** V0 on model-matched data, using exactly the old
  core test's protocol: 100 seeds, origins at days 150, 210 and 270, and
  forecast seeds `10·seed + i`. It reproduces that test's coverage counts
  and CRPS exactly. Tolerance: ±5 points with the detector off, ±8 with
  it on.
- **`{model-matched,realistic}-main.csv`, `-pit.csv`, `-pairwise.csv`: the
  T4 synthetic panel (claim B2).**
  - 100 seeds of 730 days, origins every 30 days from day 180 (16 per
    seed).
  - Methods: V0, V1, V2 and V4, every per-account baseline, and B-GLOBAL.
    B-GLOBAL is trained on 100 disjoint seeds of the same generator.
  - V4's priors come from the other seed folds, as on Berka's development
    split.
  - V2's sparse rule never fired (0 of 1,600 origins in each generator),
    so V2 equals V0 here.
  - The baselines and the V1/V4 calibration streams see the
    available-balance series, the ledger plus visible pending amounts.
    Every method is scored against the true balance.
- **`regime-*.csv`: the T6 regime sweep.**
  - Six step-change scenarios, 100 seeds each, with the change on day
    182.
  - Origins at −28, −14 and −7 days before the change and +7, +14, +21,
    +28 and +56 after it, at horizons 7, 30 and 90 days.
  - Each scenario runs with the detector on and off.
  - A "before" origin whose horizon crosses the change day is forecasting
    the change itself, which no forecaster here can anticipate.
  - `regime-paired.csv` is the detector against no detector, paired by
    seed. Positive skill means the detector is better.
  - `regime-detection.csv` has detection delay, recovery time (within 25%
    of the step for 14 days), and false alarms per year in the same seeds
    with no step.
