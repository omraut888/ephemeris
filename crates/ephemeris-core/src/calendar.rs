//! Calendar arithmetic on [`Day`] indices: civil dates, business days, and pay
//! schedules (design §1, "Pay schedules").
//!
//! Business days follow the US Federal Reserve holiday calendar: a fixed-date
//! holiday on a Sunday is observed the following Monday, and one on a Saturday
//! is not observed (banks open the Friday before).

use crate::error::EngineError;
use crate::types::Day;

/// 0 = Monday … 6 = Sunday. 1970-01-01 was a Thursday.
pub fn weekday(day: Day) -> u32 {
    (i64::from(day) + 3).rem_euclid(7) as u32
}

pub fn is_weekend(day: Day) -> bool {
    weekday(day) >= 5
}

/// Day index of a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
pub fn day_from_civil(year: i32, month: u32, dom: u32) -> Day {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(dom) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as Day
}

/// `(year, month, day-of-month)` of a day index (H. Hinnant's `civil_from_days`).
pub fn civil_from_day(day: Day) -> (i32, u32, u32) {
    let z = i64::from(day) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dom = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    (year, month, dom)
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Whether `day` is a Federal Reserve bank holiday (as observed).
pub fn is_bank_holiday(day: Day) -> bool {
    let (year, month, dom) = civil_from_day(day);
    let wd = weekday(day);
    // Fixed-date holidays falling on Sunday are observed on Monday.
    let fixed_on = |d: Day| {
        let (y, m, dom) = civil_from_day(d);
        matches!((m, dom), (1, 1) | (7, 4) | (11, 11) | (12, 25))
            || (y >= 2022 && (m, dom) == (6, 19))
    };
    if wd < 5 && fixed_on(day) {
        return true;
    }
    if wd == 0 && fixed_on(day - 1) {
        return true;
    }
    // Floating holidays: the n-th given weekday of a month.
    let nth = |n: u32| dom > 7 * (n - 1) && dom <= 7 * n;
    match (month, wd) {
        (1, 0) | (2, 0) => nth(3),                  // MLK Day, Presidents Day
        (5, 0) => dom + 7 > days_in_month(year, 5), // Memorial Day: last Monday
        (9, 0) => nth(1),                           // Labor Day
        (10, 0) => nth(2),                          // Columbus Day
        (11, 3) => nth(4),                          // Thanksgiving
        _ => false,
    }
}

pub fn is_business_day(day: Day) -> bool {
    !is_weekend(day) && !is_bank_holiday(day)
}

/// The latest business day `<= day`. Payroll moves weekend and holiday paydays earlier.
pub fn business_day_on_or_before(day: Day) -> Day {
    let mut d = day;
    while !is_business_day(d) {
        d -= 1;
    }
    d
}

/// The earliest business day `>= day`.
pub fn business_day_on_or_after(day: Day) -> Day {
    let mut d = day;
    while !is_business_day(d) {
        d += 1;
    }
    d
}

/// `n` business days after the first business day on or after `day`.
/// This is when a transaction authorized on `day` posts with a lag of `n`.
pub fn add_business_days(day: Day, n: u32) -> Day {
    let mut d = business_day_on_or_after(day);
    for _ in 0..n {
        d = business_day_on_or_after(d + 1);
    }
    d
}

/// When a recurring paycheck is nominally due. Actual pay dates move to the
/// previous business day ([`PaySchedule::pay_date`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaySchedule {
    /// Every 7 days from `anchor`.
    Weekly { anchor: Day },
    /// Every 14 days from `anchor`.
    Biweekly { anchor: Day },
    /// Two days of the month, e.g. `[1, 15]` or `[15, 31]`. Days past the end
    /// of a month mean its last day.
    SemiMonthly { days: [u32; 2] },
    /// One day of the month, clamped to the month's last day.
    Monthly { day: u32 },
}

impl PaySchedule {
    pub fn validate(&self) -> Result<(), EngineError> {
        let dom_ok = |d: u32| (1..=31).contains(&d);
        let ok = match *self {
            PaySchedule::Weekly { .. } | PaySchedule::Biweekly { .. } => true,
            PaySchedule::SemiMonthly { days: [a, b] } => dom_ok(a) && dom_ok(b) && a != b,
            PaySchedule::Monthly { day } => dom_ok(day),
        };
        if ok {
            Ok(())
        } else {
            Err(EngineError::InvalidConfig(
                "pay schedule days of month must be distinct and in 1..=31",
            ))
        }
    }

