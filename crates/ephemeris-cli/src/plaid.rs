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
