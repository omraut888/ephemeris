//! The tracker: settled checkpoint, provisional tail, adaptive noise, and
//! outlier gating (design §1–§3).
//!
//! Each [`Tracker::ingest`] takes a full feed snapshot:
//! 1. [`ObservedFeed`] classifies it and finds `settled_through`.
//! 2. If the settled days already consumed still produce identical inputs, the
//!    checkpoint advances through the newly settled days only. Otherwise
//!    (a backdated record, a changed opening balance) it is rebuilt from the
//!    prior.
//! 3. A clone of the checkpoint replays the provisional tail to today. The tail
//!    never adapts noise estimates; it is thrown away on the next sync.
//!
//! On every balance observation with NIS above the gate, the excess beyond the
//! gate is treated as an unexplained lump flow (added to `b` before the update)
//! and the clipped remainder updates normally. Gated income observations are
//! skipped for `m`. Either way the variance estimator gets the clipped sample,
//! so a too-small prior cannot lock every observation out.
//!
//! Regime changes (design §6): each channel's clipped innovations feed a
//! [`ChangeDetector`]. On an alarm, the run's mean shift `Δ̂` becomes a
//! zero-mean jump of variance `Δ̂²` in `c` (balance channel, capped at a
//! multiple of `σ̂_s²`) or `m` (income channel), the matching noise estimate
//! forgets back to its prior count, and the day's innovation is recomputed
//! and re-gated before the update. A pay cut that would otherwise be gated for ever is learned from
//! the second short paycheck. The detector also runs on the provisional tail,
//! so a change shows in the live estimate before it settles; the tail's
//! detector state is thrown away with the tail.

use nalgebra::{Matrix3, Vector3};

use crate::adaptive::{AdaptiveConfig, VarianceEstimate, VarianceEstimator, sample_weight};
use crate::change::{ChangeConfig, ChangeDetector};
use crate::error::EngineError;
use crate::kalman::Innovation;
use crate::model::{FinanceFilter, IDX_B, IDX_C, IDX_M, ModelParams};
use crate::observe::{FeedDay, ObserveConfig, ObservedFeed};
use crate::types::{Channel, Day, Transaction};

/// Prior for the parts of the state the feed does not pin down. The balance
/// prior is the snapshot's opening balance with variance `r_post`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prior {
    pub c_mean: f64,
    pub c_sd: f64,
    pub m_mean: f64,
    pub m_sd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackerConfig {
    /// First day of history; the prior describes the end of the day before.
    pub start_day: Day,
    /// Random-walk variances of `c` and `m` (fixed tuning parameters).
    pub q_c: f64,
    pub q_m: f64,
    /// `σ_s²`, estimated from balance innovations.
    pub spend_var: AdaptiveConfig,
    /// `R_inc`, estimated from income innovations.
    pub income_var: AdaptiveConfig,
    /// NIS above which an innovation is gated (χ²₁ 99.9% ≈ 10.83).
    pub gate_nis: f64,
    /// Regime-change detection on the balance channel (jumps in `c`);
    /// `None` turns it off.
    pub spend_change: Option<ChangeConfig>,
    /// The largest jump variance in `c`, as a multiple of `σ̂_s²`. One day's
    /// spending then moves `c` by at most about half its deviation, so a
    /// false alarm on a lumpy day costs little; a large change sets off
    /// several alarms in a row instead.
    pub max_spend_jump: f64,
    /// Regime-change detection on the income channel (jumps in `m`).
    pub income_change: Option<ChangeConfig>,
    pub prior: Prior,
    pub observe: ObserveConfig,
}

impl TrackerConfig {
    pub fn new(start_day: Day) -> Self {
        Self {
            start_day,
            q_c: 0.25,
            q_m: 25.0,
            spend_var: AdaptiveConfig {
                prior: 2500.0,
                prior_count: 4.0,
                forgetting: 0.995,
                min: 25.0,
                max: 1e6,
            },
            income_var: AdaptiveConfig {
                prior: 1e4,
                prior_count: 2.0,
                forgetting: 0.97,
                min: 1.0,
                max: 1e8,
            },
            gate_nis: 10.83,
            // h = 6: heavy-tailed spending sets off h = 5 too often (design §6).
            spend_change: Some(ChangeConfig {
                threshold: 6.0,
                ..ChangeConfig::default()
            }),
            max_spend_jump: 1.0,
            // h = 5: two short paychecks in a row are enough; one is not.
            income_change: Some(ChangeConfig::default()),
            prior: Prior {
                c_mean: 0.0,
                c_sd: 100.0,
                m_mean: 0.0,
                m_sd: 5000.0,
            },
            observe: ObserveConfig::default(),
        }
    }