    /// A lower bound on the gap between consecutive nominal pay dates.
    pub fn min_gap_days(&self) -> u32 {
        match *self {
            PaySchedule::Weekly { .. } => 7,
            PaySchedule::Biweekly { .. } => 14,
            PaySchedule::Monthly { .. } => 28,
            PaySchedule::SemiMonthly { days } => {
                let (a, b) = (days[0].min(days[1]), days[0].max(days[1]));
                // Within a month b clamps to at most 28 in the shortest month;
                // across months the gap is at least a + (28 - b) when b < 28.
                let within = b.min(28).saturating_sub(a);
                let across = a + 28u32.saturating_sub(b);
                within.min(across)
            }
        }
    }

    /// Nominal pay dates in `from..=to`, ascending.
    pub fn nominal_dates(&self, from: Day, to: Day) -> Vec<Day> {
        let mut out = Vec::new();
        if from > to {
            return out;
        }
        match *self {
            PaySchedule::Weekly { anchor } => periodic(anchor, 7, from, to, &mut out),
            PaySchedule::Biweekly { anchor } => periodic(anchor, 14, from, to, &mut out),
            PaySchedule::SemiMonthly { days } => {
                let mut days = days;
                days.sort_unstable();
                monthly(&days, from, to, &mut out);
            }
            PaySchedule::Monthly { day } => monthly(&[day], from, to, &mut out),
        }
        out
    }

    /// The day a paycheck nominally due on `nominal` actually lands: the same
    /// day, or the previous business day if it is a weekend or bank holiday.
    pub fn pay_date(nominal: Day) -> Day {
        business_day_on_or_before(nominal)
    }
}

fn periodic(anchor: Day, period: Day, from: Day, to: Day, out: &mut Vec<Day>) {
    let k = (i64::from(from) - i64::from(anchor)).div_euclid(i64::from(period));
    let mut d = i64::from(anchor) + k * i64::from(period);
    while d <= i64::from(to) {
        if d >= i64::from(from) {
            out.push(d as Day);
        }
        d += i64::from(period);
    }
}

