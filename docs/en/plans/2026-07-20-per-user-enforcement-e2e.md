# Per-user enforcement e2e across surfaces (Sub-project B) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A pure-e2e regression test proving per-user Ranger enforcement is consistent across SQL Lab → dataset → chart → dashboard → notebook: two OIDC identities driven against the SAME objects — `admin-sso` (full) sees all rows + real values, `member-sso` (masked + row-filtered) sees only US rows with the sensitive column masked — identically on every surface.

**Architecture:** No backend change (enforcement already consistent). One opt-in kind spec + a few new e2e helpers. Setup (provision jwt Trino, Ranger policies, add both users as workspace members, seed a governed table, create dataset/chart/dashboard) runs as the break-glass admin + `admin-sso`; then two `browser.newContext()` sessions drive the two identities and assert.

**Tech Stack:** Playwright (TypeScript), kind, Ranger-on deploy.

**Design doc:** `docs/en/specs/2026-07-20-per-user-enforcement-e2e-design.md`.

**Branch:** `feat/per-user-enforcement-e2e` (created off main; the design doc is committed here).

**Conventions:** FE cmds from `egret/frontend`. Commit per task. The spec can't run without a cluster — gate local verification on `npm run typecheck` + `eslint`. Live run is Task 4.

**Identities (verified):** `admin-sso@egret.local`/`Admin12345!` and `member-sso@egret.local`/`Member12345!` — both have `offline_access` (refresh token → per-user JWT mint works on all surfaces incl notebook). Their Trino usernames (from `ensure_username` = slugified email local-part) are **`admin-sso`** and **`member-sso`**. Break-glass `admin@egret.local` is NOT usable (no refresh token → 401 on notebook); it is used ONLY for control-plane setup.

---

## Task 1: e2e helpers (login-admin-sso, jwt provisioner, BI-create, add-member)

**Files:** `frontend/tests/e2e/helpers.ts` (append). Verify: `npm run typecheck`.

- [ ] **Step 1: `loginViaOidcAdminSso`** — mirror `loginViaOidc` (helpers.ts:14) but for the admin-sso user (env-overridable):

```ts
const ADMIN_SSO_USER = process.env.EGRET_E2E_OIDC_ADMIN_USER ?? "admin-sso@egret.local";
const ADMIN_SSO_PASSWORD = process.env.EGRET_E2E_OIDC_ADMIN_PASSWORD ?? "Admin12345!";

/** Sign in via SSO as the platform-admin OIDC user (has offline_access). */
export async function loginViaOidcAdminSso(page: Page): Promise<void> {
  await page.goto("/login");
  await page.getByRole("link", { name: /sign in with sso/i }).click();
  await page.locator("#username, input[name='username']").first().fill(ADMIN_SSO_USER);
  await page.locator("#password, input[name='password']").first().fill(ADMIN_SSO_PASSWORD);
  await page.locator("#kc-login, button[type='submit'], input[type='submit']").first().click();
  // A member-only admin-sso may land outside a workspace; just wait to leave Keycloak.
  await page.waitForURL((u) => !u.pathname.includes("/realms/"), { timeout: 30_000 });
}
```

- [ ] **Step 2: `provisionTrinoEngineJwt`** — like `provisionTrinoEngine` (helpers.ts:105) but sends `trinoConfig.auth.jwt` at creation (jwt-mode; avoids the extra PUT):

```ts
export async function provisionTrinoEngineJwt(
  req: APIRequestContext, h: Record<string, string>, runId: string, warehouseId: string,
): Promise<string> {
  const engine = await postJson(req, "/api/control-plane/engines", h, {
    name: `e2e-trino-${runId}`, warehouseId, kind: "trino",
    coordinatorReplicas: 1, workerReplicas: 1, cpuMin: "250m", cpuMax: "1", memory: "1Gi",
    trinoConfig: { auth: { jwt: {} } },
  });
  return engine.id as string;
}
```

- [ ] **Step 3: `addWorkspaceMember`** — `POST /api/ws/{workspace}/members` (principal_id accepts an email):

```ts
export async function addWorkspaceMember(
  req: APIRequestContext, h: Record<string, string>, wsSlug: string, email: string, role: string,
): Promise<void> {
  const res = await req.post(`/api/ws/${wsSlug}/members`, {
    headers: h, data: { principal_type: "user", principal_id: email, role },
  });
  expect(res.ok(), `add member ${email} → ${res.status()} ${await res.text()}`).toBeTruthy();
}
```

- [ ] **Step 4: BI-object create helpers** (no existing helpers — build on `postJson`):

