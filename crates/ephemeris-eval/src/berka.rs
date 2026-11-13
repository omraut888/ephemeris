//! The PKDD'99 Berka Czech bank data as engine feeds (paper/PLAN.md §2, §8).
//!
//! Input is the canonical `trans.tsv` written by
//! `scripts/fetch_paper_data.py`. Each account becomes its own feed with
//! one account and no transfers. At every forecast origin the feed is
//! rebuilt from the records on or before the origin only:
//!
//! - **Signs.** `PRIJEM` (credit) is an inflow; `VYDAJ` (debit) and `VYBER`
//!   (cash withdrawal) are outflows.
//! - **Dates.** Every record is posted on its booking date, and
//!   `authorized_date = date` (§8 decision 4). The calendar is
//!   [`HolidayCalendar::EveryDay`], not Czech: Berka books on weekends at
//!   the uniform rate (28.5–30.5% of records against 2/7 = 28.6%) and a
//!   pension due on the 11th lands on the 11th even on a Sunday, so no
//!   business-day calendar explains its dates ([`CALENDAR`]).
//! - **Balance.** The end-of-day balance is the opening balance plus the
//!   signed amounts up to that day, where the opening balance is the first
//!   record's reported balance minus its amount. The CTU export stores
//!   amounts and balances as whole crowns, rounded separately, so this
//!   running sum drifts from the bank's balances by a few crowns;
//!   [`AccountStats`] measures the drift.
//! - **Tagging**, causal, mirroring the Plaid adapter's rules
//!   (crates/ephemeris-sync/src/tag.rs). A counterparty is
//!   `(operation, k_symbol, bank, account)`:
//!   - *income*: the inflows of the counterparty whose posted inflows form
//!     a regular pay schedule ([`regular_pay_schedule_in`], [`CALENDAR`])
//!     with the largest yearly amount; within 10% a pension (`DUCHOD`) wins
//!     the tie, as a wage category does for Plaid. Interest (`UROK`) is
//!     never income;
//!   - *scheduled*: at least three records with the same counterparty and
//!     amount on a regular interval ([`is_regular_interval`]), or a
//!     bill-like `k_symbol` (`SIPO` household, `UVER` loan, `POJISTNE`
//!     insurance, `LEASING`, `SLUZBY` statement fee, `SANKC. UROK` penalty
//!     interest, `UROK` interest);
//!   - everything else is *discretionary*.
//!
//!   Income and scheduled records get one stream per counterparty, numbered
//!   in counterparty order.
//! - **Scale** (decision 2): at each origin, `s` is the median over the
//!   account's complete calendar months on or before the origin of the
//!   month's gross flow `Σ|amount|`; every amount and balance is multiplied
//!   by `REFERENCE_SCALE / s` ([`crate::reference`]).

use std::collections::{BTreeMap, HashMap};

use ephemeris_core::calendar::{
    HolidayCalendar, PaySchedule, civil_from_day, day_from_civil, days_in_month,
};
use ephemeris_core::schedule::{is_regular_interval, regular_pay_schedule_in};
use ephemeris_core::types::{AccountId, StreamId, TxnId};
use ephemeris_core::{Day, FlowKind, Transaction, TxnStatus};

/// Berka's business-day calendar: none (see the module docs). The plan's
/// decision 1 named Czech; on 2026-11-13 a first development run detected
/// no income at any origin, and the record dates showed why (paper/PLAN.md
/// §8, deviation 1). No held-out data was involved.
pub const CALENDAR: HolidayCalendar = HolidayCalendar::EveryDay;
/// An account whose running balance ever differs from every balance the
/// bank reported that day by more than this (CZK) is excluded as
/// inconsistent data. Rounding to whole crowns drifts by at most about 30
/// over a history; the 2% of development accounts above 50 are all above
/// 1,000 (a reported balance that resets). Fixed 2026-11-13 on development
/// data quality, before any results were read (paper/PLAN.md §8).
pub const MAX_DRIFT_CZK: f64 = 100.0;
/// Same as the Plaid adapter: same-amount repeats that make a stream a bill,
/// and the near-tie band for the income choice.
pub const RECURRING_MIN: usize = 3;
pub const INCOME_TIE: f64 = 0.1;
const BILL_SYMBOLS: [&str; 7] = [
    "SIPO",
    "UVER",
    "POJISTNE",
    "LEASING",
    "SLUZBY",
    "SANKC. UROK",
    "UROK",
];

