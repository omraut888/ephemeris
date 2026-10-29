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
//! * **Bills** recur on their day of month when anchored (deterministic,
//!   business-day shift included), otherwise at their period plus a sampled
//!   persistent drift.
//!
//! **Pay-cycle lows.** Besides the daily quantiles, the cone reports, for each
//! pay cycle (one nominal payday up to the next, or calendar weeks when no
//! income was detected), the quantiles across paths of *each path's own*
//! lowest balance in that cycle. That is not the minimum over the cycle of
//! the daily quantiles, which would be optimistic: a path's minimum is never
//! above its balance on any day, so every quantile of the minima is at or
//! below the lowest daily quantile in the cycle (a test checks this).
//!
//! **Spend floor** ([`ForecastConfig::spend_floor`], design §7). The filter's
//! `c` is Gaussian and can sit below zero, which would have a path earn money
//! every day. With a floor, `c₀` is drawn from its Gaussian conditioned on
//! `c₀ ≥ floor` (inverse CDF, [`TruncatedNormal`]) and the random walk
//! reflects at the floor. The draw is done with `P` factored in the order
//! `(c, b, m)`, so truncating `c`'s normal draw leaves `(b, m)` exactly
//! Gaussian given `c₀`: the b–c correlation is kept. Without a floor the
//! draw is the plain `x̂ + L·z`.
//!
//! **Memory and determinism.** The simulation runs day-major over `N` small
//! path states (O(N) memory, no per-day allocation). Randomness is
//! counter-based: path `i` on step `h` uses its own ChaCha8 segment
//! (`stream = i`, `word_pos = h·2²⁴`), so a path's trajectory does not depend
//! on `N`, on loop order, or on how many draws other steps consumed.

use nalgebra::{Cholesky, Matrix3, Vector3};
use rand::{Rng, RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::{ChiSquared, Distribution, Exp, StandardNormal};

use crate::adaptive::VarianceEstimate;
use crate::calendar::{PaySchedule, business_day_on_or_after};
use crate::error::EngineError;
use crate::linalg::is_psd;
use crate::model::{IDX_B, IDX_C, IDX_M};
use crate::schedule::{BillStream, OffsetDist, Schedule};
use crate::tracker::{StateEstimate, TrackerConfig};
use crate::truncated::{TruncatedNormal, open01};
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
    /// Lower bound on the spend rate `c` in the simulation ($/day), or `None`
    /// for the untruncated Gaussian (design §7).
    pub spend_floor: Option<f64>,
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
            spend_floor: None,
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
        if self.spend_floor.is_some_and(|f| !f.is_finite()) {
            return Err(E("spend_floor must be finite"));
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

/// `P(b ≥ amount)` per forecast day, and the two first-passage curves.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetCurve {
    pub amount: f64,
    on_day: Vec<f64>,
    reached_by: Vec<f64>,
    below_by: Vec<f64>,
}

/// How the pay cycles of [`Cone::cycle_lows`] were cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleBasis {
    /// At each detected payday (nominal date moved to a business day).
    Paycheck,
    /// Every 7 days from the first forecast day: no income was detected.
    Week,
}

/// The distribution of each path's lowest balance over one cycle.
#[derive(Debug, Clone, PartialEq)]
pub struct CycleLow {
    /// First and last day of the cycle, inclusive.
    pub start: Day,
    pub end: Day,
    /// Cut short by the forecast's start or end, so not a whole cycle.
    pub partial: bool,
    /// Quantiles of the per-path minimum, in the order of [`Cone::levels`].
    pub quantiles: Vec<f64>,
}

/// Per-day quantile bands over the horizon, plus target probabilities and
/// pay-cycle lows.
#[derive(Debug, Clone, PartialEq)]
pub struct Cone {
    start_day: Day,
    horizon: u32,
    levels: Vec<f64>,
    /// `horizon × levels`, row-major by day.
    quantiles: Vec<f64>,
    targets: Vec<TargetCurve>,
    cycle_basis: CycleBasis,
    cycle_lows: Vec<CycleLow>,
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

