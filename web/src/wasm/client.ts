// The app's engine interface, and its implementation over a message port:
// the Web Worker in the browser, Tauri's IPC in the desktop app
// (docs/phase4-design.md §2). Neither the WASM package nor the worker is
// referenced here, so the desktop build bundles neither: the worker engine
// is in ./browser, and the same-thread engine for tests is `directEngine`
// in ./engine.

import type { Request, Response } from "./protocol";
import type {
  ForecastOptions,
  ForecastReport,
  SimulateOptions,
} from "../schema.gen";

export interface Engine {
  /** A synthetic feed, as JSON text. */
  simulateJson(options: Partial<SimulateOptions>): Promise<string>;
  /**
   * A report for a feed's JSON text (passed through untouched). Calls that
   * name the same `lane` keep only the newest: see `portEngine`.
   */
  forecast(
    feedJson: string,
    options: Partial<ForecastOptions>,
    lane?: string,
  ): Promise<ForecastReport>;
}

/** The rejection of a lane request that a newer one replaced before it ran. */
export class SupersededError extends Error {
  constructor() {
    super("superseded by a newer request");
    this.name = "SupersededError";
  }
}

/** The part of a Worker the client uses, so tests can stand in for it. */
export interface Port {
  postMessage(req: Request): void;
  onmessage: ((e: MessageEvent<Response>) => void) | null;
}

type RequestBody =
  | { op: "simulate"; options: Partial<SimulateOptions> }
  | { op: "forecast"; feedJson: string; options: Partial<ForecastOptions> };

interface Waiting {
  body: RequestBody;
  resolve: (json: string) => void;
  reject: (e: Error) => void;
}

/**
 * The engine behind a message port. The worker answers requests one at a
 * time, each a few hundred milliseconds, so a burst of input would queue a
 * forecast per change. A lane therefore holds at most one request in the
 * worker and one waiting here: a newer request replaces the waiting one,
 * which rejects with `SupersededError` without ever being computed.
 */
export function portEngine(port: Port): Engine {
  const pending = new Map<number, (r: Response) => void>();
  let next = 1;
  port.onmessage = (e: MessageEvent<Response>) => {
    pending.get(e.data.id)?.(e.data);
    pending.delete(e.data.id);
  };
  const send = (body: RequestBody) =>
    new Promise<string>((resolve, reject) => {
      const id = next++;
      pending.set(id, (r) => (r.ok ? resolve(r.json) : reject(new Error(r.error))));
      port.postMessage({ id, ...body } as Request);
    });

  const lanes = new Map<string, { busy: boolean; waiting: Waiting | null }>();
  const start = (lane: { busy: boolean; waiting: Waiting | null }, w: Waiting) => {
    lane.busy = true;
    send(w.body)
      .then(w.resolve, w.reject)
      .finally(() => {
        lane.busy = false;
        const queued = lane.waiting;
        lane.waiting = null;
        if (queued) start(lane, queued);
      });
  };
  const call = (body: RequestBody, laneName?: string) => {
    if (laneName === undefined) return send(body);
    let lane = lanes.get(laneName);
    if (!lane) lanes.set(laneName, (lane = { busy: false, waiting: null }));
    const l = lane;
    return new Promise<string>((resolve, reject) => {
      const w = { body, resolve, reject };
      if (!l.busy) return start(l, w);
      l.waiting?.reject(new SupersededError());
      l.waiting = w;
    });
  };

  return {
    simulateJson: (options) => call({ op: "simulate", options }),
    forecast: async (feedJson, options, lane) =>
      JSON.parse(await call({ op: "forecast", feedJson, options }, lane)) as ForecastReport,
  };
}
