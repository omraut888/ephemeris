# Phase 3a design: the iOS app

Status: design only, written 2026-10-29, not implemented. It waits for
approval. Scope: an iPhone app in SwiftUI over the same Rust core, with
the same behaviour as the web and desktop apps. Android is Phase 3b.

## 0. Principles

These are carried over from Phases 2 and 4a.

- **One engine.** Every number on screen comes from `ephemeris-core`: the
  estimate, the schedule, the cone, the pay-cycle lows and the afford
  probabilities. Swift parses JSON, calls the FFI, lays out views and
  formats money and dates. Swift does no statistics and no date
  arithmetic on forecast data.
- **One contract.** The app speaks the versioned JSON the CLI, WASM and
  desktop already use (docs/forecast-schema.md). A feed exported by
  `ephemeris import-plaid` opens in the app as is.
- **One entry point.** iOS calls `ephemeris_core::json::{simulate_json,
  forecast_json}` (Phase 4a §2), the same functions as the other shells,
  so its output is byte-identical (§6).

## 1. Bindings: `crates/ephemeris-ffi` with UniFFI

### The options

**A: JSON at the boundary.** UniFFI exports three functions over strings:

```rust
uniffi::setup_scaffolding!();

#[uniffi::export] pub fn schema_version() -> u32;
#[uniffi::export] pub fn simulate_json(options_json: String) -> Result<String, EngineError>;
#[uniffi::export] pub fn forecast_json(feed_json: String, options_json: String) -> Result<String, EngineError>;

#[derive(Debug, uniffi::Error)]
pub enum EngineError { Failed { message: String } }   // Swift: `throws`
```

Swift decodes the report with `Codable` types that mirror the schema.

**B: typed records.** The schema types derive `uniffi::Record` and
`uniffi::Enum`, and Swift receives generated structs directly.

| | A: JSON strings | B: typed records |
|---|---|---|
| Byte identity | The FFI returns the report's exact bytes: a fourth path against the golden files (§6). | No JSON crosses, so there is nothing to compare byte for byte. Only field-by-field equality is possible. |
| Core changes | None. The FFI crate wraps `ephemeris_core::json`. | A `uniffi` feature on the schema types. `Date` and the serde-tagged enums (`kind`) need custom-type mappings that duplicate their serde shape. |
| Swift types | Hand-written `Codable`, about 20 types, guarded by a drift test (below). | Generated, so no drift. |
| Feeds from Files | Already JSON: passed through untouched, as on web and desktop. | Must be parsed somewhere anyway. In practice that means a JSON entry point as well, so both paths exist. |
| Cost per forecast | One copy of a ~17 kB string plus a `JSONDecoder` pass. Under a millisecond next to a 30–60 ms forecast. | Many small allocations to lift every `f64` array into Swift. Similar or worse. |
| Versioning | The JSON is versioned (`schema_version`), as for every other shell. | Unversioned in-process types, fine inside one binary. |
| Android (3b) | The same three functions, generated for Kotlin. | The same, but Kotlin inherits the custom-type mappings too. |

**Decision: A**, as you leaned. B's one real advantage is generated Swift
types, and a drift test gets most of that safety without changing the
core. B would also give up the byte-identity check, which is the
strongest evidence that "one engine" is literally true.

### Swift types: hand-written `Codable`, plus a drift test

Generators were considered and rejected:

- **`typeshare`**: a second annotation system on types that already carry
  `ts-rs`.
- **`serde-generate`**: aimed at bincode formats, and it does not follow
  serde's JSON renames and tags.
- **JSON Schema (`schemars`) → `quicktype`**: a Node tool in the Swift
  build path, and generated code nobody reads.

The Swift side only *decodes* reports and *encodes* options. Feeds pass
through as text, as in TypeScript. So the hand-written set is:

- `ForecastReport` and everything it contains;
- `SimulateOptions` and `ForecastOptions`, with their enums.

That is roughly 20 types and 250 lines.

**The drift test** has two halves, mirroring the TypeScript one
(Phase 2 §3):

