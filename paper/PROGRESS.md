# Progress log (autonomous session, started 2026-11-21)

## Resume here (updated after every step)

**Round 5 finished.** Nothing is running or half-done, and nothing was
pushed. Waiting for the user.

## Round 5 summary (2026-11-22)

**S15 (confirmatory, held-out forward period).**
- **Specification:** amended with P3b in `44afcf4` before the run.
- **Run:** once through the guard, 2026-11-22T00:53Z, 49,762 origins per
  horizon in 3,512 accounts. Results: `paper/results/confirmatory-s15/`
  (`7102b72`).
- **Outcomes**, account mean (primary) with pooled in brackets; all four
  confirmed, and pooled agrees in sign:
  - **P2:** the oracle against B-EQ at 90 days: −0.42 [−0.45, −0.39]
    (−0.20).
  - **P3:** shuffled B-EQ against B-EQ: −0.016 [−0.018, −0.014] at 30
    days and −0.015 [−0.018, −0.013] at 90 (−0.005, −0.004).
  - **P3b:** s_shuf − s_SR/4 = +0.091 [+0.086, +0.097] (+0.050). B-SR's
    90-day skill on these origins is −0.43.
  - **P1 (secondary):** predictable minus unpredictable: +0.20
    [+0.16, +0.24] at 30 days and +0.29 [+0.20, +0.38] at 90 (+0.26,
    +0.42).
- The original specification was on GitHub (00:52) before the run
  (00:53). The amendment rests on its commit time.

**S16 (development, descriptive).** The 5–95% range of ΔB − S_hist is wider
than that of ΔB at 47.9% of origins at 30 days and 51.6% at 90. The median
per-account ratio is 1.000 (quartiles 0.93 and 1.02 at 90 days). The
adjustment does not systematically widen the changes; the paper presents
this as an open hypothesis the check does not support.

**Paper.**
- Updated: the abstract, introduction, §6 (with an S15 table and the
  prior-exposure statement), the conclusion, and the timeline and
  protocol notes.
- 32 pages, 0 LaTeX warnings. Main text to p. 16, appendix from p. 19.

## Round 4 summary (2026-11-21)

**1. The engine flaw (S13; development).**
- **Mechanism.** With the income detector off:
  - a small first income record (median 4% of the later pay) sets m
    confidently too low;
  - real paychecks are then gated as outliers (median 4 of the first 12);
  - the income noise estimate inflates (S ≈ 0.91 pay²);
  - the gain on m falls to 0.002, so in 75% of the 214 income-alarm
    accounts m̂ never comes within 20% of a paycheck.
- **V0's smaller bias** is a different starting point, not compensation:
  removing the detector shifts V0's 90-day median by −9,200 and V2's by
  −10,542.
- **The fix** (post-freeze, `12def8e` then `3bf095a`): an income gate
  escape, active only with the income detector off.
  - The first version, which also fired with the detector on, changed 479
    of 3,105 forecasts per horizon for slightly worse CRPS, and was
    withdrawn.
  - Tests: three, two of which fail without the fix.
  - `test-full`: passed twice (all shells); no golden file changed.
- **Measured effect.**
  - Frozen V0/V2 on the first 100 development accounts: 0 of 3,105
    changed. All 167,130 detector-on forecasts in S9 are identical.
  - Without the detector, V2's 90-day loss is −0.015 (account mean;
    pooled −0.009) instead of −0.46, and m̂ ends at 1.05 times the pay
    instead of 0.13.
- Recorded in §9, Appendix B.4, the limitations and PLAN §11.

**2. Dependence (S14; development).** All identities pass; (b) as stated is
algebraically B-EQ.
- **D1, shuffled B-EQ, holds but is small.** It loses to B-EQ at 30 and 90
  days under both aggregations: account mean −0.012 / −0.013, pooled
  −0.003 / −0.006, about 1/30 of B-SR's loss.
- **D2, matched B-SR, fails.** It is worse than B-SR (−0.07 / −0.07
  account mean).
- **The oracle** still loses at 90 days, and its spread is about B-EQ's.
- **So** the long-horizon loss is not explained by projection error, and
  dependence explains only a small part.

