//! Filter accuracy and calibration on synthetic data with known truth (design §5).
//!
//! Each simulated day k is treated as "today": the tracker ingests exactly what
//! the feed shows at the end of k (records plus the posted ledger balance) and
//! its live estimate is compared with the hidden truth. This exercises the
//! production path: `observe` (roles, transfer matching, settlement, pending
//! R) and `tracker` (checkpoint, tail replay, gating, adaptive noise).
//!
//! Each mode runs twice:
//! - **given**: `σ_s²` and `R_inc` fixed at the generator's values;
//! - **estimated**: both start from the default priors and adapt (design §3).
//!
//! `q_c`, `q_m`, `r_post`, and the pending-noise formula are fixed tuning
//! parameters in both. The 50-seed sweeps are `#[ignore]`d; run them with
//! `scripts/test-full.sh` (add `--nocapture` after `--` to see the table).

use ephemeris_core::Day;
use ephemeris_core::adaptive::AdaptiveConfig;
use ephemeris_core::model::{IDX_B, IDX_C, IDX_M};
use ephemeris_core::synth::{SynthConfig, generate};
use ephemeris_core::tracker::{Tracker, TrackerConfig};
use nalgebra::Vector3;

const WARMUP_DAYS: Day = 30;
const SEEDS: u64 = 50;
/// 2024-01-01.
const START: Day = 19_723;
/// The generator's paycheck variance (amount_sd = 50).
const TRUE_R_INC: f64 = 2500.0;
/// Model-matched daily spend variance (sigma = 40).
const TRUE_SPEND_MATCHED: f64 = 1600.0;
/// Realistic mode's daily spend variance, measured from 36,400 simulated days
/// (Poisson-lognormal purchases with weekday factors): 3711. Gating at the
/// χ²₁ 99.9% point clips this heavy tail; clipped at 3.29σ it is ≈ 3085.
const TRUE_SPEND_REALISTIC: f64 = 3711.0;

#[derive(Clone, Copy)]
enum Noise {
    Given { spend_var: f64 },
    Estimated,
}

fn tracker_config(noise: Noise) -> TrackerConfig {
    let mut cfg = TrackerConfig::new(START);
    if let Noise::Given { spend_var } = noise {
        cfg.spend_var = AdaptiveConfig::fixed(spend_var);
        cfg.income_var = AdaptiveConfig::fixed(TRUE_R_INC);
    }
    cfg
}

struct Sample {
    err: Vector3<f64>,
    sd: Vector3<f64>,
    ledger_err: f64,
    available_err: f64,
    in_tail: bool,
}

struct RunResult {
    samples: Vec<Sample>,
    final_spend_var: f64,
    final_income_var: f64,
}

fn evaluate(cfg: &SynthConfig, noise: Noise, seed: u64) -> RunResult {
    let run = generate(cfg, seed).unwrap();
    let mut tracker = Tracker::new(tracker_config(noise)).unwrap();
    let mut samples = Vec::new();
    for k in cfg.start_day..cfg.start_day + cfg.n_days as Day {
        let snap = run.snapshot(k);
        let ledger = run.ledger_balance(k);
        tracker.ingest(&snap, ledger, k).unwrap();
        if k - cfg.start_day < WARMUP_DAYS {
            continue;
        }
        let est = tracker.estimate().unwrap();
        let truth = run.truth_on(k).unwrap();
        let truth = Vector3::new(truth.b, truth.c, truth.m);
        let shown_pending: f64 = snap
            .iter()
            .filter(|t| t.status == ephemeris_core::TxnStatus::Pending)
            .map(|t| t.amount)
            .sum();
        samples.push(Sample {
            err: est.x - truth,
            sd: Vector3::new(
                est.p[(0, 0)].sqrt(),
                est.p[(1, 1)].sqrt(),
                est.p[(2, 2)].sqrt(),
            ),
            ledger_err: ledger - truth[IDX_B],
            available_err: ledger + shown_pending - truth[IDX_B],
            in_tail: est.settled_through < k,
        });
    }
    let est = tracker.estimate().unwrap();
    RunResult {
        samples,
        final_spend_var: est.spend_var.value,
        final_income_var: est.income_var.value,
    }
}