1. **Rust: an all-fields fixture.**
   - A new golden file, `report-all-fields.json`, is built in a Rust test
     from struct literals.
   - Every `Option` is `Some`, every `Vec` is non-empty, and every enum
     variant appears, enforced by an exhaustive `match` in the builder.
   - Adding a field or variant to the schema then fails to *compile* until
     the fixture includes it.
   - Changing its output fails the golden check until the file is
     regenerated (`UPDATE_GOLDEN=1`).
2. **Swift: a round-trip.** The test does the following:
   - decodes `report-all-fields.json` and the golden `report.json` into
     the `Codable` types;
   - re-encodes them;
   - compares the result with the original as JSON values: key sets
     exact, numbers as `Double`.

   It catches:
   - a field Swift lacks (dropped on re-encode);
   - a misspelled key (missing or failed decode);
   - an unknown enum variant (failed decode).

So a schema change cannot reach the app unnoticed. Rust forces the
fixture to include the change, and Swift then fails until its types
match.

### Calling it from Swift

- The FFI calls are synchronous and CPU-bound. An `EngineClient` actor runs
  them off the main actor, on a detached task.
- It reproduces the web client's behaviour:
  - **lanes:** one forecast in flight per lane; a newer request replaces
    the waiting one, which is cancelled with `superseded`;
  - **debounce:** horizon changes wait 150 ms.
- A forecast cannot be interrupted inside Rust, which is the same as on
  web and desktop.
- **Panics.** The core does not panic on bad input (a Phase 1 rule).
  If it ever did, UniFFI turns a Rust panic into a Swift error instead
  of crashing, provided the library is built with `panic = "unwind"`.
  That is the release default, and it stays that way.

### Crate

`crates/ephemeris-ffi`:

| Item | Setting |
|---|---|
| Dependencies | `ephemeris-core` (`json`) and `uniffi` 0.32, proc-macro mode with no UDL file |
| Crate types | `staticlib` for iOS; `cdylib` for the bindings generator; `lib` for Rust tests |
| `uniffi-bindgen` binary | Behind a `bindgen` feature, so its CLI dependencies (clap, askama and others) never enter the iOS libraries |
| Workspace | A member, but not a default one, like the desktop crate. `scripts/test-full.sh` runs its tests. |
| Rust tests | Error mapping, `schema_version`, and the golden files through the exported functions, natively |

## 2. Packaging: an XCFramework

`scripts/build-ios.sh` builds everything from the command line; Xcode
never runs Cargo.

1. **Build `ephemeris-ffi` in release** for three slices:

   | Slice | Target | Why |
   |---|---|---|
   | device | `aarch64-apple-ios` | the app on a phone |
   | simulator | `aarch64-apple-ios-sim` | the app and tests in the simulator |
   | macOS | `aarch64-apple-darwin` | `swift test` on the Mac in seconds, with no simulator (§6) |

   - `IPHONEOS_DEPLOYMENT_TARGET=17.0` matches the app, which avoids
     linker warnings.
   - `--sim-only` builds just the simulator and macOS slices for the
     everyday loop.
2. **Generate the Swift bindings** from the host `cdylib`:
   `cargo run -p ephemeris-ffi --features bindgen --bin uniffi-bindgen --
   generate --library … --language swift`. It writes:
   - `ephemeris_ffi.swift`;
   - `ephemeris_ffiFFI.h`;
   - `ephemeris_ffiFFI.modulemap`, which the script renames to
     `module.modulemap`.
3. **Assemble the framework.**
   `xcodebuild -create-xcframework -library <slice>/libephemeris_ffi.a
   -headers <headers dir>` for each slice, written to
   `ios/build/EphemerisFFI.xcframework`.
4. **Hand over the Swift bindings and golden files.** The script copies
   the generated Swift file into the Swift package, and the golden files
   into its test resources.

All outputs are generated and gitignored, like `web/src/wasm/pkg`. Running
`rm -rf ios/build` frees everything the iOS build wrote.

### How the app consumes it

The app uses a local Swift package, `ios/EphemerisKit`:

```
ios/
  project.yml                    XcodeGen spec (committed)
  EphemerisKit/                  Swift package (committed, except generated files)
    Package.swift                binaryTarget EphemerisFFI → ../build/EphemerisFFI.xcframework
    Sources/EphemerisKit/
      Generated/ephemeris_ffi.swift    from uniffi-bindgen (gitignored)
      Engine.swift               EngineClient actor: lanes, superseded, JSON → Codable
      Schema.swift               the Codable report and options types
      Format.swift               money and date formatting (port of web/src/format.ts)
    Tests/EphemerisKitTests/     golden, drift, format and lane tests
  App/                           the SwiftUI app target's sources
  build/                         xcframework, DerivedData (gitignored)
```

Putting the engine wrapper and types in a package keeps them testable
without the app, on macOS with `swift test` and in the simulator. The app
target holds only views and its model.

## 3. The app (SwiftUI, Swift Charts, iOS 17+)

iOS 17 is the floor. It is where `@Observable`, `chartXSelection` and a
mature Swift Charts arrive.

The app follows the web app's behaviour and words, laid out for a phone:

```
┌───────────────────────────────┐
│ EPHEMERIS   demo · seed 7  ⋯  │  ⋯ = Demo mode/seed, Open feed…
├───────────────────────────────┤
│ 47%  chance the balance dips  │  headline risk: below_by(0), last day
│ below $0 through Jan 19, 2027 │
├───────────────────────────────┤
│ [ Daily | Pay-cycle lows ]    │  segmented Picker
│ [ 30 · 90 · 180 days ]        │
│   ░░▒▒▓▓───  cone chart       │  Swift Charts, drag to inspect a day
│ 5–95%  25–75%  median         │
├───────────────────────────────┤
│ CAN I AFFORD IT?              │
│ $[2000]  on [Nov 20]  [Check] │
│ ON NOV 20: 35%                │
│ AT ANY POINT THROUGH NOV 20:  │
│ 43%                           │
│ why they differ: …            │
├───────────────────────────────┤
│ STATE · balance, spend rate,  │  same rows as the web state panel
│ paycheck; pay schedule;       │
│ Recurring In/Out; regime      │
│ changes                       │
└───────────────────────────────┘
```

### The parts

- **Model.** `AppModel` is `@Observable` and on the main actor. It holds:
  - the source, feed text, report and status;
  - any error;
  - the view, the horizon, and the demo mode and seed.

  Only the latest request may update the screen, as with the web app's
  ticket.
- **Cone, Daily view.**
  - Two `AreaMark` bands, `yStart: q05` to `yEnd: q95` and `q25` to `q75`.
  - A `LineMark` for the median.
  - A dashed `RuleMark` at $0 when the cone reaches below zero, and a
    today rule.
- **Cone, Pay-cycle lows view.** The same marks over `cycle_lows`, drawn
  with `.interpolationMethod(.stepEnd)`.
  - Swift Charts has no hatch patterns, so partial cycles are drawn at
    lower opacity with a "partial" annotation.
  - The legend explains it, as on the web.
- **Inspecting a day.** `chartXSelection` shows a rule and a card with the
  date and the five quantiles read from the report.
- **Headline risk.** `below_by` for target 0 on the last day, with the
  same tiers and wording. Every forecast requests `targets: [0]`, as on
  the web.
- **Afford query.**
  - It reruns the forecast with `targets: [X]` and the same seed and paths.
  - It shows two cards, "On <date>" and "At any point through <date>",
    with the web's why-line.
  - The date picker is limited to the forecast's dates.
- **State panel.** The same rows and rules as the web, including:
  - "range … floored at $0.00" for the spend rate;
  - Recurring In and Out lists;
  - regime changes.
- **Formatting.** `Format.swift` ports `format.ts` exactly:
  - cents in text;
  - whole dollars on axes;
  - signed amounts;
  - the same date styles.

  The web's format tests are ported as cases, so both formatters print
  the same strings.
- **Theme.** It follows the system light/dark setting, with the web's
  palette as asset-catalog colors (dark first) and monospaced digits for
  numbers.
- **iPad** runs the iPhone layout. A two-column iPad layout is later.

