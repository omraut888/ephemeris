# EPHEMERIS — notes for Claude Code

## What this is
A financial trajectory engine: a Kalman filter for true-state estimation, then
Monte Carlo simulation for a forecast cone. Full spec: docs/spec.html

## Current phase: 1 — core engine, CLI only
- Get the math right before any UI. No web/mobile/desktop code yet.
- Data source for now: Plaid sandbox (synthetic). Keys go in .env, never committed.

## Layout
- crates/ephemeris-core: pure engine library. No I/O, no network, no printing.
  It will later compile to WASM and UniFFI bindings, so keep it portable.
- crates/ephemeris-cli: thin wrapper for loading data, calling core, and printing results.

## Rules
- Use f64 throughout. Treat covariance matrices as symmetric PSD and test for it.
- Monte Carlo runs must accept a seed so results are reproducible in tests.
- Every math component gets unit tests against hand-computed cases.
- Avoid `unsafe`. Run `cargo fmt` and `cargo clippy -- -D warnings` clean before you finish.
- Keep dependencies minimal and justify any new crate.
