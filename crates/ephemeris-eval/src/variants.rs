//! The revision candidates of paper/PLAN.md §9, on Berka.
//!
//! - V0: the original model (as `EPHEMERIS` in [`crate::berka_run`]).
//! - V1: V0 recalibrated online by adaptive conformal inference.
//! - V2: sparse spending (the core's opt-in compound-Poisson model), for
//!   accounts the rule marks sparse.
//! - V3: empirical-Bayes priors from the other four account folds.
//! - V4: V2 + V3, recalibrated as V1.
//!
//! Every constant below is the one fixed in §9 before this code was written.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ephemeris_core::calendar::{civil_from_day, day_from_civil, days_in_month};
use ephemeris_core::forecast::{ForecastConfig, ForecastModel, SparseSpending, forecast};
use ephemeris_core::model::{IDX_C, IDX_M};
use ephemeris_core::schedule::detect_schedule_in;
use ephemeris_core::schema::Date;
use ephemeris_core::tracker::{Tracker, TrackerConfig};
use ephemeris_core::{Day, FlowKind, Transaction};

use crate::berka::{Account, CALENDAR, median, tag, transactions};
use crate::berka_run::{
    HORIZONS, MIN_RECORDS_180, ORIGIN_STEP, PATHS, WARM_UP, last_day, seed_for,
};
use crate::dist::{Dist, mc_levels};
use crate::reference::REFERENCE_SCALE;
use crate::rows::Row;
use crate::split::splitmix64;

/// V1: the central levels with an ACI recursion each, the step, the cap on
/// an adjusted level, and the calibration grid.
pub const ACI_LEVELS: [f64; 11] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95, 0.98];
pub const ACI_GAMMA: f64 = 0.05;
pub const ACI_MAX_LEVEL: f64 = 0.995;
pub const ACI_GRID_START: Day = 90;
pub const ACI_GRID_STEP: Day = 14;
/// V2: the window, and the sparse threshold on the median number of
/// discretionary records per complete month in it.
pub const SPARSE_WINDOW: Day = 365;
pub const SPARSE_MAX_PER_MONTH: f64 = 10.0;
/// V3: the folds (the same as B-GLOBAL's) and the floors on prior sds.
pub const FOLDS: u64 = 5;
pub const FOLD_SALT: u64 = 0x5048_4153_4536_2d33; // "PHASE6-3"
pub const MIN_C_SD: f64 = 1.0;
pub const MIN_M_SD: f64 = 10.0;

pub const VARIANTS: [&str; 5] = ["V0", "V1", "V2", "V3", "V4"];

pub fn fold_of(account: u64) -> u64 {
    splitmix64(FOLD_SALT ^ account) % FOLDS
}

/// One account's V0 estimates for V3: `(ĉ, m̂, σ̂_s², R̂_inc)`, the income
/// terms only when income is detected.
pub type Estimates = (f64, Option<f64>, f64, Option<f64>);

/// V3's fitted priors, in normalized units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Priors {
    pub c_mean: f64,
    pub c_sd: f64,
    pub m_mean: f64,
    pub m_sd: f64,
    pub spend_var: f64,
    pub income_var: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    pub priors: Option<Priors>,
    pub sparse: bool,
}

pub const V0: Model = Model {
    priors: None,
    sparse: false,
};

/// One model's forecast from one origin.
#[derive(Debug, Clone)]
pub struct Forecast {
    pub dists: Vec<Dist>,
    pub k: f64,
    pub income_detected: bool,
    pub alarm_recent: bool,
    pub sparse_used: bool,
}

