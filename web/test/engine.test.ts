// @vitest-environment node
// The WASM bridge, end to end, in Node.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import {
  forecast,
  forecastJson,
  schemaVersion,
  simulate,
  simulateJson,
} from "../src/wasm/engine";

const golden = (name: string) =>
  readFileSync(resolve("../crates/ephemeris-wasm/tests/golden", name), "utf8");

describe("engine bridge", () => {
  it("reports the schema version", () => {
    expect(schemaVersion()).toBe(1);
  });

  it("matches the native golden files byte for byte", () => {
    // The same options as crates/ephemeris-wasm/tests/common/golden_inputs.rs,
    // through the browser build the app ships.
    const feed = simulateJson({ seed: 7, days: 240, mode: "realistic" });
    expect(feed === golden("feed.json")).toBe(true);
    const report = forecastJson(feed, {
      horizon_days: 90,
      paths: 2000,
      seed: 11,
      targets: [2000, 5000],
    });
    expect(report === golden("report.json")).toBe(true);
  });

  it("returns typed objects with the report's shape", () => {
    const feed = simulate({ days: 120, seed: 3 });
    expect(feed.schema_version).toBe(1);
    expect(feed.transactions.length).toBeGreaterThan(100);
    const r = forecast(JSON.stringify(feed), { horizon_days: 30, paths: 200, targets: [1000] });
    expect(r.as_of).toBe(feed.today);
    expect(r.forecast.days).toHaveLength(30);
    expect(r.forecast.levels).toEqual([0.05, 0.25, 0.5, 0.75, 0.95]);
    expect(r.forecast.targets[0]?.at_least_on_day).toHaveLength(30);
    const lows = r.forecast.cycle_lows;
    expect(lows?.cycles[0]?.start).toBe(r.forecast.days[0]?.date);
    expect(lows?.cycles.at(-1)?.end).toBe(r.forecast.days.at(-1)?.date);
  });

  it("is deterministic for a seed and different across seeds", () => {
    const feed = simulateJson({ days: 90 });
    const a = forecastJson(feed, { paths: 300, seed: 5 });
    expect(forecastJson(feed, { paths: 300, seed: 5 })).toBe(a);
    expect(forecastJson(feed, { paths: 300, seed: 6 })).not.toBe(a);
  });

  it("surfaces the engine's errors as exceptions", () => {
    expect(() => simulateJson({ days: 0 })).toThrow(/days must be/);
    expect(() => forecastJson("{", {})).toThrow(/^feed:/);
    const feed = simulateJson({ days: 60 });
    expect(() => forecastJson(feed, { paths: 0 })).toThrow(/n_paths/);
    // Unknown options are rejected, not silently ignored.
    expect(() =>
      forecastJson(feed, { horizon: 30 } as unknown as { horizon_days: number }),
    ).toThrow(/unknown field/);
  });
});
