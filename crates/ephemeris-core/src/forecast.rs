//! Monte Carlo forecast cone (design §4, "Monte Carlo design").
//!
//! Each path starts from `x₀ ~ N(x̂, P)` and draws its own `σ_s²` and `R_inc`
//! from scaled-inverse-χ²(dof, value), so a thin history (small dof) spreads
//! the cone. Each simulated day mirrors the filter's model:
//!
//! ```text
//! b += −c + N(0, σ_s²) + lump + income + bills
//! c += N(0, q_c),   m += N(0, q_m)
//! ```
//!
//! * **Lumps** put back the tail the tracker's gate trimmed: on each day a lump
//!   occurs with the smoothed empirical rate and its size is resampled from the
//!   lumps the tracker absorbed, mixed with a prior (see [`LumpPrior`]).
//! * **Income** lands on the detected schedule's nominal dates plus a sampled
//!   offset, moved to the previous business day ([`PaySchedule::pay_date`]),
//!   with amount `m + N(0, R_inc)`.
//! * **Bills** recur at their period plus a sampled persistent drift.
//!
//! **Memory and determinism.** The simulation runs day-major over `N` small
//! path states (O(N) memory, no per-day allocation). Randomness is
//! counter-based: path `i` on step `h` uses its own ChaCha8 segment
//! (`stream = i`, `word_pos = h·2²⁴`), so a path's trajectory does not depend
//! on `N`, on loop order, or on how many draws other steps consumed.

use nalgebra::{Cholesky, Matrix3, Vector3};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::{ChiSquared, Distribution, Exp, StandardNormal};

use crate::adaptive::VarianceEstimate;
use crate::calendar::{PaySchedule, business_day_on_or_after};
use crate::error::EngineError;
use crate::linalg::is_psd;
use crate::model::{IDX_B, IDX_C, IDX_M};
use crate::schedule::{BillStream, OffsetDist, Schedule};
use crate::tracker::{StateEstimate, TrackerConfig};
use crate::types::Day;

/// Words reserved per (path, step) segment of the ChaCha stream.
const STEP_WORDS_LOG2: u32 = 24;
/// A path's variance draw is capped at this multiple of the estimate, so a
/// scaled-inverse-χ² with very few dof cannot produce an absurd path.
const MAX_VARIANCE_MULTIPLE: f64 = 100.0;
const MAX_HORIZON: u32 = 3660;
const MAX_PATHS: u32 = 1_000_000;

/// Process-noise parameters the forecast shares with the tracker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForecastModel {
    pub q_c: f64,
    pub q_m: f64,
}

impl ForecastModel {
    pub fn from_tracker(cfg: &TrackerConfig) -> Self {
        Self {
            q_c: cfg.q_c,
            q_m: cfg.q_m,
        }
    }
}

/// How lumps are put back.
///
/// The rate is `(n + rate·rate_days) / (days + rate_days)` for `n` lumps seen
/// over `days` settled days. A lump's size is one of the observed lumps with
/// probability `n / (n + amount_count)`, otherwise a draw from the prior: the
/// excess of a Gaussian day beyond the gate, `±σ_s·Exp(gate_sigmas)`
/// (for a standard normal, `X − t | X > t` is close to exponential with
/// mean `1/t`). With no history the forecast therefore gets back exactly the
/// Gaussian tail the gate removed; with history the observed lumps dominate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LumpPrior {
    /// The Gaussian model's gate rate, P(χ²₁ > 10.83) = 0.001.
    pub rate: f64,
    pub rate_days: f64,
    pub amount_count: f64,
    /// The gate in σ units, √10.83 ≈ 3.29.
    pub gate_sigmas: f64,
}

