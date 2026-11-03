//! The native shells' side of sync (docs/phase4b-design.md §3): one sealed
//! file per device in a folder the user already syncs, and the sync key in
//! the macOS Keychain. Shared by the CLI and the desktop app; the browser
//! has its own (web/src/sync).
//!
//! Each device writes only its own file, `<replica>.ephsync`, through a
//! temporary file and a rename. It reads every `*.ephsync` in the folder,
//! the provider's conflict copies included ("… 2.ephsync"): each holds a
//! valid state, and merging one twice changes nothing. A file that cannot
//! be opened (another key, truncated, a newer format) is skipped and
//! reported, never deleted.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ephemeris_sync::envelope::{self, KEY_LEN, Key, NONCE_LEN};
use ephemeris_sync::{ReplicaId, SyncState};

/// The extension of sync files.
pub const EXTENSION: &str = "ephsync";

/// `N` random bytes from the operating system.
pub fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| format!("no randomness from the system: {e}"))?;
    Ok(b)
}

/// A new random sync key.
pub fn new_key() -> Result<Key, String> {
    random::<KEY_LEN>()
}

/// A new random replica id (never 0).
pub fn new_replica() -> Result<ReplicaId, String> {
    Ok(u64::from_le_bytes(random::<8>()?) | 1)
}

/// The file name a replica writes.
pub fn file_name(replica: ReplicaId) -> String {
    format!("{replica:016x}.{EXTENSION}")
}

// ------------------------------------------------------------- key stores

/// Where a device keeps the sync key.
pub trait KeyStore {
    fn get(&self) -> Result<Option<Key>, String>;
    fn set(&self, key: &Key) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

/// The macOS Keychain, through `security`, as a generic password. The key
/// goes in as its text form on `security -i`'s standard input, never in a
/// process's arguments, and comes out on a pipe.
pub struct Keychain {
    pub service: String,
    pub account: String,
}

impl Keychain {
    /// The key every native shell on this Mac shares: the desktop app and
    /// the CLI.
    pub fn sync_key() -> Self {
        Self {
            service: "ephemeris.sync".into(),
            account: "key".into(),
        }
    }

    fn safe(s: &str) -> bool {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    }

    fn security(&self, args: &[&str]) -> Result<std::process::Output, String> {
        std::process::Command::new("security")
            .args(args)
            .args(["-s", &self.service, "-a", &self.account])
            .output()
            .map_err(|e| format!("cannot run security (the Keychain tool): {e}"))
    }
}

/// `security`'s exit code for "no such item".
const NOT_FOUND: i32 = 44;

impl KeyStore for Keychain {
    fn get(&self) -> Result<Option<Key>, String> {
        let out = self.security(&["find-generic-password", "-w"])?;
        match out.status.code() {
            Some(0) => {
                let text = String::from_utf8(out.stdout)
                    .map_err(|_| "the Keychain's sync key is not text".to_string())?;
                envelope::key_from_text(text.trim()).map(Some)
            }
            Some(NOT_FOUND) => Ok(None),
            _ => Err(format!(
                "could not read the sync key from the Keychain ({})",
                out.status
            )),
        }
    }

    fn set(&self, key: &Key) -> Result<(), String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        if !(Self::safe(&self.service) && Self::safe(&self.account)) {
            return Err("unexpected characters in the Keychain service or account".into());
        }
        // Crockford base32 and dashes only: nothing that could end the
        // quoted value or start another command.
        let text = envelope::key_to_text(key);
        let mut child = Command::new("security")
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot run security (the Keychain tool): {e}"))?;
        let command = format!(
            "add-generic-password -U -s \"{}\" -a \"{}\" -w \"{text}\"\n",
            self.service, self.account
        );
        child
            .stdin
            .take()
            .expect("piped")
            .write_all(command.as_bytes())
            .map_err(|e| format!("cannot talk to security: {e}"))?;
        child.wait().map_err(|e| e.to_string())?;
        // `security -i` exits 0 even when a command fails: read it back.
        match self.get()? {
            Some(k) if &k == key => Ok(()),
            _ => Err("could not store the sync key in the Keychain".into()),
        }
    }

