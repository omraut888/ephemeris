//! The Berka backtest (paper/PLAN.md §2 "Backtest protocol", §8).
//!
//! Per eligible account and origin, every method forecasts the balance 7,
//! 30 and 90 days ahead from the records on or before the origin, in the
//! origin's normalized units, and is scored against the realized balance.
//! Origins start 180 days into the account's history and repeat every 30
//! days while origin + 90 is inside the data (to 1998-12-31). An origin is
//! eligible with at least 12 records in the 180 days before it.
//!
//! Accounts run in parallel on scoped threads; results are sorted by
//! account, so the output does not depend on scheduling.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ephemeris_core::adaptive::AdaptiveConfig;
use ephemeris_core::calendar::day_from_civil;
use ephemeris_core::forecast::{ForecastConfig, ForecastModel, forecast};
use ephemeris_core::schedule::{Schedule, detect_schedule_in};
use ephemeris_core::schema::Date;
use ephemeris_core::tracker::{IngestReport, LumpHistory, StateEstimate, Tracker, TrackerConfig};
use ephemeris_core::{Day, FlowKind, Transaction};

use crate::baselines::{
    LOOKBACK, avg30, empirical_changes, schedule_aware_delta, schedule_aware_plus, seasonal,
    seasonal_delta, straight_line,
};
use crate::berka::{Account, CALENDAR, tag, transactions};
use crate::dist::{Dist, mc_levels};
use crate::reference::REFERENCE_SCALE;
use crate::rows::Row;
use crate::split::{in_ablation_sample, splitmix64};

pub const HORIZONS: [Day; 3] = [7, 30, 90];
pub const WARM_UP: Day = 180;
pub const ORIGIN_STEP: Day = 30;
pub const MIN_RECORDS_180: usize = 12;
/// Pseudo-origins for B-SA+'s past errors: weekly, from this many days into
/// the history.
pub const SA_GRID_START: Day = 30;
pub const SA_GRID_STEP: Day = 7;
pub const PATHS: u32 = 1000;

pub fn last_day() -> Day {
    day_from_civil(1998, 12, 31)
}

/// The methods every eligible origin gets.
pub const METHODS: [&str; 7] = [
    "EPHEMERIS",
    "B-30",
    "B-LIN",
    "B-SA",
    "B-SA+",
    "B-SEAS",
    "B-EQ",
];
/// Ablations of the Monte Carlo forecaster (25% sample during development).
pub const ABLATIONS: [&str; 5] = [
    "abl:fixed-noise",
    "abl:no-var-draw",
    "abl:no-lumps",
    "abl:no-detector",
    "abl:no-floor",
];

/// The forecast seed of one origin: a fixed function of (account, origin).
pub fn seed_for(account: u64, origin: Day) -> u64 {
    splitmix64((account << 32) ^ u64::from(origin as u32))
}

/// Everything one account produced.
#[derive(Debug, Default)]
pub struct AccountResult {
    pub account: u64,
    pub rows: Vec<Row>,
    /// One CSV line per (origin, horizon) for the learned baseline.
    pub features: Vec<String>,
    pub eligible_origins: usize,
    pub income_origins: usize,
    /// Share of records tagged (income, scheduled, discretionary) at the
    /// last eligible origin.
    pub kinds_last: [usize; 3],
}

pub const FEATURE_HEADER: &str = "account,origin,h,k,b0,truth,months,n30,n90,d7,d30,d90,mean30,sd30,mean90,sd90,income,income_amount,days_since_income,sa_delta,seas_delta,eq_q10,eq_q50,eq_q90,alarm";

fn tracker_config(start: Day, variant: &str) -> TrackerConfig {
    let mut t = TrackerConfig::new(start);
    t.observe.holidays = CALENDAR;
    match variant {
        "abl:fixed-noise" => {
            t.spend_var = AdaptiveConfig::fixed(t.spend_var.prior);
            t.income_var = AdaptiveConfig::fixed(t.income_var.prior);
        }
        "abl:no-detector" => {
            t.spend_change = None;
            t.income_change = None;
        }
        _ => {}
    }
    t
}

struct Estimated {
    est: StateEstimate,
    ingest: IngestReport,
}

fn estimate(
    cfg: TrackerConfig,
    txns: &[Transaction],
    ledger: f64,
    today: Day,
) -> Option<Estimated> {
    let mut tracker = Tracker::new(cfg).ok()?;
    let ingest = tracker.ingest(txns, ledger, today).ok()?;
    Some(Estimated {
        est: tracker.estimate()?.clone(),
        ingest,
    })
}

