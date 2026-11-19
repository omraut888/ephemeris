//! Synthetic evaluation (paper/PLAN.md §3: T4's synthetic panel, T6; claims
//! A1 and B2), moved here from `ephemeris-core/tests` so that it writes rows
//! scored exactly as the Berka ones.
//!
//! A *seed* plays the part of an account: one generated run of a
//! [`Generator`]. The tracker ingests the run day by day, as the app would,
//! and at each origin the methods forecast the **true balance** 7, 30 and
//! 90 days ahead. Units are the generator's dollars (`k = 1`).
//!
//! What each method sees, all of it known on the origin's day:
//! - the Monte Carlo models (V0, V1, V2, V4): the tracker's live estimate,
//!   the detected schedule and the snapshot;
//! - the baselines: the series of *available* balances (ledger plus visible
//!   pending, what a simple app shows), and B-SA the snapshot and schedule;
//! - V1's and V4's calibration streams and B-SA+'s past errors: that same
//!   available-balance series, at days on or before the origin.
//!
//! Three protocols:
//! - [`a1`]: claim A1 exactly as asserted in the core's old
//!   `forecast_calibration` test (100 seeds, model-matched, origins at days
//!   150/210/270, forecast seed `10·seed + i`), V0 only, with the change
//!   detector off and on;
//! - [`panel`]: V0, V1, V2, V4 and every baseline on both generators, 100
//!   seeds of 730 days, origins every 30 days from day 180;
//! - [`regime`]: the six step-change scenarios of T6, with the detector on
//!   and off, forecasts from origins before and after the known change day.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ephemeris_core::forecast::{ForecastConfig, ForecastModel, forecast};
use ephemeris_core::model::{IDX_C, IDX_M};
use ephemeris_core::schedule::{Schedule, detect_schedule};
use ephemeris_core::schema::Date;
use ephemeris_core::synth::{StepChange, SynthConfig, SynthRun, generate};
use ephemeris_core::tracker::{StateEstimate, Tracker, TrackerConfig};
use ephemeris_core::{Channel, Day, FlowKind, Transaction, TxnStatus};

use crate::baselines::{
    LOOKBACK, avg30, empirical_changes, schedule_aware_delta, schedule_aware_plus, seasonal,
    seasonal_delta, straight_line,
};
use crate::berka_run::{HORIZONS, PATHS, SA_GRID_START, SA_GRID_STEP, seed_for};
use crate::dist::{Dist, mc_levels};
use crate::rows::Row;
use crate::variants::{
    ACI_GRID_START, ACI_GRID_STEP, Estimates, FOLDS, Priors, aci_levels, fit_priors, fold_of,
    recalibrate, sparse_spending,
};

/// 2024-01-01, as in the core's tests.
pub const START: Day = 19_723;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generator {
    /// Data drawn from exactly the model the filter assumes.
    Matched,
    /// Skewed, zero-inflated spending with a weekly pattern, tips, holds,
    /// missing authorized dates (design §5 mode 2).
    Realistic,
}

impl Generator {
    pub fn name(self) -> &'static str {
        match self {
            Generator::Matched => "model-matched",
            Generator::Realistic => "realistic",
        }
    }

    pub fn config(self, n_days: u32) -> SynthConfig {
        let base = match self {
            Generator::Matched => SynthConfig::model_matched(START),
            Generator::Realistic => SynthConfig::realistic(START),
        };
        SynthConfig { n_days, ..base }
    }
}

fn tracker_config(detect: bool, priors: Option<Priors>) -> TrackerConfig {
    let mut t = TrackerConfig::new(START);
    if !detect {
        t.spend_change = None;
        t.income_change = None;
    }
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

fn available(snap: &[Transaction], ledger: f64) -> f64 {
    ledger
        + snap
            .iter()
            .filter(|t| t.status == TxnStatus::Pending)
            .map(|t| t.amount)
            .sum::<f64>()
}

/// What a tracker pass hands over at a stop day.
pub struct Stop<'a> {
    pub day: Day,
    pub est: &'a StateEstimate,
    pub snap: &'a [Transaction],
    /// A regime change was reported for a day in the 28 days to `day`.
    pub alarm_recent: bool,
}

