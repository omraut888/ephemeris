//! Domain types shared by ingestion, the filter, and the synthetic generator.
//!
//! Conventions (docs/phase1-design.md §0):
//! - [`Day`] counts days since 1970-01-01 in the account's local calendar.
//! - Amounts are signed account-currency `f64`, **inflows positive**.

use crate::error::EngineError;

/// Days since 1970-01-01 (local calendar). Dates and time zones are the caller's job.
pub type Day = i32;

/// Identifies a single transaction record (pending and posted records are distinct).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TxnId(pub u64);

/// Identifies one of the user's linked accounts (Plaid `account_id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(pub u32);

/// Identifies a recurring stream (one employer, one subscription).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamId(pub u32);

/// How a flow enters the model (design §1).
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum FlowKind {
    /// Primary income: control input `u` for `b`, and an observation of `m`.
    Income,
    /// Recurring bills and subscriptions: control input `u` only.
    Scheduled,
    /// Everyday spending: seen only through the balance observation.
    Discretionary,
    /// Movement between the user's own linked accounts. Never spending or
    /// income: excluded from `u`, `c` and `m`, and the two legs cancel in the
    /// aggregated balance (design §0).
    Transfer,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum TxnStatus {
    /// Authorized but not settled; the amount may still change.
    Pending,
    /// Settled at its final amount.
    Posted,
}

/// One transaction record as a bank feed reports it, mirroring Plaid's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Transaction {
    pub id: TxnId,
    pub account: AccountId,
    /// Posted date for posted records, authorization date for pending ones (Plaid `date`).
    pub date: Day,
    /// When the purchase happened. Banks sometimes omit it on posted records.
    pub authorized_date: Option<Day>,
    pub status: TxnStatus,
    /// Signed, inflows positive.
    pub amount: f64,
    pub kind: FlowKind,
    pub stream: Option<StreamId>,
    /// On a posted record, the pending record it replaces (Plaid `pending_transaction_id`).
    pub pending_id: Option<TxnId>,
}

impl Transaction {
    /// The day this transaction belongs to: its authorization date, falling back
    /// to the posted date (design §0).
    pub fn day(&self) -> Day {
        self.authorized_date.unwrap_or(self.date)
    }

    pub fn validate(&self) -> Result<(), EngineError> {
        if !self.amount.is_finite() {
            return Err(EngineError::InvalidInput(
                "transaction amount is not finite",
            ));
        }
        if self.authorized_date.is_some_and(|a| a > self.date) {
            return Err(EngineError::InvalidInput(
                "transaction authorized after its date",
            ));
        }
        if self.status == TxnStatus::Pending && self.pending_id.is_some() {
            return Err(EngineError::InvalidInput(
                "pending transaction cannot replace another pending record",
            ));
        }
        Ok(())
    }
}

/// Which scalar measurement an [`Observation`] is (design §1, observations table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// End-of-day balance on the authorized basis; `H = [1 0 0]`.
    Balance,
    /// Amount of a posted primary-income transaction; `H = [0 0 1]`.
    Income,
}

/// One scalar measurement `z` with noise variance `r`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    pub channel: Channel,
    pub z: f64,
    pub r: f64,
}

impl Observation {
    pub fn validate(&self) -> Result<(), EngineError> {
        if !self.z.is_finite() {
            return Err(EngineError::InvalidInput("observation value is not finite"));
        }
        if !(self.r.is_finite() && self.r > 0.0) {
            return Err(EngineError::InvalidInput(
                "observation variance must be positive and finite",
            ));
        }
        Ok(())
    }
}

/// Everything the filter consumes for one day. No observations means predict-only.
#[derive(Debug, Clone, PartialEq)]
pub struct DayInput {
    pub day: Day,
    /// Sum of realized scheduled flows (income and bills) on this day.
    pub u: f64,
    /// Processed in order as sequential scalar updates.
    pub observations: Vec<Observation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posted(amount: f64) -> Transaction {
        Transaction {
            id: TxnId(1),
            account: AccountId(0),
            date: 10,
            authorized_date: Some(8),
            status: TxnStatus::Posted,
            amount,
            kind: FlowKind::Discretionary,
            stream: None,
            pending_id: Some(TxnId(0)),
        }
    }

    #[test]
    fn day_prefers_authorized_date() {
        let mut t = posted(-5.0);
        assert_eq!(t.day(), 8);
        t.authorized_date = None;
        assert_eq!(t.day(), 10);
    }

    #[test]
    fn transaction_validation() {
        assert_eq!(posted(-5.0).validate(), Ok(()));
        assert!(posted(f64::NAN).validate().is_err());
        assert!(posted(f64::INFINITY).validate().is_err());

        let mut late_auth = posted(-5.0);
        late_auth.authorized_date = Some(11);
        assert!(late_auth.validate().is_err());

        let mut pending = posted(-5.0);
        pending.status = TxnStatus::Pending;
        assert!(pending.validate().is_err());
        pending.pending_id = None;
        assert_eq!(pending.validate(), Ok(()));
    }

    #[test]
    fn observation_validation() {
        let ok = Observation {
            channel: Channel::Balance,
            z: 100.0,
            r: 1.0,
        };
        assert_eq!(ok.validate(), Ok(()));
        assert!(Observation { r: 0.0, ..ok }.validate().is_err());
        assert!(Observation { r: -1.0, ..ok }.validate().is_err());
        assert!(Observation { r: f64::NAN, ..ok }.validate().is_err());
        assert!(Observation { z: f64::NAN, ..ok }.validate().is_err());
    }
}
