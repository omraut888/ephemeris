//! The sync state as text and bytes in, text and bytes out (design §4.3).
//!
//! Every shell calls these, through WASM, UniFFI or directly, so the merge
//! rules exist once, in Rust. A state crosses the boundary as an opaque JSON
//! string that shells store and pass back but never parse and re-encode:
//! JavaScript, for one, formats numbers differently from `serde_json`, and
//! a state must stay byte-identical everywhere. Where a function returns a
//! state with something else, the state is a JSON *string* inside the
//! object, so `JSON.parse` hands back the exact text.
//!
//! Replica ids are 16 hex digits (a `u64` does not survive JavaScript's
//! numbers). Times are milliseconds since the Unix epoch. Errors are
//! messages that name the problem.

use serde::Serialize;

use crate::edits::Edit;
use crate::envelope::{self, Key, NONCE_LEN};
use crate::hlc::ReplicaId;
use crate::ingest::FetchBatch;
use crate::state::SyncState;

fn state(text: &str) -> Result<SyncState, String> {
    SyncState::from_json(text)
}

/// A replica id from its 16 hex digits.
pub fn parse_replica(hex: &str) -> Result<ReplicaId, String> {
    if hex.len() != 16 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("replica id {hex:?} is not 16 hex digits"));
    }
    ReplicaId::from_str_radix(hex, 16).map_err(|e| e.to_string())
}

fn key(bytes: &[u8]) -> Result<Key, String> {
    bytes
        .try_into()
        .map_err(|_| format!("a sync key is 32 bytes, not {}", bytes.len()))
}

/// A new, empty state.
pub fn sync_new() -> String {
    SyncState::new().to_json()
}

#[derive(Serialize)]
struct Edited {
    /// The new state, as a JSON string.
    state: String,
    /// The id of a query an `add_query` edit saved.
    query_id: Option<String>,
}

/// Apply one [`Edit`] (JSON) by `replica` at `now_ms`:
/// `{"state": "<state JSON>", "query_id": "<id>" | null}`.
pub fn sync_edit(
    state_json: &str,
    edit_json: &str,
    replica: &str,
    now_ms: u64,
) -> Result<String, String> {
    let mut s = state(state_json)?;
    let edit: Edit = serde_json::from_str(edit_json).map_err(|e| format!("edit: {e}"))?;
    let query_id = s.edit(&edit, parse_replica(replica)?, now_ms)?;
    Ok(serde_json::to_string(&Edited {
        state: s.to_json(),
        query_id,
    })
    .expect("strings serialize"))
}

/// Record a [`FetchBatch`] (JSON) fetched by `replica` at `now_ms`.
pub fn sync_ingest(
    state_json: &str,
    batch_json: &str,
    replica: &str,
    now_ms: u64,
) -> Result<String, String> {
    let mut s = state(state_json)?;
    let batch: FetchBatch =
        serde_json::from_str(batch_json).map_err(|e| format!("fetch batch: {e}"))?;
    s.ingest(&batch, parse_replica(replica)?, now_ms)?;
    Ok(s.to_json())
}

/// `a ⊔ b`.
pub fn sync_merge(a_json: &str, b_json: &str) -> Result<String, String> {
    let mut a = state(a_json)?;
    a.merge(&state(b_json)?)?;
    Ok(a.to_json())
}

/// The schema-v1 feed the state describes, for `forecast_json`.
pub fn sync_materialize(state_json: &str) -> Result<String, String> {
    let (feed, _) = state(state_json)?.materialize()?;
    Ok(serde_json::to_string(&feed).expect("a feed always serializes"))
}

/// The `ForecastOptions` the synced settings ask for, for `forecast_json`.
pub fn sync_forecast_options(state_json: &str) -> Result<String, String> {
    Ok(serde_json::to_string(&state(state_json)?.forecast_options()).expect("options serialize"))
}

#[derive(Serialize)]
struct Summary {
    /// Bank data has arrived.
    has_data: bool,
    /// The latest fetch's day.
    fetched_through: Option<String>,
    /// The fetching device, 16 hex digits.
    fetcher: Option<String>,
}

/// What a shell shows about a state without parsing it:
/// `{"has_data", "fetched_through", "fetcher"}`.
pub fn sync_summary(state_json: &str) -> Result<String, String> {
    let s = state(state_json)?;
    Ok(serde_json::to_string(&Summary {
        has_data: s.fetched.is_some(),
        fetched_through: s.fetched.map(|f| f.today.to_string()),
        fetcher: s.fetcher.map(|f| format!("{:016x}", f.value)),
    })
    .expect("a summary serializes"))
}

#[derive(Serialize)]
struct Query<'a> {
    id: &'a str,
    #[serde(flatten)]
    query: &'a crate::state::SavedQuery,
}