```ts
/** First database id for the ws (the SQL Lab db backing an engine). */
export async function firstDatabaseId(req: APIRequestContext, wsSlug: string): Promise<string> {
  const dbs = await (await req.get(`/api/ws/${wsSlug}/databases`)).json();
  const list = Array.isArray(dbs) ? dbs : (dbs.databases ?? dbs.items ?? []);
  expect(list.length, "at least one database").toBeGreaterThan(0);
  return list[0].id as string;
}

export async function createDataset(
  req: APIRequestContext, h: Record<string, string>, wsSlug: string,
  body: { name: string; database_id: string; source: Record<string, unknown> },
): Promise<string> {
  return (await postJson(req, `/api/ws/${wsSlug}/datasets`, h, body)).id as string;
}

export async function createChart(
  req: APIRequestContext, h: Record<string, string>, wsSlug: string,
  body: { dataset_id: string; name: string; viz_type: string; params: Record<string, unknown> },
): Promise<string> {
  return (await postJson(req, `/api/ws/${wsSlug}/charts`, h, body)).id as string;
}

export async function createDashboardWithChart(
  req: APIRequestContext, h: Record<string, string>, wsSlug: string, name: string, chartId: string,
): Promise<{ dashboardId: string; cardId: string }> {
  const dash = await postJson(req, `/api/ws/${wsSlug}/dashboards`, h, { name, description: "" });
  const upd = await req.put(`/api/ws/${wsSlug}/dashboards/${dash.id}`, {
    headers: h,
    data: {
      name, description: "", parameters: [], width: "full", tabs: [],
      cards: [{ chart_id: chartId, card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {} }],
    },
  });
  expect(upd.ok(), `set dashboard card → ${upd.status()} ${await upd.text()}`).toBeTruthy();
  const body = await upd.json();
  // cardId: read it back from the updated dashboard's cards (shape: cards[].id).
  const cardId = (body.cards?.[0]?.id ?? body.dashboard?.cards?.[0]?.id) as string;
  expect(cardId, "dashboard card id").toBeTruthy();
  return { dashboardId: dash.id as string, cardId };
}
```
NOTE: confirm the card-id field name by reading the `db_update` response DTO (`crates/egret-analytics-api/src/bi.rs`); adjust `body.cards?.[0]?.id` to the real path. If the PUT response doesn't include card ids, GET `/api/ws/{ws}/dashboards/{id}` and read `cards[0].id`.

- [ ] **Step 5:** `npm run typecheck` clean. Commit:
```bash
git add frontend/tests/e2e/helpers.ts
git commit -m "test(e2e): helpers for admin-sso login, jwt engine, BI-object + member creation"
```

---

## Task 2: the per-user-enforcement spec

**Files:** Create `frontend/tests/e2e/per-user-enforcement-kind.spec.ts`. Verify: `npm run typecheck`, `eslint`.

