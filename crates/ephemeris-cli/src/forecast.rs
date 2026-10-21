//! `ephemeris forecast`: tracker → schedule detection → Monte Carlo, then a
//! [`ForecastReport`] printed as text and optionally written as JSON.

use std::path::PathBuf;

use ephemeris_core::forecast::{ForecastConfig, ForecastModel, forecast};
use ephemeris_core::schedule::detect_schedule;
use ephemeris_core::schema::{Estimate, Feed, ForecastReport, PayCadence, SCHEMA_VERSION, Shift};
use ephemeris_core::tracker::{Tracker, TrackerConfig};

use crate::cone;
use crate::files::{read_json, write_json};

/// Horizons shown in the quantile table, plus the full horizon if absent.
const TABLE_HORIZONS: [u32; 4] = [7, 30, 60, 90];

pub struct Args {
    pub input: PathBuf,
    pub horizon: u32,
    pub paths: u32,
    pub seed: u64,
    pub json: Option<PathBuf>,
}

pub fn run(args: &Args) -> Result<(), String> {
    let report = build_report(args)?;
    if let Some(path) = &args.json {
        write_json(path, &report)?;
    }
    print_report(&report);
    if let Some(path) = &args.json {
        println!("\nWrote JSON report to {}", path.display());
    }
    Ok(())
}

fn build_report(args: &Args) -> Result<ForecastReport, String> {
    let feed: Feed = read_json(&args.input, "feed")?;
    if feed.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{} has schema_version {}; this build reads version {SCHEMA_VERSION}",
            args.input.display(),
            feed.schema_version
        ));
    }
    let input = args.input.display();
    let txns = feed.transactions().map_err(|e| format!("{input}: {e}"))?;

    let tcfg = TrackerConfig::new(feed.history_start.0);
    let mut tracker = Tracker::new(tcfg).map_err(|e| format!("tracker config: {e}"))?;
    tracker
        .ingest(&txns, feed.ledger_balance, feed.today.0)
        .map_err(|e| format!("tracker rejected {input}: {e}"))?;
    let est = tracker.estimate().ok_or("tracker produced no estimate")?;

    let schedule = detect_schedule(&txns);
    let fcfg = ForecastConfig {
        n_paths: args.paths,
        ..ForecastConfig::new(args.horizon, args.seed)
    };
    let cone = forecast(est, &schedule, &ForecastModel::from_tracker(&tcfg), &fcfg)
        .map_err(|e| format!("forecast: {e}"))?;
    Ok(ForecastReport::new(&feed, est, &schedule, &cone, &fcfg))
}

fn print_report(r: &ForecastReport) {
    println!(
        "EPHEMERIS forecast as of {} (settled through {})\n",
        r.as_of, r.settled_through
    );
    let row = |label: &str, e: &Estimate, unit: &str| {
        println!(
            "  {label:<12} {:>12}{unit:<5}  ±2σ {} to {}",
            money(e.mean),
            money(e.low_2sd),
            money(e.high_2sd)
        );
    };
    row("Balance", &r.balance, "");
    row("Spend rate", &r.spend_rate, "/day");
    row("Paycheck", &r.expected_paycheck, "");

    println!();
    match &r.income {
        Some(inc) => {
            let next = inc
                .next_scheduled
                .map_or_else(|| "none".to_string(), |d| d.to_string());
            let payer = inc
                .name
                .as_deref()
                .map_or_else(String::new, |n| format!("{n}, "));
            println!(
                "  Pay schedule {payer}{}, next {next} (last paid {})",
                cadence(&inc.cadence),
                inc.last_paid_nominal
            );
            let timing: Vec<String> = ["-2d", "-1d", "on time", "+1d", "+2d"]
                .iter()
                .zip(inc.offset_probabilities)
                .map(|(label, p)| format!("{label} {:.0}%", 100.0 * p))
                .collect();
            println!("               timing: {}", timing.join("  "));
        }
        None => println!("  Pay schedule none detected (the forecast includes no income)"),
    }
    if r.bills.is_empty() {
        println!("  Recurring bills: none detected");
    } else {
        println!("  Recurring bills:");
        for b in &r.bills {
            let cadence = match b.monthly_on {
                Some(on) => format!("monthly on the {}{}", ordinal(on.day), shift_note(on.shift)),
                None => format!("every {} days", b.period_days),
            };
            let name = b
                .name
                .clone()
                .unwrap_or_else(|| format!("stream {}", b.stream));
            println!(
                "    {:<22} {:>11}  {cadence:<40} last {}   next {}",
                truncate(&name, 22),
                money(b.amount),
                b.last_charged,
                b.next_due
            );
        }
    }

    let f = &r.forecast;
    println!(
        "\n  Balance quantiles ({} paths, seed {}):",
        f.paths, f.seed
    );
    let header: String = f
        .levels
        .iter()
        .map(|l| format!("{:>12}", format!("{}%", 100.0 * l)))
        .collect();
    println!("    {:>8}  {:<10}{header}", "horizon", "date");
    let mut horizons: Vec<u32> = TABLE_HORIZONS
        .into_iter()
        .filter(|&h| h <= f.horizon_days)
        .collect();
    if horizons.last() != Some(&f.horizon_days) {
        horizons.push(f.horizon_days);
    }
    for h in horizons {
        let Some(day) = f.days.get(h as usize - 1) else {
            continue;
        };
        let cells: String = day
            .quantiles
            .iter()
            .map(|&q| format!("{:>12}", money(q)))
            .collect();
        println!("    {:>3} days  {:<10}{cells}", h, day.date.to_string());
    }

    println!("\n  Balance cone:\n");
    print!("{}", cone::render(&f.levels, &f.days));
}

fn cadence(c: &PayCadence) -> String {
    match c {
        PayCadence::Weekly { anchor } => format!("weekly (from {anchor})"),
        PayCadence::Biweekly { anchor } => format!("biweekly (from {anchor})"),
        PayCadence::SemiMonthly { days: [a, b] } => {
            format!("semi-monthly on the {} and {}", ordinal(*a), ordinal(*b))
        }
        PayCadence::Monthly { day } => format!("monthly on the {}", ordinal(*day)),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).chain(['~']).collect()
    }
}

fn shift_note(s: Shift) -> &'static str {
    match s {
        Shift::None => "",
        Shift::PreviousBusinessDay => " (earlier on weekends)",
        Shift::NextBusinessDay => " (later on weekends)",
    }
}

fn ordinal(d: u32) -> String {
    let suffix = match (d % 10, d % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{d}{suffix}")
}

/// `$1,234.56`, or `-$1,234.56`.
fn money(x: f64) -> String {
    let s = format!("{:.2}", x.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, "00"));
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let sign = if x < 0.0 && s != "0.00" { "-" } else { "" };
    format!("{sign}${grouped}.{frac}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_formatting() {
        assert_eq!(money(0.0), "$0.00");
        assert_eq!(money(-0.001), "$0.00");
        assert_eq!(money(999.995), "$1,000.00");
        assert_eq!(money(1234567.891), "$1,234,567.89");
        assert_eq!(money(-48.19), "-$48.19");
        assert_eq!(money(-123456.0), "-$123,456.00");
    }

    #[test]
    fn ordinals() {
        let got: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 15, 21, 22, 23, 31]
            .map(ordinal)
            .into();
        assert_eq!(
            got,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "15th", "21st", "22nd", "23rd",
                "31st"
            ]
        );
    }
}