impl Default for LumpPrior {
    fn default() -> Self {
        Self {
            rate: 0.001,
            rate_days: 1000.0,
            amount_count: 5.0,
            gate_sigmas: 3.29,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForecastConfig {
    pub horizon_days: u32,
    pub n_paths: u32,
    pub seed: u64,
    /// Quantile levels reported per day, each in (0, 1).
    pub levels: Vec<f64>,
    /// Balance thresholds X for `P(b ≥ X)` queries.
    pub targets: Vec<f64>,
    pub lumps: LumpPrior,
}

impl ForecastConfig {
    /// 2000 paths, 5/25/50/75/95% bands, no targets.
    pub fn new(horizon_days: u32, seed: u64) -> Self {
        Self {
            horizon_days,
            n_paths: 2000,
            seed,
            levels: vec![0.05, 0.25, 0.5, 0.75, 0.95],
            targets: Vec::new(),
            lumps: LumpPrior::default(),
        }
    }

    pub fn validate(&self) -> Result<(), EngineError> {
        use EngineError::InvalidConfig as E;
        if !(1..=MAX_HORIZON).contains(&self.horizon_days) {
            return Err(E("forecast horizon must be 1..=3660 days"));
        }
        if !(1..=MAX_PATHS).contains(&self.n_paths) {
            return Err(E("forecast n_paths must be 1..=1_000_000"));
        }
        if self.levels.is_empty() || !self.levels.iter().all(|&q| q > 0.0 && q < 1.0) {
            return Err(E("quantile levels must be in (0, 1)"));
        }
        if !self.targets.iter().all(|t| t.is_finite()) {
            return Err(E("targets must be finite"));
        }
        let l = self.lumps;
        if !((0.0..=1.0).contains(&l.rate)
            && l.rate_days.is_finite()
            && l.rate_days >= 0.0
            && l.amount_count.is_finite()
            && l.amount_count >= 0.0
            && l.gate_sigmas.is_finite()
            && l.gate_sigmas > 0.0)
        {
            return Err(E("lump prior out of range"));
        }
        Ok(())
    }
}

/// `P(b ≥ amount)` per forecast day.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetCurve {
    pub amount: f64,
    on_day: Vec<f64>,
    reached_by: Vec<f64>,
}

/// Per-day quantile bands over the horizon, plus target probabilities.
#[derive(Debug, Clone, PartialEq)]
pub struct Cone {
    start_day: Day,
    horizon: u32,
    levels: Vec<f64>,
    /// `horizon × levels`, row-major by day.
    quantiles: Vec<f64>,
    targets: Vec<TargetCurve>,
}

impl Cone {
    /// The estimate's day; forecast days are `start_day + 1 ..= start_day + horizon`.
    pub fn start_day(&self) -> Day {
        self.start_day
    }

    pub fn horizon(&self) -> u32 {
        self.horizon
    }

    pub fn levels(&self) -> &[f64] {
        &self.levels
    }

    fn index(&self, day: Day) -> Option<usize> {
        let h = day.checked_sub(self.start_day)?;
        (1..=self.horizon as Day)
            .contains(&h)
            .then(|| (h - 1) as usize)
    }

    /// Quantiles at `day`, in the order of [`Cone::levels`].
    pub fn band(&self, day: Day) -> Option<&[f64]> {
        let i = self.index(day)?;
        let q = self.levels.len();
        Some(&self.quantiles[i * q..(i + 1) * q])
    }

    pub fn targets(&self) -> &[TargetCurve] {
        &self.targets
    }

    /// `P(b_day ≥ X)` for target `t`.
    pub fn prob_at_least_on(&self, t: usize, day: Day) -> Option<f64> {
        Some(self.targets.get(t)?.on_day[self.index(day)?])
    }

    /// `P(b ≥ X on some day from now through `day`)` for target `t`.
    pub fn prob_reached_by(&self, t: usize, day: Day) -> Option<f64> {
        Some(self.targets.get(t)?.reached_by[self.index(day)?])
    }
}

/// Run the forecast from a tracker estimate.
pub fn forecast(
    est: &StateEstimate,
    schedule: &Schedule,
    model: &ForecastModel,
    cfg: &ForecastConfig,
) -> Result<Cone, EngineError> {
    let sim = Simulator::new(est, schedule, model, cfg)?;
    let n = cfg.n_paths as usize;
    let h_max = cfg.horizon_days as usize;
    let q = cfg.levels.len();
    let t = cfg.targets.len();

    let mut quantiles = Vec::with_capacity(h_max * q);
    let mut on_day = vec![Vec::with_capacity(h_max); t];
    let mut reached_by = vec![Vec::with_capacity(h_max); t];
    let mut reached = vec![false; n * t];
    let mut reached_count = vec![0u32; t];
    let mut sorted = vec![0.0; n];

    sim.run(n, |h, balances| {
        for (k, &x) in cfg.targets.iter().enumerate() {
            let mut at_least = 0u32;
            for (i, &b) in balances.iter().enumerate() {
                if b >= x {
                    at_least += 1;
                    if !reached[i * t + k] {
                        reached[i * t + k] = true;
                        reached_count[k] += 1;
                    }
                }
            }
            if h > 0 {
                on_day[k].push(f64::from(at_least) / n as f64);
                reached_by[k].push(f64::from(reached_count[k]) / n as f64);
            }
        }
        if h > 0 {
            sorted.copy_from_slice(balances);
            sorted.sort_unstable_by(f64::total_cmp);
            quantiles.extend(cfg.levels.iter().map(|&l| quantile_sorted(&sorted, l)));
        }
    })?;

    Ok(Cone {
        start_day: est.day,
        horizon: cfg.horizon_days,
        levels: cfg.levels.clone(),
        quantiles,
        targets: cfg
            .targets
            .iter()
            .zip(on_day.into_iter().zip(reached_by))
            .map(|(&amount, (on_day, reached_by))| TargetCurve {
                amount,
                on_day,
                reached_by,
            })
            .collect(),
    })
}

/// Linear-interpolation quantile (Hyndman–Fan type 7) of a sorted slice.
fn quantile_sorted(sorted: &[f64], level: f64) -> f64 {
    let pos = level * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (pos - lo as f64) * (sorted[hi] - sorted[lo])
}

#[derive(Debug, Clone, Copy)]
struct PathState {
    b: f64,
    c: f64,
    m: f64,
    spend_sd: f64,
    income_sd: f64,
    next_pay: usize,
    pay_day: Day,
}

struct Simulator<'a> {
    base: ChaCha8Rng,
    t0: Day,
    horizon: u32,
    x: Vector3<f64>,
    chol: Matrix3<f64>,
    spend: VarianceEstimate,
    income: VarianceEstimate,
    q_c_sd: f64,
    q_m_sd: f64,
    lump_rate: f64,
    lump_empirical_weight: f64,
    lumps: &'a [f64],
    lump_exp: Exp<f64>,
    paydays: Vec<Day>,
    pay_offsets: Option<&'a OffsetDist>,
    bills: &'a [BillStream],
}

impl<'a> Simulator<'a> {
    fn new(
        est: &'a StateEstimate,
        schedule: &'a Schedule,
        model: &ForecastModel,
        cfg: &ForecastConfig,
    ) -> Result<Self, EngineError> {
        cfg.validate()?;
        validate_estimate(est)?;
        let qs_ok = |v: f64| v.is_finite() && v >= 0.0;
        if !(qs_ok(model.q_c) && qs_ok(model.q_m)) {
            return Err(EngineError::InvalidConfig(
                "q_c and q_m must be non-negative",
            ));
        }
        let lp = cfg.lumps;
        let n_lumps = est.lumps.amounts.len() as f64;
        let lump_rate = ((n_lumps + lp.rate * lp.rate_days)
            / (f64::from(est.lumps.days) + lp.rate_days))
            .clamp(0.0, 1.0);
        let lump_rate = if lump_rate.is_finite() {
            lump_rate
        } else {
            0.0
        };
        let lump_empirical_weight = if n_lumps > 0.0 {
            n_lumps / (n_lumps + lp.amount_count)
        } else {
            0.0
        };

        let t0 = est.day;
        let horizon_end = t0 + cfg.horizon_days as Day;
        let (paydays, pay_offsets) = match &schedule.income {
            Some(timing) => (
                timing
                    .schedule
                    .nominal_dates(timing.last_paid_nominal + 1, horizon_end + 8),
                Some(&timing.offsets),
            ),
            None => (Vec::new(), None),
        };

        Ok(Self {
            base: ChaCha8Rng::seed_from_u64(cfg.seed),
            t0,
            horizon: cfg.horizon_days,
            x: est.x,
            chol: chol_with_jitter(&est.p)?,
            spend: est.spend_var,
            income: est.income_var,
            q_c_sd: model.q_c.sqrt(),
            q_m_sd: model.q_m.sqrt(),
            lump_rate,
            lump_empirical_weight,
            lumps: &est.lumps.amounts,
            lump_exp: Exp::new(lp.gate_sigmas)
                .map_err(|_| EngineError::InvalidConfig("lump gate_sigmas rejected"))?,
            paydays,
            pay_offsets,
            bills: &schedule.bills,
        })
    }

