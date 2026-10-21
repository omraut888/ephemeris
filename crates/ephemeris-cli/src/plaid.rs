//! Plaid raw JSON → schema-v1 [`Feed`] (`ephemeris import-plaid`).
//!
//! Reads what `scripts/fetch_plaid_sandbox.py` saves. Lives in the CLI so the
//! core never sees Plaid's shapes, categories, or sign convention.
//!
//! Conversion, in order:
//! 1. Replay `/transactions/sync` pages (`added`, `modified`, `removed`) into
//!    the current set of records.
//! 2. Keep depository accounts only (design §0). The ledger balance is the
//!    sum of their `balances.current`, which Plaid reports excluding pending.
//! 3. Drop a pending record when a posted record names it in
//!    `pending_transaction_id`. The posted record keeps the link as `pending_id`.
//! 4. Flip the sign (Plaid: positive = money out) and round to cents. The
//!    day is `authorized_date` when present, otherwise the posted `date`.
//! 5. Tag each record (design §1 makes tagging the adapter's job):
//!    - `TRANSFER_IN_*` / `TRANSFER_OUT_*` category → transfer;
//!    - inflow with category `INCOME_WAGES`, or "payroll" in the name → income;
//!    - a counterparty with at least [`RECURRING_MIN`] posted records of the
//!      same amount, or a bill-like category ([`is_bill_category`]) → scheduled;
//!    - anything else → discretionary.
//!
//!    Income and scheduled records get a stream per counterparty.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use ephemeris_core::schema::{Date, Feed, FeedTransaction, SCHEMA_VERSION};
use ephemeris_core::{Day, FlowKind, TxnStatus};
use serde::Deserialize;

use crate::files::{read_json, write_json};

/// Same-amount repeats that make a counterparty a scheduled stream.
pub const RECURRING_MIN: usize = 3;

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

/// What the adapter decided that the data did not state outright.
#[derive(Debug, Default, PartialEq)]
pub struct Notes {
    pub skipped_accounts: usize,
    pub skipped_transactions: usize,
    pub superseded_pending: usize,
    pub missing_authorized_date: usize,
    pub transfers: usize,
    pub income_by_name: usize,
    pub scheduled_by_repeat: usize,
    pub scheduled_by_category: usize,
}

