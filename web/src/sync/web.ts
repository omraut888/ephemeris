// The browser's sync (docs/phase4b-design.md §3, §10): the sync WASM module,
// loaded only once sync is used, over the File System Access API where the
// browser has it (Chrome, Edge) and manual import and export elsewhere.
//
// - The key lives in memory for this tab only. It is never written to
//   storage; the user enters it each session.
// - The state lives in memory too, and persists through the sync folder (or
//   the exported file). Only this browser's replica id, which is not
//   secret, is kept in localStorage, so a returning browser is the same
//   device rather than a new file each visit.
// - TypeScript never decodes a state: it passes the opaque string between
//   the module's functions.

import type { SavedQuery, SyncFile, SyncHost, SyncStatus, SyncedFeed } from "./host";

type SyncWasm = typeof import("../wasm/sync-pkg/ephemeris_sync_wasm.js");

let module: Promise<SyncWasm> | null = null;

/** The sync module, fetched and instantiated on first use. */
export function loadSyncWasm(): Promise<SyncWasm> {
  module ??= import("../wasm/sync-pkg/ephemeris_sync_wasm.js").then(async (m) => {
    await m.default();
    return m;
  });
  return module;
}

/** Use a module already instantiated (tests). */
export function setSyncWasm(m: SyncWasm): void {
  module = Promise.resolve(m);
}

// The parts of the File System Access API used here, so tests can stand in.
export interface FileEntry {
  kind: "file";
  name: string;
  getFile(): Promise<File>;
}
export interface DirectoryEntry {
  kind: "directory";
  name: string;
}
export interface Writable {
  write(data: Uint8Array): Promise<void>;
  close(): Promise<void>;
}
export interface Directory {
  name: string;
  values(): AsyncIterable<FileEntry | DirectoryEntry>;
  getFileHandle(name: string, options?: { create?: boolean }): Promise<{
    createWritable(): Promise<Writable>;
  }>;
}

export interface WebSyncOptions {
  /** Where the replica id is kept; null when storage is unavailable. */
  storage?: Pick<Storage, "getItem" | "setItem"> | null;
  /** The folder picker; absent where the browser has none. */
  pickDirectory?: () => Promise<Directory>;
  /** Save bytes as a file the user downloads. */
  download?: (name: string, bytes: Uint8Array) => void;
  random?: (n: number) => Uint8Array;
  now?: () => number;
}

const REPLICA_KEY = "ephemeris.sync.replica";
const EXT = ".ephsync";

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** What to tell the user when the browser will not open or write the
 * folder: Chrome refuses some folders outright (those under ~/Library,
 * where iCloud Drive lives, among them), and permission can be denied. */
export const FOLDER_REFUSED =
  "The browser would not use that folder. Some folders are blocked (those under ~/Library, such as iCloud Drive), and access can be denied. Use “Import sync files…” to read the folder's files, and “Export this device's file” to save this device's file into it.";

function refused(e: unknown): Error {
  return new Error(`${FOLDER_REFUSED} (${message(e)})`);
}

/** Errors that mean the folder itself was refused, not a sync problem. */
const isRefusal = (e: unknown) =>
  e instanceof DOMException && (e.name === "NotAllowedError" || e.name === "SecurityError");

function browserDefaults(): WebSyncOptions {
  const w = typeof window === "undefined" ? undefined : (window as unknown as Record<string, unknown>);
  const picker = w?.showDirectoryPicker as
    | ((o: { mode: "readwrite" }) => Promise<Directory>)
    | undefined;
  let storage: Storage | null = null;
  try {
    storage = window.localStorage;
  } catch {
    storage = null;
  }
  return {
    storage,
    pickDirectory: picker ? () => picker.call(window, { mode: "readwrite" }) : undefined,
    download: (name, bytes) => {
      const url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: "application/octet-stream" }));
      const a = document.createElement("a");
      a.href = url;
      a.download = name;
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    },
  };
}

