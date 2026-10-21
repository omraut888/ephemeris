//! Regime changes on synthetic data with known truth (design §6).
//!
//! The generator steps the spend rate `c` (or the paycheck `m`) by a factor on
//! one day. For each seed the tracker ingests the feed day by day, with and
//! without the change detector, and we measure:
//! - **detection:** days from the step until the tracker reports a jump for
//!   a settled day on or after it;
//! - **recovery time:** days from the step until `|ĉ − c|` stays within 25%
//!   of the step for 14 days in a row;
//! - **false alarms:** balance-channel jumps per year in runs with no step;
//! - **coverage after the change:** forecasts from 1, 2, 3 and 4 weeks after
//!   the step, 30 days ahead; how often the 90% interval holds the true
//!   balance, and how wide it is next to the same seed with no step.
//!
//! The sweep is `#[ignore]`d; run `scripts/test-full.sh --nocapture`.

use std::collections::BTreeMap;
use std::ops::Range;

use ephemeris_core::Day;
use ephemeris_core::forecast::{ForecastConfig, ForecastModel, forecast};
use ephemeris_core::model::{IDX_C, IDX_M};
use ephemeris_core::schedule::detect_schedule;
use ephemeris_core::synth::{StepChange, SynthConfig, SynthRun, generate};
use ephemeris_core::tracker::{Tracker, TrackerConfig};
use ephemeris_core::{Channel, Transaction};

/// 2024-01-01.
const START: Day = 19_723;
/// The step lands on day 182; the run ends 120 days later.
const STEP: Day = START + 182;
const N_DAYS: u32 = 302;
/// Forecast origins, in days after the step.
const ORIGINS: [Day; 4] = [7, 14, 21, 28];
const HORIZON: Day = 30;
const PATHS: u32 = 1000;
/// A day the error has to stay within tolerance for, to count as recovered.
const SETTLE_DAYS: usize = 14;
const WARM_UP: Day = 30;
const END: Day = START + N_DAYS as Day;

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Spend,
    Income,
}

fn scenario(base: SynthConfig, target: Target, factor: f64) -> SynthConfig {
    let step = vec![StepChange { day: STEP, factor }];
    let mut cfg = SynthConfig {
        n_days: N_DAYS,
        ..base
    };
    match target {
        Target::Spend => cfg.spend_steps = step,
        Target::Income => cfg.income_steps = step,
    }
    cfg
}

fn tracker_config(detect: bool) -> TrackerConfig {
    let mut tcfg = TrackerConfig::new(START);
    if !detect {
        tcfg.change = None;
    }
    tcfg
}

/// One seed, one detector setting.
struct Track {
    /// Estimated `c` and `m` at the end of each day ingested.
    c: BTreeMap<Day, f64>,
    m: BTreeMap<Day, f64>,
    /// Days with a balance-channel (spend) jump, after the warm-up.
    spend_jumps: Vec<Day>,
    /// First day (of ingest) a jump on a settled day at or after the step was reported.
    detected: Option<Day>,
    /// `(covered, width)` of the 90% interval per origin.
    forecasts: Vec<(bool, f64)>,
}

/// Ingest every day in `daily`, and otherwise every 14th day, the forecast
/// origins and the last day. Ingesting less often settles exactly the same
/// days; it only skips intermediate live estimates.
fn track(run: &SynthRun, detect: bool, seed: u64, daily: Range<Day>, paths: u32) -> Track {
    let tcfg = tracker_config(detect);
    let model = ForecastModel::from_tracker(&tcfg);
    let mut tracker = Tracker::new(tcfg).unwrap();
    let mut out = Track {
        c: BTreeMap::new(),
        m: BTreeMap::new(),
        spend_jumps: Vec::new(),
        detected: None,
        forecasts: Vec::new(),
    };
    // A rebuild replays every settled day, so keep the latest report per day.
    let mut spend_jumps = BTreeMap::new();
    let end = END;
    let origin = |k: Day| ORIGINS.iter().any(|&o| STEP + o == k);
    for k in START..end {
        if !(daily.contains(&k) || (k - START) % 14 == 13 || origin(k) || k == end - 1) {
            continue;
        }
        let snap: Vec<Transaction> = run.snapshot(k);
        let report = tracker.ingest(&snap, run.ledger_balance(k), k).unwrap();
        if out.detected.is_none()
            && report
                .days
                .iter()
                .any(|d| d.settled && d.day >= STEP && !d.jumps.is_empty())
        {
            out.detected = Some(k - STEP);
        }
        for d in report.days.iter().filter(|d| d.settled) {
            let n = d
                .jumps
                .iter()
                .filter(|j| j.channel == Channel::Balance)
                .count();
            spend_jumps.insert(d.day, n);
        }
        let est = tracker.estimate().unwrap();
        out.c.insert(k, est.x[IDX_C]);
        out.m.insert(k, est.x[IDX_M]);
        if origin(k) {
            let fcfg = ForecastConfig {
                n_paths: paths,
                levels: vec![0.05, 0.95],
                ..ForecastConfig::new(HORIZON as u32, seed)
            };
            let cone = forecast(est, &detect_schedule(&snap), &model, &fcfg).unwrap();
            let band = cone.band(k + HORIZON).unwrap();
            let truth = run.truth_on(k + HORIZON).unwrap().b;
            out.forecasts
                .push((band[0] <= truth && truth <= band[1], band[1] - band[0]));
        }
    }
    for (&day, &n) in spend_jumps.range(START + WARM_UP..) {
        out.spend_jumps.extend(std::iter::repeat_n(day, n));
    }
    out
}

