//! Native half of the byte-identity proof (docs/phase2-design.md §2).
//!
//! The same feed and report must come out of the native build and the WASM
//! build, byte for byte. Both builds compare against the files in
//! `tests/golden/`: this test natively, `tests/wasm.rs` inside WASM (run with
//! `wasm-pack test --node crates/ephemeris-wasm`). `UPDATE_GOLDEN=1
//! cargo test -p ephemeris-wasm --test golden` rewrites the files, which is
//! only right after a deliberate change to the engine's output.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use ephemeris_wasm::{forecast_json, simulate_json};

include!("common/golden_inputs.rs");

fn check(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_GOLDEN=1)", path.display()));
    assert!(
        expected == actual,
        "{name} differs from the golden file ({} vs {} bytes)",
        expected.len(),
        actual.len()
    );
}

#[test]
fn native_output_matches_the_golden_files() {
    let feed = simulate_json(SIMULATE_OPTIONS).unwrap();
    check("feed.json", &feed);
    let report = forecast_json(&feed, FORECAST_OPTIONS).unwrap();
    check("report.json", &report);
}
