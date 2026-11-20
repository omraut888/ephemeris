# Revision plan after the external review (2026-11-21)

Tier 1 (the audit, corrections and claim narrowing) is done. See
`paper/results/audit/README.md` and the commits after `dcd6919`.

This file is **a plan only**. Nothing in it has been run.

**Rules for every analysis below:**
- It is exploratory, not confirmatory.
- It is designed and run on the **development** split first.
- Held-out use is a later, separate step. Each held-out run goes through
  `ephemeris-eval`'s guard with `--rerun-reason`, after the user approves
  it, and its results are labelled exploratory in the paper.
- No model, baseline or parameter of the frozen configuration (PLAN.md
  §10) changes. New methods are added beside the frozen ones.

**Common metrics** (unless an item says otherwise):
- CRPS skill against B-SA+, both as the account mean and as the pooled
  ratio, with account-bootstrap intervals (2,000 resamples);
- mean CRPS in normalized units;
- pooled coverage at 50, 80, 90 and 95%;
- one-sided 90% misses;
- median 90% width;
- PIT histograms;
- all at 7, 30 and 90 days, on one common origin set per comparison, with
  counts reported.

Runtimes are estimates from measured runs on this Mac (8 cores):

| Run | Measured time |
|---|---|
| Development backtest | about 4 min |
| V0–V4 on development | about 30 min |
| Synthetic panel | about 3 min |
| One LightGBM fit-predict on all development features | about 30 min, single-threaded |

## T2.1 Explaining the reversal (B-EQ against V2)

**Question.** Why does an empirical distribution of the account's own
balance changes have lower CRPS than V2 on Berka, but not on our
synthetic data?

**(a) Decomposition on development.** Each method below uses the same
365-day lookback and the same origins:
1. B-EQ as frozen: the empirical distribution of past h-day changes.
2. Calendar-conditioned B-EQ: past h-day changes from origins at the same
   phase of the month (day of month ± 3), or of the detected pay cycle
   when there is one.
3. Schedule plus resampled residuals: B-SA's deterministic schedule
   (paychecks and bills), plus a block bootstrap of the account's daily
   residual flows (all flows not on the schedule), in 7-day blocks.
4. Compound Poisson without the Kalman state: V2's spending model
   (Gamma–Poisson events, resampled amounts) plus the detected schedule,
   started from today's booked balance. No filter, no `c`, no adaptive
   noise.

Runtime: items 1–3 are cheap (minutes); item 4 is about 15 minutes on
development.

**(b) Simulation.** A new generator setting varies two factors
independently, each at 4 levels, 50 seeds per cell:
- **Transaction sparsity:** expected discretionary events per month, from
  card-heavy to Berka-like.
- **Schedule regularity:** pay-date jitter and bill drift, from exact to
  irregular.

It runs V0, V2, B-EQ, calendar-conditioned B-EQ and items 3–4 of (a), and
reports skill against B-SA+ for each cell. The question: does the B-EQ
ranking flip as sparsity rises and regularity falls? Runtime: about 20
minutes.

**(c) Ablate V2 itself, not V0, on development:**
- V2 without adaptive noise;
- V2 without the detector;
- V2 without the spend floor;
- V2 with its sparse spending replaced by V0's smooth rate (that is V0);
- the "empirical schedule-plus-spend simulator": item 4 of (a), which is
  V2 with the filter removed.

Runtime: about 40 minutes.

## T2.2 Overdraft events

**Event.** For a threshold `u`, the event is
`min over days 1..h of the booked end-of-day balance < u`:
- `u = 0`;
- `u = the account's 10th percentile of daily balances` over the 365
  days to the origin.

**What gets scored.** The forecast probability `p = P(event)` at h = 7, 30
and 90:
- Brier score, and Brier skill against climatology (the account's
  historical frequency);
- reliability diagrams with 10 bins, with bin counts;
- the event's prevalence.

**Methods:**
- **V2:** the engine's `below_by` target curve, the share of Monte Carlo
  paths whose running minimum falls below `u` by each day, from the same
  paths and seed. This needs only the target option set; the forecast is
  unchanged.
- **Empirical competitors with coherent paths:**
  - B-EQ-paths: past h-day balance *paths* (not just end points), shifted
    to today's balance;
  - item 3 of T2.1(a).
- **Not B-GLOBAL**, which has no paths.

Development first. Runtime: about 1 hour, mostly V2's paths.

## T2.3 Separating "pooled training" from "local-first execution"

**(a) Calendar-forward B-GLOBAL.** For each evaluation date, train only on
development origins whose outcome date (origin + h) is on or before it.
- Retrain quarterly, about 20 refits per horizon.
- Each refit is LightGBM with the frozen hyperparameters.
- This removes B-GLOBAL's calendar look-ahead.

