//! Bank records as the bank reported them, keyed by the bank's own stable
//! ids (design §1.2). A shell's adapter (the CLI's Plaid importer) builds
//! these; [`crate::tag`] turns a set of them into a schema-v1 feed.

use ephemeris_core::Day;
use ephemeris_core::schema::Date;

/// The bank's category for a record, in Plaid's `personal_finance_category`
/// vocabulary. The bill and transfer rules read it (design §4.2: the one
/// Plaid notion in this crate; another source would map into it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub primary: String,
    pub detailed: String,
}

/// One transaction record, pending or posted.
///
/// `amount` is signed with **inflows positive** (the adapter flips Plaid's
/// sign) and is kept exactly as reported, not rounded: the feed rounds to
/// cents when it is built, as the CLI always has.
#[derive(Debug, Clone, PartialEq)]
pub struct BankRecord {
    /// The bank's id (Plaid `transaction_id`). A pending record and the
    /// posted record that replaces it have different ids.
    pub id: String,
    /// The bank's account id (Plaid `account_id`).
    pub account: String,
    /// Posted date, or authorization date while pending (Plaid `date`).
    pub date: Date,
    pub authorized_date: Option<Date>,
    pub pending: bool,
    /// On a posted record, the pending record it replaces.
    pub pending_id: Option<String>,
    pub amount: f64,
    pub currency: Option<String>,
    pub merchant_name: Option<String>,
    /// The bank's description line (Plaid `name`).
    pub name: Option<String>,
    pub category: Option<Category>,
}

impl BankRecord {
    /// The day the record belongs to: authorized, else posted (phase1 §0).
    pub fn day(&self) -> Day {
        self.authorized_date.unwrap_or(self.date).0
    }
}

/// One account as of a fetch.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountSnapshot {
    /// The bank's account id.
    pub id: String,
    /// Checking or savings. Only these enter the feed (phase1 §0).
    pub depository: bool,
    /// Posted balance (pending excluded), as reported.
    pub current: Option<f64>,
    pub currency: Option<String>,
}