/// V2's rule and inputs at `origin`, from the tagged records (normalized).
pub fn sparse_spending(txns: &[Transaction], first: Day, origin: Day) -> Option<SparseSpending> {
    let start = (origin - SPARSE_WINDOW + 1).max(first);
    let disc: Vec<&Transaction> = txns
        .iter()
        .filter(|t| t.kind == FlowKind::Discretionary && t.day() >= start && t.day() <= origin)
        .collect();
    // Discretionary records per complete calendar month inside the window.
    let mut counts = Vec::new();
    let (mut y, mut m, _) = civil_from_day(start);
    loop {
        let (a, b) = (
            day_from_civil(y, m, 1),
            day_from_civil(y, m, days_in_month(y, m)),
        );
        if b > origin {
            break;
        }
        if a >= start {
            counts.push(disc.iter().filter(|t| (a..=b).contains(&t.day())).count() as f64);
        }
        (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    }
    let med = median(&mut counts)?;
    (med < SPARSE_MAX_PER_MONTH).then(|| SparseSpending {
        events: disc.len() as u32,
        days: (origin - start + 1) as u32,
        amounts: disc.iter().map(|t| t.amount).collect(),
    })
}

fn tracker_config(first: Day, priors: Option<Priors>) -> TrackerConfig {
    let mut t = TrackerConfig::new(first);
    t.observe.holidays = CALENDAR;
    if let Some(p) = priors {
        t.prior.c_mean = p.c_mean;
        t.prior.c_sd = p.c_sd;
        t.prior.m_mean = p.m_mean;
        t.prior.m_sd = p.m_sd;
        t.spend_var.prior = p.spend_var.clamp(t.spend_var.min, t.spend_var.max);
        t.income_var.prior = p.income_var.clamp(t.income_var.min, t.income_var.max);
    }
    t
}

/// `model`'s forecast from `origin`, or `None` if the account has no scale
/// or the engine declines. Uses the records on or before `origin` only.
pub fn forecast_at(acct: &Account, bal_raw: &[f64], origin: Day, model: Model) -> Option<Forecast> {
    let first = acct.first_day();
    let s = acct.scale_at(origin).filter(|&s| s > 0.0)?;
    let k = REFERENCE_SCALE / s;
    let t = (origin - first) as usize;
    let recs = acct.records_through(origin);
    let tagging = tag(recs);
    let txns = transactions(recs, &tagging, k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let cfg = tracker_config(first, model.priors);
    let mut tracker = Tracker::new(cfg).ok()?;
    let ingest = tracker.ingest(&txns, bal_raw[t] * k, origin).ok()?;
    let est = tracker.estimate()?;
    let sparse = if model.sparse {
        sparse_spending(&txns, first, origin)
    } else {
        None
    };
    let levels = mc_levels();
    let fcfg = ForecastConfig {
        n_paths: PATHS,
        levels: levels.clone(),
        sparse_spending: sparse.clone(),
        ..ForecastConfig::new(HORIZONS[2] as u32, seed_for(acct.id, origin))
    };
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
    Some(Forecast {
        dists,
        k,
        income_detected: schedule.income.is_some(),
        alarm_recent: ingest
            .days
            .iter()
            .any(|d| d.day > origin - 28 && !d.jumps.is_empty()),
        sparse_used: sparse.is_some(),
    })
}

/// The ACI recursions for one horizon, run over the calibration stream
/// `(dist, truth)` in order; returns the adjusted level per [`ACI_LEVELS`].
pub fn aci_levels<'a>(stream: impl Iterator<Item = (&'a Dist, f64)>) -> [f64; 11] {
    let mut alpha = ACI_LEVELS.map(|l| 1.0 - l);
    for (d, y) in stream {
        for (a, &l) in alpha.iter_mut().zip(&ACI_LEVELS) {
            let (lo, hi) = d.interval((1.0 - *a).clamp(0.0, ACI_MAX_LEVEL));
            let err = if lo <= y && y <= hi { 0.0 } else { 1.0 };
            *a += ACI_GAMMA * ((1.0 - l) - err);
        }
    }
    alpha.map(|a| (1.0 - a).clamp(0.0, ACI_MAX_LEVEL))
}

/// The recalibrated forecast: quantiles at `(1 ± ℓ)/2` and the median, each
/// taken from `d` at the adjusted level.
pub fn recalibrate(d: &Dist, adjusted: &[f64; 11]) -> Dist {
    let mut pairs: Vec<(f64, f64)> = vec![(0.5, d.median())];
    for (&l, &a) in ACI_LEVELS.iter().zip(adjusted) {
        let (lo, hi) = d.interval(a);
        pairs.push(((1.0 - l) / 2.0, lo));
        pairs.push(((1.0 + l) / 2.0, hi));
    }
    pairs.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (levels, values) = pairs.into_iter().unzip();
    Dist::quantiles(levels, values)
}

