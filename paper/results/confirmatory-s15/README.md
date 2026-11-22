# S15: the forward-period confirmatory test (held-out)

Specification: paper/REVISION.md S15, amended with P3b in 44afcf4 before the run. Run once through the guard (paper/results/heldout-runs.log, 2026-11-22T00:53:43Z, commit 44afcf4) on held-out origins dated on or after 1997-07-01. Summary: paper/analysis/explore_summary.py --heldout s15.

- predictions.csv: each prediction, both aggregations with 95% account-bootstrap intervals, and its verdict under the primary estimand (account mean).
- stats.csv, compare.csv: the methods F0 (B-EQ), F1 (B-SR), O (oracle), EQ-shuf on these origins.
