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
use ephemeris_eval::berka_run::{ABLATIONS, FEATURE_HEADER, METHODS, run_all};
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
    berka_split(&accounts, Split::Dev, limit, out, threads)
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
    berka_split(&accounts, Split::HeldOut, None, &out.join("berka"), threads)?;

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

fn berka_split(
    accounts: &std::collections::BTreeMap<u64, Account>,
    split: Split,
    limit: Option<usize>,
    out: &Path,
    threads: usize,
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
    let results = run_all(&chosen, threads, true);
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
            u8::from(in_ablation_sample(acct.id)),
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
        chosen.iter().filter(|a| in_ablation_sample(a.id)).count(),
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
