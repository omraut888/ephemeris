# Bit identity on this Mac (claim C1, T10)

Built by `scripts/paper-identity.sh`. The cases are the golden feed plus 100
generated feed-and-option cases (`crates/ephemeris-wasm/tests/common/
identity_cases.rs`). They vary the generator, the history length and start,
the holiday calendar, the horizon, the paths, the seed and the targets.
`identity-manifest.tsv` holds the SHA-256 of each feed and report, written
by the native debug build. That build's row therefore matches by
construction; every other leg is checked against it.

**Result: C1 is falsified as pre-registered** ("falsified by one differing
byte").
- WASM under Node reproduces all 100 feeds but only 99 reports.
- In case 54, one value differs between native arm64 and WASM: day 96,
  quantile index 3, `11773.822056226314` against `11773.822056226316`.
  That is one ulp.
- **The cause.** `rand_distr` 0.6's `f64` `StandardNormal` and `Exp`
  samplers call `.exp()` and `.ln()` on `f64`. In a std build those
  resolve to std's inherent methods, not to `libm`.
  - Natively, that is Apple's libm. In WASM, it is Rust's bundled libm
    port.
  - On this Mac, std's `ln` and `exp` differ from `libm`'s on about 7% and
    10% of random inputs (a 20 M-draw check).
  - The samplers reach them only in their ziggurat tail and wedge
    branches. That is why the difference is rare.

**Not covered:**
- x86_64 beyond the golden feed (the GitHub Actions workflow, run #1 at
  `26ed672`);
- the Tauri IPC path, Vitest and Swift for the 100 cases;
- a physical iPhone.

On x86_64 Linux, std's math is glibc's, so differences there are likely
too.
