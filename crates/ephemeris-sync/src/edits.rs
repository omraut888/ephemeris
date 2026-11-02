//! The user's own state: overrides, rules, settings and saved queries
//! (design §1.2, §1.3). In 4b they are set from the CLI (§4.4).

use ephemeris_core::FlowKind;
use ephemeris_core::pipeline::ForecastOptions;
use serde::{Deserialize, Serialize};

use crate::bank::BankRecord;
use crate::hlc::ReplicaId;
use crate::lww::Lww;
use crate::state::{MAX_LABEL, SavedQuery, Setting, SyncState};
use crate::tag::counterparty;

/// The largest forecast horizon and path count the engine accepts.
pub const MAX_HORIZON_DAYS: u64 = 3660;
pub const MAX_PATHS: u64 = 1_000_000;

/// One user edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "edit", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    /// Set (or, with `None`, clear) the kind of one bank record.
    Override {
        id: String,
        kind: Option<FlowKind>,
    },
    /// Set (or clear) the kind of every record from a counterparty.
    Rule {
        counterparty: String,
        kind: Option<FlowKind>,
    },
    Set {
        setting: Setting,
        value: u64,
    },
    /// Make the editing device the one that fetches from the bank (design
    /// §1.4), e.g. after moving to a new Mac. The latest handover wins.
    TakeFetcher,
    AddQuery {
        query: SavedQuery,
    },
    EditQuery {
        id: String,
        query: SavedQuery,
    },
    DeleteQuery {
        id: String,
    },
}

/// The key a rule is stored under: the counterparty as the tagging computes
/// it from a merchant name (trimmed, upper case).
pub fn rule_key(name: &str) -> String {
    counterparty(Some(name), None)
}

impl SyncState {
    /// Apply a local edit by `replica` at `now_ms`. Returns the new query's
    /// id for [`Edit::AddQuery`].
    pub fn edit(
        &mut self,
        edit: &Edit,
        replica: ReplicaId,
        now_ms: u64,
    ) -> Result<Option<String>, String> {
        // Check everything before stamping, so a refused edit changes nothing.
        match edit {
            Edit::Override { id, .. } if id.is_empty() => {
                return Err("an override needs a transaction id".into());
            }
            Edit::Rule { counterparty, .. } if rule_key(counterparty).is_empty() => {
                return Err("a rule needs a counterparty name".into());
            }
            Edit::Set { setting, value } => check_setting(*setting, *value)?,
            Edit::AddQuery { query } => check_query(query)?,
            Edit::EditQuery { id, query } => {
                self.live_query(id)?;
                check_query(query)?;
            }
            Edit::DeleteQuery { id } => {
                self.live_query(id)?;
            }
            _ => {}
        }
        let at = self.stamp(replica, now_ms);
        match edit {
            Edit::Override { id, kind } => {
                self.overrides.insert(id.clone(), Lww::new(at, *kind));
            }
            Edit::Rule { counterparty, kind } => {
                self.rules
                    .insert(rule_key(counterparty), Lww::new(at, *kind));
            }
            Edit::Set { setting, value } => {
                self.settings.insert(*setting, Lww::new(at, *value));
            }
            Edit::AddQuery { query } => {
                let id = self.next_query_id(replica);
                self.queries
                    .insert(id.clone(), Lww::new(at, Some(query.clone())));
                return Ok(Some(id));
            }
            Edit::EditQuery { id, query } => {
                self.queries
                    .insert(id.clone(), Lww::new(at, Some(query.clone())));
            }
            Edit::DeleteQuery { id } => {
                self.queries.insert(id.clone(), Lww::new(at, None));
            }
            Edit::TakeFetcher => {
                self.fetcher = Some(Lww::new(at, replica));
            }
        }
        Ok(None)
    }

    fn live_query(&self, id: &str) -> Result<&SavedQuery, String> {
        self.queries
            .get(id)
            .and_then(|v| v.value.as_ref())
            .ok_or_else(|| format!("no saved query {id:?}"))
    }

