//! `ephemeris-eval`: the paper's experiments (paper/PLAN.md §6).
//!
//! ```text
//! ephemeris-eval reference-scale
//! ephemeris-eval berka --out paper/build/berka-dev            # development split
//! ephemeris-eval berka --held-out --out paper/build/berka-heldout
//! ephemeris-eval score-quantiles --features F --quantiles Q --method B-GLOBAL --out R
//! ephemeris-eval tables --rows R1 --rows R2 --out paper/results/dev
//! ```
//!
//! The held-out split runs only with `--held-out`, from a clean working
//! tree, and each run is appended to `paper/results/heldout-runs.log`. A
//! second run is refused unless `--rerun-reason` says why; the reason is
//! logged too.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};
use ephemeris_core::calendar::civil_from_day;
use ephemeris_eval::aggregate::{main_table, pit_table, strata_table};
use ephemeris_eval::berka::{Account, MAX_DRIFT_CZK, parse_trans};
use ephemeris_eval::berka_run::{ABLATIONS, FEATURE_HEADER, METHODS, run_all_with};
use ephemeris_eval::dist::Dist;
use ephemeris_eval::reference::{REFERENCE_SCALE, compute_reference_scale};
use ephemeris_eval::rows::{HEADER, Row, read_rows};
use ephemeris_eval::split::{Split, in_ablation_sample, split_of};
use ephemeris_eval::variants::{Priors, VariantResult};

