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
# The desktop crate is not a default workspace member (it compiles Tauri),
# so it runs here: the golden files again, through Tauri's IPC, plus its
# lints (docs/phase4-design.md §4).
cargo test -p ephemeris-desktop --lib
cargo clippy -p ephemeris-desktop --all-targets -- -D warnings
# The web app: builds the WASM package, then runs Vitest. Needs its npm
# dependencies installed once (cd web && npm ci).
if [ -d web/node_modules ]; then
  (cd web && npm test)
else
  echo "SKIPPED web tests: run 'cd web && npm ci' first" >&2
fi
