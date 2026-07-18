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
  while (Date.now() < deadline) {
    const res = await req.get(`/api/control-plane/engines/${id}`);
    if (res.ok()) {
      const body = await res.json();
      last = body.status?.phase ?? "";
      if (last === "running") return;
      if (last === "failed") {
        throw new Error(`engine ${id} 'failed': ${JSON.stringify(body.status)}`);
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
 * Drive SQL Lab: open the workspace's SQL Lab, set the editor to `sql` (Monaco
 * intercepts direct clicks, so set the model value programmatically), pick the
 * engine in the Database dropdown, Run, and return the results panel text.
 */
export async function runSqlInLab(
  page: Page,
  workspaceSlug: string,
  engineName: string,
  sql: string,
): Promise<string> {
  await page.goto(`/explore/${workspaceSlug}/sql-lab`);

  // Monaco must be mounted before we can set the model value.
  await page.waitForFunction(
    () => (window as any).monaco?.editor?.getModels?.().length > 0,
    null,
    { timeout: 20_000 },
  );
  await page.evaluate(
    (q) => (window as any).monaco.editor.getModels()[0].setValue(q),
    sql,
  );

  // Pick the engine in the Database dropdown (a Radix combobox trigger).
  const dbTrigger = page.getByRole("combobox").first();
  await dbTrigger.click();
  await page.getByRole("option", { name: new RegExp(engineName, "i") }).first().click();

  // Run and wait for the results tabpanel to populate.
  await page.getByRole("button", { name: /^Run$/ }).click();

  const results = page.locator('[role="tabpanel"][data-state="active"]');
  await results.waitFor({ state: "visible", timeout: 60_000 });
  // Give the query round-trip a moment to render rows into the active panel.
  await page.waitForFunction(
    () => {
      const p = document.querySelector('[role="tabpanel"][data-state="active"]');
      return !!p && /rows|row\b/i.test(p.textContent ?? "");
    },
    null,
    { timeout: 60_000 },
  );
  return (await results.innerText()).trim();
}
