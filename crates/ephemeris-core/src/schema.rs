//! Versioned JSON contracts, behind the `serde` feature.
//!
//! - [`Feed`]: what a bank adapter hands the engine. It holds every record
//!   visible at the end of `today` plus the posted balance. `ephemeris
//!   simulate` writes it from the generator; the Plaid adapter will write the
//!   same thing.
//! - [`ForecastReport`]: the tracker's estimate, the detected schedule and the
//!   Monte Carlo cone. The Phase 2 web shell consumes it.
//!
//! These are plain transfer types, kept separate from the engine's internal
//! types so internals can change without breaking the contract. Both carry
//! `schema_version` ([`SCHEMA_VERSION`]). Dates are ISO `YYYY-MM-DD` strings
//! in the account's local calendar. Documented in docs/forecast-schema.md;
//! change the two together, and bump the version on any breaking change.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::calendar::{PaySchedule, civil_from_day, day_from_civil, days_in_month};
use crate::error::EngineError;
use crate::forecast::{Cone, CycleBasis, ForecastConfig};
use crate::model::{IDX_B, IDX_C, IDX_M};
use crate::schedule::{DayShift, Schedule};
use crate::tracker::{IngestReport, StateEstimate};
use crate::truncated::TruncatedNormal;
use crate::types::{AccountId, Channel, Day, FlowKind, StreamId, Transaction, TxnId, TxnStatus};

/// Version of both [`Feed`] and [`ForecastReport`].
pub const SCHEMA_VERSION: u32 = 1;

/// A [`Day`] that serializes as `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(rename = "IsoDate", type = "string")
)]
pub struct Date(pub Day);

impl Date {
    /// Parse a strict `YYYY-MM-DD` (four-digit year, zero-padded, a real date).
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let num = |from: usize, to: usize| -> Option<u32> {
            let part = &b[from..to];
            part.iter()
                .all(u8::is_ascii_digit)
                .then(|| part.iter().fold(0, |n, &d| n * 10 + u32::from(d - b'0')))
        };
        let (y, m, d) = (num(0, 4)? as i32, num(5, 7)?, num(8, 10)?);
        if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
            return None;
        }
        Some(Date(day_from_civil(y, m, d)))
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = civil_from_day(self.0);
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Date::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("invalid date {s:?}, expected YYYY-MM-DD"))
        })
    }
}

/// One bank-feed snapshot: everything visible at the end of `today`.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feed {
    pub schema_version: u32,
    /// First day of fetched history; no transaction is dated earlier.
    pub history_start: Date,
    pub today: Date,
    /// The bank's posted balance across all linked depository accounts at the
    /// end of `today` (pending records excluded).
    pub ledger_balance: f64,
    pub transactions: Vec<FeedTransaction>,
}

/// A [`Transaction`] on the wire. Amounts are signed, **inflows positive**.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedTransaction {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: u64,
    pub account: u32,
    pub date: Date,
    #[serde(default)]
    pub authorized_date: Option<Date>,
    pub status: TxnStatus,
    pub amount: f64,
    pub kind: FlowKind,
    #[serde(default)]
    pub stream: Option<u32>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub pending_id: Option<u64>,
    /// Display name of the counterparty (Plaid `merchant_name`), if known.
    #[serde(default)]
    pub merchant_name: Option<String>,
    /// The bank's description line (Plaid `name`), if any.
    #[serde(default)]
    pub description: Option<String>,
}

impl From<&Transaction> for FeedTransaction {
    fn from(t: &Transaction) -> Self {
        Self {
            id: t.id.0,
            account: t.account.0,
            date: Date(t.date),
            authorized_date: t.authorized_date.map(Date),
            status: t.status,
            amount: t.amount,
            kind: t.kind,
            stream: t.stream.map(|s| s.0),
            pending_id: t.pending_id.map(|p| p.0),
            merchant_name: None,
            description: None,
        }
    }
}

impl From<&FeedTransaction> for Transaction {
    fn from(t: &FeedTransaction) -> Self {
        Self {
            id: TxnId(t.id),
            account: AccountId(t.account),
            date: t.date.0,
            authorized_date: t.authorized_date.map(|d| d.0),
            status: t.status,
            amount: t.amount,
            kind: t.kind,
            stream: t.stream.map(StreamId),
            pending_id: t.pending_id.map(TxnId),
        }
    }
}