/// What a pass leaves behind: the available-balance series (index 0 is
/// [`START`]), and the estimate of `c` and `m` at the end of each day.
#[derive(Default)]
pub struct PassLog {
    pub available: Vec<f64>,
    pub c: Vec<f64>,
    pub m: Vec<f64>,
    /// First ingest day whose report has a jump on a settled day on or
    /// after `watch` (the regime sweep's detection time).
    pub detected: Option<Day>,
    /// Settled days with a balance-channel jump (latest report per day).
    pub spend_jump_days: BTreeMap<Day, usize>,
}

/// One tracker over `run`, ingesting every day from [`START`] to `last`,
/// calling `at` on each day in `stops`.
pub fn pass(
    run: &SynthRun,
    tcfg: TrackerConfig,
    last: Day,
    stops: &BTreeSet<Day>,
    watch: Option<Day>,
    mut at: impl FnMut(Stop),
) -> PassLog {
    let mut tracker = Tracker::new(tcfg).expect("tracker config");
    let mut log = PassLog::default();
    let mut jumps: BTreeMap<Day, bool> = BTreeMap::new();
    for k in START..=last {
        let snap = run.snapshot(k);
        let ledger = run.ledger_balance(k);
        let report = tracker.ingest(&snap, ledger, k).expect("ingest");
        for d in &report.days {
            jumps.insert(d.day, !d.jumps.is_empty());
            if d.settled {
                let n = d
                    .jumps
                    .iter()
                    .filter(|j| j.channel == Channel::Balance)
                    .count();
                log.spend_jump_days.insert(d.day, n);
            }
        }
        if let Some(w) = watch
            && log.detected.is_none()
            && report
                .days
                .iter()
                .any(|d| d.settled && d.day >= w && !d.jumps.is_empty())
        {
            log.detected = Some(k);
        }
        log.available.push(available(&snap, ledger));
        let est = tracker.estimate();
        log.c.push(est.map_or(f64::NAN, |e| e.x[IDX_C]));
        log.m.push(est.map_or(f64::NAN, |e| e.x[IDX_M]));
        if stops.contains(&k)
            && let Some(est) = est
        {
            at(Stop {
                day: k,
                est,
                snap: &snap,
                alarm_recent: jumps.range(k - 27..=k).any(|(_, &j)| j),
            });
        }
    }
    log
}

/// The Monte Carlo cone's distribution at each horizon.
fn cone(
    est: &StateEstimate,
    schedule: &Schedule,
    tcfg: &TrackerConfig,
    seed: u64,
    sparse: Option<ephemeris_core::forecast::SparseSpending>,
) -> Vec<Dist> {
    let levels = mc_levels();
    let fcfg = ForecastConfig {
        n_paths: PATHS,
        levels: levels.clone(),
        sparse_spending: sparse,
        ..ForecastConfig::new(HORIZONS[2] as u32, seed)
    };
    let c = forecast(est, schedule, &ForecastModel::from_tracker(tcfg), &fcfg).expect("forecast");
    HORIZONS
        .iter()
        .map(|&h| Dist::quantiles(levels.clone(), c.band(est.day + h).expect("band").to_vec()))
        .collect()
}

fn row(seed: u64, o: Day, h: Day, method: &str, truth: f64, d: &Dist, ctx: &Ctx) -> Row {
    Row {
        account: seed,
        origin: Date(o).to_string(),
        h,
        method: method.to_string(),
        k: 1.0,
        truth,
        score: d.score(truth),
        income_detected: ctx.income_detected,
        months_history: (o - START) / 30,
        alarm_recent: ctx.alarm_recent,
    }
}

/// The strata of one origin, from V0's tracker.
#[derive(Clone, Copy)]
struct Ctx {
    income_detected: bool,
    alarm_recent: bool,
}

// ---------------------------------------------------------------- A1 ----

pub const A1_ORIGINS: [Day; 3] = [150, 210, 270];
pub const A1_SEEDS: u64 = 100;
/// The tolerances asserted: detector off (the model's own assumptions) and
/// the default tracker, whose false alarms widen the cone a little.
pub const A1_TOL_OFF: f64 = 0.05;
pub const A1_TOL_ON: f64 = 0.08;

