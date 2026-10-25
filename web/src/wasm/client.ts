// The app's engine: async calls to the worker, plus a same-thread version
// with the same interface for tests and environments without workers.

import * as direct from "./engine";
import type { Request, Response } from "./protocol";
import type {
  ForecastOptions,
  ForecastReport,
  SimulateOptions,
} from "../schema.gen";

export interface Engine {
  /** A synthetic feed, as JSON text. */
  simulateJson(options: Partial<SimulateOptions>): Promise<string>;
  /** A report for a feed's JSON text (passed through untouched). */
  forecast(feedJson: string, options: Partial<ForecastOptions>): Promise<ForecastReport>;
}

type RequestBody =
  | { op: "simulate"; options: Partial<SimulateOptions> }
  | { op: "forecast"; feedJson: string; options: Partial<ForecastOptions> };

/** The engine in a Web Worker. */
export function workerEngine(): Engine {
  const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  const pending = new Map<number, (r: Response) => void>();
  let next = 1;
  worker.onmessage = (e: MessageEvent<Response>) => {
    pending.get(e.data.id)?.(e.data);
    pending.delete(e.data.id);
  };
  const call = (body: RequestBody) =>
    new Promise<string>((resolve, reject) => {
      const id = next++;
      pending.set(id, (r) => (r.ok ? resolve(r.json) : reject(new Error(r.error))));
      worker.postMessage({ id, ...body } as Request);
    });
  return {
    simulateJson: (options) => call({ op: "simulate", options }),
    forecast: async (feedJson, options) =>
      JSON.parse(await call({ op: "forecast", feedJson, options })) as ForecastReport,
  };
}

/** The engine on the calling thread; `loadEngine` must have resolved. */
export const directEngine: Engine = {
  simulateJson: async (options) => direct.simulateJson(options),
  forecast: async (feedJson, options) => direct.forecast(feedJson, options),
};
