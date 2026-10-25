/// <reference types="vitest/config" />
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  build: {
    target: "es2022",
  },
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
});
