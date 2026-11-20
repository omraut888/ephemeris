# Exploratory analyses (development split), 2026-11-21

These analyses are **exploratory**, not confirmatory. The specifications
were fixed before running (`paper/REVISION.md`, S1–S8), and every change
made during the runs is recorded there. Only development data was used;
nothing here has been run on held-out data.

- **Code:**
  - `crates/ephemeris-eval/src/explore.rs`;
  - `synthetic.rs` (the S4 grid);
  - `paper/baselines/lgbm_cf.py` (S7).
- **Summaries:** `paper/analysis/explore_summary.py`.
- **Conventions:** every comparison is on the origins its methods share,
  with counts, both the account mean and the pooled skill, and 95%
  bootstrap intervals over accounts (seeds in S4).

## S1. Eligibility and exclusions (`s1-eligibility/`)

**The past-only drift rule selects exactly the origins of the frozen
full-history rule:** 27,855 each, none added or removed. Every
drift-excluded account already drifts by at least 500 crowns at its first
grid origin.

| Group | Records/month | Volatility | Income share |
|---|---|---|---|
| Drift-excluded (19 accounts) | 5.6 | 0.29 | 0.82 |
| Kept (871 accounts) | 5.5 | 0.32 | 0.50 |
| Kept, no eligible origin (5 accounts) | 1.0 | 1.78 | – |

(Medians; volatility is the standard deviation of 30-day balance changes
over the median monthly gross flow.)

- **Drift exclusion:** the excluded accounts are not more volatile and
  not sparser.
- **Minimum-history and minimum-records rules:** they do remove volatile,
  sparse cases, but few. Grid origins failing the minimum-records rule
  (433, against 27,855 eligible) have daily balance volatility 0.198 of a
  month's flow, against 0.123.

## S2. Small additions (`s2-small/`)

**V1's adjusted levels cross often.** They are non-monotone in 24% of
forecasts (31%, 23% and 19% at 7, 30 and 90 days). Sorting moves 4.9% of
V1's quantile values.

**The selection rule's choice is robust.** With account-equal weighting,
V2 still wins: 2,271 against V4's 2,298 and V1's 2,773. V4 does not
qualify either way.

## S3. The reversal, decomposed on Berka (`s3-reversal/`)

All methods use L = 365 and the same 27,855 origins. Skill of the first
method against the second:

| Comparison | 30 d mean | 30 d pooled | 90 d mean | 90 d pooled |
|---|---|---|---|---|
| B-EQc vs B-EQ | −0.013 | −0.014 | −0.013 | −0.012 |
| B-SR vs B-EQ | −0.18 | −0.09 | −0.34 | −0.18 |
| B-SR vs V2 | +0.03 | +0.03 | −0.01 (n.s.) | +0.02 (n.s.) |
| CP-noKF vs V2 | −0.04 | −0.02 | −0.05 | −0.04 |
| CP-noKF vs B-EQ | −0.29 | −0.15 | −0.44 | −0.25 |
| V2 vs B-EQ | −0.24 | −0.12 | −0.36 | −0.20 |

**Median 90% width** (normalized units):

| Method | 30 d | 90 d |
|---|---|---|
| V2 | 43,615 | 76,308 |
| CP-noKF | 42,538 | 74,429 |
| B-SR | 24,452 | 31,185 |
| B-EQ | 24,571 | 31,775 |

V2's and CP-noKF's 50% intervals cover 69–71% and 67–68%.

**History strata, V2 against B-EQ, pooled** (30 and 90 days):

| History | 30 days | 90 days |
|---|---|---|
| Under 12 months | −0.017 [−0.042, +0.006] | +0.039 [+0.006, +0.071] |
| 12–23 months | −0.16 | −0.32 |
| 24 months or more | −0.15 | −0.29 |

**Fresh V2 against frozen V2:** 83,564 of 83,565 rows are identical. The
other differs by one ulp in its median (the post-freeze fix).

## S4. Independent simulation (`s4-grid/`)

Skill of V2 against B-EQ, pooled. Rows are sparsity, from 2.5 to 0.05
purchases a day; columns are regularity levels 0–3.

