//! Rows → tables, with account-block bootstrap intervals (paper/PLAN.md §2).
//!
//! - **Coverage** is pooled over forecasts.
//! - **CRPS skill** against a reference method is computed per account,
//!   `S_a = 1 − Σ CRPS_m / Σ CRPS_ref` over the account's forecasts that
//!   both methods made, then averaged over accounts (as pre-registered). The
//!   pooled version `1 − Σ_all CRPS_m / Σ_all CRPS_ref` in normalized units
//!   is reported next to it.
//! - **Intervals** resample whole accounts, [`BOOTSTRAP`] times from a fixed
//!   seed, and take the 2.5% and 97.5% points.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::rows::Row;

pub const BOOTSTRAP: usize = 2000;
pub const BOOTSTRAP_SEED: u64 = 6;

/// Per-account sums for one (method, horizon) cell.
#[derive(Debug, Clone, Copy, Default)]
struct Acc {
    n: f64,
    inside: [f64; 4],
    crps: f64,
    /// Over the forecasts the reference also made.
    crps_matched: f64,
    ref_matched: f64,
    crps_czk: f64,
    width_czk: f64,
    abs_err_czk: f64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = (p * (sorted.len() - 1) as f64).round() as usize;
    sorted[i]
}

/// Mean and 95% bootstrap interval of a statistic of per-account values.
fn boot(accs: &[Acc], stat: impl Fn(&[&Acc]) -> f64, rng: &mut ChaCha8Rng) -> (f64, f64, f64) {
    let all: Vec<&Acc> = accs.iter().collect();
    let point = stat(&all);
    let n = accs.len();
    let mut draws: Vec<f64> = (0..BOOTSTRAP)
        .map(|_| {
            let sample: Vec<&Acc> = (0..n).map(|_| &accs[rng.random_range(0..n)]).collect();
            stat(&sample)
        })
        .filter(|x| x.is_finite())
        .collect();
    draws.sort_by(f64::total_cmp);
    if draws.is_empty() {
        return (point, f64::NAN, f64::NAN);
    }
    (point, percentile(&draws, 0.025), percentile(&draws, 0.975))
}

fn coverage(i: usize) -> impl Fn(&[&Acc]) -> f64 {
    move |a| a.iter().map(|x| x.inside[i]).sum::<f64>() / a.iter().map(|x| x.n).sum::<f64>()
}

fn mean_skill(a: &[&Acc]) -> f64 {
    let v: Vec<f64> = a
        .iter()
        .filter(|x| x.ref_matched > 0.0)
        .map(|x| 1.0 - x.crps_matched / x.ref_matched)
        .collect();
    v.iter().sum::<f64>() / v.len() as f64
}

fn pooled_skill(a: &[&Acc]) -> f64 {
    1.0 - a.iter().map(|x| x.crps_matched).sum::<f64>()
        / a.iter().map(|x| x.ref_matched).sum::<f64>()
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// The cells of one table: per (method, h), per-account sums, for the rows
/// that pass `keep`, with skill against `reference`.
fn cells(
    rows: &[Row],
    reference: &str,
    keep: impl Fn(&Row) -> bool,
) -> BTreeMap<(String, i32), BTreeMap<u64, Acc>> {
    let ref_crps: HashMap<(u64, &str, i32), f64> = rows
        .iter()
        .filter(|r| r.method == reference)
        .map(|r| ((r.account, r.origin.as_str(), r.h), r.score.crps))
        .collect();
    let mut out: BTreeMap<(String, i32), BTreeMap<u64, Acc>> = BTreeMap::new();
    for r in rows.iter().filter(|r| keep(r)) {
        let a = out
            .entry((r.method.clone(), r.h))
            .or_default()
            .entry(r.account)
            .or_default();
        a.n += 1.0;
        for (i, &x) in r.score.inside.iter().enumerate() {
            a.inside[i] += f64::from(u8::from(x));
        }
        a.crps += r.score.crps;
        a.crps_czk += r.score.crps / r.k;
        a.width_czk += r.score.width90 / r.k;
        a.abs_err_czk += r.score.abs_err / r.k;
        if let Some(&rc) = ref_crps.get(&(r.account, r.origin.as_str(), r.h)) {
            a.crps_matched += r.score.crps;
            a.ref_matched += rc;
        }
    }
    out
}

fn fmt(x: f64) -> String {
    if x.is_finite() {
        format!("{x:.4}")
    } else {
        String::new()
    }
}

/// The method order of a table: `order` first, then the rest by name.
fn ordered<'a>(keys: impl Iterator<Item = &'a String>, order: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = keys.cloned().collect();
    v.sort();
    v.dedup();
    v.sort_by_key(|m| order.iter().position(|o| o == m).unwrap_or(order.len()));
    v
}