    fn delete(&self) -> Result<(), String> {
        let out = self.security(&["delete-generic-password"])?;
        match out.status.code() {
            Some(0) | Some(NOT_FOUND) => Ok(()),
            _ => Err(format!("could not delete the sync key ({})", out.status)),
        }
    }
}

/// A key held in memory: tests, and nothing else.
#[derive(Default)]
pub struct MemoryKeys(std::sync::Mutex<Option<Key>>);

impl KeyStore for MemoryKeys {
    fn get(&self) -> Result<Option<Key>, String> {
        Ok(*self.0.lock().unwrap())
    }
    fn set(&self, key: &Key) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(*key);
        Ok(())
    }
    fn delete(&self) -> Result<(), String> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

// ----------------------------------------------------------------- folder

/// One file found in the sync folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub file: String,
    /// The replica the file says wrote it, when it opened.
    pub replica: Option<ReplicaId>,
    pub modified: Option<SystemTime>,
    /// Why it was skipped, when it was.
    pub error: Option<String>,
}

/// What a sync with the folder did.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncReport {
    pub found: Vec<Found>,
    /// Files whose state was merged.
    pub merged: usize,
    /// This device's file, as written.
    pub wrote: PathBuf,
}

/// The sealed state as `replica`'s file in `dir`, via a temporary file and
/// a rename, so the folder never holds half a file.
pub fn write_own(
    dir: &Path,
    replica: ReplicaId,
    state: &SyncState,
    key: &Key,
) -> Result<PathBuf, String> {
    let nonce = random::<NONCE_LEN>()?;
    let sealed = envelope::seal(state, replica, key, &nonce);
    write_atomic(&dir.join(file_name(replica)), &sealed)
}

/// Write `bytes` to `path` through a hidden temporary file beside it and a
/// rename. The temporary name does not end in `.ephsync`, so no reader
/// takes it for a device's file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} is not a file path", path.display()))?;
    let tmp = path.with_file_name(format!(".{}.tmp", name.to_string_lossy()));
    fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path.to_path_buf())
}