/// A1's rows for one seed: V0 on model-matched data, as in the old core
/// test (365 days, its origins and forecast seeds). The method is `V0`
/// with the detector on and `V0 no-detector` with it off.
pub fn a1_seed(seed: u64) -> Vec<Row> {
    let run = generate(&Generator::Matched.config(365), seed).expect("generate");
    let stops: BTreeSet<Day> = A1_ORIGINS.iter().map(|&o| START + o).collect();
    let last = *stops.last().expect("origins");
    let mut rows = Vec::new();
    for (detect, method) in [(true, "V0"), (false, "V0 no-detector")] {
        let tcfg = tracker_config(detect, None);
        pass(&run, tcfg, last, &stops, None, |s| {
            let i = A1_ORIGINS
                .iter()
                .position(|&o| START + o == s.day)
                .expect("origin");
            let schedule = detect_schedule(s.snap);
            let dists = cone(s.est, &schedule, &tcfg, seed * 10 + i as u64, None);
            let ctx = Ctx {
                income_detected: schedule.income.is_some(),
                alarm_recent: s.alarm_recent,
            };
            for (hi, &h) in HORIZONS.iter().enumerate() {
                let truth = run.truth_on(s.day + h).expect("truth").b;
                rows.push(row(seed, s.day, h, method, truth, &dists[hi], &ctx));
            }
        });
    }
    rows
}

/// Claim A1, cell by cell: every central interval of V0 within the
/// tolerance of nominal, compared in counts as the old test did.
pub fn a1_table(rows: &[Row]) -> (String, bool) {
    let mut out = String::from("tracker,h,level,forecasts,inside,coverage,tolerance,pass\n");
    let mut all = true;
    for (method, tracker, tol) in [
        ("V0 no-detector", "no detector", A1_TOL_OFF),
        ("V0", "default (detector on)", A1_TOL_ON),
    ] {
        for &h in &HORIZONS {
            let cell: Vec<&Row> = rows
                .iter()
                .filter(|r| r.method == method && r.h == h)
                .collect();
            let n = cell.len() as f64;
            for (i, level) in crate::dist::INTERVALS.iter().enumerate() {
                let inside = cell.iter().filter(|r| r.score.inside[i]).count() as f64;
                let pass = (inside - level * n).abs() <= (tol * n).round();
                all &= pass;
                out.push_str(&format!(
                    "{tracker},{h},{level},{n},{inside},{:.4},{tol},{}\n",
                    inside / n,
                    u8::from(pass)
                ));
            }
        }
    }
    (out, all)
}

// ------------------------------------------------------------- panel ----

pub const PANEL_DAYS: u32 = 730;
pub const PANEL_FIRST_ORIGIN: Day = 180;
pub const PANEL_STEP: Day = 30;
pub const PANEL_SEEDS: u64 = 100;
/// B-GLOBAL's training seeds: the same generator, disjoint seeds.
pub const TRAIN_SEEDS: std::ops::Range<u64> = 1000..1100;

pub const PANEL_METHODS: [&str; 10] = [
    "V0", "V1", "V2", "V4", "B-30", "B-LIN", "B-SA", "B-SA+", "B-SEAS", "B-EQ",
];

pub fn panel_origins() -> Vec<Day> {
    let last = START + PANEL_DAYS as Day - 1;
    (0..)
        .map(|i| START + PANEL_FIRST_ORIGIN + i * PANEL_STEP)
        .take_while(|&o| o + HORIZONS[2] <= last)
        .collect()
}

fn aci_grid(last_origin: Day) -> Vec<Day> {
    (0..)
        .map(|i| START + ACI_GRID_START + i * ACI_GRID_STEP)
        .take_while(|&g| g + HORIZONS[0] <= last_origin)
        .collect()
}

/// One seed's first stage: everything but V4, B-GLOBAL's features, and the
/// V0 estimates V4's priors are fitted from.
#[derive(Default)]
pub struct SeedResult {
    pub seed: u64,
    pub rows: Vec<Row>,
    pub features: Vec<String>,
    pub origins: usize,
    pub sparse_origins: usize,
    pub estimates: Option<Estimates>,
}