pub fn to_feed(
    sync: &SyncFile,
    balances: &BalanceFile,
    today: Date,
) -> Result<(Feed, Notes), String> {
    let mut notes = Notes::default();

    let mut accounts: Vec<&Account> = balances
        .accounts
        .iter()
        .filter(|a| a.kind == "depository")
        .collect();
    notes.skipped_accounts = balances.accounts.len() - accounts.len();
    if accounts.is_empty() {
        return Err("no depository (checking or savings) account in the balances".into());
    }
    accounts.sort_by(|a, b| a.account_id.cmp(&b.account_id));
    let account_index: HashMap<&str, u32> = accounts
        .iter()
        .enumerate()
        .map(|(i, a)| (a.account_id.as_str(), i as u32))
        .collect();
    let currencies: BTreeSet<&str> = accounts
        .iter()
        .filter_map(|a| a.balances.iso_currency_code.as_deref())
        .collect();
    if currencies.len() > 1 {
        return Err(format!(
            "depository accounts mix currencies: {currencies:?}"
        ));
    }
    let mut ledger = 0.0;
    for a in &accounts {
        ledger += a
            .balances
            .current
            .ok_or_else(|| format!("account {} has no current balance", a.account_id))?;
    }

    // 1. Current records after every sync page.
    let mut current: BTreeMap<&str, &PlaidTxn> = BTreeMap::new();
    for page in &sync.pages {
        for t in page.added.iter().chain(&page.modified) {
            current.insert(&t.transaction_id, t);
        }
        for r in &page.removed {
            current.remove(r.transaction_id.as_str());
        }
    }

    // 2. Depository records only.
    let before = current.len();
    current.retain(|_, t| account_index.contains_key(t.account_id.as_str()));
    notes.skipped_transactions = before - current.len();

    // 3. A posted record replaces the pending record it names.
    let replaced: BTreeSet<&str> = current
        .values()
        .filter(|t| !t.pending)
        .filter_map(|t| t.pending_transaction_id.as_deref())
        .collect();
    let before = current.len();
    current.retain(|id, t| !(t.pending && replaced.contains(id)));
    notes.superseded_pending = before - current.len();

    // Integer ids, in string order over present and referenced records.
    let id_universe: BTreeSet<&str> = current
        .keys()
        .copied()
        .chain(replaced.iter().copied())
        .collect();
    let ids: HashMap<&str, u64> = id_universe
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, i as u64 + 1))
        .collect();

    // Same-amount repeats per counterparty, over posted records.
    let key = |t: &PlaidTxn| counterparty(t.merchant_name.as_deref(), t.name.as_deref());
    let mut repeats: HashMap<(String, i64), usize> = HashMap::new();
    for t in current.values().filter(|t| !t.pending) {
        *repeats.entry((key(t), cents_key(t.amount))).or_default() += 1;
    }

    // 4–5. Convert and tag.
    let mut out = Vec::with_capacity(current.len());
    let mut stream_keys = Vec::with_capacity(current.len());
    for t in current.values() {
        if let Some(code) = &t.iso_currency_code
            && !currencies.is_empty()
            && !currencies.contains(code.as_str())
        {
            return Err(format!("transaction {} is in {code}", t.transaction_id));
        }
        let date = parse_date(&t.date, t)?;
        let authorized_date = t
            .authorized_date
            .as_deref()
            .map(|d| parse_date(d, t))
            .transpose()?;
        if authorized_date.is_none() && !t.pending {
            notes.missing_authorized_date += 1;
        }
        let k = key(t);
        let same = repeats
            .get(&(k.clone(), cents_key(t.amount)))
            .copied()
            .unwrap_or(0);
        let kind = classify(t, same, &mut notes);
        stream_keys.push(matches!(kind, FlowKind::Income | FlowKind::Scheduled).then_some(k));
        out.push(FeedTransaction {
            id: ids[t.transaction_id.as_str()],
            account: account_index[t.account_id.as_str()],
            date,
            authorized_date,
            status: if t.pending {
                TxnStatus::Pending
            } else {
                TxnStatus::Posted
            },
            amount: -t.amount,
            kind,
            stream: None,
            pending_id: t
                .pending_transaction_id
                .as_deref()
                .filter(|_| !t.pending)
                .map(|p| ids[p]),
            merchant_name: t.merchant_name.clone(),
            description: t.name.clone(),
        });
    }

    // One stream per tagged counterparty, numbered in name order.
    let names: BTreeSet<&String> = stream_keys.iter().flatten().collect();
    let stream_ids: HashMap<&String, u32> = names
        .into_iter()
        .enumerate()
        .map(|(i, k)| (k, i as u32 + 1))
        .collect();
    for (t, k) in out.iter_mut().zip(&stream_keys) {
        t.stream = k.as_ref().map(|k| stream_ids[k]);
    }
    out.sort_by_key(|t| (t.authorized_date.unwrap_or(t.date), t.date, t.id));

    let history_start = out
        .iter()
        .map(|t| t.authorized_date.unwrap_or(t.date))
        .min()
        .unwrap_or(today);
    let feed = Feed {
        schema_version: SCHEMA_VERSION,
        history_start,
        today,
        ledger_balance: ledger,
        transactions: out,
    };
    Ok((feed.rounded_to_cents(), notes))
}

/// Amount in whole cents, for grouping equal charges.
fn cents_key(amount: f64) -> i64 {
    (amount * 100.0).round() as i64
}

