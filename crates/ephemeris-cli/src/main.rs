//! `ephemeris`: a thin command-line wrapper over `ephemeris-core`.
//!
//! The CLI loads and writes files, calls the core, and prints. All estimation
//! and forecasting math lives in the core (see CLAUDE.md).

mod cone;
mod files;
mod forecast;
mod plaid;
mod simulate;
mod sync;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use ephemeris_core::schema::Date;
use ephemeris_core::{Day, FlowKind};
use ephemeris_sync::edits::Edit;
use ephemeris_sync::state::{SavedQuery, Setting};

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
    /// Convert raw Plaid responses (scripts/fetch_plaid_sandbox.py) into a feed.
    ImportPlaid {
        /// Directory holding transactions_sync.json, accounts_balance.json
        /// and fetch_meta.json.
        #[arg(long, default_value = "data/sandbox")]
        raw: PathBuf,
        /// The snapshot day; defaults to fetch_meta.json's fetched_on.
        #[arg(long, value_parser = parse_date)]
        today: Option<Day>,
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
    /// Sync state: fetch into it, merge replicas, build the feed (docs/phase4b-design.md).
    #[command(subcommand)]
    Sync(SyncCommand),
}

#[derive(Subcommand)]
enum SyncCommand {
    /// Create a replica file with a new, empty sync state.
    Init {
        file: PathBuf,
        /// This replica's id (16 hex digits); random by default.
        #[arg(long)]
        replica: Option<String>,
    },
    /// Record a Plaid fetch (scripts/fetch_plaid_sandbox.py) in the sync state.
    ImportPlaid {
        file: PathBuf,
        #[arg(long, default_value = "data/sandbox")]
        raw: PathBuf,
        /// The snapshot day; defaults to fetch_meta.json's fetched_on.
        #[arg(long, value_parser = parse_date)]
        today: Option<Day>,
    },
    /// Merge other replica files into this one.
    Merge {
        file: PathBuf,
        #[arg(required = true)]
        others: Vec<PathBuf>,
    },
    /// Write the schema-v1 feed the sync state describes.
    Materialize {
        file: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Summarize a replica file.
    Show { file: PathBuf },
    /// Set or clear the kind of one transaction (by bank transaction id).
    Override {
        file: PathBuf,
        id: String,
        kind: KindArg,
    },
    /// Set or clear the kind of every transaction from a counterparty.
    Rule {
        file: PathBuf,
        counterparty: String,
        kind: KindArg,
    },
    /// Set a synced setting.
    Set {
        file: PathBuf,
        setting: SettingArg,
        value: u64,
    },
    /// Save a "can I afford it" query.
    QueryAdd {
        file: PathBuf,
        /// Balance asked about, in account currency (e.g. 2000 or 1999.99).
        #[arg(long)]
        amount: String,
        #[arg(long, value_parser = parse_date)]
        by: Day,
        #[arg(long, default_value = "")]
        label: String,
    },
    /// Change a saved query.
    QueryEdit {
        file: PathBuf,
        id: String,
        #[arg(long)]
        amount: String,
        #[arg(long, value_parser = parse_date)]
        by: Day,
        #[arg(long, default_value = "")]
        label: String,
    },
    /// Delete a saved query.
    QueryRm { file: PathBuf, id: String },
    /// List the live transactions with their final kind and where it came from.
    List {
        file: PathBuf,
        #[arg(long)]
        kind: Option<KindArg>,
        #[arg(long, value_parser = parse_date)]
        since: Option<Day>,
    },
    /// Write overrides, rules, settings and saved queries as plaintext JSON.
    Export {
        file: PathBuf,
        /// Required: the export is not encrypted.
        #[arg(long, required = true)]
        plain: bool,
        #[arg(long)]
        out: PathBuf,
    },
}

/// A flow kind, or `clear` to remove an override or rule.
#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum KindArg {
    Income,
    Scheduled,
    Discretionary,
    Transfer,
    Clear,
}