    /// The independent random stream for `path` at `step` (0 = initial draws).
    fn rng(&self, path: usize, step: u32) -> ChaCha8Rng {
        let mut rng = self.base.clone();
        rng.set_stream(path as u64);
        rng.set_word_pos(u128::from(step) << STEP_WORDS_LOG2);
        rng
    }

    /// Simulate `n` paths; `on_day(h, balances)` sees every path's balance at
    /// step `h` (`h = 0` is the initial draw).
    fn run(&self, n: usize, mut on_day: impl FnMut(u32, &[f64])) -> Result<(), EngineError> {
        let nb = self.bills.len();
        let mut paths = Vec::with_capacity(n);
        let mut bill_next = vec![0 as Day; n * nb];
        for i in 0..n {
            paths.push(self.init_path(i, &mut bill_next[i * nb..(i + 1) * nb]));
        }
        let mut balances: Vec<f64> = paths.iter().map(|p| p.b).collect();
        on_day(0, &balances);
        for h in 1..=self.horizon {
            let day = self.t0 + h as Day;
            for (i, p) in paths.iter_mut().enumerate() {
                self.step(p, i, h, day, &mut bill_next[i * nb..(i + 1) * nb]);
                balances[i] = p.b;
            }
            if balances.iter().any(|b| !b.is_finite()) {
                return Err(EngineError::InvalidInput(
                    "forecast produced a non-finite balance",
                ));
            }
            on_day(h, &balances);
        }
        Ok(())
    }

