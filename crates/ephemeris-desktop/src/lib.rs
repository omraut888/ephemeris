//! The desktop shell (docs/phase4-design.md): the web UI in a Tauri window,
//! with the engine called natively.
//!
//! The engine commands are [`ephemeris_core::json`]'s two functions, the
//! same ones the WASM build exports, so the desktop app returns the same
//! bytes (the golden test below). Files are read here, never by the
//! webview: it can open the dialog, open a feed on the recent list, or
//! receive a dropped feed, but it cannot ask for an arbitrary path.

#![forbid(unsafe_code)]

mod recent;
mod sync;

use std::path::{Path, PathBuf};

use ephemeris_core::json;
use serde::Serialize;
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, Runtime, State, WindowEvent};
use tauri_plugin_dialog::DialogExt;

pub use recent::{RecentFeed, RecentFile};
pub use sync::{Device, Query, SyncStatus, SyncedFeed};

/// Larger files are refused unread; a year of transactions is under 1 MiB.
pub const MAX_FEED_BYTES: u64 = 32 * 1024 * 1024;

/// A feed read from disk, for the webview to forecast.
#[derive(Debug, Clone, Serialize)]
pub struct OpenedFeed {
    pub name: String,
    pub path: String,
    pub json: String,
}

/// Events the backend sends the webview.
pub mod events {
    /// A dropped feed, read: an [`OpenedFeed`](super::OpenedFeed).
    pub const FEED_OPENED: &str = "feed-opened";
    /// A dropped file that could not be read: the message.
    pub const FEED_ERROR: &str = "feed-error";
    /// A file is being dragged over the window (`true`) or left (`false`).
    pub const FEED_HOVER: &str = "feed-hover";
}

/// Read a feed file and record it as recent.
pub fn open_path(path: &Path, recent: &RecentFile) -> Result<OpenedFeed, String> {
    let shown = path.display();
    let size = std::fs::metadata(path)
        .map_err(|e| format!("{shown}: {e}"))?
        .len();
    if size > MAX_FEED_BYTES {
        return Err(format!(
            "{shown} is {} MiB; feeds over {} MiB are not opened",
            size / (1024 * 1024),
            MAX_FEED_BYTES / (1024 * 1024)
        ));
    }
    let json = std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}"))?;
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("{shown}: the path is not valid UTF-8"))?
        .to_owned();
    let name = path
        .file_name()
        .map_or_else(|| path_str.clone(), |n| n.to_string_lossy().into_owned());
    // Losing the menu entry is no reason to refuse the feed.
    let _ = recent.touch(RecentFeed {
        name: name.clone(),
        path: path_str.clone(),
    });
    Ok(OpenedFeed {
        name,
        path: path_str,
        json,
    })
}

/// Run CPU-bound work off the main thread, which would otherwise freeze
/// the window for the length of a forecast.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// A synthetic feed (JSON) from `SimulateOptions` (JSON).
#[tauri::command]
async fn simulate(options_json: String) -> Result<String, String> {
    blocking(move || json::simulate_json(&options_json)).await
}

/// A `ForecastReport` (JSON) from a feed (JSON) and `ForecastOptions` (JSON).
#[tauri::command]
async fn forecast(feed_json: String, options_json: String) -> Result<String, String> {
    blocking(move || json::forecast_json(&feed_json, &options_json)).await
}

