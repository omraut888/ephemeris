//! Bank records → schema-v1 [`Feed`]: the tagging that used to live in the
//! CLI's Plaid adapter, moved here so every device builds the same feed
//! from the same records (docs/phase4b-design.md §4.2).
//!
//! In order:
//! 1. Keep depository accounts only (phase1 design §0). The ledger balance
//!    is the sum of their posted balances.
//! 2. Drop a pending record when a posted record names it in `pending_id`.
//! 3. Number the records: integer ids in bank-id order, over present and
//!    referenced records.
//! 4. Tag each record (phase1 design §1 makes tagging the adapter's job):
//!    - a `TRANSFER_IN` / `TRANSFER_OUT` record → transfer, but only when
//!      its counterpart leg is in another depository account: the opposite
//!      amount to the cent, within [`TRANSFER_WINDOW`] business days. A leg
//!      still inside that window of today stays a transfer if another
//!      account could show its counterpart later (the core holds it). Any
//!      other transfer-like record (Venmo, Cash App, PayPal, cash) is money
//!      leaving or entering, and is tagged by the rules below like any other;
//!    - income: the inflows of the *primary* regular income stream. Each
//!      counterparty's posted inflows are tested by the core's schedule
//!      detector ([`regular_pay_schedule`]); of the regular ones, the largest
//!      by yearly amount wins. The payee's name and category only break a
//!      near-tie (within [`INCOME_TIE`]): a wage category or a payroll-like
//!      name wins it;
//!    - scheduled: at least [`RECURRING_MIN`] posted records with the same
//!      counterparty and amount *on a regular interval* (the core's
//!      [`is_regular_interval`]: at least weekly, gaps clustered around their
//!      median), or a bill-like category ([`is_bill_category`]). Repeats
//!      alone would make a habitual coffee a bill;
//!    - anything else → discretionary.
//!
//!    Income and scheduled records get a stream per counterparty, numbered
//!    in name order.
//! 5. Sort by day, then posted date, then id, and round amounts to cents.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ephemeris_core::calendar::{PaySchedule, add_business_days};
use ephemeris_core::observe::ObserveConfig;
use ephemeris_core::schedule::{is_regular_interval, regular_pay_schedule};
use ephemeris_core::schema::{Date, Feed, FeedTransaction, SCHEMA_VERSION};
use ephemeris_core::{Day, FlowKind, TxnStatus};

use crate::bank::{AccountSnapshot, BankRecord, Category};

/// Same-amount repeats that make a counterparty a scheduled stream.
pub const RECURRING_MIN: usize = 3;
/// Two regular income streams within this fraction of each other's yearly
/// amount are a tie, broken by the payee's name or category.
pub const INCOME_TIE: f64 = 0.1;
/// Business days within which two transfer legs match: the core's window,
/// so a leg kept as in flight here is one the core will hold.
pub const TRANSFER_WINDOW: u32 = ObserveConfig::DEFAULT_TRANSFER_WINDOW;

/// What the tagging decided that the data did not state outright.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Notes {
    pub skipped_accounts: usize,
    pub skipped_transactions: usize,
    pub superseded_pending: usize,
    pub missing_authorized_date: usize,
    /// Legs of matched own-account transfer pairs.
    pub transfers: usize,
    /// The primary income stream, as `counterparty (cadence)`.
    pub income_stream: Option<String>,
    /// The income stream won a near-tie on its name or category.
    pub income_by_tiebreak: bool,
    /// Unmatched transfer legs young enough that the counterpart may still post.
    pub transfers_in_flight: usize,
    /// `TRANSFER_*` records with no counterpart, tagged by the other rules.
    pub unmatched_transfers: usize,
    pub scheduled_by_repeat: usize,
    /// Records with enough same-amount repeats but irregular timing.
    pub irregular_repeats: usize,
    pub scheduled_by_category: usize,
}

