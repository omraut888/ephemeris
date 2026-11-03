// App state: a feed (demo or file) → a forecast report → the panels.
// All computation goes through `engine` (a Web Worker in the browser, Tauri
// commands on the desktop); this file only sequences calls. `files` is the
// desktop app's native file handling; without it the browser's is used.

import { useCallback, useEffect, useRef, useState } from "react";
import type { DragEvent } from "react";

import { AffordQuery } from "./components/AffordQuery";
import { ConeChart } from "./components/ConeChart";
import type { ConeView } from "./components/ConeChart";
import { Header } from "./components/Header";
import { OVERDRAWN_TARGET, Risk } from "./components/Risk";
import type { Source, Status } from "./components/Header";
import { StatePanel } from "./components/StatePanel";
import { SyncSheet } from "./components/SyncSheet";
import type { FileHost, OpenedFeed, RecentFeed } from "./files";
import { SYNC_POLL_MS } from "./sync/host";
import type { SavedQuery, SyncHost, SyncStatus } from "./sync/host";
import type { Engine } from "./wasm/client";
import type { ForecastOptions, ForecastReport, SimMode } from "./schema.gen";

/** The cone view the app opens on: Daily, decided by the product owner
 * (docs/phase2-design.md §4). It matches the CLI. */
export const DEFAULT_VIEW: ConeView = "raw";

const HORIZONS = [30, 90, 180] as const;
const PATHS = 2000;
/** Forecast seed: fixed, so the same feed always draws the same cone. */
const FORECAST_SEED = 0;
/** A horizon change waits this long for the next before it forecasts, so
 * arrowing through the select runs one forecast, not one per step. */
export const INPUT_DEBOUNCE_MS = 150;

interface Loaded {
  source: Source;
  feedJson: string;
  report: ForecastReport;
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function App({
  engine,
  files,
  sync,
}: {
  engine: Engine;
  files?: FileHost;
  /** The shell's sync, when it has one (docs/phase4b-design.md §4). */
  sync?: SyncHost;
}) {
  const [status, setStatus] = useState<Status>("loading");
  const [error, setError] = useState<string | null>(null);
  const [data, setData] = useState<Loaded | null>(null);
  const [view, setView] = useState<ConeView>(DEFAULT_VIEW);
  const [horizon, setHorizon] = useState<number>(90);
  const [mode, setMode] = useState<SimMode>("realistic");
  const [seed, setSeed] = useState(7);
  const [dragging, setDragging] = useState(false);
  const [recent, setRecent] = useState<RecentFeed[]>([]);
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [saved, setSaved] = useState<SavedQuery[]>([]);
  // The synced settings' options, used while the synced data is on screen.
  const syncedOptions = useRef<ForecastOptions | null>(null);

  const options = useCallback(
    (h: number): Partial<ForecastOptions> => ({
      horizon_days: h,
      paths: PATHS,
      seed: FORECAST_SEED,
      // Gives the report its P(below $0 on some day) curve for the headline.
      targets: [OVERDRAWN_TARGET],
    }),
    [],
  );

  /** The options for a source: the synced settings' paths and seed for
   * synced data (so every device draws the same cone), the app's otherwise. */
  const optionsFor = useCallback(
    (source: Source, h: number): Partial<ForecastOptions> =>
      source.kind === "synced" && syncedOptions.current
        ? { ...syncedOptions.current, horizon_days: h, targets: [OVERDRAWN_TARGET] }
        : options(h),
    [options],
  );

  // Only the latest request may update the screen.
  const latest = useRef(0);
  // A debounced run not yet started; any run that starts replaces it.
  const debounced = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(debounced.current), []);

  const run = useCallback(
    async (source: Source, feedJson: string | Promise<string>, h: number) => {
      clearTimeout(debounced.current);
      const ticket = ++latest.current;
      setStatus("loading");
      try {
        const json = await feedJson;
        // One lane: a burst of changes computes only the newest.
        const report = await engine.forecast(json, optionsFor(source, h), "cone");
        if (ticket !== latest.current) return;
        setData({ source, feedJson: json, report });
        setError(null);
        setStatus("ready");
      } catch (e) {
        if (ticket !== latest.current) return;
        setError(message(e));
        setStatus("error");
      }
    },
    [engine, optionsFor],
  );