- [ ] **Step 1: Setup (`beforeAll`).** Opt-in `EGRET_E2E_KIND=1`, `test.describe.configure({ mode: "serial" })`, `test.setTimeout(900_000)`.

  1. Break-glass admin page → `loginLocalAdmin` → `request`=`page.request`, `h`=`csrf(context)`.
  2. `const ws = await firstWorkspace(request);` (slug `demo`).
  3. `provisionStorageAndWarehouse` → `provisionTrinoEngineJwt` → `waitForEngineReady` → `assignEngineToWorkspace(request,h,engineId,ws.id)`.
  4. `const service = await engineProfileName(request, \`e2e-trino-${RUN_ID}\`);`
  5. Ranger policies via `createPolicy(request, h, {...})` with `R = (o) => Object.fromEntries(Object.entries(o).map(([k,v]) => [k, { values: v }]))`:
     - shared execute: `{ kind:"access", name:\`b-exec-${RUN_ID}\`, service, resources: R({queryid:["*"]}), items:[{ users:["admin-sso","member-sso"], accesses:["execute"], effect:"allow" }] }`
     - shared impersonate: `{ kind:"access", name:\`b-imp-${RUN_ID}\`, service, resources: R({trinouser:["*"]}), items:[{ users:["admin-sso","member-sso"], accesses:["impersonate"], effect:"allow" }] }`
     - admin full: `{ kind:"access", name:\`b-admin-all-${RUN_ID}\`, service, resources: R({catalog:["egret"], schema:["*"], table:["*"], column:["*"]}), items:[{ users:["admin-sso"], accesses:["all"], effect:"allow" }] }`
     - member select: `{ kind:"access", name:\`b-mem-sel-${RUN_ID}\`, service, resources: R({catalog:["egret"], schema:["gov"], table:["sales"], column:["*"]}), items:[{ users:["member-sso"], accesses:["select"], effect:"allow" }] }`
     - member mask: `{ kind:"masking", name:\`b-mem-mask-${RUN_ID}\`, service, resources: R({catalog:["egret"], schema:["gov"], table:["sales"], column:["item"]}), items:[{ users:["member-sso"], maskType:"MASK" }] }`
     - member rowfilter: `{ kind:"rowfilter", name:\`b-mem-rowf-${RUN_ID}\`, service, resources: R({catalog:["egret"], schema:["gov"], table:["sales"]}), items:[{ users:["member-sso"], filterExpr:"country = 'US'" }] }`
  6. Two OIDC contexts (JIT-provisions the users + stores refresh tokens):
     ```ts
     const adminCtx = await browser.newContext(); const adminPage = await adminCtx.newPage();
     const memberCtx = await browser.newContext(); const memberPage = await memberCtx.newPage();
     await loginViaOidcAdminSso(adminPage);
     await loginViaOidcMember(memberPage);
     const adminReq = adminPage.request, adminH = await csrf(adminCtx);
     const memberReq = memberPage.request, memberH = await csrf(memberCtx);
     ```
  7. Add both as workspace members (as break-glass admin): `addWorkspaceMember(request,h,ws.slug,"admin-sso@egret.local","ws_admin")` and `..."member-sso@egret.local","ws_editor"`.
  8. Notebook-enable the engine for the ws: `GET /api/ws/${ws.slug}/notebook-settings` → find the trino connection id → `PUT` with `{engine_ids:[connId]}` (as break-glass admin). (Mirror the A live-debug findings.)
  9. Seed the governed table AS admin-sso (has full access): via `runQuery(adminReq, adminH, ws.slug, engineName, sql)`:
     - `CREATE SCHEMA IF NOT EXISTS egret.gov`
     - `CREATE TABLE IF NOT EXISTS egret.gov.sales (country varchar, item varchar, amount bigint)`
     - `INSERT INTO egret.gov.sales VALUES ('US','book',10),('US','pen',5),('VN','lamp',7),('SG','desk',9)`
  10. Create BI objects AS admin-sso: `dbId = firstDatabaseId(adminReq, ws.slug)`; `datasetId = createDataset(adminReq, adminH, ws.slug, { name:\`b-ds-${RUN_ID}\`, database_id: dbId, source:{ kind:"physical", catalog:"egret", schema:"gov", table:"sales" } })`; `chartId = createChart(adminReq, adminH, ws.slug, { dataset_id: datasetId, name:\`b-ch-${RUN_ID}\`, viz_type:"table", params:{ spec:{ columns:["country","item","amount"], row_limit:1000 } } })`; `{ dashboardId, cardId } = createDashboardWithChart(adminReq, adminH, ws.slug, \`b-dash-${RUN_ID}\`, chartId)`.

  Stash `engineId`, `engineName`, `ws`, ids, and both `{req,h,page}` on describe-scoped vars.

- [ ] **Step 2: helper `cells(resp, colNames)`** — extract a map of `{country, item}` per row from either `ChartRunResponse` (`{columns:[{name}],rows}`) or `runQuery`'s shape (same). E.g. `const ci = resp.columns.findIndex(c=>c.name==="country"); const ii = resp.columns.findIndex(c=>c.name==="item"); return resp.rows.map(r=>({country:String(r[ci]), item:String(r[ii])}));`

- [ ] **Step 3: assertion helper `assertGoverned(rowsAdmin, rowsMember)`** — the shared invariant:
  - admin: 4 rows, includes `VN`/`SG`, `item` contains real values (`book`,`lamp` present).
  - member: only `country==="US"` rows (2), and every `item` is masked (NOT equal to any real value `book`/`pen`/`lamp`/`desk`).
  ```ts
  function assertMember(rows: {country:string; item:string}[]) {
    expect(rows.every(r => r.country === "US"), "member sees only US rows").toBeTruthy();
    expect(rows.length, "member sees 2 US rows").toBe(2);
    const real = new Set(["book","pen","lamp","desk"]);
    expect(rows.every(r => !real.has(r.item)), "member's item is masked").toBeTruthy();
  }
  function assertAdmin(rows: {country:string; item:string}[]) {
    expect(rows.length, "admin sees all 4 rows").toBe(4);
    expect(rows.some(r => r.country === "VN") && rows.some(r => r.country === "SG"), "admin sees non-US").toBeTruthy();
    expect(rows.some(r => r.item === "book"), "admin sees real item values").toBeTruthy();
  }
  ```

