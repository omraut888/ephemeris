//! Exploratory analyses (paper/REVISION.md, specifications S1–S8).
//! Development split only; nothing here touches the frozen configuration.
//!
//! The builders take a [`Ctx`]: the balance series, the records and the
//! detected schedule as known at the origin. They read `bal[..=t]` only;
//! the caller may hand a longer series (for the truth), so every index
//! below is checked against `t`.

use std::collections::BTreeMap;

use ephemeris_core::adaptive::AdaptiveConfig;
use ephemeris_core::calendar::civil_from_day;
use ephemeris_core::forecast::{ForecastConfig, ForecastModel, forecast};
use ephemeris_core::schedule::{Schedule, detect_schedule_in};
use ephemeris_core::schema::Date;
use ephemeris_core::tracker::{Tracker, TrackerConfig};
use ephemeris_core::{Day, FlowKind, Transaction};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, Gamma, Poisson};

use crate::baselines::schedule_aware_delta;
use crate::berka::{Account, CALENDAR, tag, transactions};
use crate::berka_run::{HORIZONS, PATHS, seed_for};
use crate::dist::{Dist, mc_levels};
use crate::reference::REFERENCE_SCALE;
use crate::rows::Row;
use crate::variants::sparse_spending;

/// The lookback of every new method (S3).
pub const L: usize = 365;
/// B-EQc: the day-of-month half-width, and the fewest matching days before
/// falling back to all days.
pub const CAL_HALF_WIDTH: u32 = 3;
pub const CAL_MIN: usize = 5;
pub const H_MAX: usize = 90;

/// What a method knows at the origin.
pub struct Ctx<'a> {
    /// Balances from the first day; only `bal[..=t]` is read.
    pub bal: &'a [f64],
    pub first: Day,
    pub t: usize,
    /// Records dated on or before the origin, in the same units as `bal`.
    pub txns: &'a [Transaction],
    pub schedule: &'a Schedule,
    pub seed: u64,
}

impl Ctx<'_> {
    pub fn origin(&self) -> Day {
        self.first + self.t as Day
    }
    fn lo(&self) -> usize {
        self.t.saturating_sub(L)
    }
}

/// The schedule's records: the paychecks of the detected income stream (if
/// the schedule found one) and the scheduled records of detected bills.
pub fn schedule_records(c: &Ctx) -> Vec<Transaction> {
    c.txns
        .iter()
        .filter(|x| {
            (x.kind == FlowKind::Income && c.schedule.income.is_some())
                || (x.kind == FlowKind::Scheduled
                    && c.schedule.bills.iter().any(|b| x.stream == Some(b.stream)))
        })
        .cloned()
        .collect()
}

/// Shared per-origin pieces: the schedule's cumulative future flow by day
/// (`cum[k]`, k = 0..=90), and the prefix sums of its past daily flows.
pub struct Sched {
    pub cum: Vec<f64>,
    /// `past[i]` = the schedule's flows dated on days `first..first+i`.
    pub past: Vec<f64>,
}

pub fn sched(c: &Ctx) -> Sched {
    let recs = schedule_records(c);
    let o = c.origin();
    let cum = (0..=H_MAX as Day)
        .map(|k| {
            if k == 0 {
                0.0
            } else {
                schedule_aware_delta(&recs, c.schedule, o, k)
            }
        })
        .collect();
    let mut daily = vec![0.0; c.t + 1];
    for r in &recs {
        let d = r.day();
        if d >= c.first && d <= o {
            daily[(d - c.first) as usize] += r.amount;
        }
    }
    let mut past = vec![0.0; c.t + 2];
    for i in 0..=c.t {
        past[i + 1] = past[i] + daily[i];
    }
    Sched { cum, past }
}

impl Sched {
    /// The schedule's flows dated in `(d, d+k]` (day indices).
    fn between(&self, d: usize, k: usize) -> f64 {
        self.past[d + k + 1] - self.past[d + 1]
    }
}

fn dom(c: &Ctx, i: usize) -> u32 {
    civil_from_day(c.first + i as Day).2
}

