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

#[wasm_bindgen_test]
fn errors_cross_the_boundary() {
    assert!(simulate(r#"{"days": 0}"#).is_err());
    assert!(forecast("{}", "{}").is_err());
}
