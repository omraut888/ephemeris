# Phase 4a design: the desktop app (Tauri v2)

Status: written before any desktop code (2026-10-29). Scope: the web app's
UI as a macOS desktop app built with Tauri v2. The forecast runs natively,
and files open through native dialogs, a recent list and drag-and-drop.
Sync is Phase 4b. Mobile (Phase 3) is not started.

## 0. Scope

**In scope:**
- The same UI as the web app, from the same `web/` sources (§4).
- The engine called natively (§1, §2).
- A native open dialog, a remembered list of recent feeds, and dropping a
  feed file on the window (§3).
- A local dev build (`tauri dev`) and an unsigned, release-built `.app`.

**Out of scope:**
- Sync (Phase 4b).
- Auto-update.
- Code signing and notarization.
- Installers (`.dmg`, `.msi`), and Windows and Linux builds. They are not
  ruled out, just not built or tested here.
- Editing data.
- Live Plaid.

## 1. Where the forecast runs

**Decision: (a) natively.** The desktop app runs the forecast as Tauri
commands that call `ephemeris-core` directly. The UI reaches them through
the `Engine` interface the web client already uses (§2).

The alternative, (b), would keep the existing WASM running in a Web Worker
inside the webview.

### Speed (measured, this machine: M3, 8 GB)

The test case is the golden feed (realistic generator, seed 7, 240 days),
2000 paths, seed 0, target $0: what the app asks for. Times are the median
of 5 runs after one warm-up.

| Horizon | Native (release, in-process) | WASM (release, Node 26 / V8) |
|---|---|---|
| 90 days | 33 ms | 42 ms |
| 180 days | 64 ms | 81 ms |

Native is about 1.3× faster, which is not much. The engine is one thread
of f64 arithmetic, and V8 compiles WASM well.

(b) would run under WKWebView's JavaScriptCore, not V8. Its WASM speed
there is unmeasured.

(a) adds a round trip through Tauri's IPC: a JSON string of about 17 kB
each way. That cost is measured after the build (§6).

**Speed alone does not decide this.** The case for (a) rests on three other
points:

- **Headroom only native has.** The Monte Carlo draws each path from its
  own counter-based ChaCha8 stream (Phase 1 §4), so paths could run on
  several threads and still give byte-identical results. Natively that is
  a thread pool. In WASM it needs `SharedArrayBuffer`, cross-origin
  isolation and a threads build. That is not planned for 4a; (a) keeps the
  option cheap.
- **Files are already native.** The dialog, the recent list and dropped
  paths all live in Rust (§3). With (a), the feed text goes from disk to
  the engine without passing through the webview. With (b), it would
  cross into JavaScript and then into WASM.
- **The spec's claim.** "The entire engine is written once, in Rust, then
  compiled to native mobile bindings, WASM for the web client, and a
  native desktop build via Tauri — one engine, three shells" (spec §01).
  - Under (a), the desktop shell is a native build of the same crate.
  - Under (b), the desktop app is the web app in a frame, and the claim
    would be true only of the web.
  - Mobile (Phase 3) also calls the core natively, through UniFFI. (a)
    makes desktop the first non-browser shell to do so.

### Code reuse

Both options leave the React UI unchanged.

- **(b) reuses more:** the worker, the WASM build and the client, untouched.
- **(a) adds:**
  - about 30 lines of Rust commands;
  - a small TypeScript `Port` that sends the worker's existing
    `Request`/`Response` messages over Tauri's `invoke` instead of
    `postMessage` (§2). The lane logic (one in flight per lane, superseded
    requests dropped before they run) is reused as is.

### One engine, proven

The byte-identity test grows a third leg. The golden feed and report must
come out byte for byte from:

1. native Rust (existing);
2. WASM under Node (existing);
3. the desktop app's Tauri commands, called through Tauri's IPC layer
   (`tauri::test`), including argument decoding and response encoding (new).

All three call one function (§2), so the test guards against any shell
adding its own step.

### Costs of (a), accepted

- A second engine path to keep correct. The golden test above covers it.
- Commands must not block the main thread. A synchronous Tauri command
  runs on the main thread, which would freeze the window for the length of
  a forecast. The engine commands are `async` and run the forecast on a
  blocking-task thread.

(b) stays possible behind the same interface, if a reason ever appears.
It is not built.

## 2. The engine behind the UI

### One JSON entry point in the core

`simulate_json` and `forecast_json` (JSON text in, JSON text out) live in
`ephemeris-wasm` today. They move into `ephemeris_core::json`, behind a
new `json` feature (`serde` plus `serde_json`). After the move:

- `ephemeris-wasm` re-exports them and wraps them in `wasm_bindgen`;
- the desktop crate wraps them in `#[tauri::command]`;
- the CLI is unchanged.

Every shell then calls the same function. The feature is off by default,
so a core build without JSON stays as lean as before. `serde_json` is
already in the graph through the CLI and WASM crates.

### Commands

Engine:
- `simulate(optionsJson) → feedJson`
- `forecast(feedJson, optionsJson) → reportJson`

Files (§3):
- `pick_feed() → OpenedFeed | null`
- `recent_feeds() → RecentFeed[]`
- `open_recent(path) → OpenedFeed`

Payloads:
- `OpenedFeed` is `{ name, path, json }`.
- `RecentFeed` is `{ name, path }`.