impl FeedTransaction {
    /// Merchant name, else description, if either is non-empty.
    pub fn display_name(&self) -> Option<&str> {
        [self.merchant_name.as_deref(), self.description.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|s| !s.is_empty())
    }
}

impl Feed {
    pub fn new(history_start: Day, today: Day, ledger_balance: f64, txns: &[Transaction]) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            history_start: Date(history_start),
            today: Date(today),
            ledger_balance,
            transactions: txns.iter().map(FeedTransaction::from).collect(),
        }
    }

    /// The same feed with every amount and the ledger balance rounded to
    /// cents, as a bank reports them.
    pub fn rounded_to_cents(mut self) -> Self {
        self.ledger_balance = cents(self.ledger_balance);
        for t in &mut self.transactions {
            t.amount = cents(t.amount);
        }
        self
    }

    /// Display name per stream, from its most recent named record.
    pub fn stream_names(&self) -> BTreeMap<u32, String> {
        let mut latest: BTreeMap<u32, (Day, u64, &str)> = BTreeMap::new();
        for t in &self.transactions {
            let (Some(stream), Some(name)) = (t.stream, t.display_name()) else {
                continue;
            };
            let key = (t.authorized_date.unwrap_or(t.date).0, t.id, name);
            let e = latest.entry(stream).or_insert(key);
            if (key.0, key.1) > (e.0, e.1) {
                *e = key;
            }
        }
        latest
            .into_iter()
            .map(|(s, (_, _, n))| (s, n.to_string()))
            .collect()
    }

    /// Display name of the most recent posted income record.
    pub fn income_name(&self) -> Option<String> {
        self.transactions
            .iter()
            .filter(|t| t.kind == FlowKind::Income && t.status == TxnStatus::Posted)
            .filter_map(|t| Some((t.authorized_date.unwrap_or(t.date), t.id, t.display_name()?)))
            .max_by_key(|&(d, id, _)| (d, id))
            .map(|(_, _, n)| n.to_string())
    }

    /// The records as engine transactions, after checking the version.
    pub fn transactions(&self) -> Result<Vec<Transaction>, EngineError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(EngineError::InvalidInput("unsupported feed schema_version"));
        }
        Ok(self.transactions.iter().map(Transaction::from).collect())
    }
}

/// Nearest cent. `+ 0.0` turns a rounded `-0.0` into `0.0`.
fn cents(x: f64) -> f64 {
    (x * 100.0).round() / 100.0 + 0.0
}

/// The spend rate's Gaussian `N(mean, sd²)` truncated below at `floor`:
/// its 2.275% and 97.725% points, the coverage of ±2σ, so they equal
/// `low_2sd`/`high_2sd` whenever the floor is far below.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpendRange {
    pub floor: f64,
    pub low: f64,
    pub high: f64,
    /// Share of the untruncated Gaussian below the floor, i.e. how much the
    /// floor changed.
    pub below_floor: f64,
}

impl SpendRange {
    /// Two-sided tail of ±2σ: Φ(−2).
    const TAIL: f64 = 0.022_750_131_948_179_21;

    pub fn new(mean: f64, var: f64, floor: f64) -> Self {
        let t = TruncatedNormal {
            mean,
            sd: var.max(0.0).sqrt(),
            floor,
        };
        Self {
            floor,
            low: t.quantile(Self::TAIL),
            high: t.quantile(1.0 - Self::TAIL),
            below_floor: t.mass_below(),
        }
    }
}

/// A scalar estimate with its ±2σ range.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub mean: f64,
    pub sd: f64,
    pub low_2sd: f64,
    pub high_2sd: f64,
}

impl Estimate {
    fn from_variance(mean: f64, var: f64) -> Self {
        let sd = var.max(0.0).sqrt();
        Self {
            mean,
            sd,
            low_2sd: mean - 2.0 * sd,
            high_2sd: mean + 2.0 * sd,
        }
    }
}

/// [`PaySchedule`] on the wire, tagged by `kind`.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayCadence {
    Weekly { anchor: Date },
    Biweekly { anchor: Date },
    SemiMonthly { days: [u32; 2] },
    Monthly { day: u32 },
}

