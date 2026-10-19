//! From one bank-feed snapshot to per-day filter inputs (design §1, §2).
//!
//! Every record gets a [`Role`]:
//! - income, scheduled bills, and aged-out transfer legs are known flows (`u`);
//! - discretionary purchases are seen only through the balance observation;
//! - matched own-account transfer pairs are excluded (they cancel);
//! - a transfer leg whose counterpart has not appeared yet is held out, and
//!   blocks settlement until it is matched or ages out.
//!
//! Days up to `settled_through` (the day before the earliest pending or held
//! record) are final. Each gets an authorized-basis balance observation with
//! `R = r_post`, inflated near posted records that lack an authorized date.
//! The provisional tail after it gets one balance observation, at today,
//! including pending amounts with `R = r_post + Σ σ_j²` over pending purchases.
//! Pending known flows go into `u` at their pending amount; their variance is
//! reported separately as [`ObservedFeed::shared_variance`] because the same
//! error is in `z` too (design §2).

use std::collections::{BTreeMap, HashSet};

use crate::calendar::add_business_days;
use crate::error::EngineError;
use crate::types::{Day, FlowKind, Transaction, TxnStatus};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObserveConfig {
    /// Variance of a settled balance observation (fees, interest, rounding).
    pub r_post: f64,
    /// ρ in `σ_j = max(ρ·|a_j|, σ_min)` for a pending amount.
    pub pending_rel_sd: f64,
    /// σ_min, for hold-type authorizations.
    pub pending_min_sd: f64,
    /// ρ_lag: a posted record with no authorized date may belong up to
    /// `missing_auth_window` business days earlier; days in that window get
    /// `(ρ_lag·|a|)²` added to their balance R.
    pub missing_auth_rel_sd: f64,
    pub missing_auth_window: u32,
    /// Transfer legs match when their days are within this many business days.
    /// An unmatched leg older than this is reclassified as an external flow.
    pub transfer_window: u32,
}

impl Default for ObserveConfig {
    fn default() -> Self {
        Self {
            r_post: 1.0,
            pending_rel_sd: 0.2,
            pending_min_sd: 20.0,
            missing_auth_rel_sd: 0.5,
            missing_auth_window: 3,
            transfer_window: 3,
        }
    }
}

impl ObserveConfig {
    pub fn validate(&self) -> Result<(), EngineError> {
        let nonneg = |v: f64| v.is_finite() && v >= 0.0;
        if self.r_post.is_finite()
            && self.r_post > 0.0
            && nonneg(self.pending_rel_sd)
            && nonneg(self.pending_min_sd)
            && nonneg(self.missing_auth_rel_sd)
        {
            Ok(())
        } else {
            Err(EngineError::InvalidConfig(
                "observe: r_post must be positive, other variances non-negative",
            ))
        }
    }

    /// σ_j² for a pending amount.
    pub fn pending_var(&self, amount: f64) -> f64 {
        (self.pending_rel_sd * amount.abs())
            .max(self.pending_min_sd)
            .powi(2)
    }
}

/// How a record enters the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Known flow: part of `u` (income, scheduled, aged-out transfer leg).
    Input,
    /// Discretionary: seen only through the balance observation.
    Spend,
    /// One leg of a matched own-account transfer pair.
    MatchedTransfer,
    /// A transfer leg waiting for its counterpart.
    HeldTransfer,
}

/// One day of tracker input. Income amounts get their R from the adaptive
/// estimator, so they are plain amounts here.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedDay {
    pub day: Day,
    pub u: f64,
    /// Balance observation `(z, r)`, if any.
    pub balance: Option<(f64, f64)>,
    /// Posted primary-income amounts (observations of `m`).
    pub income: Vec<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct DayAgg {
    u_posted: f64,
    u_pending: f64,
    income: Vec<f64>,
}

/// A validated, classified snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedFeed {
    cfg: ObserveConfig,
    today: Day,
    settled_through: Day,
    opening_balance: f64,
    roles: Vec<Role>,
    days: BTreeMap<Day, DayAgg>,
    /// `(day, Σ posted counted amounts with day() <= day)`, ascending.
    posted_cum: Vec<(Day, f64)>,
    pending_total: f64,
    pending_spend_var: f64,
    shared_var: f64,
    /// `(posted day, (ρ_lag·a)²)` for posted counted records missing an
    /// authorized date, ascending by day.
    missing_auth: Vec<(Day, f64)>,
}

