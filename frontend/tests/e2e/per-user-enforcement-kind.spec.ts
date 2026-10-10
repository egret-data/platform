import { test, expect, type Page, type BrowserContext, type APIRequestContext } from "@playwright/test";
import {
  loginLocalAdmin,
  loginViaOidcAdminSso,
  loginViaOidcMember,
  csrf,
  firstWorkspace,
  provisionStorageAndWarehouse,
  provisionTrinoEngineJwt,
  waitForEngineReady,
  assignEngineToWorkspace,
  engineProfileName,
  createPolicy,
  addWorkspaceMember,
  createDataset,
  createChart,
  createDashboardWithChart,
  runQueryByDb,
  deleteEngine,
} from "./helpers";

/**
 * Per-user Ranger enforcement CONSISTENCY across every read surface (Sub-project B).
 *
 * Two OIDC identities are driven against the SAME governed objects:
 *   - `admin-sso`  — full access → sees all 4 rows + real `item` values.
 *   - `member-sso` — column-mask on `item` + row-filter country='US' → sees only
 *     the 2 US rows, with `item` masked (never the real value).
 * The same admin≠member split must hold IDENTICALLY on SQL Lab, dataset preview,
 * chart data, and dashboard tile.
 *
 * No backend change — this proves enforcement is already consistent across the
 * four surfaces. Opt-in (EGRET_E2E_KIND=1) against a RANGER-ON kind deploy; NOT in
 * `npm test` / GitHub CI. Run via `just e2e-kind-per-user`.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

/** The raw (unmasked) `item` values seeded into egret.gov.sales. */
const REAL_ITEMS = new Set(["book", "pen", "lamp", "desk"]);

type Cols = { columns: Array<{ name: string }>; rows: Array<Array<unknown>> };

/** Extract `{country, item}` per row from a `{columns:[{name}], rows}` payload. */
function cells(resp: Cols): Array<{ country: string; item: string }> {
  const ci = resp.columns.findIndex((c) => c.name === "country");
  const ii = resp.columns.findIndex((c) => c.name === "item");
  return resp.rows.map((r) => ({ country: String(r[ci]), item: String(r[ii]) }));
}

/** admin-sso invariant: all 4 rows, non-US present, real `item` values visible. */
function assertAdmin(rows: Array<{ country: string; item: string }>) {
  expect(rows.length, "admin sees all 4 rows").toBe(4);
  expect(
    rows.some((r) => r.country === "VN") && rows.some((r) => r.country === "SG"),
    "admin sees non-US rows (no row-filter)",
  ).toBeTruthy();
  expect(rows.some((r) => r.item === "book"), "admin sees real `item` values (no mask)").toBeTruthy();
}

/** member-sso invariant: only the 2 US rows, `item` masked (not a real value). */
function assertMember(rows: Array<{ country: string; item: string }>) {
  expect(rows.length, "member sees only 2 US rows (row-filter)").toBe(2);
  expect(rows.every((r) => r.country === "US"), "member sees only US rows").toBeTruthy();
  expect(rows.every((r) => !REAL_ITEMS.has(r.item)), "member's `item` is masked").toBeTruthy();
}

/** Build a Ranger resource map `{key: {values:[...]}}` from `{key:[...]}`. */
const R = (o: Record<string, string[]>): Record<string, { values: string[] }> =>
  Object.fromEntries(Object.entries(o).map(([k, v]) => [k, { values: v }]));

const SELECT_SQL = "SELECT country, item FROM egret.gov.sales ORDER BY country, item";

