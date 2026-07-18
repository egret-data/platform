import { test, expect, type Page } from "@playwright/test";
import {
  loginLocalAdmin,
  csrf,
  firstWorkspace,
  provisionStorageAndWarehouse,
  deleteEngine,
} from "./helpers";

/**
 * Engine provisioning via the CONTROL-PLANE UI, against the in-cluster kind deploy.
 * Opt-in (EGRET_E2E_KIND=1), NOT in `npm test` / GitHub CI.
 *
 * Seeds a warehouse via the API, then drives the browser: Add-engine form for a
 * Trino cluster → wait for the live status badge to reach `running` → delete via
 * the engine detail page. Proves the provisioning UI works end-to-end.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

test.describe.configure({ mode: "serial" });

test.describe("engine provisioning via the UI", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.setTimeout(600_000);

  let page: Page;
  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await loginLocalAdmin(page);
  });
  test.afterAll(async () => {
    await page.close();
  });

  test("add a Trino engine via the form, reach running, delete via the UI", async () => {
    const request = page.request;
    const h = await csrf(page.context());
    const name = `e2e-ui-${RUN_ID}`;
    let engineId: string | undefined;

    try {
      // Setup (API): a provisioned warehouse the Add-engine form can pick.
      const ws = await firstWorkspace(request);
      const { warehouseName } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);

      // Open the Add-engine dialog (Trino is the default active tab).
      await page.goto("/control-plane/engines");
      await page.getByRole("button", { name: "Add engine" }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.getByText("Add Trino engine")).toBeVisible();

      // Fill the form: name, warehouse, small resources (1 worker, 1Gi → fast + no OOM).
      await dialog.locator("#cpe-name").fill(name);
      await dialog.locator("#cpe-warehouse").click();
      await page.getByRole("option", { name: warehouseName }).click();
      await dialog.locator("#cpe-workers").fill("1");
      await dialog.locator("#cpe-memory").fill("1Gi");
      await dialog.getByRole("button", { name: "Add engine" }).click();

      // The engine appears in the Trino table; grab its id from the row link.
      const row = page.getByRole("row").filter({ hasText: name });
      await expect(row).toBeVisible({ timeout: 30_000 });
      const href = await row.getByRole("link", { name, exact: true }).getAttribute("href");
      engineId = href?.split("/").pop();

      // Wait for the live status badge to reach `running` (real k8s provisioning +
      // Trino startup; the badge auto-polls every 5s).
      await expect(row.getByText(/^running$/i)).toBeVisible({ timeout: 300_000 });

      // Delete via the UI: engine detail → ⋯ → Delete engine → confirm.
      await row.getByRole("link", { name: /Details/ }).click();
      await page.getByRole("button", { name: "More engine actions" }).click();
      await page.getByRole("menuitem", { name: "Delete engine" }).click();
      await page.getByPlaceholder("delete").fill("delete");
      await page.getByRole("button", { name: "Delete", exact: true }).click();

      // Back on the list, the engine is gone.
      await expect(page.getByRole("link", { name, exact: true })).toHaveCount(0, {
        timeout: 30_000,
      });
    } finally {
      if (engineId) await deleteEngine(request, h, engineId);
    }
  });
});
