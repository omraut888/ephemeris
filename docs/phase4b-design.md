# Phase 4b design: sync between the apps

Status: approved 2026-10-31 with five amendments, all folded in below:
- LWW orders by a hybrid logical clock, never wall time (§2.5);
- why we deviate from the spec's yrs suggestion (§2.3);
- what losing the sync key costs (§3.4);
- the Plaid access token lives in the macOS Keychain (§1.4);
- overrides are CLI-only in 4b, and their UI moves to 4c (§4.4).

Implementation follows §6.2 and pauses for review once the convergence
simulation passes, before any shell adapter. Scope: one person's devices (desktop,
iOS, web) share their data, offline first, with no server that can read
it. Android (3b) is not started and is not covered beyond "it would use
the FFI crate like iOS".

## 0. Summary

- **Sync the inputs, recompute everything else.** The forecast is a pure
  function of the feed and the options. The feed itself is a pure function
  of the bank records and the user's corrections. Only those, a few
  settings and the saved afford queries are synced (§1).
- **A small, hand-rolled state-based CRDT** in a new pure crate,
  `ephemeris-sync`. It is built only from last-writer-wins registers, a max
  and an interval union. Automerge would add 741 kB to the WASM, which is
  2.1× the size of the whole engine today. yrs would add 348 kB (§2,
  measured).
- **Transport: one encrypted file per device in a folder the user already
  syncs** (iCloud Drive, Dropbox). There is no server. Each device writes
  only its own file and merges everyone else's, so the provider's conflict
  copies cannot arise and would be harmless if they did. The files are
  sealed with XChaCha20-Poly1305 under a key that never leaves the user's
  devices. The cipher adds 12 kB to the WASM (§3).
- **Layout like the engine:** the crate is pure, with JSON in and out.
  Thin adapters per shell do the folder I/O, the keychain and the clock
  (§4).
- **Tests:** lattice-law unit tests, a seeded multi-replica simulation
  that must converge to identical bytes *and* to the bank's truth, and
  golden sync states that must give byte-identical forecasts natively, in
  WASM and through the FFI (§5).

## 1. What state is worth syncing

### 1.1 What is derived, and so never synced

| Derived | Derived from |
|---|---|
| Forecast report (estimate, cone, targets, cycle lows, regime changes) | feed + `ForecastOptions` (byte-identical on every shell already: phase4-design §2) |
| Detected pay schedule and bills | feed |
| **The schema-v1 feed itself** | bank records + overrides (below) |
| Afford answers | report for that query's target |

The third row matters most. The feed is not raw data. The Plaid adapter
(`crates/ephemeris-cli/src/plaid.rs`) builds it from the whole record set
at once:

- **Integer ids are positional.** Transaction ids number the Plaid string
  ids in sorted order (`plaid.rs:213`). Account indices and stream ids are
  numbered the same way. One new record renumbers everything after it.
- **Tags are global.** `kind` comes from transfer-pair matching, primary
  income detection and repeat detection across all records.

So two feed files cannot be merged record by record: the same Plaid
transaction has different `id`s in them. Feeds stay a derived artifact.
What is synced is the record set *behind* the feed, keyed by the bank's
stable string ids. Each device rebuilds the feed from it with the same
pure code (§4.2).

A cached forecast is never synced, even to save time. A forecast takes
33–64 ms natively (phase4-design §1), so recomputing it after a merge
costs nothing worth saving. A synced cache would also be a second copy
that could disagree with its inputs.

### 1.2 The real user state

