//! Minimal recurrence detection for the forecaster: when the primary paycheck
//! and each recurring bill will land, with their timing noise.
//!
//! **Income.** Every [`PaySchedule`] candidate (weekly and biweekly anchors,
//! every day of month, every pair of days) is scored against the observed
//! paydays: each payday costs the smallest |offset| `k` with
//! `pay_date(nominal + k) == payday` (`|k| <= 2`), or a miss penalty if none
//! works, and each nominal date inside the observed span with no paycheck costs
//! a miss too. The cheapest candidate wins. The fitted offsets become an
//! empirical distribution, smoothed toward a prior centered on zero; the
//! business-day rollback itself is deterministic and applied again when the
//! forecaster samples future paydays.
//!
//! **Bills.** For each recurring stream (a `Scheduled` record with a stream id)
//! with at least two charges: period = median gap, amount = latest posted
//! amount. Then the dates are read in one of two ways:
//! - **Day of month** ([`MonthlyAnchor`]), as with rent on the 1st: the
//!   median gap is 28–31 days and at least 80% of at least
//!   [`MONTHLY_MIN_CHARGES`] charges fall exactly on one day of the month
//!   (clamped to month end), moved by one [`DayShift`]. The shift is none for
//!   card charges, or the previous or next business day for ACH debits that
//!   avoid weekends and holidays. Future dates are then deterministic.
//! - **Every N days** otherwise: drift = empirical distribution of
//!   `gap − period`, persistent, so the phase random-walks.
//!
//! **Regularity tests for tagging.** A bank feed does not say which inflow is
//! the paycheck or which outflow is a bill, so the adapter asks:
//! - [`regular_pay_schedule`]: does a series of inflow days fit some
//!   [`PaySchedule`] at under one day of error per payday (a miss costs
//!   [`MISS_COST`])?
//! - [`is_regular_interval`]: are the gaps between charges at least weekly
//!   and tightly clustered around their median?
//!
//! Not handled yet: streams with one charge, secondary income streams.

use std::collections::{BTreeMap, HashSet};

use rand::RngExt;

use crate::calendar::{HolidayCalendar, PaySchedule};
use crate::types::{Day, FlowKind, StreamId, Transaction, TxnStatus};

/// Largest timing offset the detector and forecaster model, in days.
pub const MAX_OFFSET: i32 = 2;
const OFFSETS: usize = (2 * MAX_OFFSET + 1) as usize;
/// Prior over offsets −2..=2 and its strength in pseudo-observations.
const OFFSET_PRIOR: [f64; OFFSETS] = [0.025, 0.1, 0.75, 0.1, 0.025];
const OFFSET_PRIOR_WEIGHT: f64 = 2.0;
/// Fit cost of a payday no nominal date explains, or of a nominal date with
/// no payday.
pub const MISS_COST: u32 = 10;
/// Paydays used for fitting (about a year of biweekly pay).
const MAX_PAYDAYS: usize = 26;
/// Fewest charges for a day-of-month reading of a bill stream.
pub const MONTHLY_MIN_CHARGES: usize = 3;
/// Fewest paydays for [`regular_pay_schedule`].
pub const REGULAR_MIN_PAYDAYS: usize = 3;
/// Largest mean fit cost per payday that still counts as regular income.
pub const REGULAR_MAX_COST_PER_PAYDAY: f64 = 1.0;
/// Shortest median gap for [`is_regular_interval`]: bills are at least weekly.
pub const REGULAR_MIN_PERIOD: i32 = 7;
/// Share of gaps that must sit within tolerance of the median gap.
const REGULAR_GAP_SHARE: f64 = 0.8;

/// A smoothed distribution over day offsets `−MAX_OFFSET..=MAX_OFFSET`.
#[derive(Debug, Clone, PartialEq)]
pub struct OffsetDist {
    probs: [f64; OFFSETS],
}

