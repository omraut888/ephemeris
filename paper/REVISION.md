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
