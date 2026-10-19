//! Forecast calibration on synthetic data with known truth (design §5).
//!
//! For each seed, the tracker ingests the feed day by day (estimated noise,
//! default config). At several forecast origins it hands its live estimate,
//! the detected schedule, and its lumps to the Monte Carlo forecaster, and we
//! check where the true balance lands 7, 30, and 90 days later.
//!
//! Two baselines see the same "available balance" series (posted ledger plus
//! pending amounts, what a simple app shows):
//! - **30-day average:** mean daily change over the last 30 days, projected
//!   forward; Gaussian band `± z·sd·√h` (random walk with that drift).
//! - **Straight line:** least-squares line through the last 90 days, extended;
//!   band `± z·residual sd`.
//!
//! Each cell reports RMSE and MAE of the point forecast (MC: the median),
//! interval coverage, and CRPS of the whole predictive distribution (MC: from
//! a 200-quantile grid; Gaussian baselines: closed form).
//!
//! The sweeps are `#[ignore]`d; run `scripts/test-full.sh --nocapture`.

use ephemeris_core::Day;
use ephemeris_core::forecast::{ForecastConfig, ForecastModel, forecast};
use ephemeris_core::metrics::{crps_gaussian, crps_quantiles, midpoint_levels};
use ephemeris_core::schedule::detect_schedule;
use ephemeris_core::synth::{SynthConfig, generate};
use ephemeris_core::tracker::{Tracker, TrackerConfig};
use ephemeris_core::{Transaction, TxnStatus};

/// 2024-01-01.
const START: Day = 19_723;
const HORIZONS: [Day; 3] = [7, 30, 90];
/// Central intervals checked, with their two-sided normal z.
const INTERVALS: [(f64, f64); 4] = [
    (0.50, 0.674_489_750_196_081_7),
    (0.80, 1.281_551_565_544_600_4),
    (0.90, 1.644_853_626_951_472_2),
    (0.95, 1.959_963_984_540_054),
];
/// Days after START at which forecasts start (origin + 90 stays in the year).
const ORIGINS: [Day; 3] = [150, 210, 270];
const PATHS: u32 = 1000;
/// Quantiles of the MC cone used for its CRPS.
const CRPS_GRID: usize = 200;

const METHODS: [&str; 3] = ["EPHEMERIS MC", "30-day average", "straight line"];

/// The predictive distribution behind a [`Prediction`], for its CRPS.
enum Spread {
    /// Quantiles at `midpoint_levels(CRPS_GRID)`.
    Quantiles(Vec<f64>),
    Gaussian {
        sd: f64,
    },
}

/// One method's forecast for one (origin, horizon): point and interval bounds.
struct Prediction {
    point: f64,
    /// `(lo, hi)` per entry of `INTERVALS`.
    bounds: [(f64, f64); 4],
    spread: Spread,
}

impl Prediction {
    fn crps(&self, truth: f64) -> f64 {
        match &self.spread {
            Spread::Quantiles(q) => crps_quantiles(q, truth),
            Spread::Gaussian { sd } => crps_gaussian(self.point, *sd, truth),
        }
        .unwrap()
    }
}

#[derive(Default, Clone)]
struct Tally {
    n: usize,
    sq_err: f64,
    abs_err: f64,
    crps: f64,
    inside: [usize; 4],
    width90: f64,
}

impl Tally {
    fn add(&mut self, p: &Prediction, truth: f64) {
        self.n += 1;
        self.sq_err += (p.point - truth).powi(2);
        self.abs_err += (p.point - truth).abs();
        self.crps += p.crps(truth);
        for (k, &(lo, hi)) in p.bounds.iter().enumerate() {
            self.inside[k] += usize::from(lo <= truth && truth <= hi);
        }
        self.width90 += p.bounds[2].1 - p.bounds[2].0;
    }

    fn rmse(&self) -> f64 {
        (self.sq_err / self.n as f64).sqrt()
    }

    fn coverage(&self, k: usize) -> f64 {
        self.inside[k] as f64 / self.n as f64
    }
}

fn available(snap: &[Transaction], ledger: f64) -> f64 {
    ledger
        + snap
            .iter()
            .filter(|t| t.status == TxnStatus::Pending)
            .map(|t| t.amount)
            .sum::<f64>()
}

fn gaussian(point: f64, sd: f64) -> Prediction {
    Prediction {
        point,
        bounds: INTERVALS.map(|(_, z)| (point - z * sd, point + z * sd)),
        spread: Spread::Gaussian { sd },
    }
}

fn avg30(history: &[f64], h: Day) -> Prediction {
    let w = &history[history.len() - 31..];
    let d: Vec<f64> = w.windows(2).map(|p| p[1] - p[0]).collect();
    let mean = d.iter().sum::<f64>() / d.len() as f64;
    let var = d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (d.len() - 1) as f64;
    let h = f64::from(h);
    gaussian(w[w.len() - 1] + mean * h, var.sqrt() * h.sqrt())
}

