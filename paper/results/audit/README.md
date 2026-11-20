# Audit (review response, Tier 1 A), 2026-11-21

This audit makes no new forecasts. A1 reads the stored per-forecast rows of
the held-out run. A4 recomputes development forecasts only. The code is
cited by path.

## A1. CRPS skill estimands

**What every table used.** `aggregate.rs` defines `skill` as the mean over
accounts of each account's ratio,
`mean_a (1 − Σ_a CRPS_m / Σ_a CRPS_ref)`, summed over the forecasts both
methods made. `pooled_skill` is the ratio of sums over all forecasts,
`1 − Σ CRPS_m / Σ CRPS_ref`.
- Tables 2, 7 and 8 report `skill`, the account mean.
- The origin sets are common. Every method has the same 115,235 held-out
  forecasts in 3,514 accounts at each horizon (`origin-sets-heldout.csv`).
  B-SEAS has a forecast at every origin, so the "both made" restriction
  removes nothing.

**Discrepancy 1** (Table 2 against Table 7 at 7 days).
- Pooled ratios compose on common origins. V2 against B-SA+ is +0.2221
  pooled, and B-EQ against B-SA+ is −0.1766. These imply V2 against B-EQ =
  1 − (1 − 0.2221)/(1 + 0.1766) = +0.3389, exactly the committed pooled
  value.
- Account means of ratios do not compose. The review's +0.47 applies the
  pooled identity to the account means (+0.2771 and −0.3607). The account
  mean of V2 against B-EQ is +0.2828.

**Discrepancy 2** (Table 8 at 30 days).
- The income strata are not a partition of accounts. An account belongs
  to both when income was detected at some of its origins and not at
  others: 2,217 + 2,211 = 4,428 > 3,514.
- Within a stratum, an account's ratio uses only its forecasts there. The
  overall account mean is therefore not a weighted average of the
  stratum means, and it can lie outside them (+0.2229 against +0.1826 and
  +0.2066).
- Pooled, it cannot: +0.2746 overall, between +0.2796 and +0.2714.

**Tiny denominators.** Per-account skills have long negative tails: the
worst account is −2.5 for V2 against B-SA+ at 7 days, −10.8 for B-EQ
against B-SA+, and −16.6 for V2 against B-GLOBAL.
- Dropping the decile of accounts with the smallest mean reference CRPS
  moves account means by up to 0.17 (`mean_of_ratios_trim10`).
- Examples:
  - V2 against B-EQ at 30 days: −0.234 as an account mean, −0.170 after
    the trim, −0.122 pooled.
  - V0 against B-SA+ at 30 days: +0.003, +0.059 after the trim, +0.144
    pooled.
- **The signs of every headline comparison are the same under all three
  aggregations**, at 30 and 90 days:
  - V2 beats B-SA+;
  - V2 loses to B-EQ;
  - V2 loses to B-GLOBAL.
- The magnitudes are estimand-dependent, and the paper now reports both
  aggregations.

## A2. Two facts checked against the code

**(a) Drift threshold.**
- The code (`berka::MAX_DRIFT_CZK = 100.0`) and PLAN.md §8 say **100
  crowns**. Both were committed in `4e7c066` (2026-11-13), before any
  development results were read.
- The report that asked for approval said "diverges by more than 1,000
  crowns", and the approval repeated it. That wording was wrong: only 18
  of the 19 excluded development accounts exceed 1,000. The 19th lies
  between 50 and 1,000.
- Recount from the raw data, by split:

  | Split | Total | Drift > 50 | Drift > 100 | Drift > 1,000 |
  |---|---|---|---|---|
  | Development | 890 | 19 | 19 | 18 |
  | Held-out | 3,610 | 41 | 41 | 41 |

- **The held-out set is the same under either threshold.** On
  development, the 100-crown rule (applied) excluded one account that a
  1,000-crown rule would have kept.
- The code is unchanged. The paper discloses this as a deviation from the
  approval's wording.

**(b) The normalization constant 8,412.54.**
- `reference.rs` computes it as the median monthly gross flow of the
  **realistic synthetic generator**: seeds 0–99, one year from
  2024-01-01, whole months, non-transfer records. It is pinned, and a
  test recomputes it.
- It is not the median over development accounts, as the Nov 20 draft
  said. The text is fixed.

## A3. How each forecast's inputs are formed (Berka)

