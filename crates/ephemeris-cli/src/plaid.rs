//! `ephemeris import-plaid`: read the fetch script's files, write a feed.
//! The parsing is the `ephemeris-plaid` crate, shared with the desktop app.

use std::path::Path;

use ephemeris_core::schema::{Date, Feed};
use ephemeris_core::{Day, FlowKind, TxnStatus};
pub use ephemeris_plaid::*;
use ephemeris_sync::tag::{Notes, RECURRING_MIN, TRANSFER_WINDOW};

use crate::files::{read_json, write_json};

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
