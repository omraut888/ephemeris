// What the app needs from a shell to sync (docs/phase4b-design.md §3, §4).
// The desktop app implements it with Tauri commands over the native sync
// (Keychain key, a chosen folder); the browser with the sync WASM module,
// loaded only once sync is used, and the File System Access API or manual
// file import and export. All merge rules are in Rust: TypeScript only moves
// opaque states and files around and draws.

/** One device's file in the sync folder. */
export interface SyncFile {
  file: string;
  /** The replica that wrote it, when it opened. */
  replica: string | null;
  thisDevice: boolean;
  /** Milliseconds since 1970. */
  modified: number | null;
  /** Why it was skipped (another key, damaged), when it was. */
  error: string | null;
}

export interface SyncStatus {
  /** This device holds the sync key (on the web: for this session). */
  paired: boolean;
  /** This device's replica id, 16 hex digits. */
  replica: string;
  /** The folder synced through, as the shell names it. */
  folder: string | null;
  /** The device allowed to fetch from the bank, once one has. */
  fetcher: string | null;
  isFetcher: boolean;
  /** Bank data has arrived, so there is a synced feed to show. */
  hasData: boolean;
  fetchedThrough: string | null;
  devices: SyncFile[];
  /** Milliseconds since 1970 of the last sync with the folder. */
  lastSync: number | null;
  error: string | null;
}

/** A saved "can I afford it" question. */
export interface SavedQuery {
  id: string;
  amountCents: number;
  /** YYYY-MM-DD. */
  by: string;
  label: string;
}

/** The synced feed and the forecast options the synced settings ask for. */
export interface SyncedFeed {
  feedJson: string;
  optionsJson: string;
}

export interface SyncHost {
  /** Which shell: the sheet offers what each can do. */
  readonly kind: "desktop" | "web";
  status(): Promise<SyncStatus>;
  /** A new random key, on the first device. */
  createKey(): Promise<SyncStatus>;
  /** The key copied from another device. */
  pair(keyText: string): Promise<SyncStatus>;
  /** The key as text, to pair another device. */
  showKey(): Promise<string>;
  /** Choose the folder to sync through; the status unchanged if cancelled. */
  chooseFolder(): Promise<SyncStatus>;
  /** Merge every device's file in the folder and write this device's. */
  syncNow(): Promise<SyncStatus>;
  /** Make this device the one that fetches (desktop only). */
  takeFetcher?(): Promise<SyncStatus>;
  /** Record a Plaid fetch (desktop, when this device is the fetcher). */
  importFetch?(): Promise<SyncStatus>;
  /** Without folder access: merge sync files the user picks (web). */
  importFiles?(files: File[]): Promise<SyncStatus>;
  /** Without folder access: download this device's sync file (web). */
  exportFile?(): Promise<void>;
  /** Whether `chooseFolder` can work here (web: the File System Access API). */
  readonly canChooseFolder: boolean;
  /** Start loading what sync needs (web: the sync WASM), as the sheet opens. */
  prepare?(): void;
  feed(): Promise<SyncedFeed | null>;
  queries(): Promise<SavedQuery[]>;
  saveQuery(amountCents: number, by: string, label: string): Promise<SavedQuery[]>;
  deleteQuery(id: string): Promise<SavedQuery[]>;
}

/** How often to sync while the window has focus. */
export const SYNC_POLL_MS = 10_000;
