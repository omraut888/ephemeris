// @vitest-environment node
// The worker client's lanes, against a stand-in port that counts requests.

import { describe, expect, it } from "vitest";

import { SupersededError, portEngine } from "../src/wasm/client";
import type { Port } from "../src/wasm/client";
import { handle } from "../src/wasm/protocol";
import type { Request } from "../src/wasm/protocol";
import { FEED_JSON } from "./fixtures";

/** A port that answers only when told to, like a busy worker. */
function fakeWorker() {
  const inbox: Request[] = [];
  const answered = new Set<number>();
  const port: Port = {
    onmessage: null,
    postMessage: (req) => {
      inbox.push(req);
    },
  };
  return {
    port,
    received: () => [...inbox],
    count: () => inbox.length,
    /** Answer the oldest unanswered request with the real engine. */
    answerNext() {
      const req = inbox.find((r) => !answered.has(r.id));
      if (!req) throw new Error("nothing to answer");
      answered.add(req.id);
      port.onmessage?.({ data: handle(req) } as MessageEvent);
    },
  };
}
const flush = () => new Promise((r) => setTimeout(r, 0));

describe("portEngine", () => {
  it("computes only the in-flight and the newest of a burst in one lane", async () => {
    const w = fakeWorker();
    const engine = portEngine(w.port);
    const horizons = [30, 90, 180, 30, 90, 180, 30, 90, 180, 60];
    const results = horizons.map((h) =>
      engine.forecast(FEED_JSON, { horizon_days: h, paths: 100 }, "cone").then(
        (r) => r.forecast.horizon_days,
        (e: unknown) => e,
      ),
    );
    // The first went to the worker; the rest wait, each replacing the last.
    expect(w.count()).toBe(1);
    w.answerNext();
    await flush();
    expect(w.count()).toBe(2);
    w.answerNext();
    const settled = await Promise.all(results);

    expect(w.count()).toBe(2);
    expect(w.received().map((r) => r.op === "forecast" && r.options.horizon_days)).toEqual([30, 60]);
    expect(settled[0]).toBe(30);
    expect(settled.at(-1)).toBe(60);
    for (const s of settled.slice(1, -1)) expect(s).toBeInstanceOf(SupersededError);
  });

  it("leaves requests without a lane, and other lanes, alone", async () => {
    const w = fakeWorker();
    const engine = portEngine(w.port);
    const opts = { horizon_days: 20, paths: 100 };
    const all = Promise.all([
      engine.forecast(FEED_JSON, opts, "cone"),
      engine.forecast(FEED_JSON, opts, "afford"),
      engine.forecast(FEED_JSON, opts),
      engine.forecast(FEED_JSON, opts),
    ]);
    expect(w.count()).toBe(4);
    for (let i = 0; i < 4; i++) w.answerNext();
    expect((await all).map((r) => r.forecast.horizon_days)).toEqual([20, 20, 20, 20]);
  });

  it("starts the waiting request after a failed one", async () => {
    const w = fakeWorker();
    const engine = portEngine(w.port);
    const bad = engine.forecast("{", {}, "cone");
    const good = engine.forecast(FEED_JSON, { horizon_days: 20, paths: 100 }, "cone");
    w.answerNext();
    await expect(bad).rejects.toThrow(/^feed:/);
    await flush();
    w.answerNext();
    expect((await good).forecast.horizon_days).toBe(20);
  });
});
