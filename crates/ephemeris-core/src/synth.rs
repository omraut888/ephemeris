//! Seeded synthetic transaction generator with known ground truth (design §5).
//!
//! The generator simulates the hidden state `x = [b, c, m]` day by day and emits
//! the transaction lifecycle a bank feed would show: purchases authorized on one
//! day, pending for a few business days, then posted at a possibly different
//! amount. [`SynthRun::snapshot`] turns that lifecycle into the
//! [`Transaction`] records visible at the end of any given day.
//!
//! Truth follows the dynamics of design §1:
//! `b_k = b_{k-1} + (all final amounts authorized on day k)`, where discretionary
//! spend has mean `c_{k-1}` (times a day-of-week factor), and `c`, `m` are
//! Gaussian random walks. Paycheck amounts are drawn around the updated `m_k`.
//! Transfers between the user's two accounts move money but leave `b` unchanged.
//!
//! Every component (state walks, spending, income, transfers, each subscription)
//! draws from its own ChaCha8 stream, so changing one component's config does not
//! perturb the others' draws.

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, Poisson, StandardNormal};

use crate::calendar::{PaySchedule, add_business_days, weekday};
use crate::error::EngineError;
use crate::types::{AccountId, Day, FlowKind, StreamId, Transaction, TxnId, TxnStatus};

/// The account that receives income and pays for everything.
pub const CHECKING: AccountId = AccountId(0);
/// The other side of own-account transfers.
pub const SAVINGS: AccountId = AccountId(1);

/// Stream id of the primary income.
pub const INCOME_STREAM: StreamId = StreamId(0);

/// Stream id of the `i`-th configured subscription.
pub fn subscription_stream(i: usize) -> StreamId {
    StreamId(1 + i as u32)
}

const RNG_STATE: u64 = 0;
const RNG_SPEND: u64 = 1;
const RNG_INCOME: u64 = 2;
const RNG_TRANSFER: u64 = 3;
const RNG_SUBSCRIPTION_BASE: u64 = 16;

/// Longest horizon accepted, to keep day arithmetic far from `i32` overflow.
const MAX_DAYS: u32 = 100 * 366;

/// The most days [`PaySchedule::pay_date`] can move a payday back
/// (Monday holiday plus a weekend).
const MAX_PAY_ROLLBACK: u32 = 3;

/// The hidden state `x = [b, c, m]` (design §1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrueState {
    /// True disposable balance across all accounts, end of day ($).
    pub b: f64,
    /// Baseline discretionary spend rate ($/day, outflow positive).
    pub c: f64,
    /// Expected primary-income amount ($/event).
    pub m: f64,
}

/// Ground truth at the end of one day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TruthDay {
    pub day: Day,
    pub state: TrueState,
}

/// How discretionary spending is drawn each day. Either way the day's mean is
/// `c_{k-1}` times that weekday's (normalized) factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpendModel {
    /// One net transaction per day, `−N(mean, sigma²)`. Exactly the filter's model
    /// when all weekday factors are 1.
    Gaussian { sigma: f64 },
    /// `Poisson(mean_count)` purchases per day, each lognormal with log-sd
    /// `sigma_log` and mean `max(mean, 0) / mean_count`. Skewed and often zero.
    PoissonLogNormal { mean_count: f64, sigma_log: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IncomeConfig {
    pub schedule: PaySchedule,
    /// Standard deviation of each paycheck around `m` (√R_inc).
    pub amount_sd: f64,
    /// Probability that a paycheck lands off its nominal date (processing
    /// delays, early direct deposit), on top of weekend and holiday shifts.
    pub jitter_prob: f64,
    /// An off-schedule paycheck moves by 1..=max_jitter_days, early or late.
    /// It then moves back to the previous business day if needed.
    pub max_jitter_days: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubscriptionConfig {
    /// Charge size, positive (emitted as an outflow).
    pub amount: f64,
    /// First charge; must not precede `start_day`.
    pub first_day: Day,
    pub period_days: u32,
    /// Per charge, probability that the billing date slips a day earlier or
    /// later. Slips persist: the phase follows a random walk.
    pub drift_prob: f64,
}

/// Settlement of card transactions (discretionary and subscriptions).
/// Income is direct deposit and always posts on the day it arrives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PostingConfig {
    /// Lag from authorization to posting, uniform over 0..=max business days.
    pub max_lag_business_days: u32,
    /// Probability that a posted record omits its authorized date.
    pub missing_authorized_prob: f64,
}

/// Transfers between checking and savings. Each is two records that cancel:
/// the outgoing leg shows as pending at once, the incoming leg appears only when
/// it posts, 0–1 business days after the outgoing leg.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransferConfig {
    pub daily_prob: f64,
    /// Amounts are uniform on `[0.5, 1.5] × mean_amount`.
    pub mean_amount: f64,
    /// Probability a transfer goes checking → savings rather than back.
    pub to_savings_prob: f64,
}