fn stats(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0);
    (mean, var.sqrt())
}

/// Per origin, what the V0 pass saw.
struct AtOrigin {
    dists0: Vec<Dist>,
    dists2: Option<Vec<Dist>>,
    snap: Vec<Transaction>,
    schedule: Schedule,
    ctx: Ctx,
}

pub fn panel_seed(generator: Generator, seed: u64) -> SeedResult {
    let run = generate(&generator.config(PANEL_DAYS), seed).expect("generate");
    let origins = panel_origins();
    let last = *origins.last().expect("origins");
    let grid = aci_grid(last);
    let stops: BTreeSet<Day> = origins.iter().chain(&grid).copied().collect();
    let origin_set: BTreeSet<Day> = origins.iter().copied().collect();
    let tcfg = tracker_config(true, None);

    let mut grid0: Vec<(Day, Vec<Dist>)> = Vec::new();
    let mut at: BTreeMap<Day, AtOrigin> = BTreeMap::new();
    let mut est_last: Option<Estimates> = None;
    let log = pass(&run, tcfg, last, &stops, None, |s| {
        let schedule = detect_schedule(s.snap);
        let dists0 = cone(s.est, &schedule, &tcfg, seed_for(seed, s.day), None);
        if grid.contains(&s.day) {
            grid0.push((s.day, dists0.clone()));
        }
        if !origin_set.contains(&s.day) {
            return;
        }
        let sparse = sparse_spending(s.snap, START, s.day);
        let dists2 = sparse
            .clone()
            .map(|sp| cone(s.est, &schedule, &tcfg, seed_for(seed, s.day), Some(sp)));
        if s.day == last {
            let income = schedule.income.is_some();
            est_last = Some((
                s.est.x[IDX_C],
                income.then_some(s.est.x[IDX_M]),
                s.est.spend_var.value,
                income.then_some(s.est.income_var.value),
            ));
        }
        at.insert(
            s.day,
            AtOrigin {
                ctx: Ctx {
                    income_detected: schedule.income.is_some(),
                    alarm_recent: s.alarm_recent,
                },
                dists0,
                dists2,
                snap: s.snap.to_vec(),
                schedule,
            },
        );
    });
    let bal = &log.available;
    let idx = |d: Day| (d - START) as usize;

    // B-SA's change at each weekly pseudo-origin, for B-SA+'s past errors.
    let mut sa_cache: HashMap<Day, [f64; 3]> = HashMap::new();
    let mut sa_at = |g: Day| -> [f64; 3] {
        *sa_cache.entry(g).or_insert_with(|| {
            let snap = run.snapshot(g);
            let schedule = detect_schedule(&snap);
            HORIZONS.map(|h| schedule_aware_delta(&snap, &schedule, g, h))
        })
    };

    let mut out = SeedResult {
        seed,
        estimates: est_last,
        ..SeedResult::default()
    };
    for (&o, a) in &at {
        out.origins += 1;
        out.sparse_origins += usize::from(a.dists2.is_some());
        let t = idx(o);
        let n_in = |w: Day| a.snap.iter().filter(|r| r.day() > o - w).count();
        let diffs =
            |w: usize| -> Vec<f64> { bal[t - w..=t].windows(2).map(|p| p[1] - p[0]).collect() };
        let (mean30, sd30) = stats(&diffs(30));
        let (mean90, sd90) = stats(&diffs(90));
        let last_income = a
            .snap
            .iter()
            .filter(|x| x.kind == FlowKind::Income)
            .max_by_key(|x| (x.day(), x.id))
            .map(|x| (x.amount, o - x.day()));
        for (hi, &h) in HORIZONS.iter().enumerate() {
            let truth = run.truth_on(o + h).expect("truth").b;
            let stream = aci_levels(
                grid0
                    .iter()
                    .filter(|(g, _)| g + h <= o)
                    .map(|(g, d)| (&d[hi], bal[idx(g + h)])),
            );
            let v1 = recalibrate(&a.dists0[hi], &stream);
            let v2 = a.dists2.as_ref().map_or(&a.dists0[hi], |d| &d[hi]).clone();
            let sa_point = bal[t] + schedule_aware_delta(&a.snap, &a.schedule, o, h);
            let mut sa_errors = Vec::new();
            let mut g = START + SA_GRID_START;
            while g + h <= o {
                if (o - g) as usize <= LOOKBACK {
                    let err = bal[idx(g + h)] - (bal[idx(g)] + sa_at(g)[hi]);
                    sa_errors.push(err);
                }
                g += SA_GRID_STEP;
            }
            let eq = empirical_changes(bal, t, h);
            let mut dists: Vec<(&str, Dist)> = vec![
                ("V0", a.dists0[hi].clone()),
                ("V1", v1),
                ("V2", v2),
                ("B-30", avg30(bal, t, h)),
                ("B-LIN", straight_line(bal, t, h)),
                ("B-SA", Dist::Point(sa_point)),
                ("B-SA+", schedule_aware_plus(sa_point, &sa_errors)),
                ("B-EQ", eq.clone()),
            ];
            if let Some(d) = seasonal(bal, START, t, h) {
                dists.push(("B-SEAS", d));
            }
            for (m, d) in &dists {
                out.rows.push(row(seed, o, h, m, truth, d, &a.ctx));
            }
            let eq_q = |p: f64| eq.quantile(p) - bal[t];
            out.features.push(format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                seed,
                Date(o),
                h,
                1.0,
                bal[t],
                truth,
                (o - START) / 30,
                n_in(30),
                n_in(90),
                bal[t] - bal[t - 7],
                bal[t] - bal[t - 30],
                bal[t] - bal[t - 90],
                mean30,
                sd30,
                mean90,
                sd90,
                u8::from(a.ctx.income_detected),
                last_income.map_or(0.0, |x| x.0),
                last_income.map_or(-1, |x| x.1),
                sa_point - bal[t],
                seasonal_delta(bal, START, t, h).unwrap_or(0.0),
                eq_q(0.1),
                eq_q(0.5),
                eq_q(0.9),
                u8::from(a.ctx.alarm_recent),
            ));
        }
    }
    out
}

