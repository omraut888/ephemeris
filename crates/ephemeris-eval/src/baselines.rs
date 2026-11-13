//! The baselines of paper/PLAN.md §2 and §8 decision 3.
//!
//! Each works on a daily balance series `bal`, where `bal[i]` is the
//! balance at the end of day `first + i` and `t` is the origin's index:
//! only `bal[..=t]` may be read. The learned quantile model (B-GLOBAL) lives
//! in Python (paper/baselines/); its quantiles are scored by [`crate::dist`].
//!
//! | id | forecast of the balance h days after the origin |
//! |---|---|
//! | B-30 | 30-day mean daily change, projected; Gaussian `± sd·√h` |
//! | B-LIN | least-squares line through the last 90 days, extended; Gaussian `± residual sd` |
//! | B-SA | schedule-aware, deterministic (see [`schedule_aware_delta`]) |
//! | B-SA+ | B-SA plus the empirical distribution of its own past h-day errors |
//! | B-SEAS | the change over the same window one month earlier (three for h > 31), plus its own past errors |
//! | B-EQ | today's balance plus the account's own past h-day balance changes |
//!
//! "Past" means pseudo-origins in the 365 days before the origin whose
//! outcome is known at the origin (`d + h ≤ t`): never the future.

use ephemeris_core::calendar::{civil_from_day, day_from_civil, days_in_month};
use ephemeris_core::schedule::Schedule;
use ephemeris_core::{Day, FlowKind, Transaction, TxnStatus};

use crate::dist::Dist;

/// How far back the empirical baselines look.
pub const LOOKBACK: usize = 365;

pub fn avg30(bal: &[f64], t: usize, h: Day) -> Dist {
    let w = &bal[t - 30..=t];
    let d: Vec<f64> = w.windows(2).map(|p| p[1] - p[0]).collect();
    let mean = d.iter().sum::<f64>() / d.len() as f64;
    let var = d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (d.len() - 1) as f64;
    let h = f64::from(h);
    Dist::Gaussian {
        mean: bal[t] + mean * h,
        sd: var.sqrt() * h.sqrt(),
    }
}

pub fn straight_line(bal: &[f64], t: usize, h: Day) -> Dist {
    let w = &bal[t + 1 - 90..=t];
    let n = w.len() as f64;
    let t_mean = (n - 1.0) / 2.0;
    let y_mean = w.iter().sum::<f64>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, y) in w.iter().enumerate() {
        sxy += (i as f64 - t_mean) * (y - y_mean);
        sxx += (i as f64 - t_mean).powi(2);
    }
    let slope = sxy / sxx;
    let intercept = y_mean - slope * t_mean;
    let resid = w
        .iter()
        .enumerate()
        .map(|(i, y)| (y - intercept - slope * i as f64).powi(2))
        .sum::<f64>()
        / (n - 2.0);
    Dist::Gaussian {
        mean: intercept + slope * (n - 1.0 + f64::from(h)),
        sd: resid.sqrt(),
    }
}

/// B-EQ: `bal[t]` plus every past h-day change `bal[d + h] − bal[d]`.
pub fn empirical_changes(bal: &[f64], t: usize, h: Day) -> Dist {
    let h = h as usize;
    let from = t.saturating_sub(LOOKBACK);
    let changes: Vec<f64> = (from..=t - h)
        .map(|d| bal[t] + bal[d + h] - bal[d])
        .collect();
    Dist::sample(changes)
}

/// The same calendar date `months` months before `day` (clamped to month end).
pub fn months_back(day: Day, months: u32) -> Day {
    let (y, m, d) = civil_from_day(day);
    let total = y * 12 + m as i32 - 1 - months as i32;
    let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    day_from_civil(ny, nm, d.min(days_in_month(ny, nm)))
}

/// B-SEAS's point change at index `t` (day `first + t`): the change over
/// the window that many months earlier. One month for `h ≤ 31`, three
/// otherwise, and one more if the earlier window would end after `t`.
pub fn seasonal_delta(bal: &[f64], first: Day, t: usize, h: Day) -> Option<f64> {
    let day = first + t as Day;
    let mut months = if h <= 31 { 1 } else { 3 };
    let mut start = months_back(day, months);
    if start + h > day {
        months += 1;
        start = months_back(day, months);
    }
    let s = usize::try_from(start - first).ok()?;
    Some(bal[s + h as usize] - bal[s])
}

/// B-SEAS: the seasonal point plus its own past errors.
pub fn seasonal(bal: &[f64], first: Day, t: usize, h: Day) -> Option<Dist> {
    let point = bal[t] + seasonal_delta(bal, first, t, h)?;
    let hu = h as usize;
    let from = t.saturating_sub(LOOKBACK);
    let errors: Vec<f64> = (from..=t - hu)
        .filter_map(|d| Some(bal[d + hu] - bal[d] - seasonal_delta(bal, first, d, h)?))
        .collect();
    Some(if errors.is_empty() {
        Dist::Point(point)
    } else {
        Dist::sample(errors.into_iter().map(|e| point + e).collect())
    })
}

