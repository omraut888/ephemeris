//! Sync on the desktop (docs/phase4b-design.md §3, §4): this device's state,
//! sealed at rest with the Keychain key, synced through a folder the user
//! chooses, one encrypted file per device.
//!
//! [`Device`] holds the logic, apart from Tauri, so it is tested directly;
//! the commands in `lib.rs` add the native folder dialogs.
//!
//! On disk, in the app's data folder:
//! - `sync/config.json`: this device's replica id, the chosen folder and
//!   which Plaid fetch was imported last. Nothing secret.
//! - `sync/local.ephsync`: the state, sealed with the sync key like the
//!   files in the shared folder.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ephemeris_plaid::{BalanceFile, FetchMeta, PlaidProgress, SyncFile};
use ephemeris_sync::edits::Edit;
use ephemeris_sync::envelope::{self, Key, NONCE_LEN};
use ephemeris_sync::state::SavedQuery;
use ephemeris_sync::{ReplicaId, SyncState};
use ephemeris_sync_folder::{self as folder, KeyStore};
use serde::{Deserialize, Serialize};

/// What the Sync sheet shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// A sync key is in the Keychain.
    pub paired: bool,
    /// This device, 16 hex digits.
    pub replica: String,
    pub folder: Option<String>,
    /// The device allowed to fetch from the bank, if any has.
    pub fetcher: Option<String>,
    pub is_fetcher: bool,
    /// The state holds fetched bank data, so there is a feed to show.
    pub has_data: bool,
    pub fetched_through: Option<String>,
    pub devices: Vec<FolderFile>,
    /// Milliseconds since 1970 of the last sync with the folder.
    pub last_sync: Option<u64>,
    /// What went wrong last, for the sheet to show.
    pub error: Option<String>,
}

/// One file in the sync folder.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderFile {
    pub file: String,
    pub replica: Option<String>,
    pub this_device: bool,
    /// Milliseconds since 1970.
    pub modified: Option<u64>,
    pub error: Option<String>,
}

/// The synced feed and the options the synced settings ask for.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncedFeed {
    pub feed_json: String,
    pub options_json: String,
}

/// A saved afford query, as the UI uses it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    pub id: String,
    pub amount_cents: i64,
    pub by: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    replica: String,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    plaid: Option<PlaidProgress>,
    #[serde(default)]
    last_sync: Option<u64>,
}

struct Inner {
    config: Config,
    /// `None` until a key is there to open the local state with.
    state: Option<SyncState>,
    devices: Vec<FolderFile>,
    error: Option<String>,
}

/// This device's sync: its files, its key store, its state.
pub struct Device {
    dir: PathBuf,
    keys: Box<dyn KeyStore + Send + Sync>,
    inner: Mutex<Inner>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn ms(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not a {what}: {e}", path.display()))
}

impl Device {
    /// The device whose files live in `dir` (the app's `sync` folder),
    /// creating its replica id on first use.
    pub fn open(dir: PathBuf, keys: Box<dyn KeyStore + Send + Sync>) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let config_path = dir.join("config.json");
        let config = if config_path.exists() {
            read_json::<Config>(&config_path, "sync config")?
        } else {
            let c = Config {
                replica: format!("{:016x}", folder::new_replica()?),
                ..Config::default()
            };
            write_config(&dir, &c)?;
            c
        };
        let device = Self {
            dir,
            keys,
            inner: Mutex::new(Inner {
                config,
                state: None,
                devices: Vec::new(),
                error: None,
            }),
        };
        // With a key there, open the local state now; without one the
        // sheet asks to pair first.
        let mut inner = device.inner.lock().unwrap();
        if let Err(e) = device.load_state(&mut inner) {
            inner.error = Some(e);
        }
        drop(inner);
        Ok(device)
    }

    fn replica(inner: &Inner) -> Result<ReplicaId, String> {
        ephemeris_sync::json::parse_replica(&inner.config.replica)
    }

    fn key(&self) -> Result<Key, String> {
        self.keys
            .get()?
            .ok_or_else(|| "no sync key yet: create one, or pair with another device's".into())
    }

    fn local_path(&self) -> PathBuf {
        self.dir.join("local.ephsync")
    }

    fn load_state(&self, inner: &mut Inner) -> Result<(), String> {
        let Some(key) = self.keys.get()? else {
            return Ok(());
        };
        let path = self.local_path();
        inner.state = Some(if path.exists() {
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            envelope::open(&bytes, &key)
                .map_err(|e| format!("this device's saved sync state: {e}"))?
                .1
        } else {
            SyncState::new()
        });
        Ok(())
    }