/// B-EQc (S3): past h-day changes from days near the origin's day of month.
/// Returns the distribution and whether it fell back to all days.
pub fn eq_cal(c: &Ctx, h: usize) -> (Dist, bool) {
    let od = dom(c, c.t);
    let all: Vec<usize> = (c.lo()..=c.t - h).collect();
    let near: Vec<usize> = all
        .iter()
        .copied()
        .filter(|&d| {
            let a = dom(c, d).abs_diff(od);
            a.min(31 - a) <= CAL_HALF_WIDTH
        })
        .collect();
    let (ds, fell_back) = if near.len() < CAL_MIN {
        (all, true)
    } else {
        (near, false)
    };
    let b = c.bal;
    let xs = ds.iter().map(|&d| b[c.t] + b[d + h] - b[d]).collect();
    (Dist::sample(xs), fell_back)
}

/// B-SR (S3): the schedule plus the account's own past h-day residuals.
pub fn sched_resid(c: &Ctx, s: &Sched, h: usize) -> Dist {
    let b = c.bal;
    let xs = (c.lo()..=c.t - h)
        .map(|d| b[c.t] + s.cum[h] + (b[d + h] - b[d] - s.between(d, h)))
        .collect();
    Dist::sample(xs)
}

/// The discretionary amounts in the window `(o − L, o]` and its length.
pub fn discretionary_window(c: &Ctx) -> (Vec<f64>, usize) {
    let start = c.origin() - (L as Day) + 1;
    let start = start.max(c.first);
    let amounts = c
        .txns
        .iter()
        .filter(|x| x.kind == FlowKind::Discretionary && x.day() >= start && x.day() <= c.origin())
        .map(|x| x.amount)
        .collect();
    (amounts, (c.origin() - start + 1) as usize)
}

/// CP-noKF (S3): paths `bal[t] + sched(k) + compound-Poisson events`,
/// `paths[i][k-1]` for k = 1..=90.
pub fn cp_paths(c: &Ctx, s: &Sched) -> Vec<Vec<f64>> {
    let (amounts, days) = discretionary_window(c);
    let n = amounts.len();
    let mut rng = ChaCha8Rng::seed_from_u64(c.seed);
    let gamma = Gamma::new(n as f64 + 0.5, 1.0 / days as f64).expect("gamma");
    (0..PATHS)
        .map(|_| {
            let lambda: f64 = gamma.sample(&mut rng);
            let mut b = c.bal[c.t];
            let mut path = Vec::with_capacity(H_MAX);
            let mut prev = 0.0;
            for k in 1..=H_MAX {
                b += s.cum[k] - prev;
                prev = s.cum[k];
                if n > 0 && lambda > 0.0 {
                    let events = Poisson::new(lambda).map_or(0.0, |p| p.sample(&mut rng));
                    for _ in 0..events as usize {
                        b += amounts[rng.random_range(0..n)];
                    }
                }
                path.push(b);
            }
            path
        })
        .collect()
}

fn type7(sorted: &[f64], p: f64) -> f64 {
    let pos = p * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (pos - lo as f64) * (sorted[hi] - sorted[lo])
}

/// The quantile forecast of paths at horizon `h`.
pub fn paths_dist(paths: &[Vec<f64>], h: usize) -> Dist {
    let mut v: Vec<f64> = paths.iter().map(|p| p[h - 1]).collect();
    v.sort_by(f64::total_cmp);
    let levels = mc_levels();
    let q = levels.iter().map(|&l| type7(&v, l)).collect();
    Dist::quantiles(levels, q)
}

/// The share of paths whose minimum over days 1..=h is below `u`.
pub fn paths_below(paths: &[Vec<f64>], h: usize, u: f64) -> f64 {
    let n = paths
        .iter()
        .filter(|p| p[..h].iter().any(|&x| x < u))
        .count();
    n as f64 / paths.len() as f64
}

