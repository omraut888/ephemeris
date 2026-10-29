import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import type { FileHost } from "./files";
import "./styles.css";
import type { Engine } from "./wasm/client";

/** The engine and file host for this build. `MODE` is a build-time
 * constant, so each build keeps one branch: the web bundle has no Tauri
 * code and the desktop bundle no WASM (docs/phase4-design.md §2). */
function shell(): Promise<{ engine: Engine; files?: FileHost }> {
  return import.meta.env.MODE === "desktop"
    ? import("./desktop/shell").then((m) => m.desktopShell())
    : import("./wasm/browser").then((m) => ({ engine: m.workerEngine() }));
}

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");
void shell().then(({ engine, files }) =>
  createRoot(root).render(
    <StrictMode>
      <App engine={engine} files={files} />
    </StrictMode>,
  ),
);