    fn init_path(&self, i: usize, bill_next: &mut [Day]) -> PathState {
        let mut rng = self.rng(i, 0);
        let z = Vector3::new(normal(&mut rng), normal(&mut rng), normal(&mut rng));
        let x0 = self.x + self.chol * z;
        let spend_sd = draw_variance(self.spend, &mut rng).sqrt();
        let income_sd = draw_variance(self.income, &mut rng).sqrt();
        for (next, bill) in bill_next.iter_mut().zip(self.bills) {
            let first = bill.last_day + bill.period_days as Day + bill.drift.sample(&mut rng);
            *next = first.max(self.t0 + 1);
        }
        let pay_day = self.sample_pay_day(0, self.t0 + 1, &mut rng);
        PathState {
            b: x0[IDX_B],
            c: x0[IDX_C],
            m: x0[IDX_M],
            spend_sd,
            income_sd,
            next_pay: 0,
            pay_day,
        }
    }

    /// Actual day of the `idx`-th upcoming paycheck, no earlier than `after`.
    /// A paycheck whose sampled day has already passed unobserved arrives on
    /// the next business day instead.
    fn sample_pay_day(&self, idx: usize, after: Day, rng: &mut ChaCha8Rng) -> Day {
        match (self.paydays.get(idx), self.pay_offsets) {
            (Some(&nominal), Some(offsets)) => {
                let d = PaySchedule::pay_date(nominal + offsets.sample(rng));
                if d < after {
                    business_day_on_or_after(after)
                } else {
                    d
                }
            }
            _ => Day::MAX,
        }
    }

