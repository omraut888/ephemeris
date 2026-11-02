//! Plaid raw JSON → schema-v1 [`Feed`] (`ephemeris import-plaid`).
//!
//! Reads what `scripts/fetch_plaid_sandbox.py` saves. Lives in the CLI so
//! neither the core nor the sync crate sees Plaid's response shapes.
//!
//! This module does only the Plaid-specific part:
//! 1. Replay `/transactions/sync` pages (`added`, `modified`, `removed`) into
//!    the current set of records.
//! 2. Turn each into a [`BankRecord`]: dates parsed, sign flipped (Plaid:
//!    positive = money out), amount otherwise as reported. Accounts become
//!    [`AccountSnapshot`]s; `depository` is Plaid's `type`.
//!
//! The tagging (depository filter, pending supersession, ids, transfers,
//! income, bills, streams, cents) is [`ephemeris_sync::tag`], shared by every
//! device that builds a feed (docs/phase4b-design.md §4.2).

use std::collections::BTreeMap;
use std::path::Path;

use ephemeris_core::schema::{Date, Feed};
use ephemeris_core::{Day, FlowKind, TxnStatus};
use ephemeris_sync::tag::{self, Notes, RECURRING_MIN, TRANSFER_WINDOW};
use ephemeris_sync::{AccountSnapshot, BankRecord};
use serde::Deserialize;

use crate::files::{read_json, write_json};

#[derive(Debug, Deserialize)]
pub struct SyncFile {
    pub pages: Vec<SyncPage>,
}

#[derive(Debug, Deserialize)]
pub struct SyncPage {
    #[serde(default)]
    pub added: Vec<PlaidTxn>,
    #[serde(default)]
    pub modified: Vec<PlaidTxn>,
    #[serde(default)]
    pub removed: Vec<Removed>,
}