/// Every `*.ephsync` file in `dir`, opened with `key`, in name order.
pub fn read_all(dir: &Path, key: &Key) -> Result<Vec<(Found, Option<SyncState>)>, String> {
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("cannot read the sync folder {}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|x| x == EXTENSION)
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    paths.sort();
    Ok(paths
        .into_iter()
        .map(|p| {
            let file = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let modified = fs::metadata(&p).and_then(|m| m.modified()).ok();
            let opened = fs::read(&p)
                .map_err(|e| format!("cannot read it: {e}"))
                .and_then(|bytes| envelope::open(&bytes, key));
            match opened {
                Ok((replica, state)) => (
                    Found {
                        file,
                        replica: Some(replica),
                        modified,
                        error: None,
                    },
                    Some(state),
                ),
                Err(e) => (
                    Found {
                        file,
                        replica: None,
                        modified,
                        error: Some(e),
                    },
                    None,
                ),
            }
        })
        .collect())
}

/// Merge every readable file in `dir` into `local`, then write `local` as
/// `replica`'s own file. A file that opens but will not merge (a reused
/// stamp, a bad value) is reported like one that will not open.
pub fn sync(
    dir: &Path,
    replica: ReplicaId,
    local: &mut SyncState,
    key: &Key,
) -> Result<SyncReport, String> {
    let mut found = Vec::new();
    let mut merged = 0;
    for (mut f, state) in read_all(dir, key)? {
        if let Some(s) = state {
            match local.merge(&s) {
                Ok(()) => merged += 1,
                Err(e) => f.error = Some(e),
            }
        }
        found.push(f);
    }
    let wrote = write_own(dir, replica, local, key)?;
    Ok(SyncReport {
        found,
        merged,
        wrote,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeris_sync::edits::Edit;
    use ephemeris_sync::state::Setting;

    fn temp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ephemeris-folder-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn edited(replica: ReplicaId, horizon: u64, now: u64) -> SyncState {
        let mut s = SyncState::new();
        s.edit(
            &Edit::Set {
                setting: Setting::HorizonDays,
                value: horizon,
            },
            replica,
            now,
        )
        .unwrap();
        s
    }

    #[test]
    fn devices_converge_through_the_folder() {
        let dir = temp("converge");
        let key = new_key().unwrap();
        let (a, b) = (0xa, 0xb);
        let mut sa = edited(a, 30, 1_000);
        let mut sb = edited(b, 60, 2_000);
        sync(&dir, a, &mut sa, &key).unwrap();
        let r = sync(&dir, b, &mut sb, &key).unwrap();
        assert_eq!(r.merged, 1, "b read a's file");
        sync(&dir, a, &mut sa, &key).unwrap();
        assert_eq!(sa.to_json(), sb.to_json());
        assert_eq!(sa.forecast_options().horizon_days, 60);
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names.len(),
            2,
            "one file per device, no temporaries: {names:?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn conflict_copies_are_merged_and_bad_files_are_reported_not_deleted() {
        let dir = temp("bad");
        let key = new_key().unwrap();
        let mut sa = edited(0xa, 30, 1_000);
        let own = write_own(&dir, 0xa, &sa, &key).unwrap();
        // A provider's conflict copy of another device's file.
        let sb = edited(0xb, 60, 2_000);
        let sealed = envelope::seal(&sb, 0xb, &key, &[1; 24]);
        fs::write(dir.join("000000000000000b 2.ephsync"), &sealed).unwrap();
        // A file from another key, a truncated file, and unrelated files.
        let other = envelope::seal(&sb, 0xc, &[9; 32], &[1; 24]);
        fs::write(dir.join("000000000000000c.ephsync"), other).unwrap();
        fs::write(dir.join("000000000000000d.ephsync"), &sealed[..20]).unwrap();
        fs::write(dir.join("notes.txt"), "hello").unwrap();
        fs::write(dir.join(".000000000000000e.ephsync.tmp"), "half").unwrap();

        let r = sync(&dir, 0xa, &mut sa, &key).unwrap();
        assert_eq!(r.merged, 2, "own file and the conflict copy");
        assert_eq!(sa.forecast_options().horizon_days, 60);
        let errors: Vec<(&str, &str)> = r
            .found
            .iter()
            .filter_map(|f| f.error.as_deref().map(|e| (f.file.as_str(), e)))
            .collect();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].1.contains("another key"), "{errors:?}");
        assert!(
            errors[1].1.contains("not an Ephemeris sync file"),
            "{errors:?}"
        );
        for f in [
            "000000000000000c.ephsync",
            "000000000000000d.ephsync",
            "notes.txt",
        ] {
            assert!(dir.join(f).exists(), "{f} was left alone");
        }
        assert_eq!(r.wrote, own);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_folder_is_an_error_that_names_it() {
        let mut s = SyncState::new();
        let err = sync(Path::new("/nonexistent/ephemeris"), 1, &mut s, &[0; 32]).unwrap_err();
        assert!(err.contains("/nonexistent/ephemeris"), "{err}");
    }

    #[test]
    fn randomness_and_names() {
        assert_ne!(new_key().unwrap(), new_key().unwrap());
        assert_eq!(new_replica().unwrap() & 1, 1);
        assert_eq!(file_name(0xab), "00000000000000ab.ephsync");
    }

    #[test]
    fn memory_keys_round_trip() {
        let k = MemoryKeys::default();
        assert_eq!(k.get().unwrap(), None);
        k.set(&[3; 32]).unwrap();
        assert_eq!(k.get().unwrap(), Some([3; 32]));
        k.delete().unwrap();
        assert_eq!(k.get().unwrap(), None);
    }

    /// The real Keychain, under a throwaway service name that is deleted
    /// again. macOS only; elsewhere there is no Keychain to test.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_keychain_stores_reads_and_deletes_a_key() {
        let kc = Keychain {
            service: format!("ephemeris.sync.test-{}", std::process::id()),
            account: "key".into(),
        };
        let key = new_key().unwrap();
        assert_eq!(kc.get().unwrap(), None);
        kc.set(&key).unwrap();
        assert_eq!(kc.get().unwrap(), Some(key));
        let newer = new_key().unwrap();
        kc.set(&newer).unwrap();
        assert_eq!(kc.get().unwrap(), Some(newer), "set replaces");
        kc.delete().unwrap();
        assert_eq!(kc.get().unwrap(), None);
        kc.delete().unwrap();
    }
}
