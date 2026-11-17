# Held-out ablations (D1): the second held-out run

This is the pre-registered ablation set of paper/PLAN.md §3 and claim D1,
run on every held-out account. Decision 5 of §8 required it, but the first
held-out run left it out, through a harness gap: the ablation sample was
drawn from the development split only. It was run on 2026-11-17, after the
main held-out results (commit `cdf070d`) were known. It is logged as the
second entry in `paper/results/heldout-runs.log`, with the user's rerun
reason. The paper discloses that order.

- The code is at `ae6b74a`. The only change from the first run is which
  accounts get the ablations: all held-out accounts.
- The run recomputed every method for these accounts. Its non-ablation
  rows are byte-identical to the first run's, a check made by
  `scripts/paper-heldout-ablations.sh`. The primary results were not
  regenerated.
- `ablations.csv` and `ablations-pit.csv` have the same format as
  development's.
- `ablations-paired.csv` holds paired differences of each ablation against
  the full model (V0, `EPHEMERIS`): CRPS skill (positive means the
  ablation is better) and coverage differences, with account-bootstrap
  95% intervals. It covers all forecasts and the post-alarm and quiet
  strata. These paired intervals are an addition over development's
  tables.
