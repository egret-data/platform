import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// See vitest.config.ts — Vite 8's native config loader has no CJS `__dirname`.
const here = import.meta.dirname;

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(here, "src") } },
  // react-grid-layout (via react-draggable) reads `process.env`, which isn't defined
  // in the browser under Vite — it throws on drag. Provide NODE_ENV (for prod
  // dead-code elimination) and an empty fallback for any other process.env reads.
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