/// The Monte Carlo cone's distribution at each horizon.
fn mc(
    est: &StateEstimate,
    schedule: &Schedule,
    cfg: &TrackerConfig,
    seed: u64,
    tweak: impl Fn(&mut ForecastConfig),
) -> Option<Vec<Dist>> {
    let levels = mc_levels();
    let mut fcfg = ForecastConfig {
        n_paths: PATHS,
        levels: levels.clone(),
        ..ForecastConfig::new(HORIZONS[2] as u32, seed)
    };
    tweak(&mut fcfg);
    let cone = forecast(est, schedule, &ForecastModel::from_tracker(cfg), &fcfg).ok()?;
    HORIZONS
        .iter()
        .map(|&h| {
            let band = cone.band(est.day + h)?;
            Some(Dist::quantiles(levels.clone(), band.to_vec()))
        })
        .collect()
}

/// B-SA's raw (unnormalized) change for each horizon at one pseudo-origin.
fn sa_deltas_raw(acct: &Account, day: Day) -> [f64; 3] {
    let recs = acct.records_through(day);
    let tagging = tag(recs);
    let txns = transactions(recs, &tagging, 1.0);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    HORIZONS.map(|h| schedule_aware_delta(&txns, &schedule, day, h))
}

fn stats(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0);
    (mean, var.sqrt())
}