- **The balance series.** End-of-day balance = the opening balance (the
  first record's balance minus its amount) plus the running sum of amounts
  (`Account::daily_balance`). It is carried forward over days without
  records. The bank's reported balance column is used only to compute the
  drift, never as an input or as truth.
- **Same-day order.** Records are sorted by (date, `trans_id`).
  - An origin includes every record dated on or before it, so all of the
    origin day's records count (`records_through`).
  - Today's balance on a day without records is the previous end-of-day
    balance.
  - Truth is the end-of-day balance at origin + h.
- **Causality at the origin.** Everything below uses records dated on or
  before the origin:
  - tagging (`tag`);
  - schedule detection;
  - the scale `s` (whole months ending on or before the origin);
  - the sparse rule (365 days to the origin);
  - eligibility (records in the 180 days before).
- **B-SA+ residuals** come from weekly pseudo-origins `g` with
  `g + h ≤ origin` and `origin − g ≤ 365`. Each pseudo-origin's B-SA
  forecast uses records on or before `g`. Its error uses balances at `g`
  and `g + h`, both on or before the origin.
- **V1's calibration stream** is V0's forecasts at grid days `g` (every 14
  days from day 90) with `g + h ≤ origin`. Each is scored against the
  balance at `g + h`.
- **Not causal: a selection rule and two cross-sectional uses.**
  1. The drift exclusion uses each account's *full* history, including
     days after any origin. An app could not apply it at the origin. It
     removes 41 held-out accounts whose reported balances reset; it is a
     data-quality selection, not a forecast input.
  2. V3 and V4 priors come from other accounts' estimates at *their* last
     eligible origins, up to late 1998. These are used for held-out
     origins as early as 1993–1994, so they carry calendar-future
     cross-sectional information.
  3. B-GLOBAL is trained on development origins of every date, which is
     the same calendar look-ahead.
  
  None of these affects V0, V1 or V2. Tier 2 plans the calendar-forward
  versions.

## A4. Prefix equivalence (development only)

`ephemeris-eval prefix-check` (`crates/ephemeris-eval/src/audit.rs`):
- **Sample.** 200 development origins in 172 accounts, drawn with seed
  20261121 from all 27,855 eligible development origins.
- **Truncation.** For each, the account's lines of `trans.tsv` dated on or
  before the origin are cut from the text and parsed alone.
- **Methods recomputed:** V0, V1, V2, B-30, B-LIN, B-SA, B-SA+, B-EQ and
  B-SEAS at 7, 30 and 90 days, giving 5,400 forecasts.

**Result:** all 5,400 forecasts from the truncated feed are equal, value
for value, to those from the full file. All 5,400 rows, scored against the
realized balance, are byte-identical to the frozen development rows. Files:
`prefix-check.csv` and `prefix-check-summary.csv`.

## A5. From forecasts to scores

**Every distribution is one of four kinds** (`dist.rs`):
- **Monte Carlo cones (V0, V2, V3).** Type-7 quantiles of 1,000 paths at
  the 200 midpoint levels `(i + ½)/200` plus the interval ends. The tails
  are resolved to 0.25% and 99.75%.
- **Quantile functions (V1, V4, B-GLOBAL):**
  - Levels are increasing, and the values are sorted, so crossing
    quantiles are repaired by rearrangement.
  - The function is linear between levels and **flat beyond the first and
    last level**, a point mass at the extreme quantiles.
  - V1/V4 have 23 levels (0.01 to 0.99); B-GLOBAL has 23 (0.01 to 0.99).
- **Empirical samples (B-SA+, B-SEAS, B-EQ).** Exact CRPS of the
  empirical distribution. Quantile = the smallest sample with
  `F(x) ≥ p`, so interval ends are order statistics.
- **Gaussian (B-30, B-LIN).** Closed-form CRPS. The normal CDF is a
  Chebyshev fit with relative error under 1.2e-7. Quantiles are exact.
- **Point (B-SA).** CRPS = absolute error. Its "intervals" are the point,
  so coverage is not meaningful; the tables now show "–".

**Scoring:**
- **Coverage** of a central level `ℓ`: `lo ≤ y ≤ hi`, with
  `(lo, hi) = (q((1−ℓ)/2), q((1+ℓ)/2))`, inclusive.
- **CRPS of a quantile function:** the pinball-loss integral by the
  midpoint rule over 200 levels, each read from the function by linear
  interpolation (`metrics::crps_quantiles`). Flat tails beyond 1% and 99%
  understate tail spread for V1, V4 and B-GLOBAL relative to the cones.
- **PIT:** the inverse of the piecewise-linear quantile function. On a
  flat stretch it is the middle of the levels sitting on `y`. A sample's
  or point's PIT is the mid-point of the jump, `(F(y−) + F(y))/2`, with no
  randomization.

**How V1 becomes one quantile function** (`variants::recalibrate`):
- Each of the 11 central levels `ℓ` keeps its own ACI recursion and gives
  an adjusted level `a_ℓ`, capped at 0.995.
- Nominal level `(1−ℓ)/2` gets V0's quantile at `(1−a_ℓ)/2`, and
  `(1+ℓ)/2` gets V0's quantile at `(1+a_ℓ)/2`. The median is V0's.
- Because the recursions are separate, `a_ℓ` need not increase with `ℓ`,
  so the intervals need not nest. The values are then sorted
  (rearrangement): the function is monotone, and where two levels crossed
  their values are exchanged.
- Interpolation is linear, with flat tails beyond 0.01 and 0.99.
- How often adjusted levels cross was not recorded; counting it needs new
  computation, so it is in the Tier 2 plan.