/// The feed for `records` and `accounts` as of the end of `today`.
///
/// `records` are the bank's current records, each id at most once.
pub fn to_feed(
    records: &[BankRecord],
    accounts: &[AccountSnapshot],
    today: Date,
) -> Result<(Feed, Notes), String> {
    let mut notes = Notes::default();

    let mut accounts: Vec<&AccountSnapshot> = {
        let all = accounts.len();
        let kept: Vec<_> = accounts.iter().filter(|a| a.depository).collect();
        notes.skipped_accounts = all - kept.len();
        kept
    };
    if accounts.is_empty() {
        return Err("no depository (checking or savings) account in the balances".into());
    }
    accounts.sort_by(|a, b| a.id.cmp(&b.id));
    let account_index: HashMap<&str, u32> = accounts
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id.as_str(), i as u32))
        .collect();
    let currencies: BTreeSet<&str> = accounts
        .iter()
        .filter_map(|a| a.currency.as_deref())
        .collect();
    if currencies.len() > 1 {
        return Err(format!(
            "depository accounts mix currencies: {currencies:?}"
        ));
    }
    let mut ledger = 0.0;
    for a in &accounts {
        ledger += a
            .current
            .ok_or_else(|| format!("account {} has no current balance", a.id))?;
    }

    let mut current: BTreeMap<&str, &BankRecord> = BTreeMap::new();
    for r in records {
        if current.insert(&r.id, r).is_some() {
            return Err(format!("transaction {} appears twice", r.id));
        }
    }

    // 1. Depository records only.
    let before = current.len();
    current.retain(|_, t| account_index.contains_key(t.account.as_str()));
    notes.skipped_transactions = before - current.len();

    // 2. A posted record replaces the pending record it names.
    let replaced: BTreeSet<&str> = current
        .values()
        .filter(|t| !t.pending)
        .filter_map(|t| t.pending_id.as_deref())
        .collect();
    let before = current.len();
    current.retain(|id, t| !(t.pending && replaced.contains(id)));
    notes.superseded_pending = before - current.len();

    // 3. Integer ids, in bank-id order over present and referenced records.
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

    let days: HashMap<&str, Day> = current.values().map(|t| (t.id.as_str(), t.day())).collect();

    // 4. Tags.
    let transfers = match_transfers(&current, &days, today.0, accounts.len(), &mut notes);
    let key = |t: &BankRecord| counterparty(t.merchant_name.as_deref(), t.name.as_deref());
    let income = primary_income(
        current
            .values()
            .copied()
            .filter(|t| !transfers.contains(t.id.as_str())),
        &days,
        &mut notes,
    );

    // Days of same-amount repeats per counterparty, over posted records.
    let mut repeats: HashMap<(String, i64), Vec<Day>> = HashMap::new();
    for t in current
        .values()
        .filter(|t| !t.pending && !transfers.contains(t.id.as_str()))
    {
        repeats
            .entry((key(t), cents_key(t.amount)))
            .or_default()
            .push(days[t.id.as_str()]);
    }

    let mut out = Vec::with_capacity(current.len());
    let mut stream_keys = Vec::with_capacity(current.len());
    for t in current.values() {
        if let Some(code) = &t.currency
            && !currencies.is_empty()
            && !currencies.contains(code.as_str())
        {
            return Err(format!("transaction {} is in {code}", t.id));
        }
        if t.authorized_date.is_none() && !t.pending {
            notes.missing_authorized_date += 1;
        }
        let k = key(t);
        let same = repeats
            .get(&(k.clone(), cents_key(t.amount)))
            .map_or(&[][..], Vec::as_slice);
        let kind = if transfers.contains(t.id.as_str()) {
            FlowKind::Transfer
        } else if t.amount > 0.0 && income.as_ref() == Some(&k) {
            FlowKind::Income
        } else {
            classify(t, same, &mut notes)
        };
        stream_keys.push(matches!(kind, FlowKind::Income | FlowKind::Scheduled).then_some(k));
        out.push(FeedTransaction {
            id: ids[t.id.as_str()],
            account: account_index[t.account.as_str()],
            date: t.date,
            authorized_date: t.authorized_date,
            status: if t.pending {
                TxnStatus::Pending
            } else {
                TxnStatus::Posted
            },
            amount: t.amount,
            kind,
            stream: None,
            pending_id: t
                .pending_id
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

    // 5. Order and cents.
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

fn is_transfer_category(t: &BankRecord) -> bool {
    t.category
        .as_ref()
        .is_some_and(|c| c.primary == "TRANSFER_IN" || c.primary == "TRANSFER_OUT")
}

fn within_business_days(a: Day, b: Day, window: u32) -> bool {
    add_business_days(a.min(b), window) >= a.max(b)
}

/// Ids of the records that are own-account transfers (module docs, step 4):
/// matched pairs, plus unmatched legs still in flight when another account
/// could show their counterpart.
fn match_transfers<'a>(
    current: &BTreeMap<&'a str, &'a BankRecord>,
    days: &HashMap<&str, Day>,
    today: Day,
    n_accounts: usize,
    notes: &mut Notes,
) -> BTreeSet<&'a str> {
    let mut legs: Vec<&BankRecord> = current
        .values()
        .copied()
        .filter(|t| is_transfer_category(t))
        .collect();
    let day = |t: &BankRecord| days[t.id.as_str()];
    legs.sort_by(|a, b| (day(a), &a.id).cmp(&(day(b), &b.id)));

    let mut tagged = BTreeSet::new();
    for out in legs.iter().filter(|t| t.amount < 0.0) {
        if tagged.contains(out.id.as_str()) {
            continue;
        }
        let partner = legs.iter().find(|inc| {
            inc.amount > 0.0
                && inc.account != out.account
                && !tagged.contains(inc.id.as_str())
                && cents_key(inc.amount) == -cents_key(out.amount)
                && within_business_days(day(out), day(inc), TRANSFER_WINDOW)
        });
        if let Some(inc) = partner {
            tagged.insert(out.id.as_str());
            tagged.insert(inc.id.as_str());
            notes.transfers += 2;
        }
    }
    for t in &legs {
        let id = t.id.as_str();
        if tagged.contains(id) {
            continue;
        }
        if n_accounts > 1 && within_business_days(day(t), today, TRANSFER_WINDOW) {
            tagged.insert(id);
            notes.transfers_in_flight += 1;
        } else {
            notes.unmatched_transfers += 1;
        }
    }
    tagged
}

/// Amount in whole cents, for grouping equal charges. Rounds half away from
/// zero, so `cents_key(-x) == -cents_key(x)`.
fn cents_key(amount: f64) -> i64 {
    (amount * 100.0).round() as i64
}

/// The flow kind of a record that is neither a transfer nor income (see the
/// module docs). `repeats` are the days of the posted records that share its
/// counterparty and amount.
fn classify(t: &BankRecord, repeats: &[Day], notes: &mut Notes) -> FlowKind {
    let cat = t.category.as_ref();
    let repeated = repeats.len() >= RECURRING_MIN;
    if repeated && is_regular_interval(repeats) {
        notes.scheduled_by_repeat += 1;
        FlowKind::Scheduled
    } else if cat.is_some_and(is_bill_category) {
        notes.scheduled_by_category += 1;
        FlowKind::Scheduled
    } else {
        notes.irregular_repeats += usize::from(repeated);
        FlowKind::Discretionary
    }
}

/// Whether a record looks like wages by its category or name. Only a
/// tiebreaker between equally regular, similar-sized income streams.
fn wage_hint(t: &BankRecord) -> bool {
    let category = t
        .category
        .as_ref()
        .is_some_and(|c| c.detailed == "INCOME_WAGES");
    let name = [t.name.as_deref(), t.merchant_name.as_deref()]
        .into_iter()
        .flatten()
        .map(str::to_lowercase)
        .any(|s| {
            ["payroll", "salary", "direct dep"]
                .iter()
                .any(|w| s.contains(w))
        });
    category || name
}

fn paydays_per_year(s: &PaySchedule) -> f64 {
    match s {
        PaySchedule::Weekly { .. } => 52.0,
        PaySchedule::Biweekly { .. } => 26.0,
        PaySchedule::SemiMonthly { .. } => 24.0,
        PaySchedule::Monthly { .. } => 12.0,
    }
}

/// The counterparty of the primary income stream (module docs, step 4).
fn primary_income<'a>(
    records: impl Iterator<Item = &'a BankRecord>,
    days: &HashMap<&str, Day>,
    notes: &mut Notes,
) -> Option<String> {
    struct Stream {
        days: Vec<Day>,
        total: f64,
        hint: bool,
    }
    let mut streams: BTreeMap<String, Stream> = BTreeMap::new();
    for t in records.filter(|t| !t.pending && t.amount > 0.0) {
        let s = streams
            .entry(counterparty(t.merchant_name.as_deref(), t.name.as_deref()))
            .or_insert(Stream {
                days: Vec::new(),
                total: 0.0,
                hint: false,
            });
        s.days.push(days[t.id.as_str()]);
        s.total += t.amount;
        s.hint |= wage_hint(t);
    }
    // (yearly amount, wage hint, counterparty, cadence) of each regular stream.
    let mut regular: Vec<(f64, bool, String, PaySchedule)> = streams
        .into_iter()
        .filter_map(|(k, s)| {
            let timing = regular_pay_schedule(&s.days)?;
            let mean = s.total / s.days.len() as f64;
            Some((
                mean * paydays_per_year(&timing.schedule),
                s.hint,
                k,
                timing.schedule,
            ))
        })
        .collect();
    regular.sort_by(|a, b| b.0.total_cmp(&a.0));
    let top = regular.first()?.0;
    let leader = regular
        .iter()
        .take_while(|r| r.0 >= (1.0 - INCOME_TIE) * top)
        .find(|r| r.1)
        .unwrap_or(&regular[0]);
    notes.income_by_tiebreak = leader.2 != regular[0].2;
    notes.income_stream = Some(format!("{} ({})", leader.2, cadence_name(&leader.3)));
    Some(leader.2.clone())
}

