// The web app's engine: the WASM in a Web Worker. Its own module because
// Vite emits the worker and the WASM for any module that names the worker,
// and only the web build imports this one (docs/phase4-design.md §2).

import { portEngine } from "./client";
import type { Engine } from "./client";

export function workerEngine(): Engine {
  return portEngine(new Worker(new URL("./worker.ts", import.meta.url), { type: "module" }));
}