    pub fn validate(&self) -> Result<(), EngineError> {
        self.spend_var.validate()?;
        self.income_var.validate()?;
        self.observe.validate()?;
        for c in self.spend_change.iter().chain(&self.income_change) {
            c.validate()?;
        }
        if self.max_spend_jump.is_nan() || self.max_spend_jump <= 0.0 {
            return Err(EngineError::InvalidConfig(
                "tracker: max_spend_jump must be positive",
            ));
        }
        ModelParams {
            spend_var: self.spend_var.prior,
            q_c: self.q_c,
            q_m: self.q_m,
        }
        .validate()?;
        let p = self.prior;
        let sd_ok = |v: f64| v.is_finite() && v > 0.0;
        if !(self.gate_nis > 0.0
            && p.c_mean.is_finite()
            && p.m_mean.is_finite()
            && sd_ok(p.c_sd)
            && sd_ok(p.m_sd))
        {
            return Err(EngineError::InvalidConfig(
                "tracker: gate must be positive, prior finite with positive sds",
            ));
        }
        Ok(())
    }
}

/// Unexplained lump flows the balance gate absorbed on settled days. The
/// forecaster resamples them to put back the tail the gate trimmed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LumpHistory {
    pub amounts: Vec<f64>,
    /// Settled days with a balance observation (the denominator for the rate).
    pub days: u32,
}