/// V4 for one seed: V2's sparse rule with V3's priors, recalibrated as V1.
/// Strata come from V0's pass (`ctx` per origin) so every method shares them.
pub fn panel_v4(generator: Generator, seed: u64, priors: Priors, v0_rows: &[Row]) -> Vec<Row> {
    let run = generate(&generator.config(PANEL_DAYS), seed).expect("generate");
    let origins = panel_origins();
    let last = *origins.last().expect("origins");
    let grid = aci_grid(last);
    let stops: BTreeSet<Day> = origins.iter().chain(&grid).copied().collect();
    let tcfg = tracker_config(true, Some(priors));
    let mut grid23: Vec<(Day, Vec<Dist>)> = Vec::new();
    let mut at: BTreeMap<Day, Vec<Dist>> = BTreeMap::new();
    let log = pass(&run, tcfg, last, &stops, None, |s| {
        let schedule = detect_schedule(s.snap);
        let sparse = sparse_spending(s.snap, START, s.day);
        let d = cone(s.est, &schedule, &tcfg, seed_for(seed, s.day), sparse);
        if grid.contains(&s.day) {
            grid23.push((s.day, d.clone()));
        }
        if origins.contains(&s.day) {
            at.insert(s.day, d);
        }
    });
    let bal = &log.available;
    let idx = |d: Day| (d - START) as usize;
    let ctx: HashMap<(String, i32), Ctx> = v0_rows
        .iter()
        .filter(|r| r.method == "V0")
        .map(|r| {
            (
                (r.origin.clone(), r.h),
                Ctx {
                    income_detected: r.income_detected,
                    alarm_recent: r.alarm_recent,
                },
            )
        })
        .collect();
    let mut rows = Vec::new();
    for (&o, d23) in &at {
        for (hi, &h) in HORIZONS.iter().enumerate() {
            let Some(c) = ctx.get(&(Date(o).to_string(), h)) else {
                continue;
            };
            let stream = aci_levels(
                grid23
                    .iter()
                    .filter(|(g, _)| g + h <= o)
                    .map(|(g, d)| (&d[hi], bal[idx(g + h)])),
            );
            let v4 = recalibrate(&d23[hi], &stream);
            let truth = run.truth_on(o + h).expect("truth").b;
            rows.push(row(seed, o, h, "V4", truth, &v4, c));
        }
    }
    rows
}