/// V0's estimates at an account's last eligible origin, for V3:
/// `(ĉ, m̂, σ̂_s², R̂_inc)`, the income terms only if income is detected.
pub fn account_estimates(acct: &Account) -> Option<Estimates> {
    let origin = eligible_origins(acct).pop()?;
    let first = acct.first_day();
    let k = REFERENCE_SCALE / acct.scale_at(origin)?;
    let recs = acct.records_through(origin);
    let tagging = tag(recs);
    let txns = transactions(recs, &tagging, k);
    let bal = acct.daily_balance(origin);
    let mut tracker = Tracker::new(tracker_config(first, None)).ok()?;
    tracker.ingest(&txns, bal[bal.len() - 1] * k, origin).ok()?;
    let est = tracker.estimate()?;
    let income = detect_schedule_in(&txns, CALENDAR).income.is_some();
    Some((
        est.x[IDX_C],
        income.then_some(est.x[IDX_M]),
        est.spend_var.value,
        income.then_some(est.income_var.value),
    ))
}

/// V0's origins with the backtest's eligibility (scale present, enough records).
pub fn eligible_origins(acct: &Account) -> Vec<Day> {
    let first = acct.first_day();
    let mut out = Vec::new();
    let mut o = first + WARM_UP;
    while o + HORIZONS[2] <= last_day() {
        if acct.count_in_window(o, WARM_UP) >= MIN_RECORDS_180
            && acct.scale_at(o).is_some_and(|s| s > 0.0)
        {
            out.push(o);
        }
        o += ORIGIN_STEP;
    }
    out
}

fn mad_sd(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec();
    let m = median(&mut v).unwrap_or(0.0);
    let mut dev: Vec<f64> = xs.iter().map(|x| (x - m).abs()).collect();
    1.4826 * median(&mut dev).unwrap_or(0.0)
}

/// V3's priors from a set of accounts' estimates.
pub fn fit_priors(est: &[Estimates]) -> Option<Priors> {
    let c: Vec<f64> = est.iter().map(|e| e.0).collect();
    let m: Vec<f64> = est.iter().filter_map(|e| e.1).collect();
    let sv: Vec<f64> = est.iter().map(|e| e.2).collect();
    let iv: Vec<f64> = est.iter().filter_map(|e| e.3).collect();
    Some(Priors {
        c_mean: median(&mut c.clone())?,
        c_sd: mad_sd(&c).max(MIN_C_SD),
        m_mean: median(&mut m.clone())?,
        m_sd: mad_sd(&m).max(MIN_M_SD),
        spend_var: median(&mut sv.clone())?,
        income_var: median(&mut iv.clone())?,
    })
}

#[derive(Debug, Default)]
pub struct VariantResult {
    pub account: u64,
    pub rows: Vec<Row>,
    pub origins: usize,
    /// Origins at which V2's rule chose sparse spending.
    pub sparse_origins: usize,
}

