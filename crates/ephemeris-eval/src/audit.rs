//! Prefix equivalence (review response, Tier 1 A4): a forecast made from a
//! feed physically truncated at the origin equals the one the harness made
//! from the full file. Development split only.
//!
//! For each sampled (account, origin), the account's lines of `trans.tsv`
//! dated on or before the origin are cut out of the text and parsed on their
//! own. Every per-account method is recomputed from that truncated account
//! with the harness's own functions: V0 and V2 (`variants::forecast_at`),
//! V1 (its calibration stream, from the truncated account's grid), and the
//! baselines (as `berka_run::run_account` computes them). Two checks:
//! - the truncated forecast equals the full-file forecast, value for value;
//! - scored against the realized balance (from the full file), its row is
//!   byte-identical to the frozen development row.

use std::collections::{HashMap, HashSet};

use ephemeris_core::Day;
use ephemeris_core::schedule::detect_schedule_in;
use ephemeris_core::schema::Date;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;

use crate::baselines::{
    LOOKBACK, avg30, empirical_changes, schedule_aware_delta, schedule_aware_plus, seasonal,
    straight_line,
};
use crate::berka::{Account, CALENDAR, MAX_DRIFT_CZK, parse_trans, tag, transactions};
use crate::berka_run::{HORIZONS, SA_GRID_START, SA_GRID_STEP};
use crate::dist::Dist;
use crate::rows::Row;
use crate::split::{Split, split_of};
use crate::variants::{
    ACI_GRID_START, ACI_GRID_STEP, Forecast, Model, V0, aci_levels, eligible_origins, forecast_at,
    recalibrate,
};

const V2: Model = Model {
    priors: None,
    sparse: true,
};

/// The account's lines of `trans.tsv` dated on or before `origin`, parsed
/// on their own (the header kept).
fn truncated(tsv: &str, account: u64, origin: Day) -> Result<Account, String> {
    let mut lines = tsv.lines();
    let header = lines.next().ok_or("empty trans.tsv")?;
    let cols: Vec<&str> = header.split('\t').collect();
    let ca = cols
        .iter()
        .position(|&c| c == "account_id")
        .ok_or("no account_id")?;
    let cd = cols.iter().position(|&c| c == "date").ok_or("no date")?;
    let cutoff = Date(origin).to_string();
    let id = account.to_string();
    let mut text = String::from(header);
    text.push('\n');
    for l in lines {
        let f: Vec<&str> = l.split('\t').collect();
        if f[ca] == id && f[cd] <= cutoff.as_str() {
            text.push_str(l);
            text.push('\n');
        }
    }
    parse_trans(&text)?
        .remove(&account)
        .ok_or_else(|| format!("account {account}: nothing on or before the origin"))
}

/// Every per-account method's distribution at `origin`, per horizon, plus
/// V0's forecast (for k and the strata).
/// V0's forecast, then every method's distributions per horizon.
type Methods = (Forecast, Vec<(String, Vec<Dist>)>);

fn methods(acct: &Account, origin: Day) -> Option<Methods> {
    let first = acct.first_day();
    let bal_raw = acct.daily_balance(origin);
    let f0 = forecast_at(acct, &bal_raw, origin, V0)?;
    let f2 = forecast_at(acct, &bal_raw, origin, V2)?;
    let k = f0.k;
    // V1: V0 from the calibration grid, outcomes known at the origin.
    let mut grid: Vec<(Day, Forecast)> = Vec::new();
    let mut g = first + ACI_GRID_START;
    while g + HORIZONS[0] <= origin {
        if let Some(f) = forecast_at(acct, &bal_raw, g, V0) {
            grid.push((g, f));
        }
        g += ACI_GRID_STEP;
    }
    let truth_at = |day: Day, k: f64| bal_raw[(day - first) as usize] * k;
    let v1: Vec<Dist> = HORIZONS
        .iter()
        .enumerate()
        .map(|(hi, &h)| {
            let s = aci_levels(
                grid.iter()
                    .filter(|(g, _)| g + h <= origin)
                    .map(|(g, f)| (&f.dists[hi], truth_at(g + h, f.k))),
            );
            recalibrate(&f0.dists[hi], &s)
        })
        .collect();
    // The baselines, as berka_run computes them.
    let t = (origin - first) as usize;
    let bal: Vec<f64> = bal_raw.iter().map(|b| b * k).collect();
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let sa_raw = |g: Day| -> [f64; 3] {
        let r = acct.records_through(g);
        let tx = transactions(r, &tag(r), 1.0);
        let s = detect_schedule_in(&tx, CALENDAR);
        HORIZONS.map(|h| schedule_aware_delta(&tx, &s, g, h))
    };
    let mut cache: HashMap<Day, [f64; 3]> = HashMap::new();
    let mut base: HashMap<&str, Vec<Dist>> = HashMap::new();
    for (hi, &h) in HORIZONS.iter().enumerate() {
        let sa_point = bal[t] + schedule_aware_delta(&txns, &schedule, origin, h);
        let mut errors = Vec::new();
        let mut g = first + SA_GRID_START;
        while g + h <= origin {
            if (origin - g) as usize <= LOOKBACK {
                let d = *cache.entry(g).or_insert_with(|| sa_raw(g));
                let gi = (g - first) as usize;
                errors.push((bal_raw[gi + h as usize] - (bal_raw[gi] + d[hi])) * k);
            }
            g += SA_GRID_STEP;
        }
        let mut push = |name, d| base.entry(name).or_default().push(d);
        push("B-30", avg30(&bal, t, h));
        push("B-LIN", straight_line(&bal, t, h));
        push("B-SA", Dist::Point(sa_point));
        push("B-SA+", schedule_aware_plus(sa_point, &errors));
        push("B-EQ", empirical_changes(&bal, t, h));
        if let Some(d) = seasonal(&bal, first, t, h) {
            push("B-SEAS", d);
        }
    }
    let mut out = vec![
        ("V0".to_string(), f0.dists.clone()),
        ("V1".to_string(), v1),
        ("V2".to_string(), f2.dists),
    ];
    for name in ["B-30", "B-LIN", "B-SA", "B-SA+", "B-EQ", "B-SEAS"] {
        if let Some(d) = base.remove(name) {
            out.push((name.to_string(), d));
        }
    }
    Some((f0, out))
}

