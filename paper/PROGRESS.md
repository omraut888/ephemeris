# Progress log (autonomous session, started 2026-11-21)

## Resume here (updated after every step)

**Round 3 (second referee report), in progress.**
- Done: the x86_64 identity item (`e39d143`).
- Current: Tier A1, the detector-ablation investigation (read-only
  diagnostics on development; nothing written to the frozen results).
- Half-done: nothing.
- Next: A1, then A2 (text), then B1/B2 specifications in REVISION.md
  (commit before running), then the development runs, the Tier C plan, and
  the checkpoint summary.

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
