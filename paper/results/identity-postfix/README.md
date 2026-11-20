# Bit identity after the post-freeze fix (claim C1)

This is the same check as `../identity/`, run on the engine after commit
`f98fed5`. That commit builds `rand_distr` without its `std` feature, so
its math goes through `libm` on every platform. The realistic generator's
lognormal `ln`/`exp` call `libm` too. Built by
`scripts/paper-identity.sh paper/results/identity-postfix`.

This is a **post-freeze** result. The pre-registered outcome for the frozen
engine, falsified, is in `../identity/` and stands.

- **All 100 cases agree** on native arm64 (debug and release), UniFFI and
  WASM under Node, feeds and reports alike.
- **The fix changed one report natively:** case 54's. The native build now
  produces exactly the value WASM produced before. The manifest was
  regenerated only after every leg agreed.
- **The golden feed and report are unchanged on every shell:** native,
  WASM, UniFFI, the Tauri IPC path, Vitest and the Swift package.
- **Not covered:** x86_64 beyond the golden feed (the workflow, run #1 at
  `26ed672`, before the fix), and a physical iPhone.