- [ ] **Step 4: one `test(...)` per surface**, each running the query as BOTH identities and applying `assertAdmin`/`assertMember`. Use `.toPass({ timeout: 90_000, intervals: [5_000] })` around the first query of each surface to absorb the Ranger ~30s policy poll (mirror governance-kind).
  - **SQL Lab:** `runQuery(adminReq, adminH, ws.slug, engineName, "SELECT country, item FROM egret.gov.sales ORDER BY country, item")` vs same with `memberReq/memberH`.
  - **Dataset preview:** `adminReq.post(\`/api/ws/${ws.slug}/datasets/${datasetId}/data\`, {headers:adminH, data:{}})` → json; vs member. (Returns `ChartRunResponse`; `SELECT * FROM source` — extract country/item columns.)
  - **Chart data:** `adminReq.post(\`/api/ws/${ws.slug}/charts/${chartId}/data\`, {headers:adminH})` (no body); vs member.
  - **Dashboard tile:** `adminReq.post(\`/api/ws/${ws.slug}/dashboards/${dashboardId}/cards/${cardId}/data\`, {headers:adminH, data:{}})`; vs member.
  - **Notebook (best-effort):** in a fresh notebook (drive Monaco like `notebook-ecommerce.spec.ts`) as each user, run `df = trino("SELECT country, item FROM egret.gov.sales", engine="<engineName>")` then `print("MEMBER_ROWS", df.count_rows())` / dump `df.to_pydict()`; assert member sees 2 US rows + masked item, admin sees 4. Wrap in `test()` that TOLERATES a kernel-token/enablement failure by `test.fixme`/skip-with-log if the kernel doesn't cooperate (note it clearly — do NOT let a flaky kernel fail the whole suite; the 4 API surfaces are the hard assertions).

- [ ] **Step 5: `afterAll` cleanup:** `deleteEngine(request, h, engineId)`; close the two contexts. (BI objects tied to the ws are harmless leftovers; optionally delete them.)

- [ ] **Step 6:** `npm run typecheck` + `eslint tests/e2e/per-user-enforcement-kind.spec.ts` clean. Commit:
```bash
git add frontend/tests/e2e/per-user-enforcement-kind.spec.ts
git commit -m "test(security): e2e — per-user enforcement across SQL Lab/dataset/chart/dashboard/notebook (admin vs member)"
```

---

## Task 3: just recipe + local verification

**Files:** `justfile`.

- [ ] **Step 1:** Add `e2e-kind-per-user` mirroring `e2e-kind-governance` (Ranger-on deploy) but running this spec:
```makefile
e2e-kind-per-user: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test per-user-enforcement-kind
```
- [ ] **Step 2:** From `frontend/`: `npm run typecheck` + `npx eslint tests/e2e/per-user-enforcement-kind.spec.ts tests/e2e/helpers.ts` clean.
- [ ] **Step 3:** Commit:
```bash
git add justfile
git commit -m "test(security): just e2e-kind-per-user recipe"
```

---

## Task 4: live run on the Ranger-on kind deploy

- [ ] **Step 1:** Ensure the two SSO users are logging in cleanly; ensure node capacity for one Trino engine (free any hogging engine first).
- [ ] **Step 2:** `just e2e-kind-per-user` (builds + deploys ranger-on). **Then `kubectl rollout restart deploy/egret -n egret-system` + `rollout status`** (the recipe does NOT restart the same-`:dev`-tag backend — the A lesson), and re-run just the spec: `cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me EGRET_E2E_BUCKET=e2e EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 npx playwright test per-user-enforcement-kind`.
- [ ] **Step 3:** Confirm the 4 API surfaces pass (admin full vs member masked+US-only, identical across surfaces); notebook passes or is cleanly skipped-with-reason. Debug live as needed (Ranger poll timing, member ws-role/perms, refresh-token presence).

---

## Self-Review Notes
- No backend change — B proves existing behavior. The whole deliverable is the spec + helpers.
- Both OIDC users are added as explicit ws members (admin-sso `ws_admin`, member-sso `ws_editor`) so the test does NOT depend on `seed-sso-test-rbac.sql` group→role mapping; both have `offline_access` so per-user JWT minting works on all surfaces incl notebook.
- admin-sso is granted its full Ranger policy EXPLICITLY (A's auto-seed misses group-granted admins) — documented; candidate A follow-up.
- Masking asserted structurally (member's `item` ∉ real values) not by exact mask string; row-filter asserted by US-only + count.
- Notebook is best-effort (kernel-token lifecycle); the 4 API surfaces are the hard gate.
- Live-run reuses the A rollout-restart lesson (same-`:dev`-tag pod won't pick up config/image without a restart).
- Out of scope: any enforcement code change; extending A to group-granted admins; sub-project C.
