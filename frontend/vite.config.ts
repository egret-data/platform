import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// See vitest.config.ts — Vite 8's native config loader has no CJS `__dirname`.
const here = import.meta.dirname;

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: [
      { find: "@", replacement: path.resolve(here, "src") },
      // `monaco-yaml`'s worker (through `monaco-worker-manager/worker`) imports
      // `monaco-editor/esm/vs/editor/editor.worker.js`, a path monaco 0.56's
      // exports map rewrites to `esm/vs/esm/vs/…`, which does not exist.
      {
        find: /^monaco-editor\/esm\/vs\/editor\/editor\.worker\.js$/,
        replacement: path.resolve(here, "node_modules/monaco-editor/esm/vs/editor/editor.worker.js"),
      },
    ],
  },
  // react-grid-layout (via react-draggable) reads `process.env`, which isn't defined
  // in the browser under Vite — it throws on drag. Provide NODE_ENV (for prod
  // dead-code elimination) and an empty fallback for any other process.env reads.
  // `monaco-yaml`'s worker imports `path-browserify`, which is CommonJS. The dev server serves a
  // worker's node_modules imports as they are unless they are pre-bundled, and the worker then dies
  // with "module is not defined" (measured in Chromium); the production build converts it anyway.
  // Kestra pre-bundles it for the same reason.
  optimizeDeps: { include: ["path-browserify"] },
  define: {
    "process.env.NODE_ENV": JSON.stringify(mode),
    "process.env": "{}",
  },
  server: {
    // `ws: true` proxies WebSocket upgrades (the Data Apps pod proxy and engine
    // log streaming) to the backend; without it, those connections fail in dev.
    // `/catalog` (embedded Iceberg REST) + `/oauth` (mini-IdP) let engines and the
    // catalog e2e reach the backend through the dev server, same-origin.
    proxy: {
      "/api": { target: "http://localhost:8080", changeOrigin: true, ws: true },
      "/catalog": { target: "http://localhost:8080", changeOrigin: true },
      "/oauth": { target: "http://localhost:8080", changeOrigin: true },
    },
  },
}));
