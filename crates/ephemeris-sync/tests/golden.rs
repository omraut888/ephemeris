//! The cross-platform golden check (docs/phase4b-design.md §5), native leg.
//!
//! Two sealed replica files, from a fetcher (A) and another device (B) that
//! each edited concurrently, must open, merge, materialize and forecast to
//! the same bytes on every shell: here natively, in WASM
//! (`crates/ephemeris-wasm/tests/wasm.rs`), and through the FFI in Rust
//! (`crates/ephemeris-ffi`) and Swift (`ios/EphemerisKit`). Each merges in
//! its own order.
//!
//! `UPDATE_GOLDEN=1 cargo test -p ephemeris-sync --test golden` rewrites the
//! files, which is only right after a deliberate change to the state format,
//! the merge, the tagging or the engine.

use std::path::{Path, PathBuf};

use ephemeris_core::FlowKind;
use ephemeris_core::json::forecast_json;
use ephemeris_core::pipeline::{SimMode, SimulateOptions, simulate_feed};
use ephemeris_core::schema::Date;
use ephemeris_sync::edits::Edit;
use ephemeris_sync::ingest::FetchBatch;
use ephemeris_sync::json::{
    sync_forecast_options, sync_materialize, sync_merge, sync_new, sync_open,
};
use ephemeris_sync::state::{SavedQuery, Setting};
use ephemeris_sync::{AccountSnapshot, BankRecord, Category, SyncState, envelope};

const A: u64 = 0x00000000000000aa;
const B: u64 = 0x00000000000000bb;
const T0: u64 = 1_793_000_000_000;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// The test key, as its text form: a fixture, never a real key.
fn key() -> envelope::Key {
    let text = std::fs::read_to_string(dir().join("key.txt")).unwrap();
    envelope::key_from_text(text.trim()).unwrap()
}

/// A synthetic bank history as bank records, with the names and categories
/// the tagging reads (the synthetic feed has kinds and streams, no names).
fn bank() -> FetchBatch {
    let feed = simulate_feed(&SimulateOptions {
        seed: 7,
        days: 90,
        mode: SimMode::Realistic,
        start: Date::parse("2026-06-01").unwrap(),
    })
    .unwrap();
    const SHOPS: [&str; 6] = [
        "Cafe Uno",
        "Grocer",
        "Fuel Stop",
        "Book Barn",
        "Pizza Pie",
        "Hardware",
    ];
    let records = feed
        .transactions
        .iter()
        .map(|t| {
            let name = match (t.kind, t.stream) {
                (FlowKind::Income, _) => "Acme Payroll".to_string(),
                (FlowKind::Scheduled, Some(s)) => format!("Utility {s}"),
                (FlowKind::Transfer, _) if t.amount < 0.0 => "To Savings".into(),
                (FlowKind::Transfer, _) => "From Checking".into(),
                _ => SHOPS[(t.id as usize / 2 + t.date.0 as usize) % SHOPS.len()].to_string(),
            };
            let category = (t.kind == FlowKind::Transfer).then(|| Category {
                primary: if t.amount < 0.0 {
                    "TRANSFER_OUT"
                } else {
                    "TRANSFER_IN"
                }
                .into(),
                detailed: "TRANSFER_ACCOUNT_TRANSFER".into(),
            });
            BankRecord {
                id: format!("sim-{:05}", t.id),
                account: format!("acct-{}", t.account),
                date: t.date,
                authorized_date: t.authorized_date,
                pending: t.status == ephemeris_core::TxnStatus::Pending,
                pending_id: t.pending_id.map(|p| format!("sim-{p:05}")),
                amount: t.amount,
                currency: Some("USD".into()),
                merchant_name: Some(name.clone()),
                name: Some(name.to_uppercase()),
                category,
            }
        })
        .collect();
    let n_accounts = feed
        .transactions
        .iter()
        .map(|t| t.account)
        .max()
        .unwrap_or(0)
        + 1;
    FetchBatch {
        today: feed.today,
        window_start: feed.history_start,
        records,
        removed: Vec::new(),
        accounts: (0..n_accounts)
            .map(|a| AccountSnapshot {
                id: format!("acct-{a}"),
                depository: true,
                current: Some(if a == 0 { feed.ledger_balance } else { 0.0 }),
                currency: Some("USD".into()),
            })
            .collect(),
        complete: true,
    }
}

