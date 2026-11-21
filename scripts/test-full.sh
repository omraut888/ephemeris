#!/usr/bin/env bash
# Full test suite, including the slow multi-seed calibration sweeps.
# Release mode keeps the sweeps to seconds instead of minutes.
# Extra arguments go to the test binaries, e.g. scripts/test-full.sh --nocapture
set -euo pipefail
cd "$(dirname "$0")/.."
# one_seed reruns a single seed from $SEED (convergence.rs); it stays strict,
# so it is skipped here and run by hand with SEED set.
cargo test --release -- --include-ignored --skip one_seed "$@"
# The fetch script keeps the Plaid token out of every process's arguments
# (a recording stand-in for `security`; no Plaid, no real Keychain).
python3 -I scripts/test_fetch_plaid_sandbox.py
# The same golden files again, inside WASM under Node: native and WASM must
# produce byte-identical feeds and reports (docs/phase2-design.md §2).
wasm-pack test --node --release crates/ephemeris-wasm
# The sync module, a WASM of its own: the two sealed golden replicas merge
# to the native golden state, feed and report (docs/phase4b-design.md §5).
wasm-pack test --node --release crates/ephemeris-sync-wasm
# The desktop crate is not a default workspace member (it compiles Tauri),
# so it runs here: the golden files again, through Tauri's IPC, plus its
# lints (docs/phase4-design.md §4).
cargo test -p ephemeris-desktop --lib
cargo clippy -p ephemeris-desktop --all-targets -- -D warnings
# The iOS FFI crate, also not a default member: the golden files through its
# exported functions, and its lints (docs/phase3-design.md §1).
cargo test -p ephemeris-ffi
cargo clippy -p ephemeris-ffi --all-targets -- -D warnings
# The iOS engine package on this Mac: the golden files through the FFI, the
# Swift schema drift test, formatting and lanes (docs/phase3-design.md §6).
# Needs Xcode; skipped without it.
if xcrun --find xcodebuild >/dev/null 2>&1; then
  scripts/build-ios.sh --sim-only
  swift test --package-path ios/EphemerisKit --scratch-path ios/build/swiftpm
else
  echo "SKIPPED iOS package tests: Xcode not found" >&2
fi
# The web app: builds the WASM package, then runs Vitest. Needs its npm
# dependencies installed once (cd web && npm ci).
if [ -d web/node_modules ]; then
  (cd web && npm test)
else
  echo "SKIPPED web tests: run 'cd web && npm ci' first" >&2
fi
