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
} from "./helpers";

/**
 * Auto-admin policy e2e against a RANGER-ON kind deploy (deploy-kind-governance).
 * Opt-in (EGRET_E2E_KIND=1), NOT in `npm test` / GitHub CI.
 *
 * Proves the backend (sub-project A, Tasks 1-4) auto-seeds an "egret-admin" all-access
 * Ranger policy for every platform admin when a Ranger-bound Trino engine is provisioned.
 * Unlike `governance-kind.spec.ts` (which hand-seeds 3 access policies so its DDL/DML +
 * SELECT can run), this test creates NO access/mask/row-filter policy by hand: the admin
 * can seed a governed Iceberg table and SELECT it back SOLELY because of the auto-seeded
 * `egret-admin - …` policy. The deliberate contrast is the whole point.
 *
 * NOTE: only a StarRocks provisioning helper would let us mirror this against a StarRocks
 * service; helpers.ts exposes no StarRocks provisioning helper, so the StarRocks arm is
 * skipped here — the StarRocks branch of `seed_admin_all_access` is covered by the Task 1
 * unit test (`build_works_for_starrocks_service_type` in egret-policy).
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

/** One PolicySummary row from GET /api/control-plane/policies (snake_case). */
type PolicySummary = { id: number; name: string; principals: string[] };

test.describe.configure({ mode: "serial" });

test.describe("Ranger auto-admin policy on kind", () => {
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

  test("auto-admin: provision Ranger Trino → admin SELECT succeeds from the auto-seeded policy", async () => {
    const request = page.request;
    const h = await csrf(page.context());
    // The impersonated Trino username = slugified email local-part (egret's
    // `ensure_username`) — the same principal the backend seeds into the auto policy.
    const email = (await (await request.get("/api/auth/me")).json()).email as string;
    const username =
      email.split("@")[0].toLowerCase().replace(/[^a-z0-9_]+/g, "-").replace(/^-+|-+$/g, "") || "user";
    const T = "egret.demo.autoadmin"; // catalog.schema.table (egret names the Trino catalog `egret`)
    let engineId: string | undefined;

    try {
      // 1. Provision a Ranger-bound Trino engine (identical ordering to governance-kind).
      const ws = await firstWorkspace(request);
      const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);
      engineId = await provisionTrinoEngine(request, h, RUN_ID, warehouseId);
      await waitForEngineReady(request, engineId);
      await enableEngineJwt(request, h, engineId);
      await waitForEngineReady(request, engineId);
      await assignEngineToWorkspace(request, h, engineId, ws.id);
      const eng = `e2e-trino-${RUN_ID}`;
      const service = await engineProfileName(request, eng);

      // 2. Seed a governed Iceberg table — with NO hand-created access policy. This DDL/DML
      //    can only succeed because the backend auto-seeded the `egret-admin` all-access
      //    policy (execute + impersonate + catalog/schema/table/column) at provision time.
      //    runQuery retries internally while the Trino Ranger plugin (poll ~30s) picks it up.
      await runQuery(request, h, ws.slug, eng, "CREATE SCHEMA IF NOT EXISTS egret.demo");
      await runQuery(request, h, ws.slug, eng,
        `CREATE TABLE IF NOT EXISTS ${T} (id bigint, item varchar)`);
      await runQuery(request, h, ws.slug, eng,
        `INSERT INTO ${T} VALUES (1,'apple'),(2,'banana')`);

      // 3. Admin SELECT succeeds and returns the seeded rows — access comes SOLELY from the
      //    auto-seeded policy (no mask, no row-filter, no hand-granted SELECT).
      const out = await runQuery(request, h, ws.slug, eng, `SELECT id, item FROM ${T} ORDER BY id`);
      const idIdx = out.columns.findIndex((c) => c.name === "id");
      const itemIdx = out.columns.findIndex((c) => c.name === "item");
      expect(out.rows.length, "admin sees both seeded rows via the auto-admin policy").toBe(2);
      expect(out.rows.map((r) => String(r[idIdx]))).toEqual(["1", "2"]);
      expect(out.rows.map((r) => r[itemIdx])).toEqual(["apple", "banana"]);

      // 4. Assert the auto-policy actually exists in Ranger for this service: at least one
      //    `egret-admin …` policy whose principals include the admin's Trino username. This
      //    is the policy the query above relied on — not one this test ever created.
      const policies = (await (
        await request.get(`/api/control-plane/policies?service=${encodeURIComponent(service)}`)
      ).json()) as PolicySummary[];
      const adminPolicy = policies.find(
        (p) => p.name.startsWith("egret-admin") && p.principals.includes(username),
      );
      expect(
        adminPolicy,
        `auto-seeded egret-admin policy granting '${username}' exists on service '${service}'`,
      ).toBeTruthy();
    } finally {
      if (engineId) await deleteEngine(request, h, engineId);
    }
  });
});