impl OffsetDist {
    /// Empirical offsets (clamped to ±MAX_OFFSET) mixed with the prior.
    pub fn from_offsets(offsets: &[i32]) -> Self {
        let mut counts = OFFSET_PRIOR.map(|p| p * OFFSET_PRIOR_WEIGHT);
        for &k in offsets {
            counts[(k.clamp(-MAX_OFFSET, MAX_OFFSET) + MAX_OFFSET) as usize] += 1.0;
        }
        let total: f64 = counts.iter().sum();
        Self {
            probs: counts.map(|c| c / total),
        }
    }

    /// All mass on offset 0.
    pub fn exact() -> Self {
        let mut probs = [0.0; OFFSETS];
        probs[MAX_OFFSET as usize] = 1.0;
        Self { probs }
    }

    /// Probabilities for offsets −2, −1, 0, 1, 2.
    pub fn probabilities(&self) -> &[f64; OFFSETS] {
        &self.probs
    }

    pub fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> i32 {
        let u: f64 = rng.random();
        let mut acc = 0.0;
        for (i, p) in self.probs.iter().enumerate() {
            acc += p;
            if u < acc {
                return i as i32 - MAX_OFFSET;
            }
        }
        MAX_OFFSET
    }
}

/// When the primary paycheck lands.
#[derive(Debug, Clone, PartialEq)]
pub struct PayTiming {
    pub schedule: PaySchedule,
    /// Nominal date of the most recent observed paycheck; later nominal
    /// dates are still to come.
    pub last_paid_nominal: Day,
    pub offsets: OffsetDist,
    /// Fit cost of the chosen schedule (0 = every payday explained exactly).
    pub fit_cost: u32,
    /// The business-day calendar paydays roll back by.
    pub calendar: HolidayCalendar,
}

impl PayTiming {
    /// The day a paycheck nominally due on `nominal` lands.
    pub fn pay_date(&self, nominal: Day) -> Day {
        self.calendar.pay_date(nominal)
    }
}

/// How a bill's nominal date moves when it is not a business day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayShift {
    /// Charged on the nominal day regardless (card charges).
    None,
    PreviousBusinessDay,
    NextBusinessDay,
}

impl DayShift {
    /// Under the US calendar.
    pub fn apply(self, nominal: Day) -> Day {
        self.apply_in(nominal, HolidayCalendar::Us)
    }

    pub fn apply_in(self, nominal: Day, calendar: HolidayCalendar) -> Day {
        match self {
            DayShift::None => nominal,
            DayShift::PreviousBusinessDay => calendar.on_or_before(nominal),
            DayShift::NextBusinessDay => calendar.on_or_after(nominal),
        }
    }
}

/// A bill charged on a fixed day of the month.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyAnchor {
    /// 1–31; a day past the end of a month means its last day.
    pub day: u32,
    pub shift: DayShift,
    /// The business-day calendar `shift` moves by.
    pub calendar: HolidayCalendar,
}

impl MonthlyAnchor {
    /// Charge days whose nominal date is in `from..=to`, ascending.
    pub fn charge_days(&self, from: Day, to: Day) -> Vec<Day> {
        PaySchedule::Monthly { day: self.day }
            .nominal_dates(from, to)
            .into_iter()
            .map(|n| self.shift.apply_in(n, self.calendar))
            .collect()
    }

    /// The first charge day strictly after `after`.
    pub fn next_after(&self, after: Day) -> Day {
        // A shift moves a date by at most a few days, so nominal dates from
        // a week before `after` to two months after cover the answer.
        self.charge_days(after - 7, after + 62)
            .into_iter()
            .find(|&d| d > after)
            .unwrap_or(after + 31)
    }