/// The saved queries that are not deleted: `[{id, amount_cents, by, label}]`.
pub fn sync_saved_queries(state_json: &str) -> Result<String, String> {
    let s = state(state_json)?;
    let list: Vec<Query> = s
        .saved_queries()
        .map(|(id, query)| Query { id, query })
        .collect();
    Ok(serde_json::to_string(&list).expect("queries serialize"))
}

/// The state sealed as `replica`'s file, under `key` (32 bytes) with
/// `nonce` (24 random bytes from the platform).
pub fn sync_seal(
    state_json: &str,
    replica: &str,
    key_bytes: &[u8],
    nonce: &[u8],
) -> Result<Vec<u8>, String> {
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| format!("a nonce is {NONCE_LEN} bytes, not {}", nonce.len()))?;
    Ok(envelope::seal(
        &state(state_json)?,
        parse_replica(replica)?,
        &key(key_bytes)?,
        &nonce,
    ))
}

#[derive(Serialize)]
struct Opened {
    /// The replica that wrote the file.
    replica: String,
    /// Its state, as a JSON string.
    state: String,
}

/// A sealed file opened: `{"replica": "<16 hex>", "state": "<state JSON>"}`.
pub fn sync_open(file: &[u8], key_bytes: &[u8]) -> Result<String, String> {
    let (replica, s) = envelope::open(file, &key(key_bytes)?)?;
    Ok(serde_json::to_string(&Opened {
        replica: format!("{replica:016x}"),
        state: s.to_json(),
    })
    .expect("strings serialize"))
}

/// The key as the text people copy between devices.
pub fn sync_key_to_text(key_bytes: &[u8]) -> Result<String, String> {
    Ok(envelope::key_to_text(&key(key_bytes)?))
}

/// The key from that text.
pub fn sync_key_from_text(text: &str) -> Result<Vec<u8>, String> {
    Ok(envelope::key_from_text(text)?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "00000000000000aa";

    #[test]
    fn a_state_round_trips_through_every_function_unchanged() {
        let s = sync_new();
        let edited: serde_json::Value = serde_json::from_str(
            &sync_edit(&s, r#"{"edit":"add_query","query":{"amount_cents":200000,"by":"2026-12-01","label":"trip"}}"#, A, 1_000)
                .unwrap(),
        )
        .unwrap();
        let s = edited["state"].as_str().unwrap().to_string();
        assert_eq!(edited["query_id"], "00000000000000aa-1");
        assert_eq!(
            sync_saved_queries(&s).unwrap(),
            r#"[{"id":"00000000000000aa-1","amount_cents":200000,"by":"2026-12-01","label":"trip"}]"#
        );
        let file = sync_seal(&s, A, &[7; 32], &[1; 24]).unwrap();
        let opened: serde_json::Value =
            serde_json::from_str(&sync_open(&file, &[7; 32]).unwrap()).unwrap();
        assert_eq!(opened["replica"], A);
        assert_eq!(
            opened["state"].as_str().unwrap(),
            s,
            "byte-identical after sealing"
        );
        assert_eq!(sync_merge(&s, &sync_new()).unwrap(), s);
        assert_eq!(
            sync_summary(&s).unwrap(),
            r#"{"has_data":false,"fetched_through":null,"fetcher":null}"#
        );
        assert_eq!(
            sync_forecast_options(&s).unwrap(),
            r#"{"horizon_days":90,"paths":2000,"seed":0,"targets":[]}"#
        );
    }

    #[test]
    fn errors_name_the_problem() {
        let s = sync_new();
        assert!(sync_edit(&s, "{}", A, 0).unwrap_err().starts_with("edit:"));
        assert!(
            sync_edit(&s, r#"{"edit":"take_fetcher"}"#, "aa", 0)
                .unwrap_err()
                .contains("16 hex digits")
        );
        assert!(
            sync_ingest(&s, "[]", A, 0)
                .unwrap_err()
                .starts_with("fetch batch:")
        );
        assert!(
            sync_merge("not json", &s)
                .unwrap_err()
                .starts_with("sync state:")
        );
        assert!(
            sync_materialize(&s)
                .unwrap_err()
                .contains("nothing has been fetched")
        );
        assert!(
            sync_seal(&s, A, &[0; 31], &[0; 24])
                .unwrap_err()
                .contains("32 bytes")
        );
        assert!(
            sync_seal(&s, A, &[0; 32], &[0; 23])
                .unwrap_err()
                .contains("24 bytes")
        );
        assert!(
            sync_open(b"nope", &[0; 32])
                .unwrap_err()
                .contains("not an Ephemeris sync file")
        );
        assert!(sync_key_from_text("short").is_err());
        let text = sync_key_to_text(&[9; 32]).unwrap();
        assert_eq!(sync_key_from_text(&text).unwrap(), vec![9; 32]);
    }
}
