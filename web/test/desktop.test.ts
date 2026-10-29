// @vitest-environment node
// The desktop shell's port and file host, against a stand-in for Tauri's
// `invoke` and `listen` that answers with the real engine.

import { describe, expect, it, vi } from "vitest";

import { tauriFiles, tauriPort } from "../src/desktop/shell";
import type { Invoke, Listen } from "../src/desktop/shell";
import { SupersededError, portEngine } from "../src/wasm/client";
import { forecastJson, simulateJson } from "../src/wasm/engine";
import { FEED_JSON } from "./fixtures";

/** The Rust commands, by the arguments the webview sends them. */
async function native(cmd: string, args: Record<string, unknown> = {}): Promise<unknown> {
  const opts = (k: string) => JSON.parse(args[k] as string) as object;
  try {
    if (cmd === "simulate") return simulateJson(opts("optionsJson"));
    if (cmd === "forecast") return forecastJson(args.feedJson as string, opts("optionsJson"));
  } catch (e) {
    // Rust errors reach JavaScript as plain strings.
    throw e instanceof Error ? e.message : e;
  }
  throw `unknown command ${cmd}`;
}

describe("tauriPort", () => {
  it("runs simulate and forecast as commands and returns the engine's JSON", async () => {
    const call = vi.fn(native) as unknown as Invoke;
    const engine = portEngine(tauriPort(call));
    const feed = await engine.simulateJson({ days: 90, seed: 2 });
    expect(feed).toBe(simulateJson({ days: 90, seed: 2 }));
    const opts = { horizon_days: 20, paths: 100 };
    const report = await engine.forecast(feed, opts);
    expect(report).toEqual(JSON.parse(forecastJson(feed, opts)));
    expect(call).toHaveBeenCalledWith("forecast", {
      feedJson: feed,
      optionsJson: JSON.stringify(opts),
    });
  });

  it("surfaces the engine's error message", async () => {
    const engine = portEngine(tauriPort(native as Invoke));
    await expect(engine.forecast("{", {})).rejects.toThrow(/^feed:/);
  });

  it("keeps lanes: a burst runs the in-flight and the newest only", async () => {
    let calls = 0;
    const call = ((cmd: string, args?: Record<string, unknown>) => {
      calls++;
      return native(cmd, args);
    }) as Invoke;
    const engine = portEngine(tauriPort(call));
    const horizons = [30, 60, 90, 120, 150, 180, 45];
    const settled = await Promise.all(
      horizons.map((h) =>
        engine.forecast(FEED_JSON, { horizon_days: h, paths: 100 }, "cone").then(
          (r) => r.forecast.horizon_days,
          (e: unknown) => e,
        ),
      ),
    );
    expect(calls).toBe(2);
    expect(settled[0]).toBe(30);
    expect(settled.at(-1)).toBe(45);
    for (const s of settled.slice(1, -1)) expect(s).toBeInstanceOf(SupersededError);
  });
});

describe("tauriFiles", () => {
  const opened = { name: "f.json", path: "/data/f.json", json: "{}" };

  it("maps the file host to the Rust commands", async () => {
    const call = vi.fn(async (cmd: string) =>
      cmd === "recent_feeds" ? [{ name: "f.json", path: "/data/f.json" }] : opened,
    ) as unknown as Invoke;
    const files = tauriFiles(call, vi.fn() as unknown as Listen);
    await files.pick();
    expect(call).toHaveBeenLastCalledWith("pick_feed");
    expect(await files.recent()).toEqual([{ name: "f.json", path: "/data/f.json" }]);
    expect(await files.openRecent("/data/f.json")).toEqual(opened);
    expect(call).toHaveBeenLastCalledWith("open_recent", { path: "/data/f.json" });
  });

  it("delivers drop events and unsubscribes from all of them", async () => {
    const handlers = new Map<string, (e: { payload: unknown }) => void>();
    const unlisten = vi.fn();
    const on = (async (event: string, h: (e: { payload: unknown }) => void) => {
      handlers.set(event, h);
      return unlisten;
    }) as Listen;
    const got = { opened: vi.fn(), error: vi.fn(), hover: vi.fn() };
    const stop = tauriFiles(native as Invoke, on).onDrop(got);

    handlers.get("feed-hover")?.({ payload: true });
    handlers.get("feed-opened")?.({ payload: opened });
    handlers.get("feed-error")?.({ payload: "big.json is 40 MiB" });
    expect(got.hover).toHaveBeenCalledWith(true);
    expect(got.opened).toHaveBeenCalledWith(opened);
    expect(got.error).toHaveBeenCalledWith("big.json is 40 MiB");

    stop();
    await vi.waitFor(() => expect(unlisten).toHaveBeenCalledTimes(3));
  });
});