## 4. Feeds

- **Demo.** `simulate_json` with a mode (realistic or model-matched) and a
  seed. The app opens on the realistic demo with seed 7, like the web.
- **Files.** "Open feed…" uses `.fileImporter(allowedContentTypes:
  [.json])`.
  - The app reads the picked URL inside `startAccessingSecurityScopedResource()`.
  - It refuses files over 32 MiB, as the desktop app does.
  - It checks the text is JSON ("<name> is not JSON."), then forecasts
    it. Engine errors are shown verbatim.
  - Feeds can come from iCloud Drive, On My iPhone or any Files provider.
- **Not in 3a:**
  - a recent-files list, which would need security-scoped bookmarks;
  - "Open in EPHEMERIS" from other apps;
  - Plaid Link.
- **Screenshots** need the sandbox feed without tapping through the
  picker:
  - The screenshot script copies it into the simulator app's Documents
    folder with `simctl get_app_container`.
  - It launches with `-EPHEMERIS_FEED feed.json`.
  - The app honours that argument only in Debug builds.

## 5. Project generation (XcodeGen)

`ios/project.yml` is committed; `ios/*.xcodeproj` is gitignored. Running
`xcodegen generate --spec ios/project.yml` recreates the project.
XcodeGen 2.46 is already installed through Homebrew.

| Target | Contents |
|---|---|
| `Ephemeris` (iOS application) | Bundle id `dev.ephemeris.ios`, iOS 17.0, sources `App/`, the local package `EphemerisKit` as a dependency |
| `EphemerisTests` (unit tests) | `AppModel` with a fake engine: a burst of horizon changes runs one forecast; errors, the risk headline and the afford flow |

- **Signing.** Simulator only: no team, signed to run locally
  (`CODE_SIGN_IDENTITY = "-"`).
- **Scheme.** One shared scheme, `Ephemeris`, runs the app's tests and
  the package's tests.

## 6. Testing

1. **Rust** (`cargo test -p ephemeris-ffi`): the golden files through the
   exported functions, natively; error mapping; `schema_version`. Also the
   new all-fields fixture (§1) in the golden set.
2. **The fourth byte-identity path (Swift, through the FFI).**
   `GoldenTests` calls `simulateJson` with the golden inputs and asserts
   the `String` equals `feed.json`, then calls `forecastJson` and asserts
   it equals `report.json`. Swift strings compare by Unicode scalars and
   both files are ASCII, so this is a byte comparison.
   - The golden *inputs* move from `common/golden_inputs.rs` into
     `tests/golden/inputs.json`, which the Rust tests and Swift both read.
     This keeps one source for them.
   - It runs two ways:
     - `swift test` in `ios/EphemerisKit`: the macOS slice, seconds, no
       simulator. This is the everyday loop, and `scripts/test-full.sh`
       runs it when Xcode is present.
     - `xcodebuild test -scheme Ephemeris -destination 'platform=iOS
       Simulator,name=…'`: the iOS-simulator slice, the actual iOS path.
       It lives in `scripts/test-ios.sh` because it needs the simulator
       runtime (§8).
3. **Drift** (§1): the round-trip over `report.json` and
   `report-all-fields.json`.
4. **Formatting:** the ported web cases.
5. **The app model:** lanes and debounce (a burst runs one forecast), the
   error paths, the afford query, using a fake engine.
6. **Simulator screenshots** (`scripts/ios-screenshots.sh`). For one named
   iPhone:
   - `xcrun simctl boot` the device;
   - `simctl status_bar … override --time 9:41` for clean bars;
   - `simctl ui … appearance dark|light`;
   - `simctl install`, then `simctl launch` with the demo or sandbox
     argument and the view;
   - wait for a "ready" log line;
   - `simctl io … screenshot`.

   It writes the full matrix to a chosen folder, and to `docs/img/ios-*.png`
   only the few worth putting in the README, as with the web script.
   It fails if the app logs an error.

## 7. Out of scope for 3a

