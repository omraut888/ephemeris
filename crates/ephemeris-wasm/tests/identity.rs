//! Extended byte identity (paper/PLAN.md §1 claim C1, T10): the 100
//! generated cases of `common/identity_cases.rs`. The native build writes
//! and checks `golden/identity-manifest.tsv`; the WASM build (under
//! `wasm-pack test --node`) and the FFI crate check against the same
//! manifest. `UPDATE_GOLDEN=1 cargo test -p ephemeris-wasm --test
//! identity` rewrites it, which is only right after a deliberate change to
//! the engine's output.

use ephemeris_wasm::{forecast_json, simulate_json};

include!("common/identity_cases.rs");

const MANIFEST: &str = include_str!("golden/identity-manifest.tsv");

fn lines() -> Vec<String> {
    (0..IDENTITY_CASES)
        .map(|i| {
            identity_line(
                i,
                |s| simulate_json(s).expect("simulate"),
                |f, o| forecast_json(f, o).expect("forecast"),
            )
        })
        .collect()
}

fn compare(actual: &[String]) {
    let expected: Vec<&str> = MANIFEST.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(
        expected.len(),
        actual.len(),
        "manifest has {} cases",
        expected.len()
    );
    let differ: Vec<String> = expected
        .iter()
        .zip(actual)
        .filter(|(e, a)| *e != a)
        .map(|(e, a)| format!("expected {e} / got {a}"))
        .collect();
    assert!(
        differ.is_empty(),
        "{} cases differ: {differ:?}",
        differ.len()
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_matches_the_identity_manifest() {
    let actual = lines();
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden/identity-manifest.tsv");
        let mut text =
            String::from("# case\tfeed_sha256\treport_sha256 (tests/common/identity_cases.rs)\n");
        for l in &actual {
            text.push_str(l);
            text.push('\n');
        }
        std::fs::write(path, text).unwrap();
        return;
    }
    compare(&actual);
}

/// Failed for the frozen engine on case 54 (one ulp; paper/PLAN.md §11,
/// C1); passes since the post-freeze fix that keeps `rand_distr` off std.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen_test::wasm_bindgen_test]
fn wasm_matches_the_identity_manifest() {
    compare(&lines());
}
