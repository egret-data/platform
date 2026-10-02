import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright end-to-end tests for egret.
 *
 * These drive a REAL browser against a running local stack — they are NOT part of
 * the Vitest unit suite and do NOT run in the standard `npm test`. Prerequisites
 * (see tests/e2e/README.md): docker services up, the backend `serve` running on
 * :8080 with `jupyter kernelgateway` + `ipykernel` on its PATH (so notebook cells
 * can spawn a real kernel), the Vite dev server on :5173, and the target workspace
 * provisioned with an Iceberg warehouse. Run with `npm run test:e2e`.
 */
export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: false,
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  reporter: [["list"]],
  use: {
    baseURL: process.env.EGRET_E2E_BASE_URL ?? "http://localhost:5173",
    // The apps-on-kind run serves a self-signed certificate: `__Host-` cookies exist only over HTTPS.
    ignoreHTTPSErrors: process.env.EGRET_E2E_INSECURE_TLS === "1",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