  const runDemo = useCallback(
    (m: SimMode, s: number, h: number) =>
      run({ kind: "demo", mode: m, seed: s }, engine.simulateJson({ mode: m, seed: s }), h),
    [engine, run],
  );

  /** A feed's text, from a picked, dropped or recent file. */
  const loadText = useCallback(
    async (name: string, text: string) => {
      try {
        JSON.parse(text);
      } catch {
        setError(`${name} is not JSON.`);
        setStatus("error");
        return;
      }
      await run({ kind: "file", name }, text, horizon);
    },
    [run, horizon],
  );

  const loadFile = useCallback(
    async (file: File) => loadText(file.name, await file.text()),
    [loadText],
  );

  const fail = useCallback((e: unknown) => {
    setError(message(e));
    setStatus("error");
  }, []);

  const refreshRecent = useCallback(() => {
    files?.recent().then(setRecent, () => setRecent([]));
  }, [files]);
  useEffect(refreshRecent, [refreshRecent]);

  const openFeed = useCallback(
    async (feed: OpenedFeed) => {
      refreshRecent();
      await loadText(feed.name, feed.json);
    },
    [refreshRecent, loadText],
  );

  // Dropped files arrive as events from the shell. Subscribe once; the ref
  // always holds the current `openFeed`.
  const openRef = useRef(openFeed);
  useEffect(() => {
    openRef.current = openFeed;
  }, [openFeed]);
  useEffect(
    () =>
      files?.onDrop({
        opened: (feed) => void openRef.current(feed),
        error: fail,
        hover: setDragging,
      }),
    [files, fail],
  );

  // Sync: the status at start, saved queries once paired, and a sync on
  // focus and every SYNC_POLL_MS while the window has focus.
  const refreshQueries = useCallback(() => {
    sync?.queries().then(setSaved, () => setSaved([]));
  }, [sync]);
  const onSyncStatus = useCallback(
    (s: SyncStatus) => {
      setSyncStatus(s);
      if (s.paired) refreshQueries();
    },
    [refreshQueries],
  );
  useEffect(() => {
    sync?.status().then(onSyncStatus, fail);
  }, [sync, onSyncStatus, fail]);

  const dataRef = useRef(data);
  useEffect(() => {
    dataRef.current = data;
  }, [data]);
  const horizonRef = useRef(horizon);
  useEffect(() => {
    horizonRef.current = horizon;
  }, [horizon]);

  const showSynced = useCallback(async () => {
    if (!sync) return;
    try {
      const synced = await sync.feed();
      if (!synced) {
        fail("No synced bank data yet.");
        return;
      }
      const opts = JSON.parse(synced.optionsJson) as ForecastOptions;
      syncedOptions.current = opts;
      const through = (await sync.status()).fetchedThrough;
      setSheetOpen(false);
      setHorizon(opts.horizon_days);
      await run({ kind: "synced", through }, synced.feedJson, opts.horizon_days);
    } catch (e) {
      fail(e);
    }
  }, [sync, run, fail]);

  const syncing = !!(sync && syncStatus?.paired && syncStatus.folder);
  useEffect(() => {
    if (!sync || !syncing) return;
    const tick = async () => {
      if (typeof document !== "undefined" && !document.hasFocus()) return;
      try {
        onSyncStatus(await sync.syncNow());
        const shown = dataRef.current;
        if (shown?.source.kind !== "synced") return;
        const synced = await sync.feed();
        if (!synced) return;
        const opts = JSON.parse(synced.optionsJson) as ForecastOptions;
        const changed =
          synced.feedJson !== shown.feedJson ||
          JSON.stringify(opts) !== JSON.stringify(syncedOptions.current);
        if (changed) {
          syncedOptions.current = opts;
          const through = (await sync.status()).fetchedThrough;
          await run({ kind: "synced", through }, synced.feedJson, horizonRef.current);
        }
      } catch {
        // A failed background sync shows in the sheet's status next time.
      }
    };
    const id = setInterval(() => void tick(), SYNC_POLL_MS);
    const onFocus = () => void tick();
    window.addEventListener("focus", onFocus);
    return () => {
      clearInterval(id);
      window.removeEventListener("focus", onFocus);
    };
  }, [sync, syncing, onSyncStatus, run]);