/// All variants for one account. `priors` is V3's (from the other folds).
pub fn run_account(acct: &Account, priors: Priors) -> VariantResult {
    let mut out = VariantResult {
        account: acct.id,
        ..VariantResult::default()
    };
    let first = acct.first_day();
    let end = last_day();
    let bal_raw = acct.daily_balance(end);
    let v3 = Model {
        priors: Some(priors),
        sparse: false,
    };
    let v2 = Model {
        priors: None,
        sparse: true,
    };
    let v23 = Model {
        priors: Some(priors),
        sparse: true,
    };
    let origins = eligible_origins(acct);
    let Some(&last_origin) = origins.last() else {
        return out;
    };
    // Calibration streams: (grid day, forecast) for V0 and V2 + V3.
    let mut grid0 = Vec::new();
    let mut grid23 = Vec::new();
    let mut g = first + ACI_GRID_START;
    while g + HORIZONS[0] <= last_origin {
        if let Some(f) = forecast_at(acct, &bal_raw, g, V0) {
            grid0.push((g, f));
        }
        if let Some(f) = forecast_at(acct, &bal_raw, g, v23) {
            grid23.push((g, f));
        }
        g += ACI_GRID_STEP;
    }
    let truth_at = |day: Day, k: f64| bal_raw[(day - first) as usize] * k;

    for o in origins {
        // The same origin as V0 in the main backtest must exist for every model.
        let (Some(f0), Some(f2), Some(f3), Some(f23)) = (
            forecast_at(acct, &bal_raw, o, V0),
            forecast_at(acct, &bal_raw, o, v2),
            forecast_at(acct, &bal_raw, o, v3),
            forecast_at(acct, &bal_raw, o, v23),
        ) else {
            continue;
        };
        out.origins += 1;
        out.sparse_origins += usize::from(f2.sparse_used);
        for (hi, &h) in HORIZONS.iter().enumerate() {
            let stream = |grid: &'_ [(Day, Forecast)]| -> [f64; 11] {
                aci_levels(
                    grid.iter()
                        .filter(|(g, _)| g + h <= o)
                        .map(|(g, f)| (&f.dists[hi], truth_at(g + h, f.k))),
                )
            };
            let v1 = recalibrate(&f0.dists[hi], &stream(&grid0));
            let v4 = recalibrate(&f23.dists[hi], &stream(&grid23));
            let cases: [(&str, &Forecast, Dist); 5] = [
                ("V0", &f0, f0.dists[hi].clone()),
                ("V1", &f0, v1),
                ("V2", &f2, f2.dists[hi].clone()),
                ("V3", &f3, f3.dists[hi].clone()),
                ("V4", &f23, v4),
            ];
            for (method, f, d) in cases {
                let truth = truth_at(o + h, f.k);
                out.rows.push(Row {
                    account: acct.id,
                    origin: Date(o).to_string(),
                    h,
                    method: method.to_string(),
                    k: f.k,
                    truth,
                    score: d.score(truth),
                    income_detected: f.income_detected,
                    months_history: (o - first) / 30,
                    alarm_recent: f.alarm_recent,
                });
            }
        }
    }
    out
}

/// Run `accounts` in parallel; V3's priors per account from the other folds.
pub fn run_all(accounts: &[&Account], threads: usize) -> (Vec<VariantResult>, Vec<Priors>) {
    // Phase A: every account's V0 estimates (for the fold priors).
    let estimates: Vec<Option<Estimates>> = parallel_map(accounts, threads, account_estimates);
    let fold_priors: Vec<Priors> = (0..FOLDS)
        .map(|k| {
            let train: Vec<_> = accounts
                .iter()
                .zip(&estimates)
                .filter(|(a, _)| fold_of(a.id) != k)
                .filter_map(|(_, e)| *e)
                .collect();
            fit_priors(&train).expect("training folds have estimates")
        })
        .collect();
    eprintln!("fold priors: {fold_priors:?}");
    let results = parallel_map(accounts, threads, |a| {
        run_account(a, fold_priors[fold_of(a.id) as usize])
    });
    (results, fold_priors)
}

/// Held-out (PLAN §10): V3's priors from every development account at once.
pub fn pooled_priors(dev: &[&Account], threads: usize) -> Priors {
    let estimates: Vec<Estimates> = parallel_map(dev, threads, account_estimates)
        .into_iter()
        .flatten()
        .collect();
    fit_priors(&estimates).expect("development accounts have estimates")
}

/// All variants for `accounts` with one set of V3 priors (the held-out run).
pub fn run_with_priors(
    accounts: &[&Account],
    threads: usize,
    priors: Priors,
) -> Vec<VariantResult> {
    parallel_map(accounts, threads, |a| run_account(a, priors))
}

