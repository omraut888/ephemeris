//! `ephemeris simulate` then `ephemeris forecast`, through the real binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use ephemeris_core::schema::{Date, Feed, ForecastReport, PayCadence, SCHEMA_VERSION};

fn ephemeris(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ephemeris"))
        .args(args)
        .output()
        .expect("binary runs")
}

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn assert_ok(o: &Output) {
    assert!(
        o.status.success(),
        "exit {:?}\nstderr: {}",
        o.status,
        String::from_utf8_lossy(&o.stderr)
    );
}

fn simulate(seed: &str, days: &str, out: &Path) {
    let o = ephemeris(&[
        "simulate",
        "--seed",
        seed,
        "--days",
        days,
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_ok(&o);
    assert!(stdout(&o).starts_with("Wrote "), "{}", stdout(&o));
}

fn forecast(feed: &Path, json: &Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "forecast",
        "--input",
        feed.to_str().unwrap(),
        "--json",
        json.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    ephemeris(&args)
}

#[test]
fn simulate_then_forecast() {
    let (feed_path, json_path) = (tmp("e2e-feed.json"), tmp("e2e-report.json"));
    simulate("11", "200", &feed_path);
    let feed: Feed = serde_json::from_slice(&std::fs::read(&feed_path).unwrap()).unwrap();
    assert_eq!(feed.schema_version, SCHEMA_VERSION);
    assert_eq!(feed.history_start, Date::parse("2024-01-01").unwrap());
    assert_eq!(feed.today, Date::parse("2024-07-18").unwrap());

    let o = forecast(
        &feed_path,
        &json_path,
        &["--horizon", "60", "--paths", "400", "--seed", "5"],
    );
    assert_ok(&o);
    let text = stdout(&o);
    for section in [
        "EPHEMERIS forecast as of 2024-07-18",
        "Balance ",
        "Spend rate",
        "Pay schedule biweekly",
        "Recurring bills:",
        "Balance quantiles (400 paths, seed 5)",
        "  7 days  2024-07-25",
        " 30 days  2024-08-17",
        " 60 days  2024-09-16",
        "Balance cone:",
        "* median",
    ] {
        assert!(text.contains(section), "missing {section:?} in:\n{text}");
    }
    assert!(
        !text.contains(" 90 days"),
        "row beyond the horizon:\n{text}"
    );

    let raw = std::fs::read(&json_path).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(value["schema_version"], 1);
    let r: ForecastReport = serde_json::from_slice(&raw).unwrap();
    assert_eq!(r.as_of, feed.today);
    assert!(r.settled_through <= r.as_of);
    assert!(r.balance.sd > 0.0 && r.balance.low_2sd < r.balance.mean);
    assert!(matches!(
        r.income.as_ref().map(|i| i.cadence),
        Some(PayCadence::Biweekly { .. })
    ));
    assert_eq!(r.bills.len(), 4, "the generator has four subscriptions");

    let f = &r.forecast;
    assert_eq!((f.horizon_days, f.paths, f.seed), (60, 400, 5));
    assert_eq!(f.days.len(), 60);
    for (h, day) in f.days.iter().enumerate() {
        assert_eq!(day.date, Date(r.as_of.0 + 1 + h as i32));
        assert_eq!(day.quantiles.len(), f.levels.len());
        assert!(day.quantiles.windows(2).all(|w| w[0] <= w[1]));
    }
}

#[test]
fn same_inputs_give_identical_reports() {
    let feed = tmp("e2e-repro-feed.json");
    simulate("4", "120", &feed);
    let run = |name: &str| {
        let json = tmp(name);
        assert_ok(&forecast(
            &feed,
            &json,
            &["--horizon", "30", "--paths", "200"],
        ));
        std::fs::read(json).unwrap()
    };
    assert_eq!(run("e2e-repro-a.json"), run("e2e-repro-b.json"));
}

#[test]
fn bad_input_is_a_readable_error() {
    let missing = tmp("e2e-does-not-exist.json");
    let o = forecast(&missing, &tmp("e2e-unused.json"), &[]);
    assert!(!o.status.success());
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.starts_with("error: cannot open"), "{err}");
    assert!(!err.contains("panicked"), "{err}");

    let garbage = tmp("e2e-garbage.json");
    std::fs::write(&garbage, "{\"schema_version\": 1}").unwrap();
    let o = forecast(&garbage, &tmp("e2e-unused.json"), &[]);
    assert!(!o.status.success());
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.contains("is not a valid feed"), "{err}");
}
