// The Vite dev server for an e2e run on a PRIVATE stack (`workflows-flows.spec.ts`):
// `vite.config.ts` unchanged except its port and the backend it proxies to, so the
// run never reaches the shared dev backend on :8080 or a dev server on :5173.
//
//   EGRET_E2E_VITE_PORT  the dev server's port      (default 5197)
//   EGRET_E2E_API        the private backend's URL  (default http://localhost:8097)
//
//   npx vite --config tests/e2e/vite.e2e.config.mts     # from frontend/
import path from "node:path";
import type { ConfigEnv, UserConfig } from "vite";
import base from "../../vite.config.ts";

const frontend = path.resolve(import.meta.dirname, "../..");
const port = Number(process.env.EGRET_E2E_VITE_PORT ?? 5197);
const api = process.env.EGRET_E2E_API ?? "http://localhost:8097";

export default (env: ConfigEnv): UserConfig => {
  const config = typeof base === "function" ? base(env) : base;
  return {
    ...(config as UserConfig),
    root: frontend,
    server: {
      ...(config as UserConfig).server,
      port,
      strictPort: true,
      proxy: {
        "/api": { target: api, changeOrigin: true, ws: true },
        "/catalog": { target: api, changeOrigin: true },
        "/oauth": { target: api, changeOrigin: true },
      },
    },
  };
};
