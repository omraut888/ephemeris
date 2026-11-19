# MoneyData case study: one account, descriptive only

The MoneyData set (Firat et al., EuroVis 2023; CC BY 4.0) is **one** UK
current account, from 2015-07-27 to 2022-07-25, in GBP. With n = 1 nothing
here generalizes to other users, and the paper says so. There is one
account, so the account bootstrap has nothing to resample: `main.csv`
keeps point estimates only, and skill is the pooled ratio over this
account's forecasts.

- **Build.** `ephemeris-eval moneydata`; code in
  `crates/ephemeris-eval/src/moneydata.rs`, which lists the adapter
  decisions:
  - ISO dates are read with day and month swapped;
  - records are tagged by the app's own tagger, at each origin;
  - the `us` calendar;
  - Berka's per-origin normalization.
- **Protocol.** Weekly origins from day 180, 327 of them, all with a
  detected income. Horizons 7, 30 and 90 days.
  - Methods: V0, V1, V2 and the per-account baselines.
  - V2's sparse rule never fired, so V2 equals V0.
  - B-GLOBAL, V3 and V4 need other accounts and are not run.
- **The account.**
  - 6,567 records, about 78 a month.
  - About 44% of the gross flow is irregular: one-off inflows (one of
    £83,841), an £84,000 transfer out, and investment purchases. The
    schedule-plus-spend-rate model has no term for these flows.
  - The employer changes during the history.
