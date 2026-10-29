//! The engine as JSON text in, JSON text out (docs/phase4-design.md §2).
//!
//! Every shell calls these two functions: the WASM build through
//! `wasm-bindgen`, the desktop app through Tauri commands. One `serde_json`
//! call writes the output for all of them, so a report is byte-identical
//! whichever shell computed it (the golden files in `ephemeris-wasm`).
//! Errors are messages that name the problem, never panics.

use crate::pipeline::{ForecastOptions, SimulateOptions, forecast_feed, simulate_feed};
use crate::schema::Feed;

/// A synthetic feed (JSON) from `SimulateOptions` (JSON; `{}` for defaults).
pub fn simulate_json(options_json: &str) -> Result<String, String> {
    let opts: SimulateOptions =
        serde_json::from_str(options_json).map_err(|e| format!("simulate options: {e}"))?;
    let feed = simulate_feed(&opts).map_err(|e| e.to_string())?;
    serde_json::to_string(&feed).map_err(|e| e.to_string())
}

/// A `ForecastReport` (JSON) from a feed (JSON) and `ForecastOptions` (JSON).
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
