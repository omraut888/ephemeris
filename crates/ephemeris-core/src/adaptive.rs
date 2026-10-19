//! Adaptive noise variances by covariance matching (design §3).
//!
//! Each channel has one unknown variance. Every innovation yields an unbiased
//! single-sample estimate of it (for example `ν² − h·P⁻·hᵀ` for the income
//! channel), which is folded into an exponentially forgetting weighted mean:
//!
//! ```text
//! n ← λ·n + w
//! v ← clamp(v + w·(sample − v)/n, min, max)
//! ```
//!
//! The weight `w ∈ [0, 1]` is the sample's information relative to an ideal
//! one: a sample taken while the state is still uncertain (innovation variance
//! `S` much larger than the variance being estimated) is nearly worthless, so
//! callers pass `w = (v/S)²` via [`sample_weight`]. `n` is the effective sample
//! count; it starts at the prior pseudo-count and is reported as degrees of
//! freedom.
//!
//! Plugging the current estimate in for `v` would lock out a prior that is far
//! too small: `S` would always look huge next to it and every weight would be
//! ~0. So `v` is `max(current, sample/10)`. A diffuse state (huge `S`, sample
//! near or below zero) still gets a tiny weight; a sample far above the
//! current value gets heard. Only samples above 10× the current value are
//! up-weighted, which keeps the bias small (see [`WEIGHT_SAMPLE_DIVISOR`]).

use crate::error::EngineError;

/// A variance and how many effective samples support it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VarianceEstimate {
    pub value: f64,
    /// Effective sample count, for a scaled-inverse-χ²(dof, value) draw.
    pub dof: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdaptiveConfig {
    /// Starting value.
    pub prior: f64,
    /// Pseudo-count behind the prior (an inverse-gamma prior's strength).
    pub prior_count: f64,
    /// Per-sample forgetting factor λ in (0, 1]; the count saturates at 1/(1 − λ).
    pub forgetting: f64,
    pub min: f64,
    pub max: f64,
}

impl AdaptiveConfig {
    /// A variance that never moves: clamped to `[value, value]`.
    pub fn fixed(value: f64) -> Self {
        Self {
            prior: value,
            prior_count: 1.0,
            forgetting: 1.0,
            min: value,
            max: value,
        }
    }

