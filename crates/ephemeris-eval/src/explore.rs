//! Exploratory analyses (paper/REVISION.md, specifications S1–S8).
//! Development split only; nothing here touches the frozen configuration.
//!
//! The builders take a [`Ctx`]: the balance series, the records and the
//! detected schedule as known at the origin. They read `bal[..=t]` only;
//! the caller may hand a longer series (for the truth), so every index
//! below is checked against `t`.

use std::collections::BTreeMap;

use ephemeris_core::adaptive::AdaptiveConfig;
use ephemeris_core::calendar::civil_from_day;
use ephemeris_core::forecast::{ForecastConfig, ForecastModel, forecast};
use ephemeris_core::schedule::{Schedule, detect_schedule_in};
use ephemeris_core::schema::Date;
use ephemeris_core::tracker::{StateEstimate, Tracker, TrackerConfig};
use ephemeris_core::types::Channel;
use ephemeris_core::{Day, FlowKind, Transaction};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, Gamma, Poisson};

use crate::baselines::schedule_aware_delta;
use crate::berka::{Account, CALENDAR, tag, transactions};
use crate::berka_run::{HORIZONS, PATHS, seed_for};
use crate::dist::{Dist, mc_levels};
use crate::reference::REFERENCE_SCALE;
use crate::rows::Row;
use crate::variants::sparse_spending;

/// The lookback of every new method (S3).
pub const L: usize = 365;
/// B-EQc: the day-of-month half-width, and the fewest matching days before
/// falling back to all days.
pub const CAL_HALF_WIDTH: u32 = 3;
pub const CAL_MIN: usize = 5;
pub const H_MAX: usize = 90;

/// What a method knows at the origin.
pub struct Ctx<'a> {
    /// Balances from the first day; only `bal[..=t]` is read.
    pub bal: &'a [f64],
    pub first: Day,
    pub t: usize,
    /// Records dated on or before the origin, in the same units as `bal`.
    pub txns: &'a [Transaction],
    pub schedule: &'a Schedule,
    pub seed: u64,
}

impl Ctx<'_> {
    pub fn origin(&self) -> Day {
        self.first + self.t as Day
    }
    fn lo(&self) -> usize {
        self.t.saturating_sub(L)
    }
}

/// The schedule's records: the paychecks of the detected income stream (if
/// the schedule found one) and the scheduled records of detected bills.
pub fn schedule_records(c: &Ctx) -> Vec<Transaction> {
    c.txns
        .iter()
        .filter(|x| {
            (x.kind == FlowKind::Income && c.schedule.income.is_some())
                || (x.kind == FlowKind::Scheduled
                    && c.schedule.bills.iter().any(|b| x.stream == Some(b.stream)))
        })
        .cloned()
        .collect()
}

/// Shared per-origin pieces: the schedule's cumulative future flow by day
/// (`cum[k]`, k = 0..=90), and the prefix sums of its past daily flows.
pub struct Sched {
    pub cum: Vec<f64>,
    /// `past[i]` = the schedule's flows dated on days `first..first+i`.
    pub past: Vec<f64>,
}

pub fn sched(c: &Ctx) -> Sched {
    let recs = schedule_records(c);
    let o = c.origin();
    let cum = (0..=H_MAX as Day)
        .map(|k| {
            if k == 0 {
                0.0
            } else {
                schedule_aware_delta(&recs, c.schedule, o, k)
            }
        })
        .collect();
    let mut daily = vec![0.0; c.t + 1];
    for r in &recs {
        let d = r.day();
        if d >= c.first && d <= o {
            daily[(d - c.first) as usize] += r.amount;
        }
    }
    let mut past = vec![0.0; c.t + 2];
    for i in 0..=c.t {
        past[i + 1] = past[i] + daily[i];
    }
    Sched { cum, past }
}

impl Sched {
    /// The schedule's flows dated in `(d, d+k]` (day indices).
    pub fn between(&self, d: usize, k: usize) -> f64 {
        self.past[d + k + 1] - self.past[d + 1]
    }
}

fn dom(c: &Ctx, i: usize) -> u32 {
    civil_from_day(c.first + i as Day).2
}

/// B-EQc (S3): past h-day changes from days near the origin's day of month.
/// Returns the distribution and whether it fell back to all days.
pub fn eq_cal(c: &Ctx, h: usize) -> (Dist, bool) {
    let od = dom(c, c.t);
    let all: Vec<usize> = (c.lo()..=c.t - h).collect();
    let near: Vec<usize> = all
        .iter()
        .copied()
        .filter(|&d| {
            let a = dom(c, d).abs_diff(od);
            a.min(31 - a) <= CAL_HALF_WIDTH
        })
        .collect();
    let (ds, fell_back) = if near.len() < CAL_MIN {
        (all, true)
    } else {
        (near, false)
    };
    let b = c.bal;
    let xs = ds.iter().map(|&d| b[c.t] + b[d + h] - b[d]).collect();
    (Dist::sample(xs), fell_back)
}

/// B-SR (S3): the schedule plus the account's own past h-day residuals.
pub fn sched_resid(c: &Ctx, s: &Sched, h: usize) -> Dist {
    let b = c.bal;
    let xs = (c.lo()..=c.t - h)
        .map(|d| b[c.t] + s.cum[h] + (b[d + h] - b[d] - s.between(d, h)))
        .collect();
    Dist::sample(xs)
}

/// The discretionary amounts in the window `(o − L, o]` and its length.
pub fn discretionary_window(c: &Ctx) -> (Vec<f64>, usize) {
    let start = c.origin() - (L as Day) + 1;
    let start = start.max(c.first);
    let amounts = c
        .txns
        .iter()
        .filter(|x| x.kind == FlowKind::Discretionary && x.day() >= start && x.day() <= c.origin())
        .map(|x| x.amount)
        .collect();
    (amounts, (c.origin() - start + 1) as usize)
}

/// CP-noKF (S3): paths `bal[t] + sched(k) + compound-Poisson events`,
/// `paths[i][k-1]` for k = 1..=90.
pub fn cp_paths(c: &Ctx, s: &Sched) -> Vec<Vec<f64>> {
    let (amounts, days) = discretionary_window(c);
    let n = amounts.len();
    let mut rng = ChaCha8Rng::seed_from_u64(c.seed);
    let gamma = Gamma::new(n as f64 + 0.5, 1.0 / days as f64).expect("gamma");
    (0..PATHS)
        .map(|_| {
            let lambda: f64 = gamma.sample(&mut rng);
            let mut b = c.bal[c.t];
            let mut path = Vec::with_capacity(H_MAX);
            let mut prev = 0.0;
            for k in 1..=H_MAX {
                b += s.cum[k] - prev;
                prev = s.cum[k];
                if n > 0 && lambda > 0.0 {
                    let events = Poisson::new(lambda).map_or(0.0, |p| p.sample(&mut rng));
                    for _ in 0..events as usize {
                        b += amounts[rng.random_range(0..n)];
                    }
                }
                path.push(b);
            }
            path
        })
        .collect()
}

