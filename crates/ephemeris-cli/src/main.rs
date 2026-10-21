//! `ephemeris`: a thin command-line wrapper over `ephemeris-core`.
//!
//! The CLI loads and writes files, calls the core, and prints. All estimation
//! and forecasting math lives in the core (see CLAUDE.md).

mod files;
mod forecast;
mod simulate;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use ephemeris_core::Day;
use ephemeris_core::schema::Date;

#[derive(Parser)]
#[command(name = "ephemeris", version, about = "Financial trajectory engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a synthetic bank feed in the same format the Plaid adapter will.
    Simulate {
        #[arg(long)]
        seed: u64,
        /// Days of history to generate.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=3660))]
        days: u32,
        #[arg(long, value_enum, default_value_t = Mode::Realistic)]
        mode: Mode,
        /// First day of history.
        #[arg(long, default_value = "2024-01-01", value_parser = parse_date)]
        start: Day,
        #[arg(long)]
        out: PathBuf,
    },
    /// Estimate the current state from a feed and forecast the balance.
    Forecast {
        /// A schema-v1 feed (docs/forecast-schema.md).
        #[arg(long)]
        input: PathBuf,
        /// Days to forecast.
        #[arg(long, default_value_t = 90, value_parser = clap::value_parser!(u32).range(1..=3660))]
        horizon: u32,
        /// Monte Carlo paths.
        #[arg(long, default_value_t = 2000, value_parser = clap::value_parser!(u32).range(1..=1_000_000))]
        paths: u32,
        /// Monte Carlo seed; the same feed, seed and paths give the same cone.
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Also write the forecast report as JSON (schema version 1).
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

/// Which synthetic generator to use (design §5).
#[derive(Clone, Copy, ValueEnum)]
pub enum Mode {
    /// Departs from the filter's model: skewed spending, weekly pattern, tips and holds.
    Realistic,
    /// Drawn from exactly the model the filter assumes.
    Matched,
}

fn parse_date(s: &str) -> Result<Day, String> {
    Date::parse(s)
        .map(|d| d.0)
        .ok_or_else(|| format!("{s:?} is not a date in YYYY-MM-DD form"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Simulate {
            seed,
            days,
            mode,
            start,
            out,
        } => simulate::run(seed, days, mode, start, &out),
        Command::Forecast {
            input,
            horizon,
            paths,
            seed,
            json,
        } => forecast::run(&forecast::Args {
            input,
            horizon,
            paths,
            seed,
            json,
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}