fn monthly(days_of_month: &[u32], from: Day, to: Day, out: &mut Vec<Day>) {
    let (mut year, mut month, _) = civil_from_day(from);
    loop {
        if day_from_civil(year, month, 1) > to {
            break;
        }
        let last = days_in_month(year, month);
        for &dom in days_of_month {
            let d = day_from_civil(year, month, dom.min(last));
            if (from..=to).contains(&d) {
                out.push(d);
            }
        }
        (year, month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> Day {
        day_from_civil(y, m, d)
    }

    #[test]
    fn civil_conversion_hand_cases() {
        assert_eq!(ymd(1970, 1, 1), 0);
        assert_eq!(ymd(1969, 12, 31), -1);
        assert_eq!(ymd(2024, 1, 1), 19_723);
        assert_eq!(ymd(2000, 3, 1) - ymd(2000, 2, 28), 2); // 2000 is a leap year
        assert_eq!(ymd(1900, 3, 1) - ymd(1900, 2, 28), 1); // 1900 is not
        assert_eq!(civil_from_day(19_723), (2024, 1, 1));
        assert_eq!(civil_from_day(ymd(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn civil_conversion_round_trips() {
        for day in ymd(1899, 1, 1)..ymd(2101, 1, 1) {
            let (y, m, d) = civil_from_day(day);
            assert!(d >= 1 && d <= days_in_month(y, m));
            assert_eq!(day_from_civil(y, m, d), day);
        }
    }

    #[test]
    fn weekdays() {
        assert_eq!(weekday(0), 3); // Thursday
        assert_eq!(weekday(-1), 2);
        assert_eq!(weekday(ymd(2024, 1, 1)), 0); // Monday
        assert!(is_weekend(ymd(2024, 6, 15))); // Saturday
    }

    #[test]
    fn federal_reserve_holidays_2024() {
        let holidays = [
            ymd(2024, 1, 1),
            ymd(2024, 1, 15),
            ymd(2024, 2, 19),
            ymd(2024, 5, 27),
            ymd(2024, 6, 19),
            ymd(2024, 7, 4),
            ymd(2024, 9, 2),
            ymd(2024, 10, 14),
            ymd(2024, 11, 11),
            ymd(2024, 11, 28),
            ymd(2024, 12, 25),
        ];
        let found: Vec<Day> = (ymd(2024, 1, 1)..ymd(2025, 1, 1))
            .filter(|&d| is_bank_holiday(d))
            .collect();
        assert_eq!(found, holidays);
    }

    #[test]
    fn sunday_holidays_observed_monday_saturday_not_observed() {
        assert!(is_bank_holiday(ymd(2022, 12, 26))); // Christmas on Sunday
        assert!(is_bank_holiday(ymd(2021, 7, 5))); // July 4 on Sunday
        assert!(!is_bank_holiday(ymd(2021, 12, 24))); // Christmas on Saturday
        assert!(is_business_day(ymd(2021, 12, 24)));
        assert!(!is_bank_holiday(ymd(2021, 6, 18))); // before Juneteenth existed
    }

    #[test]
    fn business_day_arithmetic() {
        let fri = ymd(2024, 8, 30); // before Labor Day
        assert_eq!(business_day_on_or_after(fri + 1), ymd(2024, 9, 3)); // Sat → Tue
        assert_eq!(business_day_on_or_before(ymd(2024, 9, 2)), fri);
        assert_eq!(add_business_days(fri, 0), fri);
        assert_eq!(add_business_days(fri, 1), ymd(2024, 9, 3));
        assert_eq!(add_business_days(fri + 1, 0), ymd(2024, 9, 3));
        assert_eq!(add_business_days(fri + 1, 2), ymd(2024, 9, 5));
    }

    #[test]
    fn weekly_and_biweekly_dates() {
        let anchor = ymd(2024, 1, 5); // Friday
        let w = PaySchedule::Weekly { anchor }.nominal_dates(ymd(2024, 1, 1), ymd(2024, 1, 31));
        assert_eq!(w, vec![anchor, anchor + 7, anchor + 14, anchor + 21]);
        // Works for ranges before the anchor too.
        let b = PaySchedule::Biweekly { anchor }.nominal_dates(anchor - 30, anchor);
        assert_eq!(b, vec![anchor - 28, anchor - 14, anchor]);
    }

    #[test]
    fn monthly_dates_clamp_to_month_end() {
        let m = PaySchedule::Monthly { day: 31 }.nominal_dates(ymd(2024, 1, 1), ymd(2024, 4, 30));
        assert_eq!(
            m,
            vec![
                ymd(2024, 1, 31),
                ymd(2024, 2, 29),
                ymd(2024, 3, 31),
                ymd(2024, 4, 30)
            ]
        );
        let s = PaySchedule::SemiMonthly { days: [31, 15] }
            .nominal_dates(ymd(2023, 2, 1), ymd(2023, 3, 20));
        assert_eq!(
            s,
            vec![ymd(2023, 2, 15), ymd(2023, 2, 28), ymd(2023, 3, 15)]
        );
    }

    #[test]
    fn pay_dates_move_to_previous_business_day() {
        // The 15th on a Saturday pays Friday the 14th.
        assert_eq!(PaySchedule::pay_date(ymd(2024, 6, 15)), ymd(2024, 6, 14));
        // A holiday Thursday pays Wednesday.
        assert_eq!(PaySchedule::pay_date(ymd(2024, 7, 4)), ymd(2024, 7, 3));
        // Sunday the 1st before Labor Day Monday pays Friday.
        assert_eq!(PaySchedule::pay_date(ymd(2024, 9, 1)), ymd(2024, 8, 30));
        assert_eq!(PaySchedule::pay_date(ymd(2024, 9, 3)), ymd(2024, 9, 3));
    }

    #[test]
    fn min_gap_is_a_true_lower_bound() {
        let schedules = [
            PaySchedule::Weekly { anchor: 3 },
            PaySchedule::Biweekly { anchor: 3 },
            PaySchedule::Monthly { day: 31 },
            PaySchedule::SemiMonthly { days: [1, 15] },
            PaySchedule::SemiMonthly { days: [15, 31] },
            PaySchedule::SemiMonthly { days: [10, 30] },
        ];
        for s in schedules {
            let dates = s.nominal_dates(ymd(2023, 1, 1), ymd(2025, 12, 31));
            let gap = dates
                .windows(2)
                .map(|w| (w[1] - w[0]) as u32)
                .min()
                .unwrap();
            assert!(
                gap >= s.min_gap_days(),
                "{s:?}: gap {gap} < {}",
                s.min_gap_days()
            );
        }
        assert_eq!(
            PaySchedule::SemiMonthly { days: [15, 31] }.min_gap_days(),
            13
        );
    }

    #[test]
    fn schedule_validation() {
        assert!(PaySchedule::Monthly { day: 0 }.validate().is_err());
        assert!(PaySchedule::Monthly { day: 32 }.validate().is_err());
        assert!(
            PaySchedule::SemiMonthly { days: [15, 15] }
                .validate()
                .is_err()
        );
        assert_eq!(
            PaySchedule::SemiMonthly { days: [15, 31] }.validate(),
            Ok(())
        );
    }
}