fn type7(sorted: &[f64], p: f64) -> f64 {
    let pos = p * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (pos - lo as f64) * (sorted[hi] - sorted[lo])
}

/// The quantile forecast of paths at horizon `h`.
pub fn paths_dist(paths: &[Vec<f64>], h: usize) -> Dist {
    let mut v: Vec<f64> = paths.iter().map(|p| p[h - 1]).collect();
    v.sort_by(f64::total_cmp);
    let levels = mc_levels();
    let q = levels.iter().map(|&l| type7(&v, l)).collect();
    Dist::quantiles(levels, q)
}

/// The share of paths whose minimum over days 1..=h is below `u`.
pub fn paths_below(paths: &[Vec<f64>], h: usize, u: f64) -> f64 {
    let n = paths
        .iter()
        .filter(|p| p[..h].iter().any(|&x| x < u))
        .count();
    n as f64 / paths.len() as f64
}

/// B-EQp, B-SRp and CLIM event probabilities (S5) at horizon `h`.
pub fn empirical_below(c: &Ctx, s: &Sched, h: usize, u: f64) -> [f64; 3] {
    let b = c.bal;
    let ds: Vec<usize> = (c.lo()..=c.t - h).collect();
    let (mut eq, mut sr, mut clim) = (0, 0, 0);
    for &d in &ds {
        let (mut e, mut r, mut l) = (false, false, false);
        for k in 1..=h {
            let ch = b[d + k] - b[d];
            e |= b[c.t] + ch < u;
            r |= b[c.t] + s.cum[k] + ch - s.between(d, k) < u;
            l |= b[d + k] < u;
        }
        eq += usize::from(e);
        sr += usize::from(r);
        clim += usize::from(l);
    }
    let n = ds.len() as f64;
    [eq as f64 / n, sr as f64 / n, clim as f64 / n]
}

/// The S5 thresholds: 0, and the type-7 10th percentile of the balances
/// over the lookback (`bal[lo..=t]`).
pub fn thresholds(c: &Ctx) -> [f64; 2] {
    let mut v: Vec<f64> = c.bal[c.lo()..=c.t].to_vec();
    v.sort_by(f64::total_cmp);
    [0.0, type7(&v, 0.10)]
}

// ------------------------------------------------------------- Berka V2 --

/// Switches for S6 and S9 (all on: V2 as frozen). `sparse: false` is V0:
/// the same tracker and cone without the sparse rule.
#[derive(Debug, Clone, Copy)]
pub struct V2Opts {
    pub adaptive: bool,
    pub detector: bool,
    pub floor: bool,
    pub sparse: bool,
}

pub const V2_FROZEN: V2Opts = V2Opts {
    adaptive: true,
    detector: true,
    floor: true,
    sparse: true,
};

/// V2 at an origin, as `variants::forecast_at` with the sparse rule, plus
/// switches and overdraft targets. With [`V2_FROZEN`] and no targets its
/// distributions are the frozen V2's (checked by the caller).
pub struct V2Out {
    pub dists: Vec<Dist>,
    /// `below[i][hi]`: P(min over days 1..h < targets[i]).
    pub below: Vec<[f64; 3]>,
    pub k: f64,
    pub income_detected: bool,
    pub alarm_recent: bool,
    /// S9 diagnostics: the branch the cone used, the filtered estimate,
    /// and the detector and gate activity in the ingest.
    pub sparse_used: bool,
    pub est: StateEstimate,
    pub income_gated_365: u32,
    pub alarms: AlarmCounts,
}

/// Alarms by channel, over the whole history and in the last 28 days.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlarmCounts {
    pub income: u32,
    pub balance: u32,
    pub income_28: u32,
    pub balance_28: u32,
    /// Days of the first and last income alarm.
    pub first_income: Option<Day>,
    pub last_income: Option<Day>,
}

