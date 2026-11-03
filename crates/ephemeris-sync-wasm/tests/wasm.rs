//! The sync golden check, WASM leg (docs/phase4b-design.md §5): open both
//! sealed replicas, merge in this shell's own order (B, then A), and get the
//! native golden state, feed and report byte for byte. Run with
//! `wasm-pack test --node --release crates/ephemeris-sync-wasm`.

#![cfg(target_arch = "wasm32")]

use ephemeris_core::json::forecast_json;
use ephemeris_sync_wasm::{
    sync_edit, sync_forecast_options, sync_key_from_text, sync_materialize, sync_merge, sync_new,
    sync_open,
};
use wasm_bindgen_test::wasm_bindgen_test;

/// The state inside `sync_open`'s result, taken out without re-encoding,
/// as JavaScript's `JSON.parse` would.
fn state(file: &[u8], key: &[u8]) -> String {
    let opened: serde_json::Value = serde_json::from_str(&sync_open(file, key).unwrap()).unwrap();
    opened["state"].as_str().unwrap().to_string()
}

#[wasm_bindgen_test]
fn sync_replicas_merge_to_the_native_golden_files() {
    let key = sync_key_from_text(include_str!("../../ephemeris-sync/tests/golden/key.txt").trim())
        .unwrap();
    let a = state(
        include_bytes!("../../ephemeris-sync/tests/golden/replica-a.ephsync"),
        &key,
    );
    let b = state(
        include_bytes!("../../ephemeris-sync/tests/golden/replica-b.ephsync"),
        &key,
    );
    let merged = sync_merge(&b, &a).unwrap();
    assert!(
        merged == include_str!("../../ephemeris-sync/tests/golden/merged-state.json"),
        "merged state differs"
    );
    let feed = sync_materialize(&merged).unwrap();
    assert!(
        feed == include_str!("../../ephemeris-sync/tests/golden/merged-feed.json"),
        "feed differs"
    );
    let report = forecast_json(&feed, &sync_forecast_options(&merged).unwrap()).unwrap();
    assert!(
        report == include_str!("../../ephemeris-sync/tests/golden/merged-report.json"),
        "report differs"
    );
}

#[wasm_bindgen_test]
fn errors_cross_the_boundary() {
    assert!(sync_merge("not json", &sync_new()).is_err());
    assert!(sync_edit(&sync_new(), r#"{"edit":"take_fetcher"}"#, "aa", 0.0).is_err());
    assert!(
        sync_edit(
            &sync_new(),
            r#"{"edit":"take_fetcher"}"#,
            "00000000000000aa",
            f64::NAN
        )
        .is_err()
    );
    assert!(sync_open(b"nope", &[0; 32]).is_err());
}
