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
use crate::plaid::PlaidProgress;

/// A replica file: who this replica is, and what it holds.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplicaFile {
    /// 16 hex digits.
    pub replica: String,
    pub state: SyncState,
    /// The Plaid fetch this replica imported last, on the fetcher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plaid: Option<PlaidProgress>,
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
        plaid: None,
    };
    save(path, &file)?;
    println!("New replica {} in {}", file.replica, path.display());
    Ok(())
}

/// The batch for the dump in `raw`, checked against what this replica
/// imported before (`ephemeris_plaid::batch`). Returns the progress to
/// record once it is ingested.
pub fn plaid_batch(
    raw: &Path,
    today: Option<Day>,
    last: Option<&PlaidProgress>,
) -> Result<(FetchBatch, Option<PlaidProgress>), String> {
    let (sync, balances, today, meta) = plaid::load(raw, today)?;
    plaid::batch(&sync, &balances, today, meta.as_ref(), last)
}

pub fn import_plaid(path: &Path, raw: &Path, today: Option<Day>) -> Result<(), String> {
    let mut file = load(path)?;
    let (batch, progress) = plaid_batch(raw, today, file.plaid.as_ref())?;
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
    if progress.is_some() {
        file.plaid = progress;
    }
    save(path, &file)?;
    let what = match (&file.plaid, batch.complete) {
        (Some(p), true) => format!("full fetch {} of item {}", p.seq, p.item_id),
        (Some(p), false) => format!("incremental fetch {} of item {}", p.seq, p.item_id),
        (None, _) => "fetch".into(),
    };
    println!(
        "Recorded {what}: {} records, {} removed, {} accounts, {}..={}, stamped {at:?}",
        batch.records.len(),
        batch.removed.len(),
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

    use serde_json::{Value, json};

    fn txn(id: &str, amount: f64, date: &str, pending: bool) -> Value {
        json!({
            "transaction_id": id, "account_id": "chk", "amount": amount,
            "iso_currency_code": "USD", "date": date, "authorized_date": date,
            "pending": pending, "pending_transaction_id": null,
            "merchant_name": "Shop", "name": "SHOP", "personal_finance_category": null
        })
    }

    /// A fetch script dump in a fresh temp folder.
    fn dump(name: &str, pages: Value, meta: Value) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ephemeris-dump-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let write = |f: &str, v: Value| fs::write(dir.join(f), v.to_string()).unwrap();
        write("transactions_sync.json", json!({ "pages": pages }));
        write(
            "accounts_balance.json",
            json!({"accounts": [{"account_id": "chk", "type": "depository",
                   "balances": {"current": 500.0, "iso_currency_code": "USD"}}]}),
        );
        write("fetch_meta.json", meta);
        dir
    }

    fn full(seq: u64, item: &str) -> Value {
        json!({"fetched_on": "2026-10-10", "mode": "full", "item_id": item, "fetch_seq": seq})
    }

    fn delta(seq: u64, prev: u64) -> Value {
        json!({"fetched_on": "2026-10-12", "mode": "incremental", "item_id": "item1",
               "fetch_seq": seq, "previous_seq": prev, "previous_fetched_on": "2026-10-10"})
    }

    fn ingest(
        state: &mut SyncState,
        last: &mut Option<PlaidProgress>,
        raw: &Path,
    ) -> Result<(), String> {
        let (batch, p) = plaid_batch(raw, None, last.as_ref())?;
        state.ingest(&batch, 1, 0)?;
        *last = p;
        Ok(())
    }

    #[test]
    fn a_full_fetch_then_deltas_build_the_same_feed_as_one_full_fetch() {
        let first = vec![
            txn("a", 10.0, "2026-10-01", false),
            txn("p", 4.0, "2026-10-09", true),
            txn("x", 7.0, "2026-10-09", true),
        ];
        let full_dir = dump("full1", json!([{"added": first}]), full(1, "item1"));
        // The delta: p posts as q, x is cancelled, b is new, a's name changes,
        // and "old" is history Plaid backfills from before the first fetch.
        let mut q = txn("q", 4.5, "2026-10-11", false);
        q["pending_transaction_id"] = json!("p");
        let mut a2 = txn("a", 10.0, "2026-10-01", false);
        a2["merchant_name"] = json!("Shop Inc");
        let delta_dir = dump(
            "delta2",
            json!([{"added": [q.clone(), txn("b", 2.0, "2026-10-12", false), txn("old", 3.0, "2026-08-01", false)],
                    "modified": [a2.clone()],
                    "removed": [{"transaction_id": "p"}, {"transaction_id": "x"}]}]),
            delta(2, 1),
        );
        let (mut state, mut last) = (SyncState::new(), None);
        ingest(&mut state, &mut last, &full_dir).unwrap();
        ingest(&mut state, &mut last, &delta_dir).unwrap();
        assert_eq!(
            last,
            Some(PlaidProgress {
                item_id: "item1".into(),
                seq: 2
            })
        );

        // The same final records fetched in one go.
        let mut meta = full(3, "item1");
        meta["fetched_on"] = json!("2026-10-12");
        let once = dump(
            "once",
            json!([{"added": [a2, q, txn("b", 2.0, "2026-10-12", false), txn("old", 3.0, "2026-08-01", false)]}]),
            meta,
        );
        let (batch, _) = plaid_batch(&once, None, None).unwrap();
        let mut direct = SyncState::new();
        direct.ingest(&batch, 1, 0).unwrap();
        assert_eq!(state.materialize().unwrap(), direct.materialize().unwrap());
        // And import-plaid refuses the delta: it is not a whole feed.
        let err = plaid::run(&delta_dir, None, &delta_dir.join("feed.json")).unwrap_err();
        assert!(err.contains("incremental fetch"), "{err}");
        for d in [full_dir, delta_dir, once] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn deltas_must_follow_the_last_imported_fetch() {
        let page = json!([{"added": [txn("a", 1.0, "2026-10-01", false)]}]);
        let d2 = dump("seq-d2", page.clone(), delta(2, 1));
        let d3 = dump("seq-d3", page.clone(), delta(3, 2));
        let f1 = dump("seq-f1", page.clone(), full(1, "item1"));
        let other = dump("seq-other", page, full(1, "item2"));

        let (mut state, mut last) = (SyncState::new(), None);
        let err = ingest(&mut state, &mut last, &d2).unwrap_err();
        assert!(err.contains("no earlier fetch"), "{err}");
        ingest(&mut state, &mut last, &f1).unwrap();
        let err = ingest(&mut state, &mut last, &d3).unwrap_err();
        assert!(err.contains("changes in between would be lost"), "{err}");
        ingest(&mut state, &mut last, &d2).unwrap();
        let err = ingest(&mut state, &mut last, &d2).unwrap_err();
        assert!(err.contains("already imported fetch 2"), "{err}");
        ingest(&mut state, &mut last, &d3).unwrap();
        let err = ingest(&mut state, &mut last, &other).unwrap_err();
        assert!(err.contains("start a new sync state"), "{err}");
        // A full fetch of the same item is always accepted: it resets the run.
        let mut f9 = full(9, "item1");
        f9["fetched_on"] = json!("2026-10-13");
        let f9 = dump("seq-f9", json!([{"added": []}]), f9);
        ingest(&mut state, &mut last, &f9).unwrap();
        assert_eq!(last.unwrap().seq, 9);
        for d in [d2, d3, f1, other, f9] {
            fs::remove_dir_all(d).unwrap();
        }
    }
}