pub fn v2_forecast(
    acct: &Account,
    bal_raw: &[f64],
    origin: Day,
    opts: V2Opts,
    targets: &[f64],
) -> Option<V2Out> {
    let first = acct.first_day();
    let s = acct.scale_at(origin).filter(|&s| s > 0.0)?;
    let k = REFERENCE_SCALE / s;
    let t = (origin - first) as usize;
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let mut cfg = TrackerConfig::new(first);
    cfg.observe.holidays = CALENDAR;
    if !opts.adaptive {
        cfg.spend_var = AdaptiveConfig::fixed(cfg.spend_var.prior);
        cfg.income_var = AdaptiveConfig::fixed(cfg.income_var.prior);
    }
    if !opts.detector {
        cfg.spend_change = None;
        cfg.income_change = None;
    }
    let mut tracker = Tracker::new(cfg).ok()?;
    let ingest = tracker.ingest(&txns, bal_raw[t] * k, origin).ok()?;
    let est = tracker.estimate()?;
    let levels = mc_levels();
    let mut fcfg = ForecastConfig {
        n_paths: PATHS,
        levels: levels.clone(),
        sparse_spending: if opts.sparse {
            sparse_spending(&txns, first, origin)
        } else {
            None
        },
        targets: targets.to_vec(),
        ..ForecastConfig::new(HORIZONS[2] as u32, seed_for(acct.id, origin))
    };
    if !opts.floor {
        fcfg.spend_floor = None;
    }
    let cone = forecast(est, &schedule, &ForecastModel::from_tracker(&cfg), &fcfg).ok()?;
    let dists = HORIZONS
        .iter()
        .map(|&h| {
            Some(Dist::quantiles(
                levels.clone(),
                cone.band(origin + h)?.to_vec(),
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    let mut alarms = AlarmCounts::default();
    let mut income_gated_365 = 0;
    for d in &ingest.days {
        let recent = d.day > origin - 28;
        for j in &d.jumps {
            match j.channel {
                Channel::Income => {
                    alarms.income += 1;
                    alarms.income_28 += u32::from(recent);
                    alarms.first_income.get_or_insert(d.day);
                    alarms.last_income = Some(d.day);
                }
                _ => {
                    alarms.balance += 1;
                    alarms.balance_28 += u32::from(recent);
                }
            }
        }
        if d.day > origin - L as Day {
            income_gated_365 += d
                .innovations
                .iter()
                .filter(|i| i.channel == Channel::Income && i.gated)
                .count() as u32;
        }
    }
    let below = (0..targets.len())
        .map(|i| {
            let p = |h: Day| cone.prob_below_by(i, origin + h).unwrap_or(f64::NAN);
            [p(HORIZONS[0]), p(HORIZONS[1]), p(HORIZONS[2])]
        })
        .collect();
    Some(V2Out {
        dists,
        below,
        k,
        income_detected: schedule.income.is_some(),
        alarm_recent: ingest
            .days
            .iter()
            .any(|d| d.day > origin - 28 && !d.jumps.is_empty()),
        sparse_used: fcfg.sparse_spending.is_some(),
        est: est.clone(),
        income_gated_365,
        alarms,
    })
}

/// Everything S3, S5 and S6 need at one Berka origin: rows (new methods and
/// V2 variants) and one event line per (h, threshold).
pub struct OriginOut {
    pub rows: Vec<Row>,
    pub events: Vec<String>,
    pub cal_fallback: [bool; 3],
}

pub const EVENT_HEADER: &str =
    "account,origin,h,threshold,u,event,p_V2,p_CP-noKF,p_B-EQp,p_B-SRp,p_CLIM";

pub fn berka_origin(acct: &Account, origin: Day) -> Option<OriginOut> {
    let first = acct.first_day();
    let end = origin + HORIZONS[2];
    let bal_raw = acct.daily_balance(end);
    let t = (origin - first) as usize;
    // V2 as frozen, with the S5 targets (in normalized units).
    let s = acct.scale_at(origin).filter(|&s| s > 0.0)?;
    let k = REFERENCE_SCALE / s;
    let bal: Vec<f64> = bal_raw.iter().map(|b| b * k).collect();
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let c = Ctx {
        bal: &bal,
        first,
        t,
        txns: &txns,
        schedule: &schedule,
        seed: seed_for(acct.id, origin),
    };
    let us = thresholds(&c);
    let v2 = v2_forecast(acct, &bal_raw, origin, V2_FROZEN, &us)?;
    let sc = sched(&c);
    let paths = cp_paths(&c, &sc);
    let mut out = OriginOut {
        rows: Vec::new(),
        events: Vec::new(),
        cal_fallback: [false; 3],
    };
    let row = |method: &str, h: Day, d: &Dist| {
        let truth = bal[t + h as usize];
        Row {
            account: acct.id,
            origin: Date(origin).to_string(),
            h,
            method: method.to_string(),
            k,
            truth,
            score: d.score(truth),
            income_detected: v2.income_detected,
            months_history: (origin - first) / 30,
            alarm_recent: v2.alarm_recent,
        }
    };
    let ablations: Vec<(&str, V2Out)> = [
        (
            "V2-noadapt",
            V2Opts {
                adaptive: false,
                ..V2_FROZEN
            },
        ),
        (
            "V2-nodetector",
            V2Opts {
                detector: false,
                ..V2_FROZEN
            },
        ),
        (
            "V2-nofloor",
            V2Opts {
                floor: false,
                ..V2_FROZEN
            },
        ),
    ]
    .into_iter()
    .filter_map(|(n, o)| Some((n, v2_forecast(acct, &bal_raw, origin, o, &[])?)))
    .collect();
    for (hi, &h) in HORIZONS.iter().enumerate() {
        let hu = h as usize;
        let (eqc, fb) = eq_cal(&c, hu);
        out.cal_fallback[hi] = fb;
        out.rows.push(row("V2", h, &v2.dists[hi]));
        out.rows.push(row("B-EQc", h, &eqc));
        out.rows.push(row("B-SR", h, &sched_resid(&c, &sc, hu)));
        out.rows.push(row("CP-noKF", h, &paths_dist(&paths, hu)));
        for (n, a) in &ablations {
            out.rows.push(row(n, h, &a.dists[hi]));
        }
        for (i, (&u, name)) in us.iter().zip(["u0", "u10"]).enumerate() {
            let event = bal[t + 1..=t + hu].iter().any(|&x| x < u);
            let [eq, sr, clim] = empirical_below(&c, &sc, hu, u);
            out.events.push(format!(
                "{},{},{h},{name},{u},{},{},{},{},{},{}",
                acct.id,
                Date(origin),
                u8::from(event),
                v2.below[i][hi],
                paths_below(&paths, hu, u),
                eq,
                sr,
                clim
            ));
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ S9 --

pub const DETECTOR_STATE_HEADER: &str = "account,origin,config,sparse_used,income_detected,\
b,c,m,sd_b,sd_c,sd_m,income_var,income_dof,spend_var,spend_dof,income_gated_365,\
alarms_income,alarms_balance,alarms_income_28,alarms_balance_28,last_pay,future_pay_mean,\
future_pays,k,\
first_income_alarm_age,last_income_alarm_before_origin,pay1,pay2,lumps_n,lumps_sum,lump_days";

/// S9 at one origin: V0, V0 without the detector, V2 and V2 without the
/// detector, each with the score rows and one state line. `None` unless all
/// four forecast (common support).
pub fn detector_origin(acct: &Account, origin: Day) -> Option<(Vec<Row>, Vec<String>)> {
    let first = acct.first_day();
    let t = (origin - first) as usize;
    let bal_raw = acct.daily_balance(origin + HORIZONS[2]);
    let configs = [
        (
            "V0",
            V2Opts {
                sparse: false,
                ..V2_FROZEN
            },
        ),
        (
            "V0-nodet",
            V2Opts {
                sparse: false,
                detector: false,
                ..V2_FROZEN
            },
        ),
        ("V2", V2_FROZEN),
        (
            "V2-nodet",
            V2Opts {
                detector: false,
                ..V2_FROZEN
            },
        ),
    ];
    let outs: Vec<(&str, V2Out)> = configs
        .iter()
        .map(|&(n, o)| Some((n, v2_forecast(acct, &bal_raw, origin, o, &[])?)))
        .collect::<Option<_>>()?;
    let k = outs[0].1.k;
    // Paychecks (tagged income) before and after the origin. The later ones
    // are tagged from the record through origin + 90: retrospective, for
    // the diagnostic only.
    let pays = |through: Day| {
        let recs = acct.records_through(through);
        transactions(recs, &tag(recs), k)
            .into_iter()
            .filter(|x| x.kind == FlowKind::Income)
            .collect::<Vec<_>>()
    };
    let last_pay = pays(origin)
        .iter()
        .filter(|x| x.date <= origin)
        .max_by_key(|x| x.date)
        .map_or(f64::NAN, |x| x.amount);
    let future: Vec<f64> = pays(origin + HORIZONS[2])
        .iter()
        .filter(|x| x.date > origin && x.date <= origin + HORIZONS[2])
        .map(|x| x.amount)
        .collect();
    let early: Vec<f64> = pays(origin)
        .iter()
        .filter(|x| x.date <= origin)
        .take(2)
        .map(|x| x.amount)
        .collect();
    let opt = |d: Option<Day>| d.map_or(String::new(), |d| d.to_string());
    let future_mean = if future.is_empty() {
        f64::NAN
    } else {
        future.iter().sum::<f64>() / future.len() as f64
    };
    let mut rows = Vec::new();
    let mut states = Vec::new();
    for (name, o) in &outs {
        for (hi, &h) in HORIZONS.iter().enumerate() {
            let truth = bal_raw[t + h as usize] * k;
            rows.push(Row {
                account: acct.id,
                origin: Date(origin).to_string(),
                h,
                method: (*name).to_string(),
                k,
                truth,
                score: o.dists[hi].score(truth),
                income_detected: o.income_detected,
                months_history: (origin - first) / 30,
                alarm_recent: o.alarm_recent,
            });
        }
        let e = &o.est;
        let a = o.alarms;
        states.push(format!(
            "{},{},{name},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            acct.id,
            Date(origin),
            u8::from(o.sparse_used),
            u8::from(o.income_detected),
            e.x[0],
            e.x[1],
            e.x[2],
            e.p[(0, 0)].sqrt(),
            e.p[(1, 1)].sqrt(),
            e.p[(2, 2)].sqrt(),
            e.income_var.value,
            e.income_var.dof,
            e.spend_var.value,
            e.spend_var.dof,
            o.income_gated_365,
            a.income,
            a.balance,
            a.income_28,
            a.balance_28,
            last_pay,
            future_mean,
            future.len(),
            k,
            opt(a.first_income.map(|d| d - first)),
            opt(a.last_income.map(|d| origin - d)),
            early.first().copied().unwrap_or(f64::NAN),
            early.get(1).copied().unwrap_or(f64::NAN),
            e.lumps.amounts.len(),
            e.lumps.amounts.iter().sum::<f64>(),
            e.lumps.days
        ));
    }
    Some((rows, states))
}

// ----------------------------------------------------------------- S11 --

/// One projected or realized scheduled flow, by counterparty.
#[derive(Debug, Clone)]
pub struct SchedEvent {
    pub day: Day,
    pub amount: f64,
    pub cp: String,
}

/// The counterparties of the streams `schedule` detected in `recs`: the
/// income counterparty (if income was detected) and each bill's.
fn stream_counterparties(
    recs: &[crate::berka::Record],
    txns: &[Transaction],
    schedule: &Schedule,
) -> (Option<String>, Vec<Option<String>>) {
    let income = schedule.income.as_ref().and_then(|_| {
        recs.iter()
            .zip(txns)
            .rfind(|(_, x)| x.kind == FlowKind::Income)
            .map(|(r, _)| r.counterparty())
    });
    let bills = schedule
        .bills
        .iter()
        .map(|b| {
            recs.iter()
                .zip(txns)
                .rfind(|(_, x)| x.stream == Some(b.stream))
                .map(|(r, _)| r.counterparty())
        })
        .collect();
    (income, bills)
}

/// The schedule's projected events in `(today, today + h]`, as
/// `schedule_aware_delta` places them (overdue ones land tomorrow).
fn projected_events(
    recs: &[crate::berka::Record],
    txns: &[Transaction],
    schedule: &Schedule,
    today: Day,
    h: Day,
) -> Vec<SchedEvent> {
    let end = today + h;
    let (income_cp, bill_cps) = stream_counterparties(recs, txns, schedule);
    let mut out = Vec::new();
    if let (Some(timing), Some(cp)) = (&schedule.income, income_cp) {
        let pay = txns
            .iter()
            .filter(|x| x.kind == FlowKind::Income)
            .max_by_key(|x| (x.day(), x.id))
            .map_or(0.0, |x| x.amount);
        for n in timing
            .schedule
            .nominal_dates(timing.last_paid_nominal + 1, end + 8)
        {
            let d = timing.pay_date(n).max(today + 1);
            if d <= end {
                out.push(SchedEvent {
                    day: d,
                    amount: pay,
                    cp: cp.clone(),
                });
            }
        }
    }
    for (bill, cp) in schedule.bills.iter().zip(bill_cps) {
        let Some(cp) = cp else { continue };
        let mut next = bill.next_due().max(today + 1);
        while next <= end {
            out.push(SchedEvent {
                day: next,
                amount: bill.amount,
                cp: cp.clone(),
            });
            next = match bill.day_of_month {
                Some(a) => a.next_after(next),
                None => next + bill.period_days as Day,
            };
        }
    }
    out
}

/// Records in `(from, to]` whose counterparty is in `cps`, in units of `k`.
fn realized_events(acct: &Account, from: Day, to: Day, cps: &[String], k: f64) -> Vec<SchedEvent> {
    acct.records_through(to)
        .iter()
        .filter(|r| r.day > from)
        .filter_map(|r| {
            let cp = r.counterparty();
            cps.contains(&cp).then_some(SchedEvent {
                day: r.day,
                amount: r.amount * k,
                cp,
            })
        })
        .collect()
}

/// Schedule predictability from history at the origin (S11).
#[derive(Debug, Clone, Copy, Default)]
pub struct Predictability {
    pub checkpoints: u32,
    pub abs_err: f64,
    pub abs_real: f64,
    pub date_err_sum: f64,
    pub matched: u32,
    pub projected: u32,
    pub missed: u32,
    pub realized: u32,
    pub extra: u32,
}

const MATCH_DAYS: Day = 10;

fn predictability(acct: &Account, origin: Day, k: f64) -> Predictability {
    let first = acct.first_day();
    let mut p = Predictability::default();
    for j in 1..=6 {
        let c = origin - 30 * j;
        if c < first + 90 {
            break;
        }
        let recs = acct.records_through(c);
        let txns = transactions(recs, &tag(recs), k);
        let schedule = detect_schedule_in(&txns, CALENDAR);
        let (inc, bills) = stream_counterparties(recs, &txns, &schedule);
        let cps: Vec<String> = inc.into_iter().chain(bills.into_iter().flatten()).collect();
        let proj = projected_events(recs, &txns, &schedule, c, 30);
        let real = realized_events(acct, c, c + 30, &cps, k);
        p.checkpoints += 1;
        let ps: f64 = proj.iter().map(|e| e.amount).sum();
        let rs: f64 = real.iter().map(|e| e.amount).sum();
        p.abs_err += (rs - ps).abs();
        p.abs_real += rs.abs();
        p.projected += proj.len() as u32;
        p.realized += real.len() as u32;
        for e in &proj {
            let near = real
                .iter()
                .filter(|r| r.cp == e.cp)
                .map(|r| (r.day - e.day).abs())
                .min();
            match near {
                Some(dd) if dd <= MATCH_DAYS => {
                    p.matched += 1;
                    p.date_err_sum += f64::from(dd);
                }
                _ => p.missed += 1,
            }
        }
        for r in &real {
            let has = proj
                .iter()
                .any(|e| e.cp == r.cp && (e.day - r.day).abs() <= MATCH_DAYS);
            p.extra += u32::from(!has);
        }
    }
    p
}

pub const SCHEDULE_STATE_HEADER: &str = "account,origin,income,bills,checkpoints,abs_err,abs_real,\
date_err_sum,matched,projected,missed,realized,extra";

/// S11 at one origin: F_0, F_0.5, F_1 and the oracle, and the origin's
/// predictability line.
pub fn schedule_origin(acct: &Account, origin: Day) -> Option<(Vec<Row>, String)> {
    let first = acct.first_day();
    let end = origin + HORIZONS[2];
    let bal_raw = acct.daily_balance(end);
    let t = (origin - first) as usize;
    let s = acct.scale_at(origin).filter(|&s| s > 0.0)?;
    let k = REFERENCE_SCALE / s;
    let bal: Vec<f64> = bal_raw.iter().map(|b| b * k).collect();
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let c = Ctx {
        bal: &bal,
        first,
        t,
        txns: &txns,
        schedule: &schedule,
        seed: seed_for(acct.id, origin),
    };
    let sc = sched(&c);
    let (inc_cp, bill_cps) = stream_counterparties(recs, &txns, &schedule);
    let bill_cps: Vec<String> = bill_cps.into_iter().flatten().collect();
    let months = (origin - first) / 30;
    let mut rows = Vec::new();
    for &h in &HORIZONS {
        let hu = h as usize;
        if c.t < hu || c.lo() > c.t - hu {
            return None;
        }
        // S_hist's rule applied to the tagging of the records through t+h
        // (REVISION.md, S11 oracle correction).
        let recs_f = acct.records_through(origin + h);
        let tag_f = tag(recs_f);
        let s_real: f64 = recs_f
            .iter()
            .zip(&tag_f.kinds)
            .filter(|(r, _)| r.day > origin)
            .filter(|(r, kind)| {
                let cp = r.counterparty();
                (**kind == FlowKind::Income && inc_cp.as_ref() == Some(&cp))
                    || (**kind == FlowKind::Scheduled && bill_cps.contains(&cp))
            })
            .map(|(r, _)| r.amount * k)
            .sum();
        let truth = bal[t + hu];
        let mk = |w: f64, oracle: bool| {
            let xs = (c.lo()..=c.t - hu)
                .map(|d| {
                    let past = bal[d + hu] - bal[d];
                    let hist = sc.between(d, hu);
                    if oracle {
                        bal[t] + past - hist + s_real
                    } else {
                        bal[t] + past + w * (sc.cum[hu] - hist)
                    }
                })
                .collect();
            Dist::sample(xs)
        };
        // S14: within-window dependence.
        let ds: Vec<usize> = (c.lo()..=c.t - hu).collect();
        let hist: Vec<f64> = ds.iter().map(|&d| sc.between(d, hu)).collect();
        let resid: Vec<f64> = ds
            .iter()
            .zip(&hist)
            .map(|(&d, &s)| bal[d + hu] - bal[d] - s)
            .collect();
        let n = ds.len();
        let mut shuf = Vec::with_capacity(4 * n);
        for r in 0..4u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(c.seed.wrapping_add(r));
            let mut perm: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                perm.swap(i, rng.random_range(0..=i));
            }
            shuf.extend((0..n).map(|i| bal[t] + hist[i] + resid[perm[i]]));
        }
        let ident: Vec<f64> = (0..n).map(|i| bal[t] + resid[i] + hist[i]).collect();
        let matched = |target: f64| {
            let mut idx: Vec<usize> = (0..n).collect();
            idx.sort_by(|&x, &y| {
                (hist[x] - target)
                    .abs()
                    .total_cmp(&(hist[y] - target).abs())
                    .then(y.cmp(&x))
            });
            let k = 10.max((0.2 * n as f64).ceil() as usize).min(n);
            Dist::sample(
                idx[..k]
                    .iter()
                    .map(|&i| bal[t] + target + resid[i])
                    .collect(),
            )
        };
        for (name, d) in [
            ("F0", mk(0.0, false)),
            ("F0.5", mk(0.5, false)),
            ("F1", mk(1.0, false)),
            ("O", mk(0.0, true)),
            ("EQ-shuf", Dist::sample(shuf)),
            ("EQ-ident", Dist::sample(ident)),
            ("SR-match", matched(sc.cum[hu])),
            ("O-match", matched(s_real)),
        ] {
            rows.push(Row {
                account: acct.id,
                origin: Date(origin).to_string(),
                h,
                method: name.to_string(),
                k,
                truth,
                score: d.score(truth),
                income_detected: schedule.income.is_some(),
                months_history: months,
                alarm_recent: false,
            });
        }
    }
    let p = predictability(acct, origin, k);
    let line = format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{}",
        acct.id,
        Date(origin),
        u8::from(schedule.income.is_some()),
        schedule.bills.len(),
        p.checkpoints,
        p.abs_err,
        p.abs_real,
        p.date_err_sum,
        p.matched,
        p.projected,
        p.missed,
        p.realized,
        p.extra
    );
    Some((rows, line))
}

// ----------------------------------------------------------------- S12 --

/// The buffers around zero (CZK, raw), S12 item 3.
pub const BUFFERS_CZK: [f64; 2] = [100.0, 1000.0];

/// The bank's reported end-of-day balance (the day's last record, in file
/// order), carried forward over days without records, for `first..=to`.
fn reported_balance(acct: &Account, to: Day) -> Vec<f64> {
    let recs = acct.records_through(to);
    let first = acct.first_day();
    let mut out = Vec::with_capacity((to - first + 1).max(0) as usize);
    let mut b = recs.first().map_or(f64::NAN, |r| r.balance);
    let mut i = 0;
    for d in first..=to {
        while i < recs.len() && recs[i].day == d {
            b = recs[i].balance;
            i += 1;
        }
        out.push(b);
    }
    out
}

/// CLIM-D: among the past windows, the `max(10, ⌈0.2 n⌉)` whose starting
/// balance is closest to today's (ties: the more recent first), the share
/// in which the balance went below `u`.
pub fn clim_d(c: &Ctx, h: usize, u: f64) -> f64 {
    let b = c.bal;
    let mut ds: Vec<usize> = (c.lo()..=c.t - h).collect();
    ds.sort_by(|&x, &y| {
        (b[x] - b[c.t])
            .abs()
            .total_cmp(&(b[y] - b[c.t]).abs())
            .then(y.cmp(&x))
    });
    let n = ds.len();
    let k = 10.max((0.2 * n as f64).ceil() as usize).min(n);
    let hit = ds[..k]
        .iter()
        .filter(|&&d| (1..=h).any(|j| b[d + j] < u))
        .count();
    hit as f64 / k as f64
}

pub const SHORTFALL_HEADER: &str = "account,origin,h,threshold,u,b0,start_below,event,event_day0,\
event_reported,min_path,p_V2,p_CP-noKF,p_B-EQp,p_B-SRp,p_CLIM,p_CLIM-D";

/// S12 at one origin: one line per horizon and threshold.
pub fn shortfall_origin(acct: &Account, origin: Day) -> Option<Vec<String>> {
    let first = acct.first_day();
    let end = origin + HORIZONS[2];
    let bal_raw = acct.daily_balance(end);
    let rep_raw = reported_balance(acct, end);
    let t = (origin - first) as usize;
    let s = acct.scale_at(origin).filter(|&s| s > 0.0)?;
    let k = REFERENCE_SCALE / s;
    let bal: Vec<f64> = bal_raw.iter().map(|b| b * k).collect();
    let rep: Vec<f64> = rep_raw.iter().map(|b| b * k).collect();
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let c = Ctx {
        bal: &bal,
        first,
        t,
        txns: &txns,
        schedule: &schedule,
        seed: seed_for(acct.id, origin),
    };
    let [u0, u10] = thresholds(&c);
    let mut us = vec![("u0", u0), ("u10", u10)];
    for d in BUFFERS_CZK {
        us.push((if d == 100.0 { "m100" } else { "m1000" }, -d * k));
        us.push((if d == 100.0 { "p100" } else { "p1000" }, d * k));
    }
    let targets: Vec<f64> = us.iter().map(|&(_, u)| u).collect();
    let v2 = v2_forecast(acct, &bal_raw, origin, V2_FROZEN, &targets)?;
    let sc = sched(&c);
    let paths = cp_paths(&c, &sc);
    let mut out = Vec::new();
    for (hi, &h) in HORIZONS.iter().enumerate() {
        let hu = h as usize;
        let min_path = bal[t + 1..=t + hu]
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        for (i, &(name, u)) in us.iter().enumerate() {
            let event = min_path < u;
            let event0 = event || bal[t] < u;
            let event_rep = rep[t + 1..=t + hu].iter().any(|&x| x < u);
            let [eq, sr, clim] = empirical_below(&c, &sc, hu, u);
            out.push(format!(
                "{},{},{h},{name},{u},{},{},{},{},{},{},{},{},{},{},{},{}",
                acct.id,
                Date(origin),
                bal[t],
                u8::from(bal[t] < u),
                u8::from(event),
                u8::from(event0),
                u8::from(event_rep),
                min_path,
                v2.below[i][hi],
                paths_below(&paths, hu, u),
                eq,
                sr,
                clim,
                clim_d(&c, hu, u)
            ));
        }
    }
    Some(out)
}

/// S12 item 4: episodes below zero over an account's evaluated period
/// (its first eligible origin to its last plus 90 days).
pub fn shortfall_account(acct: &Account, origins: &[Day]) -> Option<String> {
    let (&lo, &hi) = (origins.first()?, origins.last()?);
    let first = acct.first_day();
    let end = hi + HORIZONS[2];
    let bal = acct.daily_balance(end);
    let (mut episodes, mut days, mut prev) = (0u32, 0u32, false);
    for d in lo..=end {
        let below = bal[(d - first) as usize] < 0.0;
        days += u32::from(below);
        episodes += u32::from(below && !prev);
        prev = below;
    }
    Some(format!(
        "{},{},{},{}",
        acct.id,
        origins.len(),
        episodes,
        days
    ))
}

// ----------------------------------------------------------------- S13 --

pub const INCOME_TRACE_HEADER: &str = "account,origin,config,n_income,pay_first,pay_last,\
gated_first12,gated_all,first_close,median_k_last12,final_s_over_pay2,final_m_over_pay,alarms_income,first15";

/// S13: the income channel's observations at an account's last eligible
/// origin, with and without the detectors (V2's tracker; V0's is the same).
pub fn income_trace(acct: &Account) -> Option<Vec<String>> {
    let origin = *crate::variants::eligible_origins(acct).last()?;
    let first = acct.first_day();
    let s = acct.scale_at(origin).filter(|&s| s > 0.0)?;
    let k = REFERENCE_SCALE / s;
    let t = (origin - first) as usize;
    let bal_raw = acct.daily_balance(origin);
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let amounts: Vec<f64> = txns
        .iter()
        .filter(|x| x.kind == FlowKind::Income)
        .map(|x| x.amount)
        .collect();
    let mut out = Vec::new();
    for (name, detector) in [("det", true), ("nodet", false)] {
        let mut cfg = TrackerConfig::new(first);
        cfg.observe.holidays = CALENDAR;
        if !detector {
            cfg.spend_change = None;
            cfg.income_change = None;
        }
        let mut tracker = Tracker::new(cfg).ok()?;
        let ingest = tracker.ingest(&txns, bal_raw[t] * k, origin).ok()?;
        let est = tracker.estimate()?;
        let inns: Vec<(f64, f64, bool)> = ingest
            .days
            .iter()
            .flat_map(|d| d.innovations.iter())
            .filter(|i| i.channel == Channel::Income)
            .map(|i| (i.innovation.nu, i.innovation.s, i.gated))
            .collect();
        let alarms = ingest
            .days
            .iter()
            .flat_map(|d| d.jumps.iter())
            .filter(|j| j.channel == Channel::Income)
            .count();
        if inns.len() != amounts.len() || inns.is_empty() {
            continue;
        }
        let n = inns.len();
        let m_pred: Vec<f64> = amounts.iter().zip(&inns).map(|(a, i)| a - i.0).collect();
        let gated12 = inns.iter().take(12).filter(|i| i.2).count();
        let gated = inns.iter().filter(|i| i.2).count();
        let first_close = (1..n)
            .find(|&i| (m_pred[i] - amounts[i]).abs() < 0.2 * amounts[i].abs())
            .map_or(String::new(), |i| i.to_string());
        let mut ks: Vec<f64> = (n.saturating_sub(13)..n - 1)
            .filter(|&i| inns[i].0 != 0.0)
            .map(|i| (m_pred[i + 1] - m_pred[i]) / inns[i].0)
            .collect();
        ks.sort_by(f64::total_cmp);
        let med_k = if ks.is_empty() {
            f64::NAN
        } else {
            ks[ks.len() / 2]
        };
        let last = amounts[n - 1];
        // The first 15 observations: amount/last : gated : NIS.
        let first15 = (0..n.min(15))
            .map(|i| {
                format!(
                    "{:.3}:{}:{:.1}",
                    amounts[i] / last,
                    u8::from(inns[i].2),
                    inns[i].0.powi(2) / inns[i].1
                )
            })
            .collect::<Vec<_>>()
            .join(";");
        out.push(format!(
            "{},{},{name},{n},{},{last},{gated12},{gated},{first_close},{med_k},{},{},{alarms},{first15}",
            acct.id,
            Date(origin),
            amounts[0],
            inns[n - 1].1 / (last * last),
            est.x[2] / last
        ));
    }
    Some(out)
}

/// The drift through each day with records (S1): `(day, max so far)`.
pub fn drift_through(acct: &Account) -> BTreeMap<Day, f64> {
    let mut out = BTreeMap::new();
    let mut b = acct.opening;
    let mut mx: f64 = 0.0;
    let mut i = 0;
    let r = &acct.records;
    while i < r.len() {
        let d = r[i].day;
        let mut reported = Vec::new();
        while i < r.len() && r[i].day == d {
            b += r[i].amount;
            reported.push(r[i].balance);
            i += 1;
        }
        let drift = reported
            .iter()
            .map(|&x| (x - b).abs())
            .fold(f64::INFINITY, f64::min);
        mx = mx.max(drift);
        out.insert(d, mx);
    }
    out
}

// ---------------------------------------------------------- S1 and S2 --

/// S1, per development account: grid origins under the frozen rule F and
/// the past-only rule P, the account measures, and the origin measures.
pub struct EligibilityAccount {
    pub account_csv: String,
    pub origin_csv: Vec<String>,
    /// Origins eligible under P but not F (to rerun).
    pub p_not_f: Vec<Day>,
}

pub const ELIG_ACCOUNT_HEADER: &str =
    "account,drift_excluded,eligible_f,eligible_p,records_per_month,volatility,income_share";
pub const ELIG_ORIGIN_HEADER: &str =
    "account,origin,f,p,minrec_ok,scale_ok,drift_to_origin,records_180,daily_sd_over_s";

pub fn eligibility_account(acct: &Account) -> EligibilityAccount {
    use crate::berka::MAX_DRIFT_CZK;
    use crate::berka_run::{MIN_RECORDS_180, ORIGIN_STEP, WARM_UP, last_day};
    let first = acct.first_day();
    let drift = drift_through(acct);
    let full_drift = drift.values().last().copied().unwrap_or(0.0);
    let excluded = full_drift > MAX_DRIFT_CZK;
    let bal = acct.daily_balance(last_day());
    let mut origins = Vec::new();
    let mut p_not_f = Vec::new();
    let (mut nf, mut np, mut grid, mut income) = (0, 0, 0, 0);
    let mut o = first + WARM_UP;
    while o + HORIZONS[2] <= last_day() {
        let n180 = acct.count_in_window(o, WARM_UP);
        let minrec = n180 >= MIN_RECORDS_180;
        let s = acct.scale_at(o).filter(|&s| s > 0.0);
        let d = drift.range(..=o).next_back().map_or(0.0, |(_, &v)| v);
        let f = minrec && s.is_some() && !excluded;
        let p = minrec && s.is_some() && d <= MAX_DRIFT_CZK;
        nf += usize::from(f);
        np += usize::from(p);
        if p && !f {
            p_not_f.push(o);
        }
        if minrec && s.is_some() {
            grid += 1;
            let recs = acct.records_through(o);
            let tx = transactions(recs, &tag(recs), 1.0);
            income += usize::from(detect_schedule_in(&tx, CALENDAR).income.is_some());
        }
        let t = (o - first) as usize;
        let lo = t.saturating_sub(WARM_UP as usize);
        let ch: Vec<f64> = bal[lo..=t].windows(2).map(|w| w[1] - w[0]).collect();
        let sd = sd(&ch);
        origins.push(format!(
            "{},{},{},{},{},{},{d},{n180},{}",
            acct.id,
            Date(o),
            u8::from(f),
            u8::from(p),
            u8::from(minrec),
            u8::from(s.is_some()),
            s.map_or(f64::NAN, |s| sd / s)
        ));
        o += ORIGIN_STEP;
    }
    let months = f64::from((acct.last_day() - first + 1) as u32) / (365.25 / 12.0);
    let ch30: Vec<f64> = bal.windows(31).map(|w| w[30] - w[0]).collect();
    let vol = acct
        .scale_at(acct.last_day())
        .filter(|&s| s > 0.0)
        .map_or(f64::NAN, |s| sd(&ch30) / s);
    EligibilityAccount {
        account_csv: format!(
            "{},{},{nf},{np},{:.4},{vol:.6},{}",
            acct.id,
            u8::from(excluded),
            acct.records.len() as f64 / months,
            if grid == 0 {
                f64::NAN
            } else {
                income as f64 / grid as f64
            }
        ),
        origin_csv: origins,
        p_not_f,
    }
}

fn sd(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return f64::NAN;
    }
    let n = xs.len() as f64;
    let m = xs.iter().sum::<f64>() / n;
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
}

/// S2(a): at each frozen origin and horizon, whether V1's adjusted levels
/// are non-decreasing, and how many of V1's quantile values the sort moved.
pub fn v1_crossings(acct: &Account) -> Vec<String> {
    use crate::variants::{
        ACI_GRID_START, ACI_GRID_STEP, ACI_LEVELS, V0, aci_levels, eligible_origins, forecast_at,
    };
    let first = acct.first_day();
    let bal_raw = acct.daily_balance(crate::berka_run::last_day());
    let origins = eligible_origins(acct);
    let Some(&last) = origins.last() else {
        return Vec::new();
    };
    let mut grid = Vec::new();
    let mut g = first + ACI_GRID_START;
    while g + HORIZONS[0] <= last {
        if let Some(f) = forecast_at(acct, &bal_raw, g, V0) {
            grid.push((g, f));
        }
        g += ACI_GRID_STEP;
    }
    let mut out = Vec::new();
    for o in origins {
        let Some(f0) = forecast_at(acct, &bal_raw, o, V0) else {
            continue;
        };
        for (hi, &h) in HORIZONS.iter().enumerate() {
            let a = aci_levels(
                grid.iter()
                    .filter(|(g, _)| g + h <= o)
                    .map(|(g, f)| (&f.dists[hi], bal_raw[(g + h - first) as usize] * f.k)),
            );
            let monotone = a.windows(2).all(|w| w[0] <= w[1]);
            // The values recalibrate() assigns, before its sort.
            let d = &f0.dists[hi];
            let mut pairs: Vec<(f64, f64)> = vec![(0.5, d.median())];
            for (&l, &al) in ACI_LEVELS.iter().zip(&a) {
                let (lo, hi_) = d.interval(al);
                pairs.push(((1.0 - l) / 2.0, lo));
                pairs.push(((1.0 + l) / 2.0, hi_));
            }
            pairs.sort_by(|x, y| x.0.total_cmp(&y.0));
            let before: Vec<f64> = pairs.iter().map(|p| p.1).collect();
            let mut after = before.clone();
            after.sort_by(f64::total_cmp);
            let moved = before.iter().zip(&after).filter(|(x, y)| x != y).count();
            out.push(format!(
                "{},{},{h},{},{moved},{}",
                acct.id,
                Date(o),
                u8::from(monotone),
                before.len()
            ));
        }
    }
    out
}

// ----------------------------------------------------------------- S8 --

/// S8: the frozen eligibility rules on a grid shifted by `offset` days.
pub fn shifted_origins(acct: &Account, offset: Day) -> Vec<Day> {
    use crate::berka_run::{MIN_RECORDS_180, ORIGIN_STEP, WARM_UP, last_day};
    let mut out = Vec::new();
    let mut o = acct.first_day() + WARM_UP + offset;
    while o + HORIZONS[2] <= last_day() {
        if acct.count_in_window(o, WARM_UP) >= MIN_RECORDS_180
            && acct.scale_at(o).is_some_and(|s| s > 0.0)
        {
            out.push(o);
        }
        o += ORIGIN_STEP;
    }
    out
}

/// S8 at one origin: V2 (frozen), B-EQ and B-SA+ (as the backtest makes
/// them), scored.
pub fn phase_origin(acct: &Account, origin: Day) -> Vec<Row> {
    use crate::baselines::{LOOKBACK, empirical_changes, schedule_aware_plus};
    use crate::berka_run::{SA_GRID_START, SA_GRID_STEP};
    let first = acct.first_day();
    let bal_raw = acct.daily_balance(origin + HORIZONS[2]);
    let Some(v2) = v2_forecast(acct, &bal_raw, origin, V2_FROZEN, &[]) else {
        return Vec::new();
    };
    let k = v2.k;
    let t = (origin - first) as usize;
    let bal: Vec<f64> = bal_raw[..=t].iter().map(|b| b * k).collect();
    let recs = acct.records_through(origin);
    let txns = transactions(recs, &tag(recs), k);
    let schedule = detect_schedule_in(&txns, CALENDAR);
    let mut cache: std::collections::HashMap<Day, [f64; 3]> = std::collections::HashMap::new();
    let mut rows = Vec::new();
    for (hi, &h) in HORIZONS.iter().enumerate() {
        let truth = bal_raw[t + h as usize] * k;
        let sa_point = bal[t] + schedule_aware_delta(&txns, &schedule, origin, h);
        let mut errors = Vec::new();
        let mut g = first + SA_GRID_START;
        while g + h <= origin {
            if (origin - g) as usize <= LOOKBACK {
                let d = *cache.entry(g).or_insert_with(|| {
                    let r = acct.records_through(g);
                    let tx = transactions(r, &tag(r), 1.0);
                    let s = detect_schedule_in(&tx, CALENDAR);
                    HORIZONS.map(|hh| schedule_aware_delta(&tx, &s, g, hh))
                });
                let gi = (g - first) as usize;
                errors.push((bal_raw[gi + h as usize] - (bal_raw[gi] + d[hi])) * k);
            }
            g += SA_GRID_STEP;
        }
        for (m, d) in [
            ("V2", v2.dists[hi].clone()),
            ("B-EQ", empirical_changes(&bal, t, h)),
            ("B-SA+", schedule_aware_plus(sa_point, &errors)),
        ] {
            rows.push(Row {
                account: acct.id,
                origin: Date(origin).to_string(),
                h,
                method: m.to_string(),
                k,
                truth,
                score: d.score(truth),
                income_detected: v2.income_detected,
                months_history: (origin - first) / 30,
                alarm_recent: v2.alarm_recent,
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type7_matches_by_hand() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(type7(&v, 0.0), 1.0);
        assert_eq!(type7(&v, 0.5), 3.0);
        assert!((type7(&v, 0.1) - 1.4).abs() < 1e-12);
    }

    #[test]
    fn empirical_paths_by_hand() {
        // Balances 10, 8, 12, 9, 11 (t = 4); no schedule.
        let bal = [10.0, 8.0, 12.0, 9.0, 11.0];
        let sched = Schedule::default();
        let c = Ctx {
            bal: &bal,
            first: 0,
            t: 4,
            txns: &[],
            schedule: &sched,
            seed: 1,
        };
        let s = Sched {
            cum: vec![0.0; H_MAX + 1],
            past: vec![0.0; 6],
        };
        // h = 1: past start days 0..=3, one-day changes −2, +4, −3, +2 →
        // shifted 9, 15, 8, 13: two of four below 10. CLIM (bal[d+1] < 10):
        // 8 yes, 12 no, 9 yes, 11 no → two of four.
        let [eq, sr, clim] = empirical_below(&c, &s, 1, 10.0);
        assert!((eq - 0.5).abs() < 1e-12);
        assert_eq!(eq, sr);
        assert!((clim - 0.5).abs() < 1e-12);
    }
}