**3. Confirmatory specification S15** (committed `fe14e1d`; summary written,
not run).
- **Design:** held-out origins on or after 1997-07-01 (49,762 per horizon,
  3,512 accounts); one guarded run; primary estimand the account mean,
  pooled also reported.
- **P2:** the oracle loses to B-EQ at 90 days.
- **P3:** shuffled B-EQ loses to B-EQ at 30 and 90 days (stated as small).
- **P1 (secondary):** B-SR's skill is higher in the predictable tertile
  than the unpredictable one. Development: account mean +0.011
  [−0.072, +0.096] at 30 days and +0.25 at 90.
- **Prior exposure** is stated.

**4. Overdraft split (held-out; guarded start-balance read, no
forecasts).**
- 281 origins start below zero, and they give 277 of the 392 events at 7
  days.
- New-crossing prevalence is 0.10 / 0.42 / 1.02%.
- Every forecast loses to climatology on new crossings at every horizon,
  with every interval below 0. The 7-day B-EQp advantage (+0.26) is
  persistence: −0.54 [−0.81, −0.33] on new crossings.

**5. Paper.** The abstract, introduction, §6, §7, §9, Appendix B.4,
limitations and conclusion are rewritten. 31 pages, 0 warnings, main text
to p. 16.

**Deviation from the requested framing.** The user asked for
within-window dependence as "the leading exploratory hypothesis". The
development results make it a small effect, so the paper calls it the
leading hypothesis tested and states that it explains a small part at
most.

## Round 3 checkpoint summary (2026-11-21)

### A1: the detector ablation is genuine, not a bug

On all 27,855 development origins:
- V2, V2 without the detector, V0, and V0 without the detector reproduce
  the stored forecasts exactly;
- the branch V2 uses is the same with and without the detector at every
  origin;
- V0 and V2 share the filter state exactly.

None of the four bug criteria in S9 is met.

**Where the effect sits.** All of it is in accounts with detected income
and an income alarm (5,663 origins, 263 accounts). There, removing the
detector costs:
- V2: pooled −2.94 at 90 days, account mean −2.98;
- V0: pooled −0.32, account mean −0.69.

Elsewhere V2's forecasts are unchanged.

**The mechanism: initialization, not later regime changes.**
- In Berka, the first income-tagged record is a median 3.8% of the later
  paycheck.
- The income alarm fires a median 44 days into the history (96.6% within
  180 days) and moves m̂ to the pay level.
- Without it, m̂ stays at a median 9.8% of the last paycheck, years later.
- V2's cone, whose spending model ignores c, then treats most of the pay
  as missing: the median bias at 90 days is −11,165 normalized units,
  against −623 with the detector.

V0 is biased too (−5,252 against +3,948). Neither its lumps nor ĉ explains
why it is biased less, so that path is not identified.

**Implication.** The frozen results are unaffected, since the detector is
on in every frozen configuration. But the V2 ablation measures the
detector's role in initializing the income state, and the engine's m prior
relies on the detector for that. This is worth an engine note; the engine
was not changed.

### A2: text changes (all committed)

- **§6:** B-SR described exactly (B_t + ΔB − S_hist + S_t; S_hist is a
  retrospective decomposition). Every "same residual distribution" claim
  removed.
- **Caveat first.** The abstract, introduction, §6 and conclusion now open
  the explanation with the caveat about unmeasured irregularity, and state
  the specific B-SR result and the simulator result.
- **§9:** the detector result corrected (same sign, larger), with the A1
  mechanism; the ablation table now shows both aggregations; new Appendix
  B.4.
- **Overdraft:**
  - "negative booked end-of-day balance" (days 1..h, day 0 excluded);
  - "calibration in the large is poor for the four model-based forecasts;
    climatology slightly under-predicts";
  - "cautious upper estimate" removed.
- **"Rejected explanations" removed.** Calendar and sparsity are
  described as limited to the tested rule and range, and an origin-phase
  paragraph is added.
- **Correction found while doing this.** The appendix compared the
  offset-0 S8 rerun (97 accounts, an identity check) with the full shifted
  grids. It now uses the full frozen grid: 30-day pooled −0.12 (account
  mean −0.24) against −0.05 / −0.04 (−0.14 / −0.12). The referee's
  "−0.13" came from that flawed comparison.