    fn step(&self, p: &mut PathState, i: usize, h: u32, day: Day, bill_next: &mut [Day]) {
        let mut rng = self.rng(i, h);
        let mut flow = -p.c + p.spend_sd * normal(&mut rng);
        p.c += self.q_c_sd * normal(&mut rng);
        p.m += self.q_m_sd * normal(&mut rng);

        if rng.random_bool(self.lump_rate) {
            flow += if rng.random_bool(self.lump_empirical_weight) {
                self.lumps[rng.random_range(0..self.lumps.len())]
            } else {
                let sign = if rng.random_bool(0.5) { 1.0 } else { -1.0 };
                sign * self.spend.value.sqrt() * self.lump_exp.sample(&mut rng)
            };
        }

        if day == p.pay_day {
            flow += p.m + p.income_sd * normal(&mut rng);
            p.next_pay += 1;
            p.pay_day = self.sample_pay_day(p.next_pay, day + 1, &mut rng);
        }

        for (next, bill) in bill_next.iter_mut().zip(self.bills) {
            if *next == day {
                flow += bill.amount;
                *next = (day + bill.period_days as Day + bill.drift.sample(&mut rng)).max(day + 1);
            }
        }
        p.b += flow;
    }
}

fn normal(rng: &mut ChaCha8Rng) -> f64 {
    rng.sample(StandardNormal)
}

/// One draw from scaled-inverse-χ²(dof, value): `dof·value / χ²_dof`.
fn draw_variance(est: VarianceEstimate, rng: &mut ChaCha8Rng) -> f64 {
    if est.value == 0.0 {
        return 0.0;
    }
    let dof = est.dof.max(1.0);
    let chi = ChiSquared::new(dof).map_or(dof, |d| d.sample(rng));
    (dof * est.value / chi.max(f64::MIN_POSITIVE)).min(MAX_VARIANCE_MULTIPLE * est.value)
}

fn validate_estimate(est: &StateEstimate) -> Result<(), EngineError> {
    let var_ok = |v: VarianceEstimate| v.value.is_finite() && v.value >= 0.0 && v.dof > 0.0;
    if !est.x.iter().all(|v| v.is_finite())
        || !is_psd(&est.p)
        || !var_ok(est.spend_var)
        || !var_ok(est.income_var)
        || !est.lumps.amounts.iter().all(|a| a.is_finite())
    {
        return Err(EngineError::InvalidInput("state estimate is not usable"));
    }
    Ok(())
}

/// Lower Cholesky factor of a PSD matrix, adding a small diagonal jitter if
/// it is only semidefinite.
fn chol_with_jitter(p: &Matrix3<f64>) -> Result<Matrix3<f64>, EngineError> {
    let scale = p.iter().fold(1.0_f64, |m, v| m.max(v.abs()));
    for eps in [0.0, 1e-12, 1e-9, 1e-6] {
        if let Some(c) = Cholesky::new(p + Matrix3::identity() * (eps * scale)) {
            return Ok(c.l());
        }
    }
    Err(EngineError::InvalidInput(
        "covariance has no Cholesky factor",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::day_from_civil as ymd;
    use crate::schedule::PayTiming;
    use crate::tracker::LumpHistory;
    use crate::types::StreamId;

    fn t0() -> Day {
        ymd(2024, 5, 31)
    }

    fn quiet_estimate() -> StateEstimate {
        StateEstimate {
            day: t0(),
            settled_through: t0(),
            x: Vector3::new(1000.0, 50.0, 2000.0),
            p: Matrix3::zeros(),
            spend_var: VarianceEstimate {
                value: 0.0,
                dof: 100.0,
            },
            income_var: VarianceEstimate {
                value: 0.0,
                dof: 100.0,
            },
            lumps: LumpHistory::default(),
        }
    }

    fn exact_schedule() -> Schedule {
        Schedule {
            income: Some(PayTiming {
                schedule: PaySchedule::SemiMonthly { days: [15, 31] },
                last_paid_nominal: t0(),
                offsets: OffsetDist::exact(),
                fit_cost: 0,
            }),
            bills: vec![BillStream {
                stream: StreamId(1),
                period_days: 30,
                last_day: t0() - 25,
                amount: -900.0,
                drift: OffsetDist::exact(),
            }],
        }
    }

    const STILL: ForecastModel = ForecastModel { q_c: 0.0, q_m: 0.0 };

    fn no_lumps(mut cfg: ForecastConfig) -> ForecastConfig {
        cfg.lumps.rate = 0.0;
        cfg
    }

    #[test]
    fn zero_noise_path_by_hand() {
        let cfg = no_lumps(ForecastConfig {
            n_paths: 3,
            targets: vec![1500.0],
            ..ForecastConfig::new(30, 1)
        });
        let cone = forecast(&quiet_estimate(), &exact_schedule(), &STILL, &cfg).unwrap();
        let b = |y, m, d| cone.band(ymd(y, m, d)).unwrap()[2];
        let close = |a: f64, e: f64| (a - e).abs() < 1e-3;
        // −50/day. Rent (−900) recurs 30 days after May 6 → June 5.
        assert!(close(b(2024, 6, 1), 950.0));
        assert!(close(b(2024, 6, 5), 1000.0 - 250.0 - 900.0));
        // June 15 is a Saturday: paid Friday June 14.
        assert!(close(b(2024, 6, 13), 1000.0 - 650.0 - 900.0));
        assert!(close(b(2024, 6, 14), 1000.0 - 700.0 - 900.0 + 2000.0));
        // June has 30 days and June 30 is a Sunday: paid Friday June 28.
        assert!(close(b(2024, 6, 28), 1000.0 - 1400.0 - 900.0 + 4000.0));
        // All paths equal, so every band level coincides.
        let band = cone.band(ymd(2024, 6, 28)).unwrap();
        assert!(band.iter().all(|&q| close(q, band[0])));
        // Target 1500: first reached June 28 (2700 − 1400 ≥ 1500).
        assert_eq!(cone.prob_reached_by(0, ymd(2024, 6, 27)), Some(0.0));
        assert_eq!(cone.prob_reached_by(0, ymd(2024, 6, 28)), Some(1.0));
        assert_eq!(cone.prob_at_least_on(0, ymd(2024, 6, 28)), Some(1.0));
        assert_eq!(cone.prob_at_least_on(0, ymd(2024, 6, 30)), Some(1.0));
        assert_eq!(cone.band(t0()), None);
        assert_eq!(cone.band(t0() + 31), None);
    }

    fn noisy_estimate(dof: f64) -> StateEstimate {
        StateEstimate {
            p: Matrix3::from_diagonal(&Vector3::new(100.0, 4.0, 900.0)),
            spend_var: VarianceEstimate { value: 1600.0, dof },
            income_var: VarianceEstimate { value: 2500.0, dof },
            ..quiet_estimate()
        }
    }

    fn width(cone: &Cone, day: Day) -> f64 {
        let band = cone.band(day).unwrap();
        band[band.len() - 1] - band[0]
    }

    #[test]
    fn same_seed_same_cone_different_seed_different_cone() {
        let model = ForecastModel {
            q_c: 0.25,
            q_m: 25.0,
        };
        let cfg = ForecastConfig::new(60, 9);
        let a = forecast(&noisy_estimate(50.0), &exact_schedule(), &model, &cfg).unwrap();
        let b = forecast(&noisy_estimate(50.0), &exact_schedule(), &model, &cfg).unwrap();
        assert_eq!(a, b);
        let c = forecast(
            &noisy_estimate(50.0),
            &exact_schedule(),
            &model,
            &ForecastConfig::new(60, 10),
        )
        .unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn a_path_does_not_depend_on_the_number_of_paths() {
        let est = noisy_estimate(50.0);
        let sched = exact_schedule();
        let model = ForecastModel {
            q_c: 0.25,
            q_m: 25.0,
        };
        let cfg = ForecastConfig::new(40, 3);
        let sim = Simulator::new(&est, &sched, &model, &cfg).unwrap();
        let record = |n: usize| {
            let mut out = Vec::new();
            sim.run(n, |_, b| out.push(b[..5].to_vec())).unwrap();
            out
        };
        assert_eq!(record(5), record(500));
    }

    #[test]
    fn thin_history_widens_the_cone() {
        let model = ForecastModel {
            q_c: 0.25,
            q_m: 25.0,
        };
        let cfg = ForecastConfig::new(60, 4);
        let thin = forecast(&noisy_estimate(3.0), &exact_schedule(), &model, &cfg).unwrap();
        let thick = forecast(&noisy_estimate(1000.0), &exact_schedule(), &model, &cfg).unwrap();
        let day = t0() + 60;
        assert!(
            width(&thin, day) > 1.1 * width(&thick, day),
            "{} vs {}",
            width(&thin, day),
            width(&thick, day)
        );
    }

    #[test]
    fn observed_lumps_widen_the_cone() {
        let model = ForecastModel {
            q_c: 0.25,
            q_m: 25.0,
        };
        let cfg = ForecastConfig::new(60, 5);
        let plain = noisy_estimate(200.0);
        let lumpy = StateEstimate {
            lumps: LumpHistory {
                amounts: vec![
                    -400.0, -650.0, 300.0, -900.0, -500.0, -350.0, -700.0, -450.0,
                ],
                days: 200,
            },
            ..plain.clone()
        };
        let a = forecast(&plain, &exact_schedule(), &model, &cfg).unwrap();
        let b = forecast(&lumpy, &exact_schedule(), &model, &cfg).unwrap();
        let day = t0() + 60;
        assert!(width(&b, day) > width(&a, day));
        // Mostly negative lumps also pull the median down.
        assert!(b.band(day).unwrap()[2] < a.band(day).unwrap()[2]);
    }

    #[test]
    fn bands_are_ordered_and_probabilities_monotone() {
        let model = ForecastModel {
            q_c: 0.25,
            q_m: 25.0,
        };
        let cfg = ForecastConfig {
            targets: vec![500.0, 2000.0],
            ..ForecastConfig::new(90, 6)
        };
        let cone = forecast(&noisy_estimate(30.0), &exact_schedule(), &model, &cfg).unwrap();
        let mut prev = [0.0; 2];
        for h in 1..=90 {
            let day = t0() + h;
            let band = cone.band(day).unwrap();
            assert!(band.windows(2).all(|w| w[0] <= w[1]));
            for (t, p) in prev.iter_mut().enumerate() {
                let r = cone.prob_reached_by(t, day).unwrap();
                assert!(r >= *p && (0.0..=1.0).contains(&r));
                assert!(cone.prob_at_least_on(t, day).unwrap() <= r);
                *p = r;
            }
        }
    }

    #[test]
    fn semidefinite_covariance_is_accepted() {
        // Perfectly correlated b and c: rank-deficient but PSD.
        let est = StateEstimate {
            p: Matrix3::new(4.0, 2.0, 0.0, 2.0, 1.0, 0.0, 0.0, 0.0, 0.0),
            ..noisy_estimate(50.0)
        };
        assert!(forecast(&est, &exact_schedule(), &STILL, &ForecastConfig::new(5, 1)).is_ok());
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        let est = noisy_estimate(50.0);
        let sched = exact_schedule();
        let bad = [
            ForecastConfig {
                horizon_days: 0,
                ..ForecastConfig::new(1, 0)
            },
            ForecastConfig {
                n_paths: 0,
                ..ForecastConfig::new(1, 0)
            },
            ForecastConfig {
                levels: vec![0.0],
                ..ForecastConfig::new(1, 0)
            },
            ForecastConfig {
                targets: vec![f64::NAN],
                ..ForecastConfig::new(1, 0)
            },
        ];
        for cfg in &bad {
            assert!(forecast(&est, &sched, &STILL, cfg).is_err(), "{cfg:?}");
        }
        let neg = StateEstimate {
            p: -Matrix3::identity(),
            ..est.clone()
        };
        assert!(forecast(&neg, &sched, &STILL, &ForecastConfig::new(1, 0)).is_err());
        let q = ForecastModel {
            q_c: -1.0,
            q_m: 0.0,
        };
        assert!(forecast(&est, &sched, &q, &ForecastConfig::new(1, 0)).is_err());
    }

    #[test]
    fn variance_draws_follow_scaled_inverse_chi_squared() {
        // Mean of scaled-inv-χ²(ν, s²) is ν·s²/(ν − 2).
        let mut rng = ChaCha8Rng::seed_from_u64(8);
        let est = VarianceEstimate {
            value: 100.0,
            dof: 10.0,
        };
        let mean = (0..100_000)
            .map(|_| draw_variance(est, &mut rng))
            .sum::<f64>()
            / 100_000.0;
        assert!((mean - 125.0).abs() < 2.0, "{mean}");
    }

    #[test]
    fn quantile_type_seven() {
        let s = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(quantile_sorted(&s, 0.5), 2.5);
        assert_eq!(quantile_sorted(&s, 0.25), 1.75);
        assert_eq!(quantile_sorted(&[7.0], 0.9), 7.0);
    }
}
