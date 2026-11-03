//! WASM bindings for `ephemeris-sync` (docs/phase4b-design.md §4.3, §10).
//!
//! A module of its own, apart from the engine's (`ephemeris-wasm`): the web
//! app imports it only when the Sync sheet opens, so the first page load
//! carries none of it. Thin wrappers over `ephemeris_sync::json`.
//!
//! A state is an opaque string to JavaScript: store it and pass it back,
//! never parse and re-encode it. Where it comes back next to other data, it
//! is a JSON string inside the object, which `JSON.parse` returns exactly.
//!
//! No `sync_ingest`: only the Mac fetches from the bank (design §1.4), and it
//! does so natively, so the browser never records a fetch.

#![forbid(unsafe_code)]

use ephemeris_sync::json as sync;
use wasm_bindgen::prelude::*;

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

/// Merge state `b` into state `a`.
#[wasm_bindgen]
pub fn sync_merge(a: &str, b: &str) -> Result<String, JsError> {
    sync::sync_merge(a, b).map_err(js)
}

/// The feed (JSON) the state describes, for the engine's `forecast`.
#[wasm_bindgen]
pub fn sync_materialize(state: &str) -> Result<String, JsError> {
    sync::sync_materialize(state).map_err(js)
}

/// The `ForecastOptions` (JSON) the synced settings ask for.
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
