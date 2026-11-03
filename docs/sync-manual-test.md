# Sync: hand-test checklist (Phase 4b)

What the automated tests cannot cover: the real desktop window, real
iCloud Drive, the real Keychain across two shells, and Chrome's folder
picker. Run the steps in order from the repository root. Each step gives
the exact command or click and what you should see. If a step's result
differs, note the step number and copy the exact text (the Sync sheet's
red message, or the terminal's `error:` line).

Background: docs/phase4b-design.md §3 (transport, key), §1.4 (one
fetcher), §10 (the adapters as built).

## 0. Setup

```sh
cd ~/projects/ephemeris
cargo build -p ephemeris-cli
E=./target/debug/ephemeris
SYNC="$HOME/Library/Mobile Documents/com~apple~CloudDocs/Ephemeris Sync"
APP="$HOME/Library/Application Support/dev.ephemeris.desktop"
```

| # | Do | Expect |
|---|---|---|
| 0.1 | `security find-generic-password -s ephemeris.sync -a key >/dev/null 2>&1 && echo "a key exists" \|\| echo "no key"` | `no key`. If a key exists, delete it with `security delete-generic-password -s ephemeris.sync -a key`. That only removes the test key. |
| 0.2 | `rm -rf "$APP/sync" data/sync` | Starts the desktop app and the CLI as new devices. `$APP/sync` holds only a device id from an earlier launch. |
| 0.3 | `mkdir -p "$SYNC" && ls -A "$SYNC"` | No output (an empty folder). |

## 1. Desktop: create a key

| # | Do | Expect |
|---|---|---|
| 1.1 | `cd web && npm run tauri dev` (in its own terminal; the first build takes a few minutes) | The EPHEMERIS window opens on the demo. |
| 1.2 | Click **Sync** (top right, after "Load feed…") | A "Sync" sheet with a key field, **Pair**, and "First device? **Create a new sync key**". |
| 1.3 | Click **Create a new sync key** | The sheet shows the rows Key (**Show key**), Folder ("none chosen", **Choose folder…**), Status ("Last synced never"), and Bank data ("None yet…", "No device fetches yet.", **Make this device the fetcher**). |
| 1.4 | `security find-generic-password -s ephemeris.sync -a key >/dev/null && echo present` | `present` (this checks that the key exists without printing it). |

## 2. Choose a folder inside iCloud Drive

| # | Do | Expect |
|---|---|---|
| 2.1 | Click **Choose folder…**, then pick **iCloud Drive › Ephemeris Sync** and click **Open** | Folder shows `/Users/<you>/Library/Mobile Documents/com~apple~CloudDocs/Ephemeris Sync`. Status shows "Last synced" with the current time, and a Devices list with one row, "This device". |
| 2.2 | `python3 -c 'import json,os;print(json.load(open(os.path.expanduser("~/Library/Application Support/dev.ephemeris.desktop/sync/config.json")))["replica"])'` | 16 hex digits: the desktop's device id. Note it as DESKTOP. |

## 3. The device file is ciphertext

| # | Do | Expect |
|---|---|---|
| 3.1 | `ls -l "$SYNC"` | One file, `<DESKTOP>.ephsync`. |
| 3.2 | `head -c 4 "$SYNC"/*.ephsync; echo` | `EPHS` (the format's magic bytes; the rest is encrypted). |
| 3.3 | `LC_ALL=C grep -a -c -e '"clocks"' -e '"version"' "$SYNC"/*.ephsync "$APP/sync/local.ephsync"` | `0` for both files: no JSON is readable in the shared file or in the desktop's local copy. |

## 4. Pair the CLI as a second device

| # | Do | Expect |
|---|---|---|
| 4.1 | In the sheet, click **Show key** and copy the text | 13 groups of 4 characters, e.g. `RNRA-CJ05-…`. |
| 4.2 | `$E sync key --pair --replace`, then paste the key and press Return | The prompt `Paste the sync key from another device, then press Return:`. Nothing shows while you paste. Then `The sync key stored in the Keychain.` (`--replace` is needed because the CLI and the desktop share this Keychain item, which already holds the key. This step exercises the pairing path.) |
| 4.3 | `$E sync key --pair --replace 'RNRA-CJ05'` | `error: unexpected argument 'RNRA-CJ05' found`: a key is never accepted on the command line. |
| 4.4 | `$E sync init` | `New replica <16 hex> in data/sync/replica.json`. Note the id as CLI. |
| 4.5 | `git check-ignore -v data/sync/replica.json; git status --short data` | `.gitignore:9:data/sync/	data/sync/replica.json`, then no status lines: the plaintext replica file cannot be committed. |
| 4.6 | `$E sync folder "$SYNC"` | `  merged   <DESKTOP>.ephsync  (<DESKTOP>)`, then `Merged 1 file(s); wrote …/<CLI>.ephsync`. |
| 4.7 | In the sheet, click **Sync now** | Devices shows "This device" and "Device <last 6 of CLI>", with no red text. |

## 5. Bank data, fetched by the CLI

| # | Do | Expect |
|---|---|---|
| 5.1 | `python3 scripts/fetch_plaid_sandbox.py --out /tmp/ephemeris-fetch` | Ends with `Fetch 1 (full) of item <id>`. The access token goes to the Keychain (service `ephemeris.plaid`), not to a file. |
| 5.2 | `grep -rl access-sandbox /tmp/ephemeris-fetch data; echo "exit $?"` | `exit 1`: no file holds the token. |
| 5.3 | `$E sync import-plaid --raw /tmp/ephemeris-fetch` | `Recorded full fetch 1 of item <id>: …`. The CLI is now the fetcher. |
| 5.4 | `$E sync folder "$SYNC"`, then **Sync now** in the sheet | Bank data shows "Fetched through <date>" and "Device <last 6 of CLI> fetches from the bank.", and a **Show synced data** button appears. |
| 5.5 | Click **Show synced data** | The sheet closes. The header reads `synced · through <date>`, and the cone is drawn from the synced data. |

## 6. Conflicting edits, then convergence

The desktop has no override control in 4b: overrides are CLI-only, and
their UI is Phase 4c (design §4.4). So 6a makes the conflicting override
on two CLI devices, and 6b makes a conflicting edit between the desktop
and the CLI on something both can edit, a saved query.

**6a. Override conflict (CLI A against CLI B)**

| # | Do | Expect |
|---|---|---|
| 6.1 | `B=data/sync/replica-b.json; $E sync --state $B init && $E sync --state $B folder "$SYNC"` | A second CLI device. It merges the desktop's and CLI A's files: `Merged 2 file(s)`. |
| 6.2 | `$E sync list \| head -3` | Recent transactions. The last column is the bank id; copy one as ID. Its kind column shows `tagged`. |
| 6.3 | `$E sync override ID transfer`, then `$E sync --state $B override ID scheduled` (without syncing in between) | `Done` twice. B's edit is the later one. |
| 6.4 | `$E sync folder "$SYNC"; $E sync --state $B folder "$SYNC"; $E sync folder "$SYNC"` | Each lists the merged files with no `skipped` lines. |
| 6.5 | `$E sync list \| grep ID; $E sync --state $B list \| grep ID` | Both show `scheduled      override`: the later edit won on both devices. |
| 6.6 | `diff <($E sync list) <($E sync --state $B list) && echo converged` | `converged`. |

**6b. Saved query: deleted on the desktop, edited in the CLI**

| # | Do | Expect |
|---|---|---|
| 6.7 | In the app's "Can I afford it?" panel, enter `2000`, any date, and the label `trip`, then click **Save** | A chip, `$2,000.00 by <date> · trip`. |
| 6.8 | `$E sync folder "$SYNC" && $E sync export --plain --out /tmp/ephemeris-q.json && python3 -c "import json;print(list(json.load(open('/tmp/ephemeris-q.json'))['queries']))"` | One id, `['<DESKTOP>-1']`. Note it as Q. |
| 6.9 | `$E sync query-edit Q --amount 2500 --by 2026-12-31 --label trip2` (do not run `sync folder`) | `Done`. |
| 6.10 | Then, in the app, click **×** on the chip | The chip goes. The delete is later than the CLI's edit. |
| 6.11 | `$E sync folder "$SYNC"`, then **Sync now** in the sheet | No chip in the app. |
| 6.12 | `$E sync export --plain --out /tmp/ephemeris-q.json && python3 -c "import json;q=json.load(open('/tmp/ephemeris-q.json'))['queries'];print({k:v['value'] for k,v in q.items()})"` | `{'<DESKTOP>-1': None}`: deleted on both. The later write won, and no conflict dialog appeared. |

## 7. Take the fetcher role on the desktop; the CLI is refused

| # | Do | Expect |
|---|---|---|
| 7.1 | In the sheet, click **Make this device the fetcher** | "This device fetches from the bank." and an **Import a Plaid fetch…** button. |
| 7.2 | `$E sync folder "$SYNC"` | The CLI merges the handover. |
| 7.3 | `python3 scripts/fetch_plaid_sandbox.py --out /tmp/ephemeris-fetch --refresh` | `Fetch 2 (incremental) of item <id>`. |
| 7.4 | `$E sync import-plaid --raw /tmp/ephemeris-fetch; echo "exit $?"` | Exactly the error below, then `exit 1`. |
| 7.5 | `$E sync show \| sed -n 3p` | The same record count as before 7.4: the refused fetch changed nothing. |
| 7.6 | (Optional) `python3 scripts/fetch_plaid_sandbox.py --out /tmp/ephemeris-fetch --full`, then **Import a Plaid fetch…** in the sheet: press ⌘⇧G, type `/tmp/ephemeris-fetch`, and click **Open** | "Fetched through <today>". The desktop imports as the fetcher. It needs a full fetch, because it never imported fetch 1. |

The expected error at 7.4:

```
error: bank data for this sync state is fetched by device <DESKTOP>, not this one (<CLI>). Only one device may fetch, or an older bank state could win (docs/phase4b-design.md §1.4). Hand the fetcher role to this device first.
  On this device: ephemeris sync take-fetcher
```

## 8. The web app in Chrome, same folder

| # | Do | Expect |
|---|---|---|
| 8.1 | `cd web && npm run dev` (its own terminal; it builds both WASM modules first) | `Local: http://localhost:5173/`. |
| 8.2 | In Chrome, open DevTools › Network, filter `wasm`, and load `http://localhost:5173/` | The demo loads, and only `ephemeris_wasm_bg…wasm` appears (the engine, from the worker). |
| 8.3 | Click **Sync** | `ephemeris_sync_wasm_bg…wasm` loads now, and not before. The sheet notes "This browser keeps the key only until you close the tab." |
| 8.4 | Paste the key (from **Show key** in the desktop app) and click **Pair** | The paired sheet. Folder shows "none chosen", **Choose folder…**, and below it the note "Or, if the browser will not open your sync folder (it refuses some, such as iCloud Drive), move the files by hand…" with **Import sync files…** and **Export this device's file**. There is no fetcher button: browsers never fetch. |
| 8.5 | Click **Choose folder…**, pick **iCloud Drive › Ephemeris Sync**, and allow Chrome's prompt to view and edit the files | **Either A:** Devices lists four files (the desktop, CLI A, CLI B and "This device") with no red text, and Bank data shows "Device <last 6 of DESKTOP> fetches from the bank." Note A, then go to 8.6. **Or B:** Chrome refuses the folder (it blocks some folders under `~/Library`, where iCloud Drive lives), either in its own dialog or by the sheet showing a red message that starts "The browser would not use that folder." and names **Import sync files…** and **Export this device's file**, with Chrome's reason in parentheses. Note B, copy the message, and continue with 8.5b. A cancelled picker shows no message. |
| 8.5b | *(Only for B.)* Click **Import sync files…**. In the file dialog, open **iCloud Drive › Ephemeris Sync**, press ⌘A to select every `.ephsync` file, and click **Open** | Devices lists the three files (the desktop, CLI A, CLI B) with no red text. Bank data shows "Device <last 6 of DESKTOP> fetches from the bank." |
| 8.5c | *(Only for B.)* Click **Export this device's file**, then run `mv ~/Downloads/*.ephsync "$SYNC"/ && ls "$SYNC"` | Chrome downloads `<16 hex>.ephsync`, this browser's device file. After the move the folder lists four files. Click **Sync now** in the desktop app's sheet: Devices now has four rows, all without red text. |
| 8.6 | Click **Show synced data** | The header reads `synced · through <date>`. The horizon select shows the synced horizon. |
| 8.7 | Save a query (`500`, any date, label `web`). *For B:* click **Export this device's file** again and repeat the `mv` from 8.5c, since a browser without the folder cannot write there. Then click **Sync now** in the desktop app's sheet | The desktop app shows the `web` chip too. |
| 8.8 | `for f in "$SYNC"/*.ephsync; do printf '%s  ' "$(head -c 4 "$f")"; basename "$f"; done` | Four lines, each `EPHS  <id>.ephsync`: all four device files are ciphertext. |
| 8.9 | Reload the tab, then click **Sync** | The sheet asks for the key again: it was not kept. |
| 8.10 | DevTools › Application: Local storage, Session storage and IndexedDB for `localhost:5173` | Local storage holds only `ephemeris.sync.replica` (a device id, not secret). Session storage and IndexedDB are empty. |

## 9. Clean up

```sh
python3 scripts/fetch_plaid_sandbox.py --out /tmp/ephemeris-fetch --forget   # removes the sandbox item and its token
security delete-generic-password -s ephemeris.sync -a key                    # the test sync key
rm -rf data/sync "$APP/sync" /tmp/ephemeris-fetch /tmp/ephemeris-q.json "$SYNC"
```

In Chrome: DevTools › Application › Storage › **Clear site data** for
`localhost:5173`. Stop `tauri dev` and `npm run dev` with Ctrl-C.