- **Pooled skill** is defined as a ratio of aggregated scores (equivalently
  reference-weighted). The zero-denominator rule is stated; it applied to
  one account (B-SA, 7 days).
- **PIT.** "Flat is calibrated" is gone. The text now says what the
  midpoint-PIT histogram can and cannot show (V2 at 7 days: 5.8% of PITs
  exactly 1). Randomized PIT would need the stored quantiles, which the
  frozen rows do not keep.
- **B-GLOBAL:** "its advantage survives removal of calendar look-ahead";
  pooled training is separated from on-device execution; both
  aggregations.
- **Table 6:** coverage, below and above now come from one count table
  (new post-hoc `s10-rows-checks`), with the coverage column added and the
  layout regrouped by horizon. The three add up to 100 before rounding
  (±0.1 after). The §8 7-day sentence is fixed (nominal 5% each side;
  86.3% coverage, 10.2% above).
- **B-SR widths:** the units are CZK, not normalized (fixed).
- **Timeline:** the specification commit, the development results, the
  three guarded exploratory runs and the result commits are added. The
  text notes that the specifications were first pushed together with their
  results, so their precedence rests on commit times.
- **AI disclosure** shortened.
- **x86_64** (the separate request, `e39d143`): run #4 (id 44827652848,
  `7ab6e12`, 2026-11-20) passed the golden feed and the 100 cases, natively
  and in WASM; run #5 repeated it. Recorded in the identity appendix, the
  timeline, the C1 row, the limitations and PLAN §11.
- **Added from B2** (a dev-only caveat on an existing claim): the 7-day
  B-EQp result is persistence (below).

### B1 (S11): schedule replacement by predictability (development, exploratory)

- **Identities.** F0 = B-EQ and F1 = B-SR, to within 2·10⁻¹⁰ (summation
  order).
- **Strata.** Every origin had a schedule and at least 3 checkpoints, so
  there are three tertiles of amount error. The cut points are 0.143 and
  0.413 (median error 0.4% / 23% / 82%; missed 1% / 7% / 24%; extra
  0.1% / 1% / 12%).
- **F1 vs F0**, pooled (account mean), at 30 / 90 days:

  | Tertile | 30 d | 90 d |
  |---|---|---|
  | predictable | +0.045 (−0.24) | +0.061 (−0.28) |
  | middle | −0.22 (−0.37) | −0.47 (−0.58) |
  | unpredictable | −0.17 (−0.26) | −0.36 (−0.54) |

  At 7 days, replacement helps overall: pooled +0.28.
- **Key test: weak support only.** Replacement helps the predictable
  tertile on the pooled score but hurts it on the account mean. It hurts
  the other two under both. The gradient is not monotone: the middle
  tertile is worse than the unpredictable one.
- **The oracle (corrected definition, recorded before the rerun)** uses the
  true future scheduled flows. It beats F1 at 7 and 30 days, but at 90 days
  it still loses to B-EQ: pooled −0.20, account mean −0.43 overall, and in
  the middle and unpredictable tertiles too.

  So at long horizons the loss is not only projection error. Replacing a
  window's scheduled flows breaks the coupling between those flows and the
  rest of the window (spending that went with that pay). This weakens
  "schedule irregularity" as *the* explanation; the paper's §6 should say
  so once the user decides.
- The first oracle (every record of the detected counterparties) was
  wrong. It is kept in `s11-schedule/first-run-oracle/`, superseded.

### B2 (S12): the shortfall audit (development, exploratory)

- **Persistence dominates at short horizons.** 185 origins start below
  zero.
  - Of the 199 events at 7 days, only 17 start at or above zero; of 305 at
    30 days, 122; of 499 at 90 days, 315.
  - With day 0 included, the counts barely change (202 / 307 / 500).
- **The 7-day "B-EQp beats CLIM" is persistence.** Among origins starting
  at or above zero, B-EQp's Brier skill against CLIM is −1.10 [−2.09,
  −0.65], and V2's is −4.97.