/// The two replicas' states: B saw A's fetch, then both edited concurrently.
fn states() -> (SyncState, SyncState) {
    let mut a = SyncState::new();
    a.ingest(&bank(), A, T0).unwrap();
    let mut b = SyncState::new();
    b.merge(&a).unwrap();
    for (r, now, edit) in [
        (
            A,
            T0 + 1_000,
            Edit::Set {
                setting: Setting::HorizonDays,
                value: 60,
            },
        ),
        (
            A,
            T0 + 2_000,
            Edit::Set {
                setting: Setting::Paths,
                value: 400,
            },
        ),
        (
            A,
            T0 + 3_000,
            Edit::Override {
                id: "sim-00017".into(),
                kind: Some(FlowKind::Scheduled),
            },
        ),
        (
            B,
            T0 + 1_500,
            Edit::Set {
                setting: Setting::HorizonDays,
                value: 45,
            },
        ),
        (
            B,
            T0 + 2_500,
            Edit::Set {
                setting: Setting::Seed,
                value: 11,
            },
        ),
        (
            B,
            T0 + 3_500,
            Edit::Rule {
                counterparty: "Pizza Pie".into(),
                kind: Some(FlowKind::Scheduled),
            },
        ),
        (
            B,
            T0 + 4_000,
            Edit::AddQuery {
                query: SavedQuery {
                    amount_cents: 150_000,
                    by: Date::parse("2026-11-15").unwrap(),
                    label: "trip".into(),
                },
            },
        ),
    ] {
        let s = if r == A { &mut a } else { &mut b };
        s.edit(&edit, r, now).unwrap();
    }
    (a, b)
}

fn check(name: &str, actual: &[u8]) {
    let path = dir().join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_GOLDEN=1)", path.display()));
    assert!(
        expected == actual,
        "{name} differs from the golden file ({} vs {} bytes)",
        expected.len(),
        actual.len()
    );
}

#[test]
fn sealed_replicas_merge_to_the_golden_state_feed_and_report() {
    let (a, b) = states();
    let key = key();
    check(
        "replica-a.ephsync",
        &envelope::seal(&a, A, &key, &[0xa1; 24]),
    );
    check(
        "replica-b.ephsync",
        &envelope::seal(&b, B, &key, &[0xb2; 24]),
    );

    // What every shell does: open both files, merge, materialize, forecast.
    let open = |name: &str| {
        let file = std::fs::read(dir().join(name)).unwrap();
        let opened: serde_json::Value =
            serde_json::from_str(&sync_open(&file, &key).unwrap()).unwrap();
        opened["state"].as_str().unwrap().to_string()
    };
    let (sa, sb) = (open("replica-a.ephsync"), open("replica-b.ephsync"));
    let merged = sync_merge(&sync_merge(&sync_new(), &sa).unwrap(), &sb).unwrap();
    assert_eq!(merged, sync_merge(&sb, &sa).unwrap(), "merge order matters");
    check("merged-state.json", merged.as_bytes());
    let feed = sync_materialize(&merged).unwrap();
    check("merged-feed.json", feed.as_bytes());
    let options = sync_forecast_options(&merged).unwrap();
    // B's horizon edit is later than A's; A's paths and B's seed both stand.
    assert_eq!(
        options,
        r#"{"horizon_days":45,"paths":400,"seed":11,"targets":[]}"#
    );
    let report = forecast_json(&feed, &options).unwrap();
    check("merged-report.json", report.as_bytes());
}