| State | Keyed by | Written by | CRDT type |
|---|---|---|---|
| **Bank records**: one version per Plaid `transaction_id` (account, dates, pending flag, amount as reported, `pending_transaction_id`, merchant, name, category), or a removal tombstone | Plaid `transaction_id` (string) | whichever device fetches | LWW register per record |
| **Account snapshots**: kind, currency, current balance | Plaid `account_id` | fetcher | LWW register per account |
| **Latest fetch mark**: the fetch's `today` | — | fetcher | max of (`today`, HLC) |
| **History coverage**: the days that fetches have covered | — | fetcher | union of day intervals |
| **Per-record overrides**: "this is a transfer / bill / income / everyday spending", or cleared | Plaid `transaction_id` | user, any device | LWW register of `Option<FlowKind>` |
| **Per-counterparty rules**: "Venmo from Sam is never income", "City Water is a bill" | counterparty key (the adapter's normalized merchant-or-name) | user | LWW register of `Option<FlowKind>` |
| **Synced settings**: horizon, paths, **seed**, the afford default | setting name | user | LWW register per setting |
| **Saved afford queries**: amount (cents), date, label | query id (replica id + counter, never reused) | user | LWW register of `Option<Query>` (None = deleted) |

The seed is a synced setting on purpose. The same records, overrides and
seed give the same cone on every device. A per-device seed would make the
phone and the laptop show slightly different numbers for no reason.

**Never synced (per device):** theme, window geometry, the current view
(daily or lows), the desktop recent-files list (its paths are
machine-local), the replica's own id and clock, and the sync key. The Plaid
access token and `/transactions/sync` cursor are also never synced: they
are the fetcher's credentials (§1.4).

**Bank amounts are stored exactly as the bank reported them** (an `f64`,
sign flipped to inflows positive). The feed rounds them to cents when it
is built, as the adapter always has. No float takes part in a merge
either way, because registers are only compared by their stamps.

*Changed during step 1.* The design first said integer cents. But the
sandbox fetch has 392 amounts with sub-cent digits (e.g. 48.2542), and
the income detector sums *unrounded* amounts to rank streams. Rounding at
ingest could therefore change which stream counts as income. Keeping the
reported value makes the move byte-identical by construction rather than
by luck. User-authored amounts (saved queries) are integer cents.

### 1.3 Which conflicts can actually happen

**Bank records.** Every device observes the same bank, so devices cannot
disagree about the truth, only about how recently they looked.
- *Pending → posted.* Plaid gives the posted record a new id that names
  the pending one, and reports the pending id as removed. There are two
  keys, so there is no conflict. The materializer drops a pending record
  that any live posted record names, as the adapter does now (step 3),
  even if the pending record's removal has not arrived yet.
- *A pending amount changes* (a tip, a hotel hold). Plaid reports a
  modification. Two devices that fetched at different times hold two
  versions, and the later fetch wins (LWW by HLC).
- *A pending charge is cancelled.* Plaid reports a removal, kept as a
  tombstone. A stale device that still holds the pending version cannot
  bring it back: its version is older.
- *Two concurrent fetches.* Both see the bank within seconds of each
  other, and whichever HLC is larger wins. A version could lose only to an
  older one if the two devices' clocks are skewed by more than the time
  between the fetches. Even then, the next fetch corrects it. This is
  accepted, not prevented.
- *Different history depths* (one fetch went back 90 days, another 730).
  The coverage union keeps both. The materialized `history_start` is the
  earliest day from which coverage is unbroken through the latest
  `today`. Records before a gap are dropped, so the feed's rule "no
  transaction is dated earlier" holds.
- *Balance vs records.* `ledger_balance` is the sum of the depository
  accounts' latest snapshots. A fetch writes its balances and its records
  with the same HLC, so after any merge they come from the same fetch,
  except in the two-concurrent-fetches case above, which the next fetch
  closes.

**Overrides.** These are real concurrent edits.
- *Two devices reclassify the same record differently.* The later edit
  wins. No dialog: the spec (§04) promises "never produces a conflict
  dialog", and the losing edit was the user's own and older.
- *An override on a pending record, then it posts.* The posted record has
  a new id. Its effective override is the **newer** of its own register
  and its pending predecessor's register. A label put on the pending
  charge on the phone therefore carries over. A later edit on the posted
  record on the laptop still wins.
- *A record override vs a counterparty rule.* There is no conflict, by
  precedence: record override, then counterparty rule, then the adapter's
  tag.
- *An override on a transfer leg.* Transfers come in pairs. An override
  on one leg applies to its matched counterpart too, so the two legs
  cannot end up half a transfer.
- *An override on a record the bank later removes.* The override stays
  but does nothing. It is a few bytes, and it is not collected (§2.4).

**Settings.** Concurrent changes resolve last writer wins. They are
harmless: the loser sees the other device's horizon on the next merge.

**Saved queries.**
- *Concurrent edits.* The later one wins, for the whole query (amount and
  date are edited together).
- *Edit vs delete.* The later one wins. A delete followed by a concurrent
  but later edit brings the query back. For a saved question that is the
  less surprising loss.
- *Two adds.* They cannot collide: ids carry the replica id.

### 1.4 Who fetches

Plaid's secret cannot ship in an app, and in this phase only the Mac has
it (`.env`, the fetch script). Today the iPhone and the browser can only
open files. Sync makes **the Mac the fetcher**: it ingests Plaid pages
into the sync state, and the other devices receive records through sync.
More fetchers would also be safe, because ingesting the same pages twice
is idempotent. But a production Plaid link needs a server, and that is
out of scope.

**The Plaid access token lives in the macOS Keychain, never in a file.**
Today `scripts/fetch_plaid_sandbox.py` makes a new sandbox item on every
run and never saves the token. Incremental fetching (one item,
`/transactions/sync` from a saved cursor) needs the token to persist.
- It is stored as a generic password: service `ephemeris.plaid`, account
  the Plaid `item_id`.
- The script writes it by piping an `add-generic-password -U …` command
  into `security -i`, so the token never appears in a process's
  arguments. It reads it back with `find-generic-password -w`.
- It is never written to `.env`, `data/` or the sync state, and never
  printed.
- The cursor is not a secret. It is kept beside the fetched files in
  `data/sandbox/` (gitignored).
- `--forget` deletes the Keychain item.

This changes the fetch script only. Nothing on the iPhone or in the
browser ever holds the token.

## 2. CRDT approach

### 2.1 The options

**(A) Automerge**: a general JSON-document CRDT with Rust and WASM builds.
It keeps every change in a compact columnar history and has a sync
protocol built in.

**(B) yrs**: the Rust port of Yjs, which the spec's stack table names as
an example. Its strength is sequences and text.

**(C) Hand-rolled**: a state-based CRDT (CvRDT) shaped to the data in §1.2.

### 2.2 Size, measured

Method: a scratch crate exporting one `#[wasm_bindgen]` function that
uses each library minimally (create, write, encode, decode, merge).
`wasm-pack build --release` (wasm-bindgen + wasm-opt), with `lto = true`
and `codegen-units = 1` like the workspace. Native: the same crate as a
stripped arm64 macOS `cdylib`, standing in for the iOS static library's
share of the app binary. Measured 2026-10-31 on this Mac.

| Variant | WASM | WASM gzip -9 | Added to WASM | Native arm64 added | Crates in tree |
|---|---|---|---|---|---|
| Baseline (wasm-bindgen + serde_json) | 75,705 B | 38,082 B | — | — | ~21 |
| + automerge 0.5.12 | 817,057 B | 307,362 B | **+741 kB** (+269 kB gz) | **+762 kB** | ~82 |
| + yrs 0.21.3 | 423,854 B | 146,086 B | **+348 kB** (+108 kB gz) | **+381 kB** | ~54 |
| + chacha20poly1305 0.10 (XChaCha20-Poly1305) | 88,042 B | 42,662 B | **+12 kB** (+4.6 kB gz) | under one 16 kB page | ~38 |
| *For scale: the engine today (`ephemeris_wasm_bg.wasm`)* | *352,597 B* | *151,748 B* | | | |

With Automerge, the web app's WASM would grow from 353 kB to about
1.1 MB. Most of that would be the sync library, not the engine. iOS cares
less about 0.8 MB, but it still counts against the 2.8 MB app.

The hand-rolled design reuses `serde`/`serde_json`, which every shell
already links. Its size is not measured yet, because it does not exist.
The estimate is tens of kB of code. It is measured in the as-built
section.

### 2.3 Comparison

| | Automerge | yrs | Hand-rolled |
|---|---|---|---|
| WASM / native size | +741 / +762 kB | +348 / +381 kB | est. tens of kB, plus 12 kB cipher |
| New crates | ~60 | ~33 | 1 (chacha20poly1305 and its RustCrypto deps, ~17 in tree) |
| Fits the data | Generic maps, lists, text. We would still encode "posted supersedes pending", "a tombstone beats a stale version" and the override precedence in our own materializer. | Same, and it is built for text, which we have none of. | The types *are* the semantics. |
| History | Keeps every change. Truly forgetting a record (a bank correction, a user wiping data) means rewriting the document. | Keeps deleted items as tombstones. | Keeps only the current value per key and id-only tombstones. |
| Correctness argument | Trust the library. | Trust the library. | Ours, but small: each component is a known semilattice, and a product of semilattices is a semilattice. It is checked by law tests and the simulation (§5). |
| Exchange protocol | Built in, efficient over a network | Built in (state vectors) | Full-state files. That suits §3(a) at today's sizes (§3.5). Delta-state later if a relay comes. |
| Determinism across shells | Needs care (a stable materialization order) | Same | Canonical by construction (`BTreeMap`, stable encoding) |

**Recommendation: (C), hand-rolled.** The data is mostly bank-authored
and append-like, has a natural stable key, and needs no sequence or text
editing. Those are the cases where a library earns its size. The size is
the deciding cost on the web: Automerge would make the sync layer twice
the size of the engine. Revisit if the app gains free-text notes edited on
several devices at once, or if full-state files grow past a few MB.

**Why we deviate from the spec's yrs suggestion.** The spec's stack table
reads "CRDT library (e.g. `yrs`, the Rust port of Yjs)". That was an
example, written before the data was known. Measured against this data:
- **Size.** yrs adds 348 kB to the WASM (+108 kB gzipped). That is 99% of
  the 353 kB engine, nearly doubling what the browser downloads. It adds
  381 kB natively, and about 33 crates. The hand-rolled design plus its
  cipher is estimated at tens of kB, 12 kB of it the measured cipher.
- **Fit.** yrs is built for shared sequences and text (Yjs's document
  model). Ephemeris has no collaborative text. Its state is a keyed map of
  bank records plus a few registers, so yrs's core structure would go
  unused. The rules that matter (a posted record supersedes its pending
  one, a removal beats a stale version, override precedence) would still
  be ours, written on top of yrs maps.
- **Retention.** yrs keeps deleted map entries as tombstones in the
  document. Purging financial data that the bank withdrew is easier when
  the state holds only current values.
- **What we give up:** yrs's tested merge code and its state-vector sync
  protocol. The first is replaced by law tests and a convergence
  simulation (§5). The second is not needed for whole-file exchange
  (§3).

The spec's §04 promise (offline edits that merge without a conflict
dialog) still holds.

