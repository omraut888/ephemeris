//! The 3-state finance model of design §1 on a daily [`Kf`].
//!
//! `x = [b, c, m]`: balance, discretionary spend rate, expected paycheck.
//!
//! ```text
//! A = | 1 -1  0 |   B = | 1 |   Q = diag(σ_s², q_c, q_m)
//!     | 0  1  0 |       | 0 |
//!     | 0  0  1 |       | 0 |
//! H_balance = [1 0 0],  H_income = [0 0 1]
//! ```
//!
//! Paychecks and bills enter `b` through `B·u` at their actual amounts; a
//! paycheck's amount is also an observation of `m` (design §1, "Paychecks are a
//! known input, not an innovation").

use nalgebra::{Matrix3, Vector3};

use crate::error::EngineError;
use crate::kalman::{Innovation, Kf};
use crate::linalg::is_psd;
use crate::types::{Channel, Day, DayInput};

/// State index of the balance `b`.
pub const IDX_B: usize = 0;
/// State index of the spend rate `c`.
pub const IDX_C: usize = 1;
/// State index of the expected paycheck `m`.
pub const IDX_M: usize = 2;

/// Process-noise parameters. Measurement variances travel with each
/// [`crate::Observation`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelParams {
    /// `σ_s²`: day-to-day variance of discretionary spending.
    pub spend_var: f64,
    /// Daily random-walk variance of `c`.
    pub q_c: f64,
    /// Daily random-walk variance of `m`.
    pub q_m: f64,
}

impl ModelParams {
    pub fn validate(&self) -> Result<(), EngineError> {
        let ok = |v: f64| v.is_finite() && v >= 0.0;
        if ok(self.spend_var) && ok(self.q_c) && ok(self.q_m) {
            Ok(())
        } else {
            Err(EngineError::InvalidConfig(
                "process noise variances must be non-negative and finite",
            ))
        }
    }
}

pub fn transition() -> Matrix3<f64> {
    Matrix3::new(1.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0)
}

pub fn control() -> Vector3<f64> {
    Vector3::new(1.0, 0.0, 0.0)
}

pub fn observation_row(channel: Channel) -> Vector3<f64> {
    match channel {
        Channel::Balance => Vector3::new(1.0, 0.0, 0.0),
        Channel::Income => Vector3::new(0.0, 0.0, 1.0),
    }
}

/// `Q` for one day.
pub fn process_noise(params: &ModelParams) -> Matrix3<f64> {
    Matrix3::from_diagonal(&Vector3::new(params.spend_var, params.q_c, params.q_m))
}

/// What one day's step did.
#[derive(Debug, Clone, PartialEq)]
pub struct StepReport {
    pub day: Day,
    /// Days run predict-only before `day` because no input covered them.
    pub gap_days: u32,
    pub innovations: Vec<(Channel, Innovation)>,
}

/// The daily filter: a [`Kf<3>`] that knows which day its estimate is for.
#[derive(Debug, Clone, PartialEq)]
pub struct FinanceFilter {
    params: ModelParams,
    kf: Kf<3>,
    day: Day,
}

impl FinanceFilter {
    /// A filter whose prior `(x0, p0)` is the state at the end of `day`.
    pub fn new(
        params: ModelParams,
        day: Day,
        x0: Vector3<f64>,
        p0: Matrix3<f64>,
    ) -> Result<Self, EngineError> {
        params.validate()?;
        Ok(Self {
            params,
            kf: Kf::new(x0, p0)?,
            day,
        })
    }

    /// The day the current estimate describes (end of day).
    pub fn day(&self) -> Day {
        self.day
    }

    pub fn state(&self) -> &Vector3<f64> {
        &self.kf.x
    }

    pub fn covariance(&self) -> &Matrix3<f64> {
        &self.kf.p
    }

    pub fn params(&self) -> &ModelParams {
        &self.params
    }

    /// Replace `σ_s²`, as the adaptive estimator refines it (design §3).
    pub fn set_spend_var(&mut self, spend_var: f64) -> Result<(), EngineError> {
        let params = ModelParams {
            spend_var,
            ..self.params
        };
        params.validate()?;
        self.params = params;
        Ok(())
    }

    /// Advance one day with no data: predict with `u = 0`, no update.
    pub fn predict_only(&mut self) -> Result<(), EngineError> {
        self.predict(0.0)
    }

    /// Advance one day with control input `u`, no update.
    pub fn predict(&mut self, u: f64) -> Result<(), EngineError> {
        self.kf
            .predict(&transition(), &control(), u, &process_noise(&self.params))?;
        self.day += 1;
        Ok(())
    }

