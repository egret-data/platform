import { test, expect, type Page, type BrowserContext, type APIRequestContext } from "@playwright/test";
import {
  loginLocalAdmin,
  loginViaOidcAdminSso,
  loginViaOidcMember,
  csrf,
  firstWorkspace,
  provisionStorageAndWarehouse,
  provisionStarRocksEngineJwt,
  provisionTrinoEngineJwt,
  waitForEngineReady,
  assignEngineToWorkspace,
  engineProfileName,
  createPolicy,
  addWorkspaceMember,
  runQueryByDb,
  deleteEngine,
} from "./helpers";

/**
 * Egret machine-IdP / STS token-broker e2e (Task 7) against a RANGER-ON kind deploy.
 *
 * Egret now mints every user's engine JWT itself (a machine-IdP / STS token broker,
 * `POST /api/kernel/engine-token` for the kernel path) instead of exchanging a
 * Keycloak refresh token. The headline proof (test 1): a LOCAL break-glass user
 * (`admin@egret.local`) never authenticates through Keycloak and so holds NO OIDC
 * refresh token — yet can still run a notebook query against a governed StarRocks
 * engine, because the token comes solely from egret's own broker.
 *
 * Opt-in (EGRET_E2E_KIND=1) against a RANGER-ON kind deploy; NOT in `npm test` /
 * GitHub CI. Run via `just e2e-kind-sts`.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

/** Set the Nth mounted Monaco editor's value (typing into Monaco is flaky). Mirrors
 *  notebook-write-isolation-kind.spec.ts / notebook-viz-kind.spec.ts. */
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

/**
 * Enable a notebook DB-connection of the given `engineKind` ("trino" | "starrocks")
 * for the workspace — assigning an engine creates its notebook connection DISABLED
 * by default (mirrors the inline logic in notebook-engine-lakehouse.spec.ts /
 * per-user-enforcement-kind.spec.ts). Returns the connection NAME, which is the
 * `engine=` target notebook cells (`trino()`/`starrocks()`) resolve by.
 */
async function enableNotebookEngine(
  request: APIRequestContext,
  h: Record<string, string>,
  wsSlug: string,
  engineKind: string,
): Promise<string> {
  const settings = await (await request.get(`/api/ws/${wsSlug}/notebook-settings`)).json();
  const conns: Array<{ id: string; name: string; engine: string }> = settings.engines ?? [];
  const conn = conns.find((c) => c.engine === engineKind);
  expect(conn, `a ${engineKind} notebook connection exists after assignment`).toBeTruthy();
  const put = await request.put(`/api/ws/${wsSlug}/notebook-settings`, {
    headers: h,
    data: { engine_ids: [conn!.id] },
  });
  expect(put.ok(), `enable notebook engine → ${put.status()} ${await put.text()}`).toBeTruthy();
  return conn!.name;
}

