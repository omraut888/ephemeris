//! The end-to-end steps behind `ephemeris simulate` and `ephemeris forecast`,
//! shared by the CLI and the WASM build so the two cannot drift apart
//! (docs/phase2-design.md §1). Behind the `serde` feature.
//!
//! Options are serde structs whose every field has a default, so `{}` is a
//! valid options object, and unknown fields are rejected so a misspelled
//! option fails loudly instead of being ignored.

use serde::{Deserialize, Serialize};

use crate::error::EngineError;
use crate::forecast::{ForecastConfig, ForecastModel, forecast};
use crate::schedule::detect_schedule;
use crate::schema::{Date, Feed, ForecastReport, SCHEMA_VERSION};
use crate::synth::{INCOME_NAME, INCOME_STREAM, SynthConfig, generate, subscription_stream};
use crate::tracker::{Tracker, TrackerConfig};
use crate::types::Day;

/// Longest synthetic history, in days.
pub const MAX_SIMULATE_DAYS: u32 = 3660;

/// Which synthetic generator to use (design §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimMode {
    /// Departs from the filter's model: skewed spending, weekly pattern,
    /// tips and holds.
    #[default]
    Realistic,
    /// Drawn from exactly the model the filter assumes.
    Matched,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SimulateOptions {
    pub seed: u64,
    /// Days of history, 1..=3660.
    pub days: u32,
    pub mode: SimMode,
    /// First day of history.
    pub start: Date,
}

impl Default for SimulateOptions {
    /// Seed 0, one realistic year from 2024-01-01 (the CLI's defaults).
    fn default() -> Self {
        Self {
            seed: 0,
            days: 365,
            mode: SimMode::Realistic,
            start: Date(19_723),
        }
    }
}

/// A synthetic feed snapshot at the last generated day, named and rounded
/// to cents the way a bank feed would be.
pub fn simulate_feed(opts: &SimulateOptions) -> Result<Feed, EngineError> {
    if !(1..=MAX_SIMULATE_DAYS).contains(&opts.days) {
        return Err(EngineError::InvalidConfig(
            "simulate: days must be 1..=3660",
        ));
    }
    let start = opts.start.0;
    let mut cfg = match opts.mode {
        SimMode::Realistic => SynthConfig::realistic(start),
        SimMode::Matched => SynthConfig::model_matched(start),
    };
    cfg.n_days = opts.days;
    let run = generate(&cfg, opts.seed)?;
    let today = start + opts.days as Day - 1;
    let txns = run.snapshot(today);
    let mut feed = Feed::new(start, today, run.ledger_balance(today), &txns);
    // Name the known streams as a bank feed's merchant names would.
    for t in &mut feed.transactions {
        t.merchant_name = match t.stream {
            Some(s) if s == INCOME_STREAM.0 => Some(INCOME_NAME.to_string()),
            Some(s) => cfg
                .subscriptions
                .iter()
                .enumerate()
                .find(|&(i, _)| subscription_stream(i).0 == s)
                .map(|(_, sub)| sub.name.to_string()),
            None => None,
        };
    }
    // Banks report cents; so does the Plaid adapter.
    Ok(feed.rounded_to_cents())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ForecastOptions {
    /// Days to forecast, 1..=3660.
    pub horizon_days: u32,
    /// Monte Carlo paths, 1..=1,000,000.
    pub paths: u32,
    /// The same feed, options and seed give the same report, byte for byte.
    pub seed: u64,
    /// Balances X for `P(b ≥ X)` curves (`forecast.targets`).
    pub targets: Vec<f64>,
}

impl Default for ForecastOptions {
    /// 90 days, 2000 paths, seed 0, no targets (the CLI's defaults).
    fn default() -> Self {
        Self {
            horizon_days: 90,
            paths: 2000,
            seed: 0,
            targets: Vec::new(),
        }
    }
}

/// Estimate the state from a feed, detect its schedule, and forecast.
pub fn forecast_feed(feed: &Feed, opts: &ForecastOptions) -> Result<ForecastReport, EngineError> {
    if feed.schema_version != SCHEMA_VERSION {
        return Err(EngineError::InvalidInput(
            "feed schema_version is not the one this build reads (1)",
        ));
    }
    let txns = feed.transactions()?;
    let tcfg = TrackerConfig::new(feed.history_start.0);
    let mut tracker = Tracker::new(tcfg)?;
    let ingest = tracker.ingest(&txns, feed.ledger_balance, feed.today.0)?;
    let est = tracker
        .estimate()
        .ok_or(EngineError::InvalidInput("tracker produced no estimate"))?;
    let schedule = detect_schedule(&txns);
    let fcfg = ForecastConfig {
        n_paths: opts.paths,
        targets: opts.targets.clone(),
        ..ForecastConfig::new(opts.horizon_days, opts.seed)
    };
    let cone = forecast(est, &schedule, &ForecastModel::from_tracker(&tcfg), &fcfg)?;
    Ok(ForecastReport::new(
        feed, est, &ingest, &schedule, &cone, &fcfg,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_options_mean_defaults() {
        let s: SimulateOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(s, SimulateOptions::default());
        assert_eq!(s.start.to_string(), "2024-01-01");
        let f: ForecastOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(f, ForecastOptions::default());
        let s: SimulateOptions =
            serde_json::from_str(r#"{"seed": 3, "mode": "matched", "start": "2025-02-01"}"#)
                .unwrap();
        assert_eq!((s.seed, s.mode, s.days), (3, SimMode::Matched, 365));
    }

    #[test]
    fn unknown_or_bad_options_are_rejected() {
        assert!(serde_json::from_str::<ForecastOptions>(r#"{"horizon": 30}"#).is_err());
        assert!(serde_json::from_str::<SimulateOptions>(r#"{"mode": "fast"}"#).is_err());
        let opts = SimulateOptions {
            days: 0,
            ..SimulateOptions::default()
        };
        assert!(simulate_feed(&opts).is_err());
    }

    #[test]
    fn simulate_then_forecast() {
        let feed = simulate_feed(&SimulateOptions {
            days: 120,
            ..SimulateOptions::default()
        })
        .unwrap();
        assert_eq!(feed.today.0 - feed.history_start.0, 119);
        assert!(
            feed.transactions
                .iter()
                .any(|t| t.merchant_name.as_deref() == Some(INCOME_NAME))
        );
        let opts = ForecastOptions {
            horizon_days: 20,
            paths: 100,
            targets: vec![1000.0],
            ..ForecastOptions::default()
        };
        let a = forecast_feed(&feed, &opts).unwrap();
        assert_eq!(a.forecast.days.len(), 20);
        assert_eq!(a.forecast.targets.len(), 1);
        assert!(a.forecast.cycle_lows.is_some());
        // Deterministic.
        assert_eq!(a, forecast_feed(&feed, &opts).unwrap());

        let mut future = feed.clone();
        future.schema_version = 2;
        assert!(forecast_feed(&future, &opts).is_err());
        let bad = ForecastOptions {
            paths: 0,
            ..opts.clone()
        };
        assert!(forecast_feed(&feed, &bad).is_err());
    }
}
