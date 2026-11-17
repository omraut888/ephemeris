# Post-hoc, after the held-out run

Computed on 2026-11-17, after the held-out results (commit `cdf070d`) were
known, at the user's request. It is not part of the pre-registered
pipeline, and the paper labels it as post-hoc (paper/PLAN.md §11).

`pairwise-v2.csv`: V2 against each baseline, paired over the forecasts both
made, at 7/30/90 days. It is computed from the scored rows of the first
held-out run (`paper/build/heldout/`), with no new forecasts:

- mean and pooled CRPS skill (positive means V2 is better);
- V2's 50% and 90% coverage minus the baseline's.

Intervals are 95%, from 2000 resamples of whole accounts. The V2/B-SA+ rows
reproduce the committed skill point estimates. Their intervals differ in
the fourth decimal, because the resampling stream differs.

Rebuild: `scripts/paper-heldout-pairwise.sh`.
