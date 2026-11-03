//! WASM half of the byte-identity proof: the exported functions, compiled
//! to `wasm32`, must produce exactly the golden files that `golden.rs`
//! checks natively. Run with `wasm-pack test --node crates/ephemeris-wasm`.

#![cfg(target_arch = "wasm32")]

use ephemeris_wasm::{forecast, schema_version, simulate};
use wasm_bindgen_test::wasm_bindgen_test;

include!("common/golden_inputs.rs");

const GOLDEN_FEED: &str = include_str!("golden/feed.json");
const GOLDEN_REPORT: &str = include_str!("golden/report.json");

#[wasm_bindgen_test]
fn wasm_output_is_byte_identical_to_native() {
    assert_eq!(schema_version(), 1);
    let feed = simulate(SIMULATE_OPTIONS).unwrap();
    assert!(
        feed == GOLDEN_FEED,
        "feed differs from the native golden file"
    );
    let report = forecast(&feed, FORECAST_OPTIONS).unwrap();
    assert!(
        report == GOLDEN_REPORT,
        "report differs from the native golden file ({} vs {} bytes)",
        report.len(),
        GOLDEN_REPORT.len()
    );
}

/// The sync golden check, WASM leg (docs/phase4b-design.md §5): open both
/// sealed replicas, merge in this shell's own order (B, then A), and get the
/// native golden state, feed and report byte for byte.
#[wasm_bindgen_test]
fn sync_replicas_merge_to_the_native_golden_files() {
    use ephemeris_wasm::{
        sync_forecast_options, sync_key_from_text, sync_materialize, sync_merge, sync_open,
    };
    const G: &str = "../ephemeris-sync/tests/golden/";
    let key = sync_key_from_text(include_str!("../../ephemeris-sync/tests/golden/key.txt").trim())
        .unwrap();
    let state = |file: &[u8]| {
        // The state is a JSON string inside the object: extract it without
        // re-encoding, as JavaScript's JSON.parse would.
        let opened: serde_json::Value =
            serde_json::from_str(&sync_open(file, &key).unwrap()).unwrap();
        opened["state"].as_str().unwrap().to_string()
    };
    let a = state(include_bytes!(
        "../../ephemeris-sync/tests/golden/replica-a.ephsync"
    ));
    let b = state(include_bytes!(
        "../../ephemeris-sync/tests/golden/replica-b.ephsync"
    ));
    let merged = sync_merge(&b, &a).unwrap();
    let golden_state = include_str!("../../ephemeris-sync/tests/golden/merged-state.json");
    assert!(merged == golden_state, "merged state differs ({G})");
    let feed = sync_materialize(&merged).unwrap();
    assert!(
        feed == include_str!("../../ephemeris-sync/tests/golden/merged-feed.json"),
        "feed differs"
    );
    let report = forecast(&feed, &sync_forecast_options(&merged).unwrap()).unwrap();
    assert!(
        report == include_str!("../../ephemeris-sync/tests/golden/merged-report.json"),
        "report differs"
    );
}

#[wasm_bindgen_test]
fn errors_cross_the_boundary() {
    assert!(simulate(r#"{"days": 0}"#).is_err());
    assert!(forecast("{}", "{}").is_err());
}