test.describe("egret-minted engine tokens (STS broker) on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against a ranger-on kind deploy");
  // Full lifecycle: engine provisioning (minutes) + kernel cold-start + Ranger
  // policy propagation. Keep the two tests serial — a single kind node runs hot.
  test.describe.configure({ mode: "serial" });
  test.setTimeout(900_000);

  test("local admin queries StarRocks via egret-minted token", async ({ page, context }) => {
    // Break-glass local admin: NO OIDC login, NO Keycloak refresh token in this
    // session at all. If the notebook query below succeeds, the StarRocks JWT it
    // used could only have come from egret's own token broker.
    await loginLocalAdmin(page);
    const request = page.request; // shares the logged-in session cookie
    const h = await csrf(context);
    let engineId: string | undefined;

    try {
      const ws = await firstWorkspace(request);
      const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);
      // StarRocks default image is 4.1+ (JWT-auth capable) — no image override needed.
      engineId = await provisionStarRocksEngineJwt(request, h, RUN_ID, warehouseId);
      await waitForEngineReady(request, engineId);
      await assignEngineToWorkspace(request, h, engineId, ws.id);

      const engineName = await enableNotebookEngine(request, h, ws.slug, "starrocks");

      // Create the notebook AFTER enabling the engine so the kernel's runtime
      // manifest (loaded at kernel start) includes it → starrocks() resolves it.
      await newNotebook(page, `sts-sr ${RUN_ID}`);
      await setCellSource(
        page,
        0,
        [
          "import daft",
          `df = starrocks("SELECT 1 AS one", engine="${engineName}")`,
          "display(df)",
          "d = df.to_pydict()",
          "print('EGRET_STS_ROWS=' + str(len(d.get('one', []))))",
          "print('EGRET_STS_VALUE=' + str(d.get('one', [None])[0]))",
        ].join("\n"),
      );
      await page.getByRole("button", { name: "Run all" }).click();

      // First run also cold-starts the per-user kernel pod → generous wait. The
      // printed markers are the unambiguous proof the query ran; the "1 row"
      // footer (DataFrameTable.tsx's row-count text) confirms display(df) also
      // rendered — both happening under a break-glass session with no refresh
      // token proves the query authenticated purely off the egret-minted token.
      await expect(page.getByText("EGRET_STS_ROWS=1")).toBeVisible({ timeout: 150_000 });
      await expect(page.getByText("EGRET_STS_VALUE=1")).toBeVisible();
      await expect(page.getByText("1 row", { exact: false })).toBeVisible();
    } finally {
      // Delete with the live `request` (not afterAll, which runs after
      // page.request is torn down) to free node memory.
      if (engineId) await deleteEngine(request, h, engineId);
    }
  });

  test("SSO enforcement parity: admin-sso full vs member-sso masked", async ({ browser }) => {
    // Mirrors per-user-enforcement-kind.spec.ts's SQL Lab surface (the hard
    // enforcement gate there): admin-sso sees all rows unmasked, member-sso sees
    // only US rows with `item` masked. The ONLY thing that changed since that
    // spec was written is WHERE the per-user Trino JWT comes from — egret's own
    // machine-IdP token broker, not a Keycloak refresh-token exchange. This test
    // re-proves the SAME Ranger enforcement invariant holds under that new path.
    const RUN = `${RUN_ID}-p`;
    const REAL_ITEMS = new Set(["book", "pen", "lamp", "desk"]);
    type Cols = { columns: Array<{ name: string }>; rows: Array<Array<unknown>> };
    const cells = (resp: Cols): Array<{ country: string; item: string }> => {
      const ci = resp.columns.findIndex((c) => c.name === "country");
      const ii = resp.columns.findIndex((c) => c.name === "item");
      return resp.rows.map((r) => ({ country: String(r[ci]), item: String(r[ii]) }));
    };
    const assertAdmin = (rows: Array<{ country: string; item: string }>) => {
      expect(rows.length, "admin sees all 4 rows").toBe(4);
      expect(
        rows.some((r) => r.country === "VN") && rows.some((r) => r.country === "SG"),
        "admin sees non-US rows (no row-filter)",
      ).toBeTruthy();
      expect(rows.some((r) => r.item === "book"), "admin sees real `item` values (no mask)").toBeTruthy();
    };
    const assertMember = (rows: Array<{ country: string; item: string }>) => {
      expect(rows.length, "member sees only 2 US rows (row-filter)").toBe(2);
      expect(rows.every((r) => r.country === "US"), "member sees only US rows").toBeTruthy();
      expect(rows.every((r) => !REAL_ITEMS.has(r.item)), "member's `item` is masked").toBeTruthy();
    };
    const R = (o: Record<string, string[]>): Record<string, { values: string[] }> =>
      Object.fromEntries(Object.entries(o).map(([k, v]) => [k, { values: v }]));
    const table = `egret.sts.sales_${RUN.replace(/[^a-zA-Z0-9_]/g, "_")}`;
    const SELECT_SQL = `SELECT country, item FROM ${table} ORDER BY country, item`;

    // Break-glass admin for control-plane setup (engine + policies).
    const adminPage = await browser.newPage();
    await loginLocalAdmin(adminPage);
    const request = adminPage.request;
    const h = await csrf(adminPage.context());

    let engineId: string | undefined;
    let adminCtx: BrowserContext | undefined;
    let memberCtx: BrowserContext | undefined;
    try {
      const ws = await firstWorkspace(request);
      const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN, ws.id);
      engineId = await provisionTrinoEngineJwt(request, h, RUN, warehouseId);
      await waitForEngineReady(request, engineId);
      await assignEngineToWorkspace(request, h, engineId, ws.id);
      const engineName = `e2e-trino-${RUN}`;
      const service = await engineProfileName(request, engineName);

      // Access plumbing granted to Ranger's `{USER}` template (matches the
      // REQUESTING user) — same shape as per-user-enforcement-kind.spec.ts. The
      // per-user differentiator is the member-only mask + row-filter below.
      await createPolicy(request, h, {
        kind: "access",
        name: `sts-exec-${RUN}`,
        service,
        resources: R({ queryid: ["*"] }),
        items: [{ users: ["{USER}"], accesses: ["execute"], effect: "allow" }],
      });
      await createPolicy(request, h, {
        kind: "access",
        name: `sts-imp-${RUN}`,
        service,
        resources: R({ trinouser: ["*"] }),
        items: [{ users: ["{USER}"], accesses: ["impersonate"], effect: "allow" }],
      });
      await createPolicy(request, h, {
        kind: "access",
        name: `sts-all-${RUN}`,
        service,
        resources: R({ catalog: ["egret"], schema: ["*"], table: ["*"], column: ["*"] }),
        items: [{ users: ["{USER}"], accesses: ["all"], effect: "allow" }],
      });
      await createPolicy(request, h, {
        kind: "masking",
        name: `sts-mem-mask-${RUN}`,
        service,
        resources: R({ catalog: ["egret"], schema: ["sts"], table: [`sales_${RUN.replace(/[^a-zA-Z0-9_]/g, "_")}`], column: ["item"] }),
        items: [{ users: ["member-sso"], maskType: "MASK" }],
      });
      await createPolicy(request, h, {
        kind: "rowfilter",
        name: `sts-mem-rowf-${RUN}`,
        service,
        resources: R({ catalog: ["egret"], schema: ["sts"], table: [`sales_${RUN.replace(/[^a-zA-Z0-9_]/g, "_")}`] }),
        items: [{ users: ["member-sso"], filterExpr: "country = 'US'" }],
      });

      // Two OIDC identities — the ones under test.
      adminCtx = await browser.newContext();
      const ssoAdminPage = await adminCtx.newPage();
      await loginViaOidcAdminSso(ssoAdminPage);
      const adminReq = ssoAdminPage.request;
      const adminH = await csrf(adminCtx);

      memberCtx = await browser.newContext();
      const memberPage = await memberCtx.newPage();
      await loginViaOidcMember(memberPage);
      const memberReq = memberPage.request;
      const memberH = await csrf(memberCtx);

      await addWorkspaceMember(request, h, ws.slug, "admin-sso@egret.local", "ws_admin");
      await addWorkspaceMember(request, h, ws.slug, "member-sso@egret.local", "ws_editor");

      // Resolve the SQL Lab db id for the engine AS the break-glass admin (the
      // databases list is manage_members-gated), robust to response shape and
      // retried (the engine's SQL Lab database materializes async post-assign).
      const engineRe = new RegExp(engineName, "i");
      let dbId = "";
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

      // Seed the governed table AS admin-sso (full Ranger access). The first
      // CREATE is retried to absorb the Ranger execute/impersonate poll.
      await expect(async () => {
        await runQueryByDb(adminReq, adminH, ws.slug, dbId, "CREATE SCHEMA IF NOT EXISTS egret.sts");
      }).toPass({ timeout: 90_000, intervals: [5_000] });
      await runQueryByDb(
        adminReq,
        adminH,
        ws.slug,
        dbId,
        `CREATE TABLE IF NOT EXISTS ${table} (country varchar, item varchar, amount bigint)`,
      );
      await runQueryByDb(
        adminReq,
        adminH,
        ws.slug,
        dbId,
        `INSERT INTO ${table} VALUES ('US','book',10),('US','pen',5),('VN','lamp',7),('SG','desk',9)`,
      );

      // The core enforcement assertion, retried while the mask/row-filter poll
      // (~30s) propagates. Both identities' Trino JWT is egret-minted.
      await expect(async () => {
        const admin = cells(await runQueryByDb(adminReq, adminH, ws.slug, dbId, SELECT_SQL));
        assertAdmin(admin);
        const member = cells(await runQueryByDb(memberReq, memberH, ws.slug, dbId, SELECT_SQL));
        assertMember(member);
      }).toPass({ timeout: 90_000, intervals: [5_000] });
    } finally {
      if (engineId) await deleteEngine(request, h, engineId);
      await adminCtx?.close();
      await memberCtx?.close();
      await adminPage.close();
    }
  });
});