    pub fn validate(&self) -> Result<(), EngineError> {
        let ok = self.min.is_finite()
            && self.min > 0.0
            && self.max.is_finite()
            && self.max >= self.min
            && (self.min..=self.max).contains(&self.prior)
            && self.prior_count.is_finite()
            && self.prior_count > 0.0
            && self.forgetting > 0.0
            && self.forgetting <= 1.0;
        if ok {
            Ok(())
        } else {
            Err(EngineError::InvalidConfig(
                "adaptive variance: need 0 < min <= prior <= max, prior_count > 0, 0 < λ <= 1",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarianceEstimator {
    cfg: AdaptiveConfig,
    value: f64,
    count: f64,
}

impl VarianceEstimator {
    pub fn new(cfg: AdaptiveConfig) -> Result<Self, EngineError> {
        cfg.validate()?;
        Ok(Self {
            cfg,
            value: cfg.prior,
            count: cfg.prior_count,
        })
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn estimate(&self) -> VarianceEstimate {
        VarianceEstimate {
            value: self.value,
            dof: self.count,
        }
    }

    /// Fold in one single-sample estimate with weight `w` (clamped to [0, 1]).
    /// Samples may be negative; only the running value is clamped.
    pub fn update(&mut self, sample: f64, weight: f64) -> Result<(), EngineError> {
        if !sample.is_finite() || weight.is_nan() {
            return Err(EngineError::InvalidInput("variance sample is not finite"));
        }
        let w = weight.clamp(0.0, 1.0);
        if w == 0.0 {
            return Ok(());
        }
        self.count = self.cfg.forgetting * self.count + w;
        let v = self.value + w * (sample - self.value) / self.count;
        self.value = v.clamp(self.cfg.min, self.cfg.max);
        Ok(())
    }
}

/// Divisor for the sample's own evidence in the weight's plug-in variance.
/// Measured on model-matched data (tests/filter_synthetic.rs, 50 seeds): 4
/// biased R̂_inc +18%, 10 biased it +5%, no escape hatch (+1%) locked out a
/// too-small prior.
pub const WEIGHT_SAMPLE_DIVISOR: f64 = 10.0;

/// The information weight `(v/S)²` of a covariance-matching sample, with
/// `v = max(current, sample / 10)`. `current` and `sample` are on the scale of
/// the variance's contribution to `S`.
pub fn sample_weight(current: f64, sample: f64, s: f64) -> f64 {
    let v = current.max(sample / WEIGHT_SAMPLE_DIVISOR);
    if s > 0.0 {
        (v / s).powi(2).min(1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;
    use rand_distr::StandardNormal;

    fn cfg() -> AdaptiveConfig {
        AdaptiveConfig {
            prior: 10.0,
            prior_count: 2.0,
            forgetting: 0.5,
            min: 1.0,
            max: 100.0,
        }
    }

    #[test]
    fn update_by_hand() {
        let mut e = VarianceEstimator::new(cfg()).unwrap();
        // n = 0.5·2 + 1 = 2; v = 10 + (16 − 10)/2 = 13.
        e.update(16.0, 1.0).unwrap();
        assert_eq!(
            e.estimate(),
            VarianceEstimate {
                value: 13.0,
                dof: 2.0
            }
        );
        // Half weight: n = 0.5·2 + 0.5 = 1.5; v = 13 + 0.5·(4 − 13)/1.5 = 10.
        e.update(4.0, 0.5).unwrap();
        assert_eq!(
            e.estimate(),
            VarianceEstimate {
                value: 10.0,
                dof: 1.5
            }
        );
        // Weight 0 is a no-op; weights above 1 count as 1.
        e.update(1e6, 0.0).unwrap();
        assert_eq!(e.value(), 10.0);
        e.update(10.0, 7.0).unwrap();
        assert_eq!(e.estimate().dof, 0.5 * 1.5 + 1.0);
    }

    #[test]
    fn clamps_and_negative_samples() {
        let mut e = VarianceEstimator::new(cfg()).unwrap();
        e.update(-1000.0, 1.0).unwrap();
        assert_eq!(e.value(), 1.0);
        e.update(1e9, 1.0).unwrap();
        assert_eq!(e.value(), 100.0);
    }

    #[test]
    fn count_saturates_at_one_over_one_minus_lambda() {
        let mut e = VarianceEstimator::new(AdaptiveConfig {
            forgetting: 0.97,
            ..cfg()
        })
        .unwrap();
        for _ in 0..2000 {
            e.update(10.0, 1.0).unwrap();
        }
        assert!((e.estimate().dof - 1.0 / 0.03).abs() < 1e-6);
    }

    #[test]
    fn fixed_never_moves() {
        let mut e = VarianceEstimator::new(AdaptiveConfig::fixed(42.0)).unwrap();
        for s in [-5.0, 0.0, 1e9] {
            e.update(s, 1.0).unwrap();
        }
        assert_eq!(e.value(), 42.0);
    }

    #[test]
    fn converges_on_iid_squared_normals() {
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let mut e = VarianceEstimator::new(AdaptiveConfig {
            prior: 1.0,
            prior_count: 1.0,
            forgetting: 1.0,
            min: 1e-6,
            max: 1e6,
        })
        .unwrap();
        for _ in 0..20_000 {
            let z: f64 = rng.sample(StandardNormal);
            e.update((5.0 * z).powi(2), 1.0).unwrap();
        }
        assert!((e.value() - 25.0).abs() < 0.75, "{}", e.value());
    }

    #[test]
    fn weights() {
        assert_eq!(sample_weight(1.0, 0.0, 1.0), 1.0);
        assert_eq!(sample_weight(1.0, 0.0, 4.0), 1.0 / 16.0);
        assert_eq!(sample_weight(2.0, 0.0, 1.0), 1.0); // capped
        assert_eq!(sample_weight(1.0, 0.0, 0.0), 0.0);
        // A negative sample never raises the weight.
        assert_eq!(sample_weight(1.0, -1e9, 4.0), 1.0 / 16.0);
        // Below 10× current the sample is ignored; above, it is the plug-in.
        assert_eq!(sample_weight(1.0, 8.0, 4.0), 1.0 / 16.0);
        assert_eq!(sample_weight(1.0, 20.0, 4.0), 0.25);
    }

    #[test]
    fn invalid() {
        for bad in [
            AdaptiveConfig { min: 0.0, ..cfg() },
            AdaptiveConfig { max: 0.5, ..cfg() },
            AdaptiveConfig {
                prior: 200.0,
                ..cfg()
            },
            AdaptiveConfig {
                prior_count: 0.0,
                ..cfg()
            },
            AdaptiveConfig {
                forgetting: 1.5,
                ..cfg()
            },
        ] {
            assert!(VarianceEstimator::new(bad).is_err(), "{bad:?}");
        }
        let mut e = VarianceEstimator::new(cfg()).unwrap();
        assert!(e.update(f64::NAN, 1.0).is_err());
        assert!(e.update(1.0, f64::NAN).is_err());
    }
}