Strings cross the boundary, as in WASM (Phase 2 §2). Errors are the
core's messages, so the UI shows the same text in both shells.

### TypeScript

The web app's engine comes from `portEngine(port)`. Today the port is
the Web Worker. The desktop app passes a port whose `postMessage` sends the
same `Request` through `invoke` and delivers the reply as a `Response`.

`App` already takes an `Engine` prop, so it does not change. Choosing the
engine and the file host happens in `main.tsx`, by Vite mode:

- `vite --mode desktop` builds the desktop UI.
- The default mode builds the web app.

`import.meta.env.MODE` is a build-time constant, and the desktop modules
are behind a dynamic `import()` in the dead branch. So the web bundle
contains no Tauri code, and its `dist/` is unchanged byte for byte. This is
checked by building before and after.

## 3. Native file handling

### Open dialog

`pick_feed` opens the system file dialog from Rust (`tauri-plugin-dialog`),
filtered to `.json`. It reads the chosen file and returns its text with the
name and path.

The webview never gets a general file-read permission:
- no `fs` plugin;
- no dialog permission in JavaScript;
- no command that reads an arbitrary path it is given.

### Recent files

- Stored as `recent.json` in the app's data directory
  (`~/Library/Application Support/<identifier>/`).
- Up to 8 entries, newest first, unique by path.
- Each successful open (dialog, drop or recent) moves the file to the top.
- Listing drops entries whose file no longer exists.
- `open_recent(path)` reads only a path already on the list. The webview
  can name a recent file, but cannot ask for any other path.
- The list logic is plain Rust with unit tests. A corrupt or missing
  `recent.json` reads as an empty list.

The header shows a "Recent" menu next to "Load feed…" when the list is
non-empty.

### Drag-and-drop

Tauri's native drag-and-drop stays on.

- The Rust side handles `DragDrop` window events.
- On a drop it reads the first file, records it as recent and emits
  `feed-opened` (or `feed-error`) to the webview.
- Enter and leave events emit `feed-hover` so the UI can show the drop
  overlay it already has.

As with the dialog, the paths never come from JavaScript.

### Limits and errors

- Files over 32 MiB are refused before reading. A year of transactions is
  well under 1 MiB.
- Read and UTF-8 errors come back as messages, not panics.
- The UI's checks stay as they are:
  - its "not JSON" check;
  - the engine's schema check.

## 4. Layout: one frontend, two shells

```
crates/ephemeris-desktop/      the Tauri crate (in the Cargo workspace)
  Cargo.toml  build.rs
  tauri.conf.json              frontendDist ../../web/dist-desktop
  capabilities/default.json    core:default only
  icons/                       generated by `tauri icon` from the mark
  src/main.rs                  generate_context!() + run
  src/lib.rs                   commands, recent list, drop handling, tests
web/                           unchanged sources, shared
  src/desktop/                 Tauri port + file host (desktop mode only)
  vite.config.ts               outDir dist-desktop in desktop mode
  package.json                 + scripts dev:desktop, build:desktop, tauri
```

- **One `web/`.** The desktop UI is the same `App` with a different
  `Engine` and an optional file host. Nothing is forked or copied.
- **Workspace membership.** The crate joins the workspace (one
  `Cargo.lock`, one `target/`) but is not a default member. A plain
  `cargo build`, `cargo test` or `cargo clippy` at the root still builds
  only the core, CLI and WASM crates, so routine work never compiles Tauri
  on this 8 GB machine. `scripts/test-full.sh` adds
  `cargo test -p ephemeris-desktop` and clippy for it.
- **CLI.** The Tauri CLI is an npm dev-dependency of `web/`
  (`npm run tauri -- dev`). There is no global install and no
  `cargo install`.

## 5. Dependencies

| Crate / package | Why |
|---|---|
| `tauri` 2 | The shell. Required. |
| `tauri-build` 2 | Its build script (config, capabilities, icons). Required. |
| `tauri-plugin-dialog` 2 | The native open dialog. Writing it on `objc2` by hand is not worth it. |
| `serde`, `serde_json` | Command payloads and `recent.json`. Already in the graph. |
| `@tauri-apps/api` 2 | `invoke` and `listen` in the webview. Bundled only in desktop mode. |
| `@tauri-apps/cli` 2 | `tauri dev` and `tauri build`. Dev only. |

There is no `fs`, `shell`, `store` or updater plugin.

## 6. Verification

- **Byte identity through Tauri's IPC** (§1). Runs in `scripts/test-full.sh`.
- **Rust unit tests:**
  - the recent list: ordering, dedupe, cap, pruning, corrupt file;
  - `open_recent` refusing a path not on the list;
  - the size limit.
- **Web:**
  - the existing 31 tests pass unchanged;
  - new tests for the invoke port (requests map to commands, errors map
    to `Response`s, lanes still drop superseded requests);
  - the header's recent menu.
- **Web build:** `web/dist` is identical before and after.
- **This machine:**
  - `tauri dev` opens the window and the demo forecast renders.
  - A feed opens from the dialog, from the recent menu, and by drop.
  - `tauri build --bundles app` produces an unsigned `.app` that launches.
  - IPC round-trip time, measured.

## 7. As built

*(filled in after implementation)*
