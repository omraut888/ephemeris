// The engine runs here, off the main thread: a 90-day, 2000-path forecast
// takes 150–250 ms (docs/overnight-log.md), long enough to stall input.

/// <reference lib="webworker" />

import { loadEngine } from "./engine";
import { handle } from "./protocol";
import type { Request, Response } from "./protocol";

const ready = loadEngine();

self.onmessage = async (e: MessageEvent<Request>) => {
  let res: Response;
  try {
    await ready;
    res = handle(e.data);
  } catch (err) {
    res = {
      id: e.data.id,
      ok: false,
      error: `engine failed to load: ${err instanceof Error ? err.message : String(err)}`,
    };
  }
  (self as unknown as DedicatedWorkerGlobalScope).postMessage(res);
};
