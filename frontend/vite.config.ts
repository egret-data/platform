import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(__dirname, "src") } },
  // react-grid-layout (via react-draggable) reads `process.env`, which isn't defined
  // in the browser under Vite — it throws on drag. Provide NODE_ENV (for prod
  // dead-code elimination) and an empty fallback for any other process.env reads.
  define: {
    "process.env.NODE_ENV": JSON.stringify(mode),
    "process.env": "{}",
  },
  server: {
    proxy: { "/api": { target: "http://localhost:8080", changeOrigin: true } },
  },
}));
