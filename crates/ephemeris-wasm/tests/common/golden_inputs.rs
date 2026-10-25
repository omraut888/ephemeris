// Options behind the golden files, shared by `golden.rs` (native) and
// `wasm.rs` (WASM). The realistic generator exercises the pending tail,
// gating, lumps and regime detection, not just the easy path.

/// A realistic synthetic year to date.
const SIMULATE_OPTIONS: &str = r#"{"seed": 7, "days": 240, "mode": "realistic"}"#;

/// Two targets, so the probability curves are covered too.
const FORECAST_OPTIONS: &str =
    r#"{"horizon_days": 90, "paths": 2000, "seed": 11, "targets": [2000, 5000]}"#;
