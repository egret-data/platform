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
  runQuery,
  engineProfileName,
  createPolicy,
} from "./helpers";

/**
 * Governance enforcement e2e against a RANGER-ON kind deploy (deploy-kind-governance).
 * Opt-in (EGRET_E2E_KIND=1), NOT in `npm test` / GitHub CI.
 *
 * Provisions a Ranger-bound Trino engine, seeds a governed Iceberg table, creates
 * access + column-mask + row-filter policies targeting the admin's own Trino user,
 * and asserts the admin's SELECT returns MASKED `item` values and ONLY country='US'
 * rows — proving Ranger mask + row-filter enforcement on a live query.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;
const POLL_MS = 12_000; // let the Trino Ranger plugin poll (pollIntervalMs ~5s)

test.describe.configure({ mode: "serial" });

test.describe("governance enforcement on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against a ranger-on kind deploy");
  test.setTimeout(900_000);

  let page: Page;
  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await loginLocalAdmin(page);
  });
  test.afterAll(async () => {
    await page.close();
  });

  test("column-mask + row-filter enforce on a governed Trino query", async () => {
    const request = page.request;
    const h = await csrf(page.context());
    // The impersonated Trino username = slugified email local-part (egret's
    // `ensure_username`). /api/auth/me returns the email; derive the same slug.
    const email = (await (await request.get("/api/auth/me")).json()).email as string;
    const username =
      email.split("@")[0].toLowerCase().replace(/[^a-z0-9_]+/g, "-").replace(/^-+|-+$/g, "") || "user";
    const T = "egret.demo.report"; // catalog.schema.table (egret names the Trino catalog `egret`)
    const policyIds: number[] = [];
    let engineId: string | undefined;

    try {
      // 1. Provision a Ranger-bound Trino engine.
      const ws = await firstWorkspace(request);
      const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);
      engineId = await provisionTrinoEngine(request, h, RUN_ID, warehouseId);
      await waitForEngineReady(request, engineId);
      await enableEngineJwt(request, h, engineId);
      await waitForEngineReady(request, engineId);
      await assignEngineToWorkspace(request, h, engineId, ws.id);
      const eng = `e2e-trino-${RUN_ID}`;
      const service = await engineProfileName(request, eng);

      // 2. Allow the admin to run queries + operate the catalog (default-deny needs
      //    execute + impersonate + table access BEFORE any query, incl. DDL/DML).
      const R = (o: Record<string, string[]>) =>
        Object.fromEntries(Object.entries(o).map(([k, v]) => [k, { values: v }]));
      policyIds.push(
        await createPolicy(request, h, {
          kind: "access", name: `e2e-exec-${RUN_ID}`, service,
          resources: R({ queryid: ["*"] }),
          items: [{ users: [username], accesses: ["execute"], effect: "allow" }],
        }),
        await createPolicy(request, h, {
          kind: "access", name: `e2e-imp-${RUN_ID}`, service,
          resources: R({ trinouser: ["*"] }),
          items: [{ users: [username], accesses: ["impersonate"], effect: "allow" }],
        }),
        await createPolicy(request, h, {
          kind: "access", name: `e2e-tbl-${RUN_ID}`, service,
          resources: R({ catalog: ["egret"], schema: ["*"], table: ["*"], column: ["*"] }),
          // NOTE: exact access strings for DDL/DML are finalized live (Task 6); "all"
          // is the broad grant. If the service-def rejects it, use the explicit list.
          items: [{ users: [username], accesses: ["all"], effect: "allow" }],
        }),
      );
      await new Promise((r) => setTimeout(r, POLL_MS));

      // 3. Seed a governed table with data (some US rows, some not).
      await runQuery(request, h, ws.slug, eng, "CREATE SCHEMA IF NOT EXISTS egret.demo");
      await runQuery(request, h, ws.slug, eng,
        `CREATE TABLE IF NOT EXISTS ${T} (id bigint, item varchar, country varchar)`);
      await runQuery(request, h, ws.slug, eng,
        `INSERT INTO ${T} VALUES (1,'book','US'),(2,'pen','VN'),(3,'lamp','US'),(4,'desk','VN')`);

      // Baseline: full data, raw item.
      const base = await runQuery(request, h, ws.slug, eng, `SELECT id, item, country FROM ${T} ORDER BY id`);
      expect(base.rows.length, "baseline sees all 4 rows").toBe(4);

      // 4. Add column-mask on `item` + row-filter country='US' for the admin.
      policyIds.push(
        await createPolicy(request, h, {
          kind: "masking", name: `e2e-mask-${RUN_ID}`, service,
          resources: R({ catalog: ["egret"], schema: ["demo"], table: ["report"], column: ["item"] }),
          items: [{ users: [username], maskType: "MASK" }],
        }),
        await createPolicy(request, h, {
          kind: "rowfilter", name: `e2e-rowf-${RUN_ID}`, service,
          resources: R({ catalog: ["egret"], schema: ["demo"], table: ["report"] }),
          items: [{ users: [username], filterExpr: "country = 'US'" }],
        }),
      );
      // 5. Enforcement: only US rows, and `item` masked (not the raw values).
      //    The Trino Ranger plugin polls every 30s (trino.rs poll_interval_ms), so
      //    re-query until the just-created mask + row-filter take effect.
      await expect(async () => {
        const enforced = await runQuery(request, h, ws.slug, eng, `SELECT id, item, country FROM ${T} ORDER BY id`);
        const itemIdx = enforced.columns.findIndex((c) => c.name === "item");
        const countryIdx = enforced.columns.findIndex((c) => c.name === "country");
        expect(enforced.rows.length, "row-filter → only 2 US rows").toBe(2);
        expect(enforced.rows.every((r) => r[countryIdx] === "US"), "only US rows").toBeTruthy();
        // MASK redacts the value; assert the raw items are gone.
        const items = enforced.rows.map((r) => r[itemIdx]);
        expect(items, "item is masked, not raw").not.toContain("book");
        expect(items, "item is masked, not raw").not.toContain("lamp");
      }).toPass({ timeout: 90_000, intervals: [5_000] });
    } finally {
      for (const id of policyIds) {
        await request.delete(`/api/control-plane/policies/${id}`, { headers: h }).catch(() => {});
      }
      if (engineId) await deleteEngine(request, h, engineId);
    }
  });
});