/// One account, with the ablations if `ablate`.
pub fn run_account(acct: &Account, ablate: bool) -> AccountResult {
    let mut out = AccountResult {
        account: acct.id,
        ..AccountResult::default()
    };
    let first = acct.first_day();
    let end = last_day();
    let bal_raw = acct.daily_balance(end);
    // B-SA's past errors come from a weekly grid of pseudo-origins.
    let mut sa_cache: HashMap<Day, [f64; 3]> = HashMap::new();

    let mut origin = first + WARM_UP;
    while origin + HORIZONS[2] <= end {
        let o = origin;
        origin += ORIGIN_STEP;
        if acct.count_in_window(o, WARM_UP) < MIN_RECORDS_180 {
            continue;
        }
        let Some(s) = acct.scale_at(o).filter(|&s| s > 0.0) else {
            continue;
        };
        let k = REFERENCE_SCALE / s;
        let t = (o - first) as usize;
        let bal: Vec<f64> = bal_raw[..=t].iter().map(|b| b * k).collect();

        let recs = acct.records_through(o);
        let tagging = tag(recs);
        let txns = transactions(recs, &tagging, k);
        let schedule = detect_schedule_in(&txns, CALENDAR);
        let seed = seed_for(acct.id, o);

        let cfg = tracker_config(first, "");
        let Some(main) = estimate(cfg, &txns, bal[t], o) else {
            continue;
        };
        let Some(mc_main) = mc(&main.est, &schedule, &cfg, seed, |_| {}) else {
            continue;
        };
        out.eligible_origins += 1;
        let income_detected = schedule.income.is_some();
        out.income_origins += usize::from(income_detected);
        out.kinds_last = [
            FlowKind::Income,
            FlowKind::Scheduled,
            FlowKind::Discretionary,
        ]
        .map(|kind| tagging.kinds.iter().filter(|&&x| x == kind).count());
        let alarm_recent = main
            .ingest
            .days
            .iter()
            .any(|d| d.day > o - 28 && !d.jumps.is_empty());
        let months_history = (o - first) / 30;

        // Ablations.
        let mut abl: Vec<(&str, Vec<Dist>)> = Vec::new();
        if ablate {
            for name in ABLATIONS {
                let dists = match name {
                    "abl:fixed-noise" | "abl:no-detector" => {
                        let c = tracker_config(first, name);
                        estimate(c, &txns, bal[t], o)
                            .and_then(|e| mc(&e.est, &schedule, &c, seed, |_| {}))
                    }
                    "abl:no-var-draw" => {
                        let mut e = main.est.clone();
                        e.spend_var.dof = 1e12;
                        e.income_var.dof = 1e12;
                        mc(&e, &schedule, &cfg, seed, |_| {})
                    }
                    "abl:no-lumps" => {
                        let mut e = main.est.clone();
                        e.lumps = LumpHistory::default();
                        mc(&e, &schedule, &cfg, seed, |f| f.lumps.rate = 0.0)
                    }
                    "abl:no-floor" => {
                        mc(&main.est, &schedule, &cfg, seed, |f| f.spend_floor = None)
                    }
                    _ => unreachable!(),
                };
                if let Some(d) = dists {
                    abl.push((name, d));
                }
            }
        }

        // Features shared by the learned baseline.
        let n_in = |w: Day| recs.iter().filter(|r| r.day > o - w).count();
        let diffs =
            |w: usize| -> Vec<f64> { bal[t - w..=t].windows(2).map(|p| p[1] - p[0]).collect() };
        let (mean30, sd30) = stats(&diffs(30));
        let (mean90, sd90) = stats(&diffs(90));
        let last_income = txns
            .iter()
            .rev()
            .find(|x| x.kind == FlowKind::Income)
            .map(|x| (x.amount, o - x.day()));

        for (hi, &h) in HORIZONS.iter().enumerate() {
            let truth = bal_raw[t + h as usize] * k;
            // B-SA and its past errors (raw units, scaled by this origin's k).
            let sa_point = bal[t] + schedule_aware_delta(&txns, &schedule, o, h);
            let mut sa_errors = Vec::new();
            let mut g = first + SA_GRID_START;
            while g + h <= o {
                if (o - g) as usize <= LOOKBACK {
                    let d = *sa_cache.entry(g).or_insert_with(|| sa_deltas_raw(acct, g));
                    let gi = (g - first) as usize;
                    let err = bal_raw[gi + h as usize] - (bal_raw[gi] + d[hi]);
                    sa_errors.push(err * k);
                }
                g += SA_GRID_STEP;
            }
            let eq = empirical_changes(&bal, t, h);
            let mut dists: Vec<(&str, Dist)> = vec![
                ("EPHEMERIS", mc_main[hi].clone()),
                ("B-30", avg30(&bal, t, h)),
                ("B-LIN", straight_line(&bal, t, h)),
                ("B-SA", Dist::Point(sa_point)),
                ("B-SA+", schedule_aware_plus(sa_point, &sa_errors)),
                ("B-EQ", eq.clone()),
            ];
            if let Some(d) = seasonal(&bal, first, t, h) {
                dists.push(("B-SEAS", d));
            }
            for (name, d) in &abl {
                dists.push((name, d[hi].clone()));
            }
            for (method, d) in dists {
                out.rows.push(Row {
                    account: acct.id,
                    origin: Date(o).to_string(),
                    h,
                    method: method.to_string(),
                    k,
                    truth,
                    score: d.score(truth),
                    income_detected,
                    months_history,
                    alarm_recent,
                });
            }

            let eq_q = |p: f64| eq.quantile(p) - bal[t];
            let seas = seasonal_delta(&bal, first, t, h).unwrap_or(0.0);
            out.features.push(format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                acct.id,
                Date(o),
                h,
                k,
                bal[t],
                truth,
                months_history,
                n_in(30),
                n_in(90),
                bal[t] - bal[t - 7],
                bal[t] - bal[t - 30],
                bal[t] - bal[t - 90],
                mean30,
                sd30,
                mean90,
                sd90,
                u8::from(income_detected),
                last_income.map_or(0.0, |x| x.0),
                last_income.map_or(-1, |x| x.1),
                sa_point - bal[t],
                seas,
                eq_q(0.1),
                eq_q(0.5),
                eq_q(0.9),
                u8::from(alarm_recent),
            ));
        }
    }
    out
}

/// Run `accounts` on `threads` threads; results in account order. The
/// ablations run on the development subsample ([`in_ablation_sample`]).
pub fn run_all(accounts: &[&Account], threads: usize, progress: bool) -> Vec<AccountResult> {
    run_all_with(accounts, threads, progress, &in_ablation_sample)
}

/// As [`run_all`], with the ablations on the accounts `ablate` picks (the
/// held-out ablation run, paper/PLAN.md §11, picks all of them). Nothing
/// else about an account's forecasts depends on it.
pub fn run_all_with(
    accounts: &[&Account],
    threads: usize,
    progress: bool,
    ablate: &(dyn Fn(u64) -> bool + Sync),
) -> Vec<AccountResult> {
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(accounts.len()));
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(acct) = accounts.get(i) else { break };
                    let r = run_account(acct, ablate(acct.id));
                    results.lock().expect("lock").push(r);
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if progress && (n.is_multiple_of(50) || n == accounts.len()) {
                        eprintln!("  {n}/{} accounts", accounts.len());
                    }
                }
            });
        }
    });
    let mut v = results.into_inner().expect("lock");
    v.sort_by_key(|r| r.account);
    v
}