#[derive(Parser)]
#[command(name = "ephemeris-eval", about = "Phase 6 experiments (paper/PLAN.md)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Recompute the reference flow scale (PLAN §8 decision 2).
    ReferenceScale,
    /// Backtest every method on the Berka accounts of one split.
    Berka {
        #[arg(long, default_value = "data/berka")]
        data: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Run the held-out split instead of development. Logged; once.
        #[arg(long)]
        held_out: bool,
        /// Why a held-out run is being repeated (required after the first).
        #[arg(long)]
        rerun_reason: Option<String>,
        /// Only the first N accounts of the split (for quick checks).
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, default_value_t = 8)]
        threads: usize,
    },
    /// Score a learned model's quantiles of the h-day balance change.
    /// The held-out evaluation, once (PLAN §10): guarded and logged; runs
    /// every method and V0–V4 on the held-out accounts.
    Heldout {
        #[arg(long, default_value = "data/berka")]
        data: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Why a held-out run is being repeated (required after the first).
        #[arg(long)]
        rerun_reason: Option<String>,
        #[arg(long, default_value_t = 8)]
        threads: usize,
    },
    /// The pre-registered ablations (PLAN §3, D1) on every held-out
    /// account: a separately guarded and logged held-out run (PLAN §11),
    /// after the first run omitted them. Writes the full backtest of those
    /// accounts, whose non-ablation rows must equal the first run's.
    HeldoutAblations {
        #[arg(long, default_value = "data/berka")]
        data: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Required: the held-out split has been run before.
        #[arg(long)]
        rerun_reason: Option<String>,
        #[arg(long, default_value_t = 8)]
        threads: usize,
    },
    /// The ablation tables of one run's rows, against the full model
    /// (EPHEMERIS): the main table, PIT, and paired differences overall and
    /// by recent regime alarm.
    AblationTables {
        #[arg(long, required = true)]
        rows: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Paired comparisons from scored rows, no new forecasts (post-hoc,
    /// PLAN §11): `--pair METHOD/REFERENCE`, repeated.
    Pairwise {
        #[arg(long, required = true)]
        rows: Vec<PathBuf>,
        #[arg(long, required = true)]
        pair: Vec<String>,
        #[arg(long)]
        out: PathBuf,
    },
    /// The synthetic evaluation (PLAN §3: A1, the T4 synthetic panel, T6):
    /// rows and features into `--out`.
    Synthetic {
        #[arg(long)]
        out: PathBuf,
        /// Seeds per panel and regime scenario (default: the protocol's).
        #[arg(long)]
        seeds: Option<u64>,
        #[arg(long, default_value_t = 8)]
        threads: usize,
    },
    /// Tables from `synthetic`'s rows (and B-GLOBAL's, if scored).
    SyntheticTables {
        #[arg(long)]
        build: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// The MoneyData case study (one account, descriptive): rows into
    /// `--out`, tables into `--results`.
    Moneydata {
        #[arg(long, default_value = "data/moneydata")]
        data: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        results: PathBuf,
    },
    /// Figure data the tables do not hold (coverage at every level from the
    /// held-out rows; one development account's example cone).
    FigureData {
        #[arg(long, default_value = "data/berka")]
        data: PathBuf,
        /// The held-out run's build directory (its scored rows).
        #[arg(long)]
        heldout: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Prefix equivalence on development origins (review response): the
    /// forecast from a feed truncated at the origin equals the frozen one.
    PrefixCheck {
        #[arg(long, default_value = "data/berka")]
        data: PathBuf,
        /// Frozen development rows (variants and the main backtest).
        #[arg(long, required = true)]
        frozen: Vec<PathBuf>,
        #[arg(long, default_value_t = 200)]
        sample: usize,
        #[arg(long, default_value_t = 20_261_121)]
        seed: u64,
        #[arg(long)]
        out: PathBuf,
    },
    /// Exploratory (paper/REVISION.md S1–S6), development split only:
    /// `eligibility`, `crossings`, `berka` or `grid`.
    Explore {
        #[arg(value_parser = ["eligibility", "crossings", "berka", "grid", "phase", "bglobal", "detector", "schedule", "shortfall", "income-trace", "startbal"])]
        what: String,
        /// Exploratory held-out run (`berka` or `bglobal` only): guarded and
        /// logged like the frozen evaluation, with a required reason.
        #[arg(long)]
        held_out: bool,
        #[arg(long)]
        rerun_reason: Option<String>,
        /// `schedule` only: keep origins on or after this date (YYYY-MM-DD).
        /// Required with `--held-out` (the round 4 confirmatory test).
        #[arg(long)]
        from: Option<String>,
        #[arg(long, default_value = "data/berka")]
        data: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 8)]
        threads: usize,
    },
    ScoreQuantiles {
        #[arg(long)]
        features: PathBuf,
        #[arg(long)]
        quantiles: PathBuf,
        #[arg(long)]
        method: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// The revision candidates V0–V4 (PLAN §9), development split only.
    Variants {
        #[arg(long, default_value = "data/berka")]
        data: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, default_value_t = 8)]
        threads: usize,
    },
    /// Tables and the selection rule for V0–V4 (PLAN §9).
    VariantTables {
        /// The variants' rows, then the main backtest's (for the baselines).
        #[arg(long, required = true)]
        rows: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Aggregate rows into the tables.
    Tables {
        #[arg(long, required = true)]
        rows: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::ReferenceScale => {
            let r = compute_reference_scale();
            println!("{r:?} (pinned: {REFERENCE_SCALE:?})");
            Ok(())
        }
        Cmd::Berka {
            data,
            out,
            held_out,
            rerun_reason,
            limit,
            threads,
        } => berka(&data, &out, held_out, rerun_reason, limit, threads),
        Cmd::Heldout {
            data,
            out,
            rerun_reason,
            threads,
        } => heldout(&data, &out, rerun_reason, threads),
        Cmd::HeldoutAblations {
            data,
            out,
            rerun_reason,
            threads,
        } => heldout_ablations(&data, &out, rerun_reason, threads),
        Cmd::AblationTables { rows, out } => ablation_tables(&rows, &out),
        Cmd::Pairwise { rows, pair, out } => pairwise(&rows, &pair, &out),
        Cmd::Synthetic {
            out,
            seeds,
            threads,
        } => synthetic(&out, seeds, threads),
        Cmd::SyntheticTables { build, out } => synthetic_tables(&build, &out),
        Cmd::Moneydata { data, out, results } => moneydata(&data, &out, &results),
        Cmd::FigureData { data, heldout, out } => figure_data(&data, &heldout, &out),
        Cmd::PrefixCheck {
            data,
            frozen,
            sample,
            seed,
            out,
        } => prefix_check(&data, &frozen, sample, seed, &out),
        Cmd::Explore {
            what,
            data,
            out,
            threads,
            held_out,
            rerun_reason,
            from,
        } => explore(&what, &data, &out, threads, held_out, rerun_reason, from),
        Cmd::ScoreQuantiles {
            features,
            quantiles,
            method,
            out,
        } => score_quantiles(&features, &quantiles, &method, &out),
        Cmd::Tables { rows, out } => tables(&rows, &out),
        Cmd::Variants {
            data,
            out,
            limit,
            threads,
        } => variants(&data, &out, limit, threads),
        Cmd::VariantTables { rows, out } => variant_tables(&rows, &out),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn git(args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {args:?} failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_day((secs / 86_400) as i32);
    let s = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

const HELD_OUT_LOG: &str = "paper/results/heldout-runs.log";

/// Refuse or record a held-out run (PLAN §8 data rules).
fn guard_held_out(rerun_reason: Option<&str>, args: &str) -> Result<(), String> {
    if !git(&["status", "--porcelain"])?.is_empty() {
        return Err("a held-out run needs a clean working tree (commit first)".into());
    }
    let commit = git(&["rev-parse", "HEAD"])?;
    let log = Path::new(HELD_OUT_LOG);
    let prior = fs::read_to_string(log).unwrap_or_default();
    let runs = prior
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .count();
    if runs > 0 && rerun_reason.is_none() {
        return Err(format!(
            "the held-out split has already been run ({runs} entr{} in {HELD_OUT_LOG}); \
             a rerun needs --rerun-reason",
            if runs == 1 { "y" } else { "ies" }
        ));
    }
    let mut text = if prior.is_empty() {
        String::from("# Every held-out evaluation run: UTC time, commit, arguments, reason.\n")
    } else {
        prior
    };
    writeln!(
        text,
        "{}\t{commit}\t{args}\t{}",
        utc_now(),
        rerun_reason.unwrap_or("first run")
    )
    .unwrap();
    write(log, &text)
}

fn berka(
    data: &Path,
    out: &Path,
    held_out: bool,
    rerun_reason: Option<String>,
    limit: Option<usize>,
    threads: usize,
) -> Result<(), String> {
    if !REFERENCE_SCALE.is_finite() {
        return Err("REFERENCE_SCALE is not pinned yet (run reference-scale)".into());
    }
    if held_out || rerun_reason.is_some() {
        // One guarded, logged run must cover the backtest and the variants
        // together (PLAN §10), so the held-out split runs only through
        // `heldout`.
        return Err("the held-out split runs only through `ephemeris-eval heldout`".into());
    }
    eprintln!("reading {}", data.display());
    let accounts = parse_trans(&read(&data.join("trans.tsv"))?)?;
    berka_split(
        &accounts,
        Split::Dev,
        limit,
        out,
        threads,
        &in_ablation_sample,
    )
}

/// The held-out evaluation (PLAN §10), exactly once: the guard and its log
/// entry, then the backtest of every method and the variants V0–V4 on the
/// held-out accounts, with V3's priors from all development accounts.
fn heldout(
    data: &Path,
    out: &Path,
    rerun_reason: Option<String>,
    threads: usize,
) -> Result<(), String> {
    use ephemeris_eval::variants::{pooled_priors, run_with_priors};
    if !REFERENCE_SCALE.is_finite() {
        return Err("REFERENCE_SCALE is not pinned yet (run reference-scale)".into());
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    guard_held_out(rerun_reason.as_deref(), &args.join(" "))?;

    eprintln!("reading {}", data.display());
    let accounts = parse_trans(&read(&data.join("trans.tsv"))?)?;
    berka_split(
        &accounts,
        Split::HeldOut,
        None,
        &out.join("berka"),
        threads,
        &in_ablation_sample,
    )?;

    let usable = |s: Split| -> Vec<&Account> {
        accounts
            .values()
            .filter(|a| split_of(a.id) == s)
            .filter(|a| a.stats().max_drift <= MAX_DRIFT_CZK)
            .collect()
    };
    let dev = usable(Split::Dev);
    let held = usable(Split::HeldOut);
    eprintln!("V3 priors from {} development accounts", dev.len());
    let t0 = std::time::Instant::now();
    let priors = pooled_priors(&dev, threads);
    eprintln!("variants V0–V4 on {} held-out accounts", held.len());
    let results = run_with_priors(&held, threads, priors);
    let secs = t0.elapsed().as_secs_f64();
    write_variants(
        &out.join("variants"),
        &results,
        held.len(),
        secs,
        &[("priors_from_all_development", priors)],
    )
}

/// The held-out ablations (PLAN §11): guarded and logged like `heldout`,
/// then the backtest of every held-out account with the ablations on all
/// of them. No variant, no learned baseline.
fn heldout_ablations(
    data: &Path,
    out: &Path,
    rerun_reason: Option<String>,
    threads: usize,
) -> Result<(), String> {
    if !REFERENCE_SCALE.is_finite() {
        return Err("REFERENCE_SCALE is not pinned yet (run reference-scale)".into());
    }
    if rerun_reason.is_none() {
        return Err("the held-out ablations are a second held-out run: --rerun-reason".into());
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    guard_held_out(rerun_reason.as_deref(), &args.join(" "))?;
    eprintln!("reading {}", data.display());
    let accounts = parse_trans(&read(&data.join("trans.tsv"))?)?;
    berka_split(&accounts, Split::HeldOut, None, out, threads, &|_| true)
}

fn read_all_rows(paths: &[PathBuf]) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for p in paths {
        rows.extend(read_rows(&read(p)?)?);
    }
    Ok(rows)
}

fn ablation_tables(paths: &[PathBuf], out: &Path) -> Result<(), String> {
    use ephemeris_eval::aggregate::{Stratum, pairwise_table};
    let rows: Vec<Row> = read_all_rows(paths)?
        .into_iter()
        .filter(|r| r.method == "EPHEMERIS" || r.method.starts_with("abl:"))
        .collect();
    let abl_accounts: std::collections::HashSet<u64> = rows
        .iter()
        .filter(|r| r.method.starts_with("abl:"))
        .map(|r| r.account)
        .collect();
    let rows: Vec<Row> = rows
        .into_iter()
        .filter(|r| abl_accounts.contains(&r.account))
        .collect();
    let mut order = vec!["EPHEMERIS"];
    order.extend(ABLATIONS);
    write(
        &out.join("ablations.csv"),
        &main_table(&rows, "EPHEMERIS", &order, |_| true),
    )?;
    write(&out.join("ablations-pit.csv"), &pit_table(&rows, &order))?;
    let pairs: Vec<(&str, &str)> = ABLATIONS.iter().map(|&a| (a, "EPHEMERIS")).collect();
    let all: &dyn Fn(&Row) -> bool = &|_| true;
    let alarm: &dyn Fn(&Row) -> bool = &|r| r.alarm_recent;
    let quiet: &dyn Fn(&Row) -> bool = &|r| !r.alarm_recent;
    let strata: [Stratum; 3] = [
        ("all", all),
        ("regime alarm in last 28 days", alarm),
        ("no recent alarm", quiet),
    ];
    write(
        &out.join("ablations-paired.csv"),
        &pairwise_table(&rows, &pairs, &strata),
    )?;
    eprintln!(
        "{} rows, {} accounts → {}",
        rows.len(),
        abl_accounts.len(),
        out.display()
    );
    Ok(())
}

fn pairwise(paths: &[PathBuf], pairs: &[String], out: &Path) -> Result<(), String> {
    use ephemeris_eval::aggregate::pairwise_table;
    let pairs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|p| {
            p.split_once('/')
                .ok_or(format!("{p}: want METHOD/REFERENCE"))
        })
        .collect::<Result<_, _>>()?;
    let rows = read_all_rows(paths)?;
    let all: &dyn Fn(&Row) -> bool = &|_| true;
    write(out, &pairwise_table(&rows, &pairs, &[("all", all)]))?;
    eprintln!(
        "{} pairs from {} rows → {}",
        pairs.len(),
        rows.len(),
        out.display()
    );
    Ok(())
}

/// The MoneyData case study. One account, so the account bootstrap's
/// intervals are degenerate: the tables keep point estimates only.
fn moneydata(data: &Path, out: &Path, results: &Path) -> Result<(), String> {
    use ephemeris_eval::moneydata::{METHODS, parse, run, summary};
    let acct = parse(&read(&data.join("transactions.tsv"))?)?;
    let t0 = std::time::Instant::now();
    let cs = run(&acct);
    let secs = t0.elapsed().as_secs_f64();
    write(&out.join("rows.csv"), &rows_csv(&cs.rows))?;
    let mut order = vec!["B-SA+"];
    order.extend(METHODS.iter().filter(|&&m| m != "B-SA+"));
    let points = |table: String| -> String {
        let mut lines = table.lines();
        let header: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
        let keep: Vec<usize> = (0..header.len())
            .filter(|&i| !header[i].ends_with("_lo") && !header[i].ends_with("_hi"))
            .collect();
        let pick = |l: &str| {
            let f: Vec<&str> = l.split(',').collect();
            keep.iter().map(|&i| f[i]).collect::<Vec<_>>().join(",")
        };
        let mut s = pick(&header.join(","));
        s.push('\n');
        for l in lines {
            s.push_str(&pick(l));
            s.push('\n');
        }
        s
    };
    write(
        &results.join("main.csv"),
        &points(main_table(&cs.rows, "B-SA+", &order, |_| true)),
    )?;
    write(&results.join("pit.csv"), &pit_table(&cs.rows, &order))?;
    let mut d = String::from("key,value\n");
    for (k, v) in summary(&acct) {
        writeln!(d, "{k},{v}").unwrap();
    }
    writeln!(
        d,
        "eligible_origins,{}\norigins_income_detected,{}\norigins_sparse_v2,{}",
        cs.origins, cs.income_origins, cs.sparse_origins
    )
    .unwrap();
    write(&results.join("data.csv"), &d)?;
    eprintln!("{d}wall_seconds,{secs:.1}\nwrote {}", results.display());
    Ok(())
}

fn figure_data(data: &Path, heldout: &Path, out: &Path) -> Result<(), String> {
    use ephemeris_eval::figures::{cone_csv, coverage_curve, example_account};
    let rows = read_all_rows(&[
        heldout.join("variants/rows.csv"),
        heldout.join("berka/rows.csv"),
        heldout.join("rows-lgbm.csv"),
    ])?;
    let methods = ["V0", "V1", "V2", "B-SA+", "B-EQ", "B-GLOBAL"];
    write(
        &out.join("coverage-curve-heldout.csv"),
        &coverage_curve(&rows, &methods),
    )?;
    drop(rows);
    let accounts = parse_trans(&read(&data.join("trans.tsv"))?)?;
    let dev: Vec<&Account> = accounts
        .values()
        .filter(|a| split_of(a.id) == Split::Dev)
        .filter(|a| a.stats().max_drift <= MAX_DRIFT_CZK)
        .collect();
    let (acct, origin) = example_account(&dev).ok_or("no example account")?;
    let cone = cone_csv(acct, origin).ok_or("cone failed")?;
    write(&out.join("example-cone.csv"), &cone)?;
    write(
        &out.join("example.csv"),
        &format!(
            "key,value\naccount,{}\norigin,{}\nrecords,{}\nsplit,development\n",
            acct.id,
            ephemeris_core::schema::Date(origin),
            acct.records.len()
        ),
    )?;
    eprintln!("figure data → {}", out.display());
    Ok(())
}

fn prefix_check(
    data: &Path,
    frozen: &[PathBuf],
    sample: usize,
    seed: u64,
    out: &Path,
) -> Result<(), String> {
    let tsv = read(&data.join("trans.tsv"))?;
    let mut texts = Vec::new();
    for p in frozen {
        texts.push(read(p)?);
    }
    let mut lines = texts
        .iter()
        .flat_map(|t| t.lines().skip(1).map(str::to_string));
    let (csv, summary) = ephemeris_eval::audit::prefix_check(&tsv, &mut lines, sample, seed)?;
    write(&out.join("prefix-check.csv"), &csv)?;
    write(
        &out.join("prefix-check-summary.csv"),
        &format!("key,value\n{summary}"),
    )?;
    eprint!("{summary}");
    Ok(())
}

/// The development accounts (all of them, or only those the frozen drift
/// rule keeps).
/// The accounts of one split that the frozen drift rule keeps.
fn split_accounts(data: &Path, split: Split) -> Result<Vec<Account>, String> {
    let accounts = parse_trans(&read(&data.join("trans.tsv"))?)?;
    Ok(accounts
        .into_values()
        .filter(|a| split_of(a.id) == split)
        .filter(|a| a.stats().max_drift <= MAX_DRIFT_CZK)
        .collect())
}

fn dev_accounts(data: &Path, kept_only: bool) -> Result<Vec<Account>, String> {
    let accounts = parse_trans(&read(&data.join("trans.tsv"))?)?;
    Ok(accounts
        .into_values()
        .filter(|a| split_of(a.id) == Split::Dev)
        .filter(|a| !kept_only || a.stats().max_drift <= MAX_DRIFT_CZK)
        .collect())
}

fn explore(
    what: &str,
    data: &Path,
    out: &Path,
    threads: usize,
    held_out: bool,
    rerun_reason: Option<String>,
    from: Option<String>,
) -> Result<(), String> {
    use ephemeris_eval::explore as ex;
    let from: Option<ephemeris_core::Day> = match &from {
        None => None,
        Some(s) => {
            let p: Vec<&str> = s.split('-').collect();
            let num = |i: usize| p.get(i).and_then(|x| x.parse::<u32>().ok());
            match (p.len(), num(0), num(1), num(2)) {
                (3, Some(y), Some(m), Some(d)) => {
                    Some(ephemeris_core::calendar::day_from_civil(y as i32, m, d))
                }
                _ => return Err(format!("--from {s}: expected YYYY-MM-DD")),
            }
        }
    };
    if from.is_some() && what != "schedule" {
        return Err("--from applies to `schedule` only".into());
    }
    if held_out {
        if !["berka", "bglobal", "startbal", "schedule"].contains(&what) {
            return Err(format!("`{what}` has no held-out mode"));
        }
        if what == "schedule" && from.is_none() {
            return Err("the held-out `schedule` run is the forward-period test: --from".into());
        }
        if rerun_reason.is_none() {
            return Err("an exploratory held-out run needs --rerun-reason".into());
        }
        let args: Vec<String> = std::env::args().skip(1).collect();
        guard_held_out(rerun_reason.as_deref(), &args.join(" "))?;
    } else if what == "bglobal" {
        return Err("`bglobal` is the held-out mode of S7; use --held-out".into());
    }
    let split = if held_out { Split::HeldOut } else { Split::Dev };
    use ephemeris_eval::synthetic::{
        GRID_REGULARITY, GRID_SEEDS, GRID_SPARSITY, grid_config, grid_seed, parallel_map,
    };
    let t0 = std::time::Instant::now();
    match what {
        "eligibility" => {
            let accts = dev_accounts(data, false)?;
            let res = parallel_map(&accts, threads, "eligibility", ex::eligibility_account);
            let mut a = format!("{}\n", ex::ELIG_ACCOUNT_HEADER);
            let mut o = format!("{}\n", ex::ELIG_ORIGIN_HEADER);
            let mut todo = Vec::new();
            for (acct, r) in accts.iter().zip(&res) {
                a.push_str(&r.account_csv);
                a.push('\n');
                for l in &r.origin_csv {
                    o.push_str(l);
                    o.push('\n');
                }
                todo.extend(r.p_not_f.iter().map(|&d| (acct, d)));
            }
            write(&out.join("accounts.csv"), &a)?;
            write(&out.join("origins.csv"), &o)?;
            // Origins only the past-only rule admits: V2, B-SA+, B-EQ.
            let rows: Vec<Vec<Row>> = parallel_map(&todo, threads, "P not F", |&(acct, o)| {
                let Some((f0, methods)) = ephemeris_eval::audit::methods(acct, o) else {
                    return Vec::new();
                };
                let first = acct.first_day();
                let bal = acct.daily_balance(o + 90);
                let mut v = Vec::new();
                for (name, dists) in &methods {
                    if !["V2", "B-SA+", "B-EQ"].contains(&name.as_str()) {
                        continue;
                    }
                    for (hi, &h) in [7, 30, 90].iter().enumerate() {
                        let truth = bal[(o + h - first) as usize] * f0.k;
                        v.push(Row {
                            account: acct.id,
                            origin: ephemeris_core::schema::Date(o).to_string(),
                            h,
                            method: name.clone(),
                            k: f0.k,
                            truth,
                            score: dists[hi].score(truth),
                            income_detected: f0.income_detected,
                            months_history: (o - first) / 30,
                            alarm_recent: f0.alarm_recent,
                        });
                    }
                }
                v
            });
            let rows: Vec<Row> = rows.into_iter().flatten().collect();
            write(&out.join("rows-p-not-f.csv"), &rows_csv(&rows))?;
            eprintln!("P not F: {} origins", todo.len());
        }
        "crossings" => {
            let accts = dev_accounts(data, true)?;
            let res = parallel_map(&accts, threads, "crossings", ex::v1_crossings);
            let mut c = String::from("account,origin,h,monotone,values_moved,values\n");
            for l in res.into_iter().flatten() {
                c.push_str(&l);
                c.push('\n');
            }
            write(&out.join("crossings.csv"), &c)?;
        }
        "bglobal" => {
            // Train on development features only; predict held-out origins.
            let py = Command::new("paper/.venv/bin/python")
                .args([
                    "-I",
                    "paper/baselines/lgbm_cf.py",
                    "--features",
                    "paper/build/berka-dev/features.csv",
                    "--predict",
                    "paper/build/heldout/berka/features.csv",
                    "--jobs",
                    "3",
                    "--out",
                ])
                .arg(out)
                .status()
                .map_err(|e| format!("python: {e}"))?;
            if !py.success() {
                return Err("lgbm_cf.py failed".into());
            }
            for v in ["cf", "cut"] {
                score_quantiles(
                    Path::new("paper/build/heldout/berka/features.csv"),
                    &out.join(format!("quantiles-{v}.csv")),
                    &format!("B-GLOBAL-{v}"),
                    &out.join(format!("rows-{v}.csv")),
                )?;
            }
        }
        "berka" => {
            let accts = split_accounts(data, split)?;
            let items: Vec<(&Account, i32)> = accts
                .iter()
                .flat_map(|a| {
                    ephemeris_eval::variants::eligible_origins(a)
                        .into_iter()
                        .map(move |o| (a, o))
                })
                .collect();
            let res = parallel_map(&items, threads, "berka", |&(a, o)| ex::berka_origin(a, o));
            let mut rows = Vec::new();
            let mut events = format!("{}\n", ex::EVENT_HEADER);
            let (mut n, mut fb) = (0, [0usize; 3]);
            for r in res.into_iter().flatten() {
                n += 1;
                for (i, f) in r.cal_fallback.iter().enumerate() {
                    fb[i] += usize::from(*f);
                }
                rows.extend(r.rows);
                for e in r.events {
                    events.push_str(&e);
                    events.push('\n');
                }
            }
            write(&out.join("rows.csv"), &rows_csv(&rows))?;
            write(&out.join("events.csv"), &events)?;
            write(
                &out.join("data.csv"),
                &format!(
                    "key,value\norigins,{n}\nitems,{}\ncal_fallback_7,{}\ncal_fallback_30,{}\ncal_fallback_90,{}\n",
                    items.len(),
                    fb[0],
                    fb[1],
                    fb[2]
                ),
            )?;
        }
        "detector" => {
            // S9 (REVISION.md, round 3 A1): development only.
            let accts = split_accounts(data, Split::Dev)?;
            let items: Vec<(&Account, i32)> = accts
                .iter()
                .flat_map(|a| {
                    ephemeris_eval::variants::eligible_origins(a)
                        .into_iter()
                        .map(move |o| (a, o))
                })
                .collect();
            let res = parallel_map(&items, threads, "detector", |&(a, o)| {
                ex::detector_origin(a, o)
            });
            let mut rows = Vec::new();
            let mut state = format!("{}\n", ex::DETECTOR_STATE_HEADER);
            let mut n = 0;
            for (r, s) in res.into_iter().flatten() {
                n += 1;
                rows.extend(r);
                for l in s {
                    state.push_str(&l);
                    state.push('\n');
                }
            }
            write(&out.join("rows.csv"), &rows_csv(&rows))?;
            write(&out.join("state.csv"), &state)?;
            write(
                &out.join("data.csv"),
                &format!("key,value\norigins,{n}\nitems,{}\n", items.len()),
            )?;
        }
        "schedule" => {
            // S11/S14 on development; with --held-out and --from, the round 4
            // forward-period confirmatory test (REVISION.md S15), guarded.
            let accts = split_accounts(data, split)?;
            let lo = from.unwrap_or(ephemeris_core::Day::MIN);
            let items: Vec<(&Account, i32)> = accts
                .iter()
                .flat_map(|a| {
                    ephemeris_eval::variants::eligible_origins(a)
                        .into_iter()
                        .filter(move |&o| o >= lo)
                        .map(move |o| (a, o))
                })
                .collect();
            let res = parallel_map(&items, threads, "schedule", |&(a, o)| {
                ex::schedule_origin(a, o)
            });
            let mut rows = Vec::new();
            let mut state = format!("{}\n", ex::SCHEDULE_STATE_HEADER);
            let mut n = 0;
            for (r, s) in res.into_iter().flatten() {
                n += 1;
                rows.extend(r);
                state.push_str(&s);
                state.push('\n');
            }
            write(&out.join("rows.csv"), &rows_csv(&rows))?;
            write(&out.join("state.csv"), &state)?;
            write(
                &out.join("data.csv"),
                &format!("key,value\norigins,{n}\nitems,{}\n", items.len()),
            )?;
        }
        "shortfall" => {
            // S12 (REVISION.md, round 3 B2): development only.
            let accts = split_accounts(data, Split::Dev)?;
            let per: Vec<(&Account, Vec<i32>)> = accts
                .iter()
                .map(|a| (a, ephemeris_eval::variants::eligible_origins(a)))
                .collect();
            let items: Vec<(&Account, i32)> = per
                .iter()
                .flat_map(|(a, os)| os.iter().map(move |&o| (*a, o)))
                .collect();
            let res = parallel_map(&items, threads, "shortfall", |&(a, o)| {
                ex::shortfall_origin(a, o)
            });
            let mut ev = format!("{}\n", ex::SHORTFALL_HEADER);
            let mut n = 0;
            for lines in res.into_iter().flatten() {
                n += 1;
                for l in lines {
                    ev.push_str(&l);
                    ev.push('\n');
                }
            }
            let mut acc = String::from("account,origins,episodes,days_below\n");
            for (a, os) in &per {
                if let Some(l) = ex::shortfall_account(a, os) {
                    acc.push_str(&l);
                    acc.push('\n');
                }
            }
            write(&out.join("events.csv"), &ev)?;
            write(&out.join("accounts.csv"), &acc)?;
            write(
                &out.join("data.csv"),
                &format!("key,value\norigins,{n}\nitems,{}\n", items.len()),
            )?;
        }
        "income-trace" => {
            // S13 (REVISION.md, round 4 item 1): development only.
            let accts = split_accounts(data, Split::Dev)?;
            let res = parallel_map(&accts, threads, "income-trace", ex::income_trace);
            let mut s = format!("{}\n", ex::INCOME_TRACE_HEADER);
            for l in res.into_iter().flatten().flatten() {
                s.push_str(&l);
                s.push('\n');
            }
            write(&out.join("trace.csv"), &s)?;
        }
        "startbal" => {
            // Round 4 item 4: each eligible origin's booked end-of-day
            // balance at the origin, in the frozen normalized units. No
            // forecasts. Held-out mode is guarded and logged.
            let accts = split_accounts(data, split)?;
            let mut s = String::from("account,origin,b0\n");
            for a in &accts {
                let first = a.first_day();
                for o in ephemeris_eval::variants::eligible_origins(a) {
                    let Some(sc) = a.scale_at(o).filter(|&x| x > 0.0) else {
                        continue;
                    };
                    let k = ephemeris_eval::reference::REFERENCE_SCALE / sc;
                    let b = a.daily_balance(o)[(o - first) as usize] * k;
                    s.push_str(&format!(
                        "{},{},{b}\n",
                        a.id,
                        ephemeris_core::schema::Date(o)
                    ));
                }
            }
            write(&out.join("startbal.csv"), &s)?;
        }
        "phase" => {
            let accts = dev_accounts(data, true)?;
            // Offset 0 is only the identity check, on the first 100 accounts.
            for offset in [0, 10, 20] {
                let take = if offset == 0 { 100 } else { accts.len() };
                let items: Vec<(&Account, i32)> = accts
                    .iter()
                    .take(take)
                    .flat_map(|a| {
                        ex::shifted_origins(a, offset)
                            .into_iter()
                            .map(move |o| (a, o))
                    })
                    .collect();
                let res = parallel_map(&items, threads, &format!("phase +{offset}"), |&(a, o)| {
                    ex::phase_origin(a, o)
                });
                let rows: Vec<Row> = res.into_iter().flatten().collect();
                write(
                    &out.join(format!("rows-offset{offset}.csv")),
                    &rows_csv(&rows),
                )?;
            }
        }
        "grid" => {
            let mut summary = String::from("sparsity,regularity,mean_count,origins,sparse_fired\n");
            for (si, &mean_count) in GRID_SPARSITY.iter().enumerate() {
                for ri in 0..GRID_REGULARITY {
                    let cfg = grid_config(si, ri);
                    let seeds: Vec<u64> = (0..GRID_SEEDS).collect();
                    let res = parallel_map(&seeds, threads, &format!("cell {si},{ri}"), |&s| {
                        grid_seed(&cfg, s)
                    });
                    let fired: usize = res.iter().map(|r| r.1).sum();
                    let rows: Vec<Row> = res.into_iter().flat_map(|r| r.0).collect();
                    let origins = rows.iter().filter(|r| r.method == "V0" && r.h == 7).count();
                    write(&out.join(format!("rows-s{si}-r{ri}.csv")), &rows_csv(&rows))?;
                    writeln!(summary, "{si},{ri},{mean_count},{origins},{fired}").unwrap();
                }
            }
            write(&out.join("data.csv"), &summary)?;
        }
        _ => unreachable!(),
    }
    eprintln!(
        "{what}: {:.1} s → {}",
        t0.elapsed().as_secs_f64(),
        out.display()
    );
    Ok(())
}

type RowFilter = Box<dyn Fn(&Row) -> bool>;

fn rows_csv(rows: &[Row]) -> String {
    let mut s = String::from(HEADER);
    s.push('\n');
    for r in rows {
        s.push_str(&r.csv());
        s.push('\n');
    }
    s
}

/// The synthetic evaluation: A1, the panel on both generators (plus
/// B-GLOBAL's training features from disjoint seeds), V4, and the regime
/// sweep. Every output is a pure function of the seeds.
fn synthetic(out: &Path, seeds: Option<u64>, threads: usize) -> Result<(), String> {
    use ephemeris_eval::synthetic::{
        A1_SEEDS, Generator, PANEL_SEEDS, REGIME_SEEDS, SCENARIOS, TRAIN_SEEDS, a1_seed,
        fold_priors, median_days, panel_seed, panel_v4, parallel_map, regime_seed,
    };
    use ephemeris_eval::variants::fold_of;
    let t0 = std::time::Instant::now();
    let mut summary = String::from("key,value\n");

    // A1: always the full protocol (it is a pre-registered check).
    let a1_seeds: Vec<u64> = (0..A1_SEEDS).collect();
    let a1: Vec<Row> = parallel_map(&a1_seeds, threads, "A1", |&s| a1_seed(s))
        .into_iter()
        .flatten()
        .collect();
    write(&out.join("a1/rows.csv"), &rows_csv(&a1))?;
    writeln!(summary, "a1_seeds,{A1_SEEDS}").unwrap();

    let n = seeds.unwrap_or(PANEL_SEEDS);
    for generator in [Generator::Matched, Generator::Realistic] {
        let name = generator.name();
        let panel_seeds: Vec<u64> = (0..n).collect();
        let results = parallel_map(&panel_seeds, threads, name, |&s| panel_seed(generator, s));
        let priors = fold_priors(&results);
        let v4: Vec<Vec<Row>> = parallel_map(&results, threads, &format!("{name} V4"), |r| {
            panel_v4(generator, r.seed, priors[fold_of(r.seed) as usize], &r.rows)
        });
        let train_seeds: Vec<u64> = TRAIN_SEEDS.take(n as usize).collect();
        let train = parallel_map(&train_seeds, threads, &format!("{name} training"), |&s| {
            panel_seed(generator, s)
        });
        let mut rows: Vec<Row> = Vec::new();
        let mut features = format!("{FEATURE_HEADER}\n");
        let (mut origins, mut sparse) = (0, 0);
        for (r, v) in results.iter().zip(&v4) {
            rows.extend(r.rows.iter().cloned());
            rows.extend(v.iter().cloned());
            origins += r.origins;
            sparse += r.sparse_origins;
            for f in &r.features {
                features.push_str(f);
                features.push('\n');
            }
        }
        let mut train_features = format!("{FEATURE_HEADER}\n");
        for r in &train {
            for f in &r.features {
                train_features.push_str(f);
                train_features.push('\n');
            }
        }
        let dir = out.join(name);
        write(&dir.join("rows.csv"), &rows_csv(&rows))?;
        write(&dir.join("features.csv"), &features)?;
        write(&dir.join("train-features.csv"), &train_features)?;
        writeln!(
            summary,
            "{name}_seeds,{n}\n{name}_origins,{origins}\n{name}_origins_sparse_v2,{sparse}\n{name}_train_seeds,{}",
            train_seeds.len()
        )
        .unwrap();
        for (k, p) in priors.iter().enumerate() {
            writeln!(
                summary,
                "{name}_fold{k}_priors,c {:.4} sd {:.4} | m {:.4} sd {:.4} | spend_var {:.4} | income_var {:.4}",
                p.c_mean, p.c_sd, p.m_mean, p.m_sd, p.spend_var, p.income_var
            )
            .unwrap();
        }
    }

    // The regime sweep.
    let rs = seeds.unwrap_or(REGIME_SEEDS);
    let mut detection = String::from(
        "scenario,tracker,seeds,detected_share,median_detect_days,median_recovery_days,recovered_within_30,false_alarms_per_year\n",
    );
    for (i, sc) in SCENARIOS.iter().enumerate() {
        let seeds: Vec<u64> = (0..rs).collect();
        let results = parallel_map(&seeds, threads, sc.name, |&s| regime_seed(sc, s));
        let rows: Vec<Row> = results
            .iter()
            .flat_map(|r| r.rows.iter().cloned())
            .collect();
        write(&out.join(format!("regime/{i}/rows.csv")), &rows_csv(&rows))?;
        let fa: usize = results.iter().map(|r| r.false_alarms).sum();
        let days: f64 = results.iter().map(|r| r.alarm_days).sum();
        for (j, tracker) in ["detector", "no detector"].iter().enumerate() {
            let det: Vec<Option<i32>> = results.iter().map(|r| r.detected[j]).collect();
            let rec: Vec<Option<i32>> = results.iter().map(|r| r.recovery[j]).collect();
            let share = |xs: &[Option<i32>], f: &dyn Fn(i32) -> bool| {
                xs.iter().filter(|x| x.is_some_and(f)).count() as f64 / xs.len() as f64
            };
            let fmt_days =
                |d: Option<i32>| d.map_or_else(|| "never".to_string(), |d| d.to_string());
            writeln!(
                detection,
                "{},{tracker},{rs},{:.4},{},{},{:.4},{:.4}",
                sc.name,
                share(&det, &|_| true),
                fmt_days(median_days(&det)),
                fmt_days(median_days(&rec)),
                share(&rec, &|d| d <= 30),
                if j == 0 {
                    365.0 * fa as f64 / days
                } else {
                    0.0
                },
            )
            .unwrap();
        }
    }
    write(&out.join("regime/detection.csv"), &detection)?;
    writeln!(summary, "regime_seeds,{rs}").unwrap();
    let secs = t0.elapsed().as_secs_f64();
    write(&out.join("data.csv"), &summary)?;
    eprintln!("{summary}wall_seconds,{secs:.1}\nwrote {}", out.display());
    Ok(())
}

fn synthetic_tables(build: &Path, out: &Path) -> Result<(), String> {
    use ephemeris_eval::aggregate::{Stratum, pairwise_table};
    use ephemeris_eval::synthetic::{
        Generator, PANEL_METHODS, REGIME_OFFSETS, SCENARIOS, START, STEP_OFFSET, a1_table,
    };
    let a1 = read_rows(&read(&build.join("a1/rows.csv"))?)?;
    let (t, pass) = a1_table(&a1);
    write(&out.join("a1.csv"), &t)?;
    eprintln!(
        "A1: {}",
        if pass {
            "every cell within tolerance"
        } else {
            "FAILS"
        }
    );

    let all: &dyn Fn(&Row) -> bool = &|_| true;
    for generator in [Generator::Matched, Generator::Realistic] {
        let name = generator.name();
        let dir = build.join(name);
        let mut paths = vec![dir.join("rows.csv")];
        let lgbm = dir.join("rows-lgbm.csv");
        if lgbm.exists() {
            paths.push(lgbm);
        }
        let rows = read_all_rows(&paths)?;
        let mut order: Vec<&str> = vec!["B-SA+"];
        order.extend(PANEL_METHODS.iter().filter(|&&m| m != "B-SA+"));
        order.push("B-GLOBAL");
        write(
            &out.join(format!("{name}-main.csv")),
            &main_table(&rows, "B-SA+", &order, |_| true),
        )?;
        write(
            &out.join(format!("{name}-pit.csv")),
            &pit_table(&rows, &order),
        )?;
        let baselines = [
            "B-SA+", "B-EQ", "B-GLOBAL", "B-30", "B-LIN", "B-SA", "B-SEAS",
        ];
        let pairs: Vec<(&str, &str)> = ["V0", "V2"]
            .iter()
            .flat_map(|&m| baselines.iter().map(move |&b| (m, b)))
            .collect();
        write(
            &out.join(format!("{name}-pairwise.csv")),
            &pairwise_table(&rows, &pairs, &[("all", all)]),
        )?;
    }

    // Regime: per scenario and origin offset, coverage and CRPS with the
    // detector on and off, and the paired difference.
    let step = START + STEP_OFFSET;
    let mut coverage = String::new();
    let mut paired = String::new();
    for (i, sc) in SCENARIOS.iter().enumerate() {
        let rows = read_rows(&read(&build.join(format!("regime/{i}/rows.csv")))?)?;
        let dates: Vec<(i32, String)> = REGIME_OFFSETS
            .iter()
            .map(|&o| (o, ephemeris_core::schema::Date(step + o).to_string()))
            .collect();
        for (o, date) in &dates {
            let t = main_table(&rows, "no detector", &["no detector", "detector"], |r| {
                &r.origin == date
            });
            for (j, line) in t.lines().enumerate() {
                if j == 0 {
                    if coverage.is_empty() {
                        coverage = format!("scenario,offset,{line}\n");
                    }
                    continue;
                }
                writeln!(coverage, "{},{o},{line}", sc.name).unwrap();
            }
        }
        let filters: Vec<(String, RowFilter)> = dates
            .iter()
            .map(|(o, date)| {
                let d = date.clone();
                (
                    format!("{o}"),
                    Box::new(move |r: &Row| r.origin == d) as RowFilter,
                )
            })
            .collect();
        let strata: Vec<Stratum> = filters
            .iter()
            .map(|(n, f)| (n.as_str(), f.as_ref() as &dyn Fn(&Row) -> bool))
            .collect();
        let t = pairwise_table(&rows, &[("detector", "no detector")], &strata);
        for (j, line) in t.lines().enumerate() {
            if j == 0 {
                if paired.is_empty() {
                    paired = format!("scenario,{}\n", line.replacen("stratum", "offset", 1));
                }
                continue;
            }
            writeln!(paired, "{},{line}", sc.name).unwrap();
        }
    }
    write(&out.join("regime-coverage.csv"), &coverage)?;
    write(&out.join("regime-paired.csv"), &paired)?;
    fs::copy(
        build.join("regime/detection.csv"),
        out.join("regime-detection.csv"),
    )
    .map_err(|e| format!("copy: {e}"))?;
    let data = read(&build.join("data.csv"))?;
    write(&out.join("data.csv"), &data)?;
    eprintln!("synthetic tables → {}", out.display());
    Ok(())
}

fn berka_split(
    accounts: &std::collections::BTreeMap<u64, Account>,
    split: Split,
    limit: Option<usize>,
    out: &Path,
    threads: usize,
    ablate: &(dyn Fn(u64) -> bool + Sync),
) -> Result<(), String> {
    let held_out = split == Split::HeldOut;
    let total = accounts.len();
    let mut chosen: Vec<&Account> = accounts
        .values()
        .filter(|a| split_of(a.id) == split)
        .collect();
    let in_split = chosen.len();
    // Data-quality exclusion (berka::MAX_DRIFT_CZK), before any forecast.
    chosen.retain(|a| a.stats().max_drift <= MAX_DRIFT_CZK);
    let excluded = in_split - chosen.len();
    if let Some(n) = limit {
        chosen.truncate(n);
    }
    eprintln!(
        "{in_split} of {total} accounts in the {} split; running {}",
        if held_out { "held-out" } else { "development" },
        chosen.len()
    );
    let t0 = std::time::Instant::now();
    let results = run_all_with(&chosen, threads, true, ablate);
    let secs = t0.elapsed().as_secs_f64();

    let mut rows = String::from(HEADER);
    rows.push('\n');
    let mut features = String::from(FEATURE_HEADER);
    features.push('\n');
    let mut per_account = String::from(
        "account,ablation,records,days_with_records,days_consistent,max_drift_czk,eligible_origins,income_origins,income_records,scheduled_records,discretionary_records,records_per_month\n",
    );
    let mut drifts = Vec::new();
    let (mut eligible_accounts, mut origins, mut income_origins) = (0, 0, 0);
    let (mut records, mut days, mut consistent) = (0, 0, 0);
    let mut per_month = Vec::new();
    for (acct, r) in chosen.iter().zip(&results) {
        assert_eq!(acct.id, r.account);
        for row in &r.rows {
            rows.push_str(&row.csv());
            rows.push('\n');
        }
        for f in &r.features {
            features.push_str(f);
            features.push('\n');
        }
        let s = acct.stats();
        let months = f64::from(acct.last_day() - acct.first_day() + 1) / 30.4375;
        let rpm = s.records as f64 / months;
        writeln!(
            per_account,
            "{},{},{},{},{},{},{},{},{},{},{},{rpm:.3}",
            acct.id,
            u8::from(ablate(acct.id)),
            s.records,
            s.days_with_records,
            s.days_consistent,
            s.max_drift,
            r.eligible_origins,
            r.income_origins,
            r.kinds_last[0],
            r.kinds_last[1],
            r.kinds_last[2]
        )
        .unwrap();
        records += s.records;
        days += s.days_with_records;
        consistent += s.days_consistent;
        per_month.push(rpm);
        drifts.push(s.max_drift);
        if r.eligible_origins > 0 {
            eligible_accounts += 1;
        }
        origins += r.eligible_origins;
        income_origins += r.income_origins;
    }
    let median_rpm = ephemeris_eval::berka::median(&mut per_month).unwrap_or(f64::NAN);
    let median_drift = ephemeris_eval::berka::median(&mut drifts).unwrap_or(f64::NAN);
    let max_drift = drifts.iter().copied().fold(0.0, f64::max);
    let summary = format!(
        "key,value\n\
         accounts_total,{total}\n\
         accounts_in_split,{in_split}\n\
         accounts_excluded_balance_inconsistent,{excluded}\n\
         accounts_run,{}\n\
         accounts_with_eligible_origin,{eligible_accounts}\n\
         ablation_accounts,{}\n\
         eligible_origins,{origins}\n\
         origins_with_income_detected,{income_origins}\n\
         records,{records}\n\
         days_with_records,{days}\n\
         days_balance_consistent,{consistent}\n\
         median_records_per_month,{median_rpm:.3}\n\
         median_account_max_drift_czk,{median_drift}\n\
         max_drift_czk,{max_drift}\n\
         reference_scale,{REFERENCE_SCALE}\n\
         wall_seconds,{secs:.1}\n\
         threads,{threads}\n",
        chosen.len(),
        chosen.iter().filter(|a| ablate(a.id)).count(),
    );
    write(&out.join("rows.csv"), &rows)?;
    write(&out.join("features.csv"), &features)?;
    write(&out.join("accounts.csv"), &per_account)?;
    write(&out.join("data.csv"), &summary)?;
    eprintln!("{summary}wrote {}", out.display());
    Ok(())
}

/// Rows for a learned model's quantiles of the balance change. The
/// quantile file has `account,origin,h` then one column per level, named
/// by the level (e.g. `q0.05`).
fn score_quantiles(
    features: &Path,
    quantiles: &Path,
    method: &str,
    out: &Path,
) -> Result<(), String> {
    let ftext = read(features)?;
    let mut flines = ftext.lines();
    let fh: Vec<&str> = flines.next().ok_or("empty features")?.split(',').collect();
    let col = |name: &str| {
        fh.iter()
            .position(|&c| c == name)
            .ok_or(format!("no {name}"))
    };
    let (ca, co, ch, ck, cb, ct, cm, ci, cal) = (
        col("account")?,
        col("origin")?,
        col("h")?,
        col("k")?,
        col("b0")?,
        col("truth")?,
        col("months")?,
        col("income")?,
        col("alarm")?,
    );
    let mut ctx = std::collections::HashMap::new();
    for l in flines {
        let f: Vec<&str> = l.split(',').collect();
        ctx.insert((f[ca].to_string(), f[co].to_string(), f[ch].to_string()), f);
    }
    let qtext = read(quantiles)?;
    let mut qlines = qtext.lines();
    let qh: Vec<&str> = qlines.next().ok_or("empty quantiles")?.split(',').collect();
    if qh[..3] != ["account", "origin", "h"] {
        return Err("quantile file must start with account,origin,h".into());
    }
    let levels: Vec<f64> = qh[3..]
        .iter()
        .map(|c| {
            c.trim_start_matches('q')
                .parse::<f64>()
                .map_err(|e| format!("{c}: {e}"))
        })
        .collect::<Result<_, _>>()?;
    let num = |s: &str| s.parse::<f64>().map_err(|e| format!("{s}: {e}"));
    let mut text = String::from(HEADER);
    text.push('\n');
    let mut n = 0;
    for l in qlines {
        let q: Vec<&str> = l.split(',').collect();
        let f = ctx
            .get(&(q[0].to_string(), q[1].to_string(), q[2].to_string()))
            .ok_or_else(|| format!("no features for {}", &l[..l.len().min(40)]))?;
        let b0 = num(f[cb])?;
        let truth = num(f[ct])?;
        let values: Vec<f64> = q[3..]
            .iter()
            .map(|s| num(s).map(|v| b0 + v))
            .collect::<Result<_, _>>()?;
        let d = Dist::quantiles(levels.clone(), values);
        let row = Row {
            account: q[0].parse().map_err(|e| format!("{e}"))?,
            origin: q[1].to_string(),
            h: q[2].parse().map_err(|e| format!("{e}"))?,
            method: method.to_string(),
            k: num(f[ck])?,
            truth,
            score: d.score(truth),
            income_detected: f[ci] == "1",
            months_history: f[cm].parse().map_err(|e| format!("{e}"))?,
            alarm_recent: f[cal] == "1",
        };
        text.push_str(&row.csv());
        text.push('\n');
        n += 1;
    }
    write(out, &text)?;
    eprintln!("scored {n} {method} forecasts into {}", out.display());
    Ok(())
}

/// V0–V4 on the development accounts (the same exclusion as `berka`).
fn variants(data: &Path, out: &Path, limit: Option<usize>, threads: usize) -> Result<(), String> {
    use ephemeris_eval::variants::run_all as run_variants;
    let accounts = parse_trans(&read(&data.join("trans.tsv"))?)?;
    let mut chosen: Vec<&Account> = accounts
        .values()
        .filter(|a| split_of(a.id) == Split::Dev)
        .filter(|a| a.stats().max_drift <= MAX_DRIFT_CZK)
        .collect();
    if let Some(n) = limit {
        chosen.truncate(n);
    }
    eprintln!("variants V0–V4 on {} development accounts", chosen.len());
    let t0 = std::time::Instant::now();
    let (results, priors) = run_variants(&chosen, threads);
    let secs = t0.elapsed().as_secs_f64();
    let named: Vec<(String, Priors)> = priors
        .iter()
        .enumerate()
        .map(|(k, p)| (format!("fold{k}_priors"), *p))
        .collect();
    let named: Vec<(&str, Priors)> = named.iter().map(|(n, p)| (n.as_str(), *p)).collect();
    write_variants(out, &results, chosen.len(), secs, &named)
}

/// Write the variants' rows and summary (development and held-out alike).
fn write_variants(
    out: &Path,
    results: &[VariantResult],
    accounts: usize,
    secs: f64,
    priors: &[(&str, Priors)],
) -> Result<(), String> {
    let mut rows = String::from(HEADER);
    rows.push('\n');
    let (mut origins, mut sparse) = (0, 0);
    for r in results {
        origins += r.origins;
        sparse += r.sparse_origins;
        for row in &r.rows {
            rows.push_str(&row.csv());
            rows.push('\n');
        }
    }
    let mut summary = format!(
        "key,value\naccounts,{accounts}\norigins,{origins}\norigins_sparse_v2,{sparse}\nwall_seconds,{secs:.1}\n",
    );
    for (name, p) in priors {
        writeln!(
            summary,
            "{name},c {:.4} sd {:.4} | m {:.4} sd {:.4} | spend_var {:.4} | income_var {:.4}",
            p.c_mean, p.c_sd, p.m_mean, p.m_sd, p.spend_var, p.income_var
        )
        .unwrap();
    }
    write(&out.join("rows.csv"), &rows)?;
    write(&out.join("data.csv"), &summary)?;
    eprintln!("{summary}wrote {}", out.display());
    Ok(())
}

fn variant_tables(paths: &[PathBuf], out: &Path) -> Result<(), String> {
    use ephemeris_eval::aggregate::selection_table;
    use ephemeris_eval::variants::VARIANTS;
    let mut rows = Vec::new();
    for p in paths {
        rows.extend(read_rows(&read(p)?)?);
    }
    let shown: Vec<&str> = ["B-SA+"]
        .into_iter()
        .chain(VARIANTS)
        .chain(["B-EQ", "B-GLOBAL"])
        .collect();
    let keep = |r: &Row| shown.contains(&r.method.as_str());
    write(
        &out.join("variants.csv"),
        &main_table(&rows, "B-SA+", &shown, keep),
    )?;
    // B-EQ next to V2 in every stratum (PLAN §10, added output).
    let strata_methods: Vec<&str> = VARIANTS
        .iter()
        .copied()
        .chain(["B-EQ", "B-GLOBAL"])
        .collect();
    write(
        &out.join("variants-strata.csv"),
        &strata_table(&rows, "B-SA+", &strata_methods),
    )?;
    let pit_rows: Vec<Row> = rows.iter().filter(|r| keep(r)).cloned().collect();
    write(&out.join("variants-pit.csv"), &pit_table(&pit_rows, &shown))?;
    let (sel, chosen, any) = selection_table(&rows, &VARIANTS[1..], &["V0", "B-EQ", "B-GLOBAL"]);
    write(&out.join("selection.csv"), &sel)?;
    eprintln!(
        "selected {chosen}{}",
        if any {
            " (qualifies: 90% coverage within ±5 points at every horizon)"
        } else {
            " (NO variant qualified; chosen as closest to nominal 90% coverage)"
        }
    );
    Ok(())
}

fn tables(paths: &[PathBuf], out: &Path) -> Result<(), String> {
    let mut rows = Vec::new();
    for p in paths {
        rows.extend(read_rows(&read(p)?)?);
    }
    let mut order: Vec<&str> = vec!["B-SA+"];
    order.extend(METHODS.iter().filter(|&&m| m != "B-SA+"));
    order.push("B-GLOBAL");
    let is_ablation = |r: &Row| r.method.starts_with("abl:");
    write(
        &out.join("main.csv"),
        &main_table(&rows, "B-SA+", &order, |r| !is_ablation(r)),
    )?;
    // Ablations: the accounts that have them, against the full model.
    let abl_accounts: std::collections::HashSet<u64> = rows
        .iter()
        .filter(|r| is_ablation(r))
        .map(|r| r.account)
        .collect();
    let mut abl_order = vec!["EPHEMERIS"];
    abl_order.extend(ABLATIONS);
    write(
        &out.join("ablations.csv"),
        &main_table(&rows, "EPHEMERIS", &abl_order, |r| {
            abl_accounts.contains(&r.account) && (is_ablation(r) || r.method == "EPHEMERIS")
        }),
    )?;
    write(
        &out.join("strata.csv"),
        &strata_table(&rows, "B-SA+", &["EPHEMERIS", "B-SA+", "B-EQ", "B-GLOBAL"]),
    )?;
    let all_order: Vec<&str> = order.iter().chain(&abl_order).copied().collect();
    write(&out.join("pit.csv"), &pit_table(&rows, &all_order))?;
    eprintln!("{} rows → {}", rows.len(), out.display());
    Ok(())
}
