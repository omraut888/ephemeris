//! `ephemeris sync …`: the Mac as fetcher and a command-line replica
//! (docs/phase4b-design.md §1.4, §4.4).
//!
//! A replica file holds this replica's id and its sync state as plain JSON.
//! It is a developer tool: the sealed per-device files and their key come
//! with the shell adapters (design §3).

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use ephemeris_core::Day;
use ephemeris_core::schema::Date;
use ephemeris_sync::ingest::FetchBatch;
use ephemeris_sync::{ReplicaId, SyncState};
use serde::{Deserialize, Serialize};

use crate::files::write_json;
use crate::plaid;

/// A replica file: who this replica is, and what it holds.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplicaFile {
    /// 16 hex digits.
    pub replica: String,
    pub state: SyncState,
}

impl ReplicaFile {
    pub fn id(&self) -> Result<ReplicaId, String> {
        parse_replica(&self.replica)
    }
}

pub fn parse_replica(s: &str) -> Result<ReplicaId, String> {
    if s.len() != 16 {
        return Err(format!("replica id {s:?} is not 16 hex digits"));
    }
    u64::from_str_radix(s, 16).map_err(|_| format!("replica id {s:?} is not 16 hex digits"))
}

fn random_replica() -> Result<ReplicaId, String> {
    let mut b = [0u8; 8];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
    Ok(u64::from_le_bytes(b))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn load(path: &Path) -> Result<ReplicaFile, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let file: ReplicaFile = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a replica file: {e}", path.display()))?;
    file.id()?;
    file.state
        .validate()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(file)
}

/// Write via a temporary file and a rename, so a crash never leaves half a file.
pub fn save(path: &Path, file: &ReplicaFile) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    write_json(&tmp, file)?;
    fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn init(path: &Path, replica: Option<&str>) -> Result<(), String> {
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let id = match replica {
        Some(s) => parse_replica(s)?,
        None => random_replica()?,
    };
    let file = ReplicaFile {
        replica: format!("{id:016x}"),
        state: SyncState::new(),
    };
    save(path, &file)?;
    println!("New replica {} in {}", file.replica, path.display());
    Ok(())
}

/// The Plaid dump as one fetch. The dump replays `/transactions/sync` from
/// no cursor, so it is the bank's whole current set: a complete fetch whose
/// window starts at its earliest record.
pub fn plaid_batch(raw: &Path, today: Option<Day>) -> Result<FetchBatch, String> {
    let (sync, balances, today) = plaid::load(raw, today)?;
    fetch_batch(&sync, &balances, today)
}

/// [`plaid_batch`] without the file reading.
pub fn fetch_batch(
    sync: &plaid::SyncFile,
    balances: &plaid::BalanceFile,
    today: Date,
) -> Result<FetchBatch, String> {
    let records = plaid::current_records(sync)
        .into_values()
        .map(plaid::bank_record)
        .collect::<Result<Vec<_>, _>>()?;
    let window_start = records
        .iter()
        .map(|r| r.authorized_date.unwrap_or(r.date))
        .min()
        .unwrap_or(today)
        .min(today);
    Ok(FetchBatch {
        today,
        window_start,
        records,
        removed: Vec::new(),
        accounts: plaid::account_snapshots(balances),
        complete: true,
    })
}

pub fn import_plaid(path: &Path, raw: &Path, today: Option<Day>) -> Result<(), String> {
    let mut file = load(path)?;
    let batch = plaid_batch(raw, today)?;
    let at = file.state.ingest(&batch, file.id()?, now_ms())?;
    save(path, &file)?;
    println!(
        "Fetched {} records, {} accounts, {}..={}, stamped {at:?}",
        batch.records.len(),
        batch.accounts.len(),
        batch.window_start,
        batch.today
    );
    Ok(())
}

pub fn merge(path: &Path, others: &[impl AsRef<Path>]) -> Result<(), String> {
    let mut file = load(path)?;
    for other in others {
        let other = other.as_ref();
        let theirs = load(other)?;
        file.state
            .merge(&theirs.state)
            .map_err(|e| format!("{}: {e}", other.display()))?;
        println!("Merged replica {} from {}", theirs.replica, other.display());
    }
    save(path, &file)
}

pub fn materialize(path: &Path, out: &Path) -> Result<(), String> {
    let file = load(path)?;
    let (feed, notes) = file.state.materialize()?;
    write_json(out, &feed)?;
    plaid::print_summary(&feed, &notes, out);
    Ok(())
}

pub fn show(path: &Path) -> Result<(), String> {
    let file = load(path)?;
    let s = &file.state;
    let live = s
        .records
        .values()
        .filter(|v| matches!(v.value, ephemeris_sync::state::RecordVersion::Live(_)))
        .count();
    println!("replica   {}", file.replica);
    println!("replicas  {}", s.clocks.len());
    println!("records   {live} live, {} removed", s.records.len() - live);
    println!("accounts  {}", s.accounts.len());
    match &s.fetched {
        Some(f) => println!("fetched   through {}", f.today),
        None => println!("fetched   never"),
    }
    println!(
        "edits     {} overrides, {} rules, {} settings, {} queries",
        s.overrides.len(),
        s.rules.len(),
        s.settings.len(),
        s.queries.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replica_ids_are_16_hex_digits() {
        assert_eq!(parse_replica("00000000000000ff"), Ok(255));
        assert!(parse_replica("ff").is_err());
        assert!(parse_replica("zz000000000000ff").is_err());
    }
}
