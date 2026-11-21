# Exploratory held-out runs (2026-11-21)

These analyses are **exploratory**, not confirmatory. They were specified in
`paper/REVISION.md` (S3, S5, S6, S7) and developed on the development split
(`../exploratory/`). Each was then run once on held-out accounts in a
guarded, logged run (`../heldout-runs.log`). The frozen held-out results in
`../heldout*` were read, never rewritten.

- **Run (a):** `explore berka --held-out`. It covers S3 (the reversal
  decomposition), S5 (overdraft events) and S6 (V2 ablations), over
  115,235 origins in 3,569 accounts. It took 2,467 s.
- **Fresh V2 against the frozen held-out V2:** 345,704 of 345,705 rows are
  identical. The other differs in the last digit, from the post-freeze
  fix.
- **Run (b):** B-GLOBAL without calendar look-ahead (S7), trained on
  development data only. G-cf covers 325,177 of 345,705 held-out forecasts
  (quarterly, outcome-before-quarter training). G-cut covers 233,328
  (origins from 1996-07-01; one model trained on outcomes before then).
  It took 207 s.
  - The first attempt was killed for memory exhaustion before it wrote
    any output (`paper/PROGRESS.md` D1.5). Both entries are in the run
    log.

Summaries: `paper/analysis/explore_summary.py --heldout`. The definitions
and aggregations are the same as on development.

**Prevalence warning (S5).** Falling below 0 within 7 days has a prevalence
of 0.34% on held-out accounts. That is below the 0.5% the specification set
for reliable results, so those cells are marked unreliable.

**Calibration in the large (S5, `s5-overdraft/citl.csv`).** This was added
on 2026-11-21, after the held-out S5 run. It is computed from the stored
held-out events by `explore_summary.py --heldout s5c`, with no new forecasts
and no guard entry. The definitions are the same as on development. V2,
CP-noKF, B-EQp and B-SRp over-predict in every cell, and every interval
excludes 0. CLIM is the exception, as on development.
