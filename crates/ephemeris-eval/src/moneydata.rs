//! The MoneyData case study (paper/PLAN.md §2): **one** UK current account,
//! 2015-07-27 to 2022-07-25, in GBP. It is descriptive only: n = 1, so it
//! supports no claim about users in general.
//!
//! Adapter decisions, made from the data's format before any forecast was
//! scored:
//! - **Dates.** The file mixes `dd/mm/yyyy` with ISO dates. The ISO ones
//!   are Excel's misreading of ambiguous `dd/mm` dates as `mm/dd`: every
//!   one has a "day" of at most 12, and only with day and month swapped do
//!   the dates increase with the record numbers. So an ISO date is read
//!   with its month and day swapped. [`parse`] checks the resulting order.
//! - **Order and balance.** Records are numbered newest first. The balance
//!   after each record chains exactly ([`parse`] checks it).
//! - **Tagging: the app's own.** At each origin, the records on or before
//!   it go through `ephemeris_sync::tag::to_feed`, the shipped tagger. The
//!   dataset's per-record categories map to the tagger's bill and wage
//!   hints ([`category`]). Nothing after the origin is seen.
//! - **Calendar: `us`, the default.** Bookings fall on weekdays only, so
//!   the every-day calendar used for Berka does not fit. The engine has no
//!   UK holiday calendar, so pay dates shift by US holidays: a stated
//!   mismatch.
//! - **Units.** The same per-origin normalization as Berka (§8 decision 2).
//!
//! Protocol: weekly origins from day 180, at least 12 records in the 180
//! days before, horizons 7/30/90. Methods: V0, V1 and V2, and the
//! per-account baselines. B-GLOBAL and V3/V4 need other accounts, so they
//! are not run here.

use std::collections::{BTreeMap, HashMap};

use ephemeris_core::calendar::{HolidayCalendar, day_from_civil};
use ephemeris_core::forecast::{ForecastConfig, ForecastModel, forecast};
use ephemeris_core::schedule::{Schedule, detect_schedule_in};
use ephemeris_core::schema::Date;
use ephemeris_core::tracker::{StateEstimate, Tracker, TrackerConfig};
use ephemeris_core::{Day, Transaction};
use ephemeris_sync::bank::{AccountSnapshot, BankRecord, Category};
use ephemeris_sync::tag::to_feed;

use crate::baselines::{
    LOOKBACK, avg30, empirical_changes, schedule_aware_delta, schedule_aware_plus, seasonal,
    straight_line,
};
use crate::berka::{Account, Record};
use crate::berka_run::{
    HORIZONS, MIN_RECORDS_180, PATHS, SA_GRID_START, SA_GRID_STEP, WARM_UP, seed_for,
};
use crate::dist::{Dist, mc_levels};
use crate::reference::REFERENCE_SCALE;
use crate::rows::Row;
use crate::variants::{ACI_GRID_START, ACI_GRID_STEP, aci_levels, recalibrate, sparse_spending};

pub const ACCOUNT: u64 = 1;
pub const CALENDAR: HolidayCalendar = HolidayCalendar::Us;
pub const ORIGIN_STEP: Day = 7;
pub const METHODS: [&str; 9] = [
    "V0", "V1", "V2", "B-30", "B-LIN", "B-SA", "B-SA+", "B-SEAS", "B-EQ",
];

fn parse_date(s: &str) -> Result<Day, String> {
    let n = |x: &str| x.parse::<u32>().map_err(|e| format!("{s:?}: {e}"));
    if let [d, m, y] = s.split('/').collect::<Vec<_>>()[..] {
        return Ok(day_from_civil(n(y)? as i32, n(m)?, n(d)?));
    }
    if let [y, a, b] = s.split('-').collect::<Vec<_>>()[..] {
        // Excel's mm/dd misreading: the "month" is the day.
        let (day, month) = (n(a)?, n(b)?);
        // Both fields came from Excel's month field, so both are ≤ 12.
        if day > 12 || month > 12 {
            return Err(format!("{s:?}: not a swapped dd/mm date"));
        }
        return Ok(day_from_civil(n(y)? as i32, month, day));
    }
    Err(format!("bad date {s:?}"))
}

fn money(s: &str) -> Result<f64, String> {
    if s == "\\N" || s.is_empty() {
        Ok(0.0)
    } else {
        s.parse().map_err(|e| format!("{s:?}: {e}"))
    }
}

/// One row: (number, day, signed amount, balance after, type, description,
/// category).
type Raw = (u64, Day, f64, f64, String, String, String);

