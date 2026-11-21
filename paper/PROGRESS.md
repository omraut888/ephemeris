# Progress log (autonomous session, started 2026-11-21)

## Resume here (updated after every step)

**Current step: 5 (self-review).**

**Finished:**
- steps 1–2;
- step 3 (the restructure);
- step 4: the PDF builds at 26 pages, the main text about 12.5; 0
  warnings, overfull boxes and undefined references; captions checked
  (`cc77171` and later).

**Half-done:** nothing running.

**Next actions:**
1. Step 5: scan the `.tex` sources for digits outside macros, and read
   the PDF text for unsupported claims and inconsistencies. Write the
   findings under "Step 5" below.
2. Step 6: `scripts/test-full.sh`.
3. The final summary.

*(The final summary goes here at the end.)*

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