    fn explains(&self, day: Day) -> bool {
        self.charge_days(day - 7, day + 7).contains(&day)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BillStream {
    pub stream: StreamId,
    /// Median gap between charges.
    pub period_days: u32,
    pub last_day: Day,
    /// Signed, as charged (negative for an outflow).
    pub amount: f64,
    /// For an every-N-days stream: persistent drift of each gap from the period.
    pub drift: OffsetDist,
    /// Set when the stream is charged on a fixed day of the month; future
    /// dates then follow it instead of `period_days` and `drift`.
    pub day_of_month: Option<MonthlyAnchor>,
}

impl BillStream {
    /// The nominal next charge after `last_day`: by day of month if anchored,
    /// otherwise `last_day + period_days` before drift.
    pub fn next_due(&self) -> Day {
        match self.day_of_month {
            Some(a) => a.next_after(self.last_day),
            None => self.last_day + self.period_days as Day,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Schedule {
    pub income: Option<PayTiming>,
    pub bills: Vec<BillStream>,
}

/// Detect recurring flows from the records in a feed snapshot, under the US
/// business-day calendar.
pub fn detect_schedule(txns: &[Transaction]) -> Schedule {
    detect_schedule_in(txns, HolidayCalendar::Us)
}

/// [`detect_schedule`] under a given business-day calendar.
pub fn detect_schedule_in(txns: &[Transaction], calendar: HolidayCalendar) -> Schedule {
    Schedule {
        income: detect_income(txns, calendar),
        bills: detect_bills(txns, calendar),
    }
}

/// The smallest-|k| offset that explains `payday` under `schedule`, with the
/// nominal date it belongs to.
fn explain(schedule: &PaySchedule, payday: Day, cal: HolidayCalendar) -> Option<(Day, i32)> {
    let nominals = schedule.nominal_dates(payday - MAX_OFFSET - 4, payday + MAX_OFFSET + 4);
    let mut best: Option<(Day, i32)> = None;
    for n in nominals {
        for k in -MAX_OFFSET..=MAX_OFFSET {
            if cal.pay_date(n + k) == payday && best.is_none_or(|(_, b)| k.abs() < b.abs()) {
                best = Some((n, k));
            }
        }
    }
    best
}

struct Fit {
    cost: u32,
    last_nominal: Day,
    offsets: Vec<i32>,
}

fn fit(schedule: &PaySchedule, paydays: &[Day], cal: HolidayCalendar) -> Option<Fit> {
    let mut cost = 0;
    let mut offsets = Vec::with_capacity(paydays.len());
    let mut used = HashSet::new();
    let mut last_nominal = None;
    for &d in paydays {
        match explain(schedule, d, cal) {
            Some((n, k)) if used.insert(n) => {
                cost += k.unsigned_abs();
                offsets.push(k);
                last_nominal = Some(n);
            }
            _ => cost += MISS_COST,
        }
    }
    let (lo, hi) = (*used.iter().min()?, *used.iter().max()?);
    let missed = schedule
        .nominal_dates(lo, hi)
        .into_iter()
        .filter(|n| !used.contains(n))
        .count() as u32;
    Some(Fit {
        cost: cost + MISS_COST * missed,
        last_nominal: last_nominal?,
        offsets,
    })
}

fn candidates(last: Day) -> Vec<PaySchedule> {
    let mut out = Vec::new();
    out.extend((0..7).map(|k| PaySchedule::Weekly { anchor: last - k }));
    out.extend((0..14).map(|k| PaySchedule::Biweekly { anchor: last - k }));
    out.extend((1..=31).map(|day| PaySchedule::Monthly { day }));
    for a in 1..=31 {
        for b in a + 1..=31 {
            out.push(PaySchedule::SemiMonthly { days: [a, b] });
        }
    }
    out
}

fn detect_income(txns: &[Transaction], cal: HolidayCalendar) -> Option<PayTiming> {
    let paydays: Vec<Day> = txns
        .iter()
        .filter(|t| t.kind == FlowKind::Income && t.status == TxnStatus::Posted)
        .map(Transaction::day)
        .collect();
    fit_pay_schedule_in(&paydays, cal)
}

/// The cheapest [`PaySchedule`] for a series of paydays (any order,
/// duplicates ignored), fitted to the last 26, under the US calendar. `None`
/// with fewer than two.
pub fn fit_pay_schedule(paydays: &[Day]) -> Option<PayTiming> {
    fit_pay_schedule_in(paydays, HolidayCalendar::Us)
}

/// [`fit_pay_schedule`] under a given business-day calendar.
pub fn fit_pay_schedule_in(paydays: &[Day], calendar: HolidayCalendar) -> Option<PayTiming> {
    let mut paydays = paydays.to_vec();
    paydays.sort_unstable();
    paydays.dedup();
    if paydays.len() < 2 {
        return None;
    }
    let paydays = &paydays[paydays.len().saturating_sub(MAX_PAYDAYS)..];
    let last = *paydays.last()?;

    let mut best: Option<(PaySchedule, Fit)> = None;
    for c in candidates(last) {
        if let Some(f) = fit(&c, paydays, calendar)
            && best.as_ref().is_none_or(|(_, b)| f.cost < b.cost)
        {
            best = Some((c, f));
        }
    }
    let (schedule, f) = best?;
    Some(PayTiming {
        schedule,
        last_paid_nominal: f.last_nominal,
        offsets: OffsetDist::from_offsets(&f.offsets),
        fit_cost: f.cost,
        calendar,
    })
}

/// The pay schedule of a regular income stream: at least
/// [`REGULAR_MIN_PAYDAYS`] distinct days whose best fit costs at most
/// [`REGULAR_MAX_COST_PER_PAYDAY`] per payday fitted. One missed paycheck in
/// ten already fails; a day of jitter on every paycheck does not.
pub fn regular_pay_schedule(paydays: &[Day]) -> Option<PayTiming> {
    regular_pay_schedule_in(paydays, HolidayCalendar::Us)
}

/// [`regular_pay_schedule`] under a given business-day calendar.
pub fn regular_pay_schedule_in(paydays: &[Day], calendar: HolidayCalendar) -> Option<PayTiming> {
    let mut distinct = paydays.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() < REGULAR_MIN_PAYDAYS {
        return None;
    }
    let timing = fit_pay_schedule_in(&distinct, calendar)?;
    let fitted = distinct.len().min(MAX_PAYDAYS) as f64;
    (f64::from(timing.fit_cost) <= REGULAR_MAX_COST_PER_PAYDAY * fitted).then_some(timing)
}

/// Whether charges on `days` (any order) recur on a regular interval: at
/// least two gaps, a median gap of at least [`REGULAR_MIN_PERIOD`] days, and
/// at least 80% of gaps within `max(3, 15% of the median)` days of it. A
/// monthly bill (gaps 28–31, or a little more with business-day shifts)
/// passes, and so does a monthly bill that skipped one month in five; a daily
/// coffee, or a shop visited a few times at random, does not.
pub fn is_regular_interval(days: &[Day]) -> bool {
    let mut days = days.to_vec();
    days.sort_unstable();
    days.dedup();
    let mut gaps: Vec<i32> = days.windows(2).map(|w| w[1] - w[0]).collect();
    if gaps.len() < 2 {
        return false;
    }
    gaps.sort_unstable();
    let median = gaps[gaps.len() / 2];
    if median < REGULAR_MIN_PERIOD {
        return false;
    }
    let tol = 3.max((0.15 * f64::from(median)).round() as i32);
    let close = gaps.iter().filter(|&&g| (g - median).abs() <= tol).count();
    close as f64 >= REGULAR_GAP_SHARE * gaps.len() as f64
}

fn detect_bills(txns: &[Transaction], cal: HolidayCalendar) -> Vec<BillStream> {
    let mut by_stream: BTreeMap<StreamId, Vec<&Transaction>> = BTreeMap::new();
    for t in txns.iter().filter(|t| t.kind == FlowKind::Scheduled) {
        if let Some(s) = t.stream {
            by_stream.entry(s).or_default().push(t);
        }
    }
    by_stream
        .into_iter()
        .filter_map(|(stream, mut charges)| {
            charges.sort_by_key(|t| (t.day(), t.id));
            let days: Vec<Day> = charges.iter().map(|t| t.day()).collect();
            let mut gaps: Vec<i32> = days.windows(2).map(|w| w[1] - w[0]).collect();
            if gaps.is_empty() {
                return None;
            }
            let drift_from: Vec<i32> = gaps.clone();
            gaps.sort_unstable();
            let period = gaps[gaps.len() / 2].max(1);
            let amount = charges
                .iter()
                .rev()
                .find(|t| t.status == TxnStatus::Posted)
                .unwrap_or(charges.last()?)
                .amount;
            let drift: Vec<i32> = drift_from.iter().map(|g| g - period).collect();
            Some(BillStream {
                stream,
                period_days: period as u32,
                last_day: *days.last()?,
                amount,
                drift: OffsetDist::from_offsets(&drift),
                day_of_month: (28..=31)
                    .contains(&period)
                    .then(|| detect_day_of_month(&days, cal))
                    .flatten(),
            })
        })
        .collect()
}

/// The (day, shift) that explains the most charge days, if it explains at
/// least 80% of them. Ties go to no shift, then to the earlier day.
fn detect_day_of_month(days: &[Day], calendar: HolidayCalendar) -> Option<MonthlyAnchor> {
    if days.len() < MONTHLY_MIN_CHARGES {
        return None;
    }
    let mut best: Option<(usize, MonthlyAnchor)> = None;
    for shift in [
        DayShift::None,
        DayShift::PreviousBusinessDay,
        DayShift::NextBusinessDay,
    ] {
        for day in 1..=31 {
            let anchor = MonthlyAnchor {
                day,
                shift,
                calendar,
            };
            let hits = days.iter().filter(|&&d| anchor.explains(d)).count();
            if best.is_none_or(|(b, _)| hits > b) {
                best = Some((hits, anchor));
            }
        }
    }
    let (hits, anchor) = best?;
    (hits * 5 >= days.len() * 4).then_some(anchor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::day_from_civil as ymd;
    use crate::synth::{SynthConfig, generate};
    use crate::types::{AccountId, TxnId};
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn pay(id: u64, day: Day) -> Transaction {
        Transaction {
            id: TxnId(id),
            account: AccountId(0),
            date: day,
            authorized_date: Some(day),
            status: TxnStatus::Posted,
            amount: 2000.0,
            kind: FlowKind::Income,
            stream: None,
            pending_id: None,
        }
    }

    fn paydays(schedule: PaySchedule, from: Day, to: Day) -> Vec<Transaction> {
        schedule
            .nominal_dates(from, to)
            .into_iter()
            .map(PaySchedule::pay_date)
            .enumerate()
            .map(|(i, d)| pay(i as u64, d))
            .collect()
    }

    fn detected(txns: &[Transaction]) -> PayTiming {
        detect_schedule(txns).income.expect("income detected")
    }

    #[test]
    fn detects_each_schedule_kind_exactly() {
        let (from, to) = (ymd(2024, 1, 1), ymd(2024, 12, 31));
        let friday = ymd(2024, 1, 5);
        for schedule in [
            PaySchedule::Weekly { anchor: friday },
            PaySchedule::Biweekly { anchor: friday },
            PaySchedule::SemiMonthly { days: [1, 15] },
            PaySchedule::SemiMonthly { days: [15, 31] },
            PaySchedule::Monthly { day: 25 },
        ] {
            let timing = detected(&paydays(schedule, from, to));
            assert_eq!(timing.fit_cost, 0, "{schedule:?}");
            // Same nominal dates (weekly/biweekly anchors may differ by whole periods).
            assert_eq!(
                timing.schedule.nominal_dates(from, to),
                schedule.nominal_dates(from, to),
                "{schedule:?}"
            );
        }
    }

    #[test]
    fn weekend_and_holiday_paydays_are_explained_by_the_rollback() {
        // 15th/last-day pay in 2024: June 15 (Sat) paid Fri 14th, and so on.
        let timing = detected(&paydays(
            PaySchedule::SemiMonthly { days: [15, 31] },
            ymd(2024, 1, 1),
            ymd(2024, 12, 31),
        ));
        assert_eq!(timing.fit_cost, 0);
        // All offsets were 0, so the distribution is the prior pulled toward 0.
        let p = timing.offsets.probabilities();
        assert!(p[2] > 0.95, "{p:?}");
        assert_eq!(timing.last_paid_nominal, ymd(2024, 12, 31));
    }

    #[test]
    fn jittered_paydays_yield_offsets() {
        let mut txns = paydays(
            PaySchedule::Biweekly {
                anchor: ymd(2024, 1, 5),
            },
            ymd(2024, 1, 1),
            ymd(2024, 6, 30),
        );
        // Two paychecks arrive a day early, on Thursday.
        txns[2].date -= 1;
        txns[2].authorized_date = Some(txns[2].date);
        txns[5].date -= 1;
        txns[5].authorized_date = Some(txns[5].date);
        let timing = detected(&txns);
        assert_eq!(timing.fit_cost, 2);
        assert!(matches!(timing.schedule, PaySchedule::Biweekly { .. }));
        let p = timing.offsets.probabilities();
        assert!(p[1] > 0.1 && p[1] > p[3], "{p:?}");
    }

    #[test]
    fn too_few_paychecks_means_no_income_timing() {
        assert!(detect_schedule(&[pay(0, ymd(2024, 1, 5))]).income.is_none());
        assert!(detect_schedule(&[]).income.is_none());
    }

    #[test]
    fn detects_synthetic_income_and_bills() {
        let cfg = SynthConfig::realistic(ymd(2024, 1, 1));
        let run = generate(&cfg, 3).unwrap();
        let s = detect_schedule(&run.snapshot(ymd(2024, 12, 31)));
        let timing = s.income.unwrap();
        assert!(matches!(timing.schedule, PaySchedule::Biweekly { .. }));
        assert_eq!(
            timing
                .schedule
                .nominal_dates(ymd(2024, 1, 1), ymd(2024, 12, 31)),
            cfg.income
                .schedule
                .nominal_dates(ymd(2024, 1, 1), ymd(2024, 12, 31))
        );
        assert_eq!(s.bills.len(), cfg.subscriptions.len());
        for (bill, sub) in s.bills.iter().zip(&cfg.subscriptions) {
            assert_eq!(bill.period_days, sub.period_days);
            assert_eq!(bill.amount, -sub.amount);
            assert_eq!(bill.day_of_month, None, "30-day streams drift off any day");
        }
    }

    #[test]
    fn bill_period_drift_and_amount() {
        let mk = |id: u64, day: Day, amount: f64, status: TxnStatus| Transaction {
            id: TxnId(id),
            account: AccountId(0),
            date: day,
            authorized_date: Some(day),
            status,
            amount,
            kind: FlowKind::Scheduled,
            stream: Some(StreamId(7)),
            pending_id: None,
        };
        let txns = [
            mk(1, 100, -15.0, TxnStatus::Posted),
            mk(2, 130, -15.0, TxnStatus::Posted),
            mk(3, 161, -16.0, TxnStatus::Posted),
            mk(4, 191, -18.0, TxnStatus::Pending),
        ];
        let bills = detect_schedule(&txns).bills;
        assert_eq!(bills.len(), 1);
        let b = &bills[0];
        assert_eq!((b.period_days, b.last_day), (30, 191));
        assert_eq!(b.amount, -16.0, "latest posted amount, not the pending one");
        // Gaps 30, 31, 30 → drift 0, +1, 0.
        let p = b.drift.probabilities();
        assert!(p[3] > OFFSET_PRIOR[3] && p[2] > p[3]);
    }

    fn bill(id: u64, day: Day) -> Transaction {
        Transaction {
            id: TxnId(id),
            account: AccountId(0),
            date: day,
            authorized_date: Some(day),
            status: TxnStatus::Posted,
            amount: -1500.0,
            kind: FlowKind::Scheduled,
            stream: Some(StreamId(1)),
            pending_id: None,
        }
    }

    fn anchor_of(days: &[Day]) -> Option<MonthlyAnchor> {
        let txns: Vec<Transaction> = days
            .iter()
            .enumerate()
            .map(|(i, &d)| bill(i as u64, d))
            .collect();
        detect_schedule(&txns).bills[0].day_of_month
    }

    #[test]
    fn day_shifts_by_hand() {
        // 2024-06-01 is a Saturday; 2024-07-04 a Thursday holiday.
        let sat = ymd(2024, 6, 1);
        assert_eq!(DayShift::None.apply(sat), sat);
        assert_eq!(DayShift::PreviousBusinessDay.apply(sat), ymd(2024, 5, 31));
        assert_eq!(DayShift::NextBusinessDay.apply(sat), ymd(2024, 6, 3));
        assert_eq!(
            DayShift::NextBusinessDay.apply(ymd(2024, 7, 4)),
            ymd(2024, 7, 5)
        );

        let rent = MonthlyAnchor {
            day: 1,
            shift: DayShift::NextBusinessDay,
            calendar: HolidayCalendar::Us,
        };
        assert_eq!(rent.next_after(ymd(2024, 5, 31)), ymd(2024, 6, 3));
        assert_eq!(rent.next_after(ymd(2024, 6, 3)), ymd(2024, 7, 1));
        // Month-end clamp: the 31st in June is June 30, a Sunday.
        let end = MonthlyAnchor {
            day: 31,
            shift: DayShift::PreviousBusinessDay,
            calendar: HolidayCalendar::Us,
        };
        assert_eq!(end.next_after(ymd(2024, 6, 1)), ymd(2024, 6, 28));
        assert_eq!(end.next_after(ymd(2024, 6, 28)), ymd(2024, 7, 31));
    }

    #[test]
    fn rent_on_the_first_with_weekend_shifts() {
        for shift in [DayShift::NextBusinessDay, DayShift::PreviousBusinessDay] {
            let days: Vec<Day> = (1..=12).map(|m| shift.apply(ymd(2024, m, 1))).collect();
            // Several firsts of 2024 fall on weekends or holidays.
            assert!(days.iter().filter(|&&d| civil_dom(d) != 1).count() >= 3);
            assert_eq!(
                anchor_of(&days),
                Some(MonthlyAnchor {
                    day: 1,
                    shift,
                    calendar: HolidayCalendar::Us
                })
            );
        }
    }

    #[test]
    fn card_charge_on_the_31st_is_clamped_not_shifted() {
        let days: Vec<Day> = (1..=8)
            .map(|m| {
                let last = crate::calendar::days_in_month(2024, m);
                ymd(2024, m, last.min(31))
            })
            .collect();
        assert_eq!(
            anchor_of(&days),
            Some(MonthlyAnchor {
                day: 31,
                shift: DayShift::None,
                calendar: HolidayCalendar::Us,
            })
        );
    }

    #[test]
    fn thirty_day_streams_are_not_day_of_month() {
        let days: Vec<Day> = (0..12).map(|k| ymd(2024, 1, 5) + 30 * k).collect();
        let txns: Vec<Transaction> = days
            .iter()
            .enumerate()
            .map(|(i, &d)| bill(i as u64, d))
            .collect();
        let b = &detect_schedule(&txns).bills[0];
        assert_eq!((b.period_days, b.day_of_month), (30, None));
        assert_eq!(b.next_due(), days[11] + 30);
        // Two charges are too few to call it either way.
        assert_eq!(anchor_of(&[ymd(2024, 1, 21), ymd(2024, 2, 21)]), None);
    }

    #[test]
    fn one_late_charge_in_ten_is_tolerated() {
        let mut days: Vec<Day> = (1..=10).map(|m| ymd(2024, m, 21)).collect();
        days[4] += 2;
        let a = anchor_of(&days).unwrap();
        assert_eq!(a.day, 21);
        let b = detect_schedule(
            &days
                .iter()
                .enumerate()
                .map(|(i, &d)| bill(i as u64, d))
                .collect::<Vec<_>>(),
        )
        .bills[0]
            .next_due();
        assert_eq!(b, a.shift.apply(ymd(2024, 11, 21)));
    }

    fn civil_dom(d: Day) -> u32 {
        crate::calendar::civil_from_day(d).2
    }

    #[test]
    fn regular_pay_schedules() {
        let friday = ymd(2024, 1, 5);
        let biweekly: Vec<Day> = PaySchedule::Biweekly { anchor: friday }
            .nominal_dates(ymd(2024, 1, 1), ymd(2024, 6, 30))
            .into_iter()
            .map(PaySchedule::pay_date)
            .collect();
        let t = regular_pay_schedule(&biweekly).expect("regular");
        assert!(matches!(t.schedule, PaySchedule::Biweekly { .. }));
        // Order and duplicates do not matter.
        let mut shuffled = biweekly.clone();
        shuffled.reverse();
        shuffled.push(biweekly[3]);
        assert_eq!(regular_pay_schedule(&shuffled), Some(t));

        // A day of jitter on every other paycheck: cost 0.5 per payday.
        let jittered: Vec<Day> = biweekly
            .iter()
            .enumerate()
            .map(|(i, &d)| if i % 2 == 1 { d - 1 } else { d })
            .collect();
        assert!(regular_pay_schedule(&jittered).is_some());

        // Too few, or scattered at random: not regular.
        assert!(regular_pay_schedule(&biweekly[..2]).is_none());
        let scattered = [3, 11, 30, 34, 61, 99, 104, 140].map(|k| friday + k);
        assert!(regular_pay_schedule(&scattered).is_none());
        // One missed paycheck in six costs 10 / 5 = 2 per payday: not regular.
        let mut missing = biweekly[..6].to_vec();
        missing.remove(3);
        assert!(regular_pay_schedule(&missing).is_none());
    }

    #[test]
    fn regular_intervals() {
        let monthly: Vec<Day> = (1..=6).map(|m| ymd(2024, m, 21)).collect();
        assert!(is_regular_interval(&monthly));
        // Rent moved to the next business day: gaps 29–33.
        let rent: Vec<Day> = (1..=12)
            .map(|m| DayShift::NextBusinessDay.apply(ymd(2024, m, 1)))
            .collect();
        assert!(is_regular_interval(&rent));
        // One skipped month in six gaps (one gap of ~60) is tolerated.
        let skipped: Vec<Day> = [1, 2, 3, 5, 6, 7, 8].map(|m| ymd(2024, m, 21)).to_vec();
        assert!(is_regular_interval(&skipped));
        assert!(is_regular_interval(&[0, 7, 14, 21]), "weekly");

        // A coffee every weekday, or every day: too frequent to be a bill.
        let weekdays: Vec<Day> = (0..28)
            .map(|k| ymd(2024, 1, 1) + k)
            .filter(|&d| crate::calendar::weekday(d) < 5)
            .collect();
        assert!(!is_regular_interval(&weekdays));
        assert!(!is_regular_interval(&(0..30).collect::<Vec<Day>>()));
        // The same shop on scattered days.
        assert!(!is_regular_interval(&[0, 5, 17, 20, 41]));
        // Two charges give one gap: not enough to judge.
        assert!(!is_regular_interval(&[0, 30]));
    }

    #[test]
    fn offset_sampling_matches_probabilities() {
        let dist = OffsetDist::from_offsets(&[1, 1, 1, 0]);
        let mut rng = ChaCha8Rng::seed_from_u64(2);
        let mut counts = [0usize; OFFSETS];
        for _ in 0..60_000 {
            counts[(dist.sample(&mut rng) + MAX_OFFSET) as usize] += 1;
        }
        for (c, p) in counts.iter().zip(dist.probabilities()) {
            assert!((*c as f64 / 60_000.0 - p).abs() < 0.01);
        }
    }

    #[test]
    fn paydays_fit_exactly_only_under_their_own_calendar() {
        // Paid on the 8th, rolled back by Czech holidays: 1996-05-08 is
        // Liberation Day (a Wednesday), so May pays on the 7th.
        let cz = HolidayCalendar::Cz;
        let days: Vec<Day> = PaySchedule::Monthly { day: 8 }
            .nominal_dates(ymd(1996, 1, 1), ymd(1996, 12, 31))
            .into_iter()
            .map(|n| cz.pay_date(n))
            .collect();
        assert!(days.contains(&ymd(1996, 5, 7)));
        let txns: Vec<Transaction> = days
            .iter()
            .enumerate()
            .map(|(i, &d)| pay(i as u64, d))
            .collect();
        let under_cz = detect_schedule_in(&txns, cz).income.unwrap();
        assert_eq!(under_cz.schedule, PaySchedule::Monthly { day: 8 });
        assert_eq!((under_cz.fit_cost, under_cz.calendar), (0, cz));
        // The US calendar has to explain 7 May as a day early.
        let under_us = detect_schedule(&txns).income.unwrap();
        assert_eq!(under_us.schedule, PaySchedule::Monthly { day: 8 });
        assert!(under_us.fit_cost > 0);
        assert_eq!(under_us.calendar, HolidayCalendar::Us);
    }
}