impl ObservedFeed {
    /// Classify `txns` (everything the feed shows at the end of `today`, all
    /// linked accounts) given the bank's posted balance across those accounts.
    pub fn new(
        txns: &[Transaction],
        ledger_balance: f64,
        today: Day,
        cfg: &ObserveConfig,
    ) -> Result<Self, EngineError> {
        cfg.validate()?;
        if !ledger_balance.is_finite() {
            return Err(EngineError::InvalidInput("ledger balance is not finite"));
        }
        let mut ids = HashSet::with_capacity(txns.len());
        for t in txns {
            t.validate()?;
            if !ids.insert(t.id) {
                return Err(EngineError::InvalidInput("duplicate transaction id"));
            }
            if t.day() > today || t.date > today {
                return Err(EngineError::InvalidInput("transaction dated after today"));
            }
        }

        let roles = assign_roles(txns, today, cfg);
        let posted_all: f64 = txns
            .iter()
            .filter(|t| t.status == TxnStatus::Posted)
            .map(|t| t.amount)
            .sum();

        let mut days: BTreeMap<Day, DayAgg> = BTreeMap::new();
        let mut posted_by_day: BTreeMap<Day, f64> = BTreeMap::new();
        let mut pending_total = 0.0;
        let mut pending_spend_var = 0.0;
        let mut shared_var = 0.0;
        let mut missing_auth = Vec::new();
        let mut settled_through = today;

        for (t, &role) in txns.iter().zip(&roles) {
            let pending = t.status == TxnStatus::Pending;
            match role {
                Role::MatchedTransfer => continue,
                Role::HeldTransfer => {
                    settled_through = settled_through.min(t.day() - 1);
                    continue;
                }
                Role::Input | Role::Spend => {}
            }
            if pending {
                settled_through = settled_through.min(t.day() - 1);
                pending_total += t.amount;
                if role == Role::Spend {
                    pending_spend_var += cfg.pending_var(t.amount);
                } else {
                    shared_var += cfg.pending_var(t.amount);
                }
            } else {
                *posted_by_day.entry(t.day()).or_default() += t.amount;
                if t.authorized_date.is_none() {
                    missing_auth.push((t.date, (cfg.missing_auth_rel_sd * t.amount).powi(2)));
                }
            }
            if role == Role::Input {
                let agg = days.entry(t.day()).or_default();
                if pending {
                    agg.u_pending += t.amount;
                } else {
                    agg.u_posted += t.amount;
                    if t.kind == FlowKind::Income {
                        agg.income.push(t.amount);
                    }
                }
            }
        }
        missing_auth.sort_by_key(|&(d, _)| d);

        let mut running = 0.0;
        let posted_cum = posted_by_day
            .into_iter()
            .map(|(d, a)| {
                running += a;
                (d, running)
            })
            .collect();

        Ok(Self {
            cfg: *cfg,
            today,
            settled_through,
            opening_balance: ledger_balance - posted_all,
            roles,
            days,
            posted_cum,
            pending_total,
            pending_spend_var,
            shared_var,
            missing_auth,
        })
    }

    pub fn today(&self) -> Day {
        self.today
    }

    /// The last day whose records are all final.
    pub fn settled_through(&self) -> Day {
        self.settled_through
    }

    /// The balance before every record in the snapshot.
    pub fn opening_balance(&self) -> f64 {
        self.opening_balance
    }

    /// Roles, parallel to the input transactions.
    pub fn roles(&self) -> &[Role] {
        &self.roles
    }

    /// Variance of pending known flows, shared by `u` and `z`. Add it to the
    /// reported `P_bb` of the live estimate.
    pub fn shared_variance(&self) -> f64 {
        self.shared_var
    }

    /// Authorized-basis balance at the end of `d`, from posted counted records.
    fn posted_balance_through(&self, d: Day) -> f64 {
        let i = self.posted_cum.partition_point(|&(day, _)| day <= d);
        self.opening_balance
            + if i == 0 {
                0.0
            } else {
                self.posted_cum[i - 1].1
            }
    }

    fn missing_auth_var(&self, d: Day) -> f64 {
        let reach = add_business_days(d, self.cfg.missing_auth_window);
        let first = self.missing_auth.partition_point(|&(p, _)| p <= d);
        self.missing_auth[first..]
            .iter()
            .take_while(|&&(p, _)| p <= reach)
            .map(|&(_, v)| v)
            .sum()
    }

    fn income_on(&self, d: Day) -> Vec<f64> {
        self.days
            .get(&d)
            .map(|a| a.income.clone())
            .unwrap_or_default()
    }

    /// Input for a settled day (`d <= settled_through`), or `None`.
    pub fn settled_day(&self, d: Day) -> Option<FeedDay> {
        if d > self.settled_through {
            return None;
        }
        let r = self.cfg.r_post + self.missing_auth_var(d);
        Some(FeedDay {
            day: d,
            u: self.days.get(&d).map_or(0.0, |a| a.u_posted),
            balance: Some((self.posted_balance_through(d), r)),
            income: self.income_on(d),
        })
    }

