//! UniFFI bindings for `ephemeris-core` (docs/phase3-design.md §1).
//!
//! The same JSON entry points as the WASM and desktop builds, exported to
//! Swift: feed and options in as JSON text, the feed or report out as JSON
//! text, written by the one `serde_json` call in `ephemeris_core::json`. So
//! iOS returns the golden files byte for byte, like every other shell, and
//! Swift decodes the report with its own `Codable` types.

#![forbid(unsafe_code)]

use ephemeris_core::json;
use ephemeris_core::schema::SCHEMA_VERSION;

uniffi::setup_scaffolding!();

/// An engine error: bad JSON, unknown options, an invalid feed. Swift sees
/// it as a thrown `EngineError.Failed(message:)`.
#[derive(Debug, PartialEq, Eq, uniffi::Error)]
pub enum EngineError {
    Failed { message: String },
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<String> for EngineError {
    fn from(message: String) -> Self {
        Self::Failed { message }
    }
}

/// The `schema_version` of the feeds this build reads and the reports it writes.
#[uniffi::export]
pub fn schema_version() -> u32 {
    SCHEMA_VERSION
}

/// A synthetic feed (JSON) from `SimulateOptions` (JSON; `{}` for defaults).
#[uniffi::export]
pub fn simulate_json(options_json: String) -> Result<String, EngineError> {
    Ok(json::simulate_json(&options_json)?)
}

/// A `ForecastReport` (JSON) from a feed (JSON) and `ForecastOptions` (JSON).
#[uniffi::export]
pub fn forecast_json(feed_json: String, options_json: String) -> Result<String, EngineError> {
    Ok(json::forecast_json(&feed_json, &options_json)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The golden files and their inputs, shared with every other shell's
    // byte-identity test (docs/phase2-design.md §2).
    include!("../../ephemeris-wasm/tests/common/golden_inputs.rs");
    const GOLDEN_FEED: &str = include_str!("../../ephemeris-wasm/tests/golden/feed.json");
    const GOLDEN_REPORT: &str = include_str!("../../ephemeris-wasm/tests/golden/report.json");

    #[test]
    fn exported_functions_match_the_golden_files() {
        let feed = simulate_json(SIMULATE_OPTIONS.into()).unwrap();
        assert!(feed == GOLDEN_FEED, "feed differs from the golden file");
        let report = forecast_json(feed, FORECAST_OPTIONS.into()).unwrap();
        assert!(
            report == GOLDEN_REPORT,
            "report differs from the golden file ({} vs {} bytes)",
            report.len(),
            GOLDEN_REPORT.len()
        );
    }

    #[test]
    fn errors_carry_the_engine_message() {
        let EngineError::Failed { message } =
            forecast_json("not json".into(), "{}".into()).unwrap_err();
        assert!(message.starts_with("feed:"), "{message}");
        let err = simulate_json(r#"{"dayz": 3}"#.into()).unwrap_err();
        assert!(err.to_string().contains("dayz"), "{err}");
    }

    #[test]
    fn schema_version_is_the_cores() {
        assert_eq!(schema_version(), 1);
    }
}