/// One `trans.tsv` row.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub trans_id: u64,
    pub day: Day,
    /// Signed CZK, inflows positive.
    pub amount: f64,
    /// The bank's balance after the record.
    pub balance: f64,
    pub operation: String,
    pub k_symbol: String,
    pub bank: String,
    pub counter_account: String,
}

impl Record {
    pub fn counterparty(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.operation, self.k_symbol, self.bank, self.counter_account
        )
    }
}

/// An account's records, sorted by (day, trans_id).
#[derive(Debug, Clone)]
pub struct Account {
    pub id: u64,
    pub records: Vec<Record>,
    /// Opening balance implied by the first record.
    pub opening: f64,
}

fn parse_day(s: &str) -> Result<Day, String> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return Err(format!("bad date {s:?}"));
    }
    let n = |r: std::ops::Range<usize>| s[r].parse::<u32>().map_err(|e| format!("{s:?}: {e}"));
    Ok(day_from_civil(n(0..4)? as i32, n(5..7)?, n(8..10)?))
}

fn field(s: &str) -> &str {
    if s == "\\N" { "" } else { s.trim() }
}

/// Parse `trans.tsv` into accounts, by account id.
pub fn parse_trans(tsv: &str) -> Result<BTreeMap<u64, Account>, String> {
    let mut lines = tsv.lines();
    let header: Vec<&str> = lines.next().ok_or("empty file")?.split('\t').collect();
    let want = [
        "trans_id",
        "account_id",
        "date",
        "type",
        "operation",
        "amount",
        "balance",
        "k_symbol",
        "bank",
        "account",
    ];
    if header != want {
        return Err(format!("unexpected header {header:?}"));
    }
    let mut by_account: BTreeMap<u64, Vec<Record>> = BTreeMap::new();
    for (i, line) in lines.enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != want.len() {
            return Err(format!("line {}: {} fields", i + 2, f.len()));
        }
        let num = |s: &str| -> Result<f64, String> {
            s.parse::<f64>()
                .map_err(|e| format!("line {}: {s:?}: {e}", i + 2))
        };
        let sign = match f[3] {
            "PRIJEM" => 1.0,
            "VYDAJ" | "VYBER" => -1.0,
            t => return Err(format!("line {}: type {t:?}", i + 2)),
        };
        let account_id: u64 = f[1].parse().map_err(|e| format!("line {}: {e}", i + 2))?;
        by_account.entry(account_id).or_default().push(Record {
            trans_id: f[0].parse().map_err(|e| format!("line {}: {e}", i + 2))?,
            day: parse_day(f[2])?,
            amount: sign * num(f[5])?,
            balance: num(f[6])?,
            operation: field(f[4]).to_string(),
            k_symbol: field(f[7]).to_string(),
            bank: field(f[8]).to_string(),
            counter_account: field(f[9]).to_string(),
        });
    }
    Ok(by_account
        .into_iter()
        .map(|(id, mut records)| {
            records.sort_by_key(|r| (r.day, r.trans_id));
            let opening = records[0].balance - records[0].amount;
            (
                id,
                Account {
                    id,
                    records,
                    opening,
                },
            )
        })
        .collect())
}

/// Data-quality counts for one account (Table 3).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AccountStats {
    pub records: usize,
    pub days_with_records: usize,
    /// Days whose end-of-day balance (opening + Σ amounts) equals one of the
    /// balances the bank reported that day, to the crown.
    pub days_consistent: usize,
    /// The largest, over days, of the distance from the end-of-day balance
    /// to the nearest balance the bank reported that day (rounding drift).
    pub max_drift: f64,
}

impl Account {
    pub fn first_day(&self) -> Day {
        self.records[0].day
    }

    pub fn last_day(&self) -> Day {
        self.records[self.records.len() - 1].day
    }

    /// End-of-day balance (CZK) for every day `first_day..=to`.
    pub fn daily_balance(&self, to: Day) -> Vec<f64> {
        let first = self.first_day();
        let mut out = Vec::with_capacity((to - first + 1).max(0) as usize);
        let mut b = self.opening;
        let mut i = 0;
        for d in first..=to {
            while i < self.records.len() && self.records[i].day == d {
                b += self.records[i].amount;
                i += 1;
            }
            out.push(b);
        }
        out
    }