    /// Inputs for the provisional tail, `settled_through + 1 ..= today`.
    pub fn tail_days(&self) -> Vec<FeedDay> {
        (self.settled_through + 1..=self.today)
            .map(|d| {
                let agg = self.days.get(&d);
                let balance = (d == self.today).then(|| {
                    (
                        self.posted_balance_through(self.today) + self.pending_total,
                        self.cfg.r_post + self.pending_spend_var,
                    )
                });
                FeedDay {
                    day: d,
                    u: agg.map_or(0.0, |a| a.u_posted + a.u_pending),
                    balance,
                    income: self.income_on(d),
                }
            })
            .collect()
    }
}

fn within_business_days(a: Day, b: Day, window: u32) -> bool {
    add_business_days(a.min(b), window) >= a.max(b)
}

fn assign_roles(txns: &[Transaction], today: Day, cfg: &ObserveConfig) -> Vec<Role> {
    let mut roles: Vec<Role> = txns
        .iter()
        .map(|t| match t.kind {
            FlowKind::Income | FlowKind::Scheduled => Role::Input,
            FlowKind::Discretionary => Role::Spend,
            FlowKind::Transfer => Role::HeldTransfer,
        })
        .collect();

    let mut legs: Vec<usize> = (0..txns.len())
        .filter(|&i| txns[i].kind == FlowKind::Transfer)
        .collect();
    legs.sort_by_key(|&i| (txns[i].day(), txns[i].id));
    for (pos, &i) in legs.iter().enumerate() {
        let out = &txns[i];
        if out.amount >= 0.0 || roles[i] != Role::HeldTransfer {
            continue;
        }
        let partner = legs.iter().enumerate().find(|&(q, &j)| {
            let inc = &txns[j];
            q != pos
                && roles[j] == Role::HeldTransfer
                && inc.amount > 0.0
                && inc.account != out.account
                && (inc.amount + out.amount).abs() < 0.005
                && within_business_days(out.day(), inc.day(), cfg.transfer_window)
        });
        if let Some((_, &j)) = partner {
            roles[i] = Role::MatchedTransfer;
            roles[j] = Role::MatchedTransfer;
        }
    }
    for &i in &legs {
        if roles[i] == Role::HeldTransfer
            && !within_business_days(txns[i].day(), today, cfg.transfer_window)
        {
            roles[i] = Role::Input;
        }
    }
    roles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::day_from_civil;
    use crate::types::{AccountId, TxnId};

    /// Monday 2024-03-04.
    fn mon() -> Day {
        day_from_civil(2024, 3, 4)
    }

    fn txn(id: u64, day: Day, amount: f64, kind: FlowKind, status: TxnStatus) -> Transaction {
        Transaction {
            id: TxnId(id),
            account: AccountId(0),
            date: day,
            authorized_date: Some(day),
            status,
            amount,
            kind,
            stream: None,
            pending_id: None,
        }
    }

    fn posted(id: u64, day: Day, amount: f64, kind: FlowKind) -> Transaction {
        txn(id, day, amount, kind, TxnStatus::Posted)
    }

    fn pending(id: u64, day: Day, amount: f64, kind: FlowKind) -> Transaction {
        txn(id, day, amount, kind, TxnStatus::Pending)
    }

    fn feed(txns: &[Transaction], ledger: f64, today: Day) -> ObservedFeed {
        ObservedFeed::new(txns, ledger, today, &ObserveConfig::default()).unwrap()
    }

    #[test]
    fn settled_days_by_hand() {
        let d = mon();
        let txns = [
            posted(1, d, 2000.0, FlowKind::Income),
            posted(2, d, -50.0, FlowKind::Discretionary),
            posted(3, d + 1, -1500.0, FlowKind::Scheduled),
            posted(4, d + 2, -30.0, FlowKind::Discretionary),
            pending(5, d + 3, -40.0, FlowKind::Discretionary),
        ];
        // Ledger counts posted records only: opening = 1000 + 2000 − 50 − 1500 − 30.
        let f = feed(&txns, 1420.0, d + 3);
        assert_eq!(f.opening_balance(), 1000.0);
        assert_eq!(f.settled_through(), d + 2);

        assert_eq!(
            f.settled_day(d).unwrap(),
            FeedDay {
                day: d,
                u: 2000.0,
                balance: Some((2950.0, 1.0)),
                income: vec![2000.0],
            }
        );
        assert_eq!(f.settled_day(d + 1).unwrap().u, -1500.0);
        assert_eq!(f.settled_day(d + 1).unwrap().balance, Some((1450.0, 1.0)));
        assert_eq!(f.settled_day(d + 2).unwrap().balance, Some((1420.0, 1.0)));
        // A settled day with no records still observes the (unchanged) balance.
        assert_eq!(f.settled_day(d - 5).unwrap().balance, Some((1000.0, 1.0)));
        assert_eq!(f.settled_day(d + 3), None);

        // Tail: today only, balance includes the pending −40 with R = 1 + 20².
        let tail = f.tail_days();
        assert_eq!(
            tail,
            vec![FeedDay {
                day: d + 3,
                u: 0.0,
                balance: Some((1380.0, 401.0)),
                income: vec![],
            }]
        );
        assert_eq!(f.shared_variance(), 0.0);
    }

    #[test]
    fn pending_known_flows_go_to_u_and_shared_variance() {
        let d = mon();
        let txns = [
            pending(1, d, -1500.0, FlowKind::Scheduled),
            pending(2, d + 1, -60.0, FlowKind::Discretionary),
        ];
        let f = feed(&txns, 500.0, d + 1);
        assert_eq!(f.settled_through(), d - 1);
        let tail = f.tail_days();
        assert_eq!(tail.len(), 2);
        assert_eq!((tail[0].u, tail[0].balance), (-1500.0, None));
        // z = 500 − 1500 − 60; R = 1 + max(0.2·60, 20)² (the rent is not in R).
        assert_eq!(tail[1].balance, Some((-1060.0, 401.0)));
        // 0.2·1500 = 300 → 90 000.
        assert_eq!(f.shared_variance(), 90_000.0);
    }

    #[test]
    fn transfers_match_hold_and_age_out() {
        let d = mon();
        let mut out = posted(1, d, -300.0, FlowKind::Transfer);
        let mut inc = posted(2, d + 1, 300.0, FlowKind::Transfer);
        inc.account = AccountId(1);

        // Both legs visible: excluded, and the balance ignores them.
        let f = feed(&[out.clone(), inc.clone()], 1000.0, d + 1);
        assert_eq!(f.roles(), &[Role::MatchedTransfer, Role::MatchedTransfer]);
        assert_eq!(f.settled_through(), d + 1);
        assert_eq!(f.settled_day(d + 1).unwrap().balance, Some((1000.0, 1.0)));

        // Only the outgoing leg so far: held, blocks settlement, not in z.
        let f = feed(&[out.clone()], 700.0, d + 1);
        assert_eq!(f.roles(), &[Role::HeldTransfer]);
        assert_eq!(f.settled_through(), d - 1);
        let tail = f.tail_days();
        assert_eq!(tail.last().unwrap().balance, Some((1000.0, 1.0)));

        // Same amount but the same account is not an own-account pair.
        inc.account = out.account;
        let f = feed(&[out.clone(), inc], 1000.0, d + 1);
        assert_eq!(f.roles(), &[Role::HeldTransfer, Role::HeldTransfer]);

        // Unmatched after the window: an external flow, in u.
        out.date = d;
        let f = feed(&[out], 700.0, d + 7);
        assert_eq!(f.roles(), &[Role::Input]);
        assert_eq!(f.settled_through(), d + 7);
        assert_eq!(f.settled_day(d).unwrap().u, -300.0);
    }

    #[test]
    fn missing_authorized_date_inflates_r_before_posting() {
        let d = mon(); // Monday
        let mut t = posted(1, d + 3, -100.0, FlowKind::Discretionary); // posted Thursday
        t.authorized_date = None;
        let f = feed(&[t], 900.0, d + 4);
        // (0.5·100)² = 2500 on every earlier day from which a 3-business-day
        // lag reaches Thursday: Saturday through Wednesday (weekend purchases
        // post from Monday). Friday's window ends Wednesday.
        let cases = [
            (d - 3, 0.0),
            (d - 2, 2500.0),
            (d - 1, 2500.0),
            (d, 2500.0),
            (d + 2, 2500.0),
            (d + 3, 0.0),
        ];
        for (day, extra) in cases {
            let r = f.settled_day(day).unwrap().balance.unwrap().1;
            assert_eq!(r, 1.0 + extra, "day {}", day - d);
        }
    }

    #[test]
    fn rejects_bad_snapshots() {
        let d = mon();
        let a = posted(1, d, -5.0, FlowKind::Discretionary);
        let cfg = ObserveConfig::default();
        assert!(ObservedFeed::new(&[a.clone(), a.clone()], 0.0, d, &cfg).is_err());
        assert!(ObservedFeed::new(std::slice::from_ref(&a), 0.0, d - 1, &cfg).is_err());
        assert!(ObservedFeed::new(std::slice::from_ref(&a), f64::NAN, d, &cfg).is_err());
        let bad_cfg = ObserveConfig { r_post: 0.0, ..cfg };
        assert!(ObservedFeed::new(&[a], 0.0, d, &bad_cfg).is_err());
    }
}