    /// `<replica>-<n>`, one past the largest `n` this replica has used.
    /// Ids are never reused: deleted queries keep their key.
    fn next_query_id(&self, replica: ReplicaId) -> String {
        let prefix = format!("{replica:016x}-");
        let n = self
            .queries
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix)?.parse::<u64>().ok())
            .max()
            .map_or(1, |n| n + 1);
        format!("{prefix}{n}")
    }

    /// The saved queries that are not deleted, by id.
    pub fn saved_queries(&self) -> impl Iterator<Item = (&str, &SavedQuery)> {
        self.queries
            .iter()
            .filter_map(|(id, v)| v.value.as_ref().map(|q| (id.as_str(), q)))
    }

    /// The record override that applies to `r`: the newer of its own and,
    /// for a posted record, its pending predecessor's (design §1.3).
    pub fn record_override(&self, r: &BankRecord) -> Option<&Lww<Option<FlowKind>>> {
        let own = self.overrides.get(&r.id);
        let pred = r
            .pending_id
            .as_ref()
            .filter(|_| !r.pending)
            .and_then(|p| self.overrides.get(p));
        newer(own, pred)
    }

    /// The kind `r` ends up with: a record override (its own, its pending
    /// predecessor's, or its transfer partner's, whichever is newest), then
    /// a counterparty rule, then the tagging's kind. A cleared override or
    /// rule falls through.
    pub fn effective_kind(
        &self,
        r: &BankRecord,
        partner: Option<&BankRecord>,
        tagged: FlowKind,
    ) -> FlowKind {
        let own = self.record_override(r);
        let theirs = partner.and_then(|p| self.record_override(p));
        if let Some(k) = newer(own, theirs).and_then(|v| v.value) {
            return k;
        }
        let key = counterparty(r.merchant_name.as_deref(), r.name.as_deref());
        self.rules.get(&key).and_then(|v| v.value).unwrap_or(tagged)
    }

    /// The forecast options the synced settings ask for, defaults elsewhere.
    pub fn forecast_options(&self) -> ForecastOptions {
        let d = ForecastOptions::default();
        let get = |s| self.settings.get(&s).map(|v| v.value);
        ForecastOptions {
            horizon_days: get(Setting::HorizonDays).map_or(d.horizon_days, |v| v as u32),
            paths: get(Setting::Paths).map_or(d.paths, |v| v as u32),
            seed: get(Setting::Seed).unwrap_or(d.seed),
            targets: Vec::new(),
        }
    }
}

fn newer<'a, T>(a: Option<&'a Lww<T>>, b: Option<&'a Lww<T>>) -> Option<&'a Lww<T>> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.at > x.at { y } else { x }),
        (x, y) => x.or(y),
    }
}

pub(crate) fn check_setting(setting: Setting, value: u64) -> Result<(), String> {
    let ok = match setting {
        Setting::HorizonDays => (1..=MAX_HORIZON_DAYS).contains(&value),
        Setting::Paths => (1..=MAX_PATHS).contains(&value),
        Setting::Seed => true,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{setting:?} cannot be {value}"))
    }
}