/// The account, in chronological order, with the checks above.
pub fn parse(tsv: &str) -> Result<Account, String> {
    let mut lines = tsv.lines();
    let header: Vec<&str> = lines.next().ok_or("empty file")?.split('\t').collect();
    let col = |name: &str| {
        header
            .iter()
            .position(|&c| c == name)
            .ok_or(format!("no column {name}"))
    };
    let (cn, cd, ct, cdesc, cdeb, ccred, cbal, ccat) = (
        col("Transaction Number")?,
        col("Transaction Date")?,
        col("Transaction Type")?,
        col("Transaction Description")?,
        col("Debit Amount")?,
        col("Credit Amount")?,
        col("Balance")?,
        col("Category")?,
    );
    let mut raw: Vec<Raw> = Vec::new();
    for l in lines.filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = l.split('\t').collect();
        raw.push((
            f[cn].parse().map_err(|e| format!("number: {e}"))?,
            parse_date(f[cd])?,
            money(f[ccred])? - money(f[cdeb])?,
            money(f[cbal])?,
            f[ct].to_string(),
            f[cdesc].trim().to_string(),
            f[ccat].trim().to_string(),
        ));
    }
    // Newest first in the file: oldest is the highest number.
    raw.sort_by_key(|r| std::cmp::Reverse(r.0));
    if raw.windows(2).any(|w| w[1].1 < w[0].1) {
        return Err("dates do not increase with the record order".into());
    }
    if let Some(w) = raw
        .windows(2)
        .find(|w| (w[0].3 + w[1].2 - w[1].3).abs() > 0.005)
    {
        return Err(format!("balance does not chain at record {}", w[1].0));
    }
    let opening = raw[0].3 - raw[0].2;
    let records = raw
        .into_iter()
        .enumerate()
        .map(|(i, (_, day, amount, balance, typ, desc, cat))| Record {
            trans_id: i as u64,
            day,
            amount,
            balance,
            operation: typ,
            k_symbol: cat,
            bank: String::new(),
            counter_account: desc,
        })
        .collect();
    Ok(Account {
        id: ACCOUNT,
        records,
        opening,
    })
}

/// The dataset's category as the tagger's hints: bills and wages only.
pub fn category(c: &str) -> Option<Category> {
    let (primary, detailed) = match c {
        "Paycheck" => ("INCOME", "INCOME_WAGES"),
        "Bills" => ("RENT_AND_UTILITIES", "RENT_AND_UTILITIES_OTHER_UTILITIES"),
        "Mortgage" => ("LOAN_PAYMENTS", "LOAN_PAYMENTS_MORTGAGE_PAYMENT"),
        "Insurance" => ("GENERAL_SERVICES", "GENERAL_SERVICES_INSURANCE"),
        _ => return None,
    };
    Some(Category {
        primary: primary.into(),
        detailed: detailed.into(),
    })
}

/// The engine's transactions at `day`, tagged by the app from the records
/// on or before it, scaled by `k`.
pub fn tagged(acct: &Account, day: Day, k: f64) -> Option<Vec<Transaction>> {
    let recs = acct.records_through(day);
    let last = recs.last()?;
    let bank: Vec<BankRecord> = recs
        .iter()
        .map(|r| BankRecord {
            id: format!("{:06}", r.trans_id),
            account: "current".into(),
            date: Date(r.day),
            authorized_date: Some(Date(r.day)),
            pending: false,
            pending_id: None,
            amount: r.amount,
            currency: Some("GBP".into()),
            merchant_name: None,
            name: Some(r.counter_account.clone()),
            category: category(&r.k_symbol),
        })
        .collect();
    let accounts = [AccountSnapshot {
        id: "current".into(),
        depository: true,
        current: Some(last.balance),
        currency: Some("GBP".into()),
    }];
    let (feed, _) = to_feed(&bank, &accounts, Date(day)).ok()?;
    let mut txns = feed.transactions().ok()?;
    for t in &mut txns {
        t.amount *= k;
    }
    Some(txns)
}

/// V0's (or V2's) forecast from `day`, with the strata.
struct Prep {
    dists: Vec<Dist>,
    sparse: Option<Vec<Dist>>,
    txns: Vec<Transaction>,
    schedule: Schedule,
    income_detected: bool,
    alarm_recent: bool,
}

