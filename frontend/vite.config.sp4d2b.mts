// SCRATCH config for the SP4d-2b Task 8 browser pass: this branch's own backend
// on :8090 and its own database, on a port nothing else holds. Deleted after the
// run — a stale Vite on :5173 pointing at the stale :8080 is exactly what this
// avoids. Identical to vite.config.ts except the port and the proxy target.
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

const here = import.meta.dirname;

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(here, "src") } },
  define: {
    "process.env.NODE_ENV": JSON.stringify(mode),
    "process.env": "{}",
  },
  server: {
    port: 5290,
    strictPort: true,
    proxy: {
      "/api": { target: "http://localhost:8090", changeOrigin: true, ws: true },
      "/catalog": { target: "http://localhost:8090", changeOrigin: true },
      "/oauth": { target: "http://localhost:8090", changeOrigin: true },
    },
  },
}));