Runtime: 3–6 hours single-threaded; about 1 hour with LightGBM's threads.

**(b) A deployable on-device variant.** Train once, on a separate cohort:
development accounts, with data up to a fixed cutoff date. Freeze the
model, ship it, and apply it to other accounts' origins after the cutoff.
- This is what a local-first app could do with a pre-trained model.
- Compared with (a) and with the frozen B-GLOBAL, it separates the effect
  of pooling from the effect of look-ahead.

Runtime: about 1 hour.

## T2.4 Origin-phase sensitivity

Development split:
- the frozen 30-day origin grid at offsets of 0, 10 and 20 days;
- and weekly origins.

Methods: V2, V1, B-EQ, B-SA+ and B-GLOBAL (cross-fitted).

Question: do the coverage and the sign of each skill hold? Runtime: about
1.5 hours (V2 and V1 dominate).

## T2.5 Eligibility sensitivity

Development split:
1. **A strictly past-only drift rule.** An origin is eligible only if the
   account's drift *up to the origin* stays within 100 crowns. This
   replaces the full-history account exclusion.
2. **Who the exclusions remove.** Compare excluded and kept accounts on:
   - records per month;
   - the coefficient of variation of balance changes;
   - the share of origins with detected income.
   
   Do this for both the drift exclusion and the minimum-history and
   minimum-records rules. Do they remove volatile accounts
   disproportionately?
3. Rerun V2, B-EQ and B-SA+ under rule 1.

Runtime: about 30 minutes.

## T2.6 Small additions

- **How often V1's adjusted levels cross** (audit A5): count the crossings
  on development. Minutes.
- **A pooled-skill selection rule, as a sensitivity check:** would the
  §9 rule choose differently with pooled CRPS? Minutes, from existing
  rows.

## Order and total

1. **T2.5 and T2.6 first.** They are cheap, and T2.5 tells us whether the
   sample itself is biased before we interpret anything else.
2. **T2.1 (a) and (b).** This is the paper's main open question, and the
   explanation the review found missing.
3. **T2.2.** Overdraft events are what the application is for, and the
   current paper cannot speak to them.
4. **T2.1 (c) and T2.3.** They refine the interpretation.
5. **T2.4.** A robustness check.

**Total compute:** about 6–9 hours on this Mac, most of it T2.3(a) and
the V2/V1 runs. **Implementation:** about 2–3 days of work. Results go to
`paper/results/exploratory/`, each with a README that states its split,
its status (exploratory) and the date.

# Tier 3 (a note)

**The most valuable new evidence would be a modern, multi-account data
set.** It would need card-heavy spending, pending transactions and
authorized dates, and many accounts with daily records. It would test the
settled-checkpoint design and V2 where the sparse rule does not fire, and
would show whether the B-EQ result is a property of sparse booked
accounts.

Without such data, the paper's claims are narrowed to **sparse, booked
bank accounts**. A title that says so:

> **When Does Transaction History Beat a State-Space Model? Probabilistic
> Balance Forecasting on Sparse Bank Accounts.**

# Specifications (fixed 2026-11-21, before any of these analyses ran)

**Common to every analysis:**
- **Data.** Development split only.
- **Frozen origins.** The 27,855 eligible origins in 866 accounts, with
  the drift exclusion, unless the item says otherwise.
- **Information.** Each method sees only records dated on or before the
  origin, and booked end-of-day balances on or before it.
- **Units.** Normalized, with the frozen per-origin `k`.
- **Lookback.** Every new method uses `L = 365` days, truncated at the
  account's first day.
- **Seeds.** Monte Carlo methods use `seed_for(account, origin)` and
  1,000 paths.
- **CRPS comparisons.** Coverage at 50/80/90/95 (pooled), mean CRPS,
  one-sided 90% misses, median 90% width, and CRPS skill. Skill is given
  as both the account mean and the pooled (origin-equal) ratio, each with
  95% intervals from 2,000 bootstrap resamples of accounts (seeds on
  synthetic data), on the origins all compared methods share, with
  counts.

**Strength of evidence**, fixed here:
- **Strong:** the interval excludes 0 under both aggregations at both 30
  and 90 days.
- **Weak:** it does so under one aggregation or at one horizon.
- **None:** otherwise.

## S1 (T2.5) Eligibility and exclusions

**Population.** All 890 development accounts, before any exclusion.

**Frozen rule F (the reference):**
- the grid is every 30 days from day 180;
- at least 12 records in the 180 days before the origin;
- origin + 90 is on or before 1998-12-31;
- the scale `s` is above 0;
- the account is excluded if its full-history drift exceeds 100.

**Past-only rule P.** The same grid and conditions, with no account
exclusion. An origin is eligible only if the drift *through the origin*
is at most 100. The drift through the origin is the maximum, over days
with records on or before it, of the minimum |running sum − reported
balance|.