- **First crossings** (start at or above zero): no raw forecast beats CLIM
  at any horizon, and CLIM-D is no better than CLIM. V2 recalibrated,
  cross-fitted out of fold:

  | Recalibration | 30 d | 90 d |
  |---|---|---|
  | isotonic | +0.08 [0.02, 0.13] | +0.11 [0.04, 0.18] |
  | Platt | +0.00 [−0.10, 0.08] | +0.09 [−0.01, 0.17] |

  The gain is in reliability; V2's resolution is no better than
  climatology's.
- **Labels.** Reported against reconstructed balances flips 3–6 labels per
  horizon. With the buffers, 1.5% of events end within 100 CZK of zero and
  10–18% within 1,000 CZK. The conclusions are unchanged.
- **Concentration.** 60 of 866 accounts ever go below zero (206 episodes).
  The top 5 accounts give 25% of the 90-day events, and the top 1% (9
  accounts) give 40%. Removing them changes no conclusion.
- **Decomposition.** In every case V2's Brier score is dominated by the
  reliability term (for example 0.082 of 0.093 at 90 days, first
  crossings).

### Proposed frozen confirmatory test of B1 (needs the user's approval)

**A forward period on held-out accounts.**
1. Fix everything from development, now: the predictability measure, the
   tertile cut points (0.143, 0.413), F0/F1/the oracle, the estimands (both
   reported; the primary chosen in advance), and the decision rule.
2. Test on held-out origins dated on or after a cutoff. I propose
   1997-07-01; the origin count per period should be checked, without
   outcomes, before fixing it.
3. Run once, through the guard, with a logged reason.

**Pre-registered predictions** (what development supports, not the
original hypothesis):
- **P1:** the pooled skill of F1 vs F0 at 30 and 90 days is higher in the
  predictable tertile than in the unpredictable one (the interaction, with
  an account-bootstrap interval of the difference);
- **P2:** the oracle loses to F0 at 90 days.

The original hypothesis (help in the predictable tertile) fails on
development under the account mean, so it should not be confirmed as
stated.

**Prior exposure, stated in the paper:** held-out B-SR vs B-EQ overall
(−0.10 / −0.19), V2 vs B-EQ by history length, and S5–S7 have been seen.
Nothing has been seen stratified by predictability, and nothing for the
oracle or after the cutoff in isolation. (The cutoff's later origins are
part of the held-out aggregates already seen.)

**Alternative, a bank-district subgroup test.** Weaker: the districts were
not held out from the aggregate results either, and they differ in size.

### Tier C: plan only (REVISION.md)

The factorial (factors varied one at a time and crossed, sparse branch
forced on and off, 7 days added) plus the two calendar controls. About 1.5
hours of compute (S4 took 134 s for 16 cells). Not run.

### Needs the user's decision

1. Approve the confirmatory test, its cutoff and its primary estimand
   (any held-out run is guarded and needs approval).
2. How the B1 result (the oracle) and the B2 audit (persistence; first
   crossings) should change §6 and §7 and the abstract. Only the 7-day
   caveat has been added so far. The overdraft section's main numbers
   still mix persistence and first crossings, and the held-out split by
   start state is not computed (it needs the stored held-out events only,
   no new forecasts; I did not run it without asking).
3. Whether to note the engine's reliance on the detector to initialize m
   (an engine design issue; no engine change was made).

### Runtime and disk

- **Compute this round:**
  - S9: 503 s, then a stale-binary rerun of 539 s (discarded; my error),
    then 509 s;
  - S11: 67 s, twice;
  - S12: 146 s;
  - `test-full` was not rerun this round; its last full pass was round 2.
- **Paper:** 29 pages, 0 LaTeX warnings, main text pp. 1–14 (statements
  p. 15). PDF: `paper/build/latex/main.pdf`.
- **Disk:** `target/` 4.5 GB (no clean needed); `paper/build/` 3.4 GB
  (explore runs 643 MB, git-ignored); 18 GB free.

## Round 2: the user's decisions on Q1–Q5 (2026-11-21)