/// B-EQp, B-SRp and CLIM event probabilities (S5) at horizon `h`.
pub fn empirical_below(c: &Ctx, s: &Sched, h: usize, u: f64) -> [f64; 3] {
    let b = c.bal;
    let ds: Vec<usize> = (c.lo()..=c.t - h).collect();
    let (mut eq, mut sr, mut clim) = (0, 0, 0);
    for &d in &ds {
        let (mut e, mut r, mut l) = (false, false, false);
        for k in 1..=h {
            let ch = b[d + k] - b[d];
            e |= b[c.t] + ch < u;
            r |= b[c.t] + s.cum[k] + ch - s.between(d, k) < u;
            l |= b[d + k] < u;
        }
        eq += usize::from(e);
        sr += usize::from(r);
        clim += usize::from(l);
    }
    let n = ds.len() as f64;
    [eq as f64 / n, sr as f64 / n, clim as f64 / n]
}

/// The S5 thresholds: 0, and the type-7 10th percentile of the balances
/// over the lookback (`bal[lo..=t]`).
pub fn thresholds(c: &Ctx) -> [f64; 2] {
    let mut v: Vec<f64> = c.bal[c.lo()..=c.t].to_vec();
    v.sort_by(f64::total_cmp);
    [0.0, type7(&v, 0.10)]
}

// ------------------------------------------------------------- Berka V2 --

/// Switches for S6 (all on: V2 as frozen).
#[derive(Debug, Clone, Copy)]
pub struct V2Opts {
    pub adaptive: bool,
    pub detector: bool,
    pub floor: bool,
}

pub const V2_FROZEN: V2Opts = V2Opts {
    adaptive: true,
    detector: true,
    floor: true,
};

/// V2 at an origin, as `variants::forecast_at` with the sparse rule, plus
/// switches and overdraft targets. With [`V2_FROZEN`] and no targets its
/// distributions are the frozen V2's (checked by the caller).
pub struct V2Out {
    pub dists: Vec<Dist>,
    /// `below[i][hi]`: P(min over days 1..h < targets[i]).
    pub below: Vec<[f64; 3]>,
    pub k: f64,
    pub income_detected: bool,
    pub alarm_recent: bool,
}

