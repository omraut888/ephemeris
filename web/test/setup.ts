// Every test runs against the real engine: the wasm-pack output, loaded
// from disk (npm test builds it first).

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { afterEach } from "vitest";

import { loadEngineSync } from "../src/wasm/engine";

// Paths are from web/, where Vitest runs (under jsdom, import.meta.url is
// not a file: URL).
loadEngineSync(readFileSync(resolve("src/wasm/pkg/ephemeris_wasm_bg.wasm")));

// jsdom tests render with Testing Library; unmount between tests.
afterEach(async () => {
  if (typeof document !== "undefined") {
    const { cleanup } = await import("@testing-library/react");
    cleanup();
  }
});