/// How a pending amount differs from the final posted amount.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PendingModel {
    /// `pending = final + N(0, max(rel_sd·|final|, min_sd)²)` for every card
    /// transaction: the pending-noise model of design §2.
    Gaussian { rel_sd: f64, min_sd: f64 },
    /// Discretionary purchases only. With `hold_prob`, the pending amount is a
    /// flat `hold_amount` pre-authorization. Otherwise, with `tip_prob`, the
    /// pending amount excludes a tip of up to `max_tip_frac`.
    Realistic {
        tip_prob: f64,
        max_tip_frac: f64,
        hold_prob: f64,
        hold_amount: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SynthConfig {
    pub start_day: Day,
    pub n_days: u32,
    /// State at the end of `start_day − 1`.
    pub initial: TrueState,
    /// Daily random-walk variance of `c`.
    pub q_c: f64,
    /// Daily random-walk variance of `m`.
    pub q_m: f64,
    pub spend: SpendModel,
    /// Spend multipliers, Monday first. Normalized to average 1, so `c` stays
    /// the weekly mean. The filter does not model these (design §1).
    pub weekday_factors: [f64; 7],
    pub income: IncomeConfig,
    pub subscriptions: Vec<SubscriptionConfig>,
    pub transfers: TransferConfig,
    pub posting: PostingConfig,
    pub pending: PendingModel,
}

impl SynthConfig {
    /// Data drawn from exactly the model the filter assumes (design §5 mode 1).
    pub fn model_matched(start_day: Day) -> Self {
        Self::base(start_day)
    }

    /// Data that departs from the filter's model (design §5 mode 2): skewed,
    /// zero-inflated spending with a weekly pattern, tips and holds, missing
    /// authorized dates.
    pub fn realistic(start_day: Day) -> Self {
        Self {
            spend: SpendModel::PoissonLogNormal {
                mean_count: 2.5,
                sigma_log: 0.9,
            },
            weekday_factors: [0.9, 0.85, 0.9, 1.0, 1.2, 1.35, 0.8],
            posting: PostingConfig {
                max_lag_business_days: 3,
                missing_authorized_prob: 0.05,
            },
            pending: PendingModel::Realistic {
                tip_prob: 0.25,
                max_tip_frac: 0.25,
                hold_prob: 0.05,
                hold_amount: 1.0,
            },
            ..Self::base(start_day)
        }
    }

    fn base(start_day: Day) -> Self {
        let first_friday = (start_day..)
            .find(|&d| weekday(d) == 4)
            .unwrap_or(start_day);
        let sub = |amount, offset, drift_prob| SubscriptionConfig {
            amount,
            first_day: start_day + offset,
            period_days: 30,
            drift_prob,
        };
        Self {
            start_day,
            n_days: 365,
            initial: TrueState {
                b: 3000.0,
                c: 60.0,
                m: 2400.0,
            },
            q_c: 0.25,
            q_m: 25.0,
            spend: SpendModel::Gaussian { sigma: 40.0 },
            weekday_factors: [1.0; 7],
            income: IncomeConfig {
                schedule: PaySchedule::Biweekly {
                    anchor: first_friday,
                },
                amount_sd: 50.0,
                jitter_prob: 0.3,
                max_jitter_days: 1,
            },
            subscriptions: vec![
                sub(1500.0, 0, 0.05),
                sub(15.99, 4, 0.2),
                sub(40.0, 11, 0.2),
                sub(65.0, 19, 0.1),
            ],
            transfers: TransferConfig {
                daily_prob: 0.05,
                mean_amount: 300.0,
                to_savings_prob: 0.7,
            },
            posting: PostingConfig {
                max_lag_business_days: 3,
                missing_authorized_prob: 0.0,
            },
            pending: PendingModel::Gaussian {
                rel_sd: 0.2,
                min_sd: 20.0,
            },
        }
    }

    pub fn validate(&self) -> Result<(), EngineError> {
        use EngineError::InvalidConfig as E;
        let finite = |x: f64| x.is_finite();
        let variance = |x: f64| x.is_finite() && x >= 0.0;
        let prob = |p: f64| (0.0..=1.0).contains(&p);

        if self.n_days > MAX_DAYS {
            return Err(E("n_days exceeds 100 years"));
        }
        if self.start_day.checked_add(MAX_DAYS as Day * 2).is_none()
            || self.start_day.checked_sub(MAX_DAYS as Day).is_none()
        {
            return Err(E("start_day too close to the edge of the Day range"));
        }
        let s = self.initial;
        if !(finite(s.b) && finite(s.c) && finite(s.m)) {
            return Err(E("initial state must be finite"));
        }
        if !(variance(self.q_c) && variance(self.q_m)) {
            return Err(E("q_c and q_m must be non-negative and finite"));
        }
        match self.spend {
            SpendModel::Gaussian { sigma } if !variance(sigma) => {
                return Err(E("spend sigma must be non-negative and finite"));
            }
            SpendModel::PoissonLogNormal {
                mean_count,
                sigma_log,
            } if !(mean_count.is_finite() && mean_count > 0.0 && variance(sigma_log)) => {
                return Err(E(
                    "spend mean_count must be positive, sigma_log non-negative",
                ));
            }
            _ => {}
        }
        if !self.weekday_factors.iter().all(|&f| variance(f))
            || self.weekday_factors.iter().sum::<f64>() <= 0.0
        {
            return Err(E(
                "weekday_factors must be non-negative with a positive sum",
            ));
        }

        let inc = &self.income;
        inc.schedule.validate()?;
        if inc.max_jitter_days > 7 {
            return Err(E("income max_jitter_days must be at most 7"));
        }
        // Jitter plus the business-day rollback must never reorder paydays.
        if inc.schedule.min_gap_days() <= 2 * inc.max_jitter_days + MAX_PAY_ROLLBACK {
            return Err(E("income schedule too frequent for max_jitter_days"));
        }
        if !variance(inc.amount_sd) || !prob(inc.jitter_prob) {
            return Err(E("income amount_sd or jitter_prob out of range"));
        }

        for sub in &self.subscriptions {
            if sub.first_day < self.start_day {
                return Err(E("subscription first_day precedes start_day"));
            }
            if sub.period_days < 3 {
                return Err(E("subscription period_days must be at least 3"));
            }
            if !(finite(sub.amount) && sub.amount >= 0.0 && prob(sub.drift_prob)) {
                return Err(E("subscription amount or drift_prob out of range"));
            }
        }

        let t = &self.transfers;
        if !(prob(t.daily_prob) && prob(t.to_savings_prob) && variance(t.mean_amount)) {
            return Err(E("transfer parameters out of range"));
        }
        if !prob(self.posting.missing_authorized_prob) {
            return Err(E("posting missing_authorized_prob out of range"));
        }
        match self.pending {
            PendingModel::Gaussian { rel_sd, min_sd }
                if !(variance(rel_sd) && variance(min_sd)) =>
            {
                Err(E(
                    "pending rel_sd and min_sd must be non-negative and finite",
                ))
            }
            PendingModel::Realistic {
                tip_prob,
                max_tip_frac,
                hold_prob,
                hold_amount,
            } if !(prob(tip_prob)
                && prob(hold_prob)
                && variance(max_tip_frac)
                && variance(hold_amount)) =>
            {
                Err(E("pending tip/hold parameters out of range"))
            }
            _ => Ok(()),
        }
    }
}

/// One transaction's full lifecycle: authorization, pending period, posting.
#[derive(Debug, Clone, PartialEq)]
pub struct SynthEvent {
    pub account: AccountId,
    pub authorized_day: Day,
    /// Always a business day, `>= authorized_day`. May fall after the horizon.
    pub posted_day: Day,
    /// Signed, inflows positive. This is what the truth balance uses.
    pub final_amount: f64,
    /// What the feed shows while the transaction is pending.
    pub pending_amount: f64,
    pub kind: FlowKind,
    pub stream: Option<StreamId>,
    /// The posted record omits its authorized date.
    pub authorized_date_missing: bool,
    /// Whether the feed shows a pending record before posting. Incoming
    /// transfer legs often appear only once posted.
    pub visible_while_pending: bool,
    /// For a transfer leg, the index in `SynthRun::events` of the other leg.
    pub counterpart: Option<usize>,
    pub pending_id: TxnId,
    pub posted_id: TxnId,
}

impl SynthEvent {
    /// Whether an end-of-day snapshot ever shows a pending record for this event.
    pub fn shows_pending(&self) -> bool {
        self.visible_while_pending && self.posted_day > self.authorized_day
    }

    /// The record visible at the end of `as_of`, if any.
    fn view(&self, as_of: Day) -> Option<Transaction> {
        if self.authorized_day > as_of {
            return None;
        }
        let base = Transaction {
            id: self.pending_id,
            account: self.account,
            date: self.authorized_day,
            authorized_date: Some(self.authorized_day),
            status: TxnStatus::Pending,
            amount: self.pending_amount,
            kind: self.kind,
            stream: self.stream,
            pending_id: None,
        };
        if self.posted_day > as_of {
            return self.visible_while_pending.then_some(base);
        }
        Some(Transaction {
            id: self.posted_id,
            date: self.posted_day,
            authorized_date: (!self.authorized_date_missing).then_some(self.authorized_day),
            status: TxnStatus::Posted,
            amount: self.final_amount,
            pending_id: self.shows_pending().then_some(self.pending_id),
            ..base
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SynthRun {
    /// State at the end of `start_day − 1`.
    pub initial: TrueState,
    pub start_day: Day,
    /// One entry per simulated day, in order.
    pub truth: Vec<TruthDay>,
    /// All transactions authorized in the horizon, in order of authorization.
    pub events: Vec<SynthEvent>,
}

impl SynthRun {
    /// Ground truth at the end of `day`, if it is inside the horizon.
    pub fn truth_on(&self, day: Day) -> Option<&TrueState> {
        let i = usize::try_from(day.checked_sub(self.start_day)?).ok()?;
        self.truth.get(i).map(|t| &t.state)
    }

    /// The transaction records a bank feed shows at the end of `as_of`.
    pub fn snapshot(&self, as_of: Day) -> Vec<Transaction> {
        self.events
            .iter()
            .take_while(|e| e.authorized_day <= as_of)
            .filter_map(|e| e.view(as_of))
            .collect()
    }

    /// The bank's posted (ledger) balance across all accounts at the end of `as_of`.
    pub fn ledger_balance(&self, as_of: Day) -> f64 {
        self.initial.b
            + self
                .events
                .iter()
                .filter(|e| e.posted_day <= as_of)
                .map(|e| e.final_amount)
                .sum::<f64>()
    }
}

/// Generate a run. Identical `(cfg, seed)` always produce identical output.
pub fn generate(cfg: &SynthConfig, seed: u64) -> Result<SynthRun, EngineError> {
    cfg.validate()?;
    let end = cfg.start_day + cfg.n_days as Day;

    let mut state_rng = stream_rng(seed, RNG_STATE);
    let mut spend_rng = stream_rng(seed, RNG_SPEND);
    let mut income_rng = stream_rng(seed, RNG_INCOME);
    let mut transfer_rng = stream_rng(seed, RNG_TRANSFER);
    let mut sub_rngs: Vec<ChaCha8Rng> = (0..cfg.subscriptions.len())
        .map(|i| stream_rng(seed, RNG_SUBSCRIPTION_BASE + i as u64))
        .collect();

    let paydays = paydays(&cfg.income, cfg.start_day, end, &mut income_rng);
    let charge_days: Vec<Vec<Day>> = cfg
        .subscriptions
        .iter()
        .zip(&mut sub_rngs)
        .map(|(sub, rng)| charge_days(sub, cfg.start_day, end, rng))
        .collect();
    let poisson = match cfg.spend {
        SpendModel::PoissonLogNormal { mean_count, .. } => Some(
            Poisson::new(mean_count)
                .map_err(|_| EngineError::InvalidConfig("spend mean_count rejected"))?,
        ),
        SpendModel::Gaussian { .. } => None,
    };
    let factor_mean = cfg.weekday_factors.iter().sum::<f64>() / 7.0;

    let mut events = Vec::new();
    let mut truth = Vec::with_capacity(cfg.n_days as usize);
    let mut state = cfg.initial;
    let mut next_payday = 0;
    let mut next_charge = vec![0; cfg.subscriptions.len()];

    for day in cfg.start_day..end {
        let first_event = events.len();

        let factor = cfg.weekday_factors[weekday(day) as usize] / factor_mean;
        for amount in discretionary(
            cfg.spend,
            poisson.as_ref(),
            state.c * factor,
            &mut spend_rng,
        ) {
            let e = card_event(
                cfg,
                &events,
                day,
                amount,
                FlowKind::Discretionary,
                None,
                &mut spend_rng,
            );
            events.push(e);
        }

        state.c += normal(&mut state_rng, 0.0, cfg.q_c.sqrt());
        state.m += normal(&mut state_rng, 0.0, cfg.q_m.sqrt());

        if paydays.get(next_payday) == Some(&day) {
            next_payday += 1;
            let amount = normal(&mut income_rng, state.m, cfg.income.amount_sd);
            let missing = income_rng.random_bool(cfg.posting.missing_authorized_prob);
            let (pending_id, posted_id) = next_ids(&events);
            events.push(SynthEvent {
                account: CHECKING,
                authorized_day: day,
                posted_day: day,
                final_amount: amount,
                pending_amount: amount,
                kind: FlowKind::Income,
                stream: Some(INCOME_STREAM),
                authorized_date_missing: missing,
                visible_while_pending: true,
                counterpart: None,
                pending_id,
                posted_id,
            });
        }

        if transfer_rng.random_bool(cfg.transfers.daily_prob) {
            push_transfer(cfg, &mut events, day, &mut transfer_rng);
        }

        for (i, sub) in cfg.subscriptions.iter().enumerate() {
            if charge_days[i].get(next_charge[i]) == Some(&day) {
                next_charge[i] += 1;
                let stream = Some(subscription_stream(i));
                let e = card_event(
                    cfg,
                    &events,
                    day,
                    -sub.amount,
                    FlowKind::Scheduled,
                    stream,
                    &mut sub_rngs[i],
                );
                events.push(e);
            }
        }

        state.b += events[first_event..]
            .iter()
            .map(|e| e.final_amount)
            .sum::<f64>();
        truth.push(TruthDay { day, state });
    }

    Ok(SynthRun {
        initial: cfg.initial,
        start_day: cfg.start_day,
        truth,
        events,
    })
}

fn stream_rng(seed: u64, stream: u64) -> ChaCha8Rng {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    rng.set_stream(stream);
    rng
}

fn normal(rng: &mut ChaCha8Rng, mean: f64, sd: f64) -> f64 {
    let z: f64 = rng.sample(StandardNormal);
    mean + sd * z
}

/// Pending and posted ids for the next event: `2i` and `2i + 1`.
fn next_ids(events: &[SynthEvent]) -> (TxnId, TxnId) {
    let i = events.len() as u64;
    (TxnId(2 * i), TxnId(2 * i + 1))
}

/// Signed (negative) purchase amounts for one day, with mean total `mean`.
fn discretionary(
    model: SpendModel,
    poisson: Option<&Poisson<f64>>,
    mean: f64,
    rng: &mut ChaCha8Rng,
) -> Vec<f64> {
    match (model, poisson) {
        (SpendModel::Gaussian { sigma }, _) => vec![-normal(rng, mean, sigma)],
        (
            SpendModel::PoissonLogNormal {
                mean_count,
                sigma_log,
            },
            Some(poisson),
        ) => {
            let count = poisson.sample(rng) as u32;
            let mean_amount = mean.max(0.0) / mean_count;
            if mean_amount == 0.0 {
                return Vec::new();
            }
            // E[exp(mu + s·Z)] = exp(mu + s²/2) = mean_amount.
            let mu = mean_amount.ln() - sigma_log * sigma_log / 2.0;
            (0..count)
                .map(|_| -normal(rng, mu, sigma_log).exp())
                .collect()
        }
        (SpendModel::PoissonLogNormal { .. }, None) => Vec::new(),
    }
}

/// A card transaction on checking authorized on `day`, with a posting lag and
/// a pending amount.
fn card_event(
    cfg: &SynthConfig,
    events: &[SynthEvent],
    day: Day,
    final_amount: f64,
    kind: FlowKind,
    stream: Option<StreamId>,
    rng: &mut ChaCha8Rng,
) -> SynthEvent {
    let lag = rng.random_range(0..=cfg.posting.max_lag_business_days);
    let pending_amount = pending_amount(cfg.pending, kind, final_amount, rng);
    let missing = rng.random_bool(cfg.posting.missing_authorized_prob);
    let (pending_id, posted_id) = next_ids(events);
    SynthEvent {
        account: CHECKING,
        authorized_day: day,
        posted_day: add_business_days(day, lag),
        final_amount,
        pending_amount,
        kind,
        stream,
        authorized_date_missing: missing,
        visible_while_pending: true,
        counterpart: None,
        pending_id,
        posted_id,
    }
}

/// Both legs of an own-account transfer. Amounts are exact negatives, so the
/// pair adds exactly zero to the aggregated balance.
fn push_transfer(cfg: &SynthConfig, events: &mut Vec<SynthEvent>, day: Day, rng: &mut ChaCha8Rng) {
    let t = &cfg.transfers;
    let amount = t.mean_amount * (0.5 + rng.random::<f64>());
    let (from, to) = if rng.random_bool(t.to_savings_prob) {
        (CHECKING, SAVINGS)
    } else {
        (SAVINGS, CHECKING)
    };
    let out_lag = rng.random_range(0..=1u32);
    let in_lag = out_lag + rng.random_range(0..=1u32);
    let p_missing = cfg.posting.missing_authorized_prob;
    let out_idx = events.len();
    for (account, signed, lag, visible) in
        [(from, -amount, out_lag, true), (to, amount, in_lag, false)]
    {
        let missing = rng.random_bool(p_missing);
        let (pending_id, posted_id) = next_ids(events);
        let idx = events.len();
        events.push(SynthEvent {
            account,
            authorized_day: day,
            posted_day: add_business_days(day, lag),
            final_amount: signed,
            pending_amount: signed,
            kind: FlowKind::Transfer,
            stream: None,
            authorized_date_missing: missing,
            visible_while_pending: visible,
            counterpart: Some(if idx == out_idx { out_idx + 1 } else { out_idx }),
            pending_id,
            posted_id,
        });
    }
}

fn pending_amount(
    model: PendingModel,
    kind: FlowKind,
    final_amount: f64,
    rng: &mut ChaCha8Rng,
) -> f64 {
    match model {
        PendingModel::Gaussian { rel_sd, min_sd } => {
            final_amount + normal(rng, 0.0, (rel_sd * final_amount.abs()).max(min_sd))
        }
        PendingModel::Realistic {
            tip_prob,
            max_tip_frac,
            hold_prob,
            hold_amount,
        } => {
            if kind != FlowKind::Discretionary {
                final_amount
            } else if rng.random_bool(hold_prob) {
                -hold_amount
            } else if rng.random_bool(tip_prob) {
                let tip: f64 = max_tip_frac * rng.random::<f64>();
                final_amount / (1.0 + tip)
            } else {
                final_amount
            }
        }
    }
}

/// Actual paydays in `[start, end)`: nominal schedule dates plus jitter, moved
/// back to the previous business day when they land on a weekend or holiday.
fn paydays(cfg: &IncomeConfig, start: Day, end: Day, rng: &mut ChaCha8Rng) -> Vec<Day> {
    let j = cfg.max_jitter_days as Day;
    let mut days = Vec::new();
    for nominal in cfg
        .schedule
        .nominal_dates(start - j, end + j + MAX_PAY_ROLLBACK as Day)
    {
        let mut offset = 0;
        if cfg.max_jitter_days > 0 && rng.random_bool(cfg.jitter_prob) {
            let k = rng.random_range(1..=cfg.max_jitter_days) as Day;
            offset = if rng.random_bool(0.5) { -k } else { k };
        }
        let actual = PaySchedule::pay_date(nominal + offset);
        if (start..end).contains(&actual) {
            days.push(actual);
        }
    }
    debug_assert!(days.windows(2).all(|w| w[0] < w[1]));
    days
}

/// Charge days in `[start, end)`. Each slip shifts all later charges.
fn charge_days(cfg: &SubscriptionConfig, start: Day, end: Day, rng: &mut ChaCha8Rng) -> Vec<Day> {
    let mut days = Vec::new();
    let mut day = cfg.first_day;
    while day < end {
        if day >= start {
            days.push(day);
        }
        let mut drift = 0;
        if rng.random_bool(cfg.drift_prob) {
            drift = if rng.random_bool(0.5) { -1 } else { 1 };
        }
        day += cfg.period_days as Day + drift;
    }
    days
}