impl From<PaySchedule> for PayCadence {
    fn from(s: PaySchedule) -> Self {
        match s {
            PaySchedule::Weekly { anchor } => Self::Weekly {
                anchor: Date(anchor),
            },
            PaySchedule::Biweekly { anchor } => Self::Biweekly {
                anchor: Date(anchor),
            },
            PaySchedule::SemiMonthly { days } => Self::SemiMonthly { days },
            PaySchedule::Monthly { day } => Self::Monthly { day },
        }
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncomeReport {
    /// Payer's display name from the latest paycheck, if the feed has one.
    #[serde(default)]
    pub name: Option<String>,
    pub cadence: PayCadence,
    /// Nominal date of the most recent observed paycheck.
    pub last_paid_nominal: Date,
    /// The next nominal date moved to the previous business day, before any
    /// timing jitter. The forecast samples the actual day around it.
    pub next_scheduled: Option<Date>,
    /// Probability that a paycheck lands at offsets −2, −1, 0, +1, +2 days
    /// from its nominal date (before the business-day shift).
    pub offset_probabilities: [f64; 5],
    /// Fit cost of the chosen cadence (0 = every payday explained exactly).
    pub fit_cost: u32,
}

/// [`DayShift`] on the wire.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shift {
    None,
    PreviousBusinessDay,
    NextBusinessDay,
}

impl From<DayShift> for Shift {
    fn from(s: DayShift) -> Self {
        match s {
            DayShift::None => Shift::None,
            DayShift::PreviousBusinessDay => Shift::PreviousBusinessDay,
            DayShift::NextBusinessDay => Shift::NextBusinessDay,
        }
    }
}

/// A bill charged on a fixed day of the month.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonthlyOn {
    /// 1–31; past the end of a month means its last day.
    pub day: u32,
    /// How a weekend or holiday date moves.
    pub shift: Shift,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillReport {
    pub stream: u32,
    /// Display name from the stream's latest record, if the feed has one.
    #[serde(default)]
    pub name: Option<String>,
    /// Latest charged amount, signed (negative for an outflow).
    pub amount: f64,
    /// Median gap between charges.
    pub period_days: u32,
    /// Set for a day-of-month bill; otherwise it recurs every `period_days`.
    #[serde(default)]
    pub monthly_on: Option<MonthlyOn>,
    pub last_charged: Date,
    /// The next charge: by day of month (shift applied) when `monthly_on` is
    /// set, otherwise `last_charged + period_days` before drift.
    pub next_due: Date,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConeDay {
    pub date: Date,
    /// Balance quantiles, in the order of [`ConeReport::levels`].
    pub quantiles: Vec<f64>,
}

/// `P(b ≥ amount)` per forecast day, parallel to [`ConeReport::days`].
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetReport {
    pub amount: f64,
    /// Probability that the end-of-day balance is at least `amount`.
    pub at_least_on_day: Vec<f64>,
    /// Probability that it is at least `amount` on some day so far.
    pub reached_by: Vec<f64>,
    /// Probability that it is below `amount` on some day so far (with
    /// `amount` 0: the chance of going overdrawn). Empty in older reports.
    #[serde(default)]
    pub below_by: Vec<f64>,
}

/// How [`CycleLowsReport::cycles`] were cut.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleBasisReport {
    /// At each detected payday.
    Paycheck,
    /// Calendar weeks from the first forecast day: no income detected.
    Week,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleLowReport {
    /// First and last day of the cycle, inclusive.
    pub start: Date,
    pub end: Date,
    /// Cut short by the forecast's start or end.
    pub partial: bool,
    /// Quantiles, across paths, of each path's lowest end-of-day balance in
    /// the cycle, in the order of [`ConeReport::levels`].
    pub quantiles: Vec<f64>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleLowsReport {
    pub basis: CycleBasisReport,
    /// Consecutive cycles covering the whole horizon.
    pub cycles: Vec<CycleLowReport>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConeReport {
    pub horizon_days: u32,
    pub paths: u32,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seed: u64,
    /// Quantile levels, ascending, each in (0, 1).
    pub levels: Vec<f64>,
    /// One entry per forecast day, `as_of + 1 ..= as_of + horizon_days`.
    pub days: Vec<ConeDay>,
    /// One curve per requested target balance; empty when none was asked for.
    #[serde(default)]
    pub targets: Vec<TargetReport>,
    /// Pay-cycle lows. Always written; `null` only in reports from before
    /// the field existed.
    #[serde(default)]
    pub cycle_lows: Option<CycleLowsReport>,
}

/// Which state variable a regime change hit.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeChannel {
    /// The discretionary spend rate `c` (balance channel).
    Spend,
    /// The expected paycheck `m` (income channel).
    Income,
}

/// A regime change the tracker detected (design §6).
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RegimeChange {
    /// The day whose observation set off the alarm.
    pub date: Date,
    /// False when it was found on a provisional (unsettled) day.
    pub settled: bool,
    pub channel: ChangeChannel,
    /// Estimated change: $/day for `spend` (positive = more spending), $ per
    /// paycheck for `income`.
    pub shift: f64,
}

impl RegimeChange {
    /// Every jump in an ingest report, in day order.
    pub fn from_ingest(ingest: &IngestReport) -> Vec<Self> {
        ingest
            .days
            .iter()
            .flat_map(|d| {
                d.jumps.iter().map(|j| RegimeChange {
                    date: Date(d.day),
                    settled: d.settled,
                    channel: match j.channel {
                        Channel::Balance => ChangeChannel::Spend,
                        Channel::Income => ChangeChannel::Income,
                    },
                    shift: j.shift,
                })
            })
            .collect()
    }
}

/// Everything `ephemeris forecast --json` writes.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForecastReport {
    pub schema_version: u32,
    /// End of the day the estimate describes (the feed's `today`).
    pub as_of: Date,
    /// Last day whose records are all final; later days are provisional.
    pub settled_through: Date,
    /// True balance `b`, including pending-amount uncertainty.
    pub balance: Estimate,
    /// Discretionary spend rate `c`, $/day, outflow positive.
    pub spend_rate: Estimate,
    /// The spend rate as the forecast samples it, when it has a floor
    /// (design §7). Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub spend_rate_range: Option<SpendRange>,
    /// Expected primary paycheck `m`.
    pub expected_paycheck: Estimate,
    pub income: Option<IncomeReport>,
    pub bills: Vec<BillReport>,
    /// Regime changes found in this ingest, in day order.
    #[serde(default)]
    pub regime_changes: Vec<RegimeChange>,
    pub forecast: ConeReport,
}

impl ForecastReport {
    /// Assemble the report. Horizon and levels come from `cone`; `cfg`
    /// supplies the path count and seed that produced it; `feed` supplies
    /// display names; `ingest` the regime changes.
    pub fn new(
        feed: &Feed,
        est: &StateEstimate,
        ingest: &IngestReport,
        schedule: &Schedule,
        cone: &Cone,
        cfg: &ForecastConfig,
    ) -> Self {
        let state = |i: usize| Estimate::from_variance(est.x[i], est.p[(i, i)]);
        let names = feed.stream_names();
        let income = schedule.income.as_ref().map(|t| IncomeReport {
            name: feed.income_name(),
            cadence: t.schedule.into(),
            last_paid_nominal: Date(t.last_paid_nominal),
            next_scheduled: t
                .schedule
                .nominal_dates(t.last_paid_nominal + 1, t.last_paid_nominal + 62)
                .first()
                .map(|&n| Date(PaySchedule::pay_date(n))),
            offset_probabilities: *t.offsets.probabilities(),
            fit_cost: t.fit_cost,
        });
        let bills = schedule
            .bills
            .iter()
            .map(|b| BillReport {
                stream: b.stream.0,
                name: names.get(&b.stream.0).cloned(),
                amount: b.amount,
                period_days: b.period_days,
                monthly_on: b.day_of_month.map(|a| MonthlyOn {
                    day: a.day,
                    shift: a.shift.into(),
                }),
                last_charged: Date(b.last_day),
                next_due: Date(b.next_due()),
            })
            .collect();
        let days = (1..=cone.horizon() as Day)
            .filter_map(|h| {
                let date = cone.start_day() + h;
                cone.band(date).map(|q| ConeDay {
                    date: Date(date),
                    quantiles: q.to_vec(),
                })
            })
            .collect();
        Self {
            schema_version: SCHEMA_VERSION,
            as_of: Date(est.day),
            settled_through: Date(est.settled_through),
            balance: state(IDX_B),
            spend_rate: state(IDX_C),
            spend_rate_range: cfg
                .spend_floor
                .map(|f| SpendRange::new(est.x[IDX_C], est.p[(IDX_C, IDX_C)], f)),
            expected_paycheck: state(IDX_M),
            income,
            bills,
            regime_changes: RegimeChange::from_ingest(ingest),
            forecast: ConeReport {
                horizon_days: cone.horizon(),
                paths: cfg.n_paths,
                seed: cfg.seed,
                levels: cone.levels().to_vec(),
                days,
                targets: cone
                    .targets()
                    .iter()
                    .map(|t| TargetReport {
                        amount: t.amount,
                        at_least_on_day: t.on_day().to_vec(),
                        reached_by: t.reached_by().to_vec(),
                        below_by: t.below_by().to_vec(),
                    })
                    .collect(),
                cycle_lows: Some(CycleLowsReport {
                    basis: match cone.cycle_basis() {
                        CycleBasis::Paycheck => CycleBasisReport::Paycheck,
                        CycleBasis::Week => CycleBasisReport::Week,
                    },
                    cycles: cone
                        .cycle_lows()
                        .iter()
                        .map(|c| CycleLowReport {
                            start: Date(c.start),
                            end: Date(c.end),
                            partial: c.partial,
                            quantiles: c.quantiles.clone(),
                        })
                        .collect(),
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spend_range_by_hand() {
        // Floor far below: exactly the ±2σ band.
        let e = Estimate::from_variance(60.0, 25.0);
        let r = SpendRange::new(60.0, 25.0, 0.0);
        assert!((r.low - e.low_2sd).abs() < 1e-9 && (r.high - e.high_2sd).abs() < 1e-9);
        assert!(r.below_floor < 1e-30);
        // The sandbox (ĉ = 67.04, σ = 36.0375): −$5.04..$139.11 becomes
        // $8.94..$139.60, with 3.14% of the Gaussian below $0 (design §7).
        let r = SpendRange::new(67.04, 36.0375 * 36.0375, 0.0);
        // (At exactly Φ(−2) = 0.0227501…, not the rounded 0.02275.)
        assert!((r.low - 8.939_911_05).abs() < 1e-6, "{r:?}");
        assert!((r.high - 139.598_602_06).abs() < 1e-6, "{r:?}");
        assert!((r.below_floor - 0.031_422_647_9).abs() < 1e-9);
    }
    use crate::adaptive::VarianceEstimate;
    use crate::forecast::{ForecastModel, forecast};
    use crate::schedule::{BillStream, OffsetDist, PayTiming};
    use crate::tracker::LumpHistory;
    use nalgebra::{Matrix3, Vector3};

    #[test]
    fn dates_round_trip_and_reject_malformed() {
        for s in ["1970-01-01", "2024-02-29", "2024-12-31", "1999-03-04"] {
            assert_eq!(Date::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(Date::parse("1970-01-02"), Some(Date(1)));
        for bad in [
            "2023-02-29",
            "2024-13-01",
            "2024-00-10",
            "2024-04-31",
            "2024-4-01",
            "+024-01-01",
            "2024/01/01",
            "2024-01-01T00:00",
            "",
        ] {
            assert_eq!(Date::parse(bad), None, "{bad}");
        }
        assert!(serde_json::from_str::<Date>("\"2024-02-30\"").is_err());
        assert_eq!(
            serde_json::to_string(&Date(19_723)).unwrap(),
            "\"2024-01-01\""
        );
    }

    #[test]
    fn feed_round_trips_through_json() {
        let start = day_from_civil(2024, 3, 1);
        let txns = vec![
            Transaction {
                id: TxnId(7),
                account: AccountId(1),
                date: start + 3,
                authorized_date: None,
                status: TxnStatus::Posted,
                amount: -12.5,
                kind: FlowKind::Discretionary,
                stream: None,
                pending_id: Some(TxnId(6)),
            },
            Transaction {
                id: TxnId(8),
                account: AccountId(0),
                date: start + 4,
                authorized_date: Some(start + 4),
                status: TxnStatus::Pending,
                amount: 2400.0,
                kind: FlowKind::Income,
                stream: Some(StreamId(2)),
                pending_id: None,
            },
        ];
        let feed = Feed::new(start, start + 4, 987.5, &txns);
        let json = serde_json::to_string(&feed).unwrap();
        assert!(json.contains("\"date\":\"2024-03-04\""), "{json}");
        assert!(json.contains("\"kind\":\"discretionary\""), "{json}");
        assert!(json.contains("\"status\":\"pending\""), "{json}");
        let back: Feed = serde_json::from_str(&json).unwrap();
        assert_eq!(back, feed);
        assert_eq!(back.transactions().unwrap(), txns);

        // Optional fields may be omitted.
        let minimal = r#"{"id":1,"account":0,"date":"2024-03-02","status":"posted",
            "amount":-3.0,"kind":"scheduled"}"#;
        let t: FeedTransaction = serde_json::from_str(minimal).unwrap();
        assert_eq!(
            (t.authorized_date, t.stream, t.pending_id),
            (None, None, None)
        );
    }

    #[test]
    fn rounding_to_cents() {
        let mut feed = Feed::new(0, 0, 1234.5678, &[]);
        feed.transactions = [48.2542, -0.004, 2.675, -15.607_323]
            .iter()
            .enumerate()
            .map(|(i, &amount)| FeedTransaction {
                id: i as u64,
                account: 0,
                date: Date(0),
                authorized_date: None,
                status: TxnStatus::Posted,
                amount,
                kind: FlowKind::Discretionary,
                stream: None,
                pending_id: None,
                merchant_name: None,
                description: None,
            })
            .collect();
        let feed = feed.rounded_to_cents();
        assert_eq!(feed.ledger_balance, 1234.57);
        let amounts: Vec<f64> = feed.transactions.iter().map(|t| t.amount).collect();
        assert_eq!(amounts, [48.25, 0.0, 2.68, -15.61]);
        assert!(amounts[1].is_sign_positive());
    }

    #[test]
    fn other_feed_versions_are_rejected() {
        let mut feed = Feed::new(0, 0, 0.0, &[]);
        feed.schema_version = SCHEMA_VERSION + 1;
        assert!(feed.transactions().is_err());
    }

    #[test]
    fn report_by_hand() {
        let t0 = day_from_civil(2024, 5, 31);
        let est = StateEstimate {
            day: t0,
            settled_through: t0 - 2,
            x: Vector3::new(1000.0, 50.0, 2000.0),
            p: Matrix3::from_diagonal(&Vector3::new(100.0, 4.0, 0.0)),
            spend_var: VarianceEstimate {
                value: 0.0,
                dof: 10.0,
            },
            income_var: VarianceEstimate {
                value: 0.0,
                dof: 10.0,
            },
            lumps: LumpHistory::default(),
        };
        let schedule = Schedule {
            income: Some(PayTiming {
                schedule: PaySchedule::SemiMonthly { days: [15, 31] },
                last_paid_nominal: t0,
                offsets: OffsetDist::exact(),
                fit_cost: 0,
            }),
            bills: vec![BillStream {
                stream: StreamId(3),
                period_days: 30,
                last_day: t0 - 25,
                amount: -900.0,
                drift: OffsetDist::exact(),
                day_of_month: None,
            }],
        };
        let mut cfg = ForecastConfig::new(10, 4);
        cfg.n_paths = 50;
        cfg.targets = vec![900.0];
        let model = ForecastModel { q_c: 0.0, q_m: 0.0 };
        let cone = forecast(&est, &schedule, &model, &cfg).unwrap();
        let mut feed = Feed::new(t0 - 60, t0, 1000.0, &[]);
        let named = |id, day, kind, stream: Option<u32>, merchant: Option<&str>, desc: &str| {
            FeedTransaction {
                id,
                account: 0,
                date: Date(day),
                authorized_date: Some(Date(day)),
                status: TxnStatus::Posted,
                amount: -900.0,
                kind,
                stream,
                pending_id: None,
                merchant_name: merchant.map(str::to_string),
                description: Some(desc.to_string()),
            }
        };
        feed.transactions = vec![
            named(
                1,
                t0 - 55,
                FlowKind::Scheduled,
                Some(3),
                Some("Old name"),
                "x",
            ),
            named(
                2,
                t0 - 25,
                FlowKind::Scheduled,
                Some(3),
                None,
                "ACME RENT ACH",
            ),
            named(
                3,
                t0 - 16,
                FlowKind::Income,
                None,
                Some("Employer"),
                "PAYROLL",
            ),
            named(
                4,
                t0,
                FlowKind::Income,
                None,
                Some("  "),
                "Employer Inc payroll",
            ),
        ];
        assert_eq!(feed.stream_names()[&3], "ACME RENT ACH");
        let ingest = IngestReport {
            rebuilt: false,
            days: vec![crate::tracker::DayReport {
                day: t0 - 3,
                settled: true,
                innovations: Vec::new(),
                lump: 0.0,
                jumps: vec![crate::tracker::Jump {
                    channel: Channel::Balance,
                    shift: 42.0,
                    run_len: 3,
                }],
            }],
        };
        let r = ForecastReport::new(&feed, &est, &ingest, &schedule, &cone, &cfg);
        // No floor: no range, and the field is not written at all.
        assert_eq!(r.spend_rate_range, None);
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("spend_rate_range"));
        assert_eq!(serde_json::from_str::<ForecastReport>(&json).unwrap(), r);
        // With a floor: the range of the estimate's c, round-tripped.
        let floored = ForecastConfig {
            spend_floor: Some(0.0),
            ..cfg.clone()
        };
        let rf = ForecastReport::new(&feed, &est, &ingest, &schedule, &cone, &floored);
        assert_eq!(
            rf.spend_rate_range,
            Some(SpendRange::new(est.x[IDX_C], est.p[(IDX_C, IDX_C)], 0.0))
        );
        let json = serde_json::to_string(&rf).unwrap();
        assert_eq!(serde_json::from_str::<ForecastReport>(&json).unwrap(), rf);
        assert_eq!(
            r.regime_changes,
            [RegimeChange {
                date: Date(t0 - 3),
                settled: true,
                channel: ChangeChannel::Spend,
                shift: 42.0
            }]
        );
        let target = &r.forecast.targets[0];
        assert_eq!(target.amount, 900.0);
        assert_eq!(target.at_least_on_day.len(), 10);
        assert_eq!(
            target.reached_by[9],
            cone.prob_reached_by(0, t0 + 10).unwrap()
        );
        assert_eq!(target.below_by[9], cone.prob_below_by(0, t0 + 10).unwrap());
        let lows = r.forecast.cycle_lows.as_ref().unwrap();
        assert_eq!(lows.basis, CycleBasisReport::Paycheck);
        assert_eq!(lows.cycles.len(), cone.cycle_lows().len());
        assert_eq!(lows.cycles[0].start, Date(t0 + 1));
        assert_eq!(
            r.income.as_ref().unwrap().name.as_deref(),
            Some("Employer Inc payroll")
        );
        assert_eq!(r.bills[0].name.as_deref(), Some("ACME RENT ACH"));

        assert_eq!(r.schema_version, SCHEMA_VERSION);
        assert_eq!((r.as_of, r.settled_through), (Date(t0), Date(t0 - 2)));
        assert_eq!(
            r.balance,
            Estimate {
                mean: 1000.0,
                sd: 10.0,
                low_2sd: 980.0,
                high_2sd: 1020.0
            }
        );
        assert_eq!((r.spend_rate.sd, r.spend_rate.low_2sd), (2.0, 46.0));
        assert_eq!(r.expected_paycheck.sd, 0.0);

        let income = r.income.as_ref().unwrap();
        assert_eq!(income.cadence, PayCadence::SemiMonthly { days: [15, 31] });
        // June 15 2024 is a Saturday: scheduled Friday June 14.
        assert_eq!(
            income.next_scheduled,
            Some(Date(day_from_civil(2024, 6, 14)))
        );
        assert_eq!(income.offset_probabilities, [0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(r.bills[0].next_due, Date(day_from_civil(2024, 6, 5)));

        assert_eq!(
            (r.forecast.horizon_days, r.forecast.paths, r.forecast.seed),
            (10, 50, 4)
        );
        assert_eq!(r.forecast.levels, cfg.levels);
        assert_eq!(r.forecast.days.len(), 10);
        assert_eq!(r.forecast.days[0].date, Date(t0 + 1));
        assert_eq!(r.forecast.days[9].quantiles, cone.band(t0 + 10).unwrap());

        let json = serde_json::to_string(&r).unwrap();
        assert!(
            json.contains(r#""cadence":{"kind":"semi_monthly","days":[15,31]}"#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<ForecastReport>(&json).unwrap(), r);
    }
}
