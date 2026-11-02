//! `ephemeris sync …`: the Mac as fetcher and a command-line replica
//! (docs/phase4b-design.md §1.4, §4.4).
//!
//! A replica file holds this replica's id and its sync state as plain JSON.
//! It is a developer tool: the sealed per-device files and their key come
//! with the shell adapters (design §3).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use ephemeris_core::schema::Date;
use ephemeris_core::{Day, FlowKind};
use ephemeris_sync::edits::{Edit, rule_key};
use ephemeris_sync::ingest::FetchBatch;
use ephemeris_sync::lww::Lww;
use ephemeris_sync::state::{SavedQuery, Setting};
use ephemeris_sync::{BankRecord, ReplicaId, SyncState, tag};
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

/// Where this device's replica file lives by default. `data/sync/` is
/// gitignored: the file is plaintext financial data.
pub const DEFAULT_STATE: &str = "data/sync/replica.json";

/// Refuse to write `path` if git would track it: inside a work tree and not
/// ignored (or already tracked). Outside any repository, or without git,
/// there is nothing to commit it to.
pub fn ensure_untracked(path: &Path) -> Result<(), String> {
    if path.file_name().is_none() {
        return Err(format!("{} is not a file path", path.display()));
    }
    // Ask git from the nearest folder that exists (init may be about to
    // create the rest), about the path relative to it.
    let mut dir = path;
    let rel = loop {
        dir = match dir.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        if dir.is_dir() {
            break path.strip_prefix(dir).unwrap_or(path);
        }
    };
    let git = |args: &[&std::ffi::OsStr]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
    };
    let inside = git(&["rev-parse".as_ref(), "--is-inside-work-tree".as_ref()]);
    let in_repo = matches!(&inside, Ok(o) if o.status.success() && o.stdout.starts_with(b"true"));
    if !in_repo {
        return Ok(());
    }
    let ignored = git(&[
        "check-ignore".as_ref(),
        "-q".as_ref(),
        "--".as_ref(),
        rel.as_os_str(),
    ]);
    match ignored {
        Ok(o) if o.status.success() => Ok(()),
        _ => Err(format!(
            "refusing to write {}: it is in a git work tree and not gitignored, so its \
             plaintext financial data could be committed. Use the default ({DEFAULT_STATE}, \
             gitignored) or a path outside the repository.",
            path.display()
        )),
    }
}

/// Write via a temporary file and a rename, so a crash never leaves half a file.
pub fn save(path: &Path, file: &ReplicaFile) -> Result<(), String> {
    ensure_untracked(path)?;
    let tmp = path.with_extension("tmp");
    write_json(&tmp, file)?;
    fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn init(path: &Path, replica: Option<&str>) -> Result<(), String> {
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    ensure_untracked(path)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
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
    let at = file
        .state
        .ingest(&batch, file.id()?, now_ms())
        .map_err(|e| {
            if e.contains("fetched by device") {
                format!("{e}\n  On this device: ephemeris sync take-fetcher")
            } else {
                e
            }
        })?;
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

/// Apply one user edit to the replica file (design §4.4: CLI-only in 4b).
pub fn edit(path: &Path, edit: &Edit) -> Result<(), String> {
    let mut file = load(path)?;
    let new_id = file.state.edit(edit, file.id()?, now_ms())?;
    save(path, &file)?;
    match new_id {
        Some(id) => println!("Saved query {id}"),
        None => println!("Done"),
    }
    Ok(())
}

/// Where a record's kind came from.
fn source(state: &SyncState, r: &BankRecord, tagged: FlowKind, kind: FlowKind) -> &'static str {
    if kind == tagged {
        "tagged"
    } else if state.record_override(r).is_some_and(|v| v.value.is_some()) {
        "override"
    } else if state.rules.contains_key(&rule_key(&tag::counterparty(
        r.merchant_name.as_deref(),
        r.name.as_deref(),
    ))) {
        "rule"
    } else {
        "transfer partner's override"
    }
}

/// The live records with their final kind and why, newest first.
pub fn list(path: &Path, kind: Option<FlowKind>, since: Option<Day>) -> Result<(), String> {
    let file = load(path)?;
    let state = &file.state;
    let seen: RefCell<BTreeMap<String, (FlowKind, FlowKind)>> = RefCell::default();
    state.materialize_with(|r, partner, tagged| {
        let k = state.effective_kind(r, partner, tagged);
        seen.borrow_mut().insert(r.id.clone(), (tagged, k));
        k
    })?;
    let seen = seen.into_inner();
    let (_, records) = state.live_records()?;
    let mut rows: Vec<&BankRecord> = records
        .into_iter()
        .filter(|r| seen.contains_key(&r.id))
        .filter(|r| since.is_none_or(|d| r.day() >= d))
        .filter(|r| kind.is_none_or(|k| seen[&r.id].1 == k))
        .collect();
    rows.sort_by(|a, b| (b.day(), &b.id).cmp(&(a.day(), &a.id)));
    for r in rows {
        let (tagged, k) = seen[&r.id];
        let name = r
            .merchant_name
            .as_deref()
            .or(r.name.as_deref())
            .unwrap_or("");
        println!(
            "{}  {:>10.2}  {:<13}  {:<8}  {}{}  {}",
            Date(r.day()),
            r.amount,
            format!("{k:?}").to_lowercase(),
            source(state, r, tagged, k),
            name,
            if r.pending { " (pending)" } else { "" },
            r.id
        );
    }
    Ok(())
}

/// The user-authored state, for the user's own backup (design §3.4).
#[derive(Serialize)]
struct UserState<'a> {
    overrides: &'a BTreeMap<String, Lww<Option<FlowKind>>>,
    rules: &'a BTreeMap<String, Lww<Option<FlowKind>>>,
    settings: &'a BTreeMap<Setting, Lww<u64>>,
    queries: &'a BTreeMap<String, Lww<Option<SavedQuery>>>,
}

pub fn export_plain(path: &Path, out: &Path) -> Result<(), String> {
    let file = load(path)?;
    let s = &file.state;
    write_json(
        out,
        &UserState {
            overrides: &s.overrides,
            rules: &s.rules,
            settings: &s.settings,
            queries: &s.queries,
        },
    )?;
    println!(
        "Wrote overrides, rules, settings and saved queries to {} (plaintext: keep it private)",
        out.display()
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

    /// The replica file holds plaintext financial data: the CLI must never
    /// write it where git would pick it up. Tests run from crates/ephemeris-cli.
    #[test]
    fn replica_files_are_only_written_where_git_ignores_them() {
        let git_works = std::process::Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .is_ok_and(|o| o.status.success());
        if !git_works {
            return; // not a checkout (e.g. a packaged source tree)
        }
        // The default location, before data/sync/ exists or after.
        let default = Path::new("../..").join(DEFAULT_STATE);
        assert_eq!(ensure_untracked(&default), Ok(()));
        assert_eq!(
            ensure_untracked(Path::new("../../data/sync/deeper/x.json")),
            Ok(())
        );
        // The repository root and a source folder are not ignored.
        for p in [
            "../../replica.json",
            "src/replica.json",
            "../../data/replica.json",
        ] {
            let err = ensure_untracked(Path::new(p)).unwrap_err();
            assert!(err.contains("refusing to write"), "{p}: {err}");
        }
        // A tracked file is refused even where a pattern might match.
        assert!(ensure_untracked(Path::new("src/main.rs")).is_err());
        // Outside any repository there is nothing to commit it to.
        let outside = std::env::temp_dir().join("ephemeris-replica-test.json");
        assert_eq!(ensure_untracked(&outside), Ok(()));
    }
}