    pub fn stats(&self) -> AccountStats {
        let mut s = AccountStats {
            records: self.records.len(),
            ..AccountStats::default()
        };
        let mut b = self.opening;
        let mut i = 0;
        while i < self.records.len() {
            let d = self.records[i].day;
            let mut reported = Vec::new();
            while i < self.records.len() && self.records[i].day == d {
                b += self.records[i].amount;
                reported.push(self.records[i].balance);
                i += 1;
            }
            s.days_with_records += 1;
            let drift = reported
                .iter()
                .map(|&r| (r - b).abs())
                .fold(f64::INFINITY, f64::min);
            s.days_consistent += usize::from(drift < 0.5);
            s.max_drift = s.max_drift.max(drift);
        }
        s
    }

    /// Records on or before `day`.
    pub fn records_through(&self, day: Day) -> &[Record] {
        &self.records[..self.records.partition_point(|r| r.day <= day)]
    }

    /// Number of records in `(day − window, day]`.
    pub fn count_in_window(&self, day: Day, window: Day) -> usize {
        self.records_through(day)
            .iter()
            .filter(|r| r.day > day - window)
            .count()
    }

    /// The account's scale `s` at `origin` (decision 2): the median of
    /// `Σ|amount|` over complete calendar months inside
    /// `first_day..=origin`. `None` if there is no complete month.
    pub fn scale_at(&self, origin: Day) -> Option<f64> {
        let mut months: BTreeMap<(i32, u32), f64> = BTreeMap::new();
        let first = self.first_day();
        for r in self.records_through(origin) {
            let (y, m, _) = civil_from_day(r.day);
            *months.entry((y, m)).or_default() += r.amount.abs();
        }
        // Complete: the month's first day ≥ first_day and its last ≤ origin.
        let mut flows: Vec<f64> = Vec::new();
        let (fy, fm, _) = civil_from_day(first);
        let (mut y, mut m) = (fy, fm);
        loop {
            let start = day_from_civil(y, m, 1);
            let end = day_from_civil(y, m, days_in_month(y, m));
            if end > origin {
                break;
            }
            if start >= first {
                flows.push(months.get(&(y, m)).copied().unwrap_or(0.0));
            }
            (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        }
        median(&mut flows)
    }
}

pub fn median(xs: &mut [f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    let n = xs.len();
    Some(if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    })
}

fn paydays_per_year(s: &PaySchedule) -> f64 {
    match s {
        PaySchedule::Weekly { .. } => 52.0,
        PaySchedule::Biweekly { .. } => 26.0,
        PaySchedule::SemiMonthly { .. } => 24.0,
        PaySchedule::Monthly { .. } => 12.0,
    }
}

/// How the tagging read one snapshot.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tagging {
    pub kinds: Vec<FlowKind>,
    pub streams: Vec<Option<StreamId>>,
    /// The primary income counterparty, if any.
    pub income: Option<String>,
}

/// Tag `records` (one account's records through some day, sorted), using
/// those records only.
pub fn tag(records: &[Record]) -> Tagging {
    // Income: the regular inflow stream with the largest yearly amount.
    struct Stream {
        days: Vec<Day>,
        total: f64,
        pension: bool,
    }
    let mut inflows: BTreeMap<String, Stream> = BTreeMap::new();
    for r in records
        .iter()
        .filter(|r| r.amount > 0.0 && r.k_symbol != "UROK")
    {
        let s = inflows.entry(r.counterparty()).or_insert(Stream {
            days: Vec::new(),
            total: 0.0,
            pension: false,
        });
        s.days.push(r.day);
        s.total += r.amount;
        s.pension |= r.k_symbol == "DUCHOD";
    }
    let mut regular: Vec<(f64, bool, String)> = inflows
        .into_iter()
        .filter_map(|(k, s)| {
            let timing = regular_pay_schedule_in(&s.days, CALENDAR)?;
            let mean = s.total / s.days.len() as f64;
            Some((mean * paydays_per_year(&timing.schedule), s.pension, k))
        })
        .collect();
    regular.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
    let income = regular.first().map(|top| {
        regular
            .iter()
            .take_while(|r| r.0 >= (1.0 - INCOME_TIE) * top.0)
            .find(|r| r.1)
            .unwrap_or(top)
            .2
            .clone()
    });

    // Repeats: days of records sharing counterparty and amount (to the crown).
    let mut repeats: HashMap<(String, i64), Vec<Day>> = HashMap::new();
    for r in records {
        repeats
            .entry((r.counterparty(), r.amount.round() as i64))
            .or_default()
            .push(r.day);
    }

    let mut kinds = Vec::with_capacity(records.len());
    for r in records {
        let cp = r.counterparty();
        let kind = if r.amount > 0.0 && income.as_deref() == Some(cp.as_str()) {
            FlowKind::Income
        } else {
            let days = &repeats[&(cp, r.amount.round() as i64)];
            if (days.len() >= RECURRING_MIN && is_regular_interval(days))
                || BILL_SYMBOLS.contains(&r.k_symbol.as_str())
            {
                FlowKind::Scheduled
            } else {
                FlowKind::Discretionary
            }
        };
        kinds.push(kind);
    }

    // One stream per income or scheduled counterparty, in counterparty order.
    let mut ids: BTreeMap<String, u32> = BTreeMap::new();
    for (r, k) in records.iter().zip(&kinds) {
        if matches!(k, FlowKind::Income | FlowKind::Scheduled) {
            ids.entry(r.counterparty()).or_insert(0);
        }
    }
    for (i, v) in ids.values_mut().enumerate() {
        *v = i as u32 + 1;
    }
    let streams = records
        .iter()
        .zip(&kinds)
        .map(|(r, k)| {
            matches!(k, FlowKind::Income | FlowKind::Scheduled)
                .then(|| StreamId(ids[&r.counterparty()]))
        })
        .collect();
    Tagging {
        kinds,
        streams,
        income,
    }
}

