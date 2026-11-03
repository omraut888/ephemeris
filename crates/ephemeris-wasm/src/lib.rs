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
use ephemeris_sync::json as sync;
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

// Sync (docs/phase4b-design.md §4.3): thin wrappers over
// `ephemeris_sync::json`. A state is an opaque string to JavaScript: store
// it and pass it back, never parse and re-encode it.

fn js(e: String) -> JsError {
    JsError::new(&e)
}

/// Milliseconds since the epoch, as JavaScript's `Date.now()` gives them.
fn millis(now_ms: f64) -> Result<u64, JsError> {
    if now_ms.is_finite() && (0.0..=9_007_199_254_740_991.0).contains(&now_ms) {
        Ok(now_ms as u64)
    } else {
        Err(JsError::new(
            "now_ms must be milliseconds since 1970, like Date.now()",
        ))
    }
}

/// A new, empty sync state.
#[wasm_bindgen]
pub fn sync_new() -> String {
    sync::sync_new()
}

/// Apply an edit (JSON): `{"state": "<state JSON>", "query_id": string | null}`.
#[wasm_bindgen]
pub fn sync_edit(
    state: &str,
    edit_json: &str,
    replica: &str,
    now_ms: f64,
) -> Result<String, JsError> {
    sync::sync_edit(state, edit_json, replica, millis(now_ms)?).map_err(js)
}

/// Record a fetch batch (JSON) made on this device.
#[wasm_bindgen]
pub fn sync_ingest(
    state: &str,
    batch_json: &str,
    replica: &str,
    now_ms: f64,
) -> Result<String, JsError> {
    sync::sync_ingest(state, batch_json, replica, millis(now_ms)?).map_err(js)
}

/// Merge state `b` into state `a`.
#[wasm_bindgen]
pub fn sync_merge(a: &str, b: &str) -> Result<String, JsError> {
    sync::sync_merge(a, b).map_err(js)
}

/// The feed (JSON) the state describes, for `forecast`.
#[wasm_bindgen]
pub fn sync_materialize(state: &str) -> Result<String, JsError> {
    sync::sync_materialize(state).map_err(js)
}

/// The `ForecastOptions` (JSON) the synced settings ask for, for `forecast`.
#[wasm_bindgen]
pub fn sync_forecast_options(state: &str) -> Result<String, JsError> {
    sync::sync_forecast_options(state).map_err(js)
}

/// The saved queries (JSON array).
#[wasm_bindgen]
pub fn sync_saved_queries(state: &str) -> Result<String, JsError> {
    sync::sync_saved_queries(state).map_err(js)
}

/// The state sealed as this replica's file, with a 24-byte random nonce.
#[wasm_bindgen]
pub fn sync_seal(state: &str, replica: &str, key: &[u8], nonce: &[u8]) -> Result<Vec<u8>, JsError> {
    sync::sync_seal(state, replica, key, nonce).map_err(js)
}

/// A sealed file opened: `{"replica": "<16 hex>", "state": "<state JSON>"}`.
#[wasm_bindgen]
pub fn sync_open(file: &[u8], key: &[u8]) -> Result<String, JsError> {
    sync::sync_open(file, key).map_err(js)
}

/// The 32-byte key as the text people copy between devices.
#[wasm_bindgen]
pub fn sync_key_to_text(key: &[u8]) -> Result<String, JsError> {
    sync::sync_key_to_text(key).map_err(js)
}

/// The key from that text.
#[wasm_bindgen]
pub fn sync_key_from_text(text: &str) -> Result<Vec<u8>, JsError> {
    sync::sync_key_from_text(text).map_err(js)
}
