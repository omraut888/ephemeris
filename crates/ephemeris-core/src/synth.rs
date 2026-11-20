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
//! A [`StepChange`] multiplies `c` or `m` at the start of its day, before that
//! day's spending and paycheck are drawn; it consumes no randomness, so every
//! other draw is unchanged (design §6).
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
/// Display name of the primary income stream.
pub const INCOME_NAME: &str = "Payroll";

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
    /// Display name for the stream, as a bank feed's merchant name would be.
    pub name: &'static str,
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

/// A regime change: from `day` on, a state variable is multiplied by `factor`
/// (design §6). The variable keeps its random walk afterwards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepChange {
    /// First day that sees the new level.
    pub day: Day,
    pub factor: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SynthConfig {
    pub start_day: Day,
    pub n_days: u32,
    /// State at the end of `start_day − 1`.
    pub initial: TrueState,
    /// Daily random-walk variance of `c`.
    pub q_c: f64,
    /// If set, the true `c` reflects at this floor, as the forecast's does
    /// with a spend floor (design §7). Reflection draws nothing, so a floor
    /// the walk never reaches leaves the data unchanged.
    pub c_floor: Option<f64>,
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
    /// Steps in the spend rate `c` (a move, a new habit).
    pub spend_steps: Vec<StepChange>,
    /// Steps in the expected paycheck `m` (a raise, a pay cut).
    pub income_steps: Vec<StepChange>,
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

    /// Realistic data whose spend rate doubles halfway through the year
    /// (design §5, the `regime_change` scenario).
    pub fn regime_change(start_day: Day) -> Self {
        Self {
            spend_steps: vec![StepChange {
                day: start_day + 182,
                factor: 2.0,
            }],
            ..Self::realistic(start_day)
        }
    }

    fn base(start_day: Day) -> Self {
        let first_friday = (start_day..)
            .find(|&d| weekday(d) == 4)
            .unwrap_or(start_day);
        let sub = |name, amount, offset, drift_prob| SubscriptionConfig {
            name,
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
            c_floor: None,
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
                sub("Rent", 1500.0, 0, 0.05),
                sub("Streaming", 15.99, 4, 0.2),
                sub("Phone", 40.0, 11, 0.2),
                sub("Gym", 65.0, 19, 0.1),
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
            spend_steps: Vec::new(),
            income_steps: Vec::new(),
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
        if self.c_floor.is_some_and(|f| !finite(f)) {
            return Err(E("c_floor must be finite"));
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
        if !self
            .spend_steps
            .iter()
            .chain(&self.income_steps)
            .all(|s| variance(s.factor))
        {
            return Err(E("step factors must be non-negative and finite"));
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
        state.c *= step_factor(&cfg.spend_steps, day);
        state.m *= step_factor(&cfg.income_steps, day);

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
        if let Some(floor) = cfg.c_floor
            && state.c < floor
        {
            state.c = 2.0 * floor - state.c;
        }
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

/// The product of the factors of the steps that take effect on `day`.
fn step_factor(steps: &[StepChange], day: Day) -> f64 {
    steps
        .iter()
        .filter(|s| s.day == day)
        .map(|s| s.factor)
        .product()
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
            // E[exp(mu + s·Z)] = exp(mu + s²/2) = mean_amount. `libm`, not
            // std, so the generator is bit-identical natively and in WASM.
            let mu = libm::log(mean_amount) - sigma_log * sigma_log / 2.0;
            (0..count)
                .map(|_| -libm::exp(normal(rng, mu, sigma_log)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::{business_day_on_or_after, day_from_civil, is_business_day};
    use std::collections::HashSet;

    /// 2024-01-01, a Monday.
    const MON: Day = 19_723;

    fn mean_sd(xs: &[f64]) -> (f64, f64) {
        let n = xs.len() as f64;
        let mean = xs.iter().sum::<f64>() / n;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
        (mean, var.sqrt())
    }

    fn of_kind(run: &SynthRun, kind: FlowKind) -> Vec<&SynthEvent> {
        run.events.iter().filter(|e| e.kind == kind).collect()
    }

    fn pay_days(run: &SynthRun) -> Vec<Day> {
        of_kind(run, FlowKind::Income)
            .iter()
            .map(|e| e.authorized_day)
            .collect()
    }

    /// Model-matched config with frozen `c`, `m` so sample moments are easy to check.
    fn stationary(n_days: u32) -> SynthConfig {
        SynthConfig {
            n_days,
            q_c: 0.0,
            q_m: 0.0,
            ..SynthConfig::model_matched(MON)
        }
    }

    #[test]
    fn same_seed_same_output() {
        for cfg in [SynthConfig::model_matched(MON), SynthConfig::realistic(MON)] {
            let a = generate(&cfg, 42).unwrap();
            let b = generate(&cfg, 42).unwrap();
            assert_eq!(a, b);
            // Bitwise, not just approximately.
            for (x, y) in a.truth.iter().zip(&b.truth) {
                assert_eq!(x.state.b.to_bits(), y.state.b.to_bits());
            }
        }
    }

    #[test]
    fn c_floor_reflects_the_truth_and_draws_nothing() {
        // c starts at 1 with step sd 2: unfloored it goes negative quickly.
        let base = SynthConfig {
            n_days: 200,
            q_c: 4.0,
            initial: TrueState {
                c: 1.0,
                ..SynthConfig::model_matched(MON).initial
            },
            ..SynthConfig::model_matched(MON)
        };
        let free = generate(&base, 3).unwrap();
        assert!(free.truth.iter().any(|t| t.state.c < 0.0));
        let floored = generate(
            &SynthConfig {
                c_floor: Some(0.0),
                ..base.clone()
            },
            3,
        )
        .unwrap();
        assert!(floored.truth.iter().all(|t| t.state.c >= 0.0));
        // Same draws: until the first crossing the two runs agree exactly.
        let first = free.truth.iter().position(|t| t.state.c < 0.0).unwrap();
        assert_eq!(free.truth[..first], floored.truth[..first]);
        // A floor the walk never reaches changes nothing at all.
        let cfg = SynthConfig::realistic(MON);
        let far = SynthConfig {
            c_floor: Some(-1e9),
            ..cfg.clone()
        };
        assert_eq!(generate(&cfg, 5).unwrap(), generate(&far, 5).unwrap());
    }

    #[test]
    fn different_seed_different_output() {
        let cfg = SynthConfig::realistic(MON);
        assert_ne!(generate(&cfg, 1).unwrap(), generate(&cfg, 2).unwrap());
    }

    #[test]
    fn components_draw_from_independent_streams() {
        let with = SynthConfig::realistic(MON);
        let without = SynthConfig {
            subscriptions: Vec::new(),
            ..with.clone()
        };
        let a = generate(&with, 7).unwrap();
        let b = generate(&without, 7).unwrap();
        let strip = |run: &SynthRun| -> Vec<(Day, Day, u64, u64)> {
            run.events
                .iter()
                .filter(|e| e.kind != FlowKind::Scheduled)
                .map(|e| {
                    (
                        e.authorized_day,
                        e.posted_day,
                        e.final_amount.to_bits(),
                        e.pending_amount.to_bits(),
                    )
                })
                .collect()
        };
        assert_eq!(strip(&a), strip(&b));
        let c_a: Vec<f64> = a.truth.iter().map(|t| t.state.c).collect();
        let c_b: Vec<f64> = b.truth.iter().map(|t| t.state.c).collect();
        assert_eq!(c_a, c_b);
    }

    /// Paychecks and `m` depend only on the state and income streams, so
    /// switching spend, pending, posting, or weekday settings leaves them
    /// bit-identical for the same seed.
    #[test]
    fn income_is_identical_across_modes() {
        for seed in 0..5 {
            let a = generate(&SynthConfig::model_matched(MON), seed).unwrap();
            let b = generate(&SynthConfig::realistic(MON), seed).unwrap();
            let pays = |run: &SynthRun| -> Vec<(Day, u64)> {
                of_kind(run, FlowKind::Income)
                    .iter()
                    .map(|e| (e.authorized_day, e.final_amount.to_bits()))
                    .collect()
            };
            assert_eq!(pays(&a), pays(&b));
            let m = |run: &SynthRun| -> Vec<u64> {
                run.truth.iter().map(|t| t.state.m.to_bits()).collect()
            };
            assert_eq!(m(&a), m(&b));
        }
    }

    #[test]
    fn truth_balance_is_cumulative_final_amounts() {
        let run = generate(&SynthConfig::realistic(MON), 3).unwrap();
        assert_eq!(run.truth.len(), 365);
        let mut prev = run.initial.b;
        for t in &run.truth {
            let flow: f64 = run
                .events
                .iter()
                .filter(|e| e.authorized_day == t.day)
                .map(|e| e.final_amount)
                .sum();
            assert!((t.state.b - (prev + flow)).abs() < 1e-6, "day {}", t.day);
            prev = t.state.b;
        }
        assert_eq!(run.truth_on(MON).unwrap(), &run.truth[0].state);
        assert!(run.truth_on(MON - 1).is_none());
        assert!(run.truth_on(MON + 365).is_none());
    }

    #[test]
    fn zero_process_noise_freezes_c_and_m() {
        let run = generate(&stationary(200), 5).unwrap();
        assert!(
            run.truth
                .iter()
                .all(|t| t.state.c == 60.0 && t.state.m == 2400.0)
        );
    }

    #[test]
    fn step_changes_multiply_c_and_m_from_their_day() {
        let step = MON + 100;
        let cfg = SynthConfig {
            spend_steps: vec![StepChange {
                day: step,
                factor: 2.0,
            }],
            income_steps: vec![StepChange {
                day: step,
                factor: 0.8,
            }],
            ..stationary(200)
        };
        let run = generate(&cfg, 5).unwrap();
        for t in &run.truth {
            let (c, m) = if t.day < step {
                (60.0, 2400.0)
            } else {
                (120.0, 1920.0)
            };
            assert_eq!((t.state.c, t.state.m), (c, m), "day {}", t.day);
        }

        // Spending on the step day already uses the new rate, and nothing
        // before the step changes: the steps consume no randomness.
        let plain = generate(&stationary(200), 5).unwrap();
        let disc = |run: &SynthRun, day: Day| -> f64 {
            run.events
                .iter()
                .filter(|e| e.kind == FlowKind::Discretionary && e.authorized_day == day)
                .map(|e| e.final_amount)
                .sum()
        };
        assert!((disc(&run, step) - disc(&plain, step) + 60.0).abs() < 1e-9);
        let before = |run: &SynthRun| -> Vec<SynthEvent> {
            run.events
                .iter()
                .filter(|e| e.authorized_day < step)
                .cloned()
                .collect()
        };
        assert_eq!(before(&run), before(&plain));
        let pay_after: Vec<f64> = of_kind(&run, FlowKind::Income)
            .iter()
            .filter(|e| e.authorized_day >= step)
            .map(|e| e.final_amount)
            .collect();
        let (mean, _) = mean_sd(&pay_after);
        assert!((mean - 1920.0).abs() < 40.0, "mean paycheck {mean}");
    }

    #[test]
    fn regime_change_scenario_doubles_spending() {
        let cfg = SynthConfig::regime_change(MON);
        let step = cfg.spend_steps[0].day;
        let run = generate(&cfg, 1).unwrap();
        let c = |d: Day| run.truth_on(d).unwrap().c;
        // c keeps its random walk; one day's step dwarfs one day's drift.
        assert!((c(step) / c(step - 1) - 2.0).abs() < 0.05);
    }

    #[test]
    fn paychecks_follow_jittered_biweekly_schedule() {
        let cfg = SynthConfig {
            n_days: 730,
            ..SynthConfig::realistic(MON)
        };
        let PaySchedule::Biweekly { anchor } = cfg.income.schedule else {
            unreachable!()
        };
        let run = generate(&cfg, 11).unwrap();
        let pays = of_kind(&run, FlowKind::Income);
        assert!(pays.len() >= 50);
        let mut off_schedule = 0;
        for p in &pays {
            assert!(is_business_day(p.authorized_day));
            assert_eq!(
                p.posted_day, p.authorized_day,
                "direct deposit posts same day"
            );
            assert_eq!(p.stream, Some(INCOME_STREAM));
            assert_eq!(p.account, CHECKING);
            // Explained by some nominal date, a jitter of at most 1, and the
            // business-day rollback (e.g. jittered onto Juneteenth → Wednesday).
            let explained = cfg
                .income
                .schedule
                .nominal_dates(p.authorized_day - 1, p.authorized_day + 4)
                .into_iter()
                .any(|n| (-1..=1).any(|j| PaySchedule::pay_date(n + j) == p.authorized_day));
            assert!(explained, "payday {}", p.authorized_day);
            let phase = (p.authorized_day - anchor).rem_euclid(14);
            off_schedule += usize::from(phase != 0);
        }
        assert!(off_schedule > 0, "jitter_prob 0.3 should move some paydays");
        for w in pays.windows(2) {
            let gap = w[1].authorized_day - w[0].authorized_day;
            assert!((12..=16).contains(&gap), "gap {gap}");
        }
    }

    #[test]
    fn no_jitter_pays_on_schedule_shifted_to_business_days() {
        let schedules = [
            PaySchedule::Weekly { anchor: MON + 4 },
            PaySchedule::Biweekly { anchor: MON + 4 },
            PaySchedule::SemiMonthly { days: [1, 15] },
            PaySchedule::SemiMonthly { days: [15, 31] },
            PaySchedule::Monthly { day: 1 },
        ];
        for schedule in schedules {
            let mut cfg = SynthConfig::realistic(MON);
            cfg.income.schedule = schedule;
            cfg.income.jitter_prob = 0.0;
            let run = generate(&cfg, 11).unwrap();
            let expected: Vec<Day> = schedule
                .nominal_dates(MON, MON + 364 + 3)
                .into_iter()
                .map(PaySchedule::pay_date)
                .filter(|&d| (MON..MON + 365).contains(&d))
                .collect();
            assert_eq!(pay_days(&run), expected, "{schedule:?}");
            assert!(pay_days(&run).iter().all(|&d| is_business_day(d)));
        }
    }

    #[test]
    fn semi_monthly_weekend_and_holiday_shifts() {
        let mut cfg = SynthConfig::realistic(MON);
        cfg.income.schedule = PaySchedule::SemiMonthly { days: [1, 15] };
        cfg.income.jitter_prob = 0.0;
        let pays = pay_days(&generate(&cfg, 0).unwrap());
        let ymd = day_from_civil;
        // 2024-06-15 is a Saturday: paid Friday the 14th.
        assert!(pays.contains(&ymd(2024, 6, 14)) && !pays.contains(&ymd(2024, 6, 15)));
        // 2024-09-01 is a Sunday before Labor Day: paid Friday 2024-08-30.
        assert!(pays.contains(&ymd(2024, 8, 30)));
        // 2024-01-01 is New Year's Day: paid the previous Friday, outside the horizon.
        // 2024-01-15 is MLK Day: paid Friday the 12th.
        assert!(!pays.contains(&ymd(2024, 1, 1)));
        assert_eq!(pays[0], ymd(2024, 1, 12));
    }

    #[test]
    fn subscription_dates_drift_as_a_random_walk() {
        let mut cfg = SynthConfig {
            n_days: 3650,
            ..SynthConfig::realistic(MON)
        };
        cfg.subscriptions = vec![
            SubscriptionConfig {
                name: "A",
                amount: 10.0,
                first_day: MON,
                period_days: 30,
                drift_prob: 0.0,
            },
            SubscriptionConfig {
                name: "B",
                amount: 20.0,
                first_day: MON,
                period_days: 30,
                drift_prob: 1.0,
            },
        ];
        let run = generate(&cfg, 9).unwrap();
        let days = |i: usize| -> Vec<Day> {
            run.events
                .iter()
                .filter(|e| e.stream == Some(subscription_stream(i)))
                .map(|e| e.authorized_day)
                .collect()
        };
        let fixed = days(0);
        assert!(fixed.windows(2).all(|w| w[1] - w[0] == 30));

        let drifting = days(1);
        assert!(drifting.len() > 100);
        assert!(
            drifting
                .windows(2)
                .all(|w| (29..=31).contains(&(w[1] - w[0])))
        );
        // With a slip on every charge, the phase must wander from the fixed schedule.
        let max_drift = drifting
            .iter()
            .enumerate()
            .map(|(k, &d)| (d - (MON + 30 * k as Day)).abs())
            .max()
            .unwrap();
        assert!(max_drift >= 3, "max drift {max_drift}");

        for e in of_kind(&run, FlowKind::Scheduled) {
            assert!(e.final_amount < 0.0);
        }
    }

    #[test]
    fn posting_lags_are_business_days_within_bound() {
        let run = generate(&SynthConfig::realistic(MON), 4).unwrap();
        let mut lagged = 0;
        for e in &run.events {
            assert!(e.posted_day >= e.authorized_day);
            assert!(is_business_day(e.posted_day));
            // Lag is counted from the first business day on or after authorization.
            let first = business_day_on_or_after(e.authorized_day);
            let lag = (first + 1..=e.posted_day)
                .filter(|&d| is_business_day(d))
                .count();
            assert!(lag <= 3, "{e:?}");
            lagged += usize::from(e.shows_pending());
        }
        assert!(lagged > run.events.len() / 2);
    }

    #[test]
    fn transfers_net_to_zero() {
        for cfg in [SynthConfig::model_matched(MON), SynthConfig::realistic(MON)] {
            let run = generate(&cfg, 31).unwrap();
            let legs = of_kind(&run, FlowKind::Transfer);
            assert!(legs.len() >= 20, "{} legs", legs.len());
            for (i, e) in run.events.iter().enumerate() {
                if e.kind != FlowKind::Transfer {
                    assert_eq!(e.counterpart, None);
                    continue;
                }
                let other = &run.events[e.counterpart.unwrap()];
                assert_eq!(other.counterpart, Some(i));
                assert_eq!(e.final_amount + other.final_amount, 0.0);
                assert_eq!(e.pending_amount, e.final_amount);
                assert_eq!(e.authorized_day, other.authorized_day);
                assert_ne!(e.account, other.account);
                assert_eq!(e.stream, None);
                if e.final_amount > 0.0 {
                    assert!(!e.visible_while_pending);
                    assert!(e.posted_day >= other.posted_day);
                }
            }
        }
    }

    #[test]
    fn transfers_never_change_spending_income_or_truth() {
        let with = SynthConfig::realistic(MON);
        let mut without = with.clone();
        without.transfers.daily_prob = 0.0;
        let a = generate(&with, 32).unwrap();
        let b = generate(&without, 32).unwrap();
        let non_transfer = |run: &SynthRun| -> Vec<(Day, u64, FlowKind)> {
            run.events
                .iter()
                .filter(|e| e.kind != FlowKind::Transfer)
                .map(|e| (e.authorized_day, e.final_amount.to_bits(), e.kind))
                .collect()
        };
        assert_eq!(non_transfer(&a), non_transfer(&b));
        for (x, y) in a.truth.iter().zip(&b.truth) {
            assert_eq!((x.state.c, x.state.m), (y.state.c, y.state.m));
            assert!((x.state.b - y.state.b).abs() < 1e-6);
        }
    }

    #[test]
    fn incoming_transfer_leg_is_invisible_until_posted() {
        let run = generate(&SynthConfig::model_matched(MON), 33).unwrap();
        let (out, inc) = run
            .events
            .iter()
            .filter(|e| e.kind == FlowKind::Transfer && e.final_amount < 0.0)
            .map(|e| (e, &run.events[e.counterpart.unwrap()]))
            .find(|(_, inc)| inc.posted_day > inc.authorized_day)
            .expect("some transfer with a lagged incoming leg");
        let snap = run.snapshot(out.authorized_day);
        let ids: HashSet<TxnId> = snap.iter().map(|t| t.id).collect();
        assert!(ids.contains(&out.pending_id) || ids.contains(&out.posted_id));
        assert!(!ids.contains(&inc.pending_id) && !ids.contains(&inc.posted_id));

        let posted = run.snapshot(inc.posted_day);
        let t = posted.iter().find(|t| t.id == inc.posted_id).unwrap();
        assert_eq!(t.kind, FlowKind::Transfer);
        assert_eq!(t.pending_id, None);
    }

    #[test]
    fn snapshot_shows_pending_then_posted() {
        let run = generate(&SynthConfig::model_matched(MON), 8).unwrap();
        let (idx, e) = run
            .events
            .iter()
            .enumerate()
            .find(|(_, e)| {
                e.kind == FlowKind::Discretionary && e.posted_day - e.authorized_day >= 2
            })
            .unwrap();

        let find = |as_of: Day, id: TxnId| run.snapshot(as_of).into_iter().find(|t| t.id == id);

        // Not visible before authorization.
        assert!(run.snapshot(e.authorized_day - 1).len() <= idx);
        assert!(find(e.authorized_day - 1, e.pending_id).is_none());

        let pending = find(e.authorized_day, e.pending_id).unwrap();
        assert_eq!(pending.status, TxnStatus::Pending);
        assert_eq!(pending.amount, e.pending_amount);
        assert_eq!(pending.date, e.authorized_day);
        assert_eq!(pending.pending_id, None);
        assert!(find(e.posted_day - 1, e.pending_id).is_some());

        let snap = run.snapshot(e.posted_day);
        assert!(snap.iter().all(|t| t.id != e.pending_id));
        let posted = snap.iter().find(|t| t.id == e.posted_id).unwrap();
        assert_eq!(posted.status, TxnStatus::Posted);
        assert_eq!(posted.amount, e.final_amount);
        assert_eq!(posted.date, e.posted_day);
        assert_eq!(posted.authorized_date, Some(e.authorized_day));
        assert_eq!(posted.pending_id, Some(e.pending_id));
    }

    #[test]
    fn snapshot_records_are_valid_and_unique() {
        let run = generate(&SynthConfig::realistic(MON), 12).unwrap();
        for as_of in [MON, MON + 100, MON + 364, MON + 400] {
            let snap = run.snapshot(as_of);
            let ids: HashSet<TxnId> = snap.iter().map(|t| t.id).collect();
            assert_eq!(ids.len(), snap.len());
            for t in &snap {
                assert_eq!(t.validate(), Ok(()));
                assert!(t.day() <= as_of);
                if t.status == TxnStatus::Pending {
                    assert!(t.authorized_date.is_some());
                }
            }
        }
    }

    #[test]
    fn ledger_equals_truth_minus_unposted() {
        let run = generate(&SynthConfig::realistic(MON), 13).unwrap();
        for t in &run.truth {
            let in_flight: f64 = run
                .events
                .iter()
                .filter(|e| e.authorized_day <= t.day && e.posted_day > t.day)
                .map(|e| e.final_amount)
                .sum();
            assert!((run.ledger_balance(t.day) - (t.state.b - in_flight)).abs() < 1e-6);
        }
    }

    #[test]
    fn missing_authorized_dates_only_on_posted_records() {
        let mut cfg = SynthConfig::realistic(MON);
        cfg.posting.missing_authorized_prob = 1.0;
        let run = generate(&cfg, 14).unwrap();
        let snap = run.snapshot(MON + 200);
        assert!(snap.iter().any(|t| t.status == TxnStatus::Pending));
        for t in &snap {
            match t.status {
                TxnStatus::Posted => assert_eq!(t.authorized_date, None),
                TxnStatus::Pending => assert!(t.authorized_date.is_some()),
            }
        }

        cfg.posting.missing_authorized_prob = 0.0;
        let run = generate(&cfg, 14).unwrap();
        assert!(
            run.snapshot(MON + 200)
                .iter()
                .all(|t| t.authorized_date.is_some())
        );
    }

    #[test]
    fn gaussian_spend_matches_model_moments() {
        let run = generate(&stationary(3650), 21).unwrap();
        let spend: Vec<f64> = of_kind(&run, FlowKind::Discretionary)
            .iter()
            .map(|e| -e.final_amount)
            .collect();
        assert_eq!(spend.len(), 3650, "one net transaction per day");
        let (mean, sd) = mean_sd(&spend);
        assert!((mean - 60.0).abs() < 3.0, "mean {mean}");
        assert!((sd - 40.0).abs() < 3.0, "sd {sd}");

        let pay: Vec<f64> = of_kind(&run, FlowKind::Income)
            .iter()
            .map(|e| e.final_amount)
            .collect();
        let (mean, sd) = mean_sd(&pay);
        assert!((mean - 2400.0).abs() < 15.0, "mean {mean}");
        assert!((sd - 50.0).abs() < 12.0, "sd {sd}");
    }

    #[test]
    fn gaussian_pending_noise_matches_design_r() {
        let run = generate(&stationary(3650), 22).unwrap();
        let z: Vec<f64> = run
            .events
            .iter()
            .filter(|e| matches!(e.kind, FlowKind::Discretionary | FlowKind::Scheduled))
            .map(|e| {
                let sd = (0.2 * e.final_amount.abs()).max(20.0);
                (e.pending_amount - e.final_amount) / sd
            })
            .collect();
        let (mean, sd) = mean_sd(&z);
        assert!(mean.abs() < 0.05, "mean {mean}");
        assert!((sd - 1.0).abs() < 0.05, "sd {sd}");
    }

    #[test]
    fn poisson_lognormal_spend_is_skewed_zero_inflated_and_weekly() {
        let cfg = SynthConfig {
            q_c: 0.0,
            q_m: 0.0,
            n_days: 3640, // whole weeks
            ..SynthConfig::realistic(MON)
        };
        let run = generate(&cfg, 23).unwrap();
        let mut daily = vec![0.0; 3640];
        for e in of_kind(&run, FlowKind::Discretionary) {
            assert!(e.final_amount < 0.0);
            daily[(e.authorized_day - MON) as usize] -= e.final_amount;
        }
        let (mean, _) = mean_sd(&daily);
        assert!((mean - 60.0).abs() < 3.0, "mean {mean}");
        let zero_frac = daily.iter().filter(|&&s| s == 0.0).count() as f64 / 3640.0;
        let expected = (-2.5f64).exp(); // P(Poisson(2.5) = 0)
        assert!(
            (zero_frac - expected).abs() < 0.02,
            "zero fraction {zero_frac}"
        );
        // Saturday (factor 1.35) vs Sunday (0.8); the factors already average 1.
        let by_weekday = |wd: usize| {
            let xs: Vec<f64> = daily.iter().skip(wd).step_by(7).copied().collect();
            mean_sd(&xs).0
        };
        let (sat, sun) = (by_weekday(5), by_weekday(6));
        assert!((sat - 81.0).abs() < 8.0, "saturday {sat}");
        assert!((sun - 48.0).abs() < 6.0, "sunday {sun}");
    }

    #[test]
    fn realistic_pending_has_tips_and_holds() {
        let run = generate(&SynthConfig::realistic(MON), 24).unwrap();
        let disc = of_kind(&run, FlowKind::Discretionary);
        let holds = disc.iter().filter(|e| e.pending_amount == -1.0).count();
        let tips = disc
            .iter()
            .filter(|e| e.pending_amount != -1.0 && e.pending_amount > e.final_amount)
            .count();
        assert!(holds > 0 && tips > 0);
        // A tip is never added at authorization: |pending| ≤ |final| unless it is a hold.
        assert!(
            disc.iter()
                .filter(|e| e.pending_amount != -1.0)
                .all(|e| e.pending_amount >= e.final_amount)
        );
        for e in of_kind(&run, FlowKind::Scheduled) {
            assert_eq!(e.pending_amount, e.final_amount);
        }
    }

    #[test]
    fn empty_horizon() {
        let run = generate(
            &SynthConfig {
                n_days: 0,
                ..SynthConfig::realistic(MON)
            },
            1,
        )
        .unwrap();
        assert!(run.truth.is_empty() && run.events.is_empty());
        assert_eq!(run.ledger_balance(MON), run.initial.b);
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let base = SynthConfig::model_matched(MON);
        let with_income = |f: fn(&mut IncomeConfig)| {
            let mut cfg = base.clone();
            f(&mut cfg.income);
            cfg
        };
        let bad: Vec<SynthConfig> = vec![
            SynthConfig {
                q_c: -1.0,
                ..base.clone()
            },
            SynthConfig {
                q_m: f64::NAN,
                ..base.clone()
            },
            SynthConfig {
                n_days: MAX_DAYS + 1,
                ..base.clone()
            },
            SynthConfig {
                spend: SpendModel::Gaussian { sigma: -1.0 },
                ..base.clone()
            },
            SynthConfig {
                spend: SpendModel::PoissonLogNormal {
                    mean_count: 0.0,
                    sigma_log: 1.0,
                },
                ..base.clone()
            },
            SynthConfig {
                weekday_factors: [0.0; 7],
                ..base.clone()
            },
            SynthConfig {
                weekday_factors: [1.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0],
                ..base.clone()
            },
            with_income(|i| i.jitter_prob = 1.5),
            with_income(|i| i.schedule = PaySchedule::Monthly { day: 0 }),
            // Weekly pay cannot absorb ±2 days of jitter plus a 3-day rollback.
            with_income(|i| {
                i.schedule = PaySchedule::Weekly { anchor: 0 };
                i.max_jitter_days = 2;
            }),
            SynthConfig {
                subscriptions: vec![SubscriptionConfig {
                    period_days: 2,
                    ..base.subscriptions[0]
                }],
                ..base.clone()
            },
            SynthConfig {
                transfers: TransferConfig {
                    daily_prob: 2.0,
                    ..base.transfers
                },
                ..base.clone()
            },
            SynthConfig {
                posting: PostingConfig {
                    missing_authorized_prob: -0.1,
                    ..base.posting
                },
                ..base.clone()
            },
            SynthConfig {
                pending: PendingModel::Gaussian {
                    rel_sd: 0.2,
                    min_sd: f64::INFINITY,
                },
                ..base.clone()
            },
            SynthConfig {
                spend_steps: vec![StepChange {
                    day: MON,
                    factor: -1.0,
                }],
                ..base.clone()
            },
            SynthConfig {
                income_steps: vec![StepChange {
                    day: MON,
                    factor: f64::NAN,
                }],
                ..base.clone()
            },
        ];
        for cfg in &bad {
            assert!(
                matches!(generate(cfg, 0), Err(EngineError::InvalidConfig(_))),
                "{cfg:?}"
            );
        }
        assert_eq!(base.validate(), Ok(()));
        assert_eq!(SynthConfig::realistic(MON).validate(), Ok(()));
        // Weekly pay with ±1 day of jitter is fine.
        assert_eq!(
            with_income(|i| i.schedule = PaySchedule::Weekly { anchor: 0 }).validate(),
            Ok(())
        );
    }
}
