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

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::calendar::{PaySchedule, civil_from_day, day_from_civil, days_in_month};
use crate::error::EngineError;
use crate::forecast::{Cone, ForecastConfig};
use crate::model::{IDX_B, IDX_C, IDX_M};
use crate::schedule::Schedule;
use crate::tracker::StateEstimate;
use crate::types::{AccountId, Day, FlowKind, StreamId, Transaction, TxnId, TxnStatus};

/// Version of both [`Feed`] and [`ForecastReport`].
pub const SCHEMA_VERSION: u32 = 1;

/// A [`Day`] that serializes as `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedTransaction {
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
    pub pending_id: Option<u64>,
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

    /// The records as engine transactions, after checking the version.
    pub fn transactions(&self) -> Result<Vec<Transaction>, EngineError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(EngineError::InvalidInput("unsupported feed schema_version"));
        }
        Ok(self.transactions.iter().map(Transaction::from).collect())
    }
}

/// A scalar estimate with its ±2σ range.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncomeReport {
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillReport {
    pub stream: u32,
    /// Latest charged amount, signed (negative for an outflow).
    pub amount: f64,
    pub period_days: u32,
    pub last_charged: Date,
    /// `last_charged + period_days`, before drift.
    pub next_due: Date,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConeDay {
    pub date: Date,
    /// Balance quantiles, in the order of [`ConeReport::levels`].
    pub quantiles: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConeReport {
    pub horizon_days: u32,
    pub paths: u32,
    pub seed: u64,
    /// Quantile levels, ascending, each in (0, 1).
    pub levels: Vec<f64>,
    /// One entry per forecast day, `as_of + 1 ..= as_of + horizon_days`.
    pub days: Vec<ConeDay>,
}

/// Everything `ephemeris forecast --json` writes.
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
    /// Expected primary paycheck `m`.
    pub expected_paycheck: Estimate,
    pub income: Option<IncomeReport>,
    pub bills: Vec<BillReport>,
    pub forecast: ConeReport,
}

impl ForecastReport {
    /// Assemble the report. Horizon and levels come from `cone`; `cfg`
    /// supplies the path count and seed that produced it.
    pub fn new(
        est: &StateEstimate,
        schedule: &Schedule,
        cone: &Cone,
        cfg: &ForecastConfig,
    ) -> Self {
        let state = |i: usize| Estimate::from_variance(est.x[i], est.p[(i, i)]);
        let income = schedule.income.as_ref().map(|t| IncomeReport {
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
                amount: b.amount,
                period_days: b.period_days,
                last_charged: Date(b.last_day),
                next_due: Date(b.last_day + b.period_days as Day),
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
            expected_paycheck: state(IDX_M),
            income,
            bills,
            forecast: ConeReport {
                horizon_days: cone.horizon(),
                paths: cfg.n_paths,
                seed: cfg.seed,
                levels: cone.levels().to_vec(),
                days,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            }],
        };
        let mut cfg = ForecastConfig::new(10, 4);
        cfg.n_paths = 50;
        let model = ForecastModel { q_c: 0.0, q_m: 0.0 };
        let cone = forecast(&est, &schedule, &model, &cfg).unwrap();
        let r = ForecastReport::new(&est, &schedule, &cone, &cfg);

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