fn cadence_name(s: &PaySchedule) -> &'static str {
    match s {
        PaySchedule::Weekly { .. } => "weekly",
        PaySchedule::Biweekly { .. } => "biweekly",
        PaySchedule::SemiMonthly { .. } => "semi-monthly",
        PaySchedule::Monthly { .. } => "monthly",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, amount: f64, date: &str) -> BankRecord {
        BankRecord {
            id: id.into(),
            account: "chk".into(),
            date: Date::parse(date).unwrap(),
            authorized_date: None,
            pending: false,
            pending_id: None,
            amount,
            currency: Some("USD".into()),
            merchant_name: None,
            name: Some(id.into()),
            category: None,
        }
    }

    fn account() -> AccountSnapshot {
        AccountSnapshot {
            id: "chk".into(),
            depository: true,
            current: Some(100.0),
            currency: Some("USD".into()),
        }
    }

    fn feed(records: &[BankRecord]) -> Result<Feed, String> {
        to_feed(records, &[account()], Date::parse("2026-10-21").unwrap()).map(|(f, _)| f)
    }

    #[test]
    fn amounts_keep_their_sign_and_round_to_cents_once() {
        let f = feed(&[
            record("a", 48.2542, "2026-10-01"),
            record("b", -0.005, "2026-10-02"),
        ])
        .unwrap();
        assert_eq!(f.transactions[0].amount, 48.25);
        // Half away from zero, then +0.0: never a negative zero.
        assert_eq!(f.transactions[1].amount, -0.01);
        assert_eq!(f.ledger_balance, 100.0);
    }

    #[test]
    fn a_repeated_bank_id_is_an_error() {
        let r = record("a", -1.0, "2026-10-01");
        let err = feed(&[r.clone(), r]).unwrap_err();
        assert!(err.contains("appears twice"), "{err}");
    }

    #[test]
    fn ids_follow_bank_id_order_including_superseded_pendings() {
        let mut pending = record("p", -5.0, "2026-10-01");
        pending.pending = true;
        let mut posted = record("q", -5.5, "2026-10-02");
        posted.pending_id = Some("p".into());
        let f = feed(&[posted, pending, record("a", -1.0, "2026-10-03")]).unwrap();
        // "a" = 1, "p" = 2 (referenced only), "q" = 3.
        let ids: Vec<_> = f
            .transactions
            .iter()
            .map(|t| (t.id, t.pending_id))
            .collect();
        assert_eq!(ids, vec![(3, Some(2)), (1, None)]);
    }

    #[test]
    fn cents_key_is_odd() {
        for x in [0.0, 0.005, 1.234, 48.2542, 1e9 + 0.015] {
            assert_eq!(cents_key(-x), -cents_key(x), "{x}");
        }
    }
}
