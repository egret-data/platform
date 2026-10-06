import { test, expect, type Page } from "@playwright/test";
import { loginLocalAdmin, csrf, postJson, deleteEngine } from "./helpers";

/**
 * Catalog + engine provisioning through the CONTROL-PLANE UI, against the
 * in-cluster kind deploy. Opt-in (EGRET_E2E_KIND=1), NOT in `npm test` /
 * GitHub CI.
 *
 * SP3c-1 REWRITE. The spec this replaces drove a dialog (`getByRole("dialog")`,
 * `#cpe-name`, `#cpe-warehouse`, a button called "Add engine") that had already
 * become a full page at `/control-plane/engines/new` with `#ec-*` ids. It could
 * not have passed, and nothing said so, because `--list` was never run: a
 * Playwright spec that is skipped without a cluster compiles its selectors
 * nowhere. Run `npx playwright test --list tests/e2e/provisioning-ui-kind.spec.ts`
 * with every change to this file.
 *
 * What it drives, all of it through the browser:
 *   1. Catalogs → New catalog → kind `egret warehouse` → name/workspace/storage
 *      → Create catalog. (SP3c-1 Task 5.)
 *   2. Engines → New engine → attach that catalog and mark it primary → size
 *      the Trino worker down → Create engine. (Task 10.)
 *   3. The engine list row reaches `running` against real k8s.
 *   4. The CATALOG's Engines tab reports that engine `applied`. (Task 7.)
 *   5. Delete the engine through the UI; 6. delete the catalog through the UI.
 *
 * The ONE thing still seeded over the API is the S3 storage connection: it is
 * not part of SP3c-1's surface, has no page in this flow, and an e2e that seeds
 * what it is meant to be testing proves only that the last click works.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

const S3_ENDPOINT = process.env.EGRET_E2E_S3_ENDPOINT ?? "http://rustfs:9000";
const S3_KEY = process.env.EGRET_E2E_S3_KEY ?? "egret";
const S3_SECRET = process.env.EGRET_E2E_S3_SECRET ?? "egretsecret";
const S3_BUCKET = process.env.EGRET_E2E_BUCKET ?? "warehouse";

test.describe.configure({ mode: "serial" });

/**
 * Confirm a `ConfirmDeleteDialog` by typing WHATEVER IT ASKS FOR.
 *
 * `confirmText` is per call site and has already drifted once: the engine
 * delete dialog asks for the ENGINE NAME (`EngineDetail.tsx:370`), while the
 * catalogs list asks for the literal word `delete`. The spec this file
 * replaces hard-coded `getByPlaceholder("delete").fill("delete")`, which on
 * the engine dialog matches no element at all — so `fill` waits out the whole
 * 600 s test timeout and the failure says "timeout", not "wrong word". The
 * dialog states its own word in the input's placeholder; reading it there is
 * the one form that cannot rot.
 */
async function confirmDeleteDialog(page: Page, confirmLabel = "Delete"): Promise<void> {
  const dialog = page.getByRole("alertdialog");
  const input = dialog.getByRole("textbox");
  await expect(input).toBeVisible({ timeout: 15_000 });
  const word = await input.getAttribute("placeholder");
  expect(word, "the dialog names the word it wants in the placeholder").toBeTruthy();
  await input.fill(word!);
  const confirm = dialog.getByRole("button", { name: confirmLabel, exact: true });
  await expect(confirm).toBeEnabled();
  await confirm.click();
}