### 2.4 The design

```text
Hlc      = (wall_ms: u64, counter: u32, replica: u64)    // total order
Lww<T>   = (Hlc, T); join = the larger Hlc

SyncState {
  version:   u32,                                     // state schema version
  clocks:    BTreeMap<ReplicaId, Hlc>,                // max per replica (HLC floor after restore)
  records:   BTreeMap<TxnKey, Lww<RecordVersion>>,    // RecordVersion = Live(BankRecord) | Removed
  accounts:  BTreeMap<AccountKey, Lww<AccountSnapshot>>,
  fetched:   Option<(Date /*today*/, Hlc)>,           // join = max
  coverage:  DayIntervals,                            // join = union, kept normalized
  overrides: BTreeMap<TxnKey, Lww<Option<FlowKind>>>,
  rules:     BTreeMap<CounterpartyKey, Lww<Option<FlowKind>>>,
  settings:  BTreeMap<SettingName, Lww<SettingValue>>,
  queries:   BTreeMap<QueryId, Lww<Option<SavedQuery>>>,
}
```

- **Merge** is per field: the key-wise join of maps and the joins above.
  It is commutative, associative and idempotent, and stale or duplicate
  states change nothing.
- **Clocks.** Every write is stamped by the replica's hybrid logical
  clock (§2.5). The caller passes `now_ms`, so the crate reads no clock.