| Purchases/day | 30 d, level 0 | 1 | 2 | 3 | 90 d, level 0 | 1 | 2 | 3 |
|---|---|---|---|---|---|---|---|---|
| 2.5 | +0.49 | +0.47 | +0.04 | −1.68 | +0.45 | +0.42 | +0.19 | −1.07 |
| 0.5 | +0.21 | +0.20 | −0.01 | −1.41 | +0.13 | +0.13 | +0.01 | −0.85 |
| 0.15 | +0.14 | +0.14 | +0.00 | −1.08 | +0.17 | +0.16 | +0.09 | −0.49 |
| 0.05 | +0.07 | +0.07 | +0.01 | −0.80 | +0.13 | +0.13 | +0.11 | −0.23 |

- V2's sparse rule fired at every origin at 0.15 and 0.05 purchases a
  day, and almost never otherwise.
- **The reversal appears only with highly irregular schedules (level 3),
  at every sparsity.**
- Sparsity alone shrinks V2's lead but never reverses it.

## S5. Overdraft events (`s5-overdraft/`)

The events are below 0 (`u0`) and below the account's own 10th
percentile (`u10`).

**Prevalence:**

| Threshold | 7 d | 30 d | 90 d |
|---|---|---|---|
| u0 | 0.7% | 1.1% | 1.8% |
| u10 | 16% | 29% | 43% |

**Brier scores, origin-equal:**

| Method | u0, 30 d | u0, 90 d | u10, 30 d | u10, 90 d |
|---|---|---|---|---|
| V2 | 0.028 | 0.093 | 0.132 | 0.185 |
| CP-noKF | 0.028 | 0.092 | 0.131 | 0.185 |
| B-EQp | 0.013 | 0.031 | 0.124 | 0.168 |
| B-SRp | 0.020 | 0.057 | 0.139 | 0.188 |
| CLIM | 0.007 | 0.014 | 0.192 | 0.250 |

- **For u0, no method beats climatology.**
- **For u10, every method does**, by a Brier skill of 0.25–0.36 at 30 and
  90 days. B-EQp is best at 30 and 90 days; V2, CP-noKF and B-SRp are best
  at 7.
- **Reliability** (`reliability.pdf`): every forecasting method
  over-predicts the events, for both thresholds and at every horizon.

## S6. Ablating V2 (`s6-v2-ablations/`)

Skill of each variant against V2:

| Variant | 7 d mean | 30 d mean | 90 d mean | 90 d pooled |
|---|---|---|---|---|
| No detector | −0.16 | −0.19 | −0.46 | −0.34 |
| Fixed noise priors | +0.009 | +0.006 | +0.002 | +0.000 (n.s.) |
| No spend floor | 0.000 | 0.000 | 0.000 | 0.000 |
| V0 (no sparse spending) | −0.02 | −0.26 | −0.56 | −0.36 |
| CP-noKF (no filter) | −0.04 | −0.04 | −0.05 | −0.04 |

## S7. B-GLOBAL without calendar look-ahead (`s7-bglobal/`)

G-cf is calendar-forward (78,115 of 83,565 forecasts). G-cut is trained
once, on outcomes before 1996-07-01 (57,201 forecasts).

**Against the original B-GLOBAL**, on common origins (30 and 90 days):

| Model | Account mean | Pooled |
|---|---|---|
| G-cf | −0.026, −0.032 | −0.031, −0.042 |
| G-cut | −0.046, −0.066 | −0.048, −0.079 |

**Against V2**, V2's skill is −0.66 and −1.08 against G-cf.

**Against B-EQ**, G-cf's skill is +0.22 and +0.30.

**90% coverage:**

| Model | 30 d | 90 d |
|---|---|---|
| G-cf | 81% | 77% |
| Original B-GLOBAL | 85% | 84% |
| V2 | 95% | 94% |

## S8. Origin phase (`s8-phase/`), reduced version

V2, B-EQ and B-SA+ on the frozen grid shifted by +10 and +20 days (about
27,600 and 27,400 origins). The offset-0 check reproduces the frozen rows
(27,945 of 27,945 identical, on the first 100 accounts).

**Every sign holds:** V2 beats B-SA+, and B-EQ beats V2 at 30 and 90 days,
with intervals excluding 0 under both aggregations. **The magnitudes
depend on the phase.** V2 against B-EQ, pooled (30 and 90 days):

| Grid | 30 days | 90 days |
|---|---|---|
| Frozen | −0.12 | −0.20 |
| +10 days | −0.05 | −0.11 |
| +20 days | −0.04 | −0.10 |

V2 against B-SA+, pooled, rises from +0.27 to +0.31–0.33 at 30 days.
