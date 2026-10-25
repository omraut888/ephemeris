# Overnight log

<!-- SUMMARY: written last; see the end of the run. -->

## Decisions, in order

### 0. Finishing Phase 1

- **0a, detector-on calibration bound.** Added as a second assertion in
  `tests/forecast_calibration.rs` with the requested ±8 points, compared in
  counts like the existing ±5-point check (so a hit exactly on the bound is
  not lost to rounding). Both share one helper. The detector-off ±5 check
  is unchanged. Measured worst cell with the detector on: 55.3% for the
  50% interval at 90 days, 5.3 points off, inside ±8.
- **0b, README gaps.** Each gap quotes a number from the Phase 1 docs
  (the −$5 to $139/day sandbox band, 9-day median detection, 44% coverage
  at one week), so the README cannot drift into vaguer claims than the
  measurements.
- **0c, CLAUDE.md.** Phase 1 is marked complete. The "no web code yet"
  rule became Phase 2's rule: all math stays in Rust, and TypeScript only
  loads, calls WASM and draws. That rule comes from item 4 of the brief.
  The layout lists the new crate and `web/`.

### 1. Phase 2 design (docs/phase2-design.md, written before any web code)

- **The CLI's forecast steps move into a core `pipeline` module** that the
  CLI and WASM both call. Otherwise the WASM crate would re-implement the
  CLI's tracker → schedule → forecast → report sequence, and "byte-identical
  native vs WASM" would compare two different programs.
- **JSON strings across the WASM boundary**, not `JsValue`. There is one
  serialization path (serde_json) in both builds, no extra crate, and the
  identity test checks the exact bytes the app parses.
- **Smoothed view = per-path minimum per pay cycle, computed in Rust**
  inside the Monte Carlo loop. The obvious shortcut, the minimum of the
  daily quantiles, is a different and optimistic statistic. Cycles are the
  detected nominal paydays, or calendar weeks when no income is detected.
- **Default cone view = raw.** The brief leaves the default open. Raw is
  what the CLI shows and what calibration measures, so it is the least
  surprising placeholder. It is a single constant in `App.tsx`.
- **"Afford $X by D" shows both probabilities**: on day D, and at some
  point by D. The brief's phrase fits either. Both come from existing core
  functions (`prob_at_least_on`, `prob_reached_by`), serialized as an
  additive, optional `forecast.targets` field (schema version unchanged).
- **Report additions are additive**, so `schema_version` stays 1:
  `regime_changes`, `forecast.targets` (only when requested) and
  `forecast.cycle_lows`.
- **TS types: generated with ts-rs, plus a drift test** (reasons in design
  §3). ts-rs sits behind a non-default core feature that only the WASM
  crate's tests enable, so no production build compiles it.
- **No chart library.** The SVG is hand-written: pixel mapping is
  presentation, not math, so it stays within "no math in TypeScript".
