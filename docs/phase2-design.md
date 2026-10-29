# Phase 2 design: the web shell

Status: written before any web code (2026-10-24). Scope: compile the core to
WASM, and build a browser app that shows the forecast. Mobile, desktop and
sync are later phases (spec §07).

## 0. Principles

- **One engine.** Every number the app shows comes out of
  `ephemeris-core`: the estimate, the schedule, the cone, the pay-cycle lows
  and the "can I afford it" probabilities. TypeScript parses JSON, calls
  WASM, lays out pixels and formats money. No statistics, no filtering, no
  date arithmetic on forecast data in TypeScript.
- **One contract.** The browser speaks the same versioned JSON the CLI
  already writes (docs/forecast-schema.md): a `Feed` in, a `ForecastReport`
  out. A feed exported by `ephemeris import-plaid` loads in the app as is.
- **One pipeline.** The steps the CLI's `forecast` command runs (tracker →
  schedule detection → Monte Carlo → report) move into the core as a single
  function, so the CLI and WASM cannot drift apart. The same goes for
  `simulate`.
- **Deterministic.** The same feed, options and seed give the same report
  JSON, byte for byte, natively and in WASM. Phase 1 chose `libm`, ChaCha8
  and counter-based streams for exactly this; Phase 2 proves it with a
  test (§2).

## 1. Core changes

### Shared pipeline (`ephemeris_core::pipeline`, behind `serde`)

```rust
pub struct SimulateOptions { seed: u64, days: u32, mode: Mode, start: Date }
pub fn simulate_feed(opts: &SimulateOptions) -> Result<Feed, EngineError>;

pub struct ForecastOptions {
    horizon_days: u32,   // default 90
    paths: u32,          // default 2000
    seed: u64,           // default 0
    targets: Vec<f64>,   // balances X for P(b ≥ X) queries; default none
}
pub fn forecast_feed(feed: &Feed, opts: &ForecastOptions) -> Result<ForecastReport, EngineError>;
```

Both option types are serde structs with defaults, so `{}` is a valid
options object. The CLI's `simulate` and `forecast` commands become thin
calls to these. The CLI's printed output and JSON must not change, apart
from the additions below.

### Report additions (schema version stays 1: additive, optional)

1. **Regime changes** (`regime_changes`). The CLI already prints them; the
   app shows them as markers. Each entry: `date`, `settled`, `channel`
   (`"spend"` or `"income"`), `shift`.
2. **Target curves** (`forecast.targets`), present only when
   `ForecastOptions::targets` is non-empty. For each target X:
   `at_least_on_day[i]` = P(b ≥ X on day i), and `reached_by[i]` = P(b ≥ X
   on some day ≤ i). These are the existing `Cone::prob_at_least_on` and
   `Cone::prob_reached_by`, serialized parallel to `forecast.days`.
   *Added later:* `below_by[i]` = P(b < X on some day ≤ i)
   (`Cone::prob_below_by`), counted in the same pass. The app's headline
   risk is `below_by` for X = 0 on the last day. It cannot be derived from
   the other two curves, because a path can be on both sides of X.
3. **Pay-cycle lows** (`forecast.cycle_lows`): see §4. Always present.

`docs/forecast-schema.md` documents all three.

## 2. The WASM API (`crates/ephemeris-wasm`)

```rust
#[wasm_bindgen] pub fn schema_version() -> u32;
#[wasm_bindgen] pub fn simulate(options_json: &str) -> Result<String, JsError>; // → Feed JSON
#[wasm_bindgen] pub fn forecast(feed_json: &str, options_json: &str) -> Result<String, JsError>; // → ForecastReport JSON
```

- **Strings, not `JsValue`.** JSON crosses the boundary as text, produced by
  the same `serde_json` call natively and in WASM. This needs no extra
  crate (no `serde-wasm-bindgen`), makes the byte-identity claim testable
  on the exact bytes the app parses, and the cost (one `JSON.parse` of
  ~25 kB) is negligible next to the Monte Carlo.
- **Errors** are `JsError` with the core's message (e.g. "feed has
  schema_version 2"). The library never panics on bad input (Phase 1 rule).
- **Dependencies:** `wasm-bindgen` (required to export functions; pinned
  through `Cargo.lock`) and `serde_json` (already in the graph). Dev-only:
  `wasm-bindgen-test`, to run the same assertions inside WASM under Node.
  The core stays as it is: `rand` without `getrandom`, so
  `wasm32-unknown-unknown` needs no JS shims.
