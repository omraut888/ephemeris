// The desktop app's sync: Tauri commands over the native sync module
// (crates/ephemeris-desktop/src/sync.rs). The key stays in the Keychain and
// the files are read in Rust; the webview only asks.

import { invoke } from "@tauri-apps/api/core";

import type { SavedQuery, SyncHost, SyncStatus, SyncedFeed } from "../sync/host";
import type { Invoke } from "./shell";

export function tauriSync(call: Invoke = invoke): SyncHost {
  return {
    kind: "desktop",
    canChooseFolder: true,
    status: () => call<SyncStatus>("sync_status"),
    createKey: () => call<SyncStatus>("sync_create_key"),
    pair: (key) => call<SyncStatus>("sync_pair", { key }),
    showKey: () => call<string>("sync_show_key"),
    chooseFolder: () => call<SyncStatus>("sync_choose_folder"),
    syncNow: () => call<SyncStatus>("sync_now"),
    takeFetcher: () => call<SyncStatus>("sync_take_fetcher"),
    importFetch: () => call<SyncStatus>("sync_import_fetch"),
    feed: () => call<SyncedFeed | null>("sync_feed"),
    queries: () => call<SavedQuery[]>("sync_queries"),
    saveQuery: (amountCents, by, label) =>
      call<SavedQuery[]>("sync_save_query", { amountCents, by, label }),
    deleteQuery: (id) => call<SavedQuery[]>("sync_delete_query", { id }),
  };
}
