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

# Round 3: the second referee report (specifications fixed before each run)

## S9 (round 3, A1) Why removing the detector costs V2 so much (development)

**Question.** Removing the detector costs V2 an account-mean skill of
−0.21 / −0.49 at 30 / 90 days, but V0 only −0.02 / −0.04 (held-out).
V2's sparse branch replaces the daily spending draw that uses the
filtered rate c. Is the difference a bug or a mechanism?

**Origins.** The frozen development origins (27,855 in 866 accounts),
restricted to those where all four configurations below forecast.

**Configurations.** Same records, scale `k`, seed `seed_for(account,
origin)` and 1,000 paths for each:
- V0: the tracker and cone as frozen, `sparse_spending = None`;
- V0-nodet: V0 with `spend_change = None` and `income_change = None`
  (the frozen ablation's definition);
- V2: as frozen (the sparse rule decides the branch);
- V2-nodet: V2 with the same two detectors removed.

**Identity checks**, which must pass before anything is read:
- V2 and V2-nodet equal the stored S6 rows;
- V0 equals the frozen development V0 rows;
- V0-nodet equals the frozen development `abl:no-detector` rows, on the
  ablation-sample accounts where those exist.

**Recorded per origin and configuration:**
- the branch actually used (sparse or not), and whether income was
  detected;
- the filtered state (b, c, m), the square roots of P's diagonal, and the
  income and spend noise estimates (value, dof);
- income observations gated in the 365 days before the origin, and income
  and balance alarms (ever, and in the last 28 days);
- for each horizon: CRPS, median, 90% width and the outcome.

**Retrospective, diagnostic only:** the last observed paycheck before the
origin, and the mean paycheck tagged in (origin, origin + 90] from the
full record.

**Reported:**
- the mean paired CRPS difference (absolute, normalized units), pooled
  skill and account-mean skill, each with 95% account-bootstrap intervals,
  for V2-nodet vs V2 and V0-nodet vs V0;
- the same split by income detected (yes / no) × any income alarm before
  the origin in the configuration with the detector (yes / no), and by an
  alarm in the last 28 days (the frozen stratum);
- the bias (mean of median − outcome) per configuration and horizon;
- |m̂_nodet − m̂| and |m̂ − last paycheck| by stratum;
- the change in ĉ between V0 and V0-nodet.

**What counts as a bug** (stop and report before fixing anything):
1. branch activation differs between a configuration and its no-detector
   version;
2. seeds or origin support differ;
3. the no-detector configuration differs from the frozen ablation's in
   anything but the two detectors;
4. a code path reads detector state when the detector is `None`.

**Hypothesis, stated before running.** The income gate skips the update of
m whenever a paycheck's normalized innovation exceeds the gate, and only an
alarm (a jump in m's variance) lets m move to a new level. Without the
detector, a lasting change in pay is gated at every paycheck, so m̂ stays
stale. V0 can absorb the error through c, which the balance channel keeps
updating; V2's sparse branch ignores c.

Predictions:
- V2's loss concentrates in accounts with detected income and an income
  alarm before the origin;
- m̂ differs between V2 and V2-nodet there, and is far from the last
  paycheck in V2-nodet;
- in V0-nodet, ĉ moves in the direction that offsets the stale m̂.

Code: `ephemeris-eval explore detector`. Summary:
`paper/analysis/explore_summary.py s9`. Results:
`paper/results/exploratory/s9-detector/`.

### S9: additions after the first run (recorded before the rerun)

The first run passed every identity check, and no bug criterion was met:
- V2, V2-nodet, V0 and V0-nodet reproduce the stored rows exactly;
- the branch is the same with and without the detector at every origin;
- V0 and V2 share the tracker state exactly.

The whole V2 effect sits in accounts with detected income and an income
alarm, where m̂ without the detector is a median 10% of the last paycheck.
Two paths need checking before the mechanism can be stated:
1. **Initialization.** Whether the alarms that matter fire early in the
   history, acting as the income state's initialization, not as later
   regime changes.
2. **V0's compensation.** Whether V0 loses less because the payday surplus
   becomes balance-gate lumps, which V0's cone resamples and V2's sparse
   branch does not.

The rerun adds, per origin and configuration:
- the day of the first and last income alarm, relative to the first day of
  history and to the origin;
- the first two tagged paychecks;
- the number of recorded lumps, their sum and the days they cover.

Nothing else changes, and the identity checks are repeated.

## S11 (round 3, B1) The schedule-replacement contrast (development)

**Origins and windows.** The frozen development origins. At origin t and
horizon h, the windows are B-EQ's: every d with lo ≤ d ≤ t − h, where
lo = max(first day, t − 365).

**Forecasts.** Each is an empirical distribution over the windows, equal
mass per window:
- **F_w,** for w ∈ {0, 0.5, 1}: samples B_t + ΔB_d + w·(S_t − S_hist,d).
  - ΔB_d = B_{d+h} − B_d.
  - S_t: the projection over (t, t+h] of the schedule detected at t (as in
    B-SR: `schedule_aware_delta`).
  - S_hist,d: the **realized** flows dated in (d, d+h] of the streams that
    the schedule detected at t contains: income-tagged records if income
    was detected, and the records of each detected bill stream. Tagging is
    on the records through t. This is a retrospective decomposition with
    the origin's schedule, not a projection made at date d. (A projection
    made at d would need a schedule detection on every past day; it is out
    of scope here and named as a limitation.)
  - F_0 is B-EQ and F_1 is B-SR. Both identities are checked against the
    stored S3 rows.
- **Oracle O (diagnostic only; uses the future):** samples
  B_t + ΔB_d − S_hist,d + S_real, where S_real is the realized flows dated
  in (t, t+h] of the same streams. The streams are identified by
  counterparty (operation, k_symbol, bank, account), not by stream id,
  because ids are positions in a sorted list and change as records are
  added. Income records are those tagged income on the records through
  t+h whose counterparty is the income counterparty at t.

**Schedule predictability at the origin (history only).** Checkpoints
c_j = t − 30j, for j = 1..6, with c_j ≥ first day + 90. At each checkpoint:
- tag the records through c_j and detect the schedule;
- list the projected events in (c_j, c_j + 30] (paydays with the last
  paycheck's amount; bill due dates with their amounts);
- list the realized events: records in (c_j, c_j + 30] whose counterparty
  is one of the detected streams' counterparties.

Measures, pooled over the checkpoints:
- **amount error** A = Σ_j |R_j − P_j| / Σ_j |R_j|, where R_j and P_j are
  the realized and projected net flows (undefined if Σ_j |R_j| = 0);
- **date error:** the mean |days| from each projected event to the
  nearest realized event of the same stream within ±10 days;
- **missed:** projected events with no realized event of the stream
  within ±10 days, per projected event;
- **extra:** realized events with no projected event of the stream within
  ±10 days, per realized event.

**Strata:**
- **No schedule:** the schedule detected at t has no income and no bills.
- **Short history:** fewer than 3 checkpoints, or A undefined.
- **Tertiles of A** among the rest. The cut points are fixed from the
  development distribution and recorded for later use.

The miss and extra rates are reported by tertile as descriptions, not
used for stratification.

**Reported:**
- for F_1 vs F_0, F_0.5 vs F_0 and O vs F_0: pooled and account-mean
  skill with 95% account-bootstrap intervals, coverage, and one-sided 90%
  misses, overall and per stratum;
- the counts of origins and accounts per stratum.

**The key test (development, exploratory).** Schedule replacement (F_1 vs
F_0) has positive skill in the most predictable tertile and negative skill
in the least predictable, under the strength-of-evidence rule above. If
the oracle helps where F_1 hurts, the loss is in the projection, not in
the replacement idea.

Code: `ephemeris-eval explore schedule`. Summary: `explore_summary.py s11`.
Results: `paper/results/exploratory/s11-schedule/`.

## S12 (round 3, B2) The shortfall audit (development)

**The event, precisely.** E(u, h) = 1 if the booked end-of-day balance is
below u on at least one of days t+1, …, t+h. Day t (the origin) is not
included. Balances are the reconstructed series (opening balance plus
signed amounts), as everywhere in the paper.

**Audit items:**
1. **Already below.** Count the origins with B_t < u, per threshold.
   Report every metric separately for origins starting below the threshold
   and for those at or above it. In the second group the event is a first
   crossing. As a check, also report E with day t included.
2. **Reported against reconstructed labels.** The reported series is the
   bank's balance on the day's last record (file order), carried forward
   over days without records. Count the origins whose u = 0 label differs
   between the two series, per horizon, and give the prevalence under each.
3. **A buffer around zero.** Thresholds u = −δ and u = +δ for δ ∈ {100,
   1,000} CZK (raw), next to u = 0. Report prevalence, Brier scores, and
   the share of u = 0 events whose minimum is in (−δ, 0).
4. **Concentration.**
   - Episodes: maximal runs of consecutive days below 0, over each account's
     evaluated period.
   - Accounts that ever cross, and events per account.
   - Prevalence and Brier skill against CLIM after removing (a) the 5
     accounts with the most u = 0 events at 90 days and (b) the top 1% of
     accounts by that count.
5. **A conditioned climatology, CLIM-D (past-only).** Among the account's
   own past windows (B-EQ's), take the K windows whose starting balance B_d
   is closest to B_t, with K = max(10, ⌈0.2·n⌉) and ties broken by recency.
   CLIM-D is the share of them in which the balance went below u during
   (d, d+h].
6. **V2 recalibrated.** Logistic recalibration of logit(clip(p, 10⁻⁴,
   1 − 10⁻⁴)), with two parameters per threshold and horizon, cross-fitted
   over 5 account folds (fold = splitmix64(account) mod 5) and evaluated
   out of fold. An isotonic recalibration, cross-fitted the same way, is a
   second variant. The full-development fit is recorded for any later
   confirmatory use; it is not applied to held-out data here.
7. **A calibration–resolution decomposition.** The Murphy decomposition with
   ten equal-width bins: Brier = REL − RES + UNC + a within-bin term,
   reported so that the parts add up to the Brier score. For every method,
   threshold and horizon; for all origins and for origins starting at or
   above the threshold.

**Methods scored:** V2, CP-noKF, B-EQp, B-SRp, CLIM (as in S5), CLIM-D,
and V2 recalibrated. Every probability is recomputed for the new
thresholds. At u = 0 and the tenth percentile they must equal the stored
S5 events (an identity check).

**Intervals:** 95% account-bootstrap intervals, 2,000 resamples, seed 6.

Code: `ephemeris-eval explore shortfall`. Summary:
`explore_summary.py s12`. Results:
`paper/results/exploratory/s12-shortfall/`.

# Tier C (plan only; not run)

## C1 The simulation factorial

**Factors**, each at the four S4 levels, varied **one at a time** with the
others at level 0 (exact), and also crossed in a 4×4×4 design at the two
extreme sparsity levels:
- pay-date jitter;
- paycheck amount noise;
- bill drift.

**Sparse branch:** forced on and forced off in every cell (bypassing the
sparse rule), so the branch effect is separated from the regularity effect.

**Horizons:** 7, 30 and 90 days.

**Reported per cell:**
- the generator parameters;
- the rate at which the sparse rule would fire, as a branch-activation
  rate;
- V2 vs B-EQ pooled and account-mean skill, with 2,000-resample seed
  bootstrap intervals;
- the same for CP-noKF.

**Controls:**
- **A sample-size-matched calendar control:** B-EQ restricted to a random
  subset of windows of the same size as B-EQc's, to separate alignment
  from losing windows.
- **Alignment to the next expected paycheck:** B-EQ windows that start the
  same number of days before a paycheck as the origin is before the next
  expected one.

**Size and time.** One-at-a-time: 3 factors × 4 levels × 4 sparsity × 2
branch settings = 96 cells. Crossed: 64 × 2 sparsity × 2 branch settings =
256 cells. In total 352 cells, each with 50 seeds and 800 origins as in S4: about
280,000 origins at 3 horizons. S4 ran its 16 cells in 134 s on 7 threads (`paper/build/explore/run.log`), about
8.4 s per cell, so the 352 cells take about 50 minutes; the sparse-branch
setting and the added 7-day scores change that little. The two calendar
controls reuse B-EQ's windows on Berka development data, about 15 minutes.
Total, with summaries: about 1.5 hours. (Recorded on 2026-11-21; S4's
runtime is from its log.)

### S11: oracle definition corrected after the first run (recorded before the rerun)

The first run's oracle lost even to F1 at 90 days (pooled skill O vs F1
−0.16), which a correct oracle should not do. The cause is the definition:
- S_hist takes only income-tagged records (inflows of the income
  counterparty) and the records of detected bill streams;
- S_real as specified took **every** record of those counterparties,
  including outflows to the income counterparty and records of a bill's
  counterparty that are not part of the stream.

The two were therefore not the same quantity.

**Corrected:** S_real is the sum over (t, t+h] of the records that the
tagging of the records through t+h marks as income, when income was
detected at t and the record's counterparty is the income counterparty at
t, or as scheduled with a counterparty among the detected bills'. This is
the rule that defines S_hist, applied to the later tagging.

F_w and the predictability measures are unchanged, and only the oracle's
rows are expected to change. The first run's oracle results are kept in
`s11-schedule/first-run-oracle/`, labelled superseded.

# Round 4: the user's decisions on the round 3 checkpoint

## S13 (round 4, item 1) The paycheck channel without the detector (development)

**Hypothesis (from S9).** The income channel starts from a small first
income record. A confident posterior then puts the real paychecks far
outside the gate, so they skip the paycheck update. R_inc's adaptation
(samples capped at gate·S) inflates R until the paychecks pass the gate,
but by then the Kalman gain P_mm/(P_mm + R) is tiny, because P_mm grows
only by q_m per day, so m̂ creeps.

**Trace.** At each development account's last eligible origin, ingest the
records twice, with and without the detectors (V2's tracker; V0's is the
same). For every income observation, in order, record from the ingest
report: the innovation ν, its variance S, the NIS, whether it was gated,
and the amount, giving m̂ before the update = amount − ν. The gain is
estimated as K_i = (m̂_{i+1} − m̂_i)/ν_i (the m random walk has mean zero).

**Per account:**
- the number of income observations, the first and last amounts, and the
  gated share among the first 12 and over all observations;
- the first observation after the first at which |m̂ − amount| < 20% of
  the amount;
- the median K over the last 12 observations;
- the final S relative to the last paycheck squared;
- the final m̂ relative to the last paycheck.

These are summarized by the S9 stratum (an income alarm with the detector
on, or none).

**V0's smaller bias.** From S9's stored results: the change in the 90-day
median bias caused by removing the detector, for V0 and for V2, in the
income-alarm stratum. If V0's smaller bias is a different starting point
rather than compensation, the two changes are of similar size.

**The fix's effect (after the post-freeze fix, development only).**
- Rerun V0 and V2 as frozen (detector on) on the first 100 development
  accounts with the fixed engine, and compare with the frozen rows: the
  number of forecasts changed; the mean CRPS, the 90-day median bias and
  the coverage at each level, before and after.
- Rerun S9 on all development origins with the fixed engine, for the
  detector-off effect.

The frozen results are not regenerated.

Code: `ephemeris-eval explore income-trace`; summary `explore_summary.py
s13`. Results: `paper/results/exploratory/s13-income/`.

## S14 (round 4, item 2) Within-window dependence (development)

**Hypothesis.** B-EQ wins at long horizons because each sampled past window
keeps its scheduled flows and the discretionary flows that followed them
together. Schedule-based methods recombine the two independently.

**Notation**, on B-EQ's windows d: ΔB_d = S_hist,d + D_d, where S_hist,d is
S11's retrospective decomposition and D_d = ΔB_d − S_hist,d is B-SR's
adjusted residual. S_t is the current projection.

**Forecasts** (all on the frozen development origins, h ∈ {7, 30, 90}):
- **(a) EQ-shuf.** Samples B_t + S_hist,d + D_π(d) over the windows d, for
  four random permutations π_1..π_4 of the windows (ChaCha8, seed
  `seed_for(account, origin)` + r): 4n samples. Both components keep
  exactly B-EQ's marginal distributions; only their within-window pairing
  is broken.
- **(b) As stated, an identity.** B-SR's residual D_d paired with the same
  window's S_hist,d is B_t + D_d + S_hist,d = B_t + ΔB_d, which is B-EQ.
  It is computed and reported as an identity check against F0, not as a
  test.
- **(b′) SR-match, the operational reverse.** B-SR with the dependence
  restored by matching. Samples B_t + S_t + D_d over the K windows whose
  S_hist,d is closest to S_t, with K = max(10, ⌈0.2·n⌉) and ties broken by
  recency. A residual is then used only with a scheduled flow like the one
  it came with.
- **(b″) O-match.** The same with the oracle's S_real in place of S_t
  (diagnostic only).

**Reported:** pooled and account-mean skill with 95% account-bootstrap
intervals at 7, 30 and 90 days, for:
- EQ-shuf vs F0 (= B-EQ);
- SR-match vs F1 (= B-SR) and vs F0;
- O-match vs O and vs F0;
- (b) vs F0 (an identity).

Coverage and one-sided misses are reported too, overall and by S11's
predictability tertile.

**Predictions, stated before running:**
- **D1.** EQ-shuf loses to B-EQ at 30 and 90 days under both aggregations
  (if dependence is the mechanism).
- **D2.** SR-match beats B-SR at 30 and 90 days under both aggregations.

The strength-of-evidence rule is the one above.

Code: added to `ephemeris-eval explore schedule`, whose F_w and oracle rows
must stay identical. Summary: `explore_summary.py s14`. Results:
`paper/results/exploratory/s14-dependence/`.

## S15 (round 4, item 3) The forward-period confirmatory test (held-out; NOT RUN)

Fixed on 2026-11-21, after the development results of S11 and S14 and before
any held-out run of these methods.

**Data.** Held-out accounts (the frozen split and drift exclusion), at
their eligible origins dated **on or after 1997-07-01**: 49,762 origins per
horizon in 3,512 accounts. These were counted from the stored held-out
rows' origin dates only, without outcomes.

**Run.** Once, through the guard, with the reason "round 4 forward-period
confirmatory test (REVISION.md S15)":

    target/release/ephemeris-eval explore schedule --held-out --from 1997-07-01 \
        --out paper/build/confirmatory-s15 \
        --rerun-reason "confirmatory forward-period test S15 (P1 secondary, P2, P3, P3b); specification committed before the run"

The code is the schedule runner at `ee45141` or later, with no change to
F0, F1, O, EQ-shuf or the predictability measure. The summary is
`explore_summary.py s15 --heldout`, to be written and committed before the
run. Results go to `paper/results/confirmatory-s15/`.

**Methods** (as defined in S11 and S14, unchanged): F0 (= B-EQ), F1
(= B-SR), O (the corrected oracle) and EQ-shuf.

**Predictability tertiles** use the development cut points as stored,
amount error ≤ 0.1428 (predictable) and > 0.4127 (unpredictable), computed
from each origin's own history (S11). The cut points are not re-estimated.

**Estimands.**
- **Primary:** the account-mean CRPS skill (the plan's original estimand),
  with a 95% interval from 2,000 bootstrap resamples of accounts (seed 6).
- **Secondary:** the pooled skill, with the same intervals.

Both are reported for every prediction, with counts.

**Predictions** (fixed from the development results; dev estimates in
brackets, account mean then pooled):
- **P2 (primary).** The oracle O loses to B-EQ at 90 days: the
  account-mean skill of O against F0 is < 0. (Development: −0.43
  [−0.48, −0.38]; pooled −0.20.)
- **P3 (primary).** Breaking the within-window pairing costs B-EQ a little:
  the account-mean skill of EQ-shuf against F0 is < 0 at both 30 and 90
  days. (Development: −0.012 [−0.015, −0.009] and −0.013 [−0.017, −0.010];
  pooled −0.003 and −0.006.)

  Stated as it came out of development, the effect is small, a few
  percent of B-SR's loss. P3 tests its sign, not its size, and confirming
  it would not show that dependence explains the reversal.
- **P3b (primary; amendment of 2026-11-21, before the run).** At 90 days,
  under the account mean, shuffled B-EQ's loss against B-EQ is less than
  a quarter of B-SR's loss against B-EQ, on the same post-cutoff origins.
  With s_shuf and s_SR the account-mean skills of EQ-shuf and F1 against
  F0, the test statistic is s_shuf − ¼·s_SR (loss_shuf < ¼·loss_SR ⇔
  s_shuf > ¼·s_SR). It has a 95% interval from resampling accounts jointly
  for both skills, and it is confirmed if the interval lies wholly above 0.
  If B-SR does not lose (s_SR ≥ 0), P3b is reported as not confirmed. This
  is the claim the paper makes, that the dependence effect is small
  relative to the schedule-based loss, so it gets a confirmatory test.
  (Development: s_shuf −0.013 and s_SR −0.34, so the statistic is about
  +0.07.)
- **P1 (secondary).** B-SR's skill against B-EQ is higher in the
  predictable tertile than in the unpredictable one: the account-mean
  skill of F1 vs F0 in the predictable tertile minus that in the
  unpredictable tertile is > 0 at 30 and 90 days. The interval of the
  difference resamples accounts jointly across the two tertiles.
  (Development: +0.011 [−0.072, +0.096] at 30 days, not significant;
  +0.25 [+0.15, +0.36] at 90; pooled +0.22 and +0.42, both significant.
  Development therefore supports P1 at 30 days only on the pooled score.)

**Decision rule.** A prediction is **confirmed** if the primary estimand's
95% interval lies wholly on the predicted side at every horizon it names,
and **not confirmed** otherwise. The pooled result is reported beside it,
and any disagreement in sign between the two is stated. There is no
multiplicity adjustment: there are three predictions, two of them primary,
and this is stated. With P3b there are four predictions, three of them
primary. Nothing is re-run or re-specified after the results
are seen; any further analysis is labelled exploratory.

**Prior exposure, stated in the paper.**
- Before this specification, the held-out results seen were:
  - the frozen evaluation (V0–V4 and the baselines; every table in the
    paper);
  - the post-hoc pairwise intervals, tails and A2 verdicts;
  - the exploratory held-out runs of S3 (B-EQc, B-SR, CP-noKF and V2
    against B-EQ, overall and by history length), S5, S6 and S7.
- Every held-out origin after the cutoff is part of those aggregates. In
  particular, held-out B-SR vs B-EQ over all origins (pooled −0.10 / −0.19
  at 30 / 90 days) is known.
- Not seen: any held-out result split by schedule predictability, for the
  oracle, or for EQ-shuf, and any held-out result restricted to origins
  after the cutoff.
- The cutoff was chosen by the user from the calendar, not from results;
  the origin count after it was checked from dates only.
- The predictions were formed on the development split, whose accounts
  are disjoint from the held-out ones.

## S16 (round 5) Does subtracting S_hist widen the sampled changes? (development; descriptive)

A short descriptive check, offered in the paper as an open hypothesis for
the long-horizon loss, not a conclusion.

**Quantity.** At each frozen development origin and h ∈ {30, 90}, the
5–95% range of the sampled changes:
- ΔB_hist (B-EQ's samples, F0);
- ΔB_hist − S_hist (B-SR's samples less the constant S_t, F1).

These are exactly F0's and F1's stored 90% interval widths (S11 rows), so
nothing is recomputed. The interquartile range is not in the stored rows
and is not computed, to keep the check minimal.

**Per account:** the median over its origins of the ratio of widths (F1 to
F0).

**Reported:** the share of accounts whose ratio exceeds 1 (the adjusted
changes are wider); the quartiles of the ratio over accounts; and the
pooled share of origins with a wider adjusted range. Summary:
`explore_summary.py s16`. Results:
`paper/results/exploratory/s16-dispersion/`.

# Round 6: the third referee report (v1 corrections; stored outputs only)

No new forecasts and no new guarded held-out runs. Everything below is
computed from stored outputs: the S15 held-out rows and state, the S11/S12/S14
development rows, and the S5 held-out events with the logged start balances.
The one exception is CLIM+, a development-only climatology computed from the
development balances (item 5f).

## S17 Analyses from stored outputs (fixed before computing)

1. **Per-tertile absolute skill.** B-SR (F1) against B-EQ (F0) in each
   predictability tertile: account mean and pooled, each with a 95%
   account-bootstrap interval (2,000 resamples, seed 6). On the S15
   held-out origins and on development.
2. **The direct oracle contrast.** Skill of O against F1 at 30 and 90 days,
   both aggregations with intervals, held-out S15 and development.
3. **Origins without a detected schedule.** Count them per tertile. None
   exist on either split (every origin has a detected income or bill), so
   the closer check is reported instead: the share of origins with a
   detected income in each tertile, and item 1 repeated excluding origins
   without a detected income.
4. **The predictability score**, defined exactly in the paper from the code
   (S11): checkpoints, normalization, matching, missing streams, cut points,
   and what it does and does not capture.
5. **New crossings** (held-out, origins starting at or above zero; stored
   events and start balances):
   - a. per method: mean forecast probability, absolute Brier score, Brier
     skill against CLIM with account-clustered intervals (the existing
     split);
   - b. a reliability plot of the four forecasts and CLIM on new crossings
     only: ten equal-width bins, bins with at least 20 forecasts;
   - c. the distinct accounts behind the positive 90-day windows, and
     "crossing clusters": runs of consecutive eligible origins (30 days
     apart) in one account whose 90-day window crossed. These are an
     approximation to episodes from the stored windows; held-out daily
     balances are not re-read;
   - d. label sensitivity to reported against reconstructed balances:
     development only (S12), stated as such;
   - e. CLIM-D, the distance-conditioned climatology: development only
     (S12);
   - f. **CLIM+**, a climatology whose risk set starts at or above zero: the
     share, among the account's past h-day windows starting at or above
     zero, that went below zero. Development only, computed from the
     development balances by `ephemeris-eval explore shortfall` (no other
     change to that runner; the other columns must equal the stored S12
     events). Scored on new crossings with the other methods.

Summaries: `explore_summary.py s17` (development) and
`explore_summary.py --heldout s17`. Results: `s17-stored/` under
`exploratory/` and `exploratory-heldout/`.

# Round 7: the fourth referee report (final v1 round; wording and hygiene)

No new forecasts and no guarded held-out runs. One development-only label
recount (S18), because the S12 label sensitivity was computed over all
origins, not over Table 5's risk set (origins starting at or above zero).

## S18 Label sensitivity on the new-crossing risk set (development; labels only)

- **Origins:** the S12 development origins (`eligible_origins`, development
  split), at 7, 30 and 90 days, threshold 0 only.
- **Balances:** the reconstructed balance (the running sum, as in S12) and
  the bank's reported balance (`reported_balance`: the day's last record in
  file order, carried forward). No forecasts are made.
- **Per origin and horizon:** the start balance under each (`b0`,
  `b0_rep`), and the event under each: some day 1..h below zero (`event`,
  `event_rep`).
- **Reported, separately:**
  1. *eligibility changes:* origins in the reconstructed risk set
     (`b0 >= 0`) that leave it under reported balances (`b0_rep < 0`), and
     origins that enter it (`b0 < 0`, `b0_rep >= 0`), with their events;
  2. *label changes* on origins eligible under both: 0→1 and 1→0;
  3. the new-crossing event count under each definition (each on its own
     risk set).
- **Identity check:** `b0` and `event` must equal the stored S12 events
  (`threshold = u0`) for every origin and horizon.
- Command: `ephemeris-eval explore shortfall-labels` (development only);
  summary `explore_summary.py s18`; results in
  `paper/results/exploratory/s18-labels/`.

# Round 8: the fifth referee report (arXiv v1 additions)

No new forecasts and no guarded runs. One exploratory addition from stored
events (S19); the larger experiments are in the v2 plan below.

## S19 Stronger shortfall references (stored events only; fixed before scoring held-out)

- **Data.** New crossings at threshold 0 (origins starting at or above
  zero), at 7, 30 and 90 days. Development: the stored S12 events
  (`paper/build/explore/shortfall/events.csv`, `threshold = u0`,
  `start_below = 0`). Held-out: the stored S5 events with the logged start
  balances (`explore-heldout/berka/events.csv`, `startbal/startbal.csv`),
  as in the existing split.
- **CONST** (constant): for every origin, the development prevalence of new
  crossings at that horizon, $\pi_h$ = events / origins on development.
- **SHRUNK** (account rate shrunk toward the prevalence):
  $p = w_h \cdot \mathrm{CLIM} + (1 - w_h)\,\pi_h$, where CLIM is the stored
  account climatology (share of the account's past h-day windows in the
  365-day lookback that went below zero). $w_h$ is chosen on development
  only, per horizon, from the grid 0, 0.05, ..., 1, to minimize the mean
  Brier score on development new crossings (ties: the larger $w$). The
  weight does not depend on history length, because the stored events do
  not carry each origin's window count. $\pi_h$ and $w_h$ are written to
  `s19-references/fit.csv` before any held-out score is computed.
- **Scores (held-out, and development for reference):** for CONST, SHRUNK
  and the existing five (V2, CP-noKF, B-EQp, B-SRp, CLIM): mean
  probability, mean Brier score, Brier skill against CLIM with the
  existing account bootstrap (2,000 resamples, seed 6).
- **The strongest reference** for the paper's comparison is fixed here as
  the one of CLIM, CONST and SHRUNK with the lowest development Brier score
  at that horizon. Every forecast also gets Brier skill against it, with
  the same bootstrap. Which reference is lowest on held-out data is
  reported as observed.
- **Arithmetic check:** the referee's figures (CONST at 1.14% on held-out at
  90 days gives Brier about 0.0101 against CLIM's 0.0107) are compared with
  the computed values and reported either way.
- Summary: `explore_summary.py s19` (development fit, then
  `explore_summary.py --heldout s19`). Results:
  `exploratory/s19-references/` and `exploratory-heldout/s19-references/`.

# Round 9: the sixth referee report (text additions; arXiv v1)

No new forecasts, no new scores and no guarded runs. Two text additions
(direct versus iterated multistep forecasting in §2 and a discussion
section; dispersion growth as a hypothesis) and the v2 plan below.

## S20 Width growth from stored medians (arithmetic only; fixed before computing)

- **Inputs:** the frozen held-out median 90% widths in
  `heldout/variants.csv` (`median_width90_czk`; for each method, the median
  over accounts of the account's mean width) for V2 and B-EQ at 7, 30 and
  90 days. Nothing is rerun or rescored.
- **Quantities:** for each method, the ratios width(30)/width(7) and
  width(90)/width(7), and the random-walk references sqrt(30/7) and
  sqrt(90/7). Also, for orientation, the linear-growth reference 90/7.
  Two decimals.
- **Caveat stated with them:** a ratio of medians over accounts is not
  the median of per-account ratios, and widths mix accounts with
  different volatility; the comparison is descriptive and the
  explanations offered for it are untested.
- Computed in `macros.py` (`derived()`), keys `d/wg/<method>/<h>` and
  `d/wg/rw/<h>`, `d/wg/lin/90`.

# Round 10: the seventh referee report (one correction; arXiv v1)

No new forecasts, no new scores and no guarded runs. One paragraph of
§10 is corrected (S21), and the v2 plan is extended.

## S21 A bound on the rate-uncertainty term (arithmetic only; fixed before computing)

- **Model (V2's sparse branch, as implemented):** lambda ~ Gamma(n + 1/2,
  rate = D), where D is the sparse window's length in days (the
  `c/sparse-window` constant, 365, or less with a shorter history), and
  the h-day discretionary sum has Var(S_h) = h E[lambda] E[A^2] +
  h^2 Var(lambda) (E[A])^2. Since Var(lambda)/E[lambda] = 1/D, the ratio of
  the quadratic to the linear term is R_h = (h/D)(E[A])^2/E[A^2] <= h/D.
- **Quantities:** for D equal to the full window, the variance bound h/D
  and the standard-deviation bound sqrt(1 + h/D) - 1, at 7, 30 and 90 days,
  in percent to one decimal. Computed in `macros.py` (`derived()`) from
  the window constant read from the engine, keys `d/rh/var/<h>` and
  `d/rh/sd/<h>`.
- **Scope stated with them:** the bound covers only the discretionary
  component under the model's own assumptions, not V2's full quantile
  widths (which also carry the balance, paycheck and bill terms); shorter
  windows give larger bounds.

# v2 (deferred; not part of v1): the plan for the journal version

Fifth referee report (major revision at a specialist forecasting journal).
Every item below is specified in full, on development data, and frozen
before any held-out or new-period run; each run goes through the guard
with a logged reason. All use the same origins, the same 365-day lookback
and the same eligibility as the v1 evaluation unless stated, and report
both aggregations with 2,000-resample account-bootstrap intervals.

1. **Balance dependence** (the untested limitation of v1).
   - Dispersion growth by horizon: the spread (5-95% range and SD) of
     realized h-day changes against h, per account, compared with V2's and
     B-EQ's forecast spreads; random-walk growth would scale with sqrt(h).
   - Realized change against starting balance: the regression of the
     realized h-day change on the origin balance (relative to the
     account's lookback mean), pooled and per account, at 7, 30 and 90
     days. A negative slope is mean reversion.
   - A balance-matched empirical forecast: B-EQ restricted to past windows
     whose starting balance is closest to today's (k nearest, k fixed on
     development, with an equal-k unconditioned control).
   - A mean-reverting balance model: an AR(1)-type model of the balance
     toward the account's lookback mean, fitted per account by least
     squares on the lookback, with innovations resampled from its own
     residuals (no Gaussian assumption).
   - Scored against B-EQ and V2 on the same origins; the claim to test is
     that balance-conditioned forecasts close B-EQ's long-horizon advantage
     over V2.
2. **The conditioned oracle.** The 2x2 design: scheduled-flow term
   (projected vs realized) crossed with residual sourcing (unconditional vs
   matched on the scheduled flow), with the effective sample sizes of the
   residual pools equalized (the unconditional arm subsampled to the
   matched arm's window count, averaged over a fixed number of draws).
   Extends the v1 oracle, shuffle and matched-B-SR diagnostics into one
   design.
3. **Calendar-period and origin-grid sensitivity.** Common eligibility
   across all variants: the primary comparisons by calendar period (half-
   or full-year blocks) and with the origin grid shifted (offsets 0, 10,
   20 days, as in S8), each on the same accounts.
4. **CZK rescoring.** The primary table rescored in original CZK (no
   per-origin normalization), both aggregations, beside the normalized
   scores.
5. **A raw-data rerun of the primary table from the tagged artifact.**
   From a clean clone of the release tag: fetch the data, rebuild the
   development stages, rerun the held-out evaluation through the guard
   with the logged reason "independent reproduction", recompute the
   pairwise table, and diff it against the committed one (Appendix H,
   command set b).
6. **An independent cohort, if available.** Card-heavy accounts with
   pending transactions; Berka cannot test this. Only with documented
   provenance and a frozen plan before access to outcomes.
7. **Also carried over from round 6:** the switching rule is dropped in
   favour of the horizon rule (V2 at 7 days, B-EQ at 30 and 90) as a
   benchmark for a frozen evaluation; the detector follow-up (CUSUM
   against a two-observation reset after fixing paycheck initialization);
   recalibrated shortfall probabilities (isotonic, cross-fitted on
   development) scored against CONST and SHRUNK on a fresh period.

Added after the sixth referee report (round 9):

8. **Rate uncertainty in V2's sparse branch.** V2 with the rate fixed at
   its posterior mean, (n + 1/2)/D, against the frozen V2 with lambda
   sampled once per path; everything else identical, same seeds. Tests
   explanation (a) of the Discussion: the h^2 Var(lambda) (E[A])^2 term.
9. **A balance-dependent spending variant.** V2's sparse branch with the
   daily event rate (or the amount distribution) depending on the
   simulated balance relative to the account's lookback mean; the form
   and its single parameter fixed on development before scoring. Tests
   explanation (b). Paired with item 1's mean-reverting model.
10. **Across horizons, for items 8 and 9 against B-EQ and the frozen V2:**
    dispersion growth (median 90% width at 7, 30 and 90 days, as ratios
    to 7 days, both as a ratio of medians and as the median of per-account
    ratios, beside sqrt(h) growth and the realized spread of item 1);
    CRPS, both aggregations; lower-tail coverage (misses below the 90%
    interval); and new-crossing shortfall probabilities (Brier against
    CLIM, CONST and SHRUNK).
11. **Shortfall information content.**
    - Cross-fitted recalibration (isotonic, account-grouped folds on
      development) scored against SHRUNK, not only CLIM.
    - Precision-recall curves for new crossings, with average precision
      and its account-bootstrap interval, against the references' curves.
    - The calibration-resolution (Murphy) decomposition of each Brier
      score: reliability, resolution, uncertainty.
    - The incremental performance of each forecast over the
      historical-risk reference: Brier skill against SHRUNK and the change
      in log loss when the forecast is added to SHRUNK in a cross-fitted
      logistic model, each with an account-bootstrap interval.
12. **The distribution of account-level paired CRPS differences** (V2
    against B-EQ, and against the strongest per-horizon alternative): the
    median difference, the share of accounts each method wins, the
    consistency of the winner across horizons (the cross-tab of 7-, 30-
    and 90-day winners), and pre-forecast predictors of which method wins
    (history length, event rate, detected streams, balance level and
    volatility), fitted on development and evaluated once on held-out.
13. **Replication on an independently sourced cohort**, with the rules,
    thresholds, eligibility and analyses frozen and committed before any
    access to outcomes (supersedes item 6's "if available" wording when a
    cohort is found).

Added after the seventh referee report (round 10). **Priorities for v2:
items 15 (the variance attribution) and 18 (the block-bootstrap ladder).**

14. **Per-origin R_h.** For each sparse-branch origin, R_h =
    (h/D)(E[A])^2/E[A^2] from the window's own transaction moments (n, D
    and the signed amounts actually passed to V2), at 7, 30 and 90 days;
    its distribution over origins and accounts, and its relation to the
    account's paired CRPS difference against B-EQ.
15. **The variance-attribution experiment (priority).** On the same
    origins and seeds, five arms of V2: (i) the frozen V2; (ii) the
    spending rate fixed at its posterior mean (item 8); (iii) the paycheck
    state held fixed during simulation (no random walk in m after the
    origin); (iv) the initial paycheck uncertainty fixed (the
    origin's paycheck variance set to a development-fixed constant,
    rather than the filter's); (v) balance-dependent spending (item 9).
    Each scored on CRPS (both aggregations), predictive variance and its
    growth with h, coverage at 50/80/90/95% with one-sided misses, and
    new-crossing Brier against CLIM, CONST and SHRUNK. Arms are compared
    with (i) and with B-EQ under the paired account bootstrap.
16. **Empirical dispersion curves.** The spread of realized cumulative
    changes at every horizon from 1 to 90 days, stratified by lookback
    length, activity (events per month), starting balance relative to
    the lookback mean, and computed from overlapping against
    non-overlapping windows; plotted against sqrt(h) and against V2's and
    B-EQ's forecast spreads.
17. **Autocovariance decomposition.** Var(sum of h daily net flows) =
    h gamma(0) + 2 sum_{k=1}^{h-1} (h - k) gamma(k), per account from the
    lookback; the share of the h-day variance contributed by the
    autocovariance terms, to test for negative serial dependence (the
    alternative to balance feedback as the source of sub-random-walk
    growth).
18. **The block-bootstrap ladder (priority).** One empirical model
    family, from iterated to direct: resampled 1-day flows (iid days),
    7-day blocks, 30-day blocks, and full horizon-length windows (B-EQ
    itself), all from the same lookback, scored on the same origins with
    the same metrics as item 15. Isolates direct against iterated
    forecasting without the state-space model's other choices.