/// V4's priors per fold, fitted from the other folds' V0 estimates (as on
/// Berka's development split).
pub fn fold_priors(results: &[SeedResult]) -> Vec<Priors> {
    (0..FOLDS)
        .map(|k| {
            let train: Vec<Estimates> = results
                .iter()
                .filter(|r| fold_of(r.seed) != k)
                .filter_map(|r| r.estimates)
                .collect();
            fit_priors(&train).expect("training folds have estimates")
        })
        .collect()
}

// ------------------------------------------------------------ regime ----

/// The step lands on day 182 after [`START`]; origins are this many days
/// from the step; the run is long enough for the last origin's 90 days.
pub const STEP_OFFSET: Day = 182;
pub const REGIME_OFFSETS: [Day; 8] = [-28, -14, -7, 7, 14, 21, 28, 56];
pub const REGIME_DAYS: u32 = 330;
pub const REGIME_SEEDS: u64 = 100;
pub const REGIME_WARM_UP: Day = 30;
pub const SETTLE_DAYS: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Spend,
    Income,
}

pub struct Scenario {
    pub name: &'static str,
    pub generator: Generator,
    pub target: Target,
    pub factor: f64,
}

pub const SCENARIOS: [Scenario; 6] = [
    Scenario {
        name: "matched: spend x2",
        generator: Generator::Matched,
        target: Target::Spend,
        factor: 2.0,
    },
    Scenario {
        name: "matched: spend x0.5",
        generator: Generator::Matched,
        target: Target::Spend,
        factor: 0.5,
    },
    Scenario {
        name: "realistic: spend x2",
        generator: Generator::Realistic,
        target: Target::Spend,
        factor: 2.0,
    },
    Scenario {
        name: "realistic: spend x0.5",
        generator: Generator::Realistic,
        target: Target::Spend,
        factor: 0.5,
    },
    Scenario {
        name: "matched: paycheck x0.8",
        generator: Generator::Matched,
        target: Target::Income,
        factor: 0.8,
    },
    Scenario {
        name: "matched: paycheck x1.1",
        generator: Generator::Matched,
        target: Target::Income,
        factor: 1.1,
    },
];

impl Scenario {
    fn config(&self, factor: f64) -> SynthConfig {
        let step = vec![StepChange {
            day: START + STEP_OFFSET,
            factor,
        }];
        let mut cfg = self.generator.config(REGIME_DAYS);
        match self.target {
            Target::Spend => cfg.spend_steps = step,
            Target::Income => cfg.income_steps = step,
        }
        cfg
    }
}

/// One seed of one scenario: rows (method `detector` or `no detector`,
/// origins before and after the step), and per detector setting the
/// detection delay, the recovery time and the false-alarm count.
#[derive(Default)]
pub struct RegimeSeed {
    pub rows: Vec<Row>,
    /// `[detector, no detector]`.
    pub detected: [Option<Day>; 2],
    pub recovery: [Option<Day>; 2],
    /// Balance-channel jumps on settled days after the warm-up, in the
    /// same seed with no step, detector on.
    pub false_alarms: usize,
    pub alarm_days: f64,
}

fn recovery_days(run: &SynthRun, log: &PassLog, target: Target, until: Day) -> Option<Day> {
    let step = START + STEP_OFFSET;
    let truth = |d: Day| {
        let s = run.truth_on(d).expect("truth");
        match target {
            Target::Spend => s.c,
            Target::Income => s.m,
        }
    };
    let est = match target {
        Target::Spend => &log.c,
        Target::Income => &log.m,
    };
    let tol = 0.25 * (truth(step) - truth(step - 1)).abs();
    let ok: Vec<bool> = (step..=until)
        .map(|d| (est[(d - START) as usize] - truth(d)).abs() <= tol)
        .collect();
    ok.windows(SETTLE_DAYS)
        .position(|w| w.iter().all(|&x| x))
        .map(|p| p as Day)
}