/// The native open dialog; `None` when cancelled.
#[tauri::command]
async fn pick_feed<R: Runtime>(
    app: AppHandle<R>,
    recent: State<'_, RecentFile>,
) -> Result<Option<OpenedFeed>, String> {
    let Some(picked) = app
        .dialog()
        .file()
        .set_title("Open a feed")
        .add_filter("Feed JSON", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    open_path(&path, &recent).map(Some)
}

/// The recent feeds, newest first, without files that have gone.
#[tauri::command]
async fn recent_feeds(recent: State<'_, RecentFile>) -> Result<Vec<RecentFeed>, String> {
    Ok(recent.list())
}

/// Open a feed from the recent list; any other path is refused.
#[tauri::command]
async fn open_recent(path: String, recent: State<'_, RecentFile>) -> Result<OpenedFeed, String> {
    if !recent.contains(&path) {
        return Err(format!("{path} is not a recent feed"));
    }
    open_path(Path::new(&path), &recent)
}

// Sync (docs/phase4b-design.md §4): thin wrappers over `sync::Device`.

#[tauri::command]
async fn sync_status(d: State<'_, Device>) -> Result<SyncStatus, String> {
    Ok(d.status())
}

#[tauri::command]
async fn sync_create_key(d: State<'_, Device>) -> Result<SyncStatus, String> {
    d.create_key()
}

#[tauri::command]
async fn sync_pair(key: String, d: State<'_, Device>) -> Result<SyncStatus, String> {
    d.pair(&key)
}

#[tauri::command]
async fn sync_show_key(d: State<'_, Device>) -> Result<String, String> {
    d.show_key()
}

/// The native folder dialog; the status unchanged when cancelled.
#[tauri::command]
async fn sync_choose_folder<R: Runtime>(
    app: AppHandle<R>,
    d: State<'_, Device>,
) -> Result<SyncStatus, String> {
    let Some(picked) = app
        .dialog()
        .file()
        .set_title("Choose the folder to sync through (iCloud Drive, Dropbox…)")
        .blocking_pick_folder()
    else {
        eprintln!("[sync] choose folder: the dialog was cancelled; no folder chosen");
        return Ok(d.status());
    };
    d.set_folder(&picked.into_path().map_err(|e| e.to_string())?)
}

#[tauri::command]
async fn sync_now(d: State<'_, Device>) -> Result<SyncStatus, String> {
    Ok(d.sync_now())
}

#[tauri::command]
async fn sync_take_fetcher(d: State<'_, Device>) -> Result<SyncStatus, String> {
    d.take_fetcher()
}

/// Pick the fetch script's output folder and record the fetch.
#[tauri::command]
async fn sync_import_fetch<R: Runtime>(
    app: AppHandle<R>,
    d: State<'_, Device>,
) -> Result<SyncStatus, String> {
    let Some(picked) = app
        .dialog()
        .file()
        .set_title("Choose the Plaid fetch folder (scripts/fetch_plaid_sandbox.py --out)")
        .blocking_pick_folder()
    else {
        eprintln!("[sync] import fetch: the dialog was cancelled");
        return Ok(d.status());
    };
    d.import_fetch(&picked.into_path().map_err(|e| e.to_string())?)
}

#[tauri::command]
async fn sync_feed(d: State<'_, Device>) -> Result<Option<SyncedFeed>, String> {
    d.feed()
}

#[tauri::command]
async fn sync_queries(d: State<'_, Device>) -> Result<Vec<Query>, String> {
    Ok(d.queries())
}

#[tauri::command]
async fn sync_save_query(
    amount_cents: i64,
    by: String,
    label: String,
    d: State<'_, Device>,
) -> Result<Vec<Query>, String> {
    d.save_query(amount_cents, &by, &label)
}

#[tauri::command]
async fn sync_delete_query(id: String, d: State<'_, Device>) -> Result<Vec<Query>, String> {
    d.delete_query(&id)
}

/// Files dropped on the window: read the first and send it to the webview.
fn on_drag_drop<R: Runtime>(window: &tauri::Window<R>, event: &DragDropEvent) {
    match event {
        DragDropEvent::Enter { .. } => {
            let _ = window.emit(events::FEED_HOVER, true);
        }
        DragDropEvent::Leave => {
            let _ = window.emit(events::FEED_HOVER, false);
        }
        DragDropEvent::Drop { paths, .. } => {
            let _ = window.emit(events::FEED_HOVER, false);
            let Some(path) = paths.first() else { return };
            let recent = window.state::<RecentFile>();
            let _ = match open_path(path, &recent) {
                Ok(feed) => window.emit(events::FEED_OPENED, feed),
                Err(e) => window.emit(events::FEED_ERROR, e),
            };
        }
        _ => {}
    }
}

/// The app: commands, the dialog plugin, the recent list and drop handling.
/// `main` runs it with the real context; tests with a mock one.
pub fn configure<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir: PathBuf = app.path().app_data_dir()?;
            app.manage(RecentFile::new(dir.join("recent.json")));
            let keys = Box::new(ephemeris_sync_folder::Keychain::sync_key());
            app.manage(Device::open(dir.join("sync"), keys)?);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(e) = event {
                on_drag_drop(window, e);
            }
        })
        .invoke_handler(tauri::generate_handler![
            simulate,
            forecast,
            pick_feed,
            recent_feeds,
            open_recent,
            sync_status,
            sync_create_key,
            sync_pair,
            sync_show_key,
            sync_choose_folder,
            sync_now,
            sync_take_fetcher,
            sync_import_fetch,
            sync_feed,
            sync_queries,
            sync_save_query,
            sync_delete_query
        ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context};
    use tauri::webview::InvokeRequest;

    // The golden files and their inputs, shared with ephemeris-wasm's
    // native and WASM tests (docs/phase2-design.md §2).
    include!("../../ephemeris-wasm/tests/common/golden_inputs.rs");
    const GOLDEN_FEED: &str = include_str!("../../ephemeris-wasm/tests/golden/feed.json");
    const GOLDEN_REPORT: &str = include_str!("../../ephemeris-wasm/tests/golden/report.json");

    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// Call a command and get its JSON value, for commands that return objects.
    fn invoke_value(
        webview: &tauri::WebviewWindow<MockRuntime>,
        cmd: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        get_ipc_response(
            webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|b| b.deserialize::<serde_json::Value>().unwrap())
        .map_err(|e| e.as_str().unwrap_or_default().to_owned())
    }

    /// The sync commands are wired, take camelCase arguments and return
    /// camelCase objects, as web/src/sync expects.
    #[test]
    fn sync_commands_through_ipc() {
        let (_app, webview) = app();
        let s = invoke_value(&webview, "sync_status", serde_json::json!({})).unwrap();
        assert_eq!(s["paired"], false);
        assert_eq!(s["isFetcher"], false);
        assert!(s["replica"].as_str().unwrap().len() == 16);
        let err = invoke_value(
            &webview,
            "sync_save_query",
            serde_json::json!({ "amountCents": 1000, "by": "2026-12-01", "label": "x" }),
        )
        .unwrap_err();
        assert!(err.contains("no sync key"), "{err}");
        let s = invoke_value(&webview, "sync_create_key", serde_json::json!({})).unwrap();
        assert_eq!(s["paired"], true);
        let q = invoke_value(
            &webview,
            "sync_save_query",
            serde_json::json!({ "amountCents": 1000, "by": "2026-12-01", "label": "x" }),
        )
        .unwrap();
        assert_eq!(q[0]["amountCents"], 1000);
        assert_eq!(q[0]["by"], "2026-12-01");
        let feed = invoke_value(&webview, "sync_feed", serde_json::json!({})).unwrap();
        assert!(feed.is_null(), "no bank data yet");
        let key = invoke(&webview, "sync_show_key", serde_json::json!({})).unwrap();
        assert_eq!(key.replace('-', "").len(), 52);
    }

    fn app() -> (tauri::App<MockRuntime>, tauri::WebviewWindow<MockRuntime>) {
        app_with(Box::new(ephemeris_sync_folder::MemoryKeys::default()))
    }

    fn app_with(
        keys: Box<dyn ephemeris_sync_folder::KeyStore + Send + Sync>,
    ) -> (tauri::App<MockRuntime>, tauri::WebviewWindow<MockRuntime>) {
        let app = configure(mock_builder())
            .build(mock_context(tauri::test::noop_assets()))
            .unwrap();
        // `setup` only runs when the event loop starts; tests provide the
        // list themselves, in a file that is never written here.
        app.manage(RecentFile::new(
            std::env::temp_dir().join("ephemeris-ipc-test-no-recent.json"),
        ));
        // A sync device with an in-memory key: never the real Keychain.
        let sync_dir = std::env::temp_dir().join(format!(
            "ephemeris-ipc-test-sync-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&sync_dir);
        app.manage(Device::open(sync_dir, keys).unwrap());
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        (app, webview)
    }

    /// Call a command the way the webview does: camelCase JSON arguments
    /// in, the command's JSON response out.
    fn invoke(
        webview: &tauri::WebviewWindow<MockRuntime>,
        cmd: &str,
        args: serde_json::Value,
    ) -> Result<String, String> {
        get_ipc_response(
            webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|b| b.deserialize::<String>().unwrap())
        .map_err(|e| e.as_str().unwrap_or_default().to_owned())
    }

    /// The third leg of the byte-identity proof: through Tauri's IPC, the
    /// desktop app produces exactly the golden feed and report.
    #[test]
    fn ipc_output_is_byte_identical_to_the_golden_files() {
        let (_app, webview) = app();
        let feed = invoke(
            &webview,
            "simulate",
            serde_json::json!({ "optionsJson": SIMULATE_OPTIONS }),
        )
        .unwrap();
        assert!(feed == GOLDEN_FEED, "feed differs from the golden file");
        let report = invoke(
            &webview,
            "forecast",
            serde_json::json!({ "feedJson": feed, "optionsJson": FORECAST_OPTIONS }),
        )
        .unwrap();
        assert!(
            report == GOLDEN_REPORT,
            "report differs from the golden file ({} vs {} bytes)",
            report.len(),
            GOLDEN_REPORT.len()
        );
    }

    #[test]
    fn engine_errors_come_back_as_messages() {
        let (_app, webview) = app();
        let err = invoke(
            &webview,
            "forecast",
            serde_json::json!({ "feedJson": "{", "optionsJson": "{}" }),
        )
        .unwrap_err();
        assert!(err.starts_with("feed:"), "{err}");
    }

    #[test]
    fn open_recent_refuses_paths_not_on_the_list() {
        let (_app, webview) = app();
        let err = invoke(
            &webview,
            "open_recent",
            serde_json::json!({ "path": "/etc/passwd" }),
        )
        .unwrap_err();
        assert_eq!(err, "/etc/passwd is not a recent feed");
    }

    #[test]
    fn open_path_reads_records_and_limits() {
        let dir = std::env::temp_dir().join(format!("ephemeris-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let recent = RecentFile::new(dir.join("recent.json"));

        let small = dir.join("small.json");
        std::fs::write(&small, GOLDEN_FEED).unwrap();
        let opened = open_path(&small, &recent).unwrap();
        assert_eq!(opened.name, "small.json");
        assert_eq!(opened.json, GOLDEN_FEED);
        assert_eq!(recent.list()[0].path, opened.path);

        let big = dir.join("big.json");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_FEED_BYTES + 1).unwrap();
        let err = open_path(&big, &recent).unwrap_err();
        assert!(err.contains("over 32 MiB"), "{err}");
        assert_eq!(recent.list().len(), 1, "a refused file is not recorded");

        assert!(open_path(&dir.join("missing.json"), &recent).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
