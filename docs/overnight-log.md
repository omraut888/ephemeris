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

### 2. ephemeris-wasm

- **Pay-cycle lows are math, so they got hand-computed tests first**
  (`cycle_lows_by_hand`, week basis), plus an invariant test: every
  quantile of the per-path minimum is at or below every daily quantile in
  its cycle. I had also written a "strictly lower" check. It failed,
  rightly: with a steady downward drift every path bottoms out on the same
  last day, so the two coincide. It wasn't a theorem, so I removed it. That
  is not a loosened tolerance; the invariant test stays.
- **Shared pipeline** (`ephemeris_core::pipeline`). The CLI's `simulate` and
  `forecast` now call it. Checked on the sandbox feed: the CLI's text output
  is byte-identical and every pre-existing JSON field is unchanged; only
  the three additive fields are new.
- **Options reject unknown fields** (`deny_unknown_fields`), so a typo from
  JavaScript (`{"horizon": 30}`) is an error instead of a silent default.
- **Byte identity, proven both ways.** `tests/golden/{feed,report}.json`
  hold a realistic 240-day feed (seed 7) and its 90-day, 2000-path report
  with two targets (seed 11). The native test (`tests/golden.rs`) and the
  WASM test (`tests/wasm.rs`, `wasm-pack test --node`) assert the same bytes,
  in both debug and release builds. Result: identical. `scripts/test-full.sh`
  now runs the release WASM test as well.
- **The exports wrap plain-Rust functions** (`simulate_json`,
  `forecast_json`), because constructing a `JsError` natively panics. The
  WASM test calls the real exports.
- **.wasm size (release, wasm-opt by wasm-pack): 346,149 bytes, 148,634
  gzipped** (`gzip -9`). I didn't tune it with `opt-level = "s"`: it is
  within reason for a first cut, and changing the workspace release profile
  would also change the CLI build.
- **Speed:** in release WASM under Node, the simulate + 2000-path 90-day
  forecast test runs in about 0.19 s. So the app runs on the main thread,
  with no Web Worker (design §4's threshold).

### 3. TypeScript types

- **Generated with ts-rs from the Rust schema types, with a drift test**
  (`crates/ephemeris-wasm/tests/ts_types.rs` → `web/src/schema.gen.ts`).
  Reasons: Rust stays the single source of truth; ts-rs reads the serde
  attributes, so the declarations describe the actual JSON (the tagged
  `PayCadence`, snake_case enums); and the test runs in plain
  `cargo test`, so `scripts/test-full.sh` catches drift without a Node
  step. Alternatives are in phase2-design.md §3. I checked that the test
  works: a one-word doc-comment change to a schema type fails it.
- **ts-rs is optional and test-only.** It sits behind core's off-by-default
  `ts` feature, which only `ephemeris-wasm`'s dev-dependencies enable.
  `cargo tree -e normal` shows it absent from the shipped WASM and the CLI.
  It is justified in phase1-design.md's dependency table.
- **`Date` is declared as `IsoDate = string`**, renamed so it doesn't shadow
  JavaScript's global `Date`. **`u64` ids and seeds are `number`** rather
  than ts-rs's default `bigint`: the values are below 2⁵³ and `JSON.parse`
  yields numbers anyway, so `bigint` would be a lie.
- **Options types are fully required in the generated file**, even though
  every field has a serde default. The bridge accepts `Partial<…>` instead
  of hand-editing generated code.
