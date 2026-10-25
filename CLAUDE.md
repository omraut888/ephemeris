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
- Mobile and desktop are still later phases.
- Data source for now: Plaid sandbox (synthetic). Keys go in .env, never committed.

## Layout
- crates/ephemeris-core: pure engine library. No I/O, no network, no printing.
  It will later compile to WASM and UniFFI bindings, so keep it portable.
- crates/ephemeris-cli: thin wrapper for loading data, calling core, and printing results.
- crates/ephemeris-wasm: wasm-bindgen wrapper; JSON in, JSON out, same schema as the CLI.
- web/: the browser app. Its TypeScript schema types are checked against Rust.

## Rules
- Use f64 throughout. Treat covariance matrices as symmetric PSD and test for it.
- Monte Carlo runs must accept a seed so results are reproducible in tests.
- Every math component gets unit tests against hand-computed cases.
- Avoid `unsafe`. Run `cargo fmt` and `cargo clippy -- -D warnings` clean before you finish.
- Keep dependencies minimal and justify any new crate.