- **Build:** `wasm-pack build crates/ephemeris-wasm --target web --release
  --out-dir ../../web/src/wasm/pkg`. The output is generated and
  gitignored; `npm run build` runs it first.

### Proving byte identity

A golden file, `crates/ephemeris-wasm/tests/golden/`, holds a fixed feed
(the realistic generator, seed 7, 240 days) and the report JSON for fixed
options (90 days, 2000 paths, seed 11, two targets).

- A **native** test computes both through `ephemeris-wasm`'s exported
  functions, called as plain Rust, and asserts they equal the golden bytes.
  `UPDATE_GOLDEN=1` rewrites them.
- A **WASM** test (`wasm-bindgen-test`, run by `wasm-pack test --node`)
  calls the same functions compiled to `wasm32` and asserts the same bytes.

Both match one file, so native and WASM agree byte for byte: every f64 of
every quantile, the variance draws, the lumps. The test uses the
realistic generator, so it covers the pending tail, gating, lumps and
regime detection, not just the easy path.

## 3. Keeping TypeScript types in sync

**Choice: generate them from Rust with `ts-rs`, and fail a test on drift.**

- `ephemeris-core` gets an optional `ts` feature that derives `ts_rs::TS`
  on the schema types (and on `FlowKind`, `TxnStatus`). It is off by
  default and never enabled for the WASM or CLI builds; only
  `ephemeris-wasm`'s dev-dependencies turn it on.
- A native test in `ephemeris-wasm` renders every type's declaration into
  one file and compares it with the committed `web/src/schema.gen.ts`. Any
  change to a Rust schema type that isn't regenerated fails `cargo test`
  (and so `scripts/test-full.sh`). `UPDATE_TS=1` regenerates it.
- The TypeScript side imports only from `schema.gen.ts`. `tsc` then fails
  wherever the app uses a field that no longer exists.

**Alternatives rejected.**

- *Hand-written types plus a fixture test.* A Vitest test can check that a
  report produced by the engine *parses* into the hand-written type only at
  runtime, which needs a validator (zod or similar) duplicating the schema
  a third time. It also catches only the fields the fixture exercises.
- *JSON Schema (`schemars`) → `json-schema-to-typescript`.* Two generators
  and a Node step in the Rust test path, for the same result.
- *`tsify`/`serde-wasm-bindgen`.* It ties the types to the `JsValue`
  boundary we chose not to use (§2).

`ts-rs` reads the serde attributes already on the types (`rename_all`,
`tag = "kind"`, `default`), so the generated types describe the actual JSON.
`Date` serializes as a string and is declared as `string` (`YYYY-MM-DD`).
`u64` fields (`id`, `seed`) become `number`: ids are below 2⁵³, and seeds
from the app are too. This is documented in the generated file's header.

## 4. Visualization

### The problem with the raw cone

The daily balance is a sawtooth. It jumps up on payday, down on rent day,
and drifts down in between. A cone of daily quantiles faithfully shows
that, but the eye reads the teeth, not the trend. The question most people
are actually asking, "will I run short before the next paycheck?", is
about the bottom of each tooth.

### Two views, one toggle

1. **Raw daily cone.** The 5–95% and 25–75% bands and the median, one point
   per day, straight from `forecast.days`.
2. **Pay-cycle lows.** For each pay cycle (the days from one nominal
   payday up to the next), the distribution across paths of *that path's
   lowest balance in the cycle*. It is drawn as a stepped band, flat over
   each cycle. It reads as "in 95% of futures your balance stays above
   $X until the next paycheck".

The per-path minimum matters. The 5th percentile of each day's balance,
minimized over the cycle, is **not** the 5th percentile of the cycle's
minimum. The latter is lower, and it is the honest answer to "how low
might I go". So the lows are computed in the core, inside the Monte Carlo
loop: each path keeps a running minimum for the current cycle, quantiles
are taken at the cycle's end, and memory stays O(N). Cycle boundaries are
the detected schedule's nominal paydays. With no detected income the
cycles are calendar weeks from the forecast start, and the report says so
(`cycle_lows.basis`: `"paycheck"` or `"week"`). A partial last cycle is
included and marked `partial`.

The toggle is in the chart header. The app opens on the raw (Daily) cone,
what the CLI shows and what the calibration tests measure. *Decided by the
product owner, 2026-10-29.* Above the chart, in both views, the app
headlines the chance of dipping below $0 on any day of the horizon
(`below_by` for target 0, §1).