    fn save_state(&self, inner: &Inner) -> Result<(), String> {
        let Some(state) = &inner.state else {
            return Ok(());
        };
        let key = self.key()?;
        let nonce = folder::random::<NONCE_LEN>()?;
        let sealed = envelope::seal(state, Self::replica(inner)?, &key, &nonce);
        folder::write_atomic(&self.local_path(), &sealed).map(|_| ())
    }

    fn status_of(&self, inner: &Inner) -> SyncStatus {
        let s = inner.state.as_ref();
        let fetcher = s
            .and_then(|s| s.fetcher.as_ref())
            .map(|f| format!("{:016x}", f.value));
        SyncStatus {
            paired: matches!(self.keys.get(), Ok(Some(_))),
            replica: inner.config.replica.clone(),
            folder: inner.config.folder.clone(),
            is_fetcher: fetcher.as_deref() == Some(inner.config.replica.as_str()),
            fetcher,
            has_data: s.is_some_and(|s| s.fetched.is_some()),
            fetched_through: s.and_then(|s| s.fetched).map(|f| f.today.to_string()),
            devices: inner.devices.clone(),
            last_sync: inner.config.last_sync,
            error: inner.error.clone(),
        }
    }

    /// Run `f` on the state, then save it and sync with the folder.
    fn change<T>(
        &self,
        f: impl FnOnce(&mut SyncState, ReplicaId) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut inner = self.inner.lock().unwrap();
        let replica = Self::replica(&inner)?;
        let state = inner
            .state
            .as_mut()
            .ok_or("no sync key yet: create one, or pair with another device's")?;
        let out = f(state, replica)?;
        self.save_state(&inner)?;
        self.sync_locked(&mut inner);
        Ok(out)
    }

    /// Merge the folder's files and write this device's, recording what
    /// happened in the status rather than failing.
    fn sync_locked(&self, inner: &mut Inner) {
        let (Some(dir), Ok(key)) = (inner.config.folder.clone(), self.key()) else {
            return;
        };
        let Ok(replica) = Self::replica(inner) else {
            return;
        };
        let Some(state) = inner.state.as_mut() else {
            return;
        };
        match folder::sync(Path::new(&dir), replica, state, &key) {
            Ok(report) => {
                let me = folder::file_name(replica);
                inner.devices = report
                    .found
                    .iter()
                    .map(|f| FolderFile {
                        file: f.file.clone(),
                        replica: f.replica.map(|r| format!("{r:016x}")),
                        this_device: f.file == me,
                        modified: f.modified.and_then(ms),
                        error: f.error.clone(),
                    })
                    .collect();
                // The folder was read before this device wrote its file:
                // on the first sync, list the file it has just written.
                if !inner.devices.iter().any(|d| d.this_device) {
                    inner.devices.push(FolderFile {
                        file: me,
                        replica: Some(format!("{replica:016x}")),
                        this_device: true,
                        modified: Some(now_ms()),
                        error: None,
                    });
                    inner.devices.sort_by(|a, b| a.file.cmp(&b.file));
                }
                inner.config.last_sync = Some(now_ms());
                inner.error = None;
                if let Err(e) = self
                    .save_state(inner)
                    .and_then(|()| write_config(&self.dir, &inner.config))
                {
                    inner.error = Some(e);
                }
            }
            Err(e) => inner.error = Some(e),
        }
    }

    pub fn status(&self) -> SyncStatus {
        self.status_of(&self.inner.lock().unwrap())
    }

    /// A new random key, for the first device. Refused when one exists.
    pub fn create_key(&self) -> Result<SyncStatus, String> {
        self.set_key(folder::new_key()?)
    }

    /// The key from another device's text. Refused when one exists.
    pub fn pair(&self, text: &str) -> Result<SyncStatus, String> {
        self.set_key(envelope::key_from_text(text)?)
    }

    fn set_key(&self, key: Key) -> Result<SyncStatus, String> {
        if self.keys.get()?.is_some() {
            return Err("this device already has a sync key".into());
        }
        let mut inner = self.inner.lock().unwrap();
        self.keys.set(&key)?;
        if let Err(e) = self.load_state(&mut inner) {
            inner.error = Some(e);
        }
        self.save_state(&inner)?;
        self.sync_locked(&mut inner);
        Ok(self.status_of(&inner))
    }

    /// The key as text, to pair another device.
    pub fn show_key(&self) -> Result<String, String> {
        Ok(envelope::key_to_text(&self.key()?))
    }