/// Days from the step until the estimate stays within 25% of the step size
/// for [`SETTLE_DAYS`]; `None` if it never does before `until`. Every day from
/// the step to `until` must have been ingested.
fn recovery_days(
    run: &SynthRun,
    estimate: &BTreeMap<Day, f64>,
    target: Target,
    until: Day,
) -> Option<Day> {
    let truth = |d: Day| {
        let s = run.truth_on(d).unwrap();
        match target {
            Target::Spend => s.c,
            Target::Income => s.m,
        }
    };
    let tol = 0.25 * (truth(STEP) - truth(STEP - 1)).abs();
    let ok: Vec<bool> = (STEP..until)
        .map(|d| (estimate[&d] - truth(d)).abs() <= tol)
        .collect();
    ok.windows(SETTLE_DAYS)
        .position(|w| w.iter().all(|&x| x))
        .map(|p| p as Day)
}

#[derive(Default)]
struct Summary {
    recovery: Vec<Option<Day>>,
    detected: Vec<Option<Day>>,
    false_alarms: usize,
    alarm_days: f64,
    covered: [usize; ORIGINS.len()],
    width: [f64; ORIGINS.len()],
    width_no_step: [f64; ORIGINS.len()],
    n: usize,
}

impl Summary {
    fn median_recovery(&self) -> Option<Day> {
        median(&self.recovery)
    }
}

/// Median, counting `None` (never) as larger than any day.
fn median(xs: &[Option<Day>]) -> Option<Day> {
    let mut r: Vec<Day> = xs.iter().map(|r| r.unwrap_or(Day::MAX)).collect();
    r.sort_unstable();
    let m = r[r.len() / 2];
    (m != Day::MAX).then_some(m)
}

impl Summary {
    fn recovered_within(&self, days: Day) -> f64 {
        let n = self
            .recovery
            .iter()
            .filter(|r| r.is_some_and(|d| d <= days));
        n.count() as f64 / self.recovery.len() as f64
    }

    fn coverage(&self, o: usize) -> f64 {
        self.covered[o] as f64 / self.n as f64
    }

    fn false_alarms_per_year(&self) -> f64 {
        365.0 * self.false_alarms as f64 / self.alarm_days
    }
}

/// `(with detector, without)`, both over the same seeds. Recovery is tracked
/// until `until`.
fn evaluate(
    base: &SynthConfig,
    target: Target,
    factor: f64,
    seeds: u64,
    until: Day,
    paths: u32,
) -> [Summary; 2] {
    let stepped = scenario(base.clone(), target, factor);
    let plain = scenario(base.clone(), target, 1.0);
    let mut out = [Summary::default(), Summary::default()];
    for seed in 0..seeds {
        let run = generate(&stepped, seed).unwrap();
        let run_plain = generate(&plain, seed).unwrap();
        for (s, detect) in out.iter_mut().zip([true, false]) {
            let t = track(&run, detect, seed, STEP - 3..until, paths);
            let est = match target {
                Target::Spend => &t.c,
                Target::Income => &t.m,
            };
            s.recovery.push(recovery_days(&run, est, target, until));
            s.detected.push(t.detected);
            let t_plain = track(&run_plain, detect, seed, STEP..STEP, paths);
            s.false_alarms += t_plain.spend_jumps.len();
            s.alarm_days += f64::from(N_DAYS) - f64::from(WARM_UP);
            for (o, (&(covered, width), &(_, plain_width))) in
                t.forecasts.iter().zip(&t_plain.forecasts).enumerate()
            {
                s.covered[o] += usize::from(covered);
                s.width[o] += width;
                s.width_no_step[o] += plain_width;
            }
            s.n += 1;
        }
    }
    for s in &mut out {
        for o in 0..ORIGINS.len() {
            s.width[o] /= s.n as f64;
            s.width_no_step[o] /= s.n as f64;
        }
    }
    out
}