fn straight_line(history: &[f64], h: Day) -> Prediction {
    let w = &history[history.len() - 90..];
    let n = w.len() as f64;
    let t_mean = (n - 1.0) / 2.0;
    let y_mean = w.iter().sum::<f64>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (t, y) in w.iter().enumerate() {
        sxy += (t as f64 - t_mean) * (y - y_mean);
        sxx += (t as f64 - t_mean).powi(2);
    }
    let slope = sxy / sxx;
    let intercept = y_mean - slope * t_mean;
    let resid = w
        .iter()
        .enumerate()
        .map(|(t, y)| (y - intercept - slope * t as f64).powi(2))
        .sum::<f64>()
        / (n - 2.0);
    gaussian(intercept + slope * (n - 1.0 + f64::from(h)), resid.sqrt())
}

/// `tallies[method][horizon]`.
fn evaluate(cfg: &SynthConfig, seeds: u64) -> Vec<Vec<Tally>> {
    let mut tallies = vec![vec![Tally::default(); HORIZONS.len()]; METHODS.len()];
    let tcfg = TrackerConfig::new(START);
    let model = ForecastModel::from_tracker(&tcfg);
    let levels: Vec<f64> = INTERVALS
        .iter()
        .flat_map(|&(c, _)| [(1.0 - c) / 2.0, (1.0 + c) / 2.0])
        .chain([0.5])
        .chain(midpoint_levels(CRPS_GRID))
        .collect();
    let median = 2 * INTERVALS.len();

    for seed in 0..seeds {
        let run = generate(cfg, seed).unwrap();
        let mut tracker = Tracker::new(tcfg).unwrap();
        let mut history = Vec::new();
        let last_origin = START + ORIGINS[ORIGINS.len() - 1];
        for k in START..=last_origin {
            let snap = run.snapshot(k);
            let ledger = run.ledger_balance(k);
            tracker.ingest(&snap, ledger, k).unwrap();
            history.push(available(&snap, ledger));
            let Some(o) = ORIGINS.iter().position(|&o| START + o == k) else {
                continue;
            };

            let est = tracker.estimate().unwrap();
            let schedule = detect_schedule(&snap);
            let fcfg = ForecastConfig {
                n_paths: PATHS,
                levels: levels.clone(),
                ..ForecastConfig::new(90, seed * 10 + o as u64)
            };
            let cone = forecast(est, &schedule, &model, &fcfg).unwrap();
            for (hi, &h) in HORIZONS.iter().enumerate() {
                let day = k + h;
                let truth = run.truth_on(day).unwrap().b;
                let band = cone.band(day).unwrap();
                let mc = Prediction {
                    point: band[median],
                    bounds: [0, 1, 2, 3].map(|i| (band[2 * i], band[2 * i + 1])),
                    spread: Spread::Quantiles(band[median + 1..].to_vec()),
                };
                tallies[0][hi].add(&mc, truth);
                tallies[1][hi].add(&avg30(&history, h), truth);
                tallies[2][hi].add(&straight_line(&history, h), truth);
            }
        }
    }
    tallies
}

fn print_table(title: &str, tallies: &[Vec<Tally>]) {
    let n = tallies[0][0].n;
    println!("\n=== {title}: {n} forecasts per cell ===");
    println!(
        "  {:<16} {:>4}  {:>9} {:>9} {:>9}  {:>6} {:>6} {:>6} {:>6}  {:>10}",
        "method", "h", "RMSE", "MAE", "CRPS", "50%", "80%", "90%", "95%", "90% width"
    );
    for (m, name) in METHODS.iter().enumerate() {
        for (hi, h) in HORIZONS.iter().enumerate() {
            let t = &tallies[m][hi];
            println!(
                "  {:<16} {:>4}  {:>9.2} {:>9.2} {:>9.2}  {:>5.1}% {:>5.1}% {:>5.1}% {:>5.1}%  {:>10.0}",
                if hi == 0 { name } else { "" },
                h,
                t.rmse(),
                t.abs_err / t.n as f64,
                t.crps / t.n as f64,
                100.0 * t.coverage(0),
                100.0 * t.coverage(1),
                100.0 * t.coverage(2),
                100.0 * t.coverage(3),
                t.width90 / t.n as f64,
            );
        }
    }
}

const SEEDS: u64 = 100;

#[test]
#[ignore = "100-seed forecast sweep; run scripts/test-full.sh"]
fn forecast_calibration() {
    let realistic = evaluate(&SynthConfig::realistic(START), SEEDS);
    let matched = evaluate(&SynthConfig::model_matched(START), SEEDS);
    print_table(
        "REALISTIC (model mismatch; reported, not asserted)",
        &realistic,
    );
    print_table("model-matched (asserted)", &matched);

    // Model-matched: every MC interval within 5 points of nominal. Forecasts
    // from the same seed share a path, so the effective sample count is below n.
    for (hi, h) in HORIZONS.iter().enumerate() {
        for (k, &(level, _)) in INTERVALS.iter().enumerate() {
            let cov = matched[0][hi].coverage(k);
            assert!(
                (cov - level).abs() <= 0.05,
                "h={h}: {level} interval covers {cov}"
            );
        }
    }
}

/// Fast smoke test: one seed through the whole pipeline.
#[test]
fn pipeline_smoke() {
    let tallies = evaluate(&SynthConfig::realistic(START), 1);
    for row in &tallies {
        for t in row {
            assert_eq!(t.n, ORIGINS.len());
            assert!(t.rmse().is_finite());
        }
    }
}