    pub fn set_folder(&self, dir: &Path) -> Result<SyncStatus, String> {
        if !dir.is_dir() {
            return Err(format!("{} is not a folder", dir.display()));
        }
        let mut inner = self.inner.lock().unwrap();
        inner.config.folder = Some(
            dir.to_str()
                .ok_or("the folder's path is not valid UTF-8")?
                .to_owned(),
        );
        write_config(&self.dir, &inner.config)?;
        self.sync_locked(&mut inner);
        Ok(self.status_of(&inner))
    }

    pub fn sync_now(&self) -> SyncStatus {
        let mut inner = self.inner.lock().unwrap();
        self.sync_locked(&mut inner);
        self.status_of(&inner)
    }

    pub fn take_fetcher(&self) -> Result<SyncStatus, String> {
        self.change(|s, r| s.edit(&Edit::TakeFetcher, r, now_ms()).map(|_| ()))?;
        Ok(self.status())
    }

    /// Record a Plaid fetch from the fetch script's folder (design §1.4).
    /// Only the fetcher may; the state refuses anyone else.
    pub fn import_fetch(&self, raw: &Path) -> Result<SyncStatus, String> {
        let sync: SyncFile = read_json(
            &raw.join("transactions_sync.json"),
            "/transactions/sync dump",
        )?;
        let balances: BalanceFile = read_json(
            &raw.join("accounts_balance.json"),
            "/accounts/balance/get dump",
        )?;
        let meta: FetchMeta = read_json(&raw.join("fetch_meta.json"), "fetch metadata")?;
        let today = ephemeris_core::schema::Date::parse(&meta.fetched_on)
            .ok_or_else(|| format!("invalid fetched_on {:?}", meta.fetched_on))?;
        let last = self.inner.lock().unwrap().config.plaid.clone();
        let (batch, progress) =
            ephemeris_plaid::batch(&sync, &balances, today, Some(&meta), last.as_ref())?;
        self.change(|s, r| s.ingest(&batch, r, now_ms()).map(|_| ()))?;
        let mut inner = self.inner.lock().unwrap();
        if progress.is_some() {
            inner.config.plaid = progress;
            write_config(&self.dir, &inner.config)?;
        }
        Ok(self.status_of(&inner))
    }

    /// The synced feed and options, once bank data has arrived.
    pub fn feed(&self) -> Result<Option<SyncedFeed>, String> {
        let inner = self.inner.lock().unwrap();
        let Some(state) = inner.state.as_ref().filter(|s| s.fetched.is_some()) else {
            return Ok(None);
        };
        let (feed, _) = state.materialize()?;
        Ok(Some(SyncedFeed {
            feed_json: serde_json::to_string(&feed).map_err(|e| e.to_string())?,
            options_json: serde_json::to_string(&state.forecast_options())
                .map_err(|e| e.to_string())?,
        }))
    }

    pub fn queries(&self) -> Vec<Query> {
        let inner = self.inner.lock().unwrap();
        inner.state.as_ref().map_or_else(Vec::new, |s| {
            s.saved_queries()
                .map(|(id, q)| Query {
                    id: id.to_string(),
                    amount_cents: q.amount_cents,
                    by: q.by.to_string(),
                    label: q.label.clone(),
                })
                .collect()
        })
    }

    pub fn save_query(
        &self,
        amount_cents: i64,
        by: &str,
        label: &str,
    ) -> Result<Vec<Query>, String> {
        let by = ephemeris_core::schema::Date::parse(by)
            .ok_or_else(|| format!("{by:?} is not a date"))?;
        let query = SavedQuery {
            amount_cents,
            by,
            label: label.to_string(),
        };
        self.change(|s, r| s.edit(&Edit::AddQuery { query }, r, now_ms()).map(|_| ()))?;
        Ok(self.queries())
    }

    pub fn delete_query(&self, id: &str) -> Result<Vec<Query>, String> {
        let edit = Edit::DeleteQuery { id: id.into() };
        self.change(|s, r| s.edit(&edit, r, now_ms()).map(|_| ()))?;
        Ok(self.queries())
    }
}