/// The flow kind of one record (see the module docs). `repeats` is how many
/// posted records share its counterparty and amount.
fn classify(t: &PlaidTxn, repeats: usize, notes: &mut Notes) -> FlowKind {
    let cat = t.personal_finance_category.as_ref();
    let inflow = t.amount < 0.0;
    let says_payroll = [t.name.as_deref(), t.merchant_name.as_deref()]
        .into_iter()
        .flatten()
        .any(|s| s.to_lowercase().contains("payroll"));
    if cat.is_some_and(|c| c.primary == "TRANSFER_IN" || c.primary == "TRANSFER_OUT") {
        notes.transfers += 1;
        FlowKind::Transfer
    } else if inflow && cat.is_some_and(|c| c.detailed == "INCOME_WAGES") {
        FlowKind::Income
    } else if inflow && says_payroll {
        notes.income_by_name += 1;
        FlowKind::Income
    } else if repeats >= RECURRING_MIN {
        notes.scheduled_by_repeat += 1;
        FlowKind::Scheduled
    } else if cat.is_some_and(is_bill_category) {
        notes.scheduled_by_category += 1;
        FlowKind::Scheduled
    } else {
        FlowKind::Discretionary
    }
}

fn parse_date(s: &str, t: &PlaidTxn) -> Result<Date, String> {
    Date::parse(s).ok_or_else(|| format!("transaction {} has invalid date {s:?}", t.transaction_id))
}

/// Categories that are bills even when seen once.
pub fn is_bill_category(c: &Category) -> bool {
    c.primary == "RENT_AND_UTILITIES"
        || matches!(
            c.detailed.as_str(),
            "LOAN_PAYMENTS_CAR_PAYMENT"
                | "LOAN_PAYMENTS_MORTGAGE_PAYMENT"
                | "LOAN_PAYMENTS_STUDENT_LOAN_PAYMENT"
                | "LOAN_PAYMENTS_PERSONAL_LOAN_PAYMENT"
                | "LOAN_PAYMENTS_CREDIT_CARD_PAYMENT"
                | "GENERAL_SERVICES_INSURANCE"
        )
}

/// The counterparty a record belongs to: its merchant name, or else its
/// description with digits and punctuation dropped (they carry reference
/// numbers that differ per charge).
pub fn counterparty(merchant_name: Option<&str>, description: Option<&str>) -> String {
    if let Some(m) = merchant_name.filter(|m| !m.trim().is_empty()) {
        return m.trim().to_uppercase();
    }
    description
        .unwrap_or("")
        .to_uppercase()
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Deserialize)]
struct FetchMeta {
    fetched_on: String,
}