/// The estimate handed to the forecaster.
#[derive(Debug, Clone, PartialEq)]
pub struct StateEstimate {
    pub day: Day,
    pub settled_through: Day,
    pub x: Vector3<f64>,
    /// Covariance, with pending known flows' variance added to `P_bb`.
    pub p: Matrix3<f64>,
    pub spend_var: VarianceEstimate,
    pub income_var: VarianceEstimate,
    pub lumps: LumpHistory,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GatedInnovation {
    pub channel: Channel,
    /// Before gating.
    pub innovation: Innovation,
    pub gated: bool,
}

/// A detected regime change and the covariance jump it triggered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jump {
    /// Balance: a jump in the spend rate `c`. Income: in the paycheck `m`.
    pub channel: Channel,
    /// Estimated change in the state, signed: $/day for `c` (positive means
    /// more spending), $ per paycheck for `m`. Its square is the jump variance.
    pub shift: f64,
    /// Observations in the run that set off the alarm.
    pub run_len: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayReport {
    pub day: Day,
    /// Part of the settled checkpoint (false for the provisional tail).
    pub settled: bool,
    /// After any jump: what the update actually used, before gating.
    pub innovations: Vec<GatedInnovation>,
    /// Unexplained flow absorbed into `b` by the balance gate.
    pub lump: f64,
    pub jumps: Vec<Jump>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IngestReport {
    /// The checkpoint was rebuilt from the prior.
    pub rebuilt: bool,
    pub days: Vec<DayReport>,
}

#[derive(Debug, Clone, PartialEq)]
struct FilterState {
    filter: FinanceFilter,
    spend: VarianceEstimator,
    income: VarianceEstimator,
    days_since_balance: u32,
    spend_var_since_balance: f64,
    lumps: LumpHistory,
    balance_change: Option<ChangeDetector>,
    income_change: Option<ChangeDetector>,
    max_spend_jump: f64,
    /// Consecutive gated income observations, signed by their innovation
    /// (the gate escape, [`INCOME_GATE_ESCAPE`]).
    income_gated_run: i32,
}

/// After this many consecutive income observations gated on the same side,
/// the gate treats them as a new pay level: `m` gets a jump of variance ν²
/// and the income noise estimate forgets, as on a detector alarm.
///
/// Without it, the paycheck estimate depends on the detector to leave its
/// first level: a small first income record makes later paychecks outliers,
/// the gate skips their updates, and the income variance inflates until the
/// gain on `m` is too small for it ever to catch up (paper §9, post-freeze
/// fix). With the detector on, its alarm usually fires on the second gated
/// paycheck first and the escape does not trigger.
pub const INCOME_GATE_ESCAPE: i32 = 2;

impl FilterState {
    fn new(cfg: &TrackerConfig, opening: f64) -> Result<Self, EngineError> {
        let params = ModelParams {
            spend_var: cfg.spend_var.prior,
            q_c: cfg.q_c,
            q_m: cfg.q_m,
        };
        let x0 = Vector3::new(opening, cfg.prior.c_mean, cfg.prior.m_mean);
        let p0 = Matrix3::from_diagonal(&Vector3::new(
            cfg.observe.r_post,
            cfg.prior.c_sd.powi(2),
            cfg.prior.m_sd.powi(2),
        ));
        Ok(Self {
            filter: FinanceFilter::new(params, cfg.start_day - 1, x0, p0)?,
            spend: VarianceEstimator::new(cfg.spend_var)?,
            income: VarianceEstimator::new(cfg.income_var)?,
            days_since_balance: 0,
            spend_var_since_balance: 0.0,
            lumps: LumpHistory::default(),
            balance_change: cfg.spend_change.map(ChangeDetector::new).transpose()?,
            income_change: cfg.income_change.map(ChangeDetector::new).transpose()?,
            max_spend_jump: cfg.max_spend_jump,
            income_gated_run: 0,
        })
    }

    fn estimate(&self, settled_through: Day, shared_var: f64) -> StateEstimate {
        let mut p = *self.filter.covariance();
        p[(IDX_B, IDX_B)] += shared_var;
        StateEstimate {
            day: self.filter.day(),
            settled_through,
            x: *self.filter.state(),
            p,
            spend_var: self.spend.estimate(),
            income_var: self.income.estimate(),
            lumps: self.lumps.clone(),
        }
    }

    /// One day: predict with `u`, gated balance update, gated income updates,
    /// and (when `adapt`) covariance-matching samples for both variances.
    fn run_day(
        &mut self,
        fd: &FeedDay,
        gate: f64,
        adapt: bool,
        settled: bool,
    ) -> Result<DayReport, EngineError> {
        debug_assert_eq!(fd.day, self.filter.day() + 1);
        let spend_var = self.spend.value();
        self.filter.set_spend_var(spend_var)?;
        self.filter.predict(fd.u)?;
        self.days_since_balance += 1;
        self.spend_var_since_balance += spend_var;

        let mut report = DayReport {
            day: fd.day,
            settled,
            innovations: Vec::new(),
            lump: 0.0,
            jumps: Vec::new(),
        };

        if let Some((z, r)) = fd.balance {
            let g = f64::from(self.days_since_balance);
            let mut inn = self.filter.innovation(Channel::Balance, r, z)?;
            if let Some(det) = &mut self.balance_change
                && let Some(alarm) = det.observe(clip(inn, gate), inn.s, g)?
            {
                // The balance fell by Δc more than predicted each day.
                let var = alarm.shift.powi(2).min(self.max_spend_jump * spend_var);
                self.filter.add_jump(IDX_C, var)?;
                self.spend.forget();
                inn = self.filter.innovation(Channel::Balance, r, z)?;
                report.jumps.push(Jump {
                    channel: Channel::Balance,
                    shift: -alarm.shift,
                    run_len: alarm.run_len,
                });
            }
            let gated = inn.nis > gate;
            let nu = clip(inn, gate);
            if gated {
                report.lump = inn.nu - nu;
                self.filter.shift_balance(report.lump)?;
            }
            self.filter.update(Channel::Balance, r, z)?;
            if settled {
                self.lumps.days += 1;
                if gated {
                    self.lumps.amounts.push(report.lump);
                }
            }
            if adapt {
                // E[ν²] = S, of which g days of spending contribute g·σ_s².
                let spend_part = self.spend_var_since_balance;
                let sample = (nu * nu - inn.s + spend_part) / g;
                self.spend
                    .update(sample, sample_weight(spend_part, sample * g, inn.s))?;
            }
            self.days_since_balance = 0;
            self.spend_var_since_balance = 0.0;
            report.innovations.push(GatedInnovation {
                channel: Channel::Balance,
                innovation: inn,
                gated,
            });
        }

        for &amount in &fd.income {
            let r = self.income.value();
            let mut inn = self.filter.innovation(Channel::Income, r, amount)?;
            if let Some(det) = &mut self.income_change
                && let Some(alarm) = det.observe(clip(inn, gate), inn.s, 1.0)?
            {
                self.filter.add_jump(IDX_M, alarm.shift.powi(2))?;
                self.income.forget();
                inn = self.filter.innovation(Channel::Income, r, amount)?;
                report.jumps.push(Jump {
                    channel: Channel::Income,
                    shift: alarm.shift,
                    run_len: alarm.run_len,
                });
            }
            let mut gated = inn.nis > gate;
            if gated {
                let side = if inn.nu > 0.0 { 1 } else { -1 };
                let run = if self.income_gated_run.signum() == side {
                    self.income_gated_run + side
                } else {
                    side
                };
                if run.abs() >= INCOME_GATE_ESCAPE {
                    // A new pay level, not an outlier (see INCOME_GATE_ESCAPE).
                    self.filter.add_jump(IDX_M, inn.nu.powi(2))?;
                    self.income.forget();
                    inn = self.filter.innovation(Channel::Income, r, amount)?;
                    gated = inn.nis > gate;
                    self.income_gated_run = 0;
                } else {
                    self.income_gated_run = run;
                }
            } else {
                self.income_gated_run = 0;
            }
            if !gated {
                self.filter.update(Channel::Income, r, amount)?;
            }
            if adapt {
                let nu2 = (inn.nu * inn.nu).min(gate * inn.s);
                let sample = nu2 - (inn.s - r);
                self.income
                    .update(sample, sample_weight(r, sample, inn.s))?;
            }
            report.innovations.push(GatedInnovation {
                channel: Channel::Income,
                innovation: inn,
                gated,
            });
        }
        Ok(report)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tracker {
    cfg: TrackerConfig,
    opening: Option<f64>,
    checkpoint: Option<FilterState>,
    /// Settled inputs already folded into the checkpoint, in day order.
    consumed: Vec<FeedDay>,
    live: Option<StateEstimate>,
}

impl Tracker {
    pub fn new(cfg: TrackerConfig) -> Result<Self, EngineError> {
        cfg.validate()?;
        Ok(Self {
            cfg,
            opening: None,
            checkpoint: None,
            consumed: Vec::new(),
            live: None,
        })
    }

    pub fn config(&self) -> &TrackerConfig {
        &self.cfg
    }

    /// The live estimate for the last ingested `today` (includes the tail).
    pub fn estimate(&self) -> Option<&StateEstimate> {
        self.live.as_ref()
    }

    /// The settled checkpoint, without the provisional tail.
    pub fn checkpoint(&self) -> Option<StateEstimate> {
        self.checkpoint
            .as_ref()
            .map(|s| s.estimate(s.filter.day(), 0.0))
    }

    /// Ingest everything the feed shows at the end of `today`, plus the bank's
    /// posted balance across all linked accounts. On error the tracker is
    /// unchanged.
    pub fn ingest(
        &mut self,
        txns: &[Transaction],
        ledger_balance: f64,
        today: Day,
    ) -> Result<IngestReport, EngineError> {
        let mut next = self.clone();
        let report = next.ingest_in_place(txns, ledger_balance, today)?;
        *self = next;
        Ok(report)
    }

    fn ingest_in_place(
        &mut self,
        txns: &[Transaction],
        ledger_balance: f64,
        today: Day,
    ) -> Result<IngestReport, EngineError> {
        if today < self.cfg.start_day {
            return Err(EngineError::InvalidInput("today precedes start_day"));
        }
        if txns.iter().any(|t| t.day() < self.cfg.start_day) {
            return Err(EngineError::InvalidInput("transaction precedes start_day"));
        }
        let feed = ObservedFeed::new(txns, ledger_balance, today, &self.cfg.observe)?;
        let settled_through = feed.settled_through().max(self.cfg.start_day - 1);

        let consistent = self
            .opening
            .is_some_and(|o| money_eq(o, feed.opening_balance()))
            && self.consumed.iter().all(|fd| {
                feed.settled_day(fd.day)
                    .is_some_and(|new| feed_day_eq(fd, &new))
            });
        let rebuilt = !consistent && self.checkpoint.is_some();
        if !consistent {
            self.checkpoint = Some(FilterState::new(&self.cfg, feed.opening_balance())?);
            self.opening = Some(feed.opening_balance());
            self.consumed.clear();
        }

        let gate = self.cfg.gate_nis;
        let mut days = Vec::new();
        let checkpoint = self
            .checkpoint
            .as_mut()
            .expect("checkpoint initialized above");
        for d in checkpoint.filter.day() + 1..=settled_through {
            let fd = feed.settled_day(d).expect("d <= settled_through");
            days.push(checkpoint.run_day(&fd, gate, true, true)?);
            self.consumed.push(fd);
        }

        let mut live = checkpoint.clone();
        let checkpoint_day = live.filter.day();
        for fd in feed.tail_days().iter().filter(|fd| fd.day > checkpoint_day) {
            days.push(live.run_day(fd, gate, false, false)?);
        }
        self.live = Some(live.estimate(settled_through, feed.shared_variance()));
        Ok(IngestReport { rebuilt, days })
    }
}

/// The innovation with its excess beyond the gate removed (Huber clip).
fn clip(inn: Innovation, gate: f64) -> f64 {
    let bound = (gate * inn.s).sqrt();
    inn.nu.clamp(-bound, bound)
}

/// Amounts recomputed from a later snapshot can differ in the last bits
/// because sums are taken in a different order; anything under a millionth of
/// a dollar is the same amount.
fn money_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6
}

fn feed_day_eq(a: &FeedDay, b: &FeedDay) -> bool {
    let balance_eq = match (a.balance, b.balance) {
        (Some((za, ra)), Some((zb, rb))) => money_eq(za, zb) && money_eq(ra, rb),
        (None, None) => true,
        _ => false,
    };
    a.day == b.day
        && money_eq(a.u, b.u)
        && balance_eq
        && a.income.len() == b.income.len()
        && a.income
            .iter()
            .zip(&b.income)
            .all(|(x, y)| money_eq(*x, *y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive::AdaptiveConfig;
    use crate::model::{IDX_C, IDX_M};
    use crate::synth::{SynthConfig, generate};
    use crate::types::{AccountId, FlowKind, TxnId, TxnStatus};

    /// 2024-01-01.
    const START: Day = 19_723;

    fn posted(id: u64, day: Day, amount: f64, kind: FlowKind) -> Transaction {
        Transaction {
            id: TxnId(id),
            account: AccountId(0),
            date: day,
            authorized_date: Some(day),
            status: TxnStatus::Posted,
            amount,
            kind,
            stream: None,
            pending_id: None,
        }
    }

    /// Run a synthetic history day by day; returns the tracker.
    fn replay(cfg: &SynthConfig, tcfg: TrackerConfig, seed: u64, days: u32) -> Tracker {
        let run = generate(cfg, seed).unwrap();
        let mut t = Tracker::new(tcfg).unwrap();
        for k in cfg.start_day..cfg.start_day + days as Day {
            t.ingest(&run.snapshot(k), run.ledger_balance(k), k)
                .unwrap();
        }
        t
    }

    #[test]
    fn daily_ingest_matches_one_shot_ingest() {
        let cfg = SynthConfig::realistic(START);
        let run = generate(&cfg, 4).unwrap();
        let daily = replay(&cfg, TrackerConfig::new(START), 4, 120);
        let today = START + 119;
        let mut once = Tracker::new(TrackerConfig::new(START)).unwrap();
        let report = once
            .ingest(&run.snapshot(today), run.ledger_balance(today), today)
            .unwrap();
        assert!(!report.rebuilt);
        // Same inputs up to summation-order rounding.
        let (a, b) = (daily.estimate().unwrap(), once.estimate().unwrap());
        assert_eq!((a.day, a.settled_through), (b.day, b.settled_through));
        assert!((a.x - b.x).amax() < 1e-6, "{} vs {}", a.x, b.x);
        assert!((a.p - b.p).amax() < 1e-6);
        assert!((a.spend_var.value - b.spend_var.value).abs() < 1e-6);
        assert_eq!(a.income_var, b.income_var);
    }

    #[test]
    fn backdated_record_triggers_rebuild() {
        let d = START + 10;
        let base = vec![
            posted(1, d, 2000.0, FlowKind::Income),
            posted(2, d + 1, -40.0, FlowKind::Discretionary),
        ];
        let mut t = Tracker::new(TrackerConfig::new(START)).unwrap();
        assert!(!t.ingest(&base, 1960.0, d + 2).unwrap().rebuilt);

        let mut late = base.clone();
        late.push(posted(3, d, -25.0, FlowKind::Discretionary));
        let report = t.ingest(&late, 1935.0, d + 3).unwrap();
        assert!(report.rebuilt);

        let mut fresh = Tracker::new(TrackerConfig::new(START)).unwrap();
        fresh.ingest(&late, 1935.0, d + 3).unwrap();
        assert_eq!(t.estimate(), fresh.estimate());
        // An unchanged snapshot does not rebuild again.
        assert!(!t.ingest(&late, 1935.0, d + 4).unwrap().rebuilt);
    }

    #[test]
    fn unexplained_lump_is_absorbed_without_dragging_c() {
        // 60 quiet days at 50/day, then an untagged $2,400 inflow.
        let mut txns: Vec<Transaction> = (0..60)
            .map(|i| posted(i, START + i as Day, -50.0, FlowKind::Discretionary))
            .collect();
        let mut balance = 10_000.0 - 50.0 * 60.0;
        let mut t = Tracker::new(TrackerConfig::new(START)).unwrap();
        t.ingest(&txns, balance, START + 59).unwrap();
        let c_before = t.estimate().unwrap().x[IDX_C];

        txns.push(posted(60, START + 60, 2400.0, FlowKind::Discretionary));
        balance += 2400.0;
        let report = t.ingest(&txns, balance, START + 60).unwrap();
        let day = report.days.last().unwrap();
        assert!(day.innovations[0].gated);
        assert!(day.lump > 2000.0, "lump {}", day.lump);
        let est = t.estimate().unwrap();
        assert!((est.x[IDX_B] - balance).abs() < 1.0, "b tracks the bank");
        assert!(
            (est.x[IDX_C] - c_before).abs() < 5.0,
            "c moved {}",
            est.x[IDX_C] - c_before
        );
    }

    #[test]
    fn bonus_paycheck_is_gated_for_m() {
        let mut txns = Vec::new();
        let mut balance = 1000.0;
        for k in 0..10 {
            let d = START + 14 * k;
            let amount = if k == 9 {
                7000.0
            } else {
                2000.0 + 10.0 * (k % 3) as f64
            };
            txns.push(posted(k as u64, d, amount, FlowKind::Income));
            balance += amount;
        }
        let mut t = Tracker::new(TrackerConfig::new(START)).unwrap();
        let report = t.ingest(&txns, balance, START + 126).unwrap();
        let income: Vec<&GatedInnovation> = report
            .days
            .iter()
            .flat_map(|d| &d.innovations)
            .filter(|i| i.channel == Channel::Income)
            .collect();
        assert_eq!(income.len(), 10);
        assert!(income[..9].iter().all(|i| !i.gated));
        assert!(income[9].gated);
        let m = t.estimate().unwrap().x[IDX_M];
        assert!((m - 2010.0).abs() < 20.0, "m = {m}");
    }

    /// Biweekly paychecks of 2000, then a 20% pay cut. One short paycheck
    /// is gated like a one-off deduction; the second sets off the income
    /// detector and `m` follows.
    #[test]
    fn pay_cut_is_learned_from_the_second_short_paycheck() {
        let mut txns = Vec::new();
        let mut balance = 1000.0;
        for k in 0..14 {
            let amount = if k < 10 { 2000.0 } else { 1600.0 };
            txns.push(posted(k as u64, START + 14 * k, amount, FlowKind::Income));
            balance += amount;
        }
        let today = START + 14 * 13;
        let run = |change| {
            let mut tcfg = TrackerConfig::new(START);
            tcfg.income_change = change;
            let mut t = Tracker::new(tcfg).unwrap();
            let report = t.ingest(&txns, balance, today).unwrap();
            let income: Vec<(Day, bool, Vec<Jump>)> = report
                .days
                .iter()
                .filter(|d| !d.jumps.is_empty() || d.innovations.len() > 1)
                .map(|d| (d.day, d.innovations[1].gated, d.jumps.clone()))
                .collect();
            (t.estimate().unwrap().x[IDX_M], income)
        };

        let (m, income) = run(Some(ChangeConfig::default()));
        assert!(income[..10].iter().all(|(_, g, j)| !g && j.is_empty()));
        let (day, gated, jumps) = &income[10];
        assert!(
            *gated && jumps.is_empty(),
            "first short paycheck: gated only"
        );
        assert_eq!(*day, START + 140);
        let (_, gated, jumps) = &income[11];
        assert!(!gated, "re-gated after the jump, and accepted");
        assert_eq!(jumps.len(), 1);
        assert_eq!(jumps[0].channel, Channel::Income);
        assert!(jumps[0].shift < -150.0, "{:?}", jumps[0]);
        assert!((m - 1600.0).abs() < 30.0, "m = {m}");

        // Without the detector the gate escape does the same: the first
        // short paycheck is gated, the second, gated on the same side,
        // starts a new pay level (INCOME_GATE_ESCAPE). Before the
        // post-freeze fix, both stayed gated and m ended above 1900.
        let (m, income) = run(None);
        assert!(income[10].1, "first short paycheck: gated");
        assert!(!income[11].1, "second: escaped and accepted");
        assert!((m - 1600.0).abs() < 30.0, "m = {m}");
    }

    /// A small first income record (as in many Berka accounts), then a
    /// monthly pay of 8000. With no detector, m must still reach the pay
    /// level: by hand, the first record sets m ≈ 300 with P_mm ≈ R = 10⁴,
    /// the next paycheck is gated (NIS ≈ 7700²/2·10⁴ ≫ 10.83), and the
    /// second gated one on the same side escapes.
    #[test]
    fn small_first_paycheck_does_not_lock_m_without_the_detector() {
        let mut txns = vec![posted(0, START, 300.0, FlowKind::Income)];
        let mut balance = 1300.0;
        for k in 1..13 {
            txns.push(posted(k, START + 30 * k as Day, 8000.0, FlowKind::Income));
            balance += 8000.0;
        }
        let mut tcfg = TrackerConfig::new(START);
        tcfg.income_change = None;
        tcfg.spend_change = None;
        let mut t = Tracker::new(tcfg).unwrap();
        let report = t.ingest(&txns, balance, START + 360).unwrap();
        let gated: Vec<bool> = report
            .days
            .iter()
            .flat_map(|d| &d.innovations)
            .filter(|i| i.channel == Channel::Income)
            .map(|i| i.gated)
            .collect();
        assert_eq!(gated[..3], [false, true, false]);
        assert!(gated[3..].iter().all(|g| !g));
        let m = t.estimate().unwrap().x[IDX_M];
        assert!((m - 8000.0).abs() < 80.0, "m = {m}");
    }

    /// Outliers on alternating sides are one-offs, not a new level: no
    /// escape, and m stays at the regular pay.
    #[test]
    fn alternating_outliers_do_not_escape_the_gate() {
        let mut txns = Vec::new();
        let mut balance = 1000.0;
        for k in 0..12u64 {
            let amount = match k {
                8 => 6000.0,
                9 => 200.0,
                _ => 2000.0,
            };
            txns.push(posted(k, START + 14 * k as Day, amount, FlowKind::Income));
            balance += amount;
        }
        let mut tcfg = TrackerConfig::new(START);
        tcfg.income_change = None;
        let mut t = Tracker::new(tcfg).unwrap();
        let report = t.ingest(&txns, balance, START + 14 * 11).unwrap();
        let gated: Vec<bool> = report
            .days
            .iter()
            .flat_map(|d| &d.innovations)
            .filter(|i| i.channel == Channel::Income)
            .map(|i| i.gated)
            .collect();
        assert!(gated[8] && gated[9], "{gated:?}");
        let m = t.estimate().unwrap().x[IDX_M];
        assert!((m - 2000.0).abs() < 20.0, "m = {m}");
    }

    /// 60 quiet days at 50/day, then 150/day: the balance detector fires
    /// within days, `c` catches up, and `P_cc` shows the uncertainty.
    #[test]
    fn spend_step_sets_off_a_jump_in_c() {
        let day_spend = |i: u64| if i < 60 { 50.0 } else { 150.0 };
        let txns: Vec<Transaction> = (0..90)
            .map(|i| {
                // ±20 alternating, so innovations are not all identical.
                let wiggle = if i % 2 == 0 { 20.0 } else { -20.0 };
                posted(
                    i,
                    START + i as Day,
                    -(day_spend(i) + wiggle),
                    FlowKind::Discretionary,
                )
            })
            .collect();
        let ledger = |n: usize| 10_000.0 + txns[..n].iter().map(|t| t.amount).sum::<f64>();
        let mut t = Tracker::new(TrackerConfig::new(START)).unwrap();
        let mut jumps = Vec::new();
        let mut c_sd_before = f64::INFINITY;
        for k in 59..90 {
            let report = t
                .ingest(&txns[..=k], ledger(k + 1), START + k as Day)
                .unwrap();
            for d in &report.days {
                jumps.extend(d.jumps.iter().map(|j| (d.day - START, *j)));
            }
            if k == 59 {
                c_sd_before = t.estimate().unwrap().p[(IDX_C, IDX_C)].sqrt();
            }
            let c_sd = t.estimate().unwrap().p[(IDX_C, IDX_C)].sqrt();
            if report.days.iter().any(|d| !d.jumps.is_empty()) {
                assert!(c_sd > 4.0 * c_sd_before, "sd {c_sd} vs {c_sd_before}");
            }
        }
        let (day, jump) = jumps[0];
        assert!((60..=64).contains(&day), "first jump on day {day}");
        assert_eq!(jump.channel, Channel::Balance);
        assert!(jump.shift > 50.0, "{jump:?}");
        let c = t.estimate().unwrap().x[IDX_C];
        assert!((c - 150.0).abs() < 10.0, "c = {c}");
    }

    /// The cap bounds the jump variance at a multiple of σ̂_s²: a large
    /// step then sets off several smaller jumps instead of one huge one.
    #[test]
    fn spend_jumps_are_capped() {
        let txns: Vec<Transaction> = (0..70)
            .map(|i| {
                let spend = if i < 60 { 50.0 } else { 400.0 };
                let wiggle = if i % 2 == 0 { 20.0 } else { -20.0 };
                posted(
                    i,
                    START + i as Day,
                    -spend - wiggle,
                    FlowKind::Discretionary,
                )
            })
            .collect();
        let ledger = 10_000.0 + txns.iter().map(|t| t.amount).sum::<f64>();
        let run = |cap: f64| {
            let mut tcfg = TrackerConfig::new(START);
            tcfg.max_spend_jump = cap;
            let mut t = Tracker::new(tcfg).unwrap();
            let report = t.ingest(&txns, ledger, START + 69).unwrap();
            let jumps: Vec<(Day, f64)> = report
                .days
                .iter()
                .filter(|d| d.day >= START + 60)
                .flat_map(|d| d.jumps.iter().map(move |j| (d.day - START, j.shift)))
                .collect();
            (jumps, t.estimate().unwrap().x[IDX_C])
        };
        let (free, _) = run(1e9);
        let (capped, c) = run(1.0);
        // The same first alarm; under the cap it buys less variance, so the
        // detector fires again within days and c still catches up.
        assert_eq!(capped[0], free[0]);
        assert!(capped.len() > free.len(), "{capped:?} vs {free:?}");
        assert!(c > 300.0, "c = {c}");

        let mut bad = TrackerConfig::new(START);
        for cap in [0.0, -1.0, f64::NAN] {
            bad.max_spend_jump = cap;
            assert!(Tracker::new(bad).is_err(), "{cap}");
        }
    }

    #[test]
    fn tiny_prior_does_not_lock_out_income_variance() {
        let mut tcfg = TrackerConfig::new(START);
        tcfg.income_var.prior = 1.0;
        let t = replay(&SynthConfig::model_matched(START), tcfg, 6, 365);
        let r = t.estimate().unwrap().income_var.value;
        assert!(r > 500.0, "R_inc stuck at {r}");
    }

    #[test]
    fn adaptive_variances_approach_truth() {
        let t = replay(
            &SynthConfig::model_matched(START),
            TrackerConfig::new(START),
            7,
            365,
        );
        let est = t.estimate().unwrap();
        let (s, r) = (est.spend_var.value, est.income_var.value);
        assert!((s - 1600.0).abs() < 400.0, "σ_s² = {s}");
        assert!((r - 2500.0).abs() < 1500.0, "R_inc = {r}");
        assert!(est.spend_var.dof > 50.0 && est.income_var.dof > 5.0);
    }

    #[test]
    fn fixed_noise_stays_fixed() {
        let mut tcfg = TrackerConfig::new(START);
        tcfg.spend_var = AdaptiveConfig::fixed(1600.0);
        tcfg.income_var = AdaptiveConfig::fixed(2500.0);
        let t = replay(&SynthConfig::model_matched(START), tcfg, 7, 100);
        let est = t.estimate().unwrap();
        assert_eq!(
            (est.spend_var.value, est.income_var.value),
            (1600.0, 2500.0)
        );
    }

    #[test]
    fn errors_leave_tracker_unchanged() {
        let mut t = Tracker::new(TrackerConfig::new(START)).unwrap();
        let ok = [posted(1, START, -10.0, FlowKind::Discretionary)];
        t.ingest(&ok, 990.0, START + 1).unwrap();
        let before = t.clone();
        assert!(t.ingest(&ok, f64::NAN, START + 2).is_err());
        assert!(
            t.ingest(
                &[posted(2, START - 1, -1.0, FlowKind::Discretionary)],
                0.0,
                START
            )
            .is_err()
        );
        assert!(t.ingest(&ok, 990.0, START - 1).is_err());
        assert_eq!(t, before);
    }
}