**Who the exclusions remove.** Reported as medians and quartiles:
- **G1:** the 19 drift-excluded accounts against the 871 kept.
- **G2:** kept accounts with no eligible origin against those with one.
- **G3:** grid origins failing the minimum-records rule (all other
  conditions met) against eligible origins.

Measures:
- *Accounts:*
  - records per month, over the full history;
  - volatility: the standard deviation of 30-day booked balance changes,
    over all days, divided by the median monthly gross flow over the full
    history;
  - the share of grid origins with a detected income.
- *Origins:* records in the prior 180 days, and the standard deviation of
  daily balance changes over the prior 180 days, divided by `s` at the
  origin.

**Rerun under P.** Run V2, B-EQ and B-SA+ on all P-eligible origins with
the frozen code, and compare with the same methods on F. Metrics: pooled
coverage, and skill V2 vs B-SA+ and V2 vs B-EQ (both aggregations).

## S2 (T2.6) Small additions

**(a) V1 crossings.** At every frozen origin and horizon, check whether
V1's adjusted levels are non-decreasing in the nominal level. Count the
cases where they are not, and the share of V1's quantile values that the
sort reorders.

**(b) Selection-rule sensitivity.**

*Correction, recorded before running.* Tier 2 item T2.6 said "pooled CRPS".
The §9 score (mean normalized CRPS over all forecasts) is already
origin-equal. The sensitivity is therefore the **account-equal** score:
- per horizon, the mean over accounts of each account's mean normalized
  CRPS;
- then averaged over 7, 30 and 90.

Qualification is unchanged. Inputs are the frozen development rows.

## S3 (T2.1a) The reversal, decomposed on Berka

**Methods**, on the frozen origins:
- **From the frozen rows:** V0, V2, B-SA+, B-EQ.
- **B-EQc:** B-EQ restricted to past start days `d` whose day of the
  month is within ±3 of the origin's. Day-of-month distance is circular
  over 31 days. With fewer than 5 such days, it falls back to all `d`;
  fallbacks are counted.
- **B-SR (schedule plus empirical residuals):**
  - *The schedule's flows.* The paychecks of the detected income stream,
    when the schedule detects one, and the scheduled records whose stream
    is a detected bill.
  - *Future.* `sched(h)` = B-SA's projection, given only those records,
    so its 30-day "other flows" term is 0.
  - *Past residual.* `R_d = (bal[d+h] − bal[d]) −` the schedule's flows
    dated in `(d, d+h]`.
  - *Forecast.* `bal[t] + sched(h) + R_d` over all `d` in `[t−L, t−h]`,
    as an empirical sample.
- **CP-noKF (compound Poisson without the filter):**
  - Paths start at today's booked balance and add the schedule's flows
    cumulatively by day (`sched(k)`, k = 1..90).
  - Discretionary events: each path draws `λ ~ Gamma(n + ½, rate D)`,
    where `n` is the number of discretionary-tagged records in the
    window `(o − L, o]` and `D` its length in days.
  - Each day has `Poisson(λ)` events, whose signed amounts are drawn
    uniformly with replacement from the window's discretionary amounts.
  - 1,000 paths; quantiles at the frozen Monte Carlo levels (type 7).
  - This is V2's spending model, with no Kalman state, no adaptive noise
    and no detector.

**Explanations and their tests**, fixed before running:
- **Calendar alignment:** B-EQc against B-EQ.
- **Schedule plus empirical residuals suffices:** B-SR against B-EQ and
  against V2.
- **Sparse event counts / the filter:** CP-noKF against V2, and against
  B-EQ.
- **History length:** V2 against B-EQ by history stratum (<12, 12–23,
  ≥24 months) on the frozen rows. It is supported if B-EQ's advantage
  grows with history, with a strong or weak contrast between the
  first and last strata.

## S4 (T2.1b) Independent simulation

**Generator.** The realistic generator, 730 days, origins every 30 days
from day 180 (16 per seed), seeds 0–49 in every cell. The cells cross two
factors:
- **Sparsity.** `mean_count` ∈ {2.5, 0.5, 0.15, 0.05} purchases a day, at
  the same mean daily spend.
- **Regularity.** Levels 0–3 of four settings:

  | Setting | Level 0 | Level 1 (the default) | Level 2 | Level 3 |
  |---|---|---|---|---|
  | paycheck jitter probability | 0 | 0.3 | 0.6 | 0.9 |
  | maximum jitter (days) | 1 | 1 | 3 | 5 |
  | paycheck amount sd | 1 | 50 | 150 | 300 |
  | bill drift probability | 0 | the defaults | 0.5 | 0.9 |