- On-device categorization (CoreML).
- Signing for a real device, TestFlight and the App Store.
- Android (3b).
- Sync (4b).
- iPad-specific layout.
- Recent files and "Open in".
- Widgets.
- Plaid Link.
- Localization.

The device slice (`aarch64-apple-ios`) is still built and linked, so a
signed device build later is a settings change.

## 8. Cost on this machine (M3, 8 GB, 32 GB free today)

| Item | Disk | Time | Basis |
|---|---|---|---|
| Xcode 27.1 with the iOS 27.1 SDK | already installed | — | checked |
| iOS Simulator runtime | about 8–10 GB, one-time | a 10–30 min download | estimate. Not installed yet: `xcrun simctl list runtimes` is empty. `xcodebuild -downloadPlatform iOS` fetches it. |
| Core, release, one iOS target | 124 MB | 9–14 s cold | measured today (`-j 6`) |
| `ephemeris-ffi` + UniFFI runtime, three slices | about 0.4–0.6 GB in `target/` | about 1–2 min cold; seconds after an edit | estimate from the core figure, plus UniFFI's runtime and proc-macro crates |
| `uniffi-bindgen` host binary (debug, once) | about 0.3 GB | about 1 min | estimate |
| XCFramework (three static libraries) | about 30–60 MB | seconds | estimate |
| Xcode DerivedData for the app and package | about 0.5–1 GB, kept in `ios/build/DerivedData` | 1–2 min cold, under 20 s incremental | estimate |
| A booted simulator device | about 1–2 GB | a 20–40 s first boot | estimate |

**In total:** about 11–14 GB the first time, of which 8–10 GB is the
simulator runtime. After that a few hundred MB per rebuild, all under
`target/` and `ios/build/`. The estimates are replaced by measurements in
an "As built" section after implementation, as in Phase 4a.

**Keeping it lean:**
- `--sim-only` for the everyday loop.
- `-j 6`, as for the desktop build.
- One simulator device.
- DerivedData inside `ios/build`, not `~/Library`.
- The bindings generator behind a feature.
- The crate is not a default member.

**Cleanup:**
- `rm -rf ios/build`;
- `xcrun simctl delete unavailable`;
- `xcrun simctl runtime delete <id>` to drop the runtime when done.

## 9. Order of work, after approval

Each step is a small commit:

1. The golden inputs move to `inputs.json`; the all-fields fixture is
   added. Rust only.
2. `ephemeris-ffi`: the exported functions, their Rust tests, and
   `test-full.sh`.
3. `scripts/build-ios.sh` and the XCFramework; `.gitignore` entries.
4. `EphemerisKit`: the `Codable` types, `EngineClient`, `Format`; the
   golden, drift, format and lane tests (`swift test`).
5. `project.yml` and the app: model, state panel, headline, cone (both
   views), afford, feeds.
6. Simulator tests (`scripts/test-ios.sh`), after the runtime download,
   which I will ask about first because of its size.
7. The screenshot script and the README shots; docs "As built"; README
   and CLAUDE.md.

## 10. As built

Built 2026-10-29 and 2026-10-30, as designed in §1–6 with the deviations
below.

### Measured (this machine, M3, 8 GB)

| Item | Disk | Time | Estimate (§8) |
|---|---|---|---|
| iOS 27.1 simulator runtime | 7.3 GB | downloaded separately | 8–10 GB |
| `ephemeris-ffi`, three slices, release | 192–203 MB per slice in `target/` | 42 s cold (`build-ios.sh`); under 1 s when unchanged | 0.4–0.6 GB, 1–2 min |
| XCFramework | 29 MB | seconds | 30–60 MB |
| `swift test` on the Mac (19 tests) | 128 MB in `ios/build/swiftpm` | 19 s cold | — |
| App build, simulator, Debug | 224 MB DerivedData; `Ephemeris.app` 2.8 MB | 147 s cold, 37 s incremental | 0.5–1 GB, 1–2 min |
| `scripts/test-ios.sh` (19 tests in the simulator, then the launch check) | — | 3 min 19 s on a quiet Mac | — |
| Simulator device "EPHEMERIS iPhone" | 2.9 GB | see below | 1–2 GB, a 20–40 s boot |
| `scripts/ios-screenshots.sh`, the README set of three | three PNGs, 0.3–0.4 MB each | 4–7 min | — |

