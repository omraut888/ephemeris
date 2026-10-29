// App state: a feed (demo or file) → a forecast report → the panels.
// All computation goes through `engine` (a Web Worker in the browser); this
// file only sequences calls.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DragEvent } from "react";

import { AffordQuery } from "./components/AffordQuery";
import { ConeChart } from "./components/ConeChart";
import type { ConeView } from "./components/ConeChart";
import { Header } from "./components/Header";
import { OVERDRAWN_TARGET, Risk } from "./components/Risk";
import type { Source, Status } from "./components/Header";
import { StatePanel } from "./components/StatePanel";
import { workerEngine } from "./wasm/client";
import type { Engine } from "./wasm/client";
import type { ForecastOptions, ForecastReport, SimMode } from "./schema.gen";

/** The cone view the app opens on: Daily, decided by the product owner
 * (docs/phase2-design.md §4). It matches the CLI. */
export const DEFAULT_VIEW: ConeView = "raw";

const HORIZONS = [30, 90, 180] as const;
const PATHS = 2000;
/** Forecast seed: fixed, so the same feed always draws the same cone. */
const FORECAST_SEED = 0;

interface Loaded {
  source: Source;
  feedJson: string;
  report: ForecastReport;
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function App({ engine: given }: { engine?: Engine }) {
  const engine = useMemo(() => given ?? workerEngine(), [given]);
  const [status, setStatus] = useState<Status>("loading");
  const [error, setError] = useState<string | null>(null);
  const [data, setData] = useState<Loaded | null>(null);
  const [view, setView] = useState<ConeView>(DEFAULT_VIEW);
  const [horizon, setHorizon] = useState<number>(90);
  const [mode, setMode] = useState<SimMode>("realistic");
  const [seed, setSeed] = useState(7);
  const [dragging, setDragging] = useState(false);

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

  // Only the latest request may update the screen.
  const latest = useRef(0);

  const run = useCallback(
    async (source: Source, feedJson: string | Promise<string>, h: number) => {
      const ticket = ++latest.current;
      setStatus("loading");
      try {
        const json = await feedJson;
        const report = await engine.forecast(json, options(h));
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
    [engine, options],
  );

  const runDemo = useCallback(
    (m: SimMode, s: number, h: number) =>
      run({ kind: "demo", mode: m, seed: s }, engine.simulateJson({ mode: m, seed: s }), h),
    [engine, run],
  );

  const loadFile = useCallback(
    async (file: File) => {
      const text = await file.text();
      try {
        JSON.parse(text);
      } catch {
        setError(`${file.name} is not JSON.`);
        setStatus("error");
        return;
      }
      await run({ kind: "file", name: file.name }, text, horizon);
    },
    [run, horizon],
  );

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
                  void run(data.source, data.feedJson, h);
                }}
              >
                {HORIZONS.map((h) => (
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
              options={options(horizon)}
              report={data.report}
              run={engine.forecast}
            />
          </div>
        </main>
      ) : (
        <div className="empty">{status === "loading" ? "Loading the engine…" : "No data yet."}</div>
      )}
      {dragging && <div className="drop-overlay">Drop a feed JSON to forecast it</div>}
    </div>
  );
}
