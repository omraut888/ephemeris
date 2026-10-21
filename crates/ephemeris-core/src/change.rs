//! Regime-change detection: a two-sided CUSUM on normalized innovations
//! (design §6).
//!
//! When the model holds, `e = ν/√S` is white with unit variance. A step in
//! the state biases it, and Page's CUSUM is the standard sequential test for
//! such a bias:
//!
//! ```text
//! g⁺ ← max(0, g⁺ + e − κ)      g⁻ ← max(0, g⁻ − e − κ)      alarm when g > h
//! ```
//!
//! Each side also keeps the mean innovation over its current run (since it
//! last sat at zero), `Σν / Σscale`, which is the size of the shift the alarm
//! reports. Callers pass the gate-clipped `ν`, so one lump moves `g` by at
//! most `√gate − κ`, and `scale` is the number of days the innovation covers
//! (1 for a paycheck).

use crate::error::EngineError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChangeConfig {
    /// Allowance κ, in standard deviations of `e`.
    pub allowance: f64,
    /// Alarm threshold h, in the same units.
    pub threshold: f64,
}

impl Default for ChangeConfig {
    /// κ = 0.5, h = 5: about 0.8 false alarms a year on Gaussian daily
    /// innovations, and a 1σ shift detected in about 10 days (design §6).
    fn default() -> Self {
        Self {
            allowance: 0.5,
            threshold: 5.0,
        }
    }
}

impl ChangeConfig {
    pub fn validate(&self) -> Result<(), EngineError> {
        if self.allowance.is_finite()
            && self.allowance >= 0.0
            && self.threshold.is_finite()
            && self.threshold > 0.0
        {
            Ok(())
        } else {
            Err(EngineError::InvalidConfig(
                "change detector: need allowance >= 0 and threshold > 0",
            ))
        }
    }
}

/// A detected shift in the innovation mean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alarm {
    /// Mean innovation per unit of scale over the alarming run, signed.
    pub shift: f64,
    /// Observations in that run.
    pub run_len: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Side {
    g: f64,
    sum_nu: f64,
    sum_scale: f64,
    run_len: u32,
}