- **Tombstones** are id-only and never collected in 4b. Removed records
  are mostly cancelled pendings, a few per month, a few dozen bytes each.
  Collecting them safely would need every replica to acknowledge, which
  file sync cannot provide cheaply.
- **Validation on merge.** Each incoming state is checked (dates real,
  cents in range, ids non-empty, HLC replica matching the clock map) before
  any join. A bad file is refused whole and reported. It is never partly
  merged.
- **Version skew.** A state with a newer `version` is refused, never
  rewritten. Because each device writes only its own file, an older app
  cannot strip fields from a newer device's file.

### 2.5 The clock: a hybrid logical clock, never wall time

Every LWW register is ordered by a **hybrid logical clock** (HLC, Kulkarni
et al., 2014), with the replica id as the final tiebreak. Raw wall time is
never compared. It only feeds the HLC as a lower bound, so stamps stay near
real time.

A stamp is `(l, c, replica)`, compared lexicographically:
- `l` is milliseconds: the largest physical time seen so far.
- `c` is a counter that orders events within one `l`.
- `replica` is a random 64-bit id per install. Two distinct writes
  therefore never compare equal.

Each replica keeps its last stamp `(l, c)` in `SyncState.clocks`.

- **Local write** at physical time `pt`: `l' = max(l, pt)`. `c' = c + 1`
  if `l' == l`, else `0`.
- **Merge** (the receive rule): first the states join. Then the replica's
  `(l, c)` becomes the largest stamp anywhere in the merged `clocks`.
  Its next local write therefore stamps above everything it has seen,
  whatever its wall clock says.
- **Counter overflow:** at `c == u32::MAX`, `l` advances by 1 ms and `c`
  resets. `l` may run ahead of physical time; that is allowed.

What this guarantees:
1. **Causality beats clocks.** If an edit was made after its device had
   merged an earlier edit to the same key, it wins, even if its wall
   clock is hours behind.