/// `ephemeris import-plaid`: read the fetcher's raw files, write a feed.
pub fn run(raw: &Path, today: Option<Day>, out: &Path) -> Result<(), String> {
    let sync: SyncFile = read_json(
        &raw.join("transactions_sync.json"),
        "/transactions/sync dump",
    )?;
    let balances: BalanceFile = read_json(
        &raw.join("accounts_balance.json"),
        "/accounts/balance/get dump",
    )?;
    let today = match today {
        Some(d) => Date(d),
        None => {
            let path = raw.join("fetch_meta.json");
            let meta: FetchMeta = read_json(&path, "fetch metadata")?;
            Date::parse(&meta.fetched_on).ok_or_else(|| {
                format!(
                    "{}: invalid fetched_on {:?}",
                    path.display(),
                    meta.fetched_on
                )
            })?
        }
    };
    let (feed, notes) = to_feed(&sync, &balances, today)?;
    write_json(out, &feed)?;

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
    let notes = [
        (
            notes.skipped_accounts,
            format!(
                "non-depository accounts skipped, with their {} transactions",
                notes.skipped_transactions
            ),
        ),
        (
            notes.income_by_name,
            "income records tagged by \"payroll\" in the name, not by category".into(),
        ),
        (
            notes.scheduled_by_repeat,
            format!(
                "records tagged scheduled: same counterparty and amount {RECURRING_MIN}+ times"
            ),
        ),
        (
            notes.scheduled_by_category,
            "records tagged scheduled by a bill-like category".into(),
        ),
        (
            notes.transfers,
            "TRANSFER_IN/OUT records tagged transfer; unmatched legs become one-off flows".into(),
        ),
        (
            notes.missing_authorized_date,
            "posted records without authorized_date (posted date used)".into(),
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        to_feed(&sync, &balances(), Date::parse("2026-10-21").unwrap()).unwrap()
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

    #[test]
    fn transfer_categories_become_transfers() {
        let (feed, notes) = convert(one_page(vec![
            category(
                with(txn("a", 145.09, "2026-10-02"), "name", json!("Venmo")),
                "TRANSFER_OUT",
                "TRANSFER_OUT_TRANSFER_OUT_FROM_APPS",
            ),
            category(
                with(
                    txn("b", -496.0, "2026-10-14"),
                    "name",
                    json!("CASH DEPOSIT"),
                ),
                "TRANSFER_IN",
                "TRANSFER_IN_DEPOSIT",
            ),
            category(
                with(txn("c", 3.0, "2026-10-14"), "name", json!("FEE")),
                "BANK_FEES",
                "BANK_FEES_OTHER_BANK_FEES",
            ),
        ]));
        assert_eq!(notes.transfers, 2);
        assert_eq!(by_desc(&feed, "Venmo").kind, FlowKind::Transfer);
        assert_eq!(by_desc(&feed, "CASH DEPOSIT").kind, FlowKind::Transfer);
        assert_eq!(by_desc(&feed, "FEE").kind, FlowKind::Discretionary);
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

    #[test]
    fn income_and_scheduled_tagging() {
        let pay = |id: &str, date: &str| {
            category(
                with(
                    txn(id, -810.0, date),
                    "name",
                    json!("Sweetgreen inc payroll ppd"),
                ),
                "FOOD_AND_DRINK",
                "FOOD_AND_DRINK_RESTAURANT",
            )
        };
        let sub =
            |id: &str, date: &str| with(txn(id, 12.65, date), "merchant_name", json!("Spotify"));
        let coffee = |id: &str, date: &str, amount: f64| {
            with(txn(id, amount, date), "merchant_name", json!("Starbucks"))
        };
        let wages = category(txn("w", -2000.0, "2026-10-15"), "INCOME", "INCOME_WAGES");
        let rent = category(
            with(txn("r", 1500.0, "2026-10-01"), "name", json!("RENT")),
            "RENT_AND_UTILITIES",
            "RENT_AND_UTILITIES_RENT",
        );
        let (feed, notes) = convert(one_page(vec![
            pay("p1", "2026-10-06"),
            pay("p2", "2026-10-13"),
            sub("s1", "2026-08-21"),
            sub("s2", "2026-09-21"),
            sub("s3", "2026-10-21"),
            coffee("c1", "2026-10-01", 4.5),
            coffee("c2", "2026-10-02", 4.5),
            coffee("c3", "2026-10-03", 5.25),
            wages,
            rent,
        ]));
        let kinds: Vec<(FlowKind, Option<u32>)> = feed
            .transactions
            .iter()
            .map(|t| (t.kind, t.stream))
            .collect();
        let kind_of = |desc_or_merchant: &str| -> Vec<FlowKind> {
            feed.transactions
                .iter()
                .filter(|t| {
                    t.merchant_name.as_deref() == Some(desc_or_merchant)
                        || t.description.as_deref() == Some(desc_or_merchant)
                })
                .map(|t| t.kind)
                .collect()
        };
        assert_eq!(kind_of("Sweetgreen inc payroll ppd"), [FlowKind::Income; 2]);
        assert_eq!(kind_of("Spotify"), [FlowKind::Scheduled; 3]);
        // Two equal charges are not enough.
        assert_eq!(kind_of("Starbucks"), [FlowKind::Discretionary; 3]);
        assert_eq!(kind_of("RENT"), [FlowKind::Scheduled]);
        assert_eq!(
            (
                notes.income_by_name,
                notes.scheduled_by_repeat,
                notes.scheduled_by_category
            ),
            (2, 3, 1)
        );
        // Streams: one per tagged counterparty, none for discretionary.
        let spotify: BTreeSet<Option<u32>> = feed
            .transactions
            .iter()
            .filter(|t| t.merchant_name.as_deref() == Some("Spotify"))
            .map(|t| t.stream)
            .collect();
        assert_eq!(spotify.len(), 1);
        assert!(spotify.iter().all(Option::is_some));
        for (kind, stream) in kinds {
            assert_eq!(
                stream.is_some(),
                matches!(kind, FlowKind::Income | FlowKind::Scheduled)
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