impl Side {
    /// Add `step` (the signed, normalized innovation minus κ) to the
    /// statistic; the run sums restart whenever it falls back to zero.
    fn push(&mut self, step: f64, nu: f64, scale: f64) {
        self.g += step;
        if self.g <= 0.0 {
            *self = Side::default();
        } else {
            self.sum_nu += nu;
            self.sum_scale += scale;
            self.run_len += 1;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeDetector {
    cfg: ChangeConfig,
    up: Side,
    down: Side,
}

impl ChangeDetector {
    pub fn new(cfg: ChangeConfig) -> Result<Self, EngineError> {
        cfg.validate()?;
        Ok(Self {
            cfg,
            up: Side::default(),
            down: Side::default(),
        })
    }

    /// `(g⁺, g⁻)`.
    pub fn statistics(&self) -> (f64, f64) {
        (self.up.g, self.down.g)
    }

    /// Fold in one innovation `nu` with variance `s` covering `scale` units
    /// (days, or one paycheck). On an alarm both sides reset.
    pub fn observe(&mut self, nu: f64, s: f64, scale: f64) -> Result<Option<Alarm>, EngineError> {
        if !(nu.is_finite() && s.is_finite() && s > 0.0 && scale.is_finite() && scale > 0.0) {
            return Err(EngineError::InvalidInput(
                "change detector: innovation must be finite with s, scale > 0",
            ));
        }
        let e = nu / s.sqrt();
        let k = self.cfg.allowance;
        self.up.push(e - k, nu, scale);
        self.down.push(-e - k, nu, scale);

        let h = self.cfg.threshold;
        let fired = [self.up, self.down]
            .into_iter()
            .filter(|side| side.g > h)
            .max_by(|a, b| a.g.total_cmp(&b.g));
        Ok(fired.map(|side| {
            self.up = Side::default();
            self.down = Side::default();
            Alarm {
                shift: side.sum_nu / side.sum_scale,
                run_len: side.run_len,
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;
    use rand_distr::{Distribution, StandardNormal};

    fn detector() -> ChangeDetector {
        ChangeDetector::new(ChangeConfig::default()).unwrap()
    }

    #[test]
    fn upward_run_by_hand() {
        let mut d = detector();
        // s = 4, so ν = 4 is e = 2: g⁺ = 1.5, 3, 4.5, then 6 > 5.
        for g in [1.5, 3.0, 4.5] {
            assert_eq!(d.observe(4.0, 4.0, 1.0).unwrap(), None);
            assert_eq!(d.statistics(), (g, 0.0));
        }
        let alarm = d.observe(4.0, 4.0, 1.0).unwrap().unwrap();
        assert_eq!(
            alarm,
            Alarm {
                shift: 4.0,
                run_len: 4
            }
        );
        assert_eq!(d.statistics(), (0.0, 0.0), "both sides reset");
    }

    #[test]
    fn shift_is_per_unit_of_scale_over_the_current_run() {
        let mut d = detector();
        // A downward run: e = −3 (g⁻ = 2.5), then a 2-day innovation of
        // ν = −12 with s = 4, e = −6 (g⁻ = 8 > 5). Shift = (−6 − 12)/(1 + 2).
        assert_eq!(d.observe(-6.0, 4.0, 1.0).unwrap(), None);
        assert_eq!(d.statistics(), (0.0, 2.5));
        let alarm = d.observe(-12.0, 4.0, 2.0).unwrap().unwrap();
        assert_eq!(alarm.shift, -6.0);
        assert_eq!(alarm.run_len, 2);
    }

    #[test]
    fn run_restarts_when_the_statistic_returns_to_zero() {
        let mut d = detector();
        // e = 2 → g⁺ = 1.5; e = −2 → 1.5 − 2.5 < 0, reset (that ν is not in
        // the run); then four innovations of e = 1.75 → 1.25 each: 5.0, not > 5.
        d.observe(2.0, 1.0, 1.0).unwrap();
        d.observe(-2.0, 1.0, 1.0).unwrap();
        assert_eq!(d.statistics().0, 0.0);
        for _ in 0..4 {
            assert_eq!(d.observe(1.75, 1.0, 1.0).unwrap(), None);
        }
        assert_eq!(d.statistics().0, 5.0);
        let alarm = d.observe(1.75, 1.0, 1.0).unwrap().unwrap();
        assert_eq!((alarm.shift, alarm.run_len), (1.75, 5));
    }

    /// Two clipped lumps in a row are a shift; one never is.
    #[test]
    fn one_clipped_lump_cannot_alarm() {
        let clip = 10.83_f64.sqrt();
        let mut d = detector();
        assert_eq!(d.observe(clip, 1.0, 1.0).unwrap(), None);
        assert!(d.statistics().0 < 5.0);
        assert!(d.observe(clip, 1.0, 1.0).unwrap().is_some());
    }

    /// Unit-variance white noise: the false-alarm rate is close to the
    /// Siegmund approximation (one alarm per ~470 observations, two-sided).
    #[test]
    fn false_alarm_rate_on_white_noise() {
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let mut d = detector();
        let n = 400_000;
        let mut alarms = 0;
        for _ in 0..n {
            let z: f64 = StandardNormal.sample(&mut rng);
            alarms += usize::from(d.observe(z, 1.0, 1.0).unwrap().is_some());
        }
        let per_alarm = n as f64 / alarms as f64;
        assert!((350.0..650.0).contains(&per_alarm), "{per_alarm}");
    }

    /// A 1.5σ shift is found in about 6 observations, and its size recovered.
    #[test]
    fn detection_delay_and_shift_estimate() {
        let mut rng = ChaCha8Rng::seed_from_u64(4);
        let (mut delays, mut shifts) = (Vec::new(), Vec::new());
        for _ in 0..2000 {
            let mut d = detector();
            for k in 1..=200 {
                let z: f64 = StandardNormal.sample(&mut rng);
                if let Some(a) = d.observe(1.5 + z, 1.0, 1.0).unwrap() {
                    delays.push(f64::from(k));
                    shifts.push(a.shift);
                    break;
                }
            }
        }
        let mean = |xs: &[f64]| xs.iter().sum::<f64>() / xs.len() as f64;
        assert_eq!(delays.len(), 2000);
        assert!((mean(&delays) - 5.7).abs() < 0.6, "{}", mean(&delays));
        // The run that crosses h is biased upward, but not by much.
        assert!((1.4..2.2).contains(&mean(&shifts)), "{}", mean(&shifts));
    }

    #[test]
    fn invalid() {
        for bad in [
            ChangeConfig {
                allowance: -0.1,
                ..ChangeConfig::default()
            },
            ChangeConfig {
                threshold: 0.0,
                ..ChangeConfig::default()
            },
            ChangeConfig {
                threshold: f64::INFINITY,
                ..ChangeConfig::default()
            },
        ] {
            assert!(ChangeDetector::new(bad).is_err(), "{bad:?}");
        }
        let mut d = detector();
        let before = d.clone();
        assert!(d.observe(f64::NAN, 1.0, 1.0).is_err());
        assert!(d.observe(1.0, 0.0, 1.0).is_err());
        assert!(d.observe(1.0, 1.0, 0.0).is_err());
        assert_eq!(d, before);
    }
}
