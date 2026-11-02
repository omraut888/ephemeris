//! Fetches in, feeds out (design §1.3, §4.2).
//!
//! [`SyncState::ingest`] records one fetch from the bank under a single
//! stamp, so a fetch's records and balances always travel together.
//! [`SyncState::materialize`] rebuilds the schema-v1 feed from whatever the
//! state holds, the same way on every device.

use std::collections::BTreeSet;

use ephemeris_core::FlowKind;
use ephemeris_core::schema::{Date, Feed};
use serde::{Deserialize, Serialize};

use crate::bank::{AccountSnapshot, BankRecord};
use crate::hlc::{Hlc, ReplicaId};
use crate::lww::Lww;
use crate::state::{Fetched, RecordVersion, SyncState};
use crate::tag::{self, Notes};

/// One fetch from the bank.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchBatch {
    /// The day the fetch describes (the end of it).
    pub today: Date,
    /// The first day the fetch covers. Records before it are not implied
    /// absent by this fetch.
    pub window_start: Date,
    /// Records added or changed (or, when `complete`, every current record).
    pub records: Vec<BankRecord>,
    /// Bank ids the bank reported removed.
    pub removed: Vec<String>,
    /// Every account, as of this fetch.
    pub accounts: Vec<AccountSnapshot>,
    /// `records` is the bank's whole current set for `accounts` over
    /// `window_start..=today`, so a live record there that is missing from
    /// it is gone (a from-scratch sync rather than a delta).
    pub complete: bool,
}

impl FetchBatch {
    fn validate(&self) -> Result<(), String> {
        if self.window_start > self.today {
            return Err(format!(
                "fetch window starts {} after its today {}",
                self.window_start, self.today
            ));
        }
        let mut ids = BTreeSet::new();
        for r in &self.records {
            if r.id.is_empty() {
                return Err("a fetched record has an empty id".into());
            }
            if !r.amount.is_finite() {
                return Err(format!("record {} has a non-finite amount", r.id));
            }
            if !ids.insert(r.id.as_str()) {
                return Err(format!("record {} appears twice in one fetch", r.id));
            }
        }
        if let Some(id) = self.removed.iter().find(|id| ids.contains(id.as_str())) {
            return Err(format!("record {id} is both fetched and removed"));
        }
        for a in &self.accounts {
            if a.current.is_some_and(|c| !c.is_finite()) {
                return Err(format!("account {} has a non-finite balance", a.id));
            }
        }
        Ok(())
    }
}