#[derive(Debug)]
struct Summary {
    n: usize,
    rmse_filter_b: f64,
    rmse_ledger: f64,
    rmse_available: f64,
    rmse_c: f64,
    rmse_m: f64,
    cover_b: f64,
    cover_b_tail: f64,
    tail_frac: f64,
    cover_c: f64,
    cover_m: f64,
    /// Mean over seeds of the final estimates.
    spend_var: f64,
    income_var: f64,
}

fn run_seeds(cfg: &SynthConfig, noise: Noise) -> Summary {
    let runs: Vec<RunResult> = (0..SEEDS).map(|seed| evaluate(cfg, noise, seed)).collect();
    let samples: Vec<&Sample> = runs.iter().flat_map(|r| &r.samples).collect();
    let n = samples.len() as f64;
    let rmse =
        |f: &dyn Fn(&Sample) -> f64| (samples.iter().map(|s| f(s).powi(2)).sum::<f64>() / n).sqrt();
    let cover = |xs: &[&&Sample], i: usize| {
        xs.iter()
            .filter(|s| s.err[i].abs() <= 2.0 * s.sd[i])
            .count() as f64
            / xs.len() as f64
    };
    let all: Vec<&&Sample> = samples.iter().collect();
    let tail: Vec<&&Sample> = samples.iter().filter(|s| s.in_tail).collect();
    let mean = |f: &dyn Fn(&RunResult) -> f64| runs.iter().map(f).sum::<f64>() / runs.len() as f64;
    Summary {
        n: samples.len(),
        rmse_filter_b: rmse(&|s| s.err[IDX_B]),
        rmse_ledger: rmse(&|s| s.ledger_err),
        rmse_available: rmse(&|s| s.available_err),
        rmse_c: rmse(&|s| s.err[IDX_C]),
        rmse_m: rmse(&|s| s.err[IDX_M]),
        cover_b: cover(&all, IDX_B),
        cover_b_tail: cover(&tail, IDX_B),
        tail_frac: tail.len() as f64 / n,
        cover_c: cover(&all, IDX_C),
        cover_m: cover(&all, IDX_M),
        spend_var: mean(&|r| r.final_spend_var),
        income_var: mean(&|r| r.final_income_var),
    }
}

fn print_table(mode: &str, given: &Summary, estimated: &Summary, true_spend: f64) {
    println!(
        "\n{mode}: {} samples per column ({SEEDS} seeds × days after {WARMUP_DAYS}-day warm-up)",
        given.n
    );
    println!("  {:<44} {:>12} {:>12}", "", "given noise", "estimated");
    let row = |label: &str, a: String, b: String| println!("  {label:<44} {a:>12} {b:>12}");
    let money = |v: f64| format!("${v:.2}");
    let pct = |v: f64| format!("{:.1}%", 100.0 * v);
    row(
        "RMSE b: filter",
        money(given.rmse_filter_b),
        money(estimated.rmse_filter_b),
    );
    row(
        "RMSE b: raw ledger",
        money(given.rmse_ledger),
        money(estimated.rmse_ledger),
    );
    row(
        "RMSE b: raw available",
        money(given.rmse_available),
        money(estimated.rmse_available),
    );
    row(
        "RMSE c ($/day)",
        format!("{:.2}", given.rmse_c),
        format!("{:.2}", estimated.rmse_c),
    );
    row("RMSE m", money(given.rmse_m), money(estimated.rmse_m));
    row("2σ coverage b", pct(given.cover_b), pct(estimated.cover_b));
    row(
        &format!(
            "2σ coverage b, pending-tail days ({:.0}%)",
            100.0 * given.tail_frac
        ),
        pct(given.cover_b_tail),
        pct(estimated.cover_b_tail),
    );
    row("2σ coverage c", pct(given.cover_c), pct(estimated.cover_c));
    row("2σ coverage m", pct(given.cover_m), pct(estimated.cover_m));
    row(
        &format!("final σ_s² (truth {true_spend:.0})"),
        format!("{:.0}", given.spend_var),
        format!("{:.0}", estimated.spend_var),
    );
    row(
        &format!("final R_inc (truth {TRUE_R_INC:.0})"),
        format!("{:.0}", given.income_var),
        format!("{:.0}", estimated.income_var),
    );
}