  // Open on a demo so there is something to look at.
  useEffect(() => {
    void runDemo("realistic", 7, 90);
  }, [runDemo]);

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setDragging(false);
    const file = e.dataTransfer.files[0];
    if (file) void loadFile(file);
  };

  return (
    <div
      className="app"
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={(e) => {
        if (e.currentTarget === e.target) setDragging(false);
      }}
      onDrop={onDrop}
    >
      <Header
        source={data?.source ?? null}
        status={status}
        mode={mode}
        seed={seed}
        onMode={setMode}
        onSeed={setSeed}
        onDemo={() => void runDemo(mode, seed, horizon)}
        onFile={(f) => void loadFile(f)}
        onPick={
          files &&
          (() => void files.pick().then((feed) => feed && openFeed(feed), fail))
        }
        recent={recent}
        onRecent={
          files && ((path) => void files.openRecent(path).then(openFeed, fail))
        }
        onSync={
          sync &&
          (() => {
            setSheetOpen(true);
            sync.status().then(onSyncStatus, fail);
          })
        }
      />
      {error && (
        <div className="banner" role="alert">
          {error}
        </div>
      )}
      {data ? (
        <main className="main">
          <StatePanel report={data.report} />
          <div className="right">
            <div className="chart-head">
              <span className="label">
                Forecast · {data.report.forecast.horizon_days} days ·{" "}
                {data.report.forecast.paths.toLocaleString("en-US")} paths
              </span>
              <select
                aria-label="Horizon"
                value={horizon}
                onChange={(e) => {
                  const h = Number(e.target.value);
                  setHorizon(h);
                  setStatus("loading");
                  clearTimeout(debounced.current);
                  debounced.current = setTimeout(
                    () => void run(data.source, data.feedJson, h),
                    INPUT_DEBOUNCE_MS,
                  );
                }}
              >
                {(HORIZONS.includes(horizon as (typeof HORIZONS)[number])
                  ? [...HORIZONS]
                  : [...HORIZONS, horizon].sort((a, b) => a - b)
                ).map((h) => (
                  <option key={h} value={h}>
                    {h} days
                  </option>
                ))}
              </select>
              <div className="segmented" role="group" aria-label="Cone view">
                <button
                  type="button"
                  aria-pressed={view === "raw"}
                  onClick={() => setView("raw")}
                >
                  Daily
                </button>
                <button
                  type="button"
                  aria-pressed={view === "lows"}
                  disabled={!data.report.forecast.cycle_lows}
                  onClick={() => setView("lows")}
                >
                  Pay-cycle lows
                </button>
              </div>
            </div>
            <Risk report={data.report} />
            <ConeChart report={data.report} view={view} />
            <div className="legend">
              <span>
                <i style={{ background: "var(--band-outer)" }} />
                5–95%
              </span>
              <span>
                <i style={{ background: "var(--band-inner)" }} />
                25–75%
              </span>
              <span>
                <i style={{ background: "var(--median)", height: 2 }} />
                median
              </span>
              {view === "lows" && data.report.forecast.cycle_lows?.cycles.some((c) => c.partial) && (
                <span>
                  <i className="hatched" />
                  partial cycle: cut by today or the horizon end
                </span>
              )}
              <span>
                {view === "raw"
                  ? "End-of-day balance, each day."
                  : `Each path's lowest balance in each ${
                      data.report.forecast.cycle_lows?.basis === "week" ? "week" : "pay cycle"
                    }.`}
              </span>
            </div>
            <AffordQuery
              key={`${data.feedJson.length}-${horizon}-${data.report.as_of}`}
              feedJson={data.feedJson}
              options={optionsFor(data.source, horizon)}
              report={data.report}
              run={engine.forecast}
              saved={sync && syncStatus?.paired ? saved : undefined}
              onSave={
                sync && syncStatus?.paired
                  ? async (cents, by, label) => setSaved(await sync.saveQuery(cents, by, label))
                  : undefined
              }
              onDelete={
                sync && syncStatus?.paired
                  ? async (id) => setSaved(await sync.deleteQuery(id))
                  : undefined
              }
            />
          </div>
        </main>
      ) : (
        <div className="empty">{status === "loading" ? "Loading the engine…" : "No data yet."}</div>
      )}
      {dragging && <div className="drop-overlay">Drop a feed JSON to forecast it</div>}
      {sync && sheetOpen && (
        <SyncSheet
          host={sync}
          status={syncStatus}
          onStatus={onSyncStatus}
          onClose={() => setSheetOpen(false)}
          onShowSynced={() => void showSynced()}
        />
      )}
    </div>
  );
}