1. **History length (Q1).**
   - Removed from the abstract and the conclusion.
   - §5 now gives both aggregations by stratum, and states that the
     planned account mean favours B-EQ at 30 and 90 days even with under a
     year of history.
   - §6 presents schedule projection (B-SR vs B-EQ) as the best-supported
     explanation, and history length as weaker, aggregation-dependent
     evidence. Its table shows pooled and account-mean skill on both splits.
   - Figure 1's left panel adds the account mean (open markers, dashed).
   - The introduction was brought in line (one sentence on the weaker
     evidence).
   - The limitation on estimands now says that within history strata the
     signs differ.
   - The "largest at 12–23 months" claim is narrowed to 90 days, where it
     holds under both aggregations on both splits; at 30 days it does not.
2. **Overdraft pessimism (Q2).** New exploratory quantity: calibration in the
   large, the mean forecast probability minus the observed frequency,
   pooled over origins, per method, threshold and horizon, with 95%
   account-bootstrap intervals (2,000 resamples, seed 6). It is computed
   from the stored development and exploratory held-out events
   (`explore_summary.py [--heldout] s5c` → `s5-overdraft/citl.csv`), with
   no new forecasts and no guard entry, since the guard governs runs that
   produce forecasts.
   - **Result:** V2, CP-noKF, B-EQp and B-SRp over-predict in every cell on
     both splits, and every interval excludes 0. Below 0 at 90 days, V2
     gives 19.9% against 1.3% observed on held-out (development: 21.3%
     against 1.8%).
   - **CLIM is the exception.** It under-predicts slightly below 0, and at
     the tenth percentile at 90 days it over-predicts by about as much as
     B-EQp. So the wording is now "every *forecast*", not "every method".
   - **In the paper:** a new table (Table 4) and a rewritten paragraph in
     §7; the abstract, introduction and conclusion use the new wording, and
     the abstract quotes V2's 90-day number.
3. **Page budget (Q3).** The V1 paragraph is kept. B-GLOBAL is now one
   paragraph in §9: the pooled-data reference; removing calendar look-ahead
   costs it about 0.03; it still has lower CRPS than B-EQ. The definitions,
   the G-cut numbers, V2 vs G-cf, the calibration and the product note
   moved to a new Appendix C. Net page effect: the main text still runs to
   the top of p. 14, because the new overdraft table took the space saved.
4. **E-mail (Q4):** kept as is.
5. **one_seed (Q5).** `--skip one_seed` added to the ignored-tests stage of
   `scripts/test-full.sh`, with a comment; the test itself is unchanged.
   `scripts/test-full.sh` then ran end to end and exited 0:
   - Rust, all stages: 313 tests passed (cargo with ignored tests including
     the soak, WASM engine and sync, desktop, FFI), with clippy clean;
   - Swift: 31 tests;
   - web: 60 tests.

## Final summary

**Completed:**
1. **Exploratory held-out runs.** S3 (reversal decomposition), S5
   (overdraft events), S6 (V2 ablations) and S7 (B-GLOBAL without
   look-ahead) ran through the guard with logged reasons. The results are in
   `paper/results/exploratory-heldout/`. Run (b) was killed once for memory
   (no output seen) and rerun with a logged reason (D1.5).
2. **README known gaps:** below-$0 over-prediction (with the 7-day
   correction, C1), and the cone over-wide at the centre for V2 while the
   shipped V0 is too narrow (D2.1). No app change.
3. **Paper restructured** under the new title. Section order:
   - introduction;
   - related work;
   - method;
   - compact protocol;
   - the reversal (Figure 1);
   - explanation;
   - overdraft events;
   - calibration;
   - components and B-GLOBAL;
   - limitations;
   - conclusion.
   The detector, identity, MoneyData, protocol details and extra tables are
   in the appendix.
4. **PDF:** 0 warnings, no overfull boxes, no undefined references. Every
   caption states its estimand and counts.
5. **Self-review:** five overstated claims fixed (R1–R5). The most important
   is R4: the "under a year of history" claim held on the pooled score only,
   and the planned account mean disagrees. No hand-typed numbers.
6. **`scripts/test-full.sh`:** all stages pass except `one_seed` (see
   below). target/ is 585 MB, so `cargo clean` was not needed.

