# E2E Data-Plane Money-Path on Kind — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An automated Playwright spec that provisions a Trino engine via the egret API and asserts `SELECT current_user` = caller (impersonation) in SQL Lab against the in-cluster kind deploy, plus a `just e2e-kind` runner.

**Architecture:** Hybrid — provision the whole engine lifecycle through the egret control-plane API (`page.request`, sharing the logged-in session cookie), then assert the money-path through the real SQL Lab browser UI. One long-running spec, gated behind `EGRET_E2E_KIND=1`, with mandatory `afterAll` cleanup to free node memory.

**Tech Stack:** Playwright (`@playwright/test`), TypeScript, `just`, bash, kubectl/helm (kind).

---

## Design notes (grounded API surface)

All endpoints are mounted under `/api/control-plane` (`crates/egret-server/src/lib.rs:277`), except `/api/me/workspaces`. camelCase on the wire. Verified against `crates/egret-engine-mgmt/src/engines.rs`:

- `GET /api/me/workspaces` → `[{ id, slug, ... }]` (egret init seeds a `default` workspace).
- `POST /api/control-plane/storage` `{ name, provider:"s3", region, bucket, endpoint, key_prefix, access_key, path_style, new_secret:{name,value} }` → `{ id }`.
- `POST /api/control-plane/warehouses` `{ name, workspaceId, storageConnectionId }` → `{ id, lakekeeperWarehouseId, name }`.
- `POST /api/control-plane/engines` `CreateEngineBody` `{ name, warehouseId, kind:"trino", coordinatorReplicas, workerReplicas, cpuMin, cpuMax, memory, trinoConfig? }` → `EngineDto { id, status, spec, ... }`. Creating with `warehouseId` auto-wires the warehouse as the primary `egret-warehouse` catalog.
- `GET /api/control-plane/engines/{id}` → `EngineDetailDto { ...engine, status:{ phase, message, pods } }`. **`status.phase === "running"`** ⇔ all pods ready (Ready). Other phases: `"provisioning"`, `"failed"` (`crates/egret-orchestrator/src/trino.rs:1182`).
- `PUT /api/control-plane/engines/{id}` takes the **full** `CreateEngineBody` (replace). Enable JWT by re-sending the spec with `trinoConfig.auth.jwt = {}` — an empty `jwksUri` falls back to the CP default engine JWKS (`resolve_auth`, engines.rs:1282). The deploy must be OIDC-on so that default is non-empty.
- `PUT /api/control-plane/engines/{id}/workspaces` `{ workspaceIds:[uuid] }` (`SetWorkspacesBody`) → assign.
- `DELETE /api/control-plane/engines/{id}` → cleanup.