/// Coverage, skill and size for every (method, h): the main table.
pub fn main_table(
    rows: &[Row],
    reference: &str,
    order: &[&str],
    keep: impl Fn(&Row) -> bool,
) -> String {
    let cells = cells(rows, reference, keep);
    let mut rng = ChaCha8Rng::seed_from_u64(BOOTSTRAP_SEED);
    let mut out = String::from(
        "method,h,forecasts,accounts,cov50,cov50_lo,cov50_hi,cov80,cov80_lo,cov80_hi,cov90,cov90_lo,cov90_hi,cov95,cov95_lo,cov95_hi,skill,skill_lo,skill_hi,pooled_skill,pooled_skill_lo,pooled_skill_hi,median_crps_czk,median_width90_czk,median_mae_czk\n",
    );
    let methods = ordered(cells.keys().map(|(m, _)| m), order);
    for m in &methods {
        for h in [7, 30, 90] {
            let Some(c) = cells.get(&(m.clone(), h)) else {
                continue;
            };
            let accs: Vec<Acc> = c.values().copied().collect();
            let n: f64 = accs.iter().map(|a| a.n).sum();
            write!(out, "{m},{h},{n},{}", accs.len()).unwrap();
            for i in 0..4 {
                let (p, lo, hi) = boot(&accs, coverage(i), &mut rng);
                write!(out, ",{},{},{}", fmt(p), fmt(lo), fmt(hi)).unwrap();
            }
            for stat in [mean_skill as fn(&[&Acc]) -> f64, pooled_skill] {
                let (p, lo, hi) = boot(&accs, stat, &mut rng);
                write!(out, ",{},{},{}", fmt(p), fmt(lo), fmt(hi)).unwrap();
            }
            let per = |f: fn(&Acc) -> f64| median(accs.iter().map(|a| f(a) / a.n).collect());
            writeln!(
                out,
                ",{},{},{}",
                fmt(per(|a| a.crps_czk)),
                fmt(per(|a| a.width_czk)),
                fmt(per(|a| a.abs_err_czk))
            )
            .unwrap();
        }
    }
    out
}

/// Coverage at 50% and 90% and skill against `reference`, per stratum.
pub fn strata_table(rows: &[Row], reference: &str, methods: &[&str]) -> String {
    type Pick = fn(&Row) -> bool;
    let strata: [(&str, Pick); 8] = [
        ("all", |_| true),
        ("income detected", |r| r.income_detected),
        ("no income detected", |r| !r.income_detected),
        ("history < 12 months", |r| r.months_history < 12),
        ("history 12-23 months", |r| {
            (12..24).contains(&r.months_history)
        }),
        ("history >= 24 months", |r| r.months_history >= 24),
        ("regime alarm in last 28 days", |r| r.alarm_recent),
        ("no recent alarm", |r| !r.alarm_recent),
    ];
    let mut rng = ChaCha8Rng::seed_from_u64(BOOTSTRAP_SEED);
    let mut out = String::from(
        "stratum,method,h,forecasts,accounts,cov50,cov90,cov90_lo,cov90_hi,skill,skill_lo,skill_hi\n",
    );
    for (name, pick) in strata {
        let c = cells(rows, reference, |r| {
            methods.contains(&r.method.as_str()) && pick(r)
        });
        for m in methods {
            for h in [7, 30, 90] {
                let Some(c) = c.get(&(m.to_string(), h)) else {
                    continue;
                };
                let accs: Vec<Acc> = c.values().copied().collect();
                let n: f64 = accs.iter().map(|a| a.n).sum();
                let cov50 = coverage(0)(&accs.iter().collect::<Vec<_>>());
                let (p90, lo90, hi90) = boot(&accs, coverage(2), &mut rng);
                let (s, slo, shi) = boot(&accs, mean_skill, &mut rng);
                writeln!(
                    out,
                    "{name},{m},{h},{n},{},{},{},{},{},{},{},{}",
                    accs.len(),
                    fmt(cov50),
                    fmt(p90),
                    fmt(lo90),
                    fmt(hi90),
                    fmt(s),
                    fmt(slo),
                    fmt(shi)
                )
                .unwrap();
            }
        }
    }
    out
}

