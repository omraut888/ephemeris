// Options behind the golden files, shared by `golden.rs` (native), `wasm.rs`
// (WASM), the desktop crate's IPC test and the iOS package's Swift test. The
// text lives in `tests/golden/*-options.json` so Swift reads the same bytes.
// The realistic generator exercises the pending tail, gating, lumps and
// regime detection, not just the easy path.

/// A realistic synthetic year to date.
const SIMULATE_OPTIONS: &str = include_str!("../golden/simulate-options.json");

/// Two targets, so the probability curves are covered too.
const FORECAST_OPTIONS: &str = include_str!("../golden/forecast-options.json");