fn print_rows(label: &str, [with, without]: &[Summary; 2]) {
    for (name, s) in [("detector", with), ("no detector", without)] {
        let recovery = s
            .median_recovery()
            .map_or_else(|| "never".to_string(), |d| d.to_string());
        let detected = median(&s.detected).map_or_else(|| "-".to_string(), |d| d.to_string());
        let recovery = format!("{detected:>4} {recovery:>5}");
        let coverage: Vec<String> = (0..ORIGINS.len())
            .map(|o| format!("{:>5.1}%", 100.0 * s.coverage(o)))
            .collect();
        let widths: Vec<String> = (0..ORIGINS.len())
            .map(|o| format!("{:>5.0}/{:<5.0}", s.width[o], s.width_no_step[o]))
            .collect();
        println!(
            "  {label:<26} {name:<12} {recovery:>6} {:>6.0}% {:>6.2}   {}   {}",
            100.0 * s.recovered_within(30),
            s.false_alarms_per_year(),
            coverage.join(" "),
            widths.join(" ")
        );
    }
}

#[test]
#[ignore = "50-seed regime-change sweep; run scripts/test-full.sh"]
fn regime_change_report() {
    const SEEDS: u64 = 50;
    println!(
        "\n=== regime changes: {SEEDS} seeds; step on day 182; 30-day forecasts from 1-4 weeks after ==="
    );
    println!(
        "  {:<26} {:<12} {:>10} {:>7} {:>6}   {:<27}   90% width, stepped/no step",
        "scenario", "", "detect/rec", "rec≤30d", "FA/yr", "90% coverage at +7/14/21/28 d"
    );
    let cases = [
        (
            "matched, spend ×2",
            SynthConfig::model_matched(START),
            Target::Spend,
            2.0,
        ),
        (
            "matched, spend ×0.5",
            SynthConfig::model_matched(START),
            Target::Spend,
            0.5,
        ),
        (
            "realistic, spend ×2",
            SynthConfig::realistic(START),
            Target::Spend,
            2.0,
        ),
        (
            "realistic, spend ×0.5",
            SynthConfig::realistic(START),
            Target::Spend,
            0.5,
        ),
        (
            "matched, paycheck ×0.8",
            SynthConfig::model_matched(START),
            Target::Income,
            0.8,
        ),
        (
            "matched, paycheck ×1.1",
            SynthConfig::model_matched(START),
            Target::Income,
            1.1,
        ),
    ];
    for (label, base, target, factor) in cases {
        print_rows(label, &evaluate(&base, target, factor, SEEDS, END, PATHS));
    }
}

/// The fast version: a doubling of spending on model-matched data, 4 seeds
/// and 200 paths (Monte Carlo dominates the run time in debug builds).
#[test]
fn detector_recovers_and_widens_the_cone() {
    let summaries = evaluate(
        &SynthConfig::model_matched(START),
        Target::Spend,
        2.0,
        4,
        STEP + 60,
        200,
    );
    print_rows("matched, spend ×2", &summaries);
    let [with, without] = &summaries;

    let rec = with.median_recovery().expect("recovers");
    assert!(rec <= 30, "median recovery {rec} days");
    assert!(
        without.median_recovery().is_none_or(|d| d > 90),
        "without the detector the random walk needs months"
    );
    // Two to four weeks after the step the 90% interval is close to nominal
    // with the detector, and almost never right without it.
    let late = |s: &Summary| (1..ORIGINS.len()).map(|o| s.coverage(o)).sum::<f64>() / 3.0;
    assert!(late(with) >= 0.75, "coverage {}", late(with));
    assert!(late(without) <= 0.3, "coverage {}", late(without));
    // Wider than the same seeds with no step while the filter relearns c.
    assert!(with.width[1] > 1.2 * with.width_no_step[1]);
}
