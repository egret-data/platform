import { test, expect, type Page } from "@playwright/test";
import { loginLocalAdmin, csrf, firstWorkspace, provisionStorageAndWarehouse } from "./helpers";

/**
 * Notebook write-isolation e2e against the IN-CLUSTER kind deploy (Sub-project C).
 * Opt-in: set EGRET_E2E_KIND=1. NOT part of `npm test` / GitHub CI.
 *
 * Proves the private-namespace scratch plane end-to-end on a REAL per-user kernel:
 *   - An admin sets the workspace's notebook (scratch) warehouse + reserved
 *     namespace (`PUT /api/ws/{ws}/notebook-settings/warehouse`).
 *   - The kernel receives a namespace-scoped catalog token in its runtime manifest
 *     and `write_iceberg(df, name)` / `read_iceberg(name)` round-trip a Daft frame
 *     DIRECTLY through that reserved Iceberg namespace — no engine, no Trino.
 *   - The catalog guard confines that token: a raw pyiceberg TABLE write to a
 *     DIFFERENT namespace is denied.
 *
 * The assertions target OUTPUT-only markers (SCRATCH_ROWS=3, ISOLATION_RESULT=DENIED)
 * so a green run proves the kernel actually executed, not just that source is present.
 *
 * NO Ranger needed — the notebook kernel stack (`deploy-kind-oidc`) suffices.
 * Run via `just e2e-kind-notebook-write`.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

/** Set the Nth mounted Monaco editor's value (typing into Monaco is flaky). */
async function setCellSource(page: Page, index: number, code: string) {
  await page.waitForFunction(
    (i) => (window as any).monaco?.editor?.getEditors?.().length > i,
    index,
    { timeout: 20_000 },
  );
  await page.evaluate(
    ({ i, src }) => (window as any).monaco.editor.getEditors()[i].setValue(src),
    { i: index, src: code },
  );
}

async function newNotebook(page: Page, name: string) {
  await page.getByRole("link", { name: "Notebooks" }).click();
  await page.getByRole("button", { name: "+ Notebook" }).click();
  await page.getByRole("textbox", { name: "Name" }).fill(name);
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.waitForURL(/\/notebooks\/[0-9a-f-]{36}$/, { timeout: 20_000 });
}

/** A direct write→read round-trip through the private namespace (no engine). The
 *  literal "SCRATCH_ROWS=3" never appears in the source, so seeing it proves the
 *  write + read-back both ran against the reserved Iceberg namespace. */
const SCRATCH_CELL = [
  "import daft",
  'df = daft.from_pydict({"x": [1, 2, 3], "label": ["a", "b", "c"]})',
  'write_iceberg(df, "scratch", mode="overwrite")',
  'back = read_iceberg("scratch")',
  'print("SCRATCH_ROWS=" + str(back.count_rows()))',
].join("\n");

/** Drive a RAW pyiceberg write to a DIFFERENT namespace with the runtime's own
 *  (namespace-scoped) token. Namespace CREATION may be permitted by the guard (a
 *  known nuance), but a TABLE create/write outside the reserved namespace MUST be
 *  denied — that table write is the confinement invariant we assert on. */
const CONFINEMENT_CELL = [
  "import daft",
  "from egret_kernel_helpers.runtime import load_runtime",
  "from pyiceberg.catalog.rest import RestCatalog",
  "rt = load_runtime()",
  'df = daft.from_pydict({"x": [1, 2, 3]})',
  'cat = RestCatalog("egret", uri=rt.catalog_uri, token=rt.catalog_jwt, warehouse=rt.catalog_warehouse)',
  'result = "UNKNOWN"',
  "try:",
  "    try:",
  '        cat.create_namespace(("evil",))',
  "    except Exception:",
  "        pass  # namespace creation may be allowed; the table write is the invariant",
  '    cat.create_table(("evil", "t"), schema=df.to_arrow().schema)',
  '    result = "LEAKED"',
  "except Exception:",
  '    result = "DENIED"',
  'print("ISOLATION_RESULT=" + result)',
].join("\n");

test.describe("notebook write isolation on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.describe.configure({ mode: "serial" });
  // First run cold-starts a per-user kernel pod (image pull + jupyter boot).
  test.setTimeout(300_000);

  test("notebook writes + reads its private namespace directly", async ({ page, context }) => {
    await loginLocalAdmin(page);
    const request = page.request; // shares the logged-in session cookie
    const h = await csrf(context);

    // Provision storage + an Iceberg warehouse, then reserve it as the workspace's
    // notebook (scratch) warehouse under the `notebook` namespace.
    const ws = await firstWorkspace(request);
    const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);
    const res = await request.put(`/api/ws/${ws.slug}/notebook-settings/warehouse`, {
      headers: h,
      data: { warehouse_id: warehouseId, namespace: ["notebook"] },
    });
    expect(
      res.ok(),
      `set notebook warehouse → ${res.status()} ${await res.text()}`,
    ).toBeTruthy();

    // Round-trip a Daft frame through the reserved namespace.
    await newNotebook(page, `write-iso ${RUN_ID}`);
    await setCellSource(page, 0, SCRATCH_CELL);
    await page.getByRole("button", { name: "Run all" }).click();

    // First run also cold-starts the per-user kernel pod → generous wait.
    await expect(page.getByText("SCRATCH_ROWS=3")).toBeVisible({ timeout: 150_000 });
  });

  test("an out-of-namespace write is denied by the catalog guard", async ({ page }) => {
    // The notebook warehouse is already set from the previous (serial) test; the
    // runtime token is namespace-scoped, so a raw pyiceberg table create in a
    // different namespace must fail the catalog guard.
    await loginLocalAdmin(page);
    await newNotebook(page, `write-iso-confine ${RUN_ID}`);
    await setCellSource(page, 0, CONFINEMENT_CELL);
    await page.getByRole("button", { name: "Run all" }).click();

    await expect(page.getByText("ISOLATION_RESULT=DENIED")).toBeVisible({ timeout: 150_000 });
  });
});