**The simulator is the expensive part on 8 GB.** Even after `bootstatus`
reports the device booted, for minutes the
simulator's own services keep the machine saturated (load average 75–166
measured). During that time an app launch is slow:

- first launch after a boot, until the app's "EPHEMERIS ready" line:
  **214 s**;
- a second launch on the same booted device, load still above 100:
  **85 s**;
- on a quiet Mac with the device just booted by `xcodebuild test`: well
  within the scripts' wait.

So the scripts wait up to `LAUNCH_TIMEOUT` seconds (default 300) rather
than the original 60. One run failed at 60 s; with 300 s, `test-ios.sh`
passed (19/19, app ready). A run also failed once with "Mach error -308
(ipc/mig) server died" while the simulator runtime was being re-registered
at the end of its download; after that the device showed as unavailable
until CoreSimulator picked the runtime up again. Neither was an app
failure.

### Deviations from the design

- **The XCFramework lives in the package,**
  `ios/EphemerisKit/EphemerisFFI.xcframework`, not in `ios/build`.
  SwiftPM requires a `binaryTarget` path inside the package directory. It
  is gitignored there, as are `Sources/EphemerisKit/Generated` and the
  copied `Tests/EphemerisKitTests/Golden`.
- **`AppModel` is in `EphemerisKit`, not the app target,** and so are
  `Reading.swift` (band indices, the headline risk, the afford answer)
  and their tests. That way `swift test` covers the model on the Mac in
  seconds. As a result there is no `EphemerisTests` app test target
  (§5): the scheme's test action runs `EphemerisKitTests`, which holds
  the model tests the design gave that target.
- **`EngineClient` is a `@MainActor` class, not an actor.** It only
  hands out tickets and awaits the engine; `FFIEngine` runs each call
  with `Task.detached`, so the work is off the main thread either way.
- **Two golden input files, not one:** `simulate-options.json` and
  `forecast-options.json` in `crates/ephemeris-wasm/tests/golden/`.
  Each is passed to the engine as it is, without a wrapper to unpack.
- **Colors are in code** (`Theme.swift`, a light and a dark value per
  token from the web palette), not the asset catalog, which holds only the
  app icon.
- **The device type.** The only iPhone the iOS 27.1 runtime supports is
  "iPhone Duo", a foldable. `ios-device.sh` picks the first iPhone the
  newest runtime supports, and `DEVICE_TYPE` overrides that. On this
  device:
  - the app draws on the cover screen (1398×2034), while the inner panel
    (2007×2853) stays black;
  - a plain `simctl io screenshot` takes the inner panel. So
    `ios-screenshots.sh` captures every display with a framebuffer and
    keeps the largest PNG.
- **`x86_64` is excluded** from simulator builds
  (`EXCLUDED_ARCHS[sdk=iphonesimulator*]`): the XCFramework has only an
  arm64 simulator slice.
- **Launch checks rely on log lines.** In Debug builds the app writes
  "EPHEMERIS ready: <source>" or "EPHEMERIS error: …" to stderr, which
  `simctl launch --console` passes through. `print` was not used because
  stdout is buffered when piped. The app runs only as long as that console
  launcher, so the scripts keep it running until the screenshot is taken.
- **Screenshots wait for the screen to settle.** Under load the screen can
  trail the ready line by seconds, so a fixed sleep caught the loading
  spinner once. The script retakes the shot until two frames 2 s apart
  are identical. Repeated runs give byte-identical PNGs.

### Not verified here

- **A device build.** The device slice is built (without `--sim-only`)
  and linked into the XCFramework, but nothing has been signed or run on
  a phone.
- **Interaction by hand:** the Files picker, the chart's drag-to-inspect,
  the afford query, and changing the horizon. Their logic is covered by
  the model tests with a fake engine (afford, debounce, errors, file size
  and JSON checks). The screens were checked only as the screenshots show
  them: the headline, the cone in both views, both appearances.