/// PIT histogram counts in 10 bins per (method, h).
pub fn pit_table(rows: &[Row], order: &[&str]) -> String {
    let mut counts: BTreeMap<(String, i32), [u64; 10]> = BTreeMap::new();
    for r in rows {
        let bin = ((r.score.pit * 10.0).floor() as usize).min(9);
        counts.entry((r.method.clone(), r.h)).or_default()[bin] += 1;
    }
    let mut out = String::from("method,h,b0,b1,b2,b3,b4,b5,b6,b7,b8,b9\n");
    for m in ordered(counts.keys().map(|(m, _)| m), order) {
        for h in [7, 30, 90] {
            if let Some(c) = counts.get(&(m.clone(), h)) {
                let cells: Vec<String> = c.iter().map(u64::to_string).collect();
                writeln!(out, "{m},{h},{}", cells.join(",")).unwrap();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::Score;

    fn row(account: u64, method: &str, crps: f64, inside: bool) -> Row {
        Row {
            account,
            origin: "1996-01-01".into(),
            h: 7,
            method: method.into(),
            k: 2.0,
            truth: 0.0,
            score: Score {
                crps,
                inside: [inside; 4],
                width90: 4.0,
                median: 0.0,
                abs_err: 1.0,
                pit: 0.95,
            },
            income_detected: true,
            months_history: 12,
            alarm_recent: false,
        }
    }

    #[test]
    fn skill_is_per_account_then_averaged() {
        // Account 1: m 1 vs ref 2 → 0.5. Account 2: m 3 vs ref 1 → −2.
        let rows = vec![
            row(1, "m", 1.0, true),
            row(1, "ref", 2.0, true),
            row(2, "m", 3.0, false),
            row(2, "ref", 1.0, true),
        ];
        let t = main_table(&rows, "ref", &["ref", "m"], |_| true);
        let line = t.lines().find(|l| l.starts_with("m,7,")).unwrap();
        let f: Vec<&str> = line.split(',').collect();
        assert_eq!(f[2], "2"); // forecasts
        assert_eq!(f[10], "0.5000"); // cov90: one of two
        assert_eq!(f[16], "-0.7500"); // mean of 0.5 and −2
        assert_eq!(f[19], "-0.3333"); // pooled: 1 − 4/3
        assert_eq!(f[22], "1.0000"); // median CRPS in CZK: (0.5, 1.5)
        assert_eq!(f[23], "2.0000"); // width 4 / k 2
        // The reference is listed first and has skill 0.
        assert!(t.lines().nth(1).unwrap().starts_with("ref,7,2,2,"));
    }

    #[test]
    fn pit_bins() {
        let rows = vec![row(1, "m", 1.0, true), row(2, "m", 1.0, true)];
        let t = pit_table(&rows, &[]);
        assert_eq!(t.lines().nth(1).unwrap(), "m,7,0,0,0,0,0,0,0,0,0,2");
    }
}