fn write_config(dir: &Path, c: &Config) -> Result<(), String> {
    let text = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    folder::write_atomic(&dir.join("config.json"), text.as_bytes()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeris_sync_folder::MemoryKeys;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ephemeris-desktop-sync-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn device(base: &Path, name: &str, keys: MemoryKeys) -> Device {
        Device::open(base.join(name), Box::new(keys)).unwrap()
    }

    /// The fetch script's files for a tiny full fetch.
    fn dump(dir: &Path, seq: u64) {
        std::fs::create_dir_all(dir).unwrap();
        let txn = |id: &str, amount: f64, date: &str| {
            serde_json::json!({"transaction_id": id, "account_id": "chk", "amount": amount,
                "iso_currency_code": "USD", "date": date, "authorized_date": date,
                "pending": false, "pending_transaction_id": null, "merchant_name": "Shop",
                "name": "SHOP", "personal_finance_category": null})
        };
        let pages = serde_json::json!({"pages": [{"added": [
            txn("a", 12.5, "2026-10-01"), txn("b", 40.0, "2026-10-05")]}]});
        std::fs::write(dir.join("transactions_sync.json"), pages.to_string()).unwrap();
        std::fs::write(
            dir.join("accounts_balance.json"),
            r#"{"accounts":[{"account_id":"chk","type":"depository","balances":{"current":800.0,"iso_currency_code":"USD"}}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("fetch_meta.json"),
            format!(
                r#"{{"fetched_on":"2026-10-06","mode":"full","item_id":"item1","fetch_seq":{seq}}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn a_new_device_has_an_id_and_asks_to_pair() {
        let base = temp("new");
        let d = device(&base, "app", MemoryKeys::default());
        let s = d.status();
        assert!(!s.paired && s.folder.is_none() && !s.has_data);
        assert_eq!(s.replica.len(), 16);
        assert!(d.take_fetcher().unwrap_err().contains("no sync key"));
        // The id survives a restart.
        let again = device(&base, "app", MemoryKeys::default());
        assert_eq!(again.status().replica, s.replica);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn two_desktops_pair_sync_fetch_and_share_queries() {
        let base = temp("pair");
        let shared = base.join("shared");
        std::fs::create_dir_all(&shared).unwrap();

        let mac = device(&base, "mac", MemoryKeys::default());
        mac.create_key().unwrap();
        let key_text = mac.show_key().unwrap();
        mac.set_folder(&shared).unwrap();
        assert!(mac.create_key().unwrap_err().contains("already has"));

        // The Mac takes the fetcher role and imports a fetch.
        mac.take_fetcher().unwrap();
        dump(&base.join("raw"), 1);
        let s = mac.import_fetch(&base.join("raw")).unwrap();
        assert!(s.is_fetcher && s.has_data, "{s:?}");
        assert_eq!(s.fetched_through.as_deref(), Some("2026-10-06"));
        mac.save_query(150_000, "2026-11-15", "trip").unwrap();

        // A second desktop pairs with the key text and picks the folder.
        let laptop = device(&base, "laptop", MemoryKeys::default());
        laptop.pair(&key_text).unwrap();
        let s = laptop.set_folder(&shared).unwrap();
        assert!(s.has_data && !s.is_fetcher, "{s:?}");
        assert_eq!(s.devices.len(), 2, "{:?}", s.devices);
        assert!(s.devices.iter().all(|d| d.error.is_none()));
        assert_eq!(laptop.queries().len(), 1);
        assert_eq!(laptop.feed().unwrap(), mac.feed().unwrap());

        // The laptop is refused as a fetcher until it takes the role.
        dump(&base.join("raw2"), 2);
        let err = laptop.import_fetch(&base.join("raw2")).unwrap_err();
        assert!(err.contains("fetched by device"), "{err}");

        // A query deleted on the laptop goes from the Mac at its next sync.
        let id = laptop.queries()[0].id.clone();
        laptop.delete_query(&id).unwrap();
        mac.sync_now();
        assert!(mac.queries().is_empty());

        // State at rest is sealed: a restart with the key opens it.
        let mac_keys = MemoryKeys::default();
        mac_keys
            .set(&envelope::key_from_text(&key_text).unwrap())
            .unwrap();
        let restarted = device(&base, "mac", mac_keys);
        assert!(restarted.status().has_data);
        let raw = std::fs::read(base.join("mac/local.ephsync")).unwrap();
        assert!(
            !String::from_utf8_lossy(&raw).contains("SHOP"),
            "sealed at rest"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn folder_problems_show_in_the_status_instead_of_failing() {
        let base = temp("err");
        let shared = base.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let d = device(&base, "app", MemoryKeys::default());
        d.create_key().unwrap();
        d.set_folder(&shared).unwrap();
        std::fs::write(shared.join("0000000000000009.ephsync"), b"garbage").unwrap();
        let s = d.sync_now();
        let bad: Vec<_> = s.devices.iter().filter(|x| x.error.is_some()).collect();
        assert_eq!(bad.len(), 1);
        std::fs::remove_dir_all(&shared).unwrap();
        let s = d.sync_now();
        assert!(s.error.unwrap().contains("cannot read the sync folder"));
        assert!(d.set_folder(&base.join("nope")).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