impl SyncState {
    /// Record a fetch made by `replica` at `now_ms`. Returns its stamp.
    pub fn ingest(
        &mut self,
        batch: &FetchBatch,
        replica: ReplicaId,
        now_ms: u64,
    ) -> Result<Hlc, String> {
        batch.validate()?;
        if let Some(f) = &self.fetcher
            && f.value != replica
        {
            return Err(format!(
                "bank data for this sync state is fetched by device {:016x}, not this one \
                 ({replica:016x}). Only one device may fetch, or an older bank state could \
                 win (docs/phase4b-design.md §1.4). Hand the fetcher role to this device first.",
                f.value
            ));
        }
        let at = self.stamp(replica, now_ms);
        if self.fetcher.is_none() {
            self.fetcher = Some(Lww::new(at, replica));
        }

        if batch.complete {
            let fetched: BTreeSet<&str> = batch.records.iter().map(|r| r.id.as_str()).collect();
            let accounts: BTreeSet<&str> = batch.accounts.iter().map(|a| a.id.as_str()).collect();
            let window = batch.window_start.0..=batch.today.0;
            let gone: Vec<String> = self
                .records
                .iter()
                .filter_map(|(id, v)| match &v.value {
                    RecordVersion::Live(r)
                        if accounts.contains(r.account.as_str())
                            && window.contains(&r.day())
                            && !fetched.contains(id.as_str()) =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .collect();
            for id in gone {
                self.records
                    .insert(id, Lww::new(at, RecordVersion::Removed));
            }
        }
        for r in &batch.records {
            self.records
                .insert(r.id.clone(), Lww::new(at, RecordVersion::Live(r.clone())));
        }
        for id in &batch.removed {
            self.records
                .insert(id.clone(), Lww::new(at, RecordVersion::Removed));
        }
        for a in &batch.accounts {
            self.accounts.insert(a.id.clone(), Lww::new(at, a.clone()));
        }
        self.fetched = self.fetched.max(Some(Fetched {
            today: batch.today,
            at,
        }));
        self.coverage.insert(batch.window_start.0, batch.today.0);
        Ok(at)
    }

    /// The live records the feed is built from: not removed, and inside the
    /// unbroken run of fetched history that ends at the latest `today`.
    pub fn live_records(&self) -> Result<(Date, Vec<&BankRecord>), String> {
        let today = self
            .fetched
            .ok_or("nothing has been fetched into this sync state yet")?
            .today;
        let start = self
            .coverage
            .run_containing(today.0)
            .ok_or("the latest fetch's day is not in the fetched history")?;
        let records = self
            .records
            .values()
            .filter_map(|v| match &v.value {
                RecordVersion::Live(r) if r.day() >= start => Some(r),
                _ => None,
            })
            .collect();
        Ok((today, records))
    }

    /// The schema-v1 feed this state describes, the user's overrides and
    /// rules applied (design §4.2).
    pub fn materialize(&self) -> Result<(Feed, Notes), String> {
        self.materialize_with(|r, partner, tagged| self.effective_kind(r, partner, tagged))
    }

    /// [`Self::materialize`] with a different kind hook (see
    /// [`tag::to_feed_with`]); the CLI uses it to show why a record has its kind.
    pub fn materialize_with(
        &self,
        edit: impl Fn(&BankRecord, Option<&BankRecord>, FlowKind) -> FlowKind,
    ) -> Result<(Feed, Notes), String> {
        let (today, records) = self.live_records()?;
        let records: Vec<BankRecord> = records.into_iter().cloned().collect();
        let accounts: Vec<AccountSnapshot> =
            self.accounts.values().map(|v| v.value.clone()).collect();
        tag::to_feed_with(&records, &accounts, today, edit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::record;
    use ephemeris_core::TxnStatus;

    const ME: ReplicaId = 7;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn rec(id: &str, amount: f64, date: &str) -> BankRecord {
        BankRecord {
            date: d(date),
            ..record(id, amount)
        }
    }

    fn pending(id: &str, amount: f64, date: &str) -> BankRecord {
        BankRecord {
            pending: true,
            ..rec(id, amount, date)
        }
    }

    fn account(balance: f64) -> AccountSnapshot {
        AccountSnapshot {
            id: "chk".into(),
            depository: true,
            current: Some(balance),
            currency: None,
        }
    }

    fn batch(today: &str, records: Vec<BankRecord>, removed: &[&str]) -> FetchBatch {
        FetchBatch {
            today: d(today),
            window_start: d("2026-10-01"),
            records,
            removed: removed.iter().map(|s| s.to_string()).collect(),
            accounts: vec![account(100.0)],
            complete: false,
        }
    }

    fn amounts(s: &SyncState) -> Vec<(f64, TxnStatus)> {
        let (feed, _) = s.materialize().unwrap();
        feed.transactions
            .iter()
            .map(|t| (t.amount, t.status))
            .collect()
    }

    #[test]
    fn pending_then_posted_then_removed() {
        let mut s = SyncState::new();
        s.ingest(
            &batch("2026-10-05", vec![pending("p", -20.0, "2026-10-04")], &[]),
            ME,
            1,
        )
        .unwrap();
        assert_eq!(amounts(&s), vec![(-20.0, TxnStatus::Pending)]);

        // The tip lands: the bank modifies the pending amount.
        s.ingest(
            &batch("2026-10-05", vec![pending("p", -24.0, "2026-10-04")], &[]),
            ME,
            2,
        )
        .unwrap();
        assert_eq!(amounts(&s), vec![(-24.0, TxnStatus::Pending)]);

        // It posts under a new id that names the pending one.
        let mut posted = rec("q", -24.0, "2026-10-06");
        posted.pending_id = Some("p".into());
        s.ingest(&batch("2026-10-06", vec![posted], &["p"]), ME, 3)
            .unwrap();
        assert_eq!(amounts(&s), vec![(-24.0, TxnStatus::Posted)]);
    }

    #[test]
    fn a_posted_record_hides_its_pending_even_before_the_removal_arrives() {
        let mut s = SyncState::new();
        let mut posted = rec("q", -5.0, "2026-10-03");
        posted.pending_id = Some("p".into());
        s.ingest(
            &batch(
                "2026-10-03",
                vec![pending("p", -5.0, "2026-10-02"), posted],
                &[],
            ),
            ME,
            1,
        )
        .unwrap();
        assert_eq!(amounts(&s), vec![(-5.0, TxnStatus::Posted)]);
    }

    #[test]
    fn a_stale_replica_cannot_resurrect_a_cancelled_charge() {
        let mut fetcher = SyncState::new();
        fetcher
            .ingest(
                &batch("2026-10-05", vec![pending("p", -9.0, "2026-10-05")], &[]),
                ME,
                10,
            )
            .unwrap();
        let stale = fetcher.clone(); // another device got this copy
        fetcher
            .ingest(&batch("2026-10-06", vec![], &["p"]), ME, 20)
            .unwrap();
        let mut phone = stale;
        phone.merge(&fetcher).unwrap();
        fetcher.merge(&phone).unwrap();
        assert!(amounts(&phone).is_empty());
        assert_eq!(phone.to_json(), fetcher.to_json());
    }

    #[test]
    fn a_complete_fetch_removes_what_it_no_longer_shows_inside_its_window() {
        let mut s = SyncState::new();
        s.ingest(
            &batch(
                "2026-10-05",
                vec![
                    rec("old", -1.0, "2026-09-20"),
                    pending("p", -2.0, "2026-10-04"),
                    rec("a", -3.0, "2026-10-02"),
                ],
                &[],
            ),
            ME,
            1,
        )
        .unwrap();
        s.coverage.insert(d("2026-09-01").0, d("2026-10-01").0);
        let mut full = batch("2026-10-06", vec![rec("a", -3.0, "2026-10-02")], &[]);
        full.complete = true;
        s.ingest(&full, ME, 2).unwrap();
        let ids: Vec<_> = s
            .records
            .iter()
            .map(|(id, v)| (id.as_str(), matches!(v.value, RecordVersion::Live(_))))
            .collect();
        // "old" is before the window, so this fetch says nothing about it.
        assert_eq!(ids, vec![("a", true), ("old", true), ("p", false)]);
    }

    #[test]
    fn history_starts_at_the_unbroken_run_ending_today() {
        let mut s = SyncState::new();
        let mut b = batch("2026-10-10", vec![rec("new", -1.0, "2026-10-08")], &[]);
        b.window_start = d("2026-10-05");
        s.ingest(&b, ME, 1).unwrap();
        // An older fetch that covered 2026-09-01..=2026-09-30: a gap remains.
        let mut old = batch("2026-09-30", vec![rec("gap", -2.0, "2026-09-15")], &[]);
        old.window_start = d("2026-09-01");
        s.ingest(&old, ME, 2).unwrap();
        let (feed, _) = s.materialize().unwrap();
        assert_eq!(feed.today, d("2026-10-10"));
        assert_eq!(feed.transactions.len(), 1);
        // Filling the gap joins the runs.
        let mut fill = batch("2026-10-10", vec![], &[]);
        fill.window_start = d("2026-10-01");
        s.ingest(&fill, ME, 3).unwrap();
        let (feed, _) = s.materialize().unwrap();
        assert_eq!(feed.transactions.len(), 2);
        assert_eq!(feed.history_start, d("2026-09-15"));
    }

    #[test]
    fn bad_batches_are_refused_and_change_nothing() {
        let mut s = SyncState::new();
        let mut b = batch("2026-10-05", vec![rec("a", 1.0, "2026-10-02")], &["a"]);
        assert!(s.ingest(&b, ME, 1).is_err());
        b.removed.clear();
        b.records.push(rec("a", 2.0, "2026-10-02"));
        assert!(s.ingest(&b, ME, 1).is_err());
        b.records.pop();
        b.window_start = d("2026-10-09");
        assert!(s.ingest(&b, ME, 1).is_err());
        assert_eq!(s, SyncState::new());
        assert!(s.materialize().is_err());
    }

    const OTHER: ReplicaId = 9;

    #[test]
    fn the_first_fetch_claims_the_fetcher_role_and_others_are_refused() {
        let mut mac = SyncState::new();
        mac.ingest(&batch("2026-10-05", vec![], &[]), ME, 1)
            .unwrap();
        assert_eq!(mac.fetcher.as_ref().map(|f| f.value), Some(ME));

        let mut laptop = SyncState::new();
        laptop.merge(&mac).unwrap();
        let before = laptop.clone();
        let err = laptop
            .ingest(
                &batch("2026-10-06", vec![rec("a", -1.0, "2026-10-06")], &[]),
                OTHER,
                2,
            )
            .unwrap_err();
        assert!(
            err.contains("fetched by device 0000000000000007")
                && err.contains("not this one (0000000000000009)")
                && err.contains("Hand the fetcher role"),
            "{err}"
        );
        assert_eq!(laptop, before, "a refused fetch changes nothing");
        // The fetcher itself keeps fetching.
        mac.ingest(&batch("2026-10-06", vec![], &[]), ME, 3)
            .unwrap();
    }

    #[test]
    fn the_role_can_be_handed_over_and_the_old_fetcher_is_then_refused() {
        use crate::edits::Edit;
        let mut old = SyncState::new();
        old.ingest(&batch("2026-10-05", vec![], &[]), ME, 1)
            .unwrap();
        let mut new = old.clone();
        new.edit(&Edit::TakeFetcher, OTHER, 2).unwrap();
        new.ingest(&batch("2026-10-06", vec![], &[]), OTHER, 3)
            .unwrap();
        old.merge(&new).unwrap();
        assert!(
            old.ingest(&batch("2026-10-07", vec![], &[]), ME, 4)
                .is_err()
        );
        assert_eq!(old.fetcher.as_ref().map(|f| f.value), Some(OTHER));
    }

    #[test]
    fn concurrent_first_claims_settle_on_one_fetcher_everywhere() {
        let (mut a, mut b) = (SyncState::new(), SyncState::new());
        a.ingest(&batch("2026-10-05", vec![], &[]), ME, 1_000)
            .unwrap();
        b.ingest(&batch("2026-10-05", vec![], &[]), OTHER, 2_000)
            .unwrap();
        let a0 = a.clone();
        a.merge(&b).unwrap();
        b.merge(&a0).unwrap();
        assert_eq!(a.to_json(), b.to_json());
        // The later claim (larger stamp) holds the role; the other is refused.
        assert_eq!(a.fetcher.as_ref().map(|f| f.value), Some(OTHER));
        assert!(
            a.ingest(&batch("2026-10-06", vec![], &[]), ME, 3_000)
                .is_err()
        );
        b.ingest(&batch("2026-10-06", vec![], &[]), OTHER, 3_000)
            .unwrap();
    }

    /// The limit design §1.3 states, pinned down: during a handover the old
    /// fetcher, not yet aware of it and with a clock a day fast, fetches
    /// an older bank state with a larger stamp than the new fetcher's
    /// later delta. The stale version wins until a complete fetch.
    #[test]
    fn a_handover_window_can_leave_a_stale_version_until_a_complete_fetch() {
        use crate::edits::Edit;
        const DAY: u64 = 86_400_000;
        let (old_id, new_id) = (ME, OTHER);
        let mut old = SyncState::new();
        let p = |amount| pending("p", amount, "2026-10-04");
        old.ingest(
            &batch("2026-10-04", vec![p(-10.0)], &[]),
            old_id,
            DAY + 1_000,
        )
        .unwrap();
        let mut new = old.clone();
        new.edit(&Edit::TakeFetcher, new_id, 2_000).unwrap();
        // True order: the old Mac sees the tip (t = 3 s), then the bank
        // cancels the charge and the new Mac sees that (t = 4 s).
        old.ingest(
            &batch("2026-10-05", vec![p(-12.0)], &[]),
            old_id,
            DAY + 3_000,
        )
        .unwrap();
        new.ingest(&batch("2026-10-05", vec![], &["p"]), new_id, 4_000)
            .unwrap();
        new.merge(&old).unwrap();
        assert_eq!(amounts(&new), vec![(-12.0, TxnStatus::Pending)], "stale");
        // The old Mac is refused from now on, and a complete fetch by the
        // new one settles the bank side.
        old.merge(&new).unwrap();
        assert!(
            old.ingest(&batch("2026-10-05", vec![], &[]), old_id, DAY + 5_000)
                .is_err()
        );
        let mut full = batch("2026-10-05", vec![], &[]);
        full.complete = true;
        new.ingest(&full, new_id, 5_000).unwrap();
        assert!(amounts(&new).is_empty());
    }

    #[test]
    fn states_written_before_the_fetcher_field_still_load() {
        let mut s = SyncState::new();
        s.ingest(&batch("2026-10-05", vec![], &[]), ME, 1).unwrap();
        s.fetcher = None;
        let old = s.to_json().replace(",\"fetcher\":null", "");
        assert!(!old.contains("fetcher"));
        assert_eq!(SyncState::from_json(&old).unwrap(), s);
    }
}
