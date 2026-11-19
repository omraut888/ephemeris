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
use ephemeris_sync::json as sync;

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

// Sync (docs/phase4b-design.md §4.3): thin wrappers over
// `ephemeris_sync::json`. Swift keeps a state as an opaque String and
// passes it back; it never decodes and re-encodes one.

/// A new, empty sync state.
#[uniffi::export]
pub fn sync_new() -> String {
    sync::sync_new()
}

/// Apply an edit (JSON): `{"state": "<state JSON>", "query_id": string | null}`.
#[uniffi::export]
pub fn sync_edit(
    state: String,
    edit_json: String,
    replica: String,
    now_ms: u64,
) -> Result<String, EngineError> {
    Ok(sync::sync_edit(&state, &edit_json, &replica, now_ms)?)
}

/// Record a fetch batch (JSON) made on this device.
#[uniffi::export]
pub fn sync_ingest(
    state: String,
    batch_json: String,
    replica: String,
    now_ms: u64,
) -> Result<String, EngineError> {
    Ok(sync::sync_ingest(&state, &batch_json, &replica, now_ms)?)
}

/// Merge state `b` into state `a`.
#[uniffi::export]
pub fn sync_merge(a: String, b: String) -> Result<String, EngineError> {
    Ok(sync::sync_merge(&a, &b)?)
}

/// The feed (JSON) the state describes, for `forecast_json`.
#[uniffi::export]
pub fn sync_materialize(state: String) -> Result<String, EngineError> {
    Ok(sync::sync_materialize(&state)?)
}

/// The `ForecastOptions` (JSON) the synced settings ask for.
#[uniffi::export]
pub fn sync_forecast_options(state: String) -> Result<String, EngineError> {
    Ok(sync::sync_forecast_options(&state)?)
}

/// What the Sync sheet shows of a state (JSON): `{"has_data", "fetched_through",
/// "fetcher"}`, the same summary the browser's module gives.
#[uniffi::export]
pub fn sync_summary(state: String) -> Result<String, EngineError> {
    Ok(sync::sync_summary(&state)?)
}

/// The saved queries (JSON array).
#[uniffi::export]
pub fn sync_saved_queries(state: String) -> Result<String, EngineError> {
    Ok(sync::sync_saved_queries(&state)?)
}

/// The state sealed as this replica's file, with a 24-byte random nonce.
#[uniffi::export]
pub fn sync_seal(
    state: String,
    replica: String,
    key: Vec<u8>,
    nonce: Vec<u8>,
) -> Result<Vec<u8>, EngineError> {
    Ok(sync::sync_seal(&state, &replica, &key, &nonce)?)
}

/// A sealed file opened: `{"replica": "<16 hex>", "state": "<state JSON>"}`.
#[uniffi::export]
pub fn sync_open(file: Vec<u8>, key: Vec<u8>) -> Result<String, EngineError> {
    Ok(sync::sync_open(&file, &key)?)
}

/// The 32-byte key as the text people copy between devices.
#[uniffi::export]
pub fn sync_key_to_text(key: Vec<u8>) -> Result<String, EngineError> {
    Ok(sync::sync_key_to_text(&key)?)
}

/// The key from that text.
#[uniffi::export]
pub fn sync_key_from_text(text: String) -> Result<Vec<u8>, EngineError> {
    Ok(sync::sync_key_from_text(&text)?)
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

    // The extended identity check (paper/PLAN.md C1, T10): the 100
    // generated cases through the exported functions, against the manifest
    // the native build wrote.
    include!("../../ephemeris-wasm/tests/common/identity_cases.rs");
    const IDENTITY_MANIFEST: &str =
        include_str!("../../ephemeris-wasm/tests/golden/identity-manifest.tsv");

    #[test]
    fn exported_functions_match_the_identity_manifest() {
        let expected: Vec<&str> = IDENTITY_MANIFEST
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect();
        assert_eq!(expected.len(), IDENTITY_CASES as usize);
        for (i, e) in (0..IDENTITY_CASES).zip(expected) {
            let line = identity_line(
                i,
                |s| simulate_json(s.into()).unwrap(),
                |f, o| forecast_json(f.into(), o.into()).unwrap(),
            );
            assert_eq!(line, e, "case {i}");
        }
    }

    #[test]
    fn errors_carry_the_engine_message() {
        let EngineError::Failed { message } =
            forecast_json("not json".into(), "{}".into()).unwrap_err();
        assert!(message.starts_with("feed:"), "{message}");
        let err = simulate_json(r#"{"dayz": 3}"#.into()).unwrap_err();
        assert!(err.to_string().contains("dayz"), "{err}");
    }

    /// The sync golden check, FFI leg in Rust (docs/phase4b-design.md §5):
    /// this shell merges B into A.
    #[test]
    fn sync_replicas_merge_to_the_golden_files() {
        let key = sync_key_from_text(
            include_str!("../../ephemeris-sync/tests/golden/key.txt")
                .trim()
                .into(),
        )
        .unwrap();
        let state = |file: &[u8]| {
            let opened = sync_open(file.to_vec(), key.clone()).unwrap();
            let v: serde_json::Value = serde_json::from_str(&opened).unwrap();
            v["state"].as_str().unwrap().to_string()
        };
        let a = state(include_bytes!(
            "../../ephemeris-sync/tests/golden/replica-a.ephsync"
        ));
        let b = state(include_bytes!(
            "../../ephemeris-sync/tests/golden/replica-b.ephsync"
        ));
        let merged = sync_merge(a, b).unwrap();
        assert!(merged == include_str!("../../ephemeris-sync/tests/golden/merged-state.json"));
        let feed = sync_materialize(merged.clone()).unwrap();
        assert!(feed == include_str!("../../ephemeris-sync/tests/golden/merged-feed.json"));
        let report = forecast_json(feed, sync_forecast_options(merged.clone()).unwrap()).unwrap();
        assert!(report == include_str!("../../ephemeris-sync/tests/golden/merged-report.json"));
        // Golden replica A (…aa) fetches.
        let summary: serde_json::Value =
            serde_json::from_str(&sync_summary(merged).unwrap()).unwrap();
        assert_eq!(summary["fetcher"], "00000000000000aa");
        assert_eq!(summary["has_data"], true);
        assert!(summary["fetched_through"].is_string());
        let empty: serde_json::Value =
            serde_json::from_str(&sync_summary(sync_new()).unwrap()).unwrap();
        assert_eq!(
            empty,
            serde_json::json!({"has_data": false, "fetched_through": null, "fetcher": null})
        );
    }

    #[test]
    fn schema_version_is_the_cores() {
        assert_eq!(schema_version(), 1);
    }
}
