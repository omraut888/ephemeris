# Progress log (autonomous session, started 2026-11-21)

## Resume here (updated after every step)

**Current step: 1 (exploratory held-out runs), with step 3 (paper
restructure) drafted in parallel.**

**Finished:**
- the guarded held-out modes (commit `3b433c0`);
- step 2, the README known gaps (`b5419b0`);
- the compact protocol section and its appendix (`2ffb6bb`);
- new figure code (`3360515`);
- abstract, introduction and reversal sections drafted, not yet built.

**Half-done:**
- **Run (a)**, the held-out reversal, overdraft and ablation pass, is
  running in the background. It writes to `paper/build/explore-heldout/berka/`
  and logs to `paper/build/explore-heldout-berka.log`.
  - Its guard entry is already committed in
    `paper/results/heldout-runs.log`.
  - If it was interrupted (no "exit" line in its log), rerun it with
    `--rerun-reason "rerun of exploratory run (a): first attempt
    interrupted"`, after `git status` is clean. It is safe to rerun: the
    output directory is simply overwritten.
- **Run (b)**, held-out B-GLOBAL, was killed for memory (D1.5) with no
  output. It still needs to run after (a) finishes.
- **The binary is not rebuilt** since `lgbm_cf.py` got the lean loader. Run
  `cargo build --release -p ephemeris-eval` first.

**Next action:**
1. When run (a)'s log has an "exit" line:
   `paper/.venv/bin/python -I paper/analysis/explore_summary.py --heldout S3 S5 S6`.
2. Commit `paper/results/exploratory-heldout/`.
3. Rebuild the binary.
4. Run (b):
   `target/release/ephemeris-eval explore bglobal --held-out --rerun-reason "rerun of the exploratory B-GLOBAL run: first attempt killed for memory exhaustion before writing any output" --out paper/build/explore-heldout/bglobal`.
5. Then `explore_summary.py --heldout S7`, and compare with development.

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