/// B-SA's forecast change over `h` days after `today` from a tagged snapshot:
/// each detected paycheck on its pay date at the latest paycheck's amount,
/// each detected bill every period after its last charge at its latest
/// amount, plus the average daily net of all other flows over the last 30
/// days. Overdue paychecks and bills land tomorrow, as in the Monte Carlo.
pub fn schedule_aware_delta(snap: &[Transaction], schedule: &Schedule, today: Day, h: Day) -> f64 {
    let end = today + h;
    let mut delta = 0.0;
    if let Some(timing) = &schedule.income {
        let pay = snap
            .iter()
            .filter(|t| t.kind == FlowKind::Income && t.status == TxnStatus::Posted)
            .max_by_key(|t| (t.day(), t.id))
            .map_or(0.0, |t| t.amount);
        let paydays = timing
            .schedule
            .nominal_dates(timing.last_paid_nominal + 1, end + 8)
            .into_iter()
            .map(|n| timing.pay_date(n).max(today + 1));
        delta += pay * paydays.filter(|&d| d <= end).count() as f64;
    }
    for bill in &schedule.bills {
        let mut next = bill.next_due().max(today + 1);
        while next <= end {
            delta += bill.amount;
            next = match bill.day_of_month {
                Some(a) => a.next_after(next),
                None => next + bill.period_days as Day,
            };
        }
    }
    let billed = |t: &Transaction| {
        t.kind == FlowKind::Scheduled && schedule.bills.iter().any(|b| t.stream == Some(b.stream))
    };
    let other: f64 = snap
        .iter()
        .filter(|t| today - t.day() < 30)
        .filter(|t| !matches!(t.kind, FlowKind::Income | FlowKind::Transfer) && !billed(t))
        .map(|t| t.amount)
        .sum();
    delta + other / 30.0 * f64::from(h)
}

/// B-SA+: the B-SA point plus its own past errors (already in the
/// origin's units). With no past errors it is B-SA.
pub fn schedule_aware_plus(point: f64, errors: &[f64]) -> Dist {
    if errors.is_empty() {
        Dist::Point(point)
    } else {
        Dist::sample(errors.iter().map(|e| point + e).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> Day {
        day_from_civil(y, m, d)
    }

    #[test]
    fn months_back_clamps() {
        assert_eq!(months_back(ymd(1996, 3, 31), 1), ymd(1996, 2, 29));
        assert_eq!(months_back(ymd(1997, 1, 15), 3), ymd(1996, 10, 15));
        assert_eq!(months_back(ymd(1997, 5, 31), 3), ymd(1997, 2, 28));
    }

    #[test]
    fn straight_line_and_avg30_on_a_line() {
        // bal = 100 + 2i: both baselines extend the line exactly.
        let bal: Vec<f64> = (0..200).map(|i| 100.0 + 2.0 * f64::from(i)).collect();
        let t = 150;
        for h in [7, 30] {
            let want = bal[t] + 2.0 * f64::from(h);
            match straight_line(&bal, t, h) {
                Dist::Gaussian { mean, sd } => {
                    assert!((mean - want).abs() < 1e-9 && sd < 1e-6)
                }
                d => panic!("{d:?}"),
            }
            match avg30(&bal, t, h) {
                Dist::Gaussian { mean, sd } => assert!((mean - want).abs() < 1e-9 && sd == 0.0),
                d => panic!("{d:?}"),
            }
        }
    }

    #[test]
    fn empirical_changes_by_hand() {
        // bal: 0, 1, 3, 6, 10 (changes over 2 days: 3, 5, 7 for d = 0, 1, 2).
        let bal = [0.0, 1.0, 3.0, 6.0, 10.0];
        let d = empirical_changes(&bal, 4, 2);
        assert_eq!(d, Dist::Sample(vec![13.0, 15.0, 17.0]));
    }

    #[test]
    fn seasonal_uses_the_previous_month_and_never_the_future() {
        let first = ymd(1996, 1, 1);
        // A monthly salary of 30 on the 10th, nothing else.
        let bal: Vec<f64> = (0..400)
            .map(|i| {
                let (y, m, d) = civil_from_day(first + i);
                let months = (y - 1996) * 12 + m as i32 - 1 + i32::from(d >= 10);
                30.0 * f64::from(months)
            })
            .collect();
        let t = (ymd(1996, 9, 5) - first) as usize;
        // Sep 5 + 7 days crosses the 10th; so did Aug 5 + 7.
        assert_eq!(seasonal_delta(&bal, first, t, 7), Some(30.0));
        // A perfect seasonal: every past error is 0.
        let d = seasonal(&bal, first, t, 7).unwrap();
        assert_eq!(d.median(), bal[t] + 30.0);
        assert_eq!(d.interval(0.95), (bal[t] + 30.0, bal[t] + 30.0));
        // h = 90 looks three months back, whose window ends by the origin.
        let s = (months_back(first + t as Day, 3) - first) as usize;
        assert!(s + 90 <= t);
    }
}