**Held-out against development (exploratory):** the same sign everywhere
in S3, S5, S6 and S7. V2 vs B-EQ, pooled skill at 30 / 90 days:
−0.12 / −0.20 on development, −0.12 / −0.18 on held-out. B-SR, which adds
the schedule's projection to B-EQ's own residuals, loses to B-EQ by
−0.10 / −0.19 on held-out. Removing the filter costs a little
(−0.03 / −0.04). Overdraft prevalence is lower on held-out (below 0:
0.34 / 0.66 / 1.26% at 7 / 30 / 90 days). Without look-ahead, B-GLOBAL loses
only about 0.03 and still beats B-EQ (+0.23 / +0.30). The full table is
under "Step 1: done" below.

**Paper:** 27 pages: main text pp. 1–13, statements p. 14, references, and
the appendix from p. 16. PDF:
`/Users/omraut/projects/ephemeris/paper/build/latex/main.pdf`.

**Decisions** (details below):
- D1.1: frozen held-out accounts and origins.
- D1.2: frozen rows are read, never rewritten.
- D1.3: the G-cut and G-cf definitions.
- D1.4 and D1.5: memory pressure; run (b) killed and rerun with a lean loader.
- D2.1: README wording separates V2's measurements from the shipped V0.
- C1: the below-zero claim is corrected at 7 days.
- Step 5: appendix floats are `[htbp]`.

**Open questions for you:**
- **Q1.** Pooled or account-mean skill for the history-length story? They
  disagree in sign under a year of history. The paper now shows both;
  dropping the claim from the abstract and conclusion is the other option.
- **Q2.** Should a summary number back "every method is too pessimistic
  about dips"? It would be a new exploratory quantity.
- **Q3.** The main text is about 13 pages against the target of 12. Candidate
  cuts are §9 (B-GLOBAL) or §8's V1 paragraph.
- **Q4.** Confirm the author e-mail in `main.tex` (omraut0888@gmail.com).
- **Q5.** `test-full.sh` runs `cargo test -- --include-ignored`, which
  includes `one_seed`. That test needs `$SEED`, so it panics without it,
  and `set -e` stops the script there. This predates the session. I ran the
  remaining stages separately: all passed, including the workspace with
  `--no-fail-fast --skip one_seed` (289 tests), WASM, desktop, FFI, Swift
  (31) and web (60). Fix options: `--skip one_seed` in the script, or have
  `one_seed` return when `SEED` is unset. I did not change it.

**Disk:**
- `target/`: 585 MB.
- `paper/build/`: 3.1 GB (stored forecasts and features, git-ignored).
- `ios/build/`: 328 MB.
- 22 GB free on the volume.


## Step log

### Step 1: exploratory held-out runs

Plan:
- **(a)** Add a guarded held-out mode to `ephemeris-eval explore berka`.
  It uses the same guard as the frozen held-out run: a clean tree, a
  logged entry with its reason, refused without `--rerun-reason`.
- **(b)** Add `ephemeris-eval explore bglobal-heldout`, which calls the
  guard and then runs `lgbm_cf.py` in a new development-to-held-out mode.

Decisions:
- **D1.1 Which held-out accounts.** The frozen held-out accounts after the
  drift exclusion (3,569) and the frozen eligibility. They are the same
  origins as the frozen held-out run, so the comparisons are on common
  origins with the frozen V0, V2, B-EQ, B-SA+ and B-GLOBAL rows.