/// The engine's records for `records` tagged by `tagging`, amounts
/// multiplied by `k`.
pub fn transactions(records: &[Record], tagging: &Tagging, k: f64) -> Vec<Transaction> {
    records
        .iter()
        .enumerate()
        .map(|(i, r)| Transaction {
            id: TxnId(r.trans_id),
            account: AccountId(0),
            date: r.day,
            authorized_date: Some(r.day),
            status: TxnStatus::Posted,
            amount: r.amount * k,
            kind: tagging.kinds[i],
            stream: tagging.streams[i],
            pending_id: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "trans_id\taccount_id\tdate\ttype\toperation\tamount\tbalance\tk_symbol\tbank\taccount\n";

    fn ymd(y: i32, m: u32, d: u32) -> Day {
        day_from_civil(y, m, d)
    }

    fn rec(id: u64, day: Day, amount: f64, op: &str, k: &str, bank: &str) -> Record {
        Record {
            trans_id: id,
            day,
            amount,
            balance: 0.0,
            operation: op.into(),
            k_symbol: k.into(),
            bank: bank.into(),
            counter_account: if bank.is_empty() {
                String::new()
            } else {
                "1".into()
            },
        }
    }

    #[test]
    fn parses_signs_nulls_and_order() {
        let tsv = format!(
            "{HEADER}\
             5\t1\t1995-04-13\tPRIJEM\tPREVOD Z UCTU\t3679\t4679\t\\N\tAB\t41403269\n\
             1\t1\t1995-03-24\tPRIJEM\tVKLAD\t1000\t1000\t\\N\t\\N\t\\N\n\
             9\t1\t1995-04-13\tVYDAJ\tVYBER\t200\t4479\t \t\\N\t\\N\n\
             7\t2\t1996-01-02\tVYBER\tVYBER\t50\t-50\t\\N\t\\N\t\\N\n"
        );
        let accounts = parse_trans(&tsv).unwrap();
        let a = &accounts[&1];
        assert_eq!(
            a.records.iter().map(|r| r.trans_id).collect::<Vec<_>>(),
            [1, 5, 9]
        );
        assert_eq!(a.opening, 0.0);
        assert_eq!(a.records[2].amount, -200.0);
        assert_eq!(a.records[2].k_symbol, ""); // " " trims to empty
        assert_eq!(a.records[1].counterparty(), "PREVOD Z UCTU||AB|41403269");
        assert_eq!(accounts[&2].records[0].amount, -50.0);
        assert_eq!(accounts[&2].opening, 0.0);
        let s = a.stats();
        assert_eq!(
            (s.records, s.days_with_records, s.days_consistent),
            (3, 2, 2)
        );
        assert_eq!(s.max_drift, 0.0);
        let b = a.daily_balance(ymd(1995, 4, 14));
        assert_eq!(
            (b[0], b[19], b[20], b[21]),
            (1000.0, 1000.0, 4479.0, 4479.0)
        );
        assert!(parse_trans("bad\theader\n").is_err());
    }

    #[test]
    fn scale_is_the_median_complete_month() {
        let first = ymd(1996, 1, 15);
        let mut records = vec![rec(1, first, 1000.0, "VKLAD", "", "")];
        // Feb: 300 gross; Mar: 500; Apr: 100 (+ partial May).
        for (i, (d, a)) in [
            (ymd(1996, 2, 3), -100.0),
            (ymd(1996, 2, 20), 200.0),
            (ymd(1996, 3, 5), -500.0),
            (ymd(1996, 4, 30), -100.0),
            (ymd(1996, 5, 2), -999.0),
        ]
        .into_iter()
        .enumerate()
        {
            records.push(rec(i as u64 + 2, d, a, "VYBER", "", ""));
        }
        let a = Account {
            id: 1,
            records,
            opening: 0.0,
        };
        // January is incomplete (history starts on the 15th), May is open.
        assert_eq!(a.scale_at(ymd(1996, 5, 10)), Some(300.0));
        assert_eq!(a.scale_at(ymd(1996, 3, 31)), Some(400.0)); // Feb, Mar
        assert_eq!(a.scale_at(ymd(1996, 2, 28)), None); // Feb not yet complete
        assert_eq!(a.scale_at(ymd(1996, 2, 29)), Some(300.0));
    }

    #[test]
    fn tagging_picks_regular_income_bills_and_spending() {
        let mut r = Vec::new();
        let mut id = 0;
        let mut push = |r: &mut Vec<Record>, d, a, op: &str, k: &str, bank: &str| {
            id += 1;
            r.push(rec(id, d, a, op, k, bank));
        };
        for m in 1..=8 {
            // Salary on the 10th (whatever the weekday), pension on
            // the 15th, smaller; household (SIPO) on the 20th; cash at random.
            let sal = CALENDAR.pay_date(ymd(1996, m, 10));
            push(
                &mut r,
                sal,
                20_000.0 + f64::from(m),
                "PREVOD Z UCTU",
                "",
                "AB",
            );
            push(
                &mut r,
                ymd(1996, m, 15),
                6_000.0,
                "PREVOD Z UCTU",
                "DUCHOD",
                "CD",
            );
            push(
                &mut r,
                ymd(1996, m, 20),
                -3_000.0,
                "PREVOD NA UCET",
                "SIPO",
                "EF",
            );
            push(
                &mut r,
                ymd(1996, m, 3 + m),
                -800.0 - f64::from(m),
                "VYBER",
                "",
                "",
            );
            push(
                &mut r,
                ymd(1996, m, 28),
                12.0 + f64::from(m),
                "",
                "UROK",
                "",
            );
        }
        r.sort_by_key(|x| (x.day, x.trans_id));
        let t = tag(&r);
        assert_eq!(t.income.as_deref(), Some("PREVOD Z UCTU||AB|1"));
        for (x, k) in r.iter().zip(&t.kinds) {
            let want = match (x.k_symbol.as_str(), x.operation.as_str()) {
                ("", "PREVOD Z UCTU") => FlowKind::Income,
                // A regular same-amount inflow that is not the primary
                // income is a scheduled stream, as for Plaid.
                ("DUCHOD", _) | ("SIPO", _) | ("UROK", _) => FlowKind::Scheduled,
                _ => FlowKind::Discretionary,
            };
            assert_eq!(*k, want, "{x:?}");
        }
        // Streams: one per income/scheduled counterparty, none for cash.
        let distinct: std::collections::BTreeSet<_> = t.streams.iter().flatten().collect();
        assert_eq!(distinct.len(), 4);
        assert!(
            r.iter()
                .zip(&t.streams)
                .all(|(x, s)| (x.operation == "VYBER") == s.is_none())
        );
    }

    #[test]
    fn a_pension_wins_a_near_tie() {
        let mut r = Vec::new();
        for m in 1..=6u32 {
            let (a, b) = (
                CALENDAR.pay_date(ymd(1997, m, 5)),
                CALENDAR.pay_date(ymd(1997, m, 12)),
            );
            r.push(rec(
                2 * u64::from(m),
                a,
                10_000.0,
                "PREVOD Z UCTU",
                "",
                "AB",
            ));
            r.push(rec(
                2 * u64::from(m) + 1,
                b,
                9_500.0,
                "PREVOD Z UCTU",
                "DUCHOD",
                "CD",
            ));
        }
        r.sort_by_key(|x| (x.day, x.trans_id));
        assert_eq!(tag(&r).income.as_deref(), Some("PREVOD Z UCTU|DUCHOD|CD|1"));
    }
}