fn assert_calibrated(label: &str, s: &Summary) {
    // 2σ is a 95.4% band. Errors are autocorrelated across days, so the
    // effective sample count is far below n; allow a few points either way.
    for (name, cover) in [
        ("b tail", s.cover_b_tail),
        ("c", s.cover_c),
        ("m", s.cover_m),
    ] {
        assert!(
            (0.92..=0.99).contains(&cover),
            "{label}: {name} coverage {cover}: {s:?}"
        );
    }
}

/// Data drawn from exactly the filter's model: strict pass/fail.
#[test]
#[ignore = "50-seed sweep; run scripts/test-full.sh"]
fn model_matched_beats_raw_balance_and_is_calibrated() {
    let cfg = SynthConfig::model_matched(START);
    let given = run_seeds(
        &cfg,
        Noise::Given {
            spend_var: TRUE_SPEND_MATCHED,
        },
    );
    let estimated = run_seeds(&cfg, Noise::Estimated);
    print_table("model-matched", &given, &estimated, TRUE_SPEND_MATCHED);

    for (label, s) in [("given", &given), ("estimated", &estimated)] {
        assert!(s.rmse_filter_b < s.rmse_ledger, "{label}: {s:?}");
        assert!(s.rmse_filter_b < s.rmse_available, "{label}: {s:?}");
        assert_calibrated(label, s);
    }
    // The estimators should land near the generator's values on average.
    let rel = |v: f64, truth: f64| (v - truth).abs() / truth;
    assert!(
        rel(estimated.spend_var, TRUE_SPEND_MATCHED) < 0.10,
        "{estimated:?}"
    );
    assert!(
        rel(estimated.income_var, TRUE_R_INC) < 0.20,
        "{estimated:?}"
    );
}

/// Data that departs from the model (lognormal, weekday pattern, tips, holds,
/// missing authorized dates). Reported; only accuracy against raw is asserted.
#[test]
#[ignore = "50-seed sweep; run scripts/test-full.sh"]
fn realistic_mode_report() {
    let cfg = SynthConfig::realistic(START);
    let given = run_seeds(
        &cfg,
        Noise::Given {
            spend_var: TRUE_SPEND_REALISTIC,
        },
    );
    let estimated = run_seeds(&cfg, Noise::Estimated);
    print_table("realistic", &given, &estimated, TRUE_SPEND_REALISTIC);
    for (label, s) in [("given", &given), ("estimated", &estimated)] {
        assert!(s.rmse_filter_b < s.rmse_ledger, "{label}: {s:?}");
    }
}

/// The paycheck estimate is decoupled from balance data and both modes draw
/// identical paychecks, so its error must match bit for bit across modes,
/// with noise given or estimated.
#[test]
fn paycheck_error_is_identical_across_modes() {
    for noise in [Noise::Given { spend_var: 2000.0 }, Noise::Estimated] {
        let a = evaluate(&SynthConfig::model_matched(START), noise, 3);
        let b = evaluate(&SynthConfig::realistic(START), noise, 3);
        assert_eq!(a.samples.len(), b.samples.len());
        for (x, y) in a.samples.iter().zip(&b.samples) {
            assert_eq!(x.err[IDX_M].to_bits(), y.err[IDX_M].to_bits());
            assert_eq!(x.sd[IDX_M].to_bits(), y.sd[IDX_M].to_bits());
        }
        assert_eq!(a.final_income_var.to_bits(), b.final_income_var.to_bits());
    }
}