### The chart

- Hand-written SVG in React: two area paths, one line, axes, a "today"
  rule, regime-change markers, and a dashed $0 line when the cone reaches
  below zero. No chart library: the shapes are few, and a library would
  bring its own scale and date logic. Mapping values to pixels is
  presentation, not forecasting.
- Hovering shows the date and the five quantiles from the report.
- Dark-first palette: near-black background, one accent hue for the bands
  (outer band low opacity, inner band higher), a light median line, and
  amber for warnings. A light theme follows `prefers-color-scheme`.
- Numbers in tabular monospace figures.

### The rest of the app (spec §05: "mission control", closer to Linear or Raycast than a budgeting app)

```
┌──────────────────────────────────────────────────────────────────────┐
│ EPHEMERIS   ● demo · realistic · seed 7        [Demo] [Load feed…]   │
├───────────────────────────┬──────────────────────────────────────────┤
│ STATE  as of 2026-10-21   │  FORECAST · 90 d · 2000 paths  [Raw|Lows]│
│ Balance   $421 ± 54       │                                          │
│ Spend     $67/d ± 72      │        ░░░░░▒▒▒▒▓▓▓▓────                 │
│ Paycheck  $810 ± 27       │   ▒▒▓▓───                                │
│ SCHEDULE                  │                                          │
│ Sweetgreen weekly → 10-27 │                                          │
│ BILLS                     ├──────────────────────────────────────────┤
│ Spotify  −$12.65  21st    │ CAN I AFFORD  $[ 2,000 ] by [2026-12-01] │
│ REGIME CHANGES            │ → 61% on that day · 74% at some point    │
└───────────────────────────┴──────────────────────────────────────────┘
```

- **Data source.** *Demo* generates a synthetic feed in WASM (mode and seed
  selectable). *Load feed* takes a dropped or picked JSON file. Either way
  the app then runs the same `forecast`.
- **Afford query.** The app re-runs `forecast` with `targets: [X]` and the
  same seed and paths, then reads `targets[0]` at day D. Same seed means
  the same paths as the cone on screen, so the probability always agrees
  with the picture. Both numbers are shown: P(b ≥ X on D) and P(b ≥ X on
  some day by D). Either can be the question, and they differ.
- **Where it runs.** On the main thread at first. If a 2000-path, 90-day
  forecast takes over ~150 ms in the browser, move the WASM into a Web
  Worker. The API is already string in, string out, so that is mechanical.
  *As built:* measured at 150–250 ms (about 400 ms at 180 days) in release
  WASM, so the engine runs in a module worker (`src/wasm/worker.ts`), with
  a same-thread engine of the same interface for tests.

## 5. App structure (`web/`)

```
web/
  package.json        scripts: wasm, dev, build, test, typecheck
  vite.config.ts      Vite + React; Vitest config (jsdom)
  src/
    schema.gen.ts     generated by ts-rs, never edited by hand
    wasm/engine.ts    the only importer of wasm/pkg: init + typed wrappers
    wasm/protocol.ts  worker messages; `handle()` is the worker's whole job
    wasm/worker.ts    runs the engine off the main thread
    wasm/client.ts    async Engine: the worker, or same-thread for tests
    wasm/pkg/         wasm-pack output (gitignored)
    App.tsx           state: source → feed → report
    components/       Header, StatePanel, ConeChart, ViewToggle,
                      AffordQuery, FeedDrop
    format.ts         money and date *formatting* only
    styles.css        design tokens (CSS variables), dark first
  test/               Vitest: components (jsdom) and the WASM bridge (node)
```

- **Dependencies:** react, react-dom; dev: vite, @vitejs/plugin-react,
  typescript, vitest, jsdom, @testing-library/react. Nothing else: no chart
  library, no state library (one `useReducer`), no CSS framework, no web
  fonts (system UI and monospace stacks, so it works offline).
- **WASM bridge tests** load the real `.wasm` from disk under Node and run
  `simulate` → `forecast` end to end. They check that the bridge returns
  typed objects, that errors surface as exceptions with the core's message,
  and that two calls with the same seed return identical strings.
- **Component tests** render with a fixture report produced by the engine,
  not a hand-made one, so they exercise real shapes.

## 6. Out of scope for Phase 2

Persisting feeds in the browser, live Plaid Link, editing data, multiple
accounts views, mobile layout polish beyond "usable at phone width"
(checked at 390 px, dark and light, with headless Chrome), a Web Worker
unless measured necessary.