function hex(bytes: Uint8Array): string {
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function webSync(options: WebSyncOptions = browserDefaults()): SyncHost & { prepare(): void } {
  const random = options.random ?? ((n: number) => crypto.getRandomValues(new Uint8Array(n)));
  const now = options.now ?? (() => Date.now());

  // This browser's replica id: kept (not secret), or new for the session.
  const replica = (() => {
    try {
      const kept = options.storage?.getItem(REPLICA_KEY);
      if (kept && /^[0-9a-f]{16}$/.test(kept)) return kept;
    } catch {
      // Unavailable storage: a new id for this session.
    }
    const bytes = random(8);
    bytes[7]! |= 1; // never zero
    const id = hex(bytes);
    try {
      options.storage?.setItem(REPLICA_KEY, id);
    } catch {
      // As above.
    }
    return id;
  })();
  const own = `${replica}${EXT}`;

  let key: Uint8Array | null = null;
  let state: string | null = null;
  let dir: Directory | null = null;
  let devices: SyncFile[] = [];
  let lastSync: number | null = null;
  let error: string | null = null;

  const needKey = () => {
    if (!key || state === null) throw new Error("no sync key yet: create one, or pair with another device's");
    return { key, state };
  };

  async function status(): Promise<SyncStatus> {
    const base: SyncStatus = {
      paired: key !== null,
      replica,
      folder: dir?.name ?? null,
      fetcher: null,
      isFetcher: false,
      hasData: false,
      fetchedThrough: null,
      devices,
      lastSync,
      error,
    };
    if (state === null) return base;
    const w = await loadSyncWasm();
    const s = JSON.parse(w.sync_summary(state)) as {
      has_data: boolean;
      fetched_through: string | null;
      fetcher: string | null;
    };
    return {
      ...base,
      fetcher: s.fetcher,
      isFetcher: s.fetcher === replica,
      hasData: s.has_data,
      fetchedThrough: s.fetched_through,
    };
  }

  /** Merge one file's bytes into the state; a file that will not open or
   * merge is reported, not thrown. */
  function mergeFile(w: SyncWasm, name: string, modified: number | null, bytes: Uint8Array): SyncFile {
    const k = needKey();
    try {
      const opened = JSON.parse(w.sync_open(bytes, k.key)) as { replica: string; state: string };
      state = w.sync_merge(k.state, opened.state);
      return { file: name, replica: opened.replica, thisDevice: name === own, modified, error: null };
    } catch (e) {
      return { file: name, replica: null, thisDevice: name === own, modified, error: message(e) };
    }
  }

  function sealed(w: SyncWasm): Uint8Array {
    const k = needKey();
    return w.sync_seal(k.state, replica, k.key, random(24));
  }

  async function writeOwn(w: SyncWasm): Promise<void> {
    if (!dir) return;
    const handle = await dir.getFileHandle(own, { create: true });
    const out = await handle.createWritable();
    await out.write(sealed(w));
    await out.close();
  }

  async function syncNow(): Promise<SyncStatus> {
    if (!dir || !key || state === null) return status();
    const w = await loadSyncWasm();
    try {
      const found: SyncFile[] = [];
      for await (const entry of dir.values()) {
        if (entry.kind !== "file" || !entry.name.endsWith(EXT) || entry.name.startsWith(".")) continue;
        const file = await entry.getFile();
        found.push(mergeFile(w, entry.name, file.lastModified, new Uint8Array(await file.arrayBuffer())));
      }
      await writeOwn(w);
      if (!found.some((f) => f.thisDevice)) {
        found.push({ file: own, replica, thisDevice: true, modified: now(), error: null });
      }
      devices = found.sort((a, b) => a.file.localeCompare(b.file));
      lastSync = now();
      error = null;
    } catch (e) {
      // Read or write access refused after the pick (permission revoked,
      // or denied when the browser asked to save changes).
      error = isRefusal(e) ? message(refused(e)) : message(e);
    }
    return status();
  }

  async function begin(k: Uint8Array): Promise<SyncStatus> {
    const w = await loadSyncWasm();
    key = k;
    state ??= w.sync_new();
    return dir ? syncNow() : status();
  }

  async function edit(json: object): Promise<string | null> {
    const w = await loadSyncWasm();
    const k = needKey();
    const out = JSON.parse(w.sync_edit(k.state, JSON.stringify(json), replica, now())) as {
      state: string;
      query_id: string | null;
    };
    state = out.state;
    await writeOwn(w);
    return out.query_id;
  }

  async function queries(): Promise<SavedQuery[]> {
    if (state === null) return [];
    const w = await loadSyncWasm();
    const list = JSON.parse(w.sync_saved_queries(state)) as {
      id: string;
      amount_cents: number;
      by: string;
      label: string;
    }[];
    return list.map((q) => ({ id: q.id, amountCents: q.amount_cents, by: q.by, label: q.label }));
  }

  return {
    kind: "web",
    canChooseFolder: !!options.pickDirectory,
    prepare: () => void loadSyncWasm(),
    status,
    createKey: () => begin(random(32)),
    pair: async (text) => {
      const w = await loadSyncWasm();
      return begin(w.sync_key_from_text(text));
    },
    showKey: async () => {
      const w = await loadSyncWasm();
      return w.sync_key_to_text(needKey().key);
    },
    chooseFolder: async () => {
      if (!options.pickDirectory) throw new Error("this browser cannot open a folder; import and export the files instead");
      try {
        dir = await options.pickDirectory();
      } catch (e) {
        // Cancelling the picker is not an error.
        if (e instanceof DOMException && e.name === "AbortError") return status();
        // Anything else is the browser refusing the folder or the access.
        throw refused(e);
      }
      return syncNow();
    },
    syncNow,
    importFiles: async (files) => {
      const w = await loadSyncWasm();
      needKey();
      const found: SyncFile[] = [];
      for (const f of files) {
        found.push(mergeFile(w, f.name, f.lastModified, new Uint8Array(await f.arrayBuffer())));
      }
      devices = found;
      lastSync = now();
      error = null;
      return status();
    },
    exportFile: async () => {
      const w = await loadSyncWasm();
      options.download?.(own, sealed(w));
    },
    feed: async (): Promise<SyncedFeed | null> => {
      if (state === null || !(await status()).hasData) return null;
      const w = await loadSyncWasm();
      return { feedJson: w.sync_materialize(state), optionsJson: w.sync_forecast_options(state) };
    },
    queries,
    saveQuery: async (amountCents, by, label) => {
      await edit({ edit: "add_query", query: { amount_cents: amountCents, by, label } });
      return queries();
    },
    deleteQuery: async (id) => {
      await edit({ edit: "delete_query", id });
      return queries();
    },
  };
}