test.describe("catalog + engine provisioning via the UI", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.setTimeout(600_000);

  let page: Page;
  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    // ONCE. `/api/auth/login` is rate-limited to 5 attempts / 5 minutes, so a
    // per-test login locks the suite out of its own cluster.
    await loginLocalAdmin(page);
  });
  test.afterAll(async () => {
    await page.close();
  });

  test("create a catalog and an engine through the UI, reach running, delete both", async () => {
    const request = page.request;
    const h = await csrf(page.context());
    // `normalize_catalog_name` maps this to ITSELF — lowercase, digits and `_`
    // only, first character a letter — so the name typed into the warehouse
    // form is the catalog name the list shows back. Anything else and the two
    // assertions below would be about different strings.
    const catalogName = `e2e_cat_${RUN_ID}`;
    const engineName = `e2e-ui-${RUN_ID}`;
    const storageName = `e2e-store-${RUN_ID}`;
    let engineId: string | undefined;
    let catalogId: string | undefined;
    let warehouseId: string | undefined;
    let storageId: string | undefined;

    try {
      // The workspace the warehouse will belong to, by the NAME the picker
      // draws — `useMyWorkspaces` on the page reads this very endpoint.
      const wsRes = await request.get("/api/me/workspaces");
      expect(wsRes.ok(), `GET /api/me/workspaces → ${wsRes.status()}`).toBeTruthy();
      const wsList = (await wsRes.json()) as { id: string; name: string }[];
      expect(wsList.length, "at least one workspace (egret init seeds 'default')").toBeGreaterThan(
        0,
      );
      const wsName = wsList[0].name;

      // Setup, and the only setup: a storage connection for the warehouse to
      // sit on. Not part of SP3c-1.
      const storage = await postJson(request, "/api/control-plane/storage", h, {
        name: storageName,
        provider: "s3",
        region: "us-east-1",
        bucket: S3_BUCKET,
        endpoint: S3_ENDPOINT,
        key_prefix: `e2e-${RUN_ID}`,
        access_key: S3_KEY,
        path_style: true,
        new_secret: { name: `e2e-sec-${RUN_ID}`, value: S3_SECRET },
      });
      storageId = storage.id as string;

      // ── 1. Create the catalog through the UI ────────────────────────────
      await page.goto("/control-plane/catalogs");
      await page.getByRole("link", { name: "New catalog" }).click();
      await expect(page.getByRole("heading", { name: "New catalog" })).toBeVisible();

      // Radix `<Select>`: click the trigger, then the option in its portal.
      await page.locator("#cc-kind").click();
      await page.getByRole("option", { name: "egret warehouse" }).click();
      await page.locator("#cc-name").fill(catalogName);
      await page.locator("#cc-wh-workspace").click();
      await page.getByRole("option", { name: wsName, exact: true }).click();
      await page.locator("#cc-wh-storage").click();
      await page.getByRole("option", { name: storageName, exact: true }).click();

      const createCatalog = page.getByRole("button", { name: "Create catalog" });
      await expect(createCatalog).toBeEnabled();
      await createCatalog.click();

      // The warehouse branch returns the WAREHOUSE, not the catalog, so the
      // page goes back to the list rather than to a detail page — the catalog
      // appearing there IS the assertion that the registry row was written.
      await page.waitForURL(/\/control-plane\/catalogs$/, { timeout: 120_000 });
      const catalogLink = page.getByRole("link", { name: catalogName, exact: true });
      await expect(catalogLink).toBeVisible({ timeout: 120_000 });
      catalogId = (await catalogLink.getAttribute("href"))?.split("/").pop();
      expect(catalogId, "catalog id from the list row's href").toBeTruthy();

      // ── 2. Create the engine through the UI ─────────────────────────────
      await page.goto("/control-plane/engines/new");
      await page.locator("#ec-name").fill(engineName);

      // `#ec-warehouse` is the RadioGroup ROOT (Task 10 kept the old id on the
      // control that replaced the warehouse `<Select>`), so it is not clicked
      // and no option is picked from a portal: the rows are checkboxes and
      // radios. Marking a row primary attaches it as well, which is the
      // invariant the wizard maintains — asserted rather than assumed.
      const primary = page.locator(`#ec-primary-${catalogId}`);
      await expect(primary).toBeVisible({ timeout: 30_000 });
      await primary.click();
      await expect(page.locator(`#ec-attach-${catalogId}`)).toHaveAttribute(
        "data-state",
        "checked",
      );
      await expect(page.getByTestId("ec-needs-primary")).toHaveCount(0);

      // Size the cluster down through the Trino editor's own Worker section —
      // 1 worker at 1Gi fits kind; the wizard's `TRINO_FIELD_DEFAULTS` (2
      // workers, 4Gi) do not. `builtTrinoConfig` always sends both role blocks,
      // and the server overlays them onto the seeded ones, so what is typed
      // here is what the helm release gets.
      await page.getByRole("button", { name: "Worker", exact: true }).click();
      await page.locator("#ced-worker-replicas").fill("1");
      await page.locator("#ced-worker-cpu-request").fill("250m");
      await page.locator("#ced-worker-cpu-limit").fill("1");
      await page.locator("#ced-worker-mem-request").fill("1Gi");
      await page.locator("#ced-worker-mem-limit").fill("1Gi");
      await page.getByRole("button", { name: "Coordinator", exact: true }).click();
      await page.locator("#ced-coord-replicas").fill("1");
      await page.locator("#ced-coord-cpu-request").fill("250m");
      await page.locator("#ced-coord-cpu-limit").fill("1");
      await page.locator("#ced-coord-mem-request").fill("1Gi");
      await page.locator("#ced-coord-mem-limit").fill("1Gi");

      await page.getByRole("button", { name: "Create engine" }).click();

      // A create with no attach failures navigates straight to the engine; a
      // create with some renders `ec-attach-errors` INSTEAD and stays put. If
      // that panel is on screen the attach failed and the run must say so
      // rather than time out on a URL.
      //
      // RACED, not read before the POST has answered. This was a bare
      // `toHaveCount(0)` on the line after the click, and `toHaveCount(0)`
      // succeeds on its FIRST poll — so it passed while the request was still
      // in flight, whatever the outcome, and the guard this comment describes
      // could never fire: the run still timed out on `waitForURL` below and
      // still said nothing (whole-branch review, F4). It is the same "absence
      // read too early" shape as the engine delete at the end of this test,
      // and as Task 10's M13.
      //
      // Both `.catch` arms are deliberate: the loser of the race rejects on
      // its own timeout afterwards, and an unhandled rejection would be
      // reported as a failure of whichever test is running by then.
      const attachErrors = page.getByTestId("ec-attach-errors");
      const settled = /\/control-plane\/engines\/[0-9a-f-]{36}/;
      await Promise.race([
        page.waitForURL(settled, { timeout: 120_000 }).catch(() => {}),
        attachErrors.waitFor({ state: "visible", timeout: 120_000 }).catch(() => {}),
      ]);
      expect(
        await attachErrors.count(),
        "the wizard stayed on the create page and rendered the attach-error panel: the catalog " +
          "attach failed, so the engine was created without it",
      ).toBe(0);
      await page.waitForURL(settled, { timeout: 120_000 });
      engineId = page.url().split("/").pop()?.split("?")[0];
      expect(engineId, "engine id from the URL after create").toBeTruthy();

      // ── 3. The list row reaches `running` (real k8s; the badge polls 5s) ──
      await page.goto("/control-plane/engines");
      const row = page.getByRole("row").filter({ hasText: engineName });
      await expect(row).toBeVisible({ timeout: 30_000 });
      await expect(row.getByText(/^running$/i)).toBeVisible({ timeout: 300_000 });

      // ── 4. The CATALOG's Engines tab reports it applied ─────────────────
      await page.goto(`/control-plane/catalogs/${catalogId}?tab=engines`);
      const attachmentRow = page.getByRole("row").filter({ hasText: engineName });
      await expect(attachmentRow.getByText("applied", { exact: true })).toBeVisible({
        timeout: 120_000,
      });
      // The primary badge is the `is_primary` column, live (Task 2 / design D2).
      await expect(attachmentRow.getByText("primary", { exact: true })).toBeVisible();

      // ── 5. Delete the engine through the UI ─────────────────────────────
      await page.goto(`/control-plane/engines/${engineId}`);
      await page.getByRole("button", { name: "More engine actions" }).click();
      await page.getByRole("menuitem", { name: "Delete engine" }).click();
      // This dialog asks for the ENGINE NAME, not the word "delete".
      await confirmDeleteDialog(page);
      // WAIT FOR THE LIST FIRST. `EngineDetail` navigates here only on a
      // successful delete, so this is the falsifiable half. Asserting the
      // absence of a link named `engineName` while still on the DETAIL page
      // proves nothing — that page renders the name as a heading and never as
      // a link, so the assertion passes whether or not anything was deleted.
      // It did exactly that on the first run of this spec: the engine was
      // still `running` in Kubernetes and the step went green (SP3c-1 Task 11;
      // the same "absence read on the wrong page" shape Task 10's M13 found).
      await page.waitForURL(/\/control-plane\/engines$/, { timeout: 60_000 });
      await expect(page.getByRole("link", { name: engineName, exact: true })).toHaveCount(0, {
        timeout: 60_000,
      });
      engineId = undefined;

      // ── 6. Delete the catalog through the UI ────────────────────────────
      // An egret warehouse catalog is deleted with its WAREHOUSE — the registry
      // refuses it 409 — so this is the branch `confirmDelete` sends to
      // `deleteWarehouse`, and the row leaving the list is the proof it went to
      // the right endpoint.
      await page.goto("/control-plane/catalogs");
      await page.getByRole("button", { name: `Delete ${catalogName}` }).click();
      await confirmDeleteDialog(page);
      await expect(page.getByRole("link", { name: catalogName, exact: true })).toHaveCount(0, {
        timeout: 60_000,
      });
      catalogId = undefined;
    } finally {
      // Best effort, in the order the server's referential rules allow — and
      // wrapped whole, because on a TIMEOUT Playwright has already torn the
      // context down, so every call in here throws "Target page, context or
      // browser has been closed" and that error REPLACES the real failure in
      // the report. Losing the cleanup is cheap; losing the diagnosis is not.
      try {
        if (engineId) await deleteEngine(request, h, engineId);
        if (catalogId) {
          const res = await request.get(`/api/control-plane/catalogs/${catalogId}`);
          if (res.ok()) warehouseId = (await res.json()).warehouseId ?? undefined;
          if (warehouseId) {
            await request.delete(`/api/control-plane/warehouses/${warehouseId}`, { headers: h });
          } else {
            await request.delete(`/api/control-plane/catalogs/${catalogId}`, { headers: h });
          }
        }
        if (storageId) {
          await request.delete(`/api/control-plane/storage/${storageId}`, { headers: h });
        }
      } catch (e) {
        console.warn(`cleanup skipped: ${(e as Error).message}`);
      }
    }
  });
});
