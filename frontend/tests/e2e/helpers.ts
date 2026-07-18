import {
  type Page,
  type BrowserContext,
  type APIRequestContext,
  expect,
} from "@playwright/test";

const OIDC_USER = process.env.EGRET_E2E_OIDC_USER ?? "tester@egret.local";
const OIDC_PASSWORD = process.env.EGRET_E2E_OIDC_PASSWORD ?? "pw";
const ADMIN_EMAIL = process.env.EGRET_E2E_EMAIL ?? "admin@egret.local";
const ADMIN_PASSWORD = process.env.EGRET_E2E_PASSWORD ?? "Admin12345!";

/** Sign in through the real Keycloak login page (SSO). Lands inside a workspace. */
export async function loginViaOidc(page: Page): Promise<void> {
  await page.goto("/login");
  await page.getByRole("link", { name: /sign in with sso/i }).click();
  // Now on the Keycloak-hosted login form (issuer origin).
  await page.locator("#username, input[name='username']").first().fill(OIDC_USER);
  await page.locator("#password, input[name='password']").first().fill(OIDC_PASSWORD);
  await page.locator("#kc-login, button[type='submit'], input[type='submit']").first().click();
  // Redirects back through the callback into a workspace.
  await page.waitForURL(/\/explore\/[^/]+\//, { timeout: 30_000 });
}

/** Break-glass: expand the local form and sign in as the platform admin. */
export async function loginLocalAdmin(page: Page): Promise<void> {
  await page.goto("/login");
  // Wait for the auth-config to resolve (it flips OIDC on and hides the local
  // form behind the break-glass toggle). Reveal the form if the toggle is shown.
  const adminToggle = page.getByRole("button", { name: "Admin sign-in (break-glass)" });
  await adminToggle.or(page.getByPlaceholder("Email")).first().waitFor({ timeout: 15_000 });
  if (await adminToggle.isVisible().catch(() => false)) {
    await adminToggle.click();
  }
  await page.getByPlaceholder("Email").waitFor({ timeout: 10_000 });
  await page.getByPlaceholder("Email").fill(ADMIN_EMAIL);
  await page.getByPlaceholder("Password").fill(ADMIN_PASSWORD);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await page.waitForURL(/\/explore\/[^/]+\//, { timeout: 20_000 });
}

/** CSRF double-submit header from the session cookie, for control-plane mutations. */
export async function csrf(context: BrowserContext): Promise<Record<string, string>> {
  const cookies = await context.cookies();
  const token = cookies.find((c) => c.name === "egret_csrf")?.value ?? "";
  return { "X-Egret-CSRF": token, "Content-Type": "application/json" };
}

/** POST that asserts a 2xx and returns the parsed JSON body. */
export async function postJson(
  req: APIRequestContext,
  url: string,
  headers: Record<string, string>,
  body: unknown,
): Promise<any> {
  const res = await req.post(url, { headers, data: body });
  expect(res.ok(), `${url} → ${res.status()} ${await res.text()}`).toBeTruthy();
  return res.json();
}

const S3_ENDPOINT = process.env.EGRET_E2E_S3_ENDPOINT ?? "http://minio:9000";
const S3_KEY = process.env.EGRET_E2E_S3_KEY ?? "egret";
const S3_SECRET = process.env.EGRET_E2E_S3_SECRET ?? "egretsecret";
const S3_BUCKET = process.env.EGRET_E2E_BUCKET ?? "warehouse";

/** The first workspace visible to the logged-in user (egret init seeds `default`). */
export async function firstWorkspace(
  req: APIRequestContext,
): Promise<{ id: string; slug: string }> {
  const res = await req.get("/api/me/workspaces");
  expect(res.ok(), `GET /api/me/workspaces → ${res.status()}`).toBeTruthy();
  const list = await res.json();
  expect(list.length, "at least one workspace (egret init seeds 'default')").toBeGreaterThan(0);
  return { id: list[0].id, slug: list[0].slug ?? list[0].id };
}

/** Provision an in-cluster MinIO storage connection + an Iceberg warehouse on it. */
export async function provisionStorageAndWarehouse(
  req: APIRequestContext,
  h: Record<string, string>,
  runId: string,
  workspaceId: string,
): Promise<{ storageId: string; warehouseId: string; warehouseName: string }> {
  const storage = await postJson(req, "/api/control-plane/storage", h, {
    name: `e2e-store-${runId}`,
    provider: "s3",
    region: "us-east-1",
    bucket: S3_BUCKET,
    endpoint: S3_ENDPOINT,
    key_prefix: `e2e-${runId}`,
    access_key: S3_KEY,
    path_style: true,
    new_secret: { name: `e2e-sec-${runId}`, value: S3_SECRET },
  });
  const warehouse = await postJson(req, "/api/control-plane/warehouses", h, {
    name: `e2e-wh-${runId}`,
    workspaceId,
    storageConnectionId: storage.id,
  });
  expect(warehouse.lakekeeperWarehouseId, "warehouse provisioned in lakekeeper").toBeTruthy();
  return { storageId: storage.id, warehouseId: warehouse.id, warehouseName: warehouse.name };
}

/** Provision a small (1Gi, 1 worker) Trino engine wired to the warehouse. Returns its id. */
export async function provisionTrinoEngine(
  req: APIRequestContext,
  h: Record<string, string>,
  runId: string,
  warehouseId: string,
): Promise<string> {
  const engine = await postJson(req, "/api/control-plane/engines", h, {
    name: `e2e-trino-${runId}`,
    warehouseId,
    kind: "trino",
    coordinatorReplicas: 1,
    workerReplicas: 1,
    cpuMin: "250m",
    cpuMax: "1",
    memory: "1Gi",
  });
  return engine.id as string;
}

/** Poll GET /engines/{id} until status.phase === "running" (all pods ready). */
export async function waitForEngineReady(
  req: APIRequestContext,
  id: string,
  timeoutMs = 240_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let last = "";
  // A rolling update (e.g. enabling JWT) briefly reports "failed": the orchestrator
  // marks the engine failed if ANY pod has phase "Failed", and the OLD coordinator
  // pod can exit non-zero (SIGTERM/OOM) while the NEW one is already Running. That
  // corpse is garbage-collected when the old ReplicaSet scales to 0, after which the
  // phase returns to "running". So only treat a PERSISTENT failure as fatal.
  let failedSince = 0;
  // Require several CONSECUTIVE "running" reads. During a rolling update the phase
  // can flip running→provisioning→running; a single "running" can also be the OLD
  // coordinator in the brief window before the new pod appears. Waiting for a stable
  // streak avoids returning mid-roll (which would race the query against a coordinator
  // that is still starting).
  let runningStreak = 0;
  while (Date.now() < deadline) {
    const res = await req.get(`/api/control-plane/engines/${id}`);
    if (res.ok()) {
      const body = await res.json();
      last = body.status?.phase ?? "";
      if (last === "running") {
        if (++runningStreak >= 3) return;
      } else {
        runningStreak = 0;
        if (last === "failed") {
          if (failedSince === 0) failedSince = Date.now();
          else if (Date.now() - failedSince > 120_000) {
            throw new Error(`engine ${id} stuck 'failed' >120s: ${JSON.stringify(body.status)}`);
          }
        } else {
          failedSince = 0;
        }
      }
    }
    await new Promise((r) => setTimeout(r, 5_000));
  }
  throw new Error(`engine ${id} not ready in ${timeoutMs}ms (last phase: ${last})`);
}

/** Enable egret-JWT auth on a Trino engine (full-replace PUT, empty jwksUri → CP default). */
export async function enableEngineJwt(
  req: APIRequestContext,
  h: Record<string, string>,
  id: string,
): Promise<void> {
  const cur = await (await req.get(`/api/control-plane/engines/${id}`)).json();
  const s = cur.spec ?? {};
  const tc = s.trinoConfig ?? {};
  const trinoConfig = { ...tc, auth: { ...(tc.auth ?? {}), jwt: {} } };
  const res = await req.put(`/api/control-plane/engines/${id}`, {
    headers: h,
    data: {
      name: cur.name,
      warehouseId: cur.warehouseId,
      kind: "trino",
      coordinatorReplicas: s.coordinatorReplicas,
      workerReplicas: s.workerReplicas,
      cpuMin: s.cpuMin,
      cpuMax: s.cpuMax,
      memory: s.memory,
      trinoConfig,
    },
  });
  expect(res.ok(), `enable JWT → ${res.status()} ${await res.text()}`).toBeTruthy();
}

/** Assign an engine to a workspace (control-plane full-set PUT). */
export async function assignEngineToWorkspace(
  req: APIRequestContext,
  h: Record<string, string>,
  engineId: string,
  workspaceId: string,
): Promise<void> {
  const res = await req.put(`/api/control-plane/engines/${engineId}/workspaces`, {
    headers: h,
    data: { workspaceIds: [workspaceId] },
  });
  expect(res.ok(), `assign engine → ${res.status()} ${await res.text()}`).toBeTruthy();
}

/** Best-effort delete (cleanup); swallows errors if already gone. */
export async function deleteEngine(
  req: APIRequestContext,
  h: Record<string, string>,
  id: string,
): Promise<void> {
  await req.delete(`/api/control-plane/engines/${id}`, { headers: h }).catch(() => {});
}

/**
 * Prove the data-plane money-path in SQL Lab: confirm the engine is wired into the
 * SQL Lab UI (it lists + auto-selects in the Database picker), then run `sql`
 * through the SAME backend the UI calls (`POST /api/ws/{ws}/queries`, via
 * `page.request` so it shares the logged-in session) and return the result JSON.
 *
 * The query goes over the real egret → Trino delegation path (JWT + TLS:8443 +
 * X-Trino-User impersonation) — identical to what the SQL Lab "Run" button submits.
 * We drive the submit via the API rather than the Monaco editor + results grid
 * because that DOM chrome is flaky to script; the money-path assertion is the same.
 */
export async function runSqlInLab(
  page: Page,
  workspaceSlug: string,
  engineName: string,
  sql: string,
): Promise<string> {
  const engineRe = new RegExp(engineName, "i");
  // The Database picker (Data panel) auto-selects the only assigned engine, so its
  // trigger reads the engine name — NOT the top-nav workspace switcher (also a
  // combobox). `useDatabases` fetches on mount and a freshly assigned engine appears
  // only once it is queryable, so reload SQL Lab until it lists + selects the engine.
  const dbSelected = page.getByRole("combobox").filter({ hasText: engineRe }).first();
  await expect(async () => {
    await page.goto(`/explore/${workspaceSlug}/sql-lab`);
    await expect(dbSelected).toBeVisible({ timeout: 15_000 });
  }).toPass({ timeout: 240_000, intervals: [0] });

  // Resolve the engine's database id from the SQL Lab databases list.
  const dbs = (await (await page.request.get(`/api/ws/${workspaceSlug}/databases`)).json()) as Array<{
    id: string;
    name?: string;
  }>;
  const db = dbs.find((d) => engineRe.test(d.name ?? ""));
  expect(db, `database for ${engineName} is listed in SQL Lab`).toBeTruthy();

  // Submit + poll, retrying while a freshly JWT-re-provisioned coordinator is still
  // warming up (it 500s for ~30-40s until SERVER STARTED, then serves).
  const h = await csrf(page.context());
  let out = "";
  await expect(async () => {
    const sub = await page.request.post(`/api/ws/${workspaceSlug}/queries`, {
      headers: h,
      data: { database_id: db!.id, sql },
    });
    expect(sub.ok(), `submit → ${sub.status()} ${await sub.text()}`).toBeTruthy();
    const qid = (await sub.json()).id as string;
    let state = "";
    let error = "";
    for (let i = 0; i < 40; i++) {
      const s = await (await page.request.get(`/api/ws/${workspaceSlug}/queries/${qid}`)).json();
      state = s.state ?? s.status ?? "";
      error = s.error ?? "";
      if (["finished", "succeeded", "failed", "error"].includes(state)) break;
      await new Promise((r) => setTimeout(r, 1_000));
    }
    expect(state, `query state (err: ${error})`).toMatch(/finished|succeeded/);
    out = (
      await (await page.request.get(`/api/ws/${workspaceSlug}/queries/${qid}/results`)).text()
    ).trim();
  }).toPass({ timeout: 240_000, intervals: [4_000] });
  return out;
}

const OIDC_MEMBER_USER = process.env.EGRET_E2E_OIDC_MEMBER_USER ?? "member-sso@egret.local";
const OIDC_MEMBER_PASSWORD = process.env.EGRET_E2E_OIDC_MEMBER_PASSWORD ?? "Member12345!";

/**
 * Sign in through the real Keycloak as a plain member. Unlike `loginViaOidc`, this
 * does NOT wait for `/explore/{ws}/` — a member may have no workspace; it only waits
 * for the OIDC callback to land back in the app (off the Keycloak `/realms/` pages),
 * which is enough to establish the JIT-provisioned session.
 */
export async function loginViaOidcMember(page: Page): Promise<void> {
  await page.goto("/login");
  await page.getByRole("link", { name: /sign in with sso/i }).click();
  await page.locator("#username, input[name='username']").first().fill(OIDC_MEMBER_USER);
  await page.locator("#password, input[name='password']").first().fill(OIDC_MEMBER_PASSWORD);
  await page.locator("#kc-login, button[type='submit'], input[type='submit']").first().click();
  await page.waitForURL((url) => !/\/realms\//.test(url.toString()), { timeout: 30_000 });
}

/**
 * Run one SQL statement through the SQL Lab query backend (POST /api/ws/{ws}/queries,
 * the endpoint the Run button calls) and return the results JSON `{columns, rows}`.
 * Retries while a freshly provisioned coordinator is warming up (500s ~30-40s until
 * SERVER STARTED) or a just-created Ranger policy hasn't propagated yet.
 */
export async function runQuery(
  request: APIRequestContext,
  h: Record<string, string>,
  workspaceSlug: string,
  engineName: string,
  sql: string,
): Promise<{ columns: Array<{ name: string }>; rows: string[][] }> {
  const engineRe = new RegExp(engineName, "i");
  const dbs = (await (await request.get(`/api/ws/${workspaceSlug}/databases`)).json()) as Array<{
    id: string;
    name?: string;
  }>;
  const db = dbs.find((d) => engineRe.test(d.name ?? ""));
  expect(db, `database for ${engineName} is listed`).toBeTruthy();

  let out: { columns: Array<{ name: string }>; rows: string[][] } = { columns: [], rows: [] };
  await expect(async () => {
    const sub = await request.post(`/api/ws/${workspaceSlug}/queries`, {
      headers: h,
      data: { database_id: db!.id, sql },
    });
    expect(sub.ok(), `submit → ${sub.status()} ${await sub.text()}`).toBeTruthy();
    const qid = (await sub.json()).id as string;
    let state = "";
    let error = "";
    for (let i = 0; i < 60; i++) {
      const s = await (await request.get(`/api/ws/${workspaceSlug}/queries/${qid}`)).json();
      state = s.state ?? s.status ?? "";
      error = s.error ?? "";
      if (["finished", "succeeded", "failed", "error"].includes(state)) break;
      await new Promise((r) => setTimeout(r, 1_000));
    }
    expect(state, `query state (err: ${error}) for: ${sql}`).toMatch(/finished|succeeded/);
    out = await (await request.get(`/api/ws/${workspaceSlug}/queries/${qid}/results`)).json();
  }).toPass({ timeout: 300_000, intervals: [4_000] });
  return out;
}

/** The Ranger profile (service) name auto-bound to `engineName` (= engine name). */
export async function engineProfileName(
  request: APIRequestContext,
  engineName: string,
): Promise<string> {
  const profiles = (await (
    await request.get("/api/control-plane/policies/services")
  ).json()) as Array<{ name: string; engines?: Array<{ name?: string }> }>;
  const p =
    profiles.find((x) => x.engines?.some((e) => e.name === engineName)) ??
    profiles.find((x) => x.name === engineName);
  expect(p, `Ranger profile for ${engineName}`).toBeTruthy();
  return p!.name;
}

/** Create one policy via the policy admin API; returns its id (for cleanup). */
export async function createPolicy(
  request: APIRequestContext,
  h: Record<string, string>,
  body: Record<string, unknown>,
): Promise<number> {
  const res = await request.post("/api/control-plane/policies", { headers: h, data: body });
  expect(res.ok(), `create policy ${body.name} → ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()).id as number;
}