    /// `P(b < X on some day from now through `day`)` for target `t`: with
    /// X = 0, the chance of going overdrawn. Not `1 − prob_reached_by`: a
    /// path can be on both sides of X on different days.
    pub fn prob_below_by(&self, t: usize, day: Day) -> Option<f64> {
        Some(self.targets.get(t)?.below_by[self.index(day)?])
    }

    pub fn cycle_basis(&self) -> CycleBasis {
        self.cycle_basis
    }

    /// Consecutive cycles covering every forecast day, in order.
    pub fn cycle_lows(&self) -> &[CycleLow] {
        &self.cycle_lows
    }
}

impl TargetCurve {
    /// `P(b ≥ amount)` on each forecast day, in day order.
    pub fn on_day(&self) -> &[f64] {
        &self.on_day
    }

    /// `P(b ≥ amount on some day so far)`, in day order.
    pub fn reached_by(&self) -> &[f64] {
        &self.reached_by
    }

    /// `P(b < amount on some day so far)`, in day order.
    pub fn below_by(&self) -> &[f64] {
        &self.below_by
    }
}

/// `(start, end, partial)` of each cycle covering `t0 + 1 ..= t0 + horizon`.
fn pay_cycles(schedule: &Schedule, t0: Day, horizon: u32) -> (CycleBasis, Vec<(Day, Day, bool)>) {
    let (first, last) = (t0 + 1, t0 + horizon as Day);
    let Some(timing) = &schedule.income else {
        let cycles = (first..=last)
            .step_by(7)
            .map(|s| (s, (s + 6).min(last), s + 6 > last))
            .collect();
        return (CycleBasis::Week, cycles);
    };
    // Paydays from the first nominal date after the last paid one, far
    // enough past the horizon to know whether the last cycle is whole.
    let mut paydays: Vec<Day> = timing
        .schedule
        .nominal_dates(timing.last_paid_nominal + 1, last + 70)
        .into_iter()
        .map(PaySchedule::pay_date)
        .filter(|&d| d > first)
        .collect();
    paydays.dedup();
    let mut starts = vec![first];
    starts.extend(paydays.iter().copied().take_while(|&d| d <= last));
    let next_after = paydays.iter().copied().find(|&d| d > last);
    let cycles = starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let end = starts.get(i + 1).map_or(last, |&n| n - 1);
            // The first cycle starts mid-cycle unless the forecast starts on
            // a payday; the last ends early unless a payday follows it.
            let cut_start = i == 0 && !timing_is_payday(timing, first);
            let cut_end = end == last && next_after != Some(last + 1);
            (s, end, cut_start || cut_end)
        })
        .collect();
    (CycleBasis::Paycheck, cycles)
}