fn parallel_map<T: Send>(
    accounts: &[&Account],
    threads: usize,
    f: impl Fn(&Account) -> T + Sync,
) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(accounts.len()));
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(a) = accounts.get(i) else { break };
                    let r = f(a);
                    out.lock().expect("lock").push((i, r));
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(100) || n == accounts.len() {
                        eprintln!("  {n}/{}", accounts.len());
                    }
                }
            });
        }
    });
    let mut v = out.into_inner().expect("lock");
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeris_core::TxnStatus;
    use ephemeris_core::types::{AccountId, TxnId};

    #[test]
    fn aci_widens_after_misses_and_narrows_after_hits() {
        let d = Dist::Gaussian { mean: 0.0, sd: 1.0 };
        // Truth always far outside: every level's α falls by γ·(1 − ℓ − 1)
        // = −γℓ per step, so the adjusted level rises toward the cap.
        let far = aci_levels(std::iter::repeat_n((&d, 10.0), 20));
        assert!((far[8] - (0.9 + 20.0 * ACI_GAMMA * 0.9).min(ACI_MAX_LEVEL)).abs() < 1e-12);
        assert_eq!(far[10], ACI_MAX_LEVEL);
        // Truth at the median: never an error, α rises by γ(1 − ℓ) per step.
        let near = aci_levels(std::iter::repeat_n((&d, 0.0), 4));
        assert!((near[8] - (0.9 - 4.0 * ACI_GAMMA * 0.1)).abs() < 1e-12);
        // No stream: the nominal levels.
        let none = aci_levels(std::iter::empty());
        assert!(
            none.iter()
                .zip(&ACI_LEVELS)
                .all(|(a, l)| (a - l).abs() < 1e-12)
        );
    }

    #[test]
    fn recalibration_at_nominal_levels_keeps_the_intervals() {
        let d = Dist::Gaussian { mean: 5.0, sd: 2.0 };
        let r = recalibrate(&d, &ACI_LEVELS);
        for l in [0.5, 0.8, 0.9, 0.95] {
            let (a, b) = (r.interval(l), d.interval(l));
            assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9, "{l}");
        }
        // Wider adjusted levels widen the intervals.
        let wide = recalibrate(&d, &ACI_LEVELS.map(|l| (l + 0.04).min(0.995)));
        assert!(wide.interval(0.9).1 > d.interval(0.9).1);
    }

    fn disc(id: u64, day: Day, amount: f64) -> Transaction {
        Transaction {
            id: TxnId(id),
            account: AccountId(0),
            date: day,
            authorized_date: Some(day),
            status: TxnStatus::Posted,
            amount,
            kind: FlowKind::Discretionary,
            stream: None,
            pending_id: None,
        }
    }

    #[test]
    fn sparse_rule_by_hand() {
        let first = day_from_civil(1996, 1, 1);
        let origin = day_from_civil(1996, 7, 15);
        // Six complete months (Jan–Jun), 4 discretionary records each.
        let mut txns: Vec<Transaction> = (1..=6)
            .flat_map(|m| (0..4).map(move |i| (m, i)))
            .map(|(m, i)| {
                disc(
                    u64::from(m * 10 + i),
                    day_from_civil(1996, m, 3 + 5 * i),
                    -100.0,
                )
            })
            .collect();
        let s = sparse_spending(&txns, first, origin).unwrap();
        assert_eq!((s.events, s.days), (24, 197));
        // Twelve a month is not sparse.
        for m in 1..=6u32 {
            for i in 0..8u32 {
                txns.push(disc(
                    u64::from(1000 + m * 10 + i),
                    day_from_civil(1996, m, 2 + 3 * i),
                    5.0,
                ));
            }
        }
        assert!(sparse_spending(&txns, first, origin).is_none());
    }

    #[test]
    fn priors_by_hand() {
        let est = [
            (10.0, Some(1000.0), 400.0, Some(100.0)),
            (20.0, None, 900.0, None),
            (40.0, Some(3000.0), 100.0, Some(50.0)),
        ];
        let p = fit_priors(&est).unwrap();
        assert_eq!(
            (p.c_mean, p.m_mean, p.spend_var, p.income_var),
            (20.0, 2000.0, 400.0, 75.0)
        );
        // MAD of c: |10−20|, 0, |40−20| → median 10.
        assert!((p.c_sd - 14.826).abs() < 1e-9);
        assert!((p.m_sd - 1.4826 * 1000.0).abs() < 1e-9);
    }

    #[test]
    fn folds_are_balanced() {
        let mut n = [0u32; 5];
        for a in 1..=10_000u64 {
            n[fold_of(a) as usize] += 1;
        }
        assert!(n.iter().all(|&c| (1850..=2150).contains(&c)), "{n:?}");
    }
}
