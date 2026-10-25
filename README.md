# EPHEMERIS

A personal financial trajectory engine. It estimates your true financial state
with a Kalman filter and projects it forward as a Monte Carlo probability cone,
not a single number. One Rust core, shipped to iOS, Android, Web, and Desktop.

Status: Phase 1 (core engine, CLI) complete; Phase 2 (web, via WASM) in
progress. Full spec: docs/spec.html

## Known gaps

Phase 1 is done, with these gaps stated plainly. Details and measurements
are in docs/phase1-design.md and docs/sandbox-comparison.md.

- **The spend-rate band can go negative.** The state is Gaussian, so a wide
  `c` gives a ±2σ band such as −$5 to $139/day (the Plaid sandbox after its
  regime changes). Net discretionary flow can be an inflow, so this is not
  impossible, but for most people it means "very unsure", not "earning".
  The cone itself is fine: it samples the same Gaussian. A truncated or
  log-scale model for `c` is not planned yet.
- **Irregular income is not income.** Income is the largest *regular*
  inflow, found by fitting a pay schedule. Gig income, commissions, or a
  one-off wages deposit are treated as unexplained inflows (lumps), so the
  forecast has no paychecks for them. One primary income stream only.
- **The first week after a regime change is overconfident.** The change
  detector needs a run of evidence (median 9 days to flag a doubling of
  spending on model-matched data). Until it fires, the cone is narrow
  around the old spend rate: 44% coverage of the 90% interval one week
  after a doubling. A halving of spending is found slowly, and a pay cut
  only at the second short paycheck.
- **Real-data validation is thin.** The only real-shaped data is one Plaid
  sandbox item: one checking account, a perfectly flat weekly payroll, two
  monthly streams, static balances (so the reconstructed balance history is
  fictional), and no pending→posted links. Everything else is validated on
  the synthetic generator, which shares the author's assumptions.
