//! The replicated state and its merge (design §1.2, §2.4).
//!
//! `SyncState` is a product of join semilattices: maps of LWW registers
//! (joined key by key), a max ([`Fetched`]), a set union ([`DayIntervals`])
//! and the per-replica clock floor (a max per key). A product of
//! semilattices is a semilattice, so [`SyncState::merge`] is commutative,
//! associative and idempotent, and stale or repeated states change nothing.
//!
//! The encoding is canonical: `BTreeMap`s, normal-form intervals and
//! shortest round-trip floats, so equal states are equal bytes.

use std::collections::BTreeMap;

use ephemeris_core::FlowKind;
use ephemeris_core::schema::Date;
use serde::{Deserialize, Serialize};

use crate::bank::{AccountSnapshot, BankRecord};
use crate::hlc::{Hlc, ReplicaId};
use crate::lww::{DayIntervals, Lww};

/// The version of the state encoding this build reads and writes.
pub const STATE_VERSION: u32 = 1;

/// The longest label a saved query may carry, in bytes.
pub const MAX_LABEL: usize = 200;

/// A bank record as last fetched, or the bank's word that it is gone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// Nearly every record is `Live`, so boxing it would add an allocation per
// record and save no memory.
#[allow(clippy::large_enum_variant)]
pub enum RecordVersion {
    Live(BankRecord),
    Removed,
}

/// The most recent fetch: the day it describes, and its stamp.
/// Join is the max of `(today, at)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fetched {
    pub today: Date,
    pub at: Hlc,
}

/// A synced setting (design §1.2). Per-device settings are not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Setting {
    HorizonDays,
    Paths,
    Seed,
}

/// A saved "can I afford it" question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedQuery {
    /// The balance asked about, in cents.
    pub amount_cents: i64,
    pub by: Date,
    pub label: String,
}

/// Everything the devices share.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncState {
    pub version: u32,
    /// The largest stamp each replica has issued.
    pub clocks: BTreeMap<ReplicaId, Hlc>,
    /// Bank records by the bank's id.
    pub records: BTreeMap<String, Lww<RecordVersion>>,
    /// Account snapshots by the bank's account id.
    pub accounts: BTreeMap<String, Lww<AccountSnapshot>>,
    pub fetched: Option<Fetched>,
    /// The days fetched history has covered.
    pub coverage: DayIntervals,
    /// Per-record kind overrides by bank id; `None` clears.
    pub overrides: BTreeMap<String, Lww<Option<FlowKind>>>,
    /// Per-counterparty kind rules by counterparty key; `None` clears.
    pub rules: BTreeMap<String, Lww<Option<FlowKind>>>,
    pub settings: BTreeMap<Setting, Lww<u64>>,
    /// Saved queries by id (`<replica hex>-<n>`); `None` = deleted.
    pub queries: BTreeMap<String, Lww<Option<SavedQuery>>>,
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            clocks: BTreeMap::new(),
            records: BTreeMap::new(),
            accounts: BTreeMap::new(),
            fetched: None,
            coverage: DayIntervals::default(),
            overrides: BTreeMap::new(),
            rules: BTreeMap::new(),
            settings: BTreeMap::new(),
            queries: BTreeMap::new(),
        }
    }
}

