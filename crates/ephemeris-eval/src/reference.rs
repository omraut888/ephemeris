//! The reference flow scale for normalizing real accounts (paper/PLAN.md §8
//! decision 2).
//!
//! `REFERENCE_SCALE` is the median monthly gross flow `Σ|amount|` of the
//! realistic synthetic generator: seeds 0–99, one year from 2024-01-01,
//! the months that lie wholly inside the generated days, over the
//! non-transfer records of the last day's snapshot (by their `day()`).
//! The engine's priors and clamps were tuned on that generator, so a real
//! account multiplied by `REFERENCE_SCALE / s` has the flow scale the
//! engine was tuned for. The constant is computed once and pinned; the test
//! recomputes it.

use ephemeris_core::calendar::{civil_from_day, day_from_civil, days_in_month};
use ephemeris_core::synth::{SynthConfig, generate};
use ephemeris_core::{Day, FlowKind};

use crate::berka::median;

/// See the module docs; recomputed by [`compute_reference_scale`].
pub const REFERENCE_SCALE: f64 = 8412.542329576623;

/// 2024-01-01.
pub const SYNTH_START: Day = 19_723;

pub fn compute_reference_scale() -> f64 {
    let cfg = SynthConfig::realistic(SYNTH_START);
    let last = SYNTH_START + cfg.n_days as Day - 1;
    let mut flows = Vec::new();
    for seed in 0..100 {
        let run = generate(&cfg, seed).expect("generator");
        let mut months = std::collections::BTreeMap::<(i32, u32), f64>::new();
        for t in run.snapshot(last) {
            if t.kind == FlowKind::Transfer {
                continue;
            }
            let (y, m, _) = civil_from_day(t.day());
            *months.entry((y, m)).or_default() += t.amount.abs();
        }
        for (&(y, m), &f) in &months {
            let whole = day_from_civil(y, m, 1) >= SYNTH_START
                && day_from_civil(y, m, days_in_month(y, m)) <= last;
            if whole {
                flows.push(f);
            }
        }
    }
    median(&mut flows).expect("months")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_reference_scale_is_the_computed_one() {
        let r = compute_reference_scale();
        assert_eq!(r, REFERENCE_SCALE, "recomputed {r:?}");
    }
}