impl KindArg {
    fn kind(self) -> Option<FlowKind> {
        match self {
            KindArg::Income => Some(FlowKind::Income),
            KindArg::Scheduled => Some(FlowKind::Scheduled),
            KindArg::Discretionary => Some(FlowKind::Discretionary),
            KindArg::Transfer => Some(FlowKind::Transfer),
            KindArg::Clear => None,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SettingArg {
    HorizonDays,
    Paths,
    Seed,
}

/// A decimal amount in cents, exactly ("12.5" → 1250), never through a float.
fn parse_cents(s: &str) -> Result<i64, String> {
    let bad = || format!("{s:?} is not an amount like 2000 or 1999.99");
    let (neg, t) = match s.strip_prefix('-') {
        Some(t) => (true, t),
        None => (false, s),
    };
    let (whole, frac) = t.split_once('.').unwrap_or((t, ""));
    if whole.is_empty()
        || frac.len() > 2
        || !(whole.chars().chain(frac.chars())).all(|c| c.is_ascii_digit())
    {
        return Err(bad());
    }
    let cents = whole
        .parse::<i64>()
        .ok()
        .and_then(|w| w.checked_mul(100))
        .and_then(|w| w.checked_add(format!("{frac:0<2}").parse::<i64>().ok()?))
        .ok_or_else(bad)?;
    Ok(if neg { -cents } else { cents })
}

fn query(amount: &str, by: Day, label: String) -> Result<SavedQuery, String> {
    Ok(SavedQuery {
        amount_cents: parse_cents(amount)?,
        by: Date(by),
        label,
    })
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
        Command::ImportPlaid { raw, today, out } => plaid::run(&raw, today, &out),
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
        Command::Sync(c) => match c {
            SyncCommand::Init { file, replica } => sync::init(&file, replica.as_deref()),
            SyncCommand::ImportPlaid { file, raw, today } => sync::import_plaid(&file, &raw, today),
            SyncCommand::Merge { file, others } => sync::merge(&file, &others),
            SyncCommand::Materialize { file, out } => sync::materialize(&file, &out),
            SyncCommand::Show { file } => sync::show(&file),
            SyncCommand::Override { file, id, kind } => sync::edit(
                &file,
                &Edit::Override {
                    id,
                    kind: kind.kind(),
                },
            ),
            SyncCommand::Rule {
                file,
                counterparty,
                kind,
            } => sync::edit(
                &file,
                &Edit::Rule {
                    counterparty,
                    kind: kind.kind(),
                },
            ),
            SyncCommand::Set {
                file,
                setting,
                value,
            } => {
                let setting = match setting {
                    SettingArg::HorizonDays => Setting::HorizonDays,
                    SettingArg::Paths => Setting::Paths,
                    SettingArg::Seed => Setting::Seed,
                };
                sync::edit(&file, &Edit::Set { setting, value })
            }
            SyncCommand::QueryAdd {
                file,
                amount,
                by,
                label,
            } => query(&amount, by, label)
                .and_then(|query| sync::edit(&file, &Edit::AddQuery { query })),
            SyncCommand::QueryEdit {
                file,
                id,
                amount,
                by,
                label,
            } => query(&amount, by, label)
                .and_then(|query| sync::edit(&file, &Edit::EditQuery { id, query })),
            SyncCommand::QueryRm { file, id } => sync::edit(&file, &Edit::DeleteQuery { id }),
            SyncCommand::List { file, kind, since } => {
                sync::list(&file, kind.and_then(KindArg::kind), since)
            }
            SyncCommand::Export { file, out, .. } => sync::export_plain(&file, &out),
        },
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_cents;

    #[test]
    fn amounts_parse_to_exact_cents() {
        for (s, c) in [
            ("2000", 200_000),
            ("1999.99", 199_999),
            ("12.5", 1250),
            ("0.07", 7),
            ("-3.10", -310),
        ] {
            assert_eq!(parse_cents(s), Ok(c), "{s}");
        }
        for s in [
            "",
            ".5",
            "1.234",
            "1e3",
            "12,5",
            "--1",
            "abc",
            "99999999999999999999",
        ] {
            assert!(parse_cents(s).is_err(), "{s}");
        }
    }
}
