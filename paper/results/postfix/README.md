# The post-freeze fix's effect on frozen results

The frozen development and held-out results were **not** regenerated.
Instead, V0 and V2 were rerun with the fixed engine (commit `f98fed5`,
`rand_distr` without std) on the first 100 development accounts after the
drift exclusion, with `ephemeris-eval variants --limit 100`. The rows were
then compared with the frozen development rows, forecast by forecast, by
`paper/analysis/postfix_delta.py`.

**Result.**
- 97 of those 100 accounts have eligible origins: 3,105 origins and
  18,630 V0 and V2 forecasts.
- Every forecast's scored row is identical to the frozen one: coverage,
  CRPS, width, median and PIT, at full printed precision. No coverage flips.
- So the fix's effect on these results is exactly zero.
- A quantile changes only when a path an ulp away lands on a reported
  level. That happened once in the 100 identity cases and never here.

This does not prove that no held-out number would move. It is a sample of
100 accounts, chosen as the first 100 by id, not at random.