fn timing_is_payday(timing: &crate::schedule::PayTiming, day: Day) -> bool {
    timing
        .schedule
        .nominal_dates(day - 7, day + 7)
        .into_iter()
        .any(|n| PaySchedule::pay_date(n) == day)
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
    let mut below_by = vec![Vec::with_capacity(h_max); t];
    let mut below = vec![false; n * t];
    let mut below_count = vec![0u32; t];
    let mut sorted = vec![0.0; n];
    let (cycle_basis, cycles) = pay_cycles(schedule, est.day, cfg.horizon_days);
    let mut cycle = 0;
    let mut lows = vec![f64::INFINITY; n];
    let mut cycle_lows = Vec::with_capacity(cycles.len());

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
                } else if !below[i * t + k] {
                    below[i * t + k] = true;
                    below_count[k] += 1;
                }
            }
            if h > 0 {
                on_day[k].push(f64::from(at_least) / n as f64);
                reached_by[k].push(f64::from(reached_count[k]) / n as f64);
                below_by[k].push(f64::from(below_count[k]) / n as f64);
            }
        }
        if h > 0 {
            sorted.copy_from_slice(balances);
            sorted.sort_unstable_by(f64::total_cmp);
            quantiles.extend(cfg.levels.iter().map(|&l| quantile_sorted(&sorted, l)));

            let day = est.day + h as Day;
            let (start, end, partial) = cycles[cycle];
            for (low, &b) in lows.iter_mut().zip(balances) {
                *low = if day == start { b } else { low.min(b) };
            }
            if day == end {
                sorted.copy_from_slice(&lows);
                sorted.sort_unstable_by(f64::total_cmp);
                cycle_lows.push(CycleLow {
                    start,
                    end,
                    partial,
                    quantiles: cfg
                        .levels
                        .iter()
                        .map(|&l| quantile_sorted(&sorted, l))
                        .collect(),
                });
                cycle += 1;
            }
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
            .zip(on_day.into_iter().zip(reached_by).zip(below_by))
            .map(|(&amount, ((on_day, reached_by), below_by))| TargetCurve {
                amount,
                on_day,
                reached_by,
                below_by,
            })
            .collect(),
        cycle_basis,
        cycle_lows,
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
    /// With a spend floor: the floor, and the Cholesky factor of `P` in the
    /// order `(c, b, m)`.
    floor: Option<(f64, Matrix3<f64>)>,
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
            floor: match cfg.spend_floor {
                Some(f) => Some((f, chol_with_jitter(&permute(&est.p, C_FIRST))?)),
                None => None,
            },
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
        let x0 = match self.floor {
            None => {
                let z = Vector3::new(normal(&mut rng), normal(&mut rng), normal(&mut rng));
                self.x + self.chol * z
            }
            Some((floor, l)) => {
                // In (c, b, m) order, c' = ĉ + L₀₀·z₀ depends on z₀ alone, so
                // c' ≥ floor ⇔ z₀ ≥ (floor − ĉ)/L₀₀; z₁, z₂ stay standard.
                let c_hat = self.x[IDX_C];
                let z0 = if l[(0, 0)] > 0.0 {
                    TruncatedNormal {
                        mean: 0.0,
                        sd: 1.0,
                        floor: (floor - c_hat) / l[(0, 0)],
                    }
                    .quantile(open01(rng.next_u64()))
                } else {
                    0.0
                };
                let z = Vector3::new(z0, normal(&mut rng), normal(&mut rng));
                let mut x0 = permute_back(&(permute_vec(&self.x, C_FIRST) + l * z), C_FIRST);
                // Only binds when c has no variance and ĉ is below the floor.
                x0[IDX_C] = x0[IDX_C].max(floor);
                x0
            }
        };
        let spend_sd = draw_variance(self.spend, &mut rng).sqrt();
        let income_sd = draw_variance(self.income, &mut rng).sqrt();
        for (next, bill) in bill_next.iter_mut().zip(self.bills) {
            let first = match bill.day_of_month {
                Some(anchor) => anchor.next_after(bill.last_day),
                None => bill.last_day + bill.period_days as Day + bill.drift.sample(&mut rng),
            };
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
        if let Some((floor, _)) = self.floor
            && p.c < floor
        {
            p.c = 2.0 * floor - p.c;
        }
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
                *next = match bill.day_of_month {
                    Some(anchor) => anchor.next_after(day),
                    None => {
                        (day + bill.period_days as Day + bill.drift.sample(&mut rng)).max(day + 1)
                    }
                };
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
/// State indices in the order `(c, b, m)`: `c` first, for the spend floor.
const C_FIRST: [usize; 3] = [IDX_C, IDX_B, IDX_M];

/// `P` reordered so that entry `(i, j)` is `p[(order[i], order[j])]`.
fn permute(p: &Matrix3<f64>, order: [usize; 3]) -> Matrix3<f64> {
    Matrix3::from_fn(|i, j| p[(order[i], order[j])])
}

fn permute_vec(x: &Vector3<f64>, order: [usize; 3]) -> Vector3<f64> {
    Vector3::from_fn(|i, _| x[order[i]])
}

/// The inverse of [`permute_vec`].
fn permute_back(y: &Vector3<f64>, order: [usize; 3]) -> Vector3<f64> {
    let mut x = Vector3::zeros();
    for (i, &k) in order.iter().enumerate() {
        x[k] = y[i];
    }
    x
}

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
                day_of_month: None,
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
            targets: vec![1500.0, 0.0],
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
        // It starts at 1000, so it is below 1500 from the first day.
        assert_eq!(cone.prob_below_by(0, ymd(2024, 6, 1)), Some(1.0));
        // Target 0: first below it June 5 (750 − 900 = −150), and it stays
        // "has been below" after the paycheck lifts it back up on June 14.
        assert_eq!(cone.prob_below_by(1, ymd(2024, 6, 4)), Some(0.0));
        assert_eq!(cone.prob_below_by(1, ymd(2024, 6, 5)), Some(1.0));
        assert_eq!(cone.prob_at_least_on(1, ymd(2024, 6, 14)), Some(1.0));
        assert_eq!(cone.prob_below_by(1, ymd(2024, 6, 30)), Some(1.0));
        assert_eq!(cone.band(t0()), None);
        assert_eq!(cone.band(t0() + 31), None);
    }

    /// The zero-noise path of `zero_noise_path_by_hand`, cut at its paydays
    /// (Fri June 14, Fri June 28): each cycle's low is the path's minimum.
    #[test]
    fn cycle_lows_by_hand() {
        let cfg = no_lumps(ForecastConfig {
            n_paths: 3,
            ..ForecastConfig::new(30, 1)
        });
        let cone = forecast(&quiet_estimate(), &exact_schedule(), &STILL, &cfg).unwrap();
        assert_eq!(cone.cycle_basis(), CycleBasis::Paycheck);
        let lows = cone.cycle_lows();
        let spans: Vec<(Day, Day, bool)> =
            lows.iter().map(|c| (c.start, c.end, c.partial)).collect();
        assert_eq!(
            spans,
            [
                // Starts mid-cycle (May 31 was the last nominal payday).
                (ymd(2024, 6, 1), ymd(2024, 6, 13), true),
                (ymd(2024, 6, 14), ymd(2024, 6, 27), false),
                // Ends June 30 but the next payday is July 15: cut short.
                (ymd(2024, 6, 28), ymd(2024, 6, 30), true),
            ]
        );
        let close = |a: f64, e: f64| (a - e).abs() < 1e-3;
        // Cycle 1: lowest on June 13, after rent: 1000 − 650 − 900.
        assert!(lows[0].quantiles.iter().all(|&q| close(q, -550.0)));
        // Cycle 2: 1000 − 700 − 900 + 2000 = 1400 on June 14, then −50/day
        // through June 27: 1400 − 13·50.
        assert!(lows[1].quantiles.iter().all(|&q| close(q, 750.0)));
        // Cycle 3: 2700 on June 28 (zero_noise_path_by_hand), 2600 by June 30.
        assert!(lows[2].quantiles.iter().all(|&q| close(q, 2600.0)));
    }

    #[test]
    fn cycle_lows_are_weeks_without_income() {
        let sched = Schedule {
            income: None,
            bills: Vec::new(),
        };
        let cfg = no_lumps(ForecastConfig {
            n_paths: 2,
            ..ForecastConfig::new(16, 1)
        });
        let cone = forecast(&quiet_estimate(), &sched, &STILL, &cfg).unwrap();
        assert_eq!(cone.cycle_basis(), CycleBasis::Week);
        let spans: Vec<(Day, Day, bool)> = cone
            .cycle_lows()
            .iter()
            .map(|c| (c.start, c.end, c.partial))
            .collect();
        let d = t0();
        assert_eq!(
            spans,
            [
                (d + 1, d + 7, false),
                (d + 8, d + 14, false),
                (d + 15, d + 16, true)
            ]
        );
        // −50/day: each week's low is its last day.
        let close = |a: f64, e: f64| (a - e).abs() < 1e-3;
        assert!(close(
            cone.cycle_lows()[1].quantiles[0],
            1000.0 - 14.0 * 50.0
        ));
    }

    /// A path's minimum is at or below its balance on every day of the cycle,
    /// so each quantile of the minima is at or below every daily quantile.
    #[test]
    fn cycle_lows_never_exceed_daily_quantiles() {
        let cfg = ForecastConfig {
            n_paths: 500,
            ..ForecastConfig::new(90, 3)
        };
        let model = ForecastModel {
            q_c: 0.25,
            q_m: 25.0,
        };
        let cone = forecast(&noisy_estimate(10.0), &exact_schedule(), &model, &cfg).unwrap();
        let lows = cone.cycle_lows();
        assert_eq!(lows.first().unwrap().start, t0() + 1);
        assert_eq!(lows.last().unwrap().end, t0() + 90);
        for w in lows.windows(2) {
            assert_eq!(w[1].start, w[0].end + 1, "cycles tile the horizon");
        }
        for c in lows {
            for (k, &q) in c.quantiles.iter().enumerate() {
                for d in c.start..=c.end {
                    assert!(q <= cone.band(d).unwrap()[k] + 1e-9, "{c:?} day {d}");
                }
            }
        }
    }

    #[test]
    fn anchored_bill_lands_on_its_shifted_day_of_month() {
        use crate::schedule::{DayShift, MonthlyAnchor};
        // Rent on the 1st, next business day; last paid Wed May 1 2024.
        let sched = Schedule {
            income: None,
            bills: vec![BillStream {
                stream: StreamId(1),
                period_days: 30,
                last_day: ymd(2024, 5, 1),
                amount: -1000.0,
                drift: OffsetDist::exact(),
                day_of_month: Some(MonthlyAnchor {
                    day: 1,
                    shift: DayShift::NextBusinessDay,
                }),
            }],
        };
        let cfg = no_lumps(ForecastConfig {
            n_paths: 2,
            ..ForecastConfig::new(40, 1)
        });
        let cone = forecast(&quiet_estimate(), &sched, &STILL, &cfg).unwrap();
        let b = |y, m, d| cone.band(ymd(y, m, d)).unwrap()[2];
        // Sat June 1 → Mon June 3; then Mon July 1.
        let close = |a: f64, e: f64| (a - e).abs() < 1e-3;
        assert!(close(b(2024, 6, 2), 1000.0 - 50.0 * 2.0));
        assert!(close(b(2024, 6, 3), 1000.0 - 50.0 * 3.0 - 1000.0));
        assert!(close(b(2024, 6, 30), 1000.0 - 50.0 * 30.0 - 1000.0));
        assert!(close(b(2024, 7, 1), 1000.0 - 50.0 * 31.0 - 2000.0));
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
        let mut prev = [(0.0, 0.0); 2];
        for h in 1..=90 {
            let day = t0() + h;
            let band = cone.band(day).unwrap();
            assert!(band.windows(2).all(|w| w[0] <= w[1]));
            for (t, (p_reached, p_below)) in prev.iter_mut().enumerate() {
                let r = cone.prob_reached_by(t, day).unwrap();
                assert!(r >= *p_reached && (0.0..=1.0).contains(&r));
                let on = cone.prob_at_least_on(t, day).unwrap();
                assert!(on <= r);
                let d = cone.prob_below_by(t, day).unwrap();
                assert!(d >= *p_below && (0.0..=1.0).contains(&d));
                // Below X today means it has been below X by today.
                assert!(1.0 - on <= d + 1e-12);
                (*p_reached, *p_below) = (r, d);
            }
        }
    }

    fn no_schedule() -> Schedule {
        Schedule {
            income: None,
            bills: Vec::new(),
        }
    }

    /// `c ~ N(10, 20²)`, nothing else random, one day: `b₁ = 1000 − c₀`.
    fn spend_only_estimate() -> StateEstimate {
        StateEstimate {
            p: Matrix3::from_diagonal(&Vector3::new(0.0, 400.0, 0.0)),
            x: Vector3::new(1000.0, 10.0, 2000.0),
            ..quiet_estimate()
        }
    }

    fn day_one_band(floor: Option<f64>, levels: Vec<f64>) -> Vec<f64> {
        let cfg = no_lumps(ForecastConfig {
            n_paths: 20_000,
            levels,
            spend_floor: floor,
            ..ForecastConfig::new(1, 3)
        });
        let cone = forecast(&spend_only_estimate(), &no_schedule(), &STILL, &cfg).unwrap();
        cone.band(t0() + 1).unwrap().to_vec()
    }

    #[test]
    fn spend_floor_samples_the_truncated_normal() {
        // N(10, 20²) truncated at 0 (31% of the mass is below): its 5/50/95%
        // points are 1.920, 17.937 and 46.349 (computed separately), so
        // b₁ = 1000 − c₀ has 95/50/5% points 998.080, 982.063, 953.651.
        let band = day_one_band(Some(0.0), vec![0.05, 0.5, 0.95, 0.999_999]);
        assert!((band[0] - 953.651).abs() < 1.0, "{band:?}");
        assert!((band[1] - 982.063).abs() < 0.6, "{band:?}");
        assert!((band[2] - 998.080).abs() < 0.3, "{band:?}");
        // No path spends a negative amount, so none ends above 1000.
        assert!(band[3] <= 1000.0);
        // Without the floor, the Gaussian's 95% point of b₁ is
        // 1000 − (10 − 1.645·20) = 1022.9: those paths earn money.
        let plain = day_one_band(None, vec![0.95]);
        assert!((plain[0] - 1022.9).abs() < 1.0, "{plain:?}");
    }

    #[test]
    fn spend_floor_keeps_the_balance_correlation() {
        // b and c perfectly correlated (b̂ = 100, ĉ = 0, both variance 1):
        // b₀ = 100 + (c₀ − 0), so b₁ = b₀ − c₀ = 100 on every path. Half the
        // c mass is below the floor; drawing c alone, without b given c,
        // would scatter b₁.
        let est = StateEstimate {
            x: Vector3::new(100.0, 0.0, 2000.0),
            p: Matrix3::new(1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0),
            ..quiet_estimate()
        };
        let cfg = no_lumps(ForecastConfig {
            n_paths: 500,
            spend_floor: Some(0.0),
            ..ForecastConfig::new(1, 4)
        });
        let cone = forecast(&est, &no_schedule(), &STILL, &cfg).unwrap();
        let band = cone.band(t0() + 1).unwrap();
        // Up to the Cholesky jitter (1e-12 relative) on a singular P.
        assert!(band.iter().all(|&q| (q - 100.0).abs() < 1e-4), "{band:?}");
    }

    #[test]
    fn spend_floor_reflects_the_random_walk() {
        // c starts at exactly 0.5 and walks with step sd 1. With the floor
        // every path spends ≥ 0 each day, so each path's balance, and hence
        // every quantile, never rises. Without it, half the walks go
        // negative and the upper quantiles climb.
        let est = StateEstimate {
            x: Vector3::new(1000.0, 0.5, 2000.0),
            ..quiet_estimate()
        };
        let model = ForecastModel { q_c: 1.0, q_m: 0.0 };
        let run = |floor| {
            let cfg = no_lumps(ForecastConfig {
                n_paths: 400,
                spend_floor: floor,
                ..ForecastConfig::new(60, 5)
            });
            forecast(&est, &no_schedule(), &model, &cfg).unwrap()
        };
        let rises = |cone: &Cone| {
            (1..60).any(|h| {
                let (a, b) = (
                    cone.band(t0() + h).unwrap(),
                    cone.band(t0() + h + 1).unwrap(),
                );
                a.iter().zip(b).any(|(x, y)| y > x)
            })
        };
        assert!(!rises(&run(Some(0.0))));
        assert!(rises(&run(None)));
    }

    #[test]
    fn spend_floor_must_be_finite() {
        let cfg = ForecastConfig {
            spend_floor: Some(f64::NAN),
            ..ForecastConfig::new(5, 1)
        };
        assert!(forecast(&quiet_estimate(), &no_schedule(), &STILL, &cfg).is_err());
    }

    #[test]
    fn permutation_round_trips() {
        let x = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(permute_vec(&x, C_FIRST), Vector3::new(2.0, 1.0, 3.0));
        assert_eq!(permute_back(&permute_vec(&x, C_FIRST), C_FIRST), x);
        let p = Matrix3::new(1.0, 2.0, 3.0, 2.0, 4.0, 5.0, 3.0, 5.0, 6.0);
        let q = permute(&p, C_FIRST);
        assert_eq!(q[(0, 0)], p[(IDX_C, IDX_C)]);
        assert_eq!(q[(0, 1)], p[(IDX_C, IDX_B)]);
        assert_eq!(q[(2, 2)], p[(IDX_M, IDX_M)]);
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