2. **Concurrent edits** (neither device had seen the other's) resolve to
   the larger stamp, then the larger replica id. That is arbitrary but
   identical on every device.
3. **A clock that jumps back** (an NTP step, a manual change) never makes
   a replica's stamps go backward.
4. **A clock far in the future** pulls every replica's `l` forward when
   they merge. Ordering stays consistent. Stamps simply stop tracking real
   time until real time catches up, and `c` carries the order meanwhile.
   Remote stamps are not rejected for being far ahead: refusing a file
   would break convergence.

Fetched bank versions use the same stamps. Their one weak spot is in
§1.3: two *concurrent* fetches are ordered by stamp, not by which saw the
bank later. Both are real bank states, and the next fetch settles it.

**Tests** (§5):
- unit cases for both rules, overflow, and a backward-jumping clock;
- for each guarantee, a case with replicas whose clocks differ by ±1 h and
  ±1 day;
- the simulation runs every seed with per-replica skews drawn from
  {−1 day, −5 min, 0, +5 min, +1 day} and random backward jumps. It checks
  guarantee 1 against the true happens-before order, which the simulator
  tracks itself.

## 3. Transport

### 3.1 The options, simplest first

| | (a) File in a synced folder | (b) LAN peer-to-peer | (c) Relay server |
|---|---|---|---|
| Server to run | none | none | yes: hosting, uptime, cost, abuse |
| Works away from home | yes (provider's network) | no: both devices on one LAN at once | yes |
| Latency | the provider's (seconds to minutes; unmeasured) | sub-second when both are up | sub-second |
| Desktop | plain file I/O on a chosen folder | Bonjour + sockets | HTTPS/WebSocket |
| iOS | folder picker + security-scoped bookmark, file coordination; no iCloud entitlement or paid account needed | Local Network permission, Bonjour, foreground only | URLSession |
| Web | Chromium: File System Access API on a folder. Safari and Firefox: manual import/export of files | no: browsers cannot open raw sockets, and WebRTC needs a signaling server, i.e. (c) | WebSocket |
| Third party sees | ciphertext, file sizes, write times, device count | nothing off the LAN | ciphertext, sizes, timing, IPs |

**Recommendation: (a) for 4b.** It needs no server and no account beyond
one the user already has. A state-based CRDT is exactly what makes "drop
files in a folder" correct. (b) fails the web outright, and works only
when two devices are awake together. (c) is the path to live,
browser-first sync, and the same sealed states could ride on it later
unchanged. It is deferred until there is a reason to run a server
(production Plaid needs one anyway, §1.4).

What (a) gives up:
- **"Live" is the provider's speed.** Edits arrive in seconds to minutes,
  not instantly. The spec's "edits propagate live" is met only loosely.
- **Web is second class** outside Chromium: there the user moves files by
  hand.

### 3.2 Folder layout

```text
<user's folder>/Ephemeris Sync/
  <replica-id-hex>.ephsync        one per device, written only by that device
```

- Each device writes **only its own file**: a temp file, then an atomic
  rename, debounced about 2 s after the last edit.
- It reads **every** `*.ephsync`. That includes provider artifacts such
  as `… 2.ephsync` or `… (conflicted copy).ephsync`: each is a valid
  state, so merging it is safe. Re-reading the same file changes nothing.
- A truncated, foreign-key or unreadable file is skipped and reported,
  never deleted automatically.
- A device that is gone leaves a file whose state is already merged
  everywhere. "Forget device" deletes it, but nothing requires that.
- **Polling:** desktop stats the folder when the window gains focus and
  every 10 s while focused (no new file-watching dependency). iOS reads on
  launch, on becoming active, and on pull to refresh. iOS uses
  `NSFileCoordinator` for iCloud, and a not-yet-downloaded file
  triggers the download. There is no background sync in 4b.

### 3.3 Encryption

**At rest and in transit:** every file is sealed before it leaves the
process. The provider transports and stores only ciphertext. Its own TLS
is a second layer, not the one we rely on.

```text
file = "EPHS" | u8 envelope_version | u64 replica_id | 24-byte nonce
       | XChaCha20-Poly1305(key, nonce, aad = header, plaintext = canonical state JSON)
```

- **Key:** 32 random bytes, generated on the first device. It is shown
  once as a base32 string for the user to copy to their other devices
  (AirDrop, a password manager or typing it in). A QR code is later: it
  needs a dependency.
  - No password: a random key needs no KDF, and Argon2 would add size.
  - The key never goes to any server. What losing it costs is in §3.4.
- **Nonce:** random 192-bit, supplied by the shell. The crate stays free
  of `getrandom` and testable with fixed nonces. Random nonces are safe at
  this size for any number of writes.
- **The header is authenticated.** A file renamed onto another replica's
  name, or tampered with, fails to open.
- **Key storage:** Keychain on iOS and macOS. The desktop needs one
  small crate for it, justified in step 8 of §6. On the web the key is
  held in memory for the session and asked for on each visit. "Remember
  on this browser" is opt-in, with the plain caveat that anything else
  able to read this browser profile could then read it.
- **The local copy** of each device's state uses the same envelope and
  key. iOS also sets complete file protection. On the web it lives in
  IndexedDB, sealed.
- **Trust model:** every device holding the key is trusted. Revoking a
  lost device means a new key: generate it, re-pair the remaining
  devices, delete the old files. This is documented, not automated, in
  4b.
- **What still leaks to the provider:** the number of devices, the
  file sizes (which track the record count) and the write times. Padding
  sizes to buckets is cheap and could be added; it is not planned.

### 3.4 What losing the sync key costs

There is no recovery: no server holds the key, by design. Losing a phone
or a browser profile costs nothing while another device still has the key
(re-pair from it). If **every** copy is lost, the sync files are
unreadable for good. What that costs, per kind of state:

| State | After the key is lost |
|---|---|
| Bank records, account balances, history coverage | **Recoverable.** The Mac fetches them again from Plaid (§1.4). The Plaid token is in the Keychain, not under the sync key, so it survives. Records older than Plaid's history window (730 days requested) are gone. |
| Forecasts, detected bills and income, the feed | **Recoverable.** They are recomputed from the re-fetched records. |
| Per-record overrides and per-counterparty rules | **Lost.** They exist only in the sync state. |
| Synced settings (horizon, paths, seed) | **Lost.** Back to the defaults; the user sets them again. |
| Saved afford queries | **Lost.** |

The only real loss is user-authored state, and in 4b that is small.

Each device's local state is sealed with the same key. A device that still
opens its own local copy still has the key, so "lost the key" means
losing every device as well. Setup says this plainly when it shows the
key, and suggests keeping the key in a password manager.

`ephemeris sync export --plain` (CLI, step 4) writes the user-authored
state as plaintext JSON for the user's own backup. It is never written
anywhere automatically.

### 3.5 Sizes

The golden feed holds 674 records in 136 kB of JSON, about 200 B each.
The sync state stores a little more per record (string ids, category), so
it is estimated at 250–300 B each: about 0.3 MB for a year of a typical
account, sent whole on each write. That is fine over iCloud or Dropbox.
Delta files or compression can come later; neither is needed for 4b.

## 4. Where it lives

### 4.1 Crates

```text
ephemeris-core          the engine, unchanged; no Plaid, no crypto
ephemeris-sync   NEW    pure: SyncState, HLC, merge, ingest, materialize,
                        seal/open. Depends on core (json), serde,
                        serde_json, chacha20poly1305. No I/O, no clock,
                        no randomness.
ephemeris-cli           Plaid JSON → BankRecord batches; `ephemeris sync …`
ephemeris-wasm          + sync_* exports (JSON/bytes in and out)
ephemeris-ffi           + the same exports through UniFFI
ephemeris-desktop       folder adapter, Keychain, debounce, polling
ios/EphemerisKit        folder bookmark, NSFileCoordinator, Keychain
web/                    File System Access / manual files, IndexedDB
```

`ephemeris-sync` is a separate crate, not a module of core. The engine
stays free of crypto and of bank-record shapes, and a build that only
forecasts does not pay for sync. It is a default workspace member, since
it is small. Like the engine, it is pure: shells pass in bytes, `now_ms`
and nonces, and get bytes and JSON back.

### 4.2 Moving the tagging out of the CLI

Every shell must rebuild the feed identically. So the adapter's steps 3–5
(supersede pendings, number ids, tag transfers, income and bills, assign
streams) move from `ephemeris-cli/src/plaid.rs` into
`ephemeris-sync::materialize`. They are already pure functions over
records, using the core's schedule detector.
- The CLI keeps steps 1–2: parsing Plaid's JSON shapes and replaying sync
  pages into `BankRecord` batches.
- `BankRecord.category` keeps Plaid's `personal_finance_category`
  vocabulary, which the bill and transfer rules read. A second data source
  would map into it. This is the one place a Plaid notion crosses into the
  sync crate, and it is stated here rather than hidden.
- **Regression gate:** `ephemeris import-plaid` must produce a
  byte-identical feed before and after the move, on the sandbox fetch.

```text
SyncState ──materialize──▶ Feed (schema v1, canonical) ──forecast_json──▶ report
          (live records → supersede → tag → overrides/rules → ids/streams → cents→f64)
```

Overrides are applied **after** tagging. A rule therefore cannot change
which counterparty is detected as the primary employer. That would mean
feeding rules into the tagger, which is a reasonable later step, but not
in 4b.

### 4.3 The boundary API (same in WASM, FFI and Rust)

| Function | In | Out |
|---|---|---|
| `sync_new(replica_id)` | u64 | state JSON |
| `sync_edit(state, edit_json, now_ms)` | an override, rule, setting or query edit | state JSON |
| `sync_ingest(state, batch_json, now_ms)` | one fetch: records, removals, account snapshots, window | state JSON |
| `sync_merge(a, b)` | two states | state JSON |
| `sync_materialize(state)` | state | feed JSON + settings JSON |
| `sync_seal(state, key, nonce)` / `sync_open(bytes, key)` | | bytes / state JSON |

TypeScript and Swift never implement merge rules. As with the engine, they
load, call and draw.

### 4.4 What the UI gains

No shell has a transaction list today, and editing data was out of scope
in 4a.

**Overrides are CLI-only in 4b.** The crate models them fully, and the
apps show their *effect*, since the feed they forecast is materialized
with overrides applied. Setting one is done on the Mac:

```text
ephemeris sync override <state> <transaction_id> transfer|scheduled|income|discretionary|clear
ephemeris sync rule     <state> "<counterparty>" <kind>|clear
ephemeris sync list     <state> [--kind K] [--since DATE]   # ids, dates, amounts, effective kind and why
```

**4b's UI** is limited to:
- a **Save** action on the afford query, and a saved-query list;
- a **Sync** sheet: choose the folder, show or enter the key, list the
  devices with their last write, and show errors such as "a file from
  another key".

The web and desktop UIs share React, so that work happens once for both.
The iOS work happens once in SwiftUI.

**Phase 4c** adds the transactions list with a kind menu (record
override) and "apply to all from this payee" (rule), in all three shells.
The sync format does not change for it.

Opening a feed file ("Open feed…", the demo) stays as it is: a view of
that file, not imported into sync. Feed files have positional ids, so
they cannot be merged (§1.1).

## 5. Testing

**Unit (hand-computed cases, per CLAUDE.md):**
- HLC (§2.5): local and merge rules, counter overflow, a clock jumping
  back, and each guarantee across replicas with clocks skewed by ±1 h and
  ±1 day.
- Each join with small explicit tables.
- Interval-union normalization and `history_start` with gaps.
- Effective override with pending predecessors.
- Transfer-pair override propagation.
- Amounts survive a state round-trip bit for bit (`float_roundtrip`), and the feed rounds them as `import-plaid` did.

**Lattice laws**, on random states (seeded): for every component and for
`SyncState`: `merge(a,b) == merge(b,a)`,
`merge(merge(a,b),c) == merge(a,merge(b,c))`, `merge(a,a) == a`, compared
as **encoded bytes**, not as structs.

**Multi-replica simulation** (seeded with `rand_chacha`, already in the
graph, so no new dependency; a failure prints its seed):
- A simulated bank timeline: records arrive pending, some change amount,
  some are cancelled, most post under a new id that names the pending one,
  posted history extends backward on a deep fetch.
- 3–5 replicas. One or two act as fetchers. Each has a clock skew drawn
  from {−1 day, −5 min, 0, +5 min, +1 day}, and its clock jumps back now
  and then. Random steps:
  - a fetch of the bank "now";
  - a user edit (override, rule, setting, query add/edit/delete);
  - a delivery of some replica's file to another, which may be a **stale
    older copy**, duplicated, or out of order, as the provider's lag would
    deliver it.
- At the end, every replica receives everyone's latest file until
  nothing changes. The test then asserts:
  1. **Identical:** every replica's encoded state is byte-identical. So
     are its materialized feed and its `forecast_json` report (with few
     paths, to stay fast).
  2. **Correct, not just identical:** if the last fetch came after the
     bank's last event, the materialized live records equal the bank's
     final set. No cancelled pending survives; no superseded pending
     double-counts.
  3. **User intent:** for each key, the surviving edit is the one with the
     largest HLC among those made. Whenever one edit causally followed
     another on the same key (by the simulator's own happens-before
     tracking, not by stamps), the later one survives, whatever the
     skews.
- Several hundred seeds run in the normal test pass. A `--ignored` soak
  test runs thousands.

**Envelope:**
- open(seal(s)) == s;
- a wrong key, a flipped byte, a swapped replica id in the header, or a
  truncated file each fail;
- an unknown envelope version is refused.

**Cross-platform golden check.** Fixtures under
`crates/ephemeris-sync/tests/golden/`: two sealed replica files (fixed
test key, fixed nonces) and the expected report. Each shell opens both,
merges them in **its own order** (natively A then B, WASM B then A, Swift
A then B), materializes, forecasts, and compares the bytes with the
golden report.
- Native: `cargo test`.
- WASM: `wasm-pack test --node`, as the engine's golden test does now.
- iOS: `swift test --package-path ios/EphemerisKit` through the FFI on
  the Mac. No simulator is needed for this check.

**Adapters:**
- the desktop folder adapter against a temp dir: atomic writes,
  conflict-copy names, a truncated file, a foreign-key file;
- the TypeScript and Swift adapters with fake file systems, like their
  existing fake-engine tests;
- iCloud and Dropbox behavior: a manual check, recorded as not verified
  by tests.

## 6. Scope, order and costs

### 6.1 In and out of scope

**In:**
- the sync crate;
- moving the tagging into it;
- the CLI as fetcher (`ephemeris sync import-plaid`), with the Plaid
  token in the Keychain (§1.4);
- overrides and rules through the CLI (§4.4);
- folder transport and encryption on desktop and iOS;
- the web: File System Access on Chromium, manual import and export
  elsewhere;
- the two UI additions in §4.4 (saved queries, the Sync sheet).

**Out:**
- a relay or LAN transport;
- background sync on iOS;
- automated key rotation;
- QR pairing;
- tombstone collection;
- feeding rules into the tagger;
- the transactions list and override UI (Phase 4c);
- production Plaid;
- Android.

### 6.2 Order of work, after approval (each a small commit)

1. Create `ephemeris-sync` and move the tagging (steps 3–5 of the
   adapter) into it. Gate: `ephemeris import-plaid` on the sandbox fetch
   writes a byte-identical feed before and after.
2. HLC, `Lww`, intervals, `SyncState`, merge, with unit, skew and law
   tests.
3. Ingest of Plaid-shaped batches and `materialize`. CLI `sync init`,
   `sync import-plaid`, `sync merge`, `sync materialize`.
4. Overrides, rules, settings and queries, with their precedence tests;
   CLI `sync override`, `sync rule`, `sync list`, `sync export --plain`.
5. The envelope (adds `chacha20poly1305`; justified by §2.2: 12 kB, the
   standard RustCrypto AEAD, no unsafe in our code).
6. The multi-replica simulation.

**Checkpoint: stop and report here**, before any shell adapter.

7. The fetch script keeps the Plaid token in the Keychain and fetches
   incrementally (§1.4).
8. The WASM and FFI exports, and the cross-platform golden check.
9. Desktop: folder adapter, Keychain for the sync key (one small crate
   such as `keyring`, desktop only, justified with its measured size
   before adding), and the Sync sheet.
10. Shared React: saved queries, the Sync sheet, and the web adapter.
11. iOS: folder bookmark, file coordination, Keychain, and the same two
    views.
12. An as-built section here, with measured sizes and times.

### 6.3 Costs on this machine (M3, 8 GB, 18 GB free today)

| Item | Disk | Time | Basis |
|---|---|---|---|
| Size measurements for this doc (scratch crate, four variants, WASM + native) | 456 MB scratch `target/`, deleted afterwards; wasm-pack installed `wasm-bindgen-cli` 0.2.130 into its cache (63 MB total) | a few minutes | measured today |
| `ephemeris-sync` + chacha20poly1305 in the workspace `target/` | tens of MB | well under a minute cold | estimate: the RustCrypto crates are small, and the rest is already built |
| WASM growth | +12 kB cipher, plus tens of kB sync code | — | cipher measured; code estimated |
| Simulation tests, default pass | — | under ~30 s | estimate. The forecast runs in release mode even in dev builds (`opt-level = 3` for core), and with few paths |
| Desktop rebuild with the adapter | a few hundred MB, as in 4a | the Tauri incremental build | estimate from 4a |
| iOS: XCFramework rebuild + `swift test` | as in 3a (29 MB framework, ~0.4–0.6 GB target slices) | 42 s cold framework build | measured in 3a |
| iOS simulator UI check (step 11 only) | as in 3a | minutes; **run it alone**, the simulator saturates 8 GB | 3a |

**In total:** under 1 GB of new build output beyond what 3a and 4a already
use. Disk is the tighter limit (18 GB free). `cargo clean -p` on the
scratch and FFI slices and `rm -rf ios/build` recover space, as in 3a.

## 7. Decisions (approved 2026-10-31)

1. A hand-rolled CRDT, not Automerge or yrs (§2.3).
2. Folder transport (§3.1).
3. A random key with copy-paste pairing, no password (§3.3). What losing
   it costs is in §3.4.
4. The Mac is the only fetcher, and the Plaid token is kept in its
   Keychain (§1.4).
5. LWW is ordered by a hybrid logical clock with a replica-id tiebreak,
   never by wall time (§2.5).
6. Overrides are CLI-only in 4b, and the override UI is Phase 4c (§4.4).
