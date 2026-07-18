# E2E Engine Provisioning via the UI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One Playwright spec that drives the control-plane engine-provisioning UI — fill the "Add engine" form for a Trino cluster, wait for the status badge to reach `running`, then delete the engine via its detail page.

**Architecture:** Warehouse seeded via the API (reuse slice-1 helper); the engine lifecycle is driven through the browser UI. Login once (shared page, serial — slice-2 rate-limit lesson). `try/finally` deletes the engine via the API as a safety net.

**Tech Stack:** Playwright, TypeScript.

---

## Design notes (grounded selectors)

`ControlPlaneEngines.tsx` (`/control-plane/engines`) — Trino is the **default active tab** (`ENGINE_TYPES[0]`). Header **"Add engine"** button opens `AddEngineDialog` (title **"Add Trino engine"**):
- `#cpe-name` (name), `#cpe-warehouse` (Radix `SelectTrigger`, placeholder "Select a warehouse" → `option` per provisioned warehouse, text = warehouse name), `#cpe-coordinators`, `#cpe-workers`, `#cpe-cpu-min`, `#cpe-cpu-max`, `#cpe-memory`.
- The dialog **submit** button is also labelled **"Add engine"** → scope it to the dialog to avoid matching the header button.
- Only **provisioned** warehouses (with `lakekeeperWarehouseId`) appear in the Select.

Engine list rows: the name is a `Link` to `/control-plane/engines/{id}`; a **"Details"** link (`aria-label="View {name}"`); and a live **`<EngineStatusBadge>`** that polls `getEngine` every 5s and renders `status.phase` as the badge **text** (`provisioning` → `running`).

`EngineDetail.tsx` (`/control-plane/engines/{id}`) — a `⋯` button (`aria-label="More engine actions"`) → `menuitem` **"Delete engine"** → `ConfirmDeleteDialog` (title `Delete «{name}»?`, `confirmText="delete"` → type `delete` → **"Delete"** button) → deletes + navigates back to `/engines`.

Reuse slice-1 helpers: `csrf`, `firstWorkspace`, `provisionStorageAndWarehouse` (returns `{ storageId, warehouseId, warehouseName }`), `deleteEngine`. Provision-to-running needs no S3/JWT (the coordinator connects to S3 lazily), so bucket `e2e` with the default (bare `minio`) endpoint is enough — no FQDN needed.

## File Structure

- **Create** `frontend/tests/e2e/provisioning-ui-kind.spec.ts` — the UI provisioning spec.
- **Modify** `frontend/tests/e2e/README.md` — add the provisioning-UI subsection.

---

### Task 1: Provisioning-UI spec

**Files:**
- Create: `frontend/tests/e2e/provisioning-ui-kind.spec.ts`

- [ ] **Step 1: Write the spec**

Create `frontend/tests/e2e/provisioning-ui-kind.spec.ts`:

```ts
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
```

- [ ] **Step 2: Typecheck + gate-skip**

Run: `cd frontend && npm run typecheck && npx playwright test provisioning-ui-kind`
Expected: typecheck PASS; Playwright reports **1 skipped** (gate `EGRET_E2E_KIND` unset — do NOT set it). If browsers are missing, `npx playwright install chromium` first.

- [ ] **Step 3: Commit**

```bash
git add frontend/tests/e2e/provisioning-ui-kind.spec.ts
git commit -m "test(e2e): engine provisioning via the UI spec (Trino add → running → delete)"
```

> Note: the form field ids, the warehouse `option`, the status-badge text, and the delete menu/confirm are pinned from `ControlPlaneEngines.tsx` / `EngineDetail.tsx`, but browser selectors are finalized against the live UI in Task 2 — expect minor tweaks (e.g. the "Details" link name, the row/badge scoping).

---

### Task 2: Document + live acceptance

**Files:**
- Modify: `frontend/tests/e2e/README.md`

- [ ] **Step 1: Add the README subsection**

Append to `frontend/tests/e2e/README.md`:

```markdown
## Engine provisioning via the UI (`provisioning-ui-kind.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), runs against any in-cluster deploy (e.g. after
`just e2e-kind`). Seeds a warehouse via the API, then drives the control-plane
UI: fills the **Add engine** form for a Trino cluster (1 worker, 1Gi), waits for
the row's status badge to reach **running** (~1–2 min of real k8s provisioning),
then deletes the engine via its detail page. Needs `EGRET_E2E_BUCKET=e2e` (the
default `warehouse` bucket carries a lakekeeper orphan — see the data-plane note).

```bash
cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
  EGRET_E2E_BUCKET=e2e npx playwright test provisioning-ui-kind
```
```

- [ ] **Step 2: Commit**

```bash
git add frontend/tests/e2e/README.md
git commit -m "docs(e2e): document engine provisioning via the UI spec"
```

- [ ] **Step 3: Live acceptance**

Ensure a deploy is up (from a prior slice, or `just e2e-kind`). Run:
```bash
cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
  EGRET_E2E_BUCKET=e2e npx playwright test provisioning-ui-kind --reporter=list
```
Expected: **1 passed** — the UI-created Trino reaches `running`, then deletes.

- [ ] **Step 4: Fix any selector drift**

If a form field / warehouse option / status badge / delete step fails, adjust it against the live DOM (Playwright's failure screenshot/trace) and re-run until green. Commit any fix:

```bash
git add frontend/tests/e2e/provisioning-ui-kind.spec.ts
git commit -m "test(e2e): finalize provisioning-UI selectors against live UI"
```

- [ ] **Step 5: Confirm unit gates**

Run: `cd frontend && npm run lint && npm run typecheck && npm test`
Expected: all pass (the spec is opt-in, excluded from Vitest).

---

## Self-Review

- **Spec coverage:** Add-engine form fill (Task 1) ✓; wait for `running` status badge ✓; delete via the UI detail page ✓; API warehouse setup + `finally` safety-net cleanup ✓; docs (Task 2) ✓; live acceptance (Task 2) ✓.
- **No placeholders:** form ids, warehouse option, badge text, delete menu/confirm pinned from source. Browser-selector finalization is the explicit Task-2 live step.
- **Type consistency:** `csrf` / `firstWorkspace` / `provisionStorageAndWarehouse` / `deleteEngine` used match their slice-1 helper signatures; `provisionStorageAndWarehouse` returns `warehouseName`.
- **Small-resources + no-JWT:** 1 worker / 1Gi keeps provisioning fast + OOM-free; no JWT/query avoids slice-1's flakiest paths.