pub fn v2_forecast(
    acct: &Account,
    bal_raw: &[f64],
    origin: Day,
    opts: V2Opts,
    targets: &[f64],
) -> Option<V2Out> {
    let first = acct.first_day();
    let s = acct.scale_at(origin).filter(|&s| s > 0.0)?;
    let k = REFERENCE_SCALE / s;
    let t = (origin - first) as usize;
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let mut cfg = TrackerConfig::new(first);
    cfg.observe.holidays = CALENDAR;
    if !opts.adaptive {
        cfg.spend_var = AdaptiveConfig::fixed(cfg.spend_var.prior);
        cfg.income_var = AdaptiveConfig::fixed(cfg.income_var.prior);
    }
    if !opts.detector {
        cfg.spend_change = None;
        cfg.income_change = None;
    }
    let mut tracker = Tracker::new(cfg).ok()?;
    let ingest = tracker.ingest(&txns, bal_raw[t] * k, origin).ok()?;
    let est = tracker.estimate()?;
    let levels = mc_levels();
    let mut fcfg = ForecastConfig {
        n_paths: PATHS,
        levels: levels.clone(),
        sparse_spending: sparse_spending(&txns, first, origin),
        targets: targets.to_vec(),
        ..ForecastConfig::new(HORIZONS[2] as u32, seed_for(acct.id, origin))
    };
    if !opts.floor {
        fcfg.spend_floor = None;
    }
    let cone = forecast(est, &schedule, &ForecastModel::from_tracker(&cfg), &fcfg).ok()?;
    let dists = HORIZONS
        .iter()
        .map(|&h| {
            Some(Dist::quantiles(
                levels.clone(),
                cone.band(origin + h)?.to_vec(),
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    let below = (0..targets.len())
        .map(|i| {
            let p = |h: Day| cone.prob_below_by(i, origin + h).unwrap_or(f64::NAN);
            [p(HORIZONS[0]), p(HORIZONS[1]), p(HORIZONS[2])]
        })
        .collect();
    Some(V2Out {
        dists,
        below,
        k,
        income_detected: schedule.income.is_some(),
        alarm_recent: ingest
            .days
            .iter()
            .any(|d| d.day > origin - 28 && !d.jumps.is_empty()),
    })
}

/// Everything S3, S5 and S6 need at one Berka origin: rows (new methods and
/// V2 variants) and one event line per (h, threshold).
pub struct OriginOut {
    pub rows: Vec<Row>,
    pub events: Vec<String>,
    pub cal_fallback: [bool; 3],
}

pub const EVENT_HEADER: &str =
    "account,origin,h,threshold,u,event,p_V2,p_CP-noKF,p_B-EQp,p_B-SRp,p_CLIM";

pub fn berka_origin(acct: &Account, origin: Day) -> Option<OriginOut> {
    let first = acct.first_day();
    let end = origin + HORIZONS[2];
    let bal_raw = acct.daily_balance(end);
    let t = (origin - first) as usize;
    // V2 as frozen, with the S5 targets (in normalized units).
    let s = acct.scale_at(origin).filter(|&s| s > 0.0)?;
    let k = REFERENCE_SCALE / s;
    let bal: Vec<f64> = bal_raw.iter().map(|b| b * k).collect();
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let c = Ctx {
        bal: &bal,
        first,
        t,
        txns: &txns,
        schedule: &schedule,
        seed: seed_for(acct.id, origin),
    };
    let us = thresholds(&c);
    let v2 = v2_forecast(acct, &bal_raw, origin, V2_FROZEN, &us)?;
    let sc = sched(&c);
    let paths = cp_paths(&c, &sc);
    let mut out = OriginOut {
        rows: Vec::new(),
        events: Vec::new(),
        cal_fallback: [false; 3],
    };
    let row = |method: &str, h: Day, d: &Dist| {
        let truth = bal[t + h as usize];
        Row {
            account: acct.id,
            origin: Date(origin).to_string(),
            h,
            method: method.to_string(),
            k,
            truth,
            score: d.score(truth),
            income_detected: v2.income_detected,
            months_history: (origin - first) / 30,
            alarm_recent: v2.alarm_recent,
        }
    };
    let ablations: Vec<(&str, V2Out)> = [
        (
            "V2-noadapt",
            V2Opts {
                adaptive: false,
                ..V2_FROZEN
            },
        ),
        (
            "V2-nodetector",
            V2Opts {
                detector: false,
                ..V2_FROZEN
            },
        ),
        (
            "V2-nofloor",
            V2Opts {
                floor: false,
                ..V2_FROZEN
            },
        ),
    ]
    .into_iter()
    .filter_map(|(n, o)| Some((n, v2_forecast(acct, &bal_raw, origin, o, &[])?)))
    .collect();
    for (hi, &h) in HORIZONS.iter().enumerate() {
        let hu = h as usize;
        let (eqc, fb) = eq_cal(&c, hu);
        out.cal_fallback[hi] = fb;
        out.rows.push(row("V2", h, &v2.dists[hi]));
        out.rows.push(row("B-EQc", h, &eqc));
        out.rows.push(row("B-SR", h, &sched_resid(&c, &sc, hu)));
        out.rows.push(row("CP-noKF", h, &paths_dist(&paths, hu)));
        for (n, a) in &ablations {
            out.rows.push(row(n, h, &a.dists[hi]));
        }
        for (i, (&u, name)) in us.iter().zip(["u0", "u10"]).enumerate() {
            let event = bal[t + 1..=t + hu].iter().any(|&x| x < u);
            let [eq, sr, clim] = empirical_below(&c, &sc, hu, u);
            out.events.push(format!(
                "{},{},{h},{name},{u},{},{},{},{},{},{}",
                acct.id,
                Date(origin),
                u8::from(event),
                v2.below[i][hi],
                paths_below(&paths, hu, u),
                eq,
                sr,
                clim
            ));
        }
    }
    Some(out)
}

/// The drift through each day with records (S1): `(day, max so far)`.
pub fn drift_through(acct: &Account) -> BTreeMap<Day, f64> {
    let mut out = BTreeMap::new();
    let mut b = acct.opening;
    let mut mx: f64 = 0.0;
    let mut i = 0;
    let r = &acct.records;
    while i < r.len() {
        let d = r[i].day;
        let mut reported = Vec::new();
        while i < r.len() && r[i].day == d {
            b += r[i].amount;
            reported.push(r[i].balance);
            i += 1;
        }
        let drift = reported
            .iter()
            .map(|&x| (x - b).abs())
            .fold(f64::INFINITY, f64::min);
        mx = mx.max(drift);
        out.insert(d, mx);
    }
    out
}

// ---------------------------------------------------------- S1 and S2 --

/// S1, per development account: grid origins under the frozen rule F and
/// the past-only rule P, the account measures, and the origin measures.
pub struct EligibilityAccount {
    pub account_csv: String,
    pub origin_csv: Vec<String>,
    /// Origins eligible under P but not F (to rerun).
    pub p_not_f: Vec<Day>,
}

pub const ELIG_ACCOUNT_HEADER: &str =
    "account,drift_excluded,eligible_f,eligible_p,records_per_month,volatility,income_share";
pub const ELIG_ORIGIN_HEADER: &str =
    "account,origin,f,p,minrec_ok,scale_ok,drift_to_origin,records_180,daily_sd_over_s";

pub fn eligibility_account(acct: &Account) -> EligibilityAccount {
    use crate::berka::MAX_DRIFT_CZK;
    use crate::berka_run::{MIN_RECORDS_180, ORIGIN_STEP, WARM_UP, last_day};
    let first = acct.first_day();
    let drift = drift_through(acct);
    let full_drift = drift.values().last().copied().unwrap_or(0.0);
    let excluded = full_drift > MAX_DRIFT_CZK;
    let bal = acct.daily_balance(last_day());
    let mut origins = Vec::new();
    let mut p_not_f = Vec::new();
    let (mut nf, mut np, mut grid, mut income) = (0, 0, 0, 0);
    let mut o = first + WARM_UP;
    while o + HORIZONS[2] <= last_day() {
        let n180 = acct.count_in_window(o, WARM_UP);
        let minrec = n180 >= MIN_RECORDS_180;
        let s = acct.scale_at(o).filter(|&s| s > 0.0);
        let d = drift.range(..=o).next_back().map_or(0.0, |(_, &v)| v);
        let f = minrec && s.is_some() && !excluded;
        let p = minrec && s.is_some() && d <= MAX_DRIFT_CZK;
        nf += usize::from(f);
        np += usize::from(p);
        if p && !f {
            p_not_f.push(o);
        }
        if minrec && s.is_some() {
            grid += 1;
            let recs = acct.records_through(o);
            let tx = transactions(recs, &tag(recs), 1.0);
            income += usize::from(detect_schedule_in(&tx, CALENDAR).income.is_some());
        }
        let t = (o - first) as usize;
        let lo = t.saturating_sub(WARM_UP as usize);
        let ch: Vec<f64> = bal[lo..=t].windows(2).map(|w| w[1] - w[0]).collect();
        let sd = sd(&ch);
        origins.push(format!(
            "{},{},{},{},{},{},{d},{n180},{}",
            acct.id,
            Date(o),
            u8::from(f),
            u8::from(p),
            u8::from(minrec),
            u8::from(s.is_some()),
            s.map_or(f64::NAN, |s| sd / s)
        ));
        o += ORIGIN_STEP;
    }
    let months = f64::from((acct.last_day() - first + 1) as u32) / (365.25 / 12.0);
    let ch30: Vec<f64> = bal.windows(31).map(|w| w[30] - w[0]).collect();
    let vol = acct
        .scale_at(acct.last_day())
        .filter(|&s| s > 0.0)
        .map_or(f64::NAN, |s| sd(&ch30) / s);
    EligibilityAccount {
        account_csv: format!(
            "{},{},{nf},{np},{:.4},{vol:.6},{}",
            acct.id,
            u8::from(excluded),
            acct.records.len() as f64 / months,
            if grid == 0 {
                f64::NAN
            } else {
                income as f64 / grid as f64
            }
        ),
        origin_csv: origins,
        p_not_f,
    }
}

fn sd(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return f64::NAN;
    }
    let n = xs.len() as f64;
    let m = xs.iter().sum::<f64>() / n;
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
}

/// S2(a): at each frozen origin and horizon, whether V1's adjusted levels
/// are non-decreasing, and how many of V1's quantile values the sort moved.
pub fn v1_crossings(acct: &Account) -> Vec<String> {
    use crate::variants::{
        ACI_GRID_START, ACI_GRID_STEP, ACI_LEVELS, V0, aci_levels, eligible_origins, forecast_at,
    };
    let first = acct.first_day();
    let bal_raw = acct.daily_balance(crate::berka_run::last_day());
    let origins = eligible_origins(acct);
    let Some(&last) = origins.last() else {
        return Vec::new();
    };
    let mut grid = Vec::new();
    let mut g = first + ACI_GRID_START;
    while g + HORIZONS[0] <= last {
        if let Some(f) = forecast_at(acct, &bal_raw, g, V0) {
            grid.push((g, f));
        }
        g += ACI_GRID_STEP;
    }
    let mut out = Vec::new();
    for o in origins {
        let Some(f0) = forecast_at(acct, &bal_raw, o, V0) else {
            continue;
        };
        for (hi, &h) in HORIZONS.iter().enumerate() {
            let a = aci_levels(
                grid.iter()
                    .filter(|(g, _)| g + h <= o)
                    .map(|(g, f)| (&f.dists[hi], bal_raw[(g + h - first) as usize] * f.k)),
            );
            let monotone = a.windows(2).all(|w| w[0] <= w[1]);
            // The values recalibrate() assigns, before its sort.
            let d = &f0.dists[hi];
            let mut pairs: Vec<(f64, f64)> = vec![(0.5, d.median())];
            for (&l, &al) in ACI_LEVELS.iter().zip(&a) {
                let (lo, hi_) = d.interval(al);
                pairs.push(((1.0 - l) / 2.0, lo));
                pairs.push(((1.0 + l) / 2.0, hi_));
            }
            pairs.sort_by(|x, y| x.0.total_cmp(&y.0));
            let before: Vec<f64> = pairs.iter().map(|p| p.1).collect();
            let mut after = before.clone();
            after.sort_by(f64::total_cmp);
            let moved = before.iter().zip(&after).filter(|(x, y)| x != y).count();
            out.push(format!(
                "{},{},{h},{},{moved},{}",
                acct.id,
                Date(o),
                u8::from(monotone),
                before.len()
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type7_matches_by_hand() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(type7(&v, 0.0), 1.0);
        assert_eq!(type7(&v, 0.5), 3.0);
        assert!((type7(&v, 0.1) - 1.4).abs() < 1e-12);
    }

    #[test]
    fn empirical_paths_by_hand() {
        // Balances 10, 8, 12, 9, 11 (t = 4); no schedule.
        let bal = [10.0, 8.0, 12.0, 9.0, 11.0];
        let sched = Schedule::default();
        let c = Ctx {
            bal: &bal,
            first: 0,
            t: 4,
            txns: &[],
            schedule: &sched,
            seed: 1,
        };
        let s = Sched {
            cum: vec![0.0; H_MAX + 1],
            past: vec![0.0; 6],
        };
        // h = 1: past start days 0..=3, one-day changes −2, +4, −3, +2 →
        // shifted 9, 15, 8, 13: two of four below 10. CLIM (bal[d+1] < 10):
        // 8 yes, 12 no, 9 yes, 11 no → two of four.
        let [eq, sr, clim] = empirical_below(&c, &s, 1, 10.0);
        assert!((eq - 0.5).abs() < 1e-12);
        assert_eq!(eq, sr);
        assert!((clim - 0.5).abs() < 1e-12);
    }
}