fn join_map<K: Ord + Clone, V: Clone + PartialEq>(
    into: &mut BTreeMap<K, Lww<V>>,
    from: &BTreeMap<K, Lww<V>>,
) -> Result<(), String> {
    for (k, v) in from {
        match into.get_mut(k) {
            Some(mine) => mine.join(v)?,
            None => {
                into.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(())
}

impl SyncState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The largest stamp in the state: everything this replica has seen.
    pub fn seen(&self) -> Option<Hlc> {
        self.clocks.values().copied().max()
    }

    /// A fresh stamp for a local write by `replica` at `now_ms` (design
    /// §2.5). It is above every stamp in the state, which is the HLC's
    /// receive rule applied to everything merged so far.
    pub fn stamp(&mut self, replica: ReplicaId, now_ms: u64) -> Hlc {
        let h = Hlc::tick(self.seen(), replica, now_ms);
        self.clocks.insert(replica, h);
        h
    }

    /// `self ⊔ other`. `other` is validated first and the merge is all or
    /// nothing: on any error `self` is unchanged.
    pub fn merge(&mut self, other: &SyncState) -> Result<(), String> {
        other.validate()?;
        let mut next = self.clone();
        next.join(other)?;
        *self = next;
        Ok(())
    }

    fn join(&mut self, other: &SyncState) -> Result<(), String> {
        for (r, h) in &other.clocks {
            let mine = self.clocks.entry(*r).or_insert(*h);
            *mine = (*mine).max(*h);
        }
        join_map(&mut self.records, &other.records)?;
        join_map(&mut self.accounts, &other.accounts)?;
        self.fetched = self.fetched.max(other.fetched);
        self.coverage.join(&other.coverage);
        join_map(&mut self.overrides, &other.overrides)?;
        join_map(&mut self.rules, &other.rules)?;
        join_map(&mut self.settings, &other.settings)?;
        join_map(&mut self.queries, &other.queries)?;
        Ok(())
    }

    /// Checks a state from elsewhere before anything is joined (design §2.4).
    pub fn validate(&self) -> Result<(), String> {
        if self.version != STATE_VERSION {
            return Err(format!(
                "sync state version {} (this build reads {STATE_VERSION}); update the app",
                self.version
            ));
        }
        for (r, h) in &self.clocks {
            if h.r != *r {
                return Err(format!("clock of replica {r:016x} carries {h:?}"));
            }
        }
        let issued = |at: &Hlc, what: &str| -> Result<(), String> {
            match self.clocks.get(&at.r) {
                Some(top) if at <= top => Ok(()),
                _ => Err(format!("{what} stamped {at:?}, beyond its replica's clock")),
            }
        };
        for (id, v) in &self.records {
            issued(&v.at, "record")?;
            if let RecordVersion::Live(r) = &v.value {
                if &r.id != id {
                    return Err(format!("record {id:?} holds id {:?}", r.id));
                }
                if !r.amount.is_finite() {
                    return Err(format!("record {id:?} has a non-finite amount"));
                }
            }
            if id.is_empty() {
                return Err("a record has an empty id".into());
            }
        }
        for (id, v) in &self.accounts {
            issued(&v.at, "account")?;
            if &v.value.id != id {
                return Err(format!("account {id:?} holds id {:?}", v.value.id));
            }
            if v.value.current.is_some_and(|c| !c.is_finite()) {
                return Err(format!("account {id:?} has a non-finite balance"));
            }
        }
        if let Some(f) = &self.fetched {
            issued(&f.at, "fetch")?;
        }
        if !self.coverage.is_normal() {
            return Err("coverage is not in normal form".into());
        }
        for v in self.overrides.values() {
            issued(&v.at, "override")?;
        }
        for v in self.rules.values() {
            issued(&v.at, "rule")?;
        }
        for v in self.settings.values() {
            issued(&v.at, "setting")?;
        }
        for (id, v) in &self.queries {
            issued(&v.at, "query")?;
            if let Some(q) = &v.value
                && q.label.len() > MAX_LABEL
            {
                return Err(format!("query {id:?} label is over {MAX_LABEL} bytes"));
            }
        }
        Ok(())
    }

    /// The canonical encoding.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a sync state always serializes")
    }

    /// Decode and validate.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let s: SyncState = serde_json::from_str(text).map_err(|e| format!("sync state: {e}"))?;
        s.validate()?;
        Ok(s)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    /// `k` random valid states that could all exist at once: each holds a
    /// random subset of one shared history of writes, as real replicas
    /// hold different subsets of what has been written. Stamps are unique
    /// per write, keys come from a small space so they collide often, and
    /// clocks are skewed and jump back (`now` is random).
    pub(crate) fn random_states(rng: &mut ChaCha8Rng, k: usize) -> Vec<SyncState> {
        let writes = history(rng);
        (0..k)
            .map(|_| {
                let mut s = SyncState::new();
                for w in writes.iter().filter(|_| rng.random_bool(0.5)) {
                    s.merge(w).unwrap();
                }
                s
            })
            .collect()
    }

    /// Single-write states, one per write in a random history.
    fn history(rng: &mut ChaCha8Rng) -> Vec<SyncState> {
        let mut clock = SyncState::new();
        let n = rng.random_range(0..16);
        (0..n).map(|_| one_write(rng, &mut clock)).collect()
    }

    fn one_write(rng: &mut ChaCha8Rng, clock: &mut SyncState) -> SyncState {
        let mut s = SyncState::new();
        {
            let replica = rng.random_range(1..4u64);
            let now = rng.random_range(0..50u64);
            let at = clock.stamp(replica, now);
            s.clocks.insert(replica, at);
            let key = format!("k{}", rng.random_range(0..4));
            let kind = [
                None,
                Some(FlowKind::Transfer),
                Some(FlowKind::Scheduled),
                Some(FlowKind::Discretionary),
            ][rng.random_range(0..4)];
            match rng.random_range(0..7) {
                0 => {
                    s.overrides.insert(key, Lww::new(at, kind));
                }
                1 => {
                    s.rules.insert(key, Lww::new(at, kind));
                }
                2 => {
                    let setting = [Setting::HorizonDays, Setting::Paths, Setting::Seed]
                        [rng.random_range(0..3)];
                    s.settings
                        .insert(setting, Lww::new(at, rng.random_range(0..5)));
                }
                3 => {
                    let v = rng.random_bool(0.7).then(|| SavedQuery {
                        amount_cents: rng.random_range(-500..500),
                        by: Date(rng.random_range(0..9)),
                        label: String::new(),
                    });
                    s.queries.insert(key, Lww::new(at, v));
                }
                4 => {
                    let v = if rng.random_bool(0.8) {
                        RecordVersion::Live(record(&key, rng.random_range(-900..900) as f64 / 7.0))
                    } else {
                        RecordVersion::Removed
                    };
                    s.records.insert(key, Lww::new(at, v));
                }
                5 => {
                    let f = Fetched {
                        today: Date(rng.random_range(0..9)),
                        at,
                    };
                    s.fetched = s.fetched.max(Some(f));
                }
                _ => {
                    let a = rng.random_range(0..30);
                    s.coverage.insert(a, a + rng.random_range(0..5));
                }
            }
        }
        s
    }

    pub(crate) fn record(id: &str, amount: f64) -> BankRecord {
        BankRecord {
            id: id.into(),
            account: "chk".into(),
            date: Date(3),
            authorized_date: None,
            pending: false,
            pending_id: None,
            amount,
            currency: None,
            merchant_name: None,
            name: None,
            category: None,
        }
    }

    fn merged(a: &SyncState, b: &SyncState) -> SyncState {
        let mut x = a.clone();
        x.merge(b).unwrap();
        x
    }

    /// Lattice laws, compared as encoded bytes (design §5).
    #[test]
    fn merge_is_commutative_associative_and_idempotent() {
        for seed in 0..500 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let [a, b, c]: [SyncState; 3] = random_states(&mut rng, 3).try_into().unwrap();
            for s in [&a, &b, &c] {
                s.validate().unwrap();
            }
            assert_eq!(
                merged(&a, &b).to_json(),
                merged(&b, &a).to_json(),
                "seed {seed}"
            );
            assert_eq!(
                merged(&merged(&a, &b), &c).to_json(),
                merged(&a, &merged(&b, &c)).to_json(),
                "seed {seed}"
            );
            assert_eq!(merged(&a, &a).to_json(), a.to_json(), "seed {seed}");
            // Inflationary: merging in more never loses what was there.
            let ab = merged(&a, &b);
            assert_eq!(merged(&ab, &a).to_json(), ab.to_json(), "seed {seed}");
        }
    }

    #[test]
    fn encoding_round_trips_byte_for_byte() {
        for seed in 0..200 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let s = random_states(&mut rng, 1).pop().unwrap();
            let text = s.to_json();
            let back = SyncState::from_json(&text).unwrap();
            assert_eq!(back, s);
            assert_eq!(back.to_json(), text, "seed {seed}");
        }
    }

    #[test]
    fn later_write_wins_and_stamps_track_the_merge() {
        let (a_id, b_id) = (1, 2);
        let mut a = SyncState::new();
        let at = a.stamp(a_id, 86_400_000);
        a.settings.insert(Setting::HorizonDays, Lww::new(at, 90));

        // B's clock reads a day earlier than A's. It merges A, then writes.
        let mut b = SyncState::new();
        b.merge(&a).unwrap();
        let at = b.stamp(b_id, 0);
        assert!(at > a.seen().unwrap());
        b.settings.insert(Setting::HorizonDays, Lww::new(at, 180));

        a.merge(&b).unwrap();
        assert_eq!(a.settings[&Setting::HorizonDays].value, 180);
        assert_eq!(a.to_json(), merged(&b, &a).to_json());
    }

    #[test]
    fn a_failed_merge_changes_nothing() {
        let mut a = SyncState::new();
        let at = a.stamp(1, 10);
        a.settings.insert(Setting::Seed, Lww::new(at, 1));
        let mut bad = a.clone();
        bad.settings.insert(Setting::Seed, Lww::new(at, 2)); // reused stamp
        bad.overrides.insert("x".into(), Lww::new(at, None));
        let before = a.to_json();
        assert!(a.merge(&bad).is_err());
        assert_eq!(a.to_json(), before);
    }

    #[test]
    fn validation_refuses_bad_states() {
        let mut s = SyncState::new();
        let at = s.stamp(1, 10);
        let ok = s.clone();

        let mut v = ok.clone();
        v.version = 2;
        assert!(v.validate().unwrap_err().contains("update the app"));

        let mut v = ok.clone();
        let ahead = Hlc { l: 11, ..at };
        v.overrides.insert("x".into(), Lww::new(ahead, None));
        assert!(v.validate().unwrap_err().contains("beyond"));

        let mut v = ok.clone();
        v.records.insert(
            "a".into(),
            Lww::new(at, RecordVersion::Live(record("b", 1.0))),
        );
        assert!(v.validate().unwrap_err().contains("holds id"));

        let mut v = ok.clone();
        v.records.insert(
            "a".into(),
            Lww::new(at, RecordVersion::Live(record("a", f64::NAN))),
        );
        assert!(v.validate().is_err());

        let mut v = ok.clone();
        v.queries.insert(
            "q".into(),
            Lww::new(
                at,
                Some(SavedQuery {
                    amount_cents: 1,
                    by: Date(0),
                    label: "x".repeat(MAX_LABEL + 1),
                }),
            ),
        );
        assert!(v.validate().is_err());

        assert!(SyncState::from_json(r#"{"version":1}"#).is_err());
        assert!(
            SyncState::from_json(
                &ok.to_json()
                    .replace("\"version\":1", "\"version\":1,\"x\":0")
            )
            .is_err()
        );
    }
}
