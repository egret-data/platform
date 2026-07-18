import { test, expect } from "@playwright/test";
import {
  loginLocalAdmin,
  csrf,
  firstWorkspace,
  provisionStorageAndWarehouse,
  provisionTrinoEngine,
  waitForEngineReady,
  enableEngineJwt,
  assignEngineToWorkspace,
  deleteEngine,
  runSqlInLab,
} from "./helpers";

/**
 * Data-plane money-path e2e against the IN-CLUSTER kind deploy (egret.localtest.me).
 * Opt-in: set EGRET_E2E_KIND=1. NOT part of `npm test` / GitHub CI.
 *
 * Provisions the whole lifecycle via the egret API, then asserts impersonation in
 * SQL Lab: `SELECT current_user` returns the CALLER (admin), not `egret_delegator`.
 * Proves OIDC → engine JWKS → JWT → TLS:8443 → impersonation → query end-to-end.
 *
 * Prereq (one-time): a kind `egret` cluster + ingress + coredns-rewrite, then the
 * OIDC-on deploy — `just e2e-kind` handles the deploy + readiness wait.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

test.describe("data-plane on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");

  // Engine provisioning takes minutes (two provision→Ready waits + a reload-poll
  // for the engine to appear in SQL Lab); give the whole lifecycle 15 minutes.
  test.setTimeout(900_000);

  test("provision Trino → enable JWT → assign → SELECT current_user = caller", async ({
    page,
    context,
  }) => {
    await loginLocalAdmin(page);
    const request = page.request; // shares the logged-in session cookie
    const h = await csrf(context);
    let engineId: string | undefined;

    try {
      // 1. Storage + warehouse.
      const ws = await firstWorkspace(request);
      const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);

      // 2. Provision a small Trino engine and wait until it is Ready.
      engineId = await provisionTrinoEngine(request, h, RUN_ID, warehouseId);
      await waitForEngineReady(request, engineId);

      // 3. Enable egret-JWT auth, then wait for the coordinator to re-provision.
      await enableEngineJwt(request, h, engineId);
      await waitForEngineReady(request, engineId);

      // 4. Assign the engine to the workspace.
      await assignEngineToWorkspace(request, h, engineId, ws.id);

      // 5. Assert impersonation in SQL Lab.
      const out = await runSqlInLab(page, ws.slug, `e2e-trino-${RUN_ID}`, "SELECT current_user");
      expect(out, "SELECT current_user returns the caller (impersonation), not egret_delegator")
        .toMatch(/\badmin\b/);
      expect(out).not.toMatch(/egret_delegator/);
    } finally {
      // Delete the engine to free node memory (single kind node runs hot). Must run
      // here with the live `request` — a test.afterAll runs after the `page` fixture
      // (and page.request) is torn down, so cleanup there silently no-ops.
      if (engineId) await deleteEngine(request, h, engineId);
    }
  });
});