pub fn regime_seed(sc: &Scenario, seed: u64) -> RegimeSeed {
    let step = START + STEP_OFFSET;
    let run = generate(&sc.config(sc.factor), seed).expect("generate");
    let last = START + REGIME_DAYS as Day - 1;
    let stops: BTreeSet<Day> = REGIME_OFFSETS.iter().map(|&o| step + o).collect();
    let mut out = RegimeSeed::default();
    for (i, (detect, method)) in [(true, "detector"), (false, "no detector")]
        .into_iter()
        .enumerate()
    {
        let tcfg = tracker_config(detect, None);
        let mut rows = Vec::new();
        let log = pass(&run, tcfg, last, &stops, Some(step), |s| {
            let schedule = detect_schedule(s.snap);
            let dists = cone(s.est, &schedule, &tcfg, seed_for(seed, s.day), None);
            let ctx = Ctx {
                income_detected: schedule.income.is_some(),
                alarm_recent: s.alarm_recent,
            };
            for (hi, &h) in HORIZONS.iter().enumerate() {
                let truth = run.truth_on(s.day + h).expect("truth").b;
                rows.push(row(seed, s.day, h, method, truth, &dists[hi], &ctx));
            }
        });
        out.rows.extend(rows);
        out.detected[i] = log.detected.map(|k| k - step);
        out.recovery[i] = recovery_days(&run, &log, sc.target, last);
    }
    let plain = generate(&sc.config(1.0), seed).expect("generate");
    let log = pass(
        &plain,
        tracker_config(true, None),
        last,
        &BTreeSet::new(),
        None,
        |_| {},
    );
    out.false_alarms = log
        .spend_jump_days
        .range(START + REGIME_WARM_UP..)
        .map(|(_, &n)| n)
        .sum();
    out.alarm_days = f64::from(REGIME_DAYS) - f64::from(REGIME_WARM_UP);
    out
}

/// Median with `None` (never) above every day.
pub fn median_days(xs: &[Option<Day>]) -> Option<Day> {
    let mut r: Vec<Day> = xs.iter().map(|r| r.unwrap_or(Day::MAX)).collect();
    r.sort_unstable();
    let m = r[r.len() / 2];
    (m != Day::MAX).then_some(m)
}

// ------------------------------------------------------------ plumbing --

/// `f` over `items` on `threads` threads; results in item order.
pub fn parallel_map<I: Sync, T: Send>(
    items: &[I],
    threads: usize,
    label: &str,
    f: impl Fn(&I) -> T + Sync,
) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(x) = items.get(i) else { break };
                    let r = f(x);
                    out.lock().expect("lock").push((i, r));
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(25) || n == items.len() {
                        eprintln!("  {label}: {n}/{}", items.len());
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

    /// Claim A1 (moved from the core's `forecast_calibration` sweep): every
    /// central interval of V0 within 5 points of nominal with the detector
    /// off, and within 8 with the default tracker, at 7, 30 and 90 days.
    #[test]
    #[ignore = "100-seed A1 sweep; run scripts/test-full.sh"]
    fn a1_coverage_within_tolerance() {
        let seeds: Vec<u64> = (0..A1_SEEDS).collect();
        let rows: Vec<Row> = parallel_map(&seeds, 8, "A1", |&s| a1_seed(s))
            .into_iter()
            .flatten()
            .collect();
        let (table, pass) = a1_table(&rows);
        assert!(pass, "{table}");
    }

    #[test]
    fn panel_origins_fit_in_the_run() {
        let o = panel_origins();
        assert_eq!(o.first(), Some(&(START + 180)));
        assert_eq!(o.len(), 16);
        assert!(o.last().unwrap() + 90 < START + PANEL_DAYS as Day);
        assert!(START + STEP_OFFSET + 56 + 90 < START + REGIME_DAYS as Day);
    }

    #[test]
    fn one_panel_seed_scores_every_method() {
        let r = panel_seed(Generator::Realistic, 3);
        assert_eq!(r.origins, 16);
        for m in PANEL_METHODS.iter().filter(|&&m| m != "V4") {
            let n = r.rows.iter().filter(|x| x.method == *m).count();
            assert_eq!(n, 48, "{m}");
        }
        assert_eq!(r.features.len(), 48);
        assert!(r.estimates.is_some());
    }
}
