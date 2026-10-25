// @vitest-environment node
// The worker's message handling, without a worker.

import { describe, expect, it } from "vitest";

import { forecastJson, simulateJson } from "../src/wasm/engine";
import { handle } from "../src/wasm/protocol";

describe("worker protocol", () => {
  it("answers simulate and forecast with the engine's exact JSON", () => {
    const sim = handle({ id: 1, op: "simulate", options: { days: 90, seed: 2 } });
    expect(sim).toEqual({ id: 1, ok: true, json: simulateJson({ days: 90, seed: 2 }) });
    if (!sim.ok) throw new Error(sim.error);
    const opts = { horizon_days: 20, paths: 100 };
    const fc = handle({ id: 2, op: "forecast", feedJson: sim.json, options: opts });
    expect(fc).toEqual({ id: 2, ok: true, json: forecastJson(sim.json, opts) });
  });

  it("turns engine errors into error responses instead of throwing", () => {
    const r = handle({ id: 3, op: "forecast", feedJson: "{", options: {} });
    expect(r.ok).toBe(false);
    expect(r.id).toBe(3);
    if (!r.ok) expect(r.error).toMatch(/^feed:/);
  });
});
