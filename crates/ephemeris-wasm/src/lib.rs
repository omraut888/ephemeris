//! WASM bindings for `ephemeris-core` (docs/phase2-design.md §2).
//!
//! Everything crosses the boundary as JSON text produced by `serde_json`,
//! the same writer the CLI uses, so a report computed here is byte-identical
//! to one computed natively from the same feed, options and seed. The
//! exports are thin wrappers over [`simulate_json`] and [`forecast_json`],
//! which are plain Rust and so testable natively.

#![forbid(unsafe_code)]

use ephemeris_core::pipeline::{ForecastOptions, SimulateOptions, forecast_feed, simulate_feed};
use ephemeris_core::schema::{Feed, SCHEMA_VERSION};
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

/// [`simulate`] without the JavaScript error type.
pub fn simulate_json(options_json: &str) -> Result<String, String> {
    let opts: SimulateOptions =
        serde_json::from_str(options_json).map_err(|e| format!("simulate options: {e}"))?;
    let feed = simulate_feed(&opts).map_err(|e| e.to_string())?;
    serde_json::to_string(&feed).map_err(|e| e.to_string())
}

/// [`forecast`] without the JavaScript error type.
pub fn forecast_json(feed_json: &str, options_json: &str) -> Result<String, String> {
    let feed: Feed = serde_json::from_str(feed_json).map_err(|e| format!("feed: {e}"))?;
    let opts: ForecastOptions =
        serde_json::from_str(options_json).map_err(|e| format!("forecast options: {e}"))?;
    let report = forecast_feed(&feed, &opts).map_err(|e| e.to_string())?;
    serde_json::to_string(&report).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_name_the_problem() {
        let err = simulate_json(r#"{"dayz": 3}"#).unwrap_err();
        assert!(
            err.starts_with("simulate options:") && err.contains("dayz"),
            "{err}"
        );
        let err = simulate_json(r#"{"days": 0}"#).unwrap_err();
        assert!(err.contains("days must be"), "{err}");
        let err = forecast_json("not json", "{}").unwrap_err();
        assert!(err.starts_with("feed:"), "{err}");
        let feed = simulate_json(r#"{"days": 60}"#).unwrap();
        let err = forecast_json(&feed, r#"{"paths": 0}"#).unwrap_err();
        assert!(err.contains("n_paths"), "{err}");
        let future = feed.replacen("\"schema_version\":1", "\"schema_version\":2", 1);
        assert!(
            forecast_json(&future, "{}")
                .unwrap_err()
                .contains("schema_version")
        );
    }
}
