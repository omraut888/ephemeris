//! Native half of the byte-identity proof (docs/phase2-design.md §2).
//!
//! The same feed and report must come out of the native build and the WASM
//! build, byte for byte. Both builds compare against the files in
//! `tests/golden/`: this test natively, `tests/wasm.rs` inside WASM (run with
//! `wasm-pack test --node crates/ephemeris-wasm`). `UPDATE_GOLDEN=1
//! cargo test -p ephemeris-wasm --test golden` rewrites the files, which is
//! only right after a deliberate change to the engine's output.

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use ephemeris_wasm::{forecast_json, simulate_json};

include!("common/golden_inputs.rs");

fn check(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_GOLDEN=1)", path.display()));
    assert!(
        expected == actual,
        "{name} differs from the golden file ({} vs {} bytes)",
        expected.len(),
        actual.len()
    );
}

#[test]
fn native_output_matches_the_golden_files() {
    let feed = simulate_json(SIMULATE_OPTIONS).unwrap();
    check("feed.json", &feed);
    let report = forecast_json(&feed, FORECAST_OPTIONS).unwrap();
    check("report.json", &report);
}

/// Every field and every enum variant of the report schema, for the Swift
/// drift test (docs/phase3-design.md §1). Built from struct literals, so a
/// new field fails to compile here until the fixture includes it; each
/// enum's variants are listed through a `match` with no wildcard, so a new
/// variant fails the same way. The values are arbitrary, not engine output.
mod all_fields {
    use ephemeris_core::schema::*;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn est(mean: f64) -> Estimate {
        Estimate {
            mean,
            sd: 10.5,
            low_2sd: mean - 21.0,
            high_2sd: mean + 21.0,
        }
    }

    fn cadences() -> Vec<PayCadence> {
        let all = vec![
            PayCadence::Weekly {
                anchor: d("2026-10-02"),
            },
            PayCadence::Biweekly {
                anchor: d("2026-10-09"),
            },
            PayCadence::SemiMonthly { days: [1, 15] },
            PayCadence::Monthly { day: 28 },
        ];
        for c in &all {
            match c {
                PayCadence::Weekly { .. }
                | PayCadence::Biweekly { .. }
                | PayCadence::SemiMonthly { .. }
                | PayCadence::Monthly { .. } => {}
            }
        }
        all
    }

    fn shifts() -> Vec<Shift> {
        let all = vec![
            Shift::None,
            Shift::PreviousBusinessDay,
            Shift::NextBusinessDay,
        ];
        for s in &all {
            match s {
                Shift::None | Shift::PreviousBusinessDay | Shift::NextBusinessDay => {}
            }
        }
        all
    }

    fn channels() -> Vec<ChangeChannel> {
        let all = vec![ChangeChannel::Spend, ChangeChannel::Income];
        for c in &all {
            match c {
                ChangeChannel::Spend | ChangeChannel::Income => {}
            }
        }
        all
    }

    fn bases() -> Vec<CycleBasisReport> {
        let all = vec![CycleBasisReport::Paycheck, CycleBasisReport::Week];
        for b in &all {
            match b {
                CycleBasisReport::Paycheck | CycleBasisReport::Week => {}
            }
        }
        all
    }

    fn report(cadence: Option<PayCadence>, basis: Option<CycleBasisReport>) -> ForecastReport {
        let q = |m: f64| vec![m - 200.0, m - 75.5, m, m + 75.5, m + 200.0];
        ForecastReport {
            schema_version: SCHEMA_VERSION,
            as_of: d("2026-10-21"),
            settled_through: d("2026-10-20"),
            balance: est(421.38),
            spend_rate: est(67.04),
            spend_rate_range: Some(SpendRange {
                floor: 0.0,
                low: 8.94,
                high: 139.59,
                below_floor: 0.03125,
            }),
            expected_paycheck: est(810.0),
            income: cadence.map(|cadence| IncomeReport {
                name: Some("Payroll".into()),
                cadence,
                last_paid_nominal: d("2026-10-16"),
                next_scheduled: Some(d("2026-10-23")),
                offset_probabilities: [0.0625, 0.125, 0.625, 0.125, 0.0625],
                fit_cost: 3,
            }),
            bills: shifts()
                .into_iter()
                .zip(1u32..)
                .map(|(shift, stream)| BillReport {
                    stream,
                    name: Some(format!("Bill {stream}")),
                    amount: -12.65 * f64::from(stream),
                    period_days: 30,
                    monthly_on: Some(MonthlyOn { day: 21, shift }),
                    last_charged: d("2026-09-21"),
                    next_due: d("2026-11-21"),
                })
                .chain([BillReport {
                    stream: 9,
                    name: None,
                    amount: 0.12,
                    period_days: 14,
                    monthly_on: None,
                    last_charged: d("2026-10-14"),
                    next_due: d("2026-10-28"),
                }])
                .collect(),
            regime_changes: channels()
                .into_iter()
                .map(|channel| RegimeChange {
                    date: d("2026-09-24"),
                    settled: channel == ChangeChannel::Spend,
                    channel,
                    shift: 19.5,
                })
                .collect(),
            forecast: ConeReport {
                horizon_days: 2,
                paths: 100,
                seed: 11,
                levels: vec![0.05, 0.25, 0.5, 0.75, 0.95],
                days: vec![
                    ConeDay {
                        date: d("2026-10-22"),
                        quantiles: q(400.0),
                    },
                    ConeDay {
                        date: d("2026-10-23"),
                        quantiles: q(1150.25),
                    },
                ],
                targets: vec![TargetReport {
                    amount: 0.0,
                    at_least_on_day: vec![0.875, 0.9375],
                    reached_by: vec![0.875, 0.96875],
                    below_by: vec![0.125, 0.15625],
                }],
                cycle_lows: basis.map(|basis| CycleLowsReport {
                    basis,
                    cycles: vec![
                        CycleLowReport {
                            start: d("2026-10-22"),
                            end: d("2026-10-22"),
                            partial: true,
                            quantiles: q(350.0),
                        },
                        CycleLowReport {
                            start: d("2026-10-23"),
                            end: d("2026-10-23"),
                            partial: false,
                            quantiles: q(1000.0),
                        },
                    ],
                }),
            },
        }
    }

    /// One report per cadence, cycling through the cycle bases, plus one
    /// with no income and no cycle lows to cover the nulls.
    pub fn reports() -> Vec<ForecastReport> {
        let bases = bases();
        let mut out: Vec<ForecastReport> = cadences()
            .into_iter()
            .enumerate()
            .map(|(i, c)| report(Some(c), Some(bases[i % bases.len()])))
            .collect();
        let mut bare = report(None, None);
        bare.spend_rate_range = None;
        out.push(bare);
        out
    }
}

#[test]
fn all_fields_fixture_matches_its_golden_file() {
    let json = serde_json::to_string_pretty(&all_fields::reports()).unwrap();
    check("report-all-fields.json", &json);
    // It reads back as the same reports.
    let back: Vec<ephemeris_core::schema::ForecastReport> = serde_json::from_str(&json).unwrap();
    assert_eq!(serde_json::to_string_pretty(&back).unwrap(), json);
}
