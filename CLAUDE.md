# EPHEMERIS — notes for Claude Code

## What this is
A financial trajectory engine: a Kalman filter for true-state estimation, then
Monte Carlo simulation for a forecast cone. Full spec: docs/spec.html

## Phase 1 — core engine, CLI only: complete
- Filter, regime detection, Monte Carlo cone, Plaid sandbox adapter.
  Known gaps: README.md. Design and measurements: docs/phase1-design.md.

## Current phase: 2 — web, in progress
- The core compiles to WASM (crates/ephemeris-wasm) and a Vite + React +
  TypeScript app in web/ draws the cone. Design: docs/phase2-design.md.
- All math stays in Rust. TypeScript only loads data, calls WASM, and draws.
- Phase 4a, the desktop app, is in progress: crates/ephemeris-desktop
  (Tauri v2) runs the engine natively behind the same UI. Design:
  docs/phase4-design.md.
- Phase 3a, the iOS app, is in progress: crates/ephemeris-ffi (UniFFI)
  exposes the engine to a SwiftUI app in ios/. Design and as-built notes:
  docs/phase3-design.md. Android (3b) comes later.
- Phase 4b, sync between the apps, is in progress: the sync crates, the
  CLI's `ephemeris sync` commands, and the desktop and web adapters (Sync
  sheet, folder sync, saved queries) are built; iOS is not. Design and
  as-built notes: docs/phase4b-design.md (§10 for the adapters). One device
  fetches from Plaid, enforced in the sync state (§1.4). The Plaid token and
  the sync key live in the macOS Keychain, never in files; the browser
  holds the sync key in memory for the tab only.
- Data source for now: Plaid sandbox (synthetic). Keys go in .env, never committed.

## Layout
- crates/ephemeris-core: pure engine library. No I/O, no network, no printing.
  It will later compile to WASM and UniFFI bindings, so keep it portable.
- crates/ephemeris-cli: thin wrapper for loading data, calling core, and printing results.
- crates/ephemeris-sync: pure sync state (no I/O, clock or randomness):
  bank records, the tagging that builds the feed, the HLC, the merge, user
  edits and sealed files. Its convergence simulation is
  `cargo test -p ephemeris-sync --test convergence` (soak: `-- --ignored`).
- crates/ephemeris-wasm: wasm-bindgen wrapper; JSON in, JSON out, same schema as the CLI.
  Engine only: its bytes must not change when sync does.
- crates/ephemeris-sync-wasm: the browser's sync exports, a WASM module of
  its own that web/src/sync/web.ts imports only when the Sync sheet opens.
- crates/ephemeris-plaid: Plaid's JSON to bank records and fetch batches
  (pure), for the CLI and the desktop app, either of which can fetch.
- crates/ephemeris-sync-folder: folder transport and the Keychain sync key
  for the native shells (CLI, desktop).
- crates/ephemeris-desktop: the Tauri app. It is not a default workspace
  member, so use `-p ephemeris-desktop` (scripts/test-full.sh does this).
- crates/ephemeris-ffi: UniFFI exports for Swift, JSON in and out like the
  WASM crate. Not a default member either; scripts/build-ios.sh builds the
  XCFramework from it.
- ios/: the Swift package EphemerisKit (Codable schema types with a drift
  test against Rust, the engine client, the app model) and the SwiftUI app
  (XcodeGen, ios/project.yml). `swift test --package-path ios/EphemerisKit`
  runs on the Mac; scripts/test-ios.sh runs the simulator. The simulator
  saturates this 8 GB Mac for minutes after a boot, so run it alone.
- web/: the browser app, and with `--mode desktop` the desktop app's UI. Its
  TypeScript schema types are checked against Rust.

## Rules
- Use f64 throughout. Treat covariance matrices as symmetric PSD and test for it.
- Monte Carlo runs must accept a seed so results are reproducible in tests.
- Every math component gets unit tests against hand-computed cases.
- Avoid `unsafe`. Run `cargo fmt` and `cargo clippy -- -D warnings` clean before you finish.
- Keep dependencies minimal and justify any new crate.