- **D1.2 Frozen rows are read, not rewritten.** They come from
  `paper/build/heldout/` (the first run's stored forecasts). Nothing in
  `paper/results/heldout*` is touched; new results go to
  `paper/results/exploratory-heldout/`.
- **D1.3 B-GLOBAL on held-out.** "Without look-ahead, trained only on
  development data before the cutoff":
  - G-cut: one model per horizon, trained on all development rows with
    outcome before 1996-07-01, predicting held-out origins on or after
    1996-07-01;
  - G-cf: per quarter, trained on development rows with outcome before
    the quarter's start, predicting held-out origins in that quarter.
  
  Both use the frozen features, hyperparameters and levels. There are no
  folds, because training and prediction sets are disjoint by split.

### Step 2: README known gaps (done)

**D2.1 Wording.** The shipped app runs the engine's defaults, which is V0,
not V2. "Over-wide at the centre" is true of V2 on held-out Berka (50%
coverage 64–71%). The shipped V0 is too narrow there instead (about 27% at
30 and 90 days). The below-$0 over-prediction was measured for V2,
CP-noKF, B-EQp and B-SRp, not for V0. The README says exactly that rather
than attributing V2's properties to the app. No app behaviour was
changed.

**D1.4 Memory pressure during run (b).** `lgbm_cf.py` in held-out mode
started 8 worker processes. Each parsed both feature files into Python
dictionaries (about 1 GB each at peak). Together with run (a), this filled
swap (13.5 of 14 GB). Memory recovered once the workers were fitting.
- I let it finish rather than restarting, because restarting would have
  needed a second guarded log entry for no analytic gain.
- The cost was wall time: run (a) slowed while both ran.
- The fix for any future run: fewer workers, or a load that keeps only
  arrays and shares them.

**D1.5 Run (b) killed and to be rerun.** The memory pressure got worse:
- load average about 120, 130 MB of RAM free, swap nearly full, and run
  (a) slowed to about 150 forecasts a minute;
- I killed run (b) before it wrote any output, so no result from it was
  seen;
- run (a) recovered to about 2,700 a minute.

Run (b) will be rerun after run (a), with a lean loader: the parent loads
the features once into arrays, and 3 workers share them read-only. It
will be logged with its own reason, "rerun of the exploratory B-GLOBAL
run: first attempt killed for memory exhaustion before writing any
output". The guard log therefore shows two entries for it. The first one
produced nothing.

### Correction C1 (found in step 1)

The development report and the README said that, for falling below 0, "no
method beats the account's own historical frequency". That is wrong at 7
days. B-EQp beats CLIM there on development (Brier skill +0.49 [0.28,
0.62]) and on held-out (+0.26 [0.13, 0.36]). The held-out cell's prevalence
is 0.34%, below the 0.5% the specification set for reliable results, so it
is marked unreliable.
- **Corrected in:** README, `exploratory/README.md`, and the abstract,
  introduction, overdraft section and conclusion.
- **The claim now reads:** no method beats CLIM at 30 or 90 days, and V2
  never does.

### Step 1: done. Held-out against development (exploratory)

Pooled skill at 30 / 90 days, account mean in brackets:

| Comparison | Development | Held-out | Agree? |
|---|---|---|---|
| V2 vs B-EQ | −0.12 / −0.20 (−0.24 / −0.36) | −0.12 / −0.18 (−0.23 / −0.34) | yes |
| B-EQc vs B-EQ | −0.014 / −0.012 | −0.015 / −0.012 | yes: alignment slightly worse |
| B-SR vs B-EQ | −0.09 / −0.18 | −0.10 / −0.19 | yes |
| CP-noKF vs B-EQ | −0.15 / −0.25 | −0.15 / −0.23 | yes |
| CP-noKF vs V2 | −0.025 / −0.036 | −0.029 / −0.042 | yes: the filter helps a little |
| B-SR vs V2 | +0.027 / +0.018 n.s. (90 d mean −0.014 n.s.) | +0.017 / −0.005 n.s. (90 d mean −0.022, CI excludes 0) | mostly; at 90 d the account mean is now significant |
| History < 12 months, V2 vs B-EQ | −0.017 n.s. / +0.039 | −0.024 / +0.046 | yes in direction; at 30 d now significant (small) |
| History ≥ 24 months | −0.15 / −0.29 | −0.16 / −0.27 | yes |
| V2 without detector vs V2 (mean) | −0.19 / −0.46 | −0.21 / −0.49 | yes |
| V2 without adaptive noise (mean) | +0.006 / +0.002 | +0.005 / +0.001 n.s. | yes |
| V2 without floor | 0 / 0 | 0 / 0 | yes |
| G-cf vs B-GLOBAL (mean) | −0.026 / −0.032 | −0.027 / −0.030 | yes |
| G-cf vs B-EQ (mean) | +0.22 / +0.30 | +0.23 / +0.30 | yes |
| G-cf 90% coverage | 81% / 77% | 82% / 78% | yes |

**Overdraft (S5):**
- Prevalence is lower on held-out accounts: below 0 is 0.34 / 0.66 / 1.26%
  at 7 / 30 / 90 days, against 0.71 / 1.09 / 1.79% on development.
- **Below 0:** CLIM is best at 30 and 90 days on both splits. At 7 days,
  B-EQp beats CLIM on both (see correction C1; the held-out cell is marked
  unreliable).
- **Below the tenth percentile:** every method beats CLIM on both splits.
  The schedule-based methods are best at 7 days, and B-EQp at 30 and 90.

**No disagreement in sign** anywhere in S3, S5, S6 or S7.

### Step 5: self-review

**Numbers not from macros:** none. A scan of the `.tex` sources for digits
outside `\R{}`, labels, citations and layout found no hand-typed result
numbers. Fixed constants (365, ±3 days, tenth percentile, 0.5%) are macros.

**Unsupported or overstated claims, fixed:**
- **R1.** Introduction: "the gap needs at least a year of history to
  appear". It is "small or reversed" with under a year (pooled). Fixed.
- **R2.** "Every schedule-based method loses to B-EQ by a similar margin"
  (abstract, introduction, §6): false for B-SA+, which loses by far more
  (B-EQ vs B-SA+ +0.35 / +0.54). Now "loses at 30 and 90 days (B-SA+ by the
  most)".