fn prepare(acct: &Account, bal_raw: &[f64], day: Day, k: f64, with_sparse: bool) -> Option<Prep> {
    let first = acct.first_day();
    let txns = tagged(acct, day, k)?;
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let cfg = TrackerConfig::new(first);
    let mut tracker = Tracker::new(cfg).ok()?;
    let ingest = tracker
        .ingest(&txns, bal_raw[(day - first) as usize] * k, day)
        .ok()?;
    let est: StateEstimate = tracker.estimate()?.clone();
    let model = ForecastModel::from_tracker(&cfg);
    let levels = mc_levels();
    let run = |sparse| -> Option<Vec<Dist>> {
        let fcfg = ForecastConfig {
            n_paths: PATHS,
            levels: levels.clone(),
            sparse_spending: sparse,
            ..ForecastConfig::new(HORIZONS[2] as u32, seed_for(ACCOUNT, day))
        };
        let cone = forecast(&est, &schedule, &model, &fcfg).ok()?;
        HORIZONS
            .iter()
            .map(|&h| {
                Some(Dist::quantiles(
                    levels.clone(),
                    cone.band(day + h)?.to_vec(),
                ))
            })
            .collect()
    };
    let dists = run(None)?;
    let sparse = if with_sparse {
        match sparse_spending(&txns, first, day) {
            Some(sp) => Some(run(Some(sp))?),
            None => None,
        }
    } else {
        None
    };
    Some(Prep {
        dists,
        sparse,
        income_detected: schedule.income.is_some(),
        alarm_recent: ingest
            .days
            .iter()
            .any(|d| d.day > day - 28 && !d.jumps.is_empty()),
        txns,
        schedule,
    })
}

pub fn origins(acct: &Account) -> Vec<Day> {
    let first = acct.first_day();
    let mut out = Vec::new();
    let mut o = first + WARM_UP;
    while o + HORIZONS[2] <= acct.last_day() {
        if acct.count_in_window(o, WARM_UP) >= MIN_RECORDS_180
            && acct.scale_at(o).is_some_and(|s| s > 0.0)
        {
            out.push(o);
        }
        o += ORIGIN_STEP;
    }
    out
}

/// Everything the case study scores, plus how often V2's rule chose sparse
/// spending.
pub struct CaseStudy {
    pub rows: Vec<Row>,
    pub origins: usize,
    pub sparse_origins: usize,
    pub income_origins: usize,
}

pub fn run(acct: &Account) -> CaseStudy {
    let first = acct.first_day();
    let end = acct.last_day();
    let bal_raw = acct.daily_balance(end);
    let os = origins(acct);
    let last_origin = *os.last().expect("eligible origins");
    // V1's calibration stream: V0 from every grid day, in raw units with
    // each grid day's own k (so truth and forecast share units).
    let mut grid: Vec<(Day, f64, Vec<Dist>)> = Vec::new();
    let mut g = first + ACI_GRID_START;
    while g + HORIZONS[0] <= last_origin {
        if let Some(s) = acct.scale_at(g).filter(|&s| s > 0.0) {
            let k = REFERENCE_SCALE / s;
            if let Some(p) = prepare(acct, &bal_raw, g, k, false) {
                grid.push((g, k, p.dists));
            }
        }
        g += ACI_GRID_STEP;
        if (g - first) % (ACI_GRID_STEP * 20) == 0 {
            eprintln!("  grid {}", Date(g));
        }
    }
    let mut sa_raw: HashMap<Day, [f64; 3]> = HashMap::new();
    let mut out = CaseStudy {
        rows: Vec::new(),
        origins: 0,
        sparse_origins: 0,
        income_origins: 0,
    };
    for (n, &o) in os.iter().enumerate() {
        if n % 50 == 0 {
            eprintln!("  origin {n}/{}", os.len());
        }
        let k = REFERENCE_SCALE / acct.scale_at(o).expect("eligible");
        let Some(p) = prepare(acct, &bal_raw, o, k, true) else {
            continue;
        };
        out.origins += 1;
        out.sparse_origins += usize::from(p.sparse.is_some());
        out.income_origins += usize::from(p.income_detected);
        let t = (o - first) as usize;
        let bal: Vec<f64> = bal_raw[..=t].iter().map(|b| b * k).collect();
        for (hi, &h) in HORIZONS.iter().enumerate() {
            let truth = bal_raw[t + h as usize] * k;
            let stream = aci_levels(
                grid.iter()
                    .filter(|(g, _, _)| g + h <= o)
                    .map(|(g, kg, d)| (&d[hi], bal_raw[(g + h - first) as usize] * kg)),
            );
            let v1 = recalibrate(&p.dists[hi], &stream);
            let v2 = p.sparse.as_ref().map_or(&p.dists[hi], |d| &d[hi]).clone();
            let sa_point = bal[t] + schedule_aware_delta(&p.txns, &p.schedule, o, h);
            let mut sa_errors = Vec::new();
            let mut g = first + SA_GRID_START;
            while g + h <= o {
                if (o - g) as usize <= LOOKBACK {
                    let d = *sa_raw.entry(g).or_insert_with(|| {
                        tagged(acct, g, 1.0).map_or([f64::NAN; 3], |tx| {
                            let s = detect_schedule_in(&tx, CALENDAR);
                            HORIZONS.map(|hh| schedule_aware_delta(&tx, &s, g, hh))
                        })
                    });
                    let gi = (g - first) as usize;
                    let err = bal_raw[gi + h as usize] - (bal_raw[gi] + d[hi]);
                    if err.is_finite() {
                        sa_errors.push(err * k);
                    }
                }
                g += SA_GRID_STEP;
            }
            let mut dists: Vec<(&str, Dist)> = vec![
                ("V0", p.dists[hi].clone()),
                ("V1", v1),
                ("V2", v2),
                ("B-30", avg30(&bal, t, h)),
                ("B-LIN", straight_line(&bal, t, h)),
                ("B-SA", Dist::Point(sa_point)),
                ("B-SA+", schedule_aware_plus(sa_point, &sa_errors)),
                ("B-EQ", empirical_changes(&bal, t, h)),
            ];
            if let Some(d) = seasonal(&bal, first, t, h) {
                dists.push(("B-SEAS", d));
            }
            for (method, d) in dists {
                out.rows.push(Row {
                    account: ACCOUNT,
                    origin: Date(o).to_string(),
                    h,
                    method: method.to_string(),
                    k,
                    truth,
                    score: d.score(truth),
                    income_detected: p.income_detected,
                    months_history: (o - first) / 30,
                    alarm_recent: p.alarm_recent,
                });
            }
        }
    }
    out
}

