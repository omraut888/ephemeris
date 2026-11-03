//! WASM bindings for `ephemeris-core` (docs/phase2-design.md §2).
//!
//! Everything crosses the boundary as JSON text produced by `serde_json`,
//! the same writer the CLI uses, so a report computed here is byte-identical
//! to one computed natively from the same feed, options and seed. The
//! exports are thin wrappers over [`simulate_json`] and [`forecast_json`]
//! from `ephemeris_core::json`, which the desktop app calls too.

#![forbid(unsafe_code)]

#[doc(inline)]
pub use ephemeris_core::json::{forecast_json, simulate_json};
use ephemeris_core::schema::SCHEMA_VERSION;
use wasm_bindgen::prelude::*;

/// The `schema_version` of the feeds this build reads and the reports it writes.
#[wasm_bindgen]
pub fn schema_version() -> u32 {
    SCHEMA_VERSION
}

/// A synthetic feed (JSON) from `SimulateOptions` (JSON; `{}` for defaults).
#[wasm_bindgen]
pub fn simulate(options_json: &str) -> Result<String, JsError> {
    simulate_json(options_json).map_err(|e| JsError::new(&e))
}

/// A `ForecastReport` (JSON) from a feed (JSON) and `ForecastOptions` (JSON).
#[wasm_bindgen]
pub fn forecast(feed_json: &str, options_json: &str) -> Result<String, JsError> {
    forecast_json(feed_json, options_json).map_err(|e| JsError::new(&e))
}
