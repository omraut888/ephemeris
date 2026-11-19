//! Data behind the paper's figures that the committed tables do not hold:
//! coverage at every central level, and one example forecast cone.
//!
//! - **Coverage curve.** For a level ℓ, a forecast covers the truth iff
//!   `|PIT − ½| ≤ ℓ/2`: the PIT inverts the forecast's quantile function,
//!   so this is the central interval's coverage. At 50/80/90/95% the table
//!   also has the exact coverage from the stored interval flags, as a
//!   check. Ties in a step distribution can make the two differ a little.
//! - **Example cone.** One **development** account, chosen by a fixed rule
//!   before looking at any cone: among development accounts (drift
//!   exclusion applied) with income detected at their last eligible
//!   origin, the one with the median number of records (ties: lower id).
//!   Its V0 and V2 forecasts are recomputed exactly as the backtest made
//!   them (same configuration, same seed), for every day of the 90.

use ephemeris_core::Day;
use ephemeris_core::forecast::{ForecastConfig, ForecastModel, forecast};
use ephemeris_core::schedule::detect_schedule_in;
use ephemeris_core::schema::Date;
use ephemeris_core::tracker::{Tracker, TrackerConfig};

use crate::berka::{Account, CALENDAR, tag, transactions};
use crate::berka_run::{HORIZONS, PATHS, seed_for};
use crate::dist::{INTERVALS, mc_levels};
use crate::reference::REFERENCE_SCALE;
use crate::rows::Row;
use crate::variants::{eligible_origins, sparse_spending};

/// The levels of the coverage curve.
pub fn curve_levels() -> Vec<f64> {
    (1..=19)
        .map(|i| f64::from(i) * 0.05)
        .chain([0.98])
        .collect()
}

/// `method,h,level,forecasts,coverage_pit,coverage_exact`.
pub fn coverage_curve(rows: &[Row], methods: &[&str]) -> String {
    let mut out = String::from("method,h,level,forecasts,coverage_pit,coverage_exact\n");
    for &m in methods {
        for &h in &HORIZONS {
            let cell: Vec<&Row> = rows.iter().filter(|r| r.method == m && r.h == h).collect();
            if cell.is_empty() {
                continue;
            }
            let n = cell.len() as f64;
            for level in curve_levels() {
                let pit = cell
                    .iter()
                    .filter(|r| (r.score.pit - 0.5).abs() <= level / 2.0 + 1e-12)
                    .count() as f64
                    / n;
                let exact = INTERVALS
                    .iter()
                    .position(|&c| (c - level).abs() < 1e-9)
                    .map(|i| cell.iter().filter(|r| r.score.inside[i]).count() as f64 / n);
                out.push_str(&format!(
                    "{m},{h},{level:.2},{n},{pit:.4},{}\n",
                    exact.map_or(String::new(), |e| format!("{e:.4}"))
                ));
            }
        }
    }
    out
}

/// The example account and its origin (module docs).
pub fn example_account<'a>(dev: &[&'a Account]) -> Option<(&'a Account, Day)> {
    let mut cands: Vec<(usize, u64, &Account, Day)> = dev
        .iter()
        .filter_map(|a| {
            let o = *eligible_origins(a).last()?;
            let k = REFERENCE_SCALE / a.scale_at(o)?;
            let recs = a.records_through(o);
            let txns = transactions(recs, &tag(recs), k);
            detect_schedule_in(&txns, CALENDAR).income.as_ref()?;
            Some((a.records.len(), a.id, *a, o))
        })
        .collect();
    cands.sort_by_key(|c| (c.0, c.1));
    let mid = cands.get((cands.len().saturating_sub(1)) / 2)?;
    Some((mid.2, mid.3))
}

/// `model,day,date,q05,q25,q50,q75,q95,balance` in CZK for every day from
/// 180 days before `origin` to 90 after; quantiles only after the origin.
pub fn cone_csv(acct: &Account, origin: Day) -> Option<String> {
    let first = acct.first_day();
    let end = origin + HORIZONS[2];
    let bal_raw = acct.daily_balance(end);
    let k = REFERENCE_SCALE / acct.scale_at(origin)?;
    let t = (origin - first) as usize;
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let mut cfg = TrackerConfig::new(first);
    cfg.observe.holidays = CALENDAR;
    let mut tracker = Tracker::new(cfg).ok()?;
    tracker.ingest(&txns, bal_raw[t] * k, origin).ok()?;
    let est = tracker.estimate()?.clone();
    let levels = mc_levels();
    let idx = |p: f64| levels.iter().position(|&l| (l - p).abs() < 1e-12);
    let picks = [0.05, 0.25, 0.5, 0.75, 0.95].map(|p| idx(p).expect("level in the grid"));
    let mut out = String::from("model,day,date,q05,q25,q50,q75,q95,balance\n");
    let start = origin.max(first + 180) - 180;
    for d in start..=end {
        let b = bal_raw[(d - first) as usize];
        out.push_str(&format!("history,{},{},,,,,,{b:.2}\n", d - origin, Date(d)));
    }
    for (model, sparse) in [("V0", None), ("V2", sparse_spending(&txns, first, origin))] {
        let fcfg = ForecastConfig {
            n_paths: PATHS,
            levels: levels.clone(),
            sparse_spending: sparse,
            ..ForecastConfig::new(HORIZONS[2] as u32, seed_for(acct.id, origin))
        };
        let cone = forecast(&est, &schedule, &ForecastModel::from_tracker(&cfg), &fcfg).ok()?;
        for d in origin + 1..=end {
            let band = cone.band(d)?;
            let q: Vec<String> = picks
                .iter()
                .map(|&i| format!("{:.2}", band[i] / k))
                .collect();
            out.push_str(&format!(
                "{model},{},{},{},{:.2}\n",
                d - origin,
                Date(d),
                q.join(","),
                bal_raw[(d - first) as usize]
            ));
        }
    }
    Some(out)
}