test.describe("per-user enforcement across surfaces on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against a ranger-on kind deploy");
  test.describe.configure({ mode: "serial" });
  // Full lifecycle: provision jwt Trino (minutes) + Ranger policy propagation + BI setup.
  test.setTimeout(900_000);

  // Break-glass admin session (control-plane setup only — no refresh token, so it
  // cannot drive the data plane).
  let adminPage: Page; // break-glass admin browser page
  let request: APIRequestContext;
  let h: Record<string, string>;

  // The two OIDC identities under test.
  let adminCtx: BrowserContext;
  let memberCtx: BrowserContext;
  let ssoAdminPage: Page;
  let memberPage: Page;
  let adminReq: APIRequestContext;
  let adminH: Record<string, string>;
  let memberReq: APIRequestContext;
  let memberH: Record<string, string>;

  let ws: { id: string; slug: string };
  let engineId: string | undefined;
  const engineName = `e2e-trino-${RUN_ID}`;
  let datasetId: string;
  let chartId: string;
  let dashboardId: string;
  let cardId: string;
  let dbId = ""; // engine's SQL Lab database id (resolved once, as admin)

  test.beforeAll(async ({ browser }) => {
    // 1. Break-glass admin for control-plane setup.
    adminPage = await browser.newPage();
    await loginLocalAdmin(adminPage);
    request = adminPage.request;
    h = await csrf(adminPage.context());

    // 2-4. Storage + warehouse + a jwt-mode Trino engine, wired to the workspace.
    ws = await firstWorkspace(request);
    const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);
    engineId = await provisionTrinoEngineJwt(request, h, RUN_ID, warehouseId);
    await waitForEngineReady(request, engineId);
    await assignEngineToWorkspace(request, h, engineId, ws.id);

    // 5. Ranger service auto-bound to the engine.
    const service = await engineProfileName(request, engineName);

    // 6. Access plumbing granted to Ranger's `{USER}` template (matches the
    //    REQUESTING user), so every identity — incl. the service connection that
    //    dataset-create uses to introspect columns (ConnSpec::from_database has no
    //    per-user bearer) — can run + read. The PER-USER differentiator is the
    //    member-only MASK + row-filter below (the chosen scenario); the enforcement
    //    proof is that member sees masked+US-only while admin sees all, on the same
    //    objects. Both users can access the table; only member is masked+filtered.
    await createPolicy(request, h, {
      kind: "access",
      name: `b-exec-${RUN_ID}`,
      service,
      resources: R({ queryid: ["*"] }),
      items: [{ users: ["{USER}"], accesses: ["execute"], effect: "allow" }],
    });
    await createPolicy(request, h, {
      kind: "access",
      name: `b-imp-${RUN_ID}`,
      service,
      resources: R({ trinouser: ["*"] }),
      items: [{ users: ["{USER}"], accesses: ["impersonate"], effect: "allow" }],
    });
    await createPolicy(request, h, {
      kind: "access",
      name: `b-all-${RUN_ID}`,
      service,
      resources: R({ catalog: ["egret"], schema: ["*"], table: ["*"], column: ["*"] }),
      items: [{ users: ["{USER}"], accesses: ["all"], effect: "allow" }],
    });
    await createPolicy(request, h, {
      kind: "masking",
      name: `b-mem-mask-${RUN_ID}`,
      service,
      resources: R({ catalog: ["egret"], schema: ["gov"], table: ["sales"], column: ["item"] }),
      items: [{ users: ["member-sso"], maskType: "MASK" }],
    });
    await createPolicy(request, h, {
      kind: "rowfilter",
      name: `b-mem-rowf-${RUN_ID}`,
      service,
      resources: R({ catalog: ["egret"], schema: ["gov"], table: ["sales"] }),
      items: [{ users: ["member-sso"], filterExpr: "country = 'US'" }],
    });

    // 7. Two OIDC sessions (JIT-provisions the users + stores refresh tokens).
    adminCtx = await browser.newContext();
    ssoAdminPage = await adminCtx.newPage();
    await loginViaOidcAdminSso(ssoAdminPage);
    adminReq = ssoAdminPage.request;
    adminH = await csrf(adminCtx);

    memberCtx = await browser.newContext();
    memberPage = await memberCtx.newPage();
    await loginViaOidcMember(memberPage);
    memberReq = memberPage.request;
    memberH = await csrf(memberCtx);

    // 8. Add both OIDC users as explicit workspace members (as break-glass admin).
    await addWorkspaceMember(request, h, ws.slug, "admin-sso@egret.local", "ws_admin");
    await addWorkspaceMember(request, h, ws.slug, "member-sso@egret.local", "ws_editor");

    // 9. Resolve the SQL Lab db id for the engine — AS the break-glass admin, since
    //     the databases list is gated on workspace:manage_members. Robust to the
    //     response shape (array or {databases|items:[]}) and retried, since the
    //     engine's SQL Lab database is materialized asynchronously after assign.
    const engineRe = new RegExp(engineName, "i");
    await expect(async () => {
      const resp = await request.get(`/api/ws/${ws.slug}/databases`);
      expect(resp.ok(), `list databases → ${resp.status()}`).toBeTruthy();
      const raw = (await resp.json()) as unknown;
      const list = (Array.isArray(raw)
        ? raw
        : ((raw as { databases?: unknown[]; items?: unknown[] }).databases ??
          (raw as { items?: unknown[] }).items ??
          [])) as Array<{ id: string; name?: string }>;
      expect(list.length, "engine's SQL Lab database exists").toBeGreaterThan(0);
      dbId = (list.find((d) => engineRe.test(d.name ?? "")) ?? list[0]).id;
    }).toPass({ timeout: 60_000, intervals: [3_000] });

    // 10. Seed the governed table AS admin-sso (has full Ranger access). The first
    //     CREATE is wrapped in a retry to absorb the Ranger execute/impersonate poll.
    await expect(async () => {
      await runQueryByDb(adminReq, adminH, ws.slug, dbId, "CREATE SCHEMA IF NOT EXISTS egret.gov");
    }).toPass({ timeout: 90_000, intervals: [5_000] });
    await runQueryByDb(
      adminReq,
      adminH,
      ws.slug,
      dbId,
      "CREATE TABLE IF NOT EXISTS egret.gov.sales (country varchar, item varchar, amount bigint)",
    );
    await runQueryByDb(
      adminReq,
      adminH,
      ws.slug,
      dbId,
      "INSERT INTO egret.gov.sales VALUES ('US','book',10),('US','pen',5),('VN','lamp',7),('SG','desk',9)",
    );

    // 11. Create the BI objects AS admin-sso, all pointing at the same governed table.
    datasetId = await createDataset(adminReq, adminH, ws.slug, {
      name: `b-ds-${RUN_ID}`,
      database_id: dbId,
      source: { kind: "physical", catalog: "egret", schema: "gov", table: "sales" },
    });
    chartId = await createChart(adminReq, adminH, ws.slug, {
      dataset_id: datasetId,
      name: `b-ch-${RUN_ID}`,
      viz_type: "table",
      params: { spec: { columns: ["country", "item", "amount"], row_limit: 1000 } },
    });
    ({ dashboardId, cardId } = await createDashboardWithChart(
      adminReq,
      adminH,
      ws.slug,
      `b-dash-${RUN_ID}`,
      chartId,
    ));
  });

  test.afterAll(async () => {
    if (engineId) await deleteEngine(request, h, engineId);
    await adminCtx?.close();
    await memberCtx?.close();
    await adminPage?.close();
  });

  test("SQL Lab enforces per-user", async () => {
    // Wrap in a retry to absorb the Ranger mask/row-filter poll (~30s).
    await expect(async () => {
      const admin = cells(await runQueryByDb(adminReq, adminH, ws.slug, dbId, SELECT_SQL));
      assertAdmin(admin);
      const member = cells(await runQueryByDb(memberReq, memberH, ws.slug, dbId, SELECT_SQL));
      assertMember(member);
    }).toPass({ timeout: 90_000, intervals: [5_000] });
  });

  test("Dataset preview enforces per-user", async () => {
    await expect(async () => {
      const a = await adminReq.post(`/api/ws/${ws.slug}/datasets/${datasetId}/data`, {
        headers: adminH,
        data: {},
      });
      expect(a.ok(), `admin dataset preview → ${a.status()} ${await a.text()}`).toBeTruthy();
      assertAdmin(cells(await a.json()));

      const m = await memberReq.post(`/api/ws/${ws.slug}/datasets/${datasetId}/data`, {
        headers: memberH,
        data: {},
      });
      expect(m.ok(), `member dataset preview → ${m.status()} ${await m.text()}`).toBeTruthy();
      assertMember(cells(await m.json()));
    }).toPass({ timeout: 90_000, intervals: [5_000] });
  });

  test("Chart data enforces per-user", async () => {
    await expect(async () => {
      const a = await adminReq.post(`/api/ws/${ws.slug}/charts/${chartId}/data`, { headers: adminH });
      expect(a.ok(), `admin chart data → ${a.status()} ${await a.text()}`).toBeTruthy();
      assertAdmin(cells(await a.json()));

      const m = await memberReq.post(`/api/ws/${ws.slug}/charts/${chartId}/data`, { headers: memberH });
      expect(m.ok(), `member chart data → ${m.status()} ${await m.text()}`).toBeTruthy();
      assertMember(cells(await m.json()));
    }).toPass({ timeout: 90_000, intervals: [5_000] });
  });

  test("Dashboard tile enforces per-user", async () => {
    await expect(async () => {
      const a = await adminReq.post(
        `/api/ws/${ws.slug}/dashboards/${dashboardId}/cards/${cardId}/data`,
        { headers: adminH, data: {} },
      );
      expect(a.ok(), `admin dashboard tile → ${a.status()} ${await a.text()}`).toBeTruthy();
      assertAdmin(cells(await a.json()));

      const m = await memberReq.post(
        `/api/ws/${ws.slug}/dashboards/${dashboardId}/cards/${cardId}/data`,
        { headers: memberH, data: {} },
      );
      expect(m.ok(), `member dashboard tile → ${m.status()} ${await m.text()}`).toBeTruthy();
      assertMember(cells(await m.json()));
    }).toPass({ timeout: 90_000, intervals: [5_000] });
  });
});