/// Monthly balance path and what the tagger made of the full history, for
/// the data table (T3).
pub fn summary(acct: &Account) -> BTreeMap<&'static str, String> {
    let mut s = BTreeMap::new();
    let st = acct.stats();
    s.insert("records", st.records.to_string());
    s.insert("days_with_records", st.days_with_records.to_string());
    s.insert("first_day", Date(acct.first_day()).to_string());
    s.insert("last_day", Date(acct.last_day()).to_string());
    s.insert("max_drift_gbp", format!("{:.2}", st.max_drift));
    let months = f64::from((acct.last_day() - acct.first_day() + 1) as u32) / (365.25 / 12.0);
    s.insert(
        "records_per_month",
        format!("{:.1}", st.records as f64 / months),
    );
    if let Some(tx) = tagged(acct, acct.last_day(), 1.0) {
        for (kind, name) in [
            (ephemeris_core::FlowKind::Income, "tagged_income"),
            (ephemeris_core::FlowKind::Scheduled, "tagged_scheduled"),
            (
                ephemeris_core::FlowKind::Discretionary,
                "tagged_discretionary",
            ),
            (ephemeris_core::FlowKind::Transfer, "tagged_transfer"),
        ] {
            s.insert(
                name,
                tx.iter().filter(|t| t.kind == kind).count().to_string(),
            );
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_both_formats() {
        assert_eq!(parse_date("25/07/2022"), Ok(day_from_civil(2022, 7, 25)));
        // Excel's misreading of 12/07/2022 (12 July) as December 7.
        assert_eq!(parse_date("2022-12-07"), Ok(day_from_civil(2022, 7, 12)));
        assert!(parse_date("2022-07-25").is_err());
    }

    #[test]
    fn parse_orders_chains_and_numbers() {
        let tsv = "Transaction Number\tTransaction Date\tTransaction Type\tTransaction Description\tDebit Amount\tCredit Amount\tBalance\tCategory\tLocation City\tLocation Country\n\
                   2\t02/01/2020\tDEB\tSHOP\t10\t\\N\t90\tGroceries\tX\tUK\n\
                   1\t2020-03-01\tBGC\tSWANSEA UNI - PAY\t\\N\t1000\t1090\tPaycheck\tX\tUK\n\
                   3\t01/01/2020\tDEB\tCAFE\t5\t\\N\t100\tDine Out\tX\tUK\n";
        let a = parse(tsv).unwrap();
        assert_eq!(a.opening, 105.0);
        let days: Vec<Day> = a.records.iter().map(|r| r.day).collect();
        assert_eq!(
            days,
            [
                day_from_civil(2020, 1, 1),
                day_from_civil(2020, 1, 2),
                day_from_civil(2020, 1, 3)
            ]
        );
        assert_eq!(
            a.daily_balance(day_from_civil(2020, 1, 3)),
            [100.0, 90.0, 1090.0]
        );
        let broken = tsv.replace("\t90\tGroceries", "\t91\tGroceries");
        assert!(parse(&broken).is_err());
    }
}