/// The check; `frozen` holds the frozen rows' text by (account, origin, h,
/// method). Returns the per-forecast CSV and a summary line.
pub fn prefix_check(
    tsv: &str,
    frozen_lines: &mut dyn Iterator<Item = String>,
    sample: usize,
    seed: u64,
) -> Result<(String, String), String> {
    let accounts = parse_trans(tsv)?;
    let dev: Vec<&Account> = accounts
        .values()
        .filter(|a| split_of(a.id) == Split::Dev)
        .filter(|a| a.stats().max_drift <= MAX_DRIFT_CZK)
        .collect();
    let mut cands: Vec<(u64, Day)> = dev
        .iter()
        .flat_map(|a| eligible_origins(a).into_iter().map(move |o| (a.id, o)))
        .collect();
    cands.sort_unstable();
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    cands.shuffle(&mut rng);
    cands.truncate(sample);
    cands.sort_unstable();
    let wanted: HashSet<(String, String)> = cands
        .iter()
        .map(|&(a, o)| (a.to_string(), Date(o).to_string()))
        .collect();
    let mut frozen: HashMap<(String, String, String, String), String> = HashMap::new();
    for l in frozen_lines {
        let f: Vec<&str> = l.splitn(5, ',').collect();
        if f.len() == 5 && wanted.contains(&(f[0].to_string(), f[1].to_string())) {
            frozen.insert(
                (f[0].into(), f[1].into(), f[2].into(), f[3].into()),
                l.clone(),
            );
        }
    }
    let mut csv = String::from("account,origin,h,method,truncated_equals_full,row_equals_frozen\n");
    let (mut n, mut same, mut rows_same, mut missing) = (0, 0, 0, 0);
    for &(id, o) in &cands {
        let full = accounts[&id].clone();
        let trunc = truncated(tsv, id, o)?;
        let (Some((f0, a)), Some((_, b))) = (methods(&full, o), methods(&trunc, o)) else {
            return Err(format!("account {id} at {}: no forecast", Date(o)));
        };
        let bal_full = full.daily_balance(o + HORIZONS[2]);
        let first = full.first_day();
        for ((name, da), (name_b, db)) in a.iter().zip(&b) {
            assert_eq!(name, name_b);
            for (hi, &h) in HORIZONS.iter().enumerate() {
                n += 1;
                let eq = da[hi] == db[hi];
                same += usize::from(eq);
                let truth = bal_full[(o + h - first) as usize] * f0.k;
                let row = Row {
                    account: id,
                    origin: Date(o).to_string(),
                    h,
                    method: name.clone(),
                    k: f0.k,
                    truth,
                    score: db[hi].score(truth),
                    income_detected: f0.income_detected,
                    months_history: (o - first) / 30,
                    alarm_recent: f0.alarm_recent,
                }
                .csv();
                let key = (
                    id.to_string(),
                    Date(o).to_string(),
                    h.to_string(),
                    name.clone(),
                );
                let row_eq = match frozen.get(&key) {
                    Some(l) => *l == row,
                    None => {
                        missing += 1;
                        false
                    }
                };
                rows_same += usize::from(row_eq);
                csv.push_str(&format!(
                    "{id},{},{h},{name},{},{}\n",
                    Date(o),
                    u8::from(eq),
                    u8::from(row_eq)
                ));
            }
        }
    }
    let summary = format!(
        "origins,{}\naccounts,{}\nforecasts,{n}\ntruncated_equals_full,{same}\nrow_equals_frozen,{rows_same}\nfrozen_row_missing,{missing}\nseed,{seed}\n",
        cands.len(),
        cands.iter().map(|c| c.0).collect::<HashSet<_>>().len()
    );
    Ok((csv, summary))
}