#[derive(Debug, Deserialize)]
pub struct Removed {
    pub transaction_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlaidTxn {
    pub transaction_id: String,
    pub account_id: String,
    pub amount: f64,
    pub iso_currency_code: Option<String>,
    pub date: String,
    pub authorized_date: Option<String>,
    pub pending: bool,
    pub pending_transaction_id: Option<String>,
    pub merchant_name: Option<String>,
    pub name: Option<String>,
    pub personal_finance_category: Option<Category>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    pub primary: String,
    pub detailed: String,
}

#[derive(Debug, Deserialize)]
pub struct BalanceFile {
    pub accounts: Vec<Account>,
}

#[derive(Debug, Deserialize)]
pub struct Account {
    pub account_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub balances: Balances,
}

#[derive(Debug, Deserialize)]
pub struct Balances {
    pub current: Option<f64>,
    pub iso_currency_code: Option<String>,
}

pub fn to_feed(
    sync: &SyncFile,
    balances: &BalanceFile,
    today: Date,
) -> Result<(Feed, Notes), String> {
    let accounts = account_snapshots(balances);
    let records = current_records(sync)
        .into_values()
        .map(bank_record)
        .collect::<Result<Vec<_>, _>>()?;
    tag::to_feed(&records, &accounts, today)
}

/// Step 1: the current records after replaying every sync page in order.
pub fn current_records(sync: &SyncFile) -> BTreeMap<&str, &PlaidTxn> {
    let mut current: BTreeMap<&str, &PlaidTxn> = BTreeMap::new();
    for page in &sync.pages {
        for t in page.added.iter().chain(&page.modified) {
            current.insert(&t.transaction_id, t);
        }
        for r in &page.removed {
            current.remove(r.transaction_id.as_str());
        }
    }
    current
}

/// A Plaid record as a [`BankRecord`]: dates parsed, sign flipped to
/// inflows positive, amount otherwise as reported.
pub fn bank_record(t: &PlaidTxn) -> Result<BankRecord, String> {
    Ok(BankRecord {
        id: t.transaction_id.clone(),
        account: t.account_id.clone(),
        date: parse_date(&t.date, t)?,
        authorized_date: t
            .authorized_date
            .as_deref()
            .map(|d| parse_date(d, t))
            .transpose()?,
        pending: t.pending,
        pending_id: t.pending_transaction_id.clone(),
        amount: -t.amount,
        currency: t.iso_currency_code.clone(),
        merchant_name: t.merchant_name.clone(),
        name: t.name.clone(),
        category: t
            .personal_finance_category
            .as_ref()
            .map(|c| ephemeris_sync::Category {
                primary: c.primary.clone(),
                detailed: c.detailed.clone(),
            }),
    })
}

fn parse_date(s: &str, t: &PlaidTxn) -> Result<Date, String> {
    Date::parse(s).ok_or_else(|| format!("transaction {} has invalid date {s:?}", t.transaction_id))
}

/// fetch_meta.json. Fetches made before incremental fetching existed have
/// only `fetched_on` and count as full.
#[derive(Debug, Default, Deserialize)]
pub struct FetchMeta {
    pub fetched_on: String,
    /// `"full"` (from no cursor) or `"incremental"` (changes since the last fetch).
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    /// This fetch's number for its item, and the previous fetch's.
    #[serde(default)]
    pub fetch_seq: Option<u64>,
    #[serde(default)]
    pub previous_seq: Option<u64>,
    #[serde(default)]
    pub previous_fetched_on: Option<String>,
}

impl FetchMeta {
    pub fn incremental(&self) -> bool {
        self.mode.as_deref() == Some("incremental")
    }
}

/// `ephemeris import-plaid`: read the fetcher's raw files, write a feed.
pub fn run(raw: &Path, today: Option<Day>, out: &Path) -> Result<(), String> {
    let (sync, balances, today, meta) = load(raw, today)?;
    if meta.as_ref().is_some_and(FetchMeta::incremental) {
        return Err(format!(
            "{} holds an incremental fetch: only the changes since the last one, not a \
             whole feed. Record it with `ephemeris sync import-plaid`, or fetch again with \
             --full.",
            raw.display()
        ));
    }
    let (feed, notes) = to_feed(&sync, &balances, today)?;
    write_json(out, &feed)?;
    print_summary(&feed, &notes, out);
    Ok(())
}

/// The fetcher's raw files and the snapshot day (`today`, else the fetch
/// date in fetch_meta.json).
pub fn load(
    raw: &Path,
    today: Option<Day>,
) -> Result<(SyncFile, BalanceFile, Date, Option<FetchMeta>), String> {
    let sync: SyncFile = read_json(
        &raw.join("transactions_sync.json"),
        "/transactions/sync dump",
    )?;
    let balances: BalanceFile = read_json(
        &raw.join("accounts_balance.json"),
        "/accounts/balance/get dump",
    )?;
    let path = raw.join("fetch_meta.json");
    let meta: Option<FetchMeta> = if path.exists() || today.is_none() {
        Some(read_json(&path, "fetch metadata")?)
    } else {
        None
    };
    let today = match (today, &meta) {
        (Some(d), _) => Date(d),
        (None, Some(meta)) => Date::parse(&meta.fetched_on).ok_or_else(|| {
            format!(
                "{}: invalid fetched_on {:?}",
                path.display(),
                meta.fetched_on
            )
        })?,
        (None, None) => unreachable!("read above when today is None"),
    };
    Ok((sync, balances, today, meta))
}

/// The bank accounts in a balances dump.
pub fn account_snapshots(balances: &BalanceFile) -> Vec<AccountSnapshot> {
    balances
        .accounts
        .iter()
        .map(|a| AccountSnapshot {
            id: a.account_id.clone(),
            depository: a.kind == "depository",
            current: a.balances.current,
            currency: a.balances.iso_currency_code.clone(),
        })
        .collect()
}

/// What `import-plaid` and `sync materialize` print about a feed.
pub fn print_summary(feed: &Feed, notes: &Notes, out: &Path) {
    let count = |k: FlowKind| feed.transactions.iter().filter(|t| t.kind == k).count();
    let pending = feed
        .transactions
        .iter()
        .filter(|t| t.status == TxnStatus::Pending)
        .count();
    println!(
        "Wrote {} transactions ({pending} pending), {} to {}, ledger balance {:.2}, to {}",
        feed.transactions.len(),
        feed.history_start,
        feed.today,
        feed.ledger_balance,
        out.display()
    );
    println!(
        "  kinds: {} income, {} scheduled, {} discretionary, {} transfer",
        count(FlowKind::Income),
        count(FlowKind::Scheduled),
        count(FlowKind::Discretionary),
        count(FlowKind::Transfer)
    );
    println!("  adapter guesses:");
    println!("    ledger balance = sum of depository balances.current (Plaid excludes pending)");
    println!("    history_start = earliest transaction day (not the requested window)");
    if let Some(stream) = &notes.income_stream {
        let how = if notes.income_by_tiebreak {
            "won a near-tie on its name or category"
        } else {
            "the largest regular inflow"
        };
        println!("    income = {stream}: {how}");
    } else {
        println!("    no regular income stream found: the forecast has no paychecks");
    }
    let notes = [
        (
            notes.skipped_accounts,
            format!(
                "non-depository accounts skipped, with their {} transactions",
                notes.skipped_transactions
            ),
        ),
        (
            notes.scheduled_by_repeat,
            format!(
                "records tagged scheduled: same counterparty and amount {RECURRING_MIN}+ times, at a regular interval"
            ),
        ),
        (
            notes.irregular_repeats,
            format!(
                "records left discretionary: {RECURRING_MIN}+ same-amount repeats, but irregular timing"
            ),
        ),
        (
            notes.scheduled_by_category,
            "records tagged scheduled by a bill-like category".into(),
        ),
        (
            notes.transfers,
            "transfer legs matched to a counterpart in another account".into(),
        ),
        (
            notes.transfers_in_flight,
            format!(
                "unmatched transfer legs kept as transfers: under {TRANSFER_WINDOW} business days old"
            ),
        ),
        (
            notes.unmatched_transfers,
            "TRANSFER_* records with no counterpart leg, tagged as spending or income instead"
                .into(),
        ),
        (
            notes.missing_authorized_date,
            "posted records without authorized_date (posted date used)".into(),
        ),
        (
            notes.overridden,
            "records whose kind an override or rule changed (ephemeris sync list)".into(),
        ),
        (
            notes.superseded_pending,
            "pending records dropped for their posted versions".into(),
        ),
    ];
    for (n, what) in notes {
        if n > 0 {
            println!("    {n} {what}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use ephemeris_core::schema::{FeedTransaction, SCHEMA_VERSION};
    use ephemeris_sync::tag::counterparty;
    use serde_json::{Value, json};

    fn txn(id: &str, amount: f64, date: &str) -> Value {
        json!({
            "transaction_id": id,
            "account_id": "chk",
            "amount": amount,
            "iso_currency_code": "USD",
            "date": date,
            "authorized_date": date,
            "pending": false,
            "pending_transaction_id": null,
            "merchant_name": null,
            "name": "SOMETHING",
            "personal_finance_category": {"primary": "GENERAL_MERCHANDISE",
                                          "detailed": "GENERAL_MERCHANDISE_OTHER"},
            "location": {"city": null}
        })
    }

    fn with(mut t: Value, field: &str, v: Value) -> Value {
        t[field] = v;
        t
    }

    fn category(t: Value, primary: &str, detailed: &str) -> Value {
        with(
            t,
            "personal_finance_category",
            json!({"primary": primary, "detailed": detailed}),
        )
    }

    fn balances() -> BalanceFile {
        serde_json::from_value(json!({"accounts": [
            {"account_id": "chk", "type": "depository",
             "balances": {"current": 1000.004, "available": 900, "iso_currency_code": "USD"}},
            {"account_id": "sav", "type": "depository",
             "balances": {"current": 250.0, "iso_currency_code": "USD"}},
            {"account_id": "cc", "type": "credit",
             "balances": {"current": 300.0, "iso_currency_code": "USD"}}
        ]}))
        .unwrap()
    }

    fn convert(pages: Value) -> (Feed, Notes) {
        let sync: SyncFile = serde_json::from_value(json!({ "pages": pages })).unwrap();
        let today = Date::parse("2026-10-21").unwrap();
        let direct = to_feed(&sync, &balances(), today).unwrap();
        // The same fetch recorded in a sync state builds the same feed: every
        // test below checks `sync materialize` as well as `import-plaid`.
        let batch = crate::sync::fetch_batch(&sync, &balances(), today).unwrap();
        let mut state = ephemeris_sync::SyncState::new();
        state.ingest(&batch, 1, 0).unwrap();
        assert_eq!(state.materialize().unwrap(), direct);
        direct
    }

    fn one_page(added: Vec<Value>) -> Value {
        json!([{"added": added, "modified": [], "removed": []}])
    }

    fn by_desc<'a>(feed: &'a Feed, desc: &str) -> &'a FeedTransaction {
        feed.transactions
            .iter()
            .find(|t| t.description.as_deref() == Some(desc))
            .unwrap_or_else(|| panic!("no record {desc:?}"))
    }

    #[test]
    fn sign_is_flipped_and_amounts_rounded_to_cents() {
        let (feed, _) = convert(one_page(vec![
            with(txn("a", 48.2542, "2026-10-01"), "name", json!("OUT")),
            with(txn("b", -95.0, "2026-10-02"), "name", json!("IN")),
        ]));
        assert_eq!(by_desc(&feed, "OUT").amount, -48.25);
        assert_eq!(by_desc(&feed, "IN").amount, 95.0);
        // Depository current balances only, rounded: 1000.004 + 250.
        assert_eq!(feed.ledger_balance, 1250.0);
        assert_eq!(feed.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn posted_record_replaces_the_pending_one_it_names() {
        let pending = with(
            with(txn("p1", 10.0, "2026-10-19"), "pending", json!(true)),
            "name",
            json!("PENDING"),
        );
        let posted = with(
            with(
                txn("q1", 12.5, "2026-10-20"),
                "pending_transaction_id",
                json!("p1"),
            ),
            "name",
            json!("POSTED"),
        );
        let still_pending = with(
            with(txn("p2", 7.0, "2026-10-21"), "pending", json!(true)),
            "name",
            json!("OPEN"),
        );
        let (feed, notes) = convert(one_page(vec![pending, posted, still_pending]));
        assert_eq!(notes.superseded_pending, 1);
        assert_eq!(feed.transactions.len(), 2);
        let posted = by_desc(&feed, "POSTED");
        assert_eq!(posted.status, TxnStatus::Posted);
        let open = by_desc(&feed, "OPEN");
        assert_eq!((open.status, open.pending_id), (TxnStatus::Pending, None));
        // The link survives as an id distinct from every present record.
        let link = posted.pending_id.expect("pending_id kept");
        assert!(feed.transactions.iter().all(|t| t.id != link));
        assert!(feed.transactions().is_ok(), "core accepts the records");
    }

    #[test]
    fn sync_pages_replay_added_modified_and_removed() {
        let pages = json!([
            {"added": [with(txn("a", 5.0, "2026-10-01"), "name", json!("A")),
                       with(txn("b", 6.0, "2026-10-01"), "name", json!("B"))],
             "modified": [], "removed": []},
            {"added": [],
             "modified": [with(txn("a", 9.0, "2026-10-01"), "name", json!("A"))],
             "removed": [{"transaction_id": "b", "account_id": "chk"}]}
        ]);
        let (feed, _) = convert(pages);
        assert_eq!(feed.transactions.len(), 1);
        assert_eq!(by_desc(&feed, "A").amount, -9.0);
    }

    #[test]
    fn authorized_date_is_used_when_present() {
        let early = with(
            with(
                txn("a", 5.0, "2026-10-03"),
                "authorized_date",
                json!("2026-09-30"),
            ),
            "name",
            json!("AUTH"),
        );
        let missing = with(
            with(txn("b", 5.0, "2026-10-05"), "authorized_date", Value::Null),
            "name",
            json!("NOAUTH"),
        );
        let (feed, notes) = convert(one_page(vec![early, missing]));
        let a = by_desc(&feed, "AUTH");
        assert_eq!(a.authorized_date, Date::parse("2026-09-30"));
        assert_eq!(a.date, Date::parse("2026-10-03").unwrap());
        assert_eq!(by_desc(&feed, "NOAUTH").authorized_date, None);
        assert_eq!(notes.missing_authorized_date, 1);
        // History starts at the earliest authorized day.
        assert_eq!(feed.history_start, Date::parse("2026-09-30").unwrap());
        let txns = feed.transactions().unwrap();
        assert_eq!(txns[0].day(), Date::parse("2026-09-30").unwrap().0);
    }

    fn transfer(t: Value, out: bool) -> Value {
        if out {
            category(t, "TRANSFER_OUT", "TRANSFER_OUT_ACCOUNT_TRANSFER")
        } else {
            category(t, "TRANSFER_IN", "TRANSFER_IN_ACCOUNT_TRANSFER")
        }
    }

    fn named(t: Value, name: &str) -> Value {
        with(t, "name", json!(name))
    }

    #[test]
    fn only_matched_legs_are_transfers() {
        // Checking → savings, the incoming leg posting a business day later
        // (2026-10-02 is a Friday, 10-05 the next Monday).
        let out = transfer(named(txn("o", 300.0, "2026-10-02"), "TO SAVINGS"), true);
        let inc = with(
            transfer(
                named(txn("i", -300.0, "2026-10-05"), "FROM CHECKING"),
                false,
            ),
            "account_id",
            json!("sav"),
        );
        // P2P and cash with no counterpart in a linked account.
        let venmo = category(
            named(txn("v", 145.09, "2026-10-02"), "Venmo"),
            "TRANSFER_OUT",
            "TRANSFER_OUT_TRANSFER_OUT_FROM_APPS",
        );
        let cash = category(
            named(txn("c", -496.0, "2026-10-14"), "CASH DEPOSIT"),
            "TRANSFER_IN",
            "TRANSFER_IN_DEPOSIT",
        );
        // Same amount on the same account is not an own-account pair, and a
        // counterpart a week later is too late.
        let same_acct = transfer(
            named(txn("s", -145.09, "2026-10-02"), "SAME ACCOUNT"),
            false,
        );
        let late = with(
            transfer(named(txn("l", -300.0, "2026-10-12"), "LATE"), false),
            "account_id",
            json!("sav"),
        );
        let (feed, notes) = convert(one_page(vec![out, inc, venmo, cash, same_acct, late]));
        assert_eq!(by_desc(&feed, "TO SAVINGS").kind, FlowKind::Transfer);
        assert_eq!(by_desc(&feed, "FROM CHECKING").kind, FlowKind::Transfer);
        for d in ["Venmo", "CASH DEPOSIT", "SAME ACCOUNT", "LATE"] {
            assert_eq!(by_desc(&feed, d).kind, FlowKind::Discretionary, "{d}");
        }
        assert_eq!(
            (
                notes.transfers,
                notes.transfers_in_flight,
                notes.unmatched_transfers
            ),
            (2, 0, 4)
        );
        // The core sees a matched pair it can cancel.
        assert!(feed.transactions().is_ok());
    }

    #[test]
    fn a_fresh_unmatched_leg_is_in_flight_only_with_another_account() {
        // Today is 2026-10-21 (Wednesday); a leg from Monday may still match.
        let fresh = || transfer(named(txn("f", 200.0, "2026-10-19"), "TO SAVINGS"), true);
        let (feed, notes) = convert(one_page(vec![fresh()]));
        assert_eq!(by_desc(&feed, "TO SAVINGS").kind, FlowKind::Transfer);
        assert_eq!(notes.transfers_in_flight, 1);

        // With checking as the only depository account there is nowhere for
        // the counterpart to appear: spending.
        let only_checking: BalanceFile = serde_json::from_value(json!({"accounts": [
            {"account_id": "chk", "type": "depository",
             "balances": {"current": 1000.0, "iso_currency_code": "USD"}}
        ]}))
        .unwrap();
        let sync: SyncFile =
            serde_json::from_value(json!({ "pages": one_page(vec![fresh()]) })).unwrap();
        let (feed, notes) =
            to_feed(&sync, &only_checking, Date::parse("2026-10-21").unwrap()).unwrap();
        assert_eq!(by_desc(&feed, "TO SAVINGS").kind, FlowKind::Discretionary);
        assert_eq!(
            (notes.transfers_in_flight, notes.unmatched_transfers),
            (0, 1)
        );
    }

    #[test]
    fn merchant_name_and_description_are_kept() {
        let t = with(
            with(
                txn("a", 12.65, "2026-10-21"),
                "merchant_name",
                json!("Spotify"),
            ),
            "name",
            json!("SPOTIFY USA 8777"),
        );
        let (feed, _) = convert(one_page(vec![t]));
        let t = &feed.transactions[0];
        assert_eq!(t.merchant_name.as_deref(), Some("Spotify"));
        assert_eq!(t.description.as_deref(), Some("SPOTIFY USA 8777"));
    }

    #[test]
    fn only_depository_accounts_are_kept() {
        let card = with(txn("cc1", 20.0, "2026-10-01"), "account_id", json!("cc"));
        let savings = with(txn("s1", -5.0, "2026-10-01"), "account_id", json!("sav"));
        let (feed, notes) = convert(one_page(vec![card, savings, txn("c1", 1.0, "2026-10-01")]));
        assert_eq!((notes.skipped_accounts, notes.skipped_transactions), (1, 1));
        let mut accounts: Vec<u32> = feed.transactions.iter().map(|t| t.account).collect();
        accounts.sort();
        assert_eq!(
            accounts,
            [0, 1],
            "checking and savings get distinct accounts"
        );

        let none: BalanceFile = serde_json::from_value(json!({"accounts": [
            {"account_id": "cc", "type": "credit", "balances": {"current": 1.0}}
        ]}))
        .unwrap();
        let sync: SyncFile = serde_json::from_value(json!({"pages": []})).unwrap();
        assert!(to_feed(&sync, &none, Date(0)).is_err());
    }

    fn kinds_of(feed: &Feed, name: &str) -> Vec<FlowKind> {
        feed.transactions
            .iter()
            .filter(|t| {
                t.merchant_name.as_deref() == Some(name) || t.description.as_deref() == Some(name)
            })
            .map(|t| t.kind)
            .collect()
    }

    /// `n` records `period` days apart from `first`.
    fn every(
        n: usize,
        first: &str,
        period: i32,
        f: impl Fn(String, String) -> Value,
    ) -> Vec<Value> {
        let d0 = Date::parse(first).unwrap().0;
        (0..n)
            .map(|k| {
                f(
                    format!("{first}-{k}"),
                    Date(d0 + period * k as i32).to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn income_is_the_largest_regular_inflow_whatever_its_name() {
        // Weekly $810 under a restaurant category and an employer's name
        // (the Plaid sandbox's payroll), from Tuesday 2026-09-01.
        let mut records = every(6, "2026-09-01", 7, |id, d| {
            category(
                named(txn(&id, -810.0, &d), "SWEETGREEN"),
                "FOOD_AND_DRINK",
                "FOOD_AND_DRINK_RESTAURANT",
            )
        });
        // A regular but tiny inflow, a big one-off, and an irregular one
        // whose name says payroll: none of them is the income stream.
        records.extend(every(3, "2026-08-21", 31, |id, d| {
            named(txn(&id, -0.12, &d), "Interest payment")
        }));
        records.push(named(txn("bonus", -5000.0, "2026-09-15"), "BIG DEPOSIT"));
        for (id, d) in [
            ("p1", "2026-09-02"),
            ("p2", "2026-09-05"),
            ("p3", "2026-09-29"),
        ] {
            records.push(named(txn(id, -700.0, d), "ACME PAYROLL"));
        }
        let (feed, notes) = convert(one_page(records));
        assert_eq!(kinds_of(&feed, "SWEETGREEN"), [FlowKind::Income; 6]);
        assert_eq!(kinds_of(&feed, "BIG DEPOSIT"), [FlowKind::Discretionary]);
        assert_eq!(
            kinds_of(&feed, "ACME PAYROLL"),
            [FlowKind::Discretionary; 3]
        );
        assert!(!kinds_of(&feed, "Interest payment").contains(&FlowKind::Income));
        assert_eq!(notes.income_stream.as_deref(), Some("SWEETGREEN (weekly)"));
        assert!(!notes.income_by_tiebreak);
        // One stream for the income counterparty.
        let streams: BTreeSet<Option<u32>> = feed
            .transactions
            .iter()
            .filter(|t| t.kind == FlowKind::Income)
            .map(|t| t.stream)
            .collect();
        assert_eq!(streams.len(), 1);
        assert!(streams.iter().all(Option::is_some));
    }

    #[test]
    fn a_wage_name_only_breaks_a_near_tie() {
        // Two biweekly streams: $2,000 and $1,900 (within 10%). The smaller
        // one is called payroll, so it wins the tie.
        let stream = |name: &'static str, amount: f64, first: &str| {
            every(6, first, 14, move |id, d| {
                named(txn(&id, -amount, &d), name)
            })
        };
        let mut records = stream("GIG PLATFORM", 2000.0, "2026-07-03");
        records.extend(stream("ACME PAYROLL", 1900.0, "2026-07-10"));
        let (feed, notes) = convert(one_page(records.clone()));
        assert_eq!(kinds_of(&feed, "ACME PAYROLL"), [FlowKind::Income; 6]);
        assert_ne!(kinds_of(&feed, "GIG PLATFORM")[0], FlowKind::Income);
        assert!(notes.income_by_tiebreak);

        // At $1,500 it is no longer a tie: the larger stream is the income.
        let mut records = stream("GIG PLATFORM", 2000.0, "2026-07-03");
        records.extend(stream("ACME PAYROLL", 1500.0, "2026-07-10"));
        let (feed, notes) = convert(one_page(records));
        assert_eq!(kinds_of(&feed, "GIG PLATFORM"), [FlowKind::Income; 6]);
        assert!(!notes.income_by_tiebreak);
    }

    #[test]
    fn no_regular_inflow_means_no_income() {
        let wages = category(txn("w", -2000.0, "2026-10-15"), "INCOME", "INCOME_WAGES");
        let (feed, notes) = convert(one_page(vec![wages]));
        assert_eq!(feed.transactions[0].kind, FlowKind::Discretionary);
        assert_eq!(notes.income_stream, None);
    }

    #[test]
    fn scheduled_tagging() {
        use ephemeris_core::calendar::weekday;
        let merchant = |id: String, amount: f64, d: &str, name: &str| {
            with(txn(&id, amount, d), "merchant_name", json!(name))
        };
        // The same $4.50 coffee on 12 weekdays: plenty of repeats, but daily.
        let mut records: Vec<Value> = (1..=16)
            .map(|d| format!("2026-10-{d:02}"))
            .filter(|d| weekday(Date::parse(d).unwrap().0) < 5)
            .enumerate()
            .map(|(i, d)| merchant(format!("c{i}"), 4.5, &d, "Starbucks"))
            .collect();
        assert_eq!(records.len(), 12);
        // A climbing gym on scattered days, same amount: repeats, irregular.
        for (i, d) in ["2026-08-03", "2026-08-09", "2026-09-14", "2026-09-20"]
            .iter()
            .enumerate()
        {
            records.push(merchant(format!("g{i}"), 25.0, d, "Climbing gym"));
        }
        // A weekly class and a monthly subscription: regular, so bills.
        records.extend(every(4, "2026-09-07", 7, |id, d| {
            merchant(id, 20.0, &d, "Yoga")
        }));
        for (i, d) in ["2026-08-21", "2026-09-21", "2026-10-21"]
            .iter()
            .enumerate()
        {
            records.push(merchant(format!("s{i}"), 12.65, d, "Spotify"));
        }
        // A bill-like category is a bill even when seen once.
        records.push(category(
            named(txn("r", 1500.0, "2026-10-01"), "RENT"),
            "RENT_AND_UTILITIES",
            "RENT_AND_UTILITIES_RENT",
        ));
        let (feed, notes) = convert(one_page(records));
        assert_eq!(kinds_of(&feed, "Spotify"), [FlowKind::Scheduled; 3]);
        assert_eq!(kinds_of(&feed, "Yoga"), [FlowKind::Scheduled; 4]);
        assert_eq!(kinds_of(&feed, "Starbucks"), [FlowKind::Discretionary; 12]);
        assert_eq!(
            kinds_of(&feed, "Climbing gym"),
            [FlowKind::Discretionary; 4]
        );
        assert_eq!(kinds_of(&feed, "RENT"), [FlowKind::Scheduled]);
        assert_eq!(
            (
                notes.scheduled_by_repeat,
                notes.irregular_repeats,
                notes.scheduled_by_category
            ),
            (7, 16, 1)
        );
        for t in &feed.transactions {
            assert_eq!(
                t.stream.is_some(),
                matches!(t.kind, FlowKind::Income | FlowKind::Scheduled)
            );
        }
    }

    #[test]
    fn counterparty_keys() {
        assert_eq!(counterparty(Some(" Spotify "), Some("x")), "SPOTIFY");
        assert_eq!(
            counterparty(None, Some("CHECKCARD 538036 CLEO AI IN")),
            "CHECKCARD CLEO AI IN"
        );
        assert_eq!(
            counterparty(Some(""), Some("Interest payment")),
            "INTEREST PAYMENT"
        );
        assert_eq!(counterparty(None, None), "");
    }

    #[test]
    fn output_is_sorted_and_ids_are_unique() {
        let (feed, _) = convert(one_page(vec![
            txn("z", 1.0, "2026-10-01"),
            txn("a", 1.0, "2026-10-03"),
            txn("m", 1.0, "2026-10-02"),
        ]));
        let days: Vec<Date> = feed.transactions.iter().map(|t| t.date).collect();
        assert!(days.windows(2).all(|w| w[0] <= w[1]));
        let ids: BTreeSet<u64> = feed.transactions.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), 3);
    }
}
