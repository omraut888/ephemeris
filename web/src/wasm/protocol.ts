// Messages between the app and the engine worker. `handle` is the worker's
// whole job, kept free of worker globals so it can be tested directly.

import { forecastJson, simulateJson } from "./engine";
import type { ForecastOptions, SimulateOptions } from "../schema.gen";

export type Request =
  | { id: number; op: "simulate"; options: Partial<SimulateOptions> }
  | { id: number; op: "forecast"; feedJson: string; options: Partial<ForecastOptions> };

export type Response =
  | { id: number; ok: true; json: string }
  | { id: number; ok: false; error: string };

/** Run one request on the (already loaded) engine. Never throws. */
export function handle(req: Request): Response {
  try {
    const json =
      req.op === "simulate"
        ? simulateJson(req.options)
        : forecastJson(req.feedJson, req.options);
    return { id: req.id, ok: true, json };
  } catch (e) {
    return { id: req.id, ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}
