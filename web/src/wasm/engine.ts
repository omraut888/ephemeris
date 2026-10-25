// The only module that touches the WASM package (docs/phase2-design.md §5).
// Every number the app shows comes from these calls; TypeScript only
// serializes options, parses the JSON the engine returns, and draws.

import init, {
  forecast as wasmForecast,
  initSync,
  schema_version as wasmSchemaVersion,
  simulate as wasmSimulate,
} from "./pkg/ephemeris_wasm.js";
import type {
  Feed,
  ForecastOptions,
  ForecastReport,
  SimulateOptions,
} from "../schema.gen";

let ready: Promise<void> | null = null;
let loaded = false;

/** Fetch and instantiate the WASM module once; later calls reuse it. */
export function loadEngine(): Promise<void> {
  if (loaded) return Promise.resolve();
  ready ??= init().then(() => {
    loaded = true;
  });
  return ready;
}

/** Instantiate from bytes already in hand (tests, or a preloaded module). */
export function loadEngineSync(module: BufferSource): void {
  if (loaded) return;
  initSync({ module });
  loaded = true;
}

function requireLoaded(): void {
  if (!loaded) {
    throw new Error("EPHEMERIS engine not loaded: await loadEngine() first");
  }
}

/** The feed and report `schema_version` this engine reads and writes. */
export function schemaVersion(): number {
  requireLoaded();
  return wasmSchemaVersion();
}

/** A synthetic feed, as JSON text. Every option has a default. */
export function simulateJson(options: Partial<SimulateOptions> = {}): string {
  requireLoaded();
  return wasmSimulate(JSON.stringify(options));
}

export function simulate(options: Partial<SimulateOptions> = {}): Feed {
  return JSON.parse(simulateJson(options)) as Feed;
}

/**
 * A forecast report, as JSON text. `feedJson` is passed through untouched,
 * so a loaded file reaches the engine byte for byte.
 */
export function forecastJson(
  feedJson: string,
  options: Partial<ForecastOptions> = {},
): string {
  requireLoaded();
  return wasmForecast(feedJson, JSON.stringify(options));
}

export function forecast(
  feedJson: string,
  options: Partial<ForecastOptions> = {},
): ForecastReport {
  return JSON.parse(forecastJson(feedJson, options)) as ForecastReport;
}
