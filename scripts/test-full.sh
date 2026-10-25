#!/usr/bin/env bash
# Full test suite, including the slow multi-seed calibration sweeps.
# Release mode keeps the sweeps to seconds instead of minutes.
# Extra arguments go to the test binaries, e.g. scripts/test-full.sh --nocapture
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test --release -- --include-ignored "$@"
# The same golden files again, inside WASM under Node: native and WASM must
# produce byte-identical feeds and reports (docs/phase2-design.md §2).
wasm-pack test --node --release crates/ephemeris-wasm
