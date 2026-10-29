//! The recent-feeds list (docs/phase4-design.md §3): newest first, unique
//! by path, at most [`CAP`] entries. Plain functions over the list; the
//! file I/O is in [`RecentFile`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The most entries kept.
pub const CAP: usize = 8;

/// A feed opened before: what the menu shows and how to open it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentFeed {
    pub name: String,
    pub path: String,
}

/// The list from `recent.json`; anything unreadable is an empty list.
pub fn parse(text: &str) -> Vec<RecentFeed> {
    let mut list: Vec<RecentFeed> = serde_json::from_str(text).unwrap_or_default();
    list.truncate(CAP);
    list
}

/// `entry` moved (or added) to the top, the list capped.
pub fn touch(list: &[RecentFeed], entry: RecentFeed) -> Vec<RecentFeed> {
    let rest: Vec<RecentFeed> = list
        .iter()
        .filter(|e| e.path != entry.path)
        .take(CAP - 1)
        .cloned()
        .collect();
    std::iter::once(entry).chain(rest).collect()
}

/// The entries whose file still exists.
pub fn existing(list: &[RecentFeed], exists: impl Fn(&Path) -> bool) -> Vec<RecentFeed> {
    list.iter()
        .filter(|e| exists(Path::new(&e.path)))
        .cloned()
        .collect()
}

/// `recent.json` on disk; the lock keeps a read-modify-write whole.
pub struct RecentFile {
    path: PathBuf,
    lock: Mutex<()>,
}

impl RecentFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> Vec<RecentFeed> {
        std::fs::read_to_string(&self.path)
            .map(|t| parse(&t))
            .unwrap_or_default()
    }

    /// The list, without files that have gone.
    pub fn list(&self) -> Vec<RecentFeed> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        existing(&self.read(), Path::exists)
    }

    /// Whether `path` is on the list (the only paths the webview may name).
    pub fn contains(&self, path: &str) -> bool {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.read().iter().any(|e| e.path == path)
    }

    /// Record an open. Failing to save only loses the menu entry, so the
    /// error is returned for the caller to ignore or report.
    pub fn touch(&self, entry: RecentFeed) -> std::io::Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let list = touch(&self.read(), entry);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&list).map_err(std::io::Error::other)?;
        std::fs::write(&self.path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(n: usize) -> RecentFeed {
        RecentFeed {
            name: format!("f{n}.json"),
            path: format!("/data/f{n}.json"),
        }
    }

    #[test]
    fn newest_first_unique_and_capped() {
        let mut list = Vec::new();
        for n in 0..10 {
            list = touch(&list, feed(n));
        }
        let paths: Vec<_> = list.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            paths,
            [
                "f9.json", "f8.json", "f7.json", "f6.json", "f5.json", "f4.json", "f3.json",
                "f2.json"
            ]
        );
        // Opening one again moves it up instead of duplicating it.
        let list = touch(&list, feed(5));
        assert_eq!(list.len(), CAP);
        assert_eq!(list[0], feed(5));
        assert_eq!(list.iter().filter(|e| **e == feed(5)).count(), 1);
        assert_eq!(list[1], feed(9));
    }

    #[test]
    fn unreadable_files_read_as_empty() {
        assert_eq!(parse(""), vec![]);
        assert_eq!(parse("{not json"), vec![]);
        assert_eq!(parse(r#"{"path": "/x"}"#), vec![]);
        let long: Vec<_> = (0..20).map(feed).collect();
        assert_eq!(parse(&serde_json::to_string(&long).unwrap()).len(), CAP);
    }

    #[test]
    fn listing_drops_missing_files() {
        let list = vec![feed(1), feed(2), feed(3)];
        let kept = existing(&list, |p| p != Path::new("/data/f2.json"));
        assert_eq!(kept, vec![feed(1), feed(3)]);
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("ephemeris-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let feed_path = dir.join("feed.json");
        let file = RecentFile::new(dir.join("app/recent.json"));
        assert_eq!(file.list(), vec![]);

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&feed_path, "{}").unwrap();
        let entry = RecentFeed {
            name: "feed.json".into(),
            path: feed_path.to_str().unwrap().into(),
        };
        file.touch(entry.clone()).unwrap();
        assert_eq!(file.list(), vec![entry.clone()]);
        assert!(file.contains(&entry.path));
        assert!(!file.contains("/etc/passwd"));

        // A deleted feed leaves the menu but stays known until replaced.
        std::fs::remove_file(&feed_path).unwrap();
        assert_eq!(file.list(), vec![]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