**Methods:** V0, V2 (the frozen sparse rule), B-SA+, B-EQ, B-EQc, B-SR and
CP-noKF. They use the available-balance series and the snapshot's tags,
are scored against the true balance, and have L = 365.

**Metrics, per cell:**
- skill against B-SA+, and V2 against B-EQ (both aggregations; seed
  bootstrap);
- 90% coverage;
- the share of origins where V2's sparse rule fired.

**What would support the explanation:** B-EQ's skill against V2 rises
with sparsity and with irregularity, and the Berka-like corner reproduces
the reversal.

## S5 (T2.2) Overdraft events

**Thresholds** (normalized units): `u0 = 0`, and `u10` = the type-7 10th
percentile of the booked balances over the L days to the origin.

**Event.** `E_h(u)`: the minimum booked end-of-day balance over days
1..h after the origin is below `u`, for h = 7, 30 and 90.

**Methods (probabilities):**
- **V2:** the engine's `below_by` for targets `[u0, u10]`, with the frozen
  configuration and seed. A check confirms its quantiles equal the frozen
  rows.
- **CP-noKF:** the share of paths whose running minimum falls below `u`.
- **B-EQp:** over past start days `d ∈ [t−L, t−h]`, the share of shifted
  past paths `bal[t] + bal[d+k] − bal[d]` (k ≤ h) whose minimum falls
  below `u`.
- **B-SRp:** the same, with paths `bal[t] + sched(k) +` (the residual
  flows over `(d, d+k]`).
- **CLIM:** the share of past windows `(d, d+h]` in which the account's
  own balance fell below `u`. The account's climatology.

**Metrics:**
- prevalence, pooled and account-mean;
- Brier score, origin-equal and account-equal, with account-bootstrap
  intervals;
- Brier skill against CLIM;
- reliability: 10 equal-width bins of `p`, with the mean `p`, the
  observed frequency and the count per bin, plotted.

If a prevalence is below 0.5%, its results are marked unreliable.

## S6 (T2.1c) Ablating V2

**Variants**, on the frozen origins:
- V2 as frozen;
- V2 without adaptive noise (both channels fixed at their priors);
- V2 without the detector;
- V2 without the spend floor;
- V2 without sparse spending (= V0, frozen rows);
- V2 without the filter (= CP-noKF).

**Metrics:** skill against V2, both aggregations, and the coverage
difference.

## S7 (T2.3) B-GLOBAL without calendar look-ahead (development)

Features are the frozen development features. Hyperparameters and levels
are frozen, LightGBM runs single-threaded, and fits run in parallel
processes.

- **G-orig:** the frozen cross-fitted rows.
- **G-cf (calendar-forward).** For fold k and each quarter
  `[C_q, C_{q+1})`, with quarters starting 1994-01-01 and ending
  1998-10-01:
  - the training rows are the other folds' rows with outcome date
    (origin + h) before `C_q`;
  - the model predicts fold k's origins in that quarter;
  - with fewer than 1,000 training rows, there is no forecast.
- **G-cut (a model trained once).** Training rows: other folds with
  outcome before 1996-07-01. It predicts fold k's origins on or after
  1996-07-01.

**Comparison.** On the origins all compared methods share, with counts:
G-orig, G-cf, G-cut, V2, B-EQ and B-SA+. Metrics as above.

## S8 (T2.4) Origin phase: only if under 30 minutes

**A reduced version, recorded before running.** V2, B-EQ and B-SA+ on the
frozen rule's grid shifted by +10 and +20 days. V1 (its calibration grid)
and B-GLOBAL (the fits) are dropped to stay under 30 minutes.

## Changes and notes recorded during the runs (2026-11-21)

- **S1.** Rule P selected exactly the same origins as rule F on
  development: 27,855 each, none added or removed. Every drift-excluded
  account already drifts by at least 500 at its first grid origin. The
  planned rerun under P is therefore identical to F and was not repeated.
- **S3, the summary script.** The first version renamed the fresh V2 rows
  before filtering by method name, so the "V2 identity" check compared
  nothing (0/0). Fixed before any conclusion was drawn. The comparisons
  themselves used the frozen V2 rows throughout. After the fix, 83,564 of
  83,565 fresh V2 rows equal the frozen ones. The one that differs
  (account 5984, 1996-03-30, h = 90) differs in the last digit of its
  median, with identical coverage, CRPS and PIT. This is the post-freeze
  `rand_distr` fix (paper §8): the exploratory runs use the fixed engine.
- **S8.** Added offset 0 to the shifted grids before running, as a check:
  its rows must equal the frozen development rows. This adds no new
  analysis.
- **S8, runtime.** The first S8 run took about 12 minutes per offset.
  Three full offsets would have exceeded the 30-minute limit. It was
  stopped during offset 0 and rerun with offsets +10 and +20 in full, and
  the offset-0 identity check on the first 100 development accounts only.