fn check_query(q: &SavedQuery) -> Result<(), String> {
    if q.label.len() > MAX_LABEL {
        return Err(format!("a query label is at most {MAX_LABEL} bytes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::AccountSnapshot;
    use crate::ingest::FetchBatch;
    use crate::state::tests::record;
    use ephemeris_core::schema::Date;

    const A: ReplicaId = 0xa;
    const B: ReplicaId = 0xb;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn named(id: &str, amount: f64, date: &str, name: &str) -> BankRecord {
        BankRecord {
            date: d(date),
            name: Some(name.into()),
            ..record(id, amount)
        }
    }

    fn fetched(records: Vec<BankRecord>, accounts: &[&str]) -> SyncState {
        let mut s = SyncState::new();
        let batch = FetchBatch {
            today: d("2026-10-10"),
            window_start: d("2026-10-01"),
            records,
            removed: vec![],
            accounts: accounts
                .iter()
                .map(|id| AccountSnapshot {
                    id: (*id).into(),
                    depository: true,
                    current: Some(0.0),
                    currency: None,
                })
                .collect(),
            complete: false,
        };
        s.ingest(&batch, A, 1).unwrap();
        s
    }

    fn kinds(s: &SyncState) -> Vec<(String, FlowKind)> {
        let (feed, _) = s.materialize().unwrap();
        feed.transactions
            .iter()
            .map(|t| (t.description.clone().unwrap(), t.kind))
            .collect()
    }

    fn set(s: &mut SyncState, id: &str, kind: Option<FlowKind>, r: ReplicaId, now: u64) {
        let e = Edit::Override {
            id: id.into(),
            kind,
        };
        s.edit(&e, r, now).unwrap();
    }

    #[test]
    fn record_override_beats_rule_beats_tag_and_clearing_falls_through() {
        let mut s = fetched(vec![named("a", -50.0, "2026-10-02", "Water Co")], &["chk"]);
        assert_eq!(kinds(&s)[0].1, FlowKind::Discretionary);
        let rule = Edit::Rule {
            counterparty: " water co ".into(),
            kind: Some(FlowKind::Scheduled),
        };
        s.edit(&rule, A, 2).unwrap();
        assert_eq!(kinds(&s)[0].1, FlowKind::Scheduled);
        set(&mut s, "a", Some(FlowKind::Transfer), A, 3);
        assert_eq!(kinds(&s)[0].1, FlowKind::Transfer);
        set(&mut s, "a", None, A, 4);
        assert_eq!(kinds(&s)[0].1, FlowKind::Scheduled);
        // The overridden scheduled record gets a stream, as tagged ones do.
        let (feed, notes) = s.materialize().unwrap();
        assert!(feed.transactions[0].stream.is_some());
        assert_eq!(notes.overridden, 1);
    }

    #[test]
    fn a_pending_override_carries_over_unless_the_posted_one_is_newer() {
        let mut p = named("p", -9.0, "2026-10-02", "CAFE");
        p.pending = true;
        let mut s = fetched(vec![p], &["chk"]);
        set(&mut s, "p", Some(FlowKind::Scheduled), A, 2);
        assert_eq!(kinds(&s)[0].1, FlowKind::Scheduled);

        let mut q = named("q", -9.0, "2026-10-03", "CAFE");
        q.pending_id = Some("p".into());
        let posted = FetchBatch {
            today: d("2026-10-10"),
            window_start: d("2026-10-01"),
            records: vec![q],
            removed: vec!["p".into()],
            accounts: vec![],
            complete: false,
        };
        s.ingest(&posted, A, 3).unwrap();
        assert_eq!(kinds(&s), vec![("CAFE".into(), FlowKind::Scheduled)]);

        set(&mut s, "q", Some(FlowKind::Discretionary), B, 4);
        assert_eq!(kinds(&s)[0].1, FlowKind::Discretionary);
        // An even newer edit on the pending id wins again: newest intent.
        set(&mut s, "p", Some(FlowKind::Income), A, 5);
        assert_eq!(kinds(&s)[0].1, FlowKind::Income);
    }

    #[test]
    fn an_override_on_one_transfer_leg_moves_both() {
        let mut out = named("o", -300.0, "2026-10-05", "TO SAVINGS");
        let mut inc = named("i", 300.0, "2026-10-05", "FROM CHECKING");
        inc.account = "sav".into();
        for t in [&mut out, &mut inc] {
            t.category = Some(crate::Category {
                primary: if t.amount < 0.0 {
                    "TRANSFER_OUT"
                } else {
                    "TRANSFER_IN"
                }
                .into(),
                detailed: "TRANSFER_X".into(),
            });
        }
        let mut s = fetched(vec![out, inc], &["chk", "sav"]);
        assert!(kinds(&s).iter().all(|(_, k)| *k == FlowKind::Transfer));
        set(&mut s, "o", Some(FlowKind::Discretionary), A, 2);
        assert!(kinds(&s).iter().all(|(_, k)| *k == FlowKind::Discretionary));
    }

    #[test]
    fn concurrent_overrides_resolve_the_same_everywhere() {
        let base = fetched(vec![named("a", -5.0, "2026-10-02", "X")], &["chk"]);
        let (mut a, mut b) = (base.clone(), base);
        // B's clock reads a day ahead; neither has seen the other's edit.
        set(&mut a, "a", Some(FlowKind::Scheduled), A, 1_000);
        set(&mut b, "a", Some(FlowKind::Transfer), B, 86_401_000);
        let (a0, b0) = (a.clone(), b.clone());
        a.merge(&b0).unwrap();
        b.merge(&a0).unwrap();
        assert_eq!(a.to_json(), b.to_json());
        assert_eq!(kinds(&a)[0].1, FlowKind::Transfer);
        // A, having merged, edits again with its slow clock and still wins.
        set(&mut a, "a", Some(FlowKind::Income), A, 2_000);
        b.merge(&a).unwrap();
        assert_eq!(kinds(&b)[0].1, FlowKind::Income);
    }

    #[test]
    fn queries_get_fresh_ids_and_edits_need_a_live_query() {
        let mut s = SyncState::new();
        let q = SavedQuery {
            amount_cents: 200_000,
            by: d("2026-12-01"),
            label: "trip".into(),
        };
        let id1 = s
            .edit(&Edit::AddQuery { query: q.clone() }, A, 1)
            .unwrap()
            .unwrap();
        let id2 = s
            .edit(&Edit::AddQuery { query: q.clone() }, A, 2)
            .unwrap()
            .unwrap();
        assert_eq!(id1, "000000000000000a-1");
        assert_eq!(id2, "000000000000000a-2");
        s.edit(&Edit::DeleteQuery { id: id2.clone() }, A, 3)
            .unwrap();
        let id3 = s
            .edit(&Edit::AddQuery { query: q.clone() }, A, 4)
            .unwrap()
            .unwrap();
        assert_eq!(id3, "000000000000000a-3", "never reuses a deleted id");
        assert!(
            s.edit(&Edit::EditQuery { id: id2, query: q }, A, 5)
                .is_err()
        );
        let ids: Vec<_> = s.saved_queries().map(|(id, _)| id.to_string()).collect();
        assert_eq!(ids, vec![id1, id3]);
    }

    #[test]
    fn settings_become_forecast_options_and_are_range_checked() {
        let mut s = SyncState::new();
        assert_eq!(s.forecast_options(), ForecastOptions::default());
        for (setting, value) in [(Setting::HorizonDays, 180), (Setting::Seed, u64::MAX)] {
            s.edit(&Edit::Set { setting, value }, A, 1).unwrap();
        }
        let o = s.forecast_options();
        assert_eq!((o.horizon_days, o.paths, o.seed), (180, 2000, u64::MAX));
        let before = s.clone();
        for (setting, value) in [
            (Setting::HorizonDays, 0),
            (Setting::HorizonDays, MAX_HORIZON_DAYS + 1),
            (Setting::Paths, 0),
        ] {
            assert!(s.edit(&Edit::Set { setting, value }, A, 2).is_err());
        }
        assert_eq!(s, before, "a refused edit changes nothing");
    }

    #[test]
    fn edits_round_trip_as_json() {
        let e = Edit::Override {
            id: "x".into(),
            kind: Some(FlowKind::Transfer),
        };
        let text = serde_json::to_string(&e).unwrap();
        assert_eq!(text, r#"{"edit":"override","id":"x","kind":"transfer"}"#);
        assert_eq!(serde_json::from_str::<Edit>(&text).unwrap(), e);
    }
}
