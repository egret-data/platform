import { test, expect, type Page } from "@playwright/test";
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
} from "./helpers";

/**
 * Notebook ⇄ Trino + Iceberg lakehouse e2e against the in-cluster kind deploy.
 * Opt-in: EGRET_E2E_KIND=1. NOT part of `npm test` / GitHub CI.
 *
 * Provisions a governed Trino engine wired to an Iceberg warehouse, then drives a
 * notebook through the full money path on a REAL per-user kernel:
 *   1. `trino("SELECT …")`            — read from Trino into a Daft DataFrame
 *   2. `write_table(df, "egret.…")`   — write that DataFrame to the Iceberg lakehouse
 *   3. `trino("SELECT … FROM …")`     — read it back through Trino
 * The roundtrip line proves read + write + read against the live engine + lakehouse
 * (the auto-provisioned warehouse catalog is named `egret`).
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

async function setFirstCellSource(page: Page, code: string) {
  await page.waitForFunction(() => (window as any).monaco?.editor?.getEditors?.().length > 0, null, {
    timeout: 20_000,
  });
  await page.evaluate((src) => {
    (window as any).monaco.editor.getEditors()[0].setValue(src);
  }, code);
}

async function newNotebook(page: Page, name: string) {
  await page.getByRole("link", { name: "Notebooks" }).click();
  await page.getByRole("button", { name: "+ Notebook" }).click();
  await page.getByRole("textbox", { name: "Name" }).fill(name);
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.waitForURL(/\/notebooks\/[0-9a-f-]{36}$/, { timeout: 20_000 });
}

test.describe("notebook ⇄ Trino + lakehouse on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  // Provisioning a Trino engine (two Ready waits) + a kernel cold-start + an
  // Iceberg write/read roundtrip — give the whole lifecycle 15 minutes.
  test.setTimeout(900_000);

  test("read from Trino, write to the Iceberg lakehouse, read it back", async ({ page, context }) => {
    await loginLocalAdmin(page);
    const request = page.request; // shares the logged-in session cookie
    const h = await csrf(context);
    let engineId: string | undefined;

    try {
      // Provision storage + warehouse + a JWT-enabled Trino engine, assigned to the ws.
      const ws = await firstWorkspace(request);
      const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);
      engineId = await provisionTrinoEngine(request, h, RUN_ID, warehouseId);
      await waitForEngineReady(request, engineId);
      await enableEngineJwt(request, h, engineId);
      await waitForEngineReady(request, engineId);
      await assignEngineToWorkspace(request, h, engineId, ws.id);

      // Assigning an engine creates a notebook DB-connection for the ws, but it is
      // DISABLED by default — `trino()` fails with "engine not enabled for
      // notebooks" until it is turned on. The notebook-settings API keys off the
      // analytics *connection* id (distinct from the control-plane engine id), so
      // read it back from GET, then enable it. Use the connection *name* as the
      // trino() target.
      const settings = await (await request.get(`/api/ws/${ws.slug}/notebook-settings`)).json();
      const conn = (settings.engines as { id: string; name: string; engine: string }[]).find(
        (e) => e.engine === "trino",
      );
      expect(conn, "a trino notebook connection exists after assignment").toBeTruthy();
      const put = await request.put(`/api/ws/${ws.slug}/notebook-settings`, {
        headers: h,
        data: { engine_ids: [conn!.id] },
      });
      expect(put.ok(), `enable notebook engine → ${put.status()} ${await put.text()}`).toBeTruthy();
      const engineName = conn!.name;

      // Create the notebook AFTER enabling the engine so the kernel's runtime
      // manifest (loaded at kernel start) includes it → trino() resolves it.
      await newNotebook(page, `nb-lakehouse ${RUN_ID}`);
      const table = `egret.analytics.nb_e2e_${RUN_ID}`;
      const marker = `nb-${RUN_ID}`;
      await setFirstCellSource(
        page,
        [
          "import daft",
          // 1) read from Trino into a Daft DataFrame (governed, per-user JWT).
          //    Target the connection by name (a bare kind may not resolve).
          `df = trino("SELECT '${marker}' AS who, CAST(6 * 7 AS bigint) AS n", engine="${engineName}")`,
          "print('READ_OK', df.to_pydict()['n'][0])",
          // 2) write it to the Iceberg lakehouse through the governed engine
          `write_table(df, "${table}", engine="${engineName}", mode="overwrite")`,
          "print('WRITE_OK')",
          // 3) read it back from the lakehouse through Trino
          `back = trino("SELECT who, n FROM ${table}", engine="${engineName}").to_pydict()`,
          "print('ROUNDTRIP', back['who'][0], back['n'][0])",
        ].join("\n"),
      );
      await page.getByRole("button", { name: "Run all" }).click();

      // `${marker} 42` appears ONLY in the roundtrip output — the source has the
      // marker and `6 * 7` separately, never the adjacent literal (mirrors
      // notebook.spec.ts) — so it proves the read→write→read actually ran.
      await expect(page.getByText(`ROUNDTRIP ${marker} 42`)).toBeVisible({ timeout: 180_000 });
    } finally {
      // Delete the engine to free node memory (single kind node runs hot). Must run
      // here with the live `request` — a test.afterAll runs after page.request is
      // torn down, so cleanup there silently no-ops.
      if (engineId) await deleteEngine(request, h, engineId);
    }
  });
});