    /// The innovation a measurement would see, without applying it.
    pub fn innovation(&self, channel: Channel, r: f64, z: f64) -> Result<Innovation, EngineError> {
        self.kf.innovation(&observation_row(channel), r, z)
    }

    /// Apply one scalar measurement for the current day.
    pub fn update(&mut self, channel: Channel, r: f64, z: f64) -> Result<Innovation, EngineError> {
        self.kf.update_scalar(&observation_row(channel), r, z)
    }

    /// Add an unexplained lump flow to today's balance, as if it had been part
    /// of `u` (design §1, "Guard"). Covariance is unchanged.
    pub fn shift_balance(&mut self, amount: f64) -> Result<(), EngineError> {
        if !amount.is_finite() {
            return Err(EngineError::InvalidInput("lump flow is not finite"));
        }
        self.kf.x[IDX_B] += amount;
        Ok(())
    }

    /// Advance to `input.day`: predict-only through any skipped days, then
    /// predict with `input.u` and apply the observations in order. An input
    /// with no observations is a predict-only day with a control input.
    ///
    /// The input is validated before anything changes.
    pub fn step(&mut self, input: &DayInput) -> Result<StepReport, EngineError> {
        if input.day <= self.day {
            return Err(EngineError::InvalidInput(
                "day input is not after the filter's day",
            ));
        }
        if !input.u.is_finite() {
            return Err(EngineError::InvalidInput("control input u is not finite"));
        }
        for obs in &input.observations {
            obs.validate()?;
        }

        let gap_days = (input.day - self.day - 1) as u32;
        for _ in 0..gap_days {
            self.predict_only()?;
        }
        self.predict(input.u)?;

        let mut innovations = Vec::with_capacity(input.observations.len());
        for obs in &input.observations {
            let inn = self.update(obs.channel, obs.r, obs.z)?;
            innovations.push((obs.channel, inn));
        }
        debug_assert!(is_psd(&self.kf.p));
        Ok(StepReport {
            day: input.day,
            gap_days,
            innovations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::is_psd;
    use crate::types::Observation;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    const PARAMS: ModelParams = ModelParams {
        spend_var: 1600.0,
        q_c: 0.25,
        q_m: 25.0,
    };

    fn balance(z: f64, r: f64) -> Observation {
        Observation {
            channel: Channel::Balance,
            z,
            r,
        }
    }

    fn filter() -> FinanceFilter {
        let p0 = Matrix3::from_diagonal(&Vector3::new(1.0, 100.0, 10_000.0));
        FinanceFilter::new(PARAMS, 0, Vector3::new(1000.0, 50.0, 2000.0), p0).unwrap()
    }

    #[test]
    fn paycheck_is_control_input_not_balance_innovation() {
        let mut f = filter();
        // Paycheck of 2100 posts. Predicted b = 1000 − 50 + 2100 = 3050, and the
        // bank says 3050: zero balance innovation even though the balance jumped.
        let report = f
            .step(&DayInput {
                day: 1,
                u: 2100.0,
                observations: vec![
                    balance(3050.0, 1.0),
                    Observation {
                        channel: Channel::Income,
                        z: 2100.0,
                        r: 2500.0,
                    },
                ],
            })
            .unwrap();
        let (ch, bal) = report.innovations[0];
        assert_eq!((ch, bal.nu), (Channel::Balance, 0.0));
        // The paycheck teaches m instead: ν = 2100 − 2000, S = 10000 + 25 + 2500.
        let (ch, inc) = report.innovations[1];
        assert_eq!(ch, Channel::Income);
        assert_eq!(inc.nu, 100.0);
        assert!((inc.s - 12_525.0).abs() < 1e-9);
        let k_m = 10_025.0 / 12_525.0;
        assert!((f.state()[IDX_M] - (2000.0 + 100.0 * k_m)).abs() < 1e-9);
        // With no b–m correlation, the income update leaves b alone.
        assert!((f.state()[IDX_B] - 3050.0).abs() < 1e-9);
    }

    #[test]
    fn spend_rate_learns_from_balance_drift() {
        let mut f = filter();
        // The balance falls 80/day while the prior says 50/day. With σ_s² = 1600
        // and q_c = 0.25 the c-loop time constant is ~√(1600/0.25) = 80 days.
        let mut c_at = Vec::new();
        for day in 1..=365 {
            f.step(&DayInput {
                day,
                u: 0.0,
                observations: vec![balance(1000.0 - 80.0 * f64::from(day), 1.0)],
            })
            .unwrap();
            c_at.push(f.state()[IDX_C]);
        }
        assert!(
            c_at[59] > 65.0 && c_at[59] < 80.0,
            "day 60: c = {}",
            c_at[59]
        );
        assert!((c_at[364] - 80.0).abs() < 0.5, "day 365: c = {}", c_at[364]);
    }

    #[test]
    fn gaps_are_filled_with_predict_only_days() {
        let mut stepped = filter();
        let mut manual = filter();
        let report = stepped
            .step(&DayInput {
                day: 4,
                u: 10.0,
                observations: vec![],
            })
            .unwrap();
        assert_eq!(report.gap_days, 3);
        assert!(report.innovations.is_empty());
        for _ in 0..3 {
            manual.predict_only().unwrap();
        }
        manual
            .step(&DayInput {
                day: 4,
                u: 10.0,
                observations: vec![],
            })
            .unwrap();
        assert_eq!(stepped, manual);
        assert_eq!(stepped.day(), 4);
        // b drifted 4 days at c = 50, plus u = 10.
        assert_eq!(stepped.state()[IDX_B], 1000.0 - 200.0 + 10.0);
    }

    #[test]
    fn invalid_steps_leave_state_unchanged() {
        let mut f = filter();
        let before = f.clone();
        let bad = [
            DayInput {
                day: 0,
                u: 0.0,
                observations: vec![],
            },
            DayInput {
                day: 1,
                u: f64::NAN,
                observations: vec![],
            },
            DayInput {
                day: 1,
                u: f64::INFINITY,
                observations: vec![],
            },
            // A bad observation after a valid one: nothing may be applied, not
            // even the gap days.
            DayInput {
                day: 5,
                u: 0.0,
                observations: vec![balance(1.0, 1.0), balance(f64::NAN, 1.0)],
            },
            DayInput {
                day: 5,
                u: 0.0,
                observations: vec![balance(1.0, 0.0)],
            },
        ];
        for input in &bad {
            assert!(f.step(input).is_err(), "{input:?}");
            assert_eq!(f, before);
        }
        let neg = ModelParams {
            q_c: -1.0,
            ..PARAMS
        };
        assert!(FinanceFilter::new(neg, 0, Vector3::zeros(), Matrix3::identity()).is_err());
    }

    /// `A` has no `m` column and Q is diagonal, so from a diagonal prior the
    /// covariance never couples `m` to `b` or `c`: balance data cannot move `m`.
    #[test]
    fn m_is_decoupled_from_balance() {
        let mut rng = ChaCha8Rng::seed_from_u64(5);
        let mut f = filter();
        for day in 1..=500 {
            let m_before = f.state()[IDX_M];
            let mut observations = vec![balance(rng.random_range(-1e4..1e4), 1.0)];
            let paid = day % 14 == 0;
            if paid {
                observations.push(Observation {
                    channel: Channel::Income,
                    z: rng.random_range(1500.0..2500.0),
                    r: 2500.0,
                });
            }
            f.step(&DayInput {
                day,
                u: rng.random_range(-100.0..100.0),
                observations,
            })
            .unwrap();
            let p = f.covariance();
            assert_eq!((p[(IDX_B, IDX_M)], p[(IDX_C, IDX_M)]), (0.0, 0.0));
            if !paid {
                assert_eq!(f.state()[IDX_M], m_before);
            }
        }
    }

    /// Thousands of steps with random inputs, gaps, and measurement variances
    /// spanning twelve orders of magnitude: P must stay symmetric PSD throughout.
    #[test]
    fn covariance_stays_symmetric_psd() {
        let mut rng = ChaCha8Rng::seed_from_u64(99);
        let mut f = filter();
        let mut day = 0;
        for _ in 0..20_000 {
            day += rng.random_range(1..=3);
            let mut observations = Vec::new();
            if rng.random_bool(0.8) {
                let r = 10f64.powf(rng.random_range(-6.0..6.0));
                observations.push(balance(rng.random_range(-1e4..1e4), r));
            }
            if rng.random_bool(0.1) {
                observations.push(Observation {
                    channel: Channel::Income,
                    z: rng.random_range(0.0..5000.0),
                    r: 10f64.powf(rng.random_range(-3.0..5.0)),
                });
            }
            f.step(&DayInput {
                day,
                u: rng.random_range(-3000.0..3000.0),
                observations,
            })
            .unwrap();
            let p = f.covariance();
            assert!(is_psd(p), "day {day}: {p}");
            assert_eq!(*p, p.transpose(), "exactly symmetric after symmetrize");
            assert!((0..3).all(|i| p[(i, i)] > 0.0));
        }
    }
}