test.describe("OIDC-off deployment still queries via egret token", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against a ranger-on kind deploy");

  test("local admin queries an engine on an OIDC-disabled deploy", async () => {
    // NOT run: this spec's other two tests target the OIDC-on governance stack
    // (`just deploy-kind-governance` → `--set oidc.enabled=true --set
    // ranger.enabled=true`, base URL http://egret.localtest.me). Proving the
    // headline claim on an OIDC-DISABLED cluster needs a genuinely different
    // deploy variant (`just deploy-kind` — OIDC left off entirely) and this
    // harness runs exactly one deploy per `just e2e-kind-*` recipe / playwright
    // invocation, so it can't stand up both stacks in the same run without a
    // second recipe + a second base URL wired through env — out of scope here.
    //
    // Manually verified instead: `just deploy-kind` (OIDC left off) →
    // loginLocalAdmin → provision + assign a StarRocks engine → run test 1's
    // `starrocks("SELECT 1 AS one", ...)` notebook cell against it. It succeeds
    // with zero OIDC/Keycloak wiring anywhere in the cluster, because the
    // kernel's engine JWT comes solely from egret's own `/api/kernel/engine-token`
    // broker — never from a Keycloak grant of any kind.
    test.skip(true, "needs a separate OIDC-disabled kind deploy variant; see comment above");
  });
});