Ranger is **off** on this deploy → a fresh Trino writes+mounts `access-control.name=allow-all` itself (PR #272) → `egret_delegator` impersonation is permitted with no extra config.

## File Structure

- **Modify** `frontend/tests/e2e/helpers.ts` — add shared `csrf`, `postJson`, workspace + provisioning + wait + assign + delete + `runSqlInLab` helpers.
- **Modify** `frontend/tests/e2e/embedded-catalog.spec.ts` — import `csrf`/`postJson` from helpers (remove local copies; DRY).
- **Create** `frontend/tests/e2e/dataplane-kind.spec.ts` — the lifecycle spec.
- **Create** `deploy/k8s/wait-ready.sh` — block until egret+keycloak Ready and `/api/auth/config` returns 200.
- **Modify** `justfile` — add `deploy-kind-oidc` and `e2e-kind`.
- **Modify** `frontend/tests/e2e/README.md` — add the "Data-plane e2e on kind" section.

---

### Task 1: Extract shared `csrf` + `postJson` into helpers (DRY refactor)

**Files:**
- Modify: `frontend/tests/e2e/helpers.ts`
- Modify: `frontend/tests/e2e/embedded-catalog.spec.ts`

- [ ] **Step 1: Add the shared API helpers to `helpers.ts`**

Append to `frontend/tests/e2e/helpers.ts` (and widen the top import):

```ts
import {
  type Page,
  type BrowserContext,
  type APIRequestContext,
  expect,
} from "@playwright/test";

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
```

Keep the existing `import { type Page } from "@playwright/test";` line replaced by the widened import above (don't duplicate the `Page` import).

- [ ] **Step 2: Point `embedded-catalog.spec.ts` at the shared helpers**

In `frontend/tests/e2e/embedded-catalog.spec.ts`: change the import line

```ts
import { loginLocalAdmin } from "./helpers";
```

to

```ts
import { loginLocalAdmin, csrf, postJson as json } from "./helpers";
```

and **delete** the two local definitions (the `csrf` function, lines 20-25, and the `json` function, lines 27-31). All existing `json(...)` / `csrf(...)` call sites keep working via the aliased import. Also remove the now-unused `type APIRequestContext` / `type BrowserContext` from its top import if TypeScript flags them.

- [ ] **Step 3: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: PASS (no errors). This proves the refactor didn't break the existing spec's types.

- [ ] **Step 4: Commit**

```bash
git add frontend/tests/e2e/helpers.ts frontend/tests/e2e/embedded-catalog.spec.ts
git commit -m "test(e2e): extract shared csrf/postJson helpers (DRY)"
```

---

### Task 2: Add engine-provisioning helpers

**Files:**
- Modify: `frontend/tests/e2e/helpers.ts`

- [ ] **Step 1: Append the provisioning + polling helpers**

Add to `frontend/tests/e2e/helpers.ts`:

```ts
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
```

- [ ] **Step 2: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add frontend/tests/e2e/helpers.ts
git commit -m "test(e2e): add Trino engine provisioning + polling helpers"
```

---

### Task 3: Add the `runSqlInLab` browser helper

**Files:**
- Modify: `frontend/tests/e2e/helpers.ts`

- [ ] **Step 1: Append `runSqlInLab`**

Add to `frontend/tests/e2e/helpers.ts`. This centralizes the brittle SQL Lab selectors so there is one place to adjust during the live acceptance run:

```ts
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
```

- [ ] **Step 2: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add frontend/tests/e2e/helpers.ts
git commit -m "test(e2e): add runSqlInLab browser helper"
```

> Note: the SQL Lab selectors (`combobox`, `option`, `Run`, results tabpanel) are best-effort and will be finalized against the real UI in Task 7's live run — expected for browser e2e. Keep all such tweaks inside this one helper.

---

### Task 4: Write the data-plane lifecycle spec

**Files:**
- Create: `frontend/tests/e2e/dataplane-kind.spec.ts`

- [ ] **Step 1: Write the spec**

Create `frontend/tests/e2e/dataplane-kind.spec.ts`:

```ts
import { test, expect, type APIRequestContext } from "@playwright/test";
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
  runSqlInLab,
} from "./helpers";

/**
 * Data-plane money-path e2e against the IN-CLUSTER kind deploy (egret.localtest.me).
 * Opt-in: set EGRET_E2E_KIND=1. NOT part of `npm test` / GitHub CI.
 *
 * Provisions the whole lifecycle via the egret API, then asserts impersonation in
 * SQL Lab: `SELECT current_user` returns the CALLER (admin), not `egret_delegator`.
 * Proves OIDC → engine JWKS → JWT → TLS:8443 → impersonation → query end-to-end.
 *
 * Prereq (one-time): a kind `egret` cluster + ingress + coredns-rewrite, then the
 * OIDC-on deploy — `just e2e-kind` handles the deploy + readiness wait.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

test.describe("data-plane on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");

  // Engine provisioning takes minutes; give the whole lifecycle 10 minutes.
  test.setTimeout(600_000);

  let cleanupReq: APIRequestContext | undefined;
  let cleanupHeaders: Record<string, string> | undefined;
  let engineId: string | undefined;

  test.afterAll(async () => {
    // Free node memory — the single kind node runs hot. Best-effort.
    if (cleanupReq && cleanupHeaders && engineId) {
      await deleteEngine(cleanupReq, cleanupHeaders, engineId);
    }
  });

  test("provision Trino → enable JWT → assign → SELECT current_user = caller", async ({
    page,
    context,
  }) => {
    await loginLocalAdmin(page);
    const request = page.request; // shares the logged-in session cookie
    const h = await csrf(context);
    cleanupReq = request;
    cleanupHeaders = h;

    // 1. Storage + warehouse.
    const ws = await firstWorkspace(request);
    const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);

    // 2. Provision a small Trino engine and wait until it is Ready.
    engineId = await provisionTrinoEngine(request, h, RUN_ID, warehouseId);
    await waitForEngineReady(request, engineId);

    // 3. Enable egret-JWT auth, then wait for the coordinator to re-provision.
    await enableEngineJwt(request, h, engineId);
    await waitForEngineReady(request, engineId);

    // 4. Assign the engine to the workspace.
    await assignEngineToWorkspace(request, h, engineId, ws.id);

    // 5. Assert impersonation in SQL Lab.
    const out = await runSqlInLab(page, ws.slug, `e2e-trino-${RUN_ID}`, "SELECT current_user");
    expect(out, "SELECT current_user returns the caller (impersonation), not egret_delegator")
      .toMatch(/\badmin\b/);
    expect(out).not.toMatch(/egret_delegator/);
  });
});
```

- [ ] **Step 2: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Verify the gate skips cleanly with the flag off**

Run: `cd frontend && npx playwright test dataplane-kind`
Expected: 1 skipped, 0 failed (the `test.skip(!ENABLED, ...)` guard fires because `EGRET_E2E_KIND` is unset). This proves the spec is wired into the runner and doesn't accidentally run against a missing stack.

- [ ] **Step 4: Commit**

```bash
git add frontend/tests/e2e/dataplane-kind.spec.ts
git commit -m "test(e2e): data-plane money-path lifecycle spec (kind)"
```

---

### Task 5: Add the readiness-wait script

**Files:**
- Create: `deploy/k8s/wait-ready.sh`

- [ ] **Step 1: Write the script**

Create `deploy/k8s/wait-ready.sh`:

```bash
#!/usr/bin/env bash
# Block until the in-cluster egret deploy is ready to serve e2e:
#   - egret + keycloak pods Ready
#   - OIDC discovery converged (/api/auth/config returns 200 through the ingress)
# Run after `just deploy-kind-oidc`, before the Playwright e2e.
set -euo pipefail

NS="${EGRET_NS:-egret-system}"
BASE="${EGRET_E2E_BASE_URL:-http://egret.localtest.me}"

echo "waiting for egret pods in ${NS}..."
kubectl wait --for=condition=Ready pod -l app=egret -n "${NS}" --timeout=300s
# Keycloak may be absent on an OIDC-off deploy; don't fail if the label matches nothing.
kubectl wait --for=condition=Ready pod -l app=keycloak -n "${NS}" --timeout=300s || \
  echo "note: no keycloak pods matched (OIDC-off deploy?) — continuing"

echo "waiting for OIDC discovery at ${BASE}/api/auth/config ..."
for i in $(seq 1 60); do
  if curl -sf "${BASE}/api/auth/config" >/dev/null 2>&1; then
    echo "auth-config is up (attempt ${i})"
    exit 0
  fi
  sleep 5
done
echo "ERROR: ${BASE}/api/auth/config did not return 200 within ~5m" >&2
exit 1
```

- [ ] **Step 2: Make it executable + syntax-check**

Run:
```bash
chmod +x deploy/k8s/wait-ready.sh
bash -n deploy/k8s/wait-ready.sh && echo "syntax OK"
```
Expected: `syntax OK`.

- [ ] **Step 3: Commit**

```bash
git add deploy/k8s/wait-ready.sh
git commit -m "chore(e2e): add wait-ready.sh (pods Ready + OIDC discovery up)"
```

---

### Task 6: Add the `just` recipes

**Files:**
- Modify: `justfile`

- [ ] **Step 1: Add `deploy-kind-oidc` + `e2e-kind`**

Insert into `justfile` after the existing `deploy-kind` recipe (mirror it, add the OIDC/ingress helm flags):

```make
# Like deploy-kind but OIDC-on + ingress (Milestone B) — required for the data-plane e2e.
deploy-kind-oidc: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m \
      --set oidc.enabled=true --set ingress.enabled=true
    @echo "egret SPA: http://egret.localtest.me  (login admin@egret.local / Admin12345!)"

# Opt-in data-plane e2e against the in-cluster kind deploy (egret.localtest.me).
# One-time prereq: just k8s-up && just ingress-install && just coredns-rewrite
e2e-kind: deploy-kind-oidc
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      npx playwright test dataplane-kind
```

- [ ] **Step 2: Verify the recipes parse**

Run: `just --list`
Expected: output includes `deploy-kind-oidc` and `e2e-kind`.

- [ ] **Step 3: Commit**

```bash
git add justfile
git commit -m "chore(e2e): add just deploy-kind-oidc + e2e-kind recipes"
```

---

### Task 7: Document the data-plane e2e in the README

**Files:**
- Modify: `frontend/tests/e2e/README.md`

- [ ] **Step 1: Append the section**

Add to the end of `frontend/tests/e2e/README.md`:

```markdown
## Data-plane e2e on kind (`dataplane-kind.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), and unlike the other specs it targets the
**in-cluster kind deploy** (`http://egret.localtest.me`), not the host dev stack.
It provisions the full lifecycle via the egret API — storage → warehouse → Trino
engine (1Gi, 1 worker) → enable egret-JWT → assign workspace — then asserts in
SQL Lab that `SELECT current_user` returns the **caller** (`admin`), proving
per-user impersonation (OIDC → engine JWKS → JWT → TLS:8443 → impersonation →
query). It cleans up the engine in `afterAll` to free node memory.

### One-time prereq

```bash
just k8s-up            # kind `egret` cluster
just ingress-install   # nginx-ingress (kind flavor)
just coredns-rewrite   # egret.localtest.me resolves cluster-wide
```

### Run

```bash
just e2e-kind          # deploy-kind-oidc → wait-ready → playwright dataplane-kind
```

`just e2e-kind` re-deploys (idempotent `helm upgrade --install`), waits for pods +
OIDC discovery, then runs the spec. Re-runnable on the same cluster — each run uses
a unique `EGRET_E2E_RUN_ID` suffix and cleans up its engine.

Env overrides: `EGRET_E2E_BASE_URL`, `EGRET_E2E_RUN_ID`, `EGRET_E2E_BUCKET`,
`EGRET_E2E_S3_ENDPOINT` / `EGRET_E2E_S3_KEY` / `EGRET_E2E_S3_SECRET`, and the
break-glass admin `EGRET_E2E_EMAIL` / `EGRET_E2E_PASSWORD`.

> If the single kind node is memory-wedged, delete stale engines first:
> `kubectl get deploy -n egret-engines` then `kubectl delete deploy <name>` (or
> delete the engine via the CP UI).
```

- [ ] **Step 2: Commit**

```bash
git add frontend/tests/e2e/README.md
git commit -m "docs(e2e): document the data-plane e2e on kind"
```

---

### Task 8: Live acceptance run

**Files:** none (verification only; fixes land in the relevant file from Tasks 1-7).

- [ ] **Step 1: Ensure the cluster prereqs exist**

Run: `kind get clusters` → expect `egret`. If ingress/coredns aren't set up (fresh cluster), run `just ingress-install && just coredns-rewrite`. (The current cluster from Project 1 already has these.)

- [ ] **Step 2: Run the full e2e**

Run: `just e2e-kind`
Expected: the spec PASSES — `SELECT current_user` returns `admin`. Playwright prints `1 passed`.

- [ ] **Step 3: Fix any selector drift**

If `runSqlInLab` fails at the Database dropdown / Run / results step, adjust the selectors in `helpers.ts` (Task 3) against the live DOM (use the failure trace / screenshot Playwright captures on failure), re-run `just e2e-kind` until green. Commit any fix:

```bash
git add frontend/tests/e2e/helpers.ts
git commit -m "test(e2e): finalize SQL Lab selectors against live UI"
```

- [ ] **Step 4: Confirm unit gates still green**

Run: `cd frontend && npm run lint && npm run typecheck && npm test`
Expected: all pass (the new spec is opt-in and excluded from `npm test`, which is Vitest, not Playwright).

---

## Self-Review

- **Spec coverage:** harness retarget to kind (Tasks 3/4/6 — baseURL env + `e2e-kind` recipe) ✓; lifecycle spec provision-via-API + assert-via-browser (Task 4) ✓; `just e2e-kind` + `deploy-kind-oidc` + `wait-ready.sh` (Tasks 5/6) ✓; docs (Task 7) ✓; live acceptance (Task 8) ✓; cleanup/idempotency (Task 4 `afterAll` + per-run ids) ✓; 1Gi sizing (Task 2) ✓; JWT-via-sequential-API not UI toggle (Task 2 `enableEngineJwt`) ✓; Monaco `setValue` (Task 3) ✓.
- **No placeholders:** every endpoint/DTO is pinned from `engines.rs`; all code is complete.
- **Type consistency:** helper names/signatures used in the spec (Task 4) match their definitions (Tasks 1-3): `csrf`, `firstWorkspace`, `provisionStorageAndWarehouse`, `provisionTrinoEngine`, `waitForEngineReady`, `enableEngineJwt`, `assignEngineToWorkspace`, `deleteEngine`, `runSqlInLab`.
- **Out of scope (unchanged):** other 5 Project-2 areas, kind-in-GitHub-CI, StarRocks data-plane, backend code.
