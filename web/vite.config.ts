/// <reference types="vitest/config" />
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// `--mode desktop` builds the Tauri app's UI (docs/phase4-design.md §4):
// the same sources, its own output folder and dev port.
export default defineConfig(({ mode }) => ({
  plugins: [react()],
  build: {
    target: "es2022",
    outDir: mode === "desktop" ? "dist-desktop" : "dist",
  },
  server: mode === "desktop" ? { port: 5174, strictPort: true } : {},
  // Keep Tauri's output visible when it runs the dev server.
  clearScreen: mode !== "desktop",
  // The engine runs in a module worker that imports the WASM package.
  worker: {
    format: "es",
  },
  test: {
    // Component tests need a DOM; the bridge test overrides this to node.
    environment: "jsdom",
    include: ["test/**/*.test.{ts,tsx}"],
    setupFiles: ["test/setup.ts"],
  },
}));