- **R3.** "In simulation, irregular schedules reproduce the reversal"
  (abstract, introduction, §6, conclusion): the grid has only 30 and 90
  days, so it shows B-EQ's long-horizon advantage, not the 7-day half of
  the reversal. Reworded everywhere.
- **R4 (most important).** Under a year of history the *pooled* skill of V2
  vs B-EQ is −0.02 / +0.05 (held-out), but the planned *account-mean* skill
  is −0.23 / −0.23 (CI excludes 0), and at 7 days the account mean is −0.01
  (n.s.) while the pooled skill is +0.23. The introduction ("V2 at 90") and the
  conclusion ("the state-space model wins ... at three months when an
  account has under a year of history") relied on the pooled estimand
  only. Now the account mean is reported in §5 and §6 and the claims say
  "on the pooled score only".
- **R5.** §7: "B-EQp best at 90 days" for the tenth-percentile event; it is
  best at 30 days as well (Brier 0.126 vs 0.129–0.138). Fixed.

**Checked and consistent:** the abstract's and conclusion's overdraft claims
(V2 never beats CLIM below 0; the 7-day B-EQp exception marked unreliable),
the A2 counts (4 fail, 7 pass, 1 inconclusive) against Table 4, the B1
outcomes against Table 1, the ablation signs against the prose, G-cf
coverage (stated on its own 109,363 common origins, so it differs slightly
from Table 4's V2 numbers on 115,235), and the limitations against the
sections they cite.

**Build:** the 9 "`h` float specifier changed to `ht`" warnings predated
this step (confirmed by building the previous commit). The appendix floats
now use `[htbp]`: 0 warnings, 27 pages (the appendix grew by one page; the
main text is unchanged).

**Needs the user's decision:**
- **Q1. Which estimand leads the history-length story.** The pooled and
  account-mean skills disagree in sign for under-a-year accounts at 7 and
  90 days. The paper now reports both. You may prefer to drop the history
  claim from the abstract and conclusion entirely.
- **Q2. "Every method is too pessimistic about dips"** is a reading of the
  reliability plots (mostly below the diagonal) with one stated
  exception. No single summary number (for example, a mean forecast minus
  the observed frequency) backs it. Adding one would be a new exploratory
  quantity; I did not add it.
- **Q3. Page budget.** The main text runs to p. 13 (sections 1–11) plus the
  statements on p. 14, about 12.5–13 pages against the target of about 12.
  Further cuts would come from §9 (B-GLOBAL) or §8's V1 paragraph.
- **Q4.** The author e-mail in `main.tex` is omraut0888@gmail.com (as set
  earlier); confirm it is the one to submit with.
