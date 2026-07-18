# E2E Governance Enforcement on Kind — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One Playwright lifecycle spec on a Ranger-enabled kind deploy that provisions a Ranger-bound Trino engine, seeds a governed Iceberg table, creates access + mask + row-filter policies via the egret policy admin API, and asserts a query returns masked column values + only the row-filtered rows.

**Architecture:** egret embeds the Ranger admin (`/service`) — no separate Ranger server. A `ranger.enabled` deploy makes provisioned engines Ranger-bound. Provisioning + policy setup + queries go through the egret API (`page.request`); the enforcement assertion is a `SELECT` through the SQL Lab query backend, run as the admin whose username the policies target.

**Tech Stack:** Playwright, TypeScript, Helm, `just`.

---

## Design notes (grounded surface)

- `config.ranger.enabled` defaults **false** (`egret-config`); with it true, `main.rs:1039` builds the Ranger state → egret mounts `/service` and `EngineMgmtState.ranger_enabled=true` (`main.rs:1481`) makes engine creation **auto-bind a fresh Ranger profile named after the engine** (`engines.rs` `create_service(&b.name, "trino", …)`). `auth_token=""` = anonymous `/service` for the stock plugin.
- **Policy admin API** `POST /api/control-plane/policies` takes an **editor** JSON `{kind, name, service, isEnabled, resources, items}` (`editor_to_ranger_json`, `admin/ranger_json.rs`):
  - `kind:"access"` → `items:[{users:[…], accesses:[…], effect:"allow"}]`
  - `kind:"masking"` → `items:[{users:[…], maskType:"MASK"}]`
  - `kind:"rowfilter"` → `items:[{users:[…], filterExpr:"country = 'US'"}]`
  - `resources` = `{ level: { values:[…] } }`; Trino levels: `catalog, schema, table, column`, plus `queryid`, `trinouser` for the default-deny globals.
  - `service` = the engine's auto-bound profile name (= engine name). Find it via `GET /api/control-plane/policies/services` → `[{id, name, type, engines:[…]}]`.
  - `DELETE /api/control-plane/policies/{id}` removes one (cleanup).
- **Query runs as the caller** = the caller's Trino username, derived from the email local-part (`ensure_username` in `egret-auth/users.rs`) → `admin@egret.local` → **`admin`**. Resolve it via `GET /api/me` → `username`. Policies target `users:[username]`.
- **Default-deny ordering:** with Ranger bound, the admin needs `execute` (queryid=*) + `impersonate` (trinouser=*) + broad table access BEFORE any query (incl. DDL/DML). So create the allow policies + wait for the plugin poll, THEN seed, THEN add mask/row-filter + wait, THEN assert. The plugin polls ~5s.
- **Reuse** slice-1 helpers (`csrf`, `provisionStorageAndWarehouse`, `provisionTrinoEngine`, `waitForEngineReady`, `enableEngineJwt`, `assignEngineToWorkspace`, `deleteEngine`, `firstWorkspace`). Login once per spec (shared page, serial — slice-2 rate-limit lesson).
- `deploy-kind-oidc` recipe + `configmap-env.yaml` emit env values gated by `.Values.X` (e.g. `EGRET_OIDC__ENABLED: "{{ .Values.oidc.enabled }}"`). Mirror for ranger.

## File Structure

- **Modify** `deploy/helm/egret/values.yaml` — add `ranger: { enabled: false }`.
- **Modify** `deploy/helm/egret/templates/configmap-env.yaml` — add `EGRET_RANGER__ENABLED` + `EGRET_RANGER__AUTH_TOKEN`.
- **Modify** `justfile` — `deploy-kind-governance` + `e2e-kind-governance`.
- **Modify** `frontend/tests/e2e/helpers.ts` — add `runQuery` + `engineProfileName` + `createPolicy`.
- **Create** `frontend/tests/e2e/governance-kind.spec.ts` — the enforcement lifecycle.
- **Modify** `frontend/tests/e2e/README.md` — governance subsection.

---

### Task 1: Chart — enable Ranger

**Files:**
- Modify: `deploy/helm/egret/values.yaml`
- Modify: `deploy/helm/egret/templates/configmap-env.yaml`

- [ ] **Step 1: Add the values block**

In `deploy/helm/egret/values.yaml`, after the `oidc:` block, add:

```yaml
ranger:
  enabled: false
```

- [ ] **Step 2: Emit the env**

In `deploy/helm/egret/templates/configmap-env.yaml`, after the `EGRET_OIDC__*` lines, add:

```yaml
  EGRET_RANGER__ENABLED: "{{ .Values.ranger.enabled }}"
  EGRET_RANGER__AUTH_TOKEN: ""
```

- [ ] **Step 3: Verify the template renders**

Run: `helm template egret deploy/helm/egret --set ranger.enabled=true | grep EGRET_RANGER`
Expected: shows `EGRET_RANGER__ENABLED: "true"` and `EGRET_RANGER__AUTH_TOKEN: ""`.

- [ ] **Step 4: Commit**

```bash
git add deploy/helm/egret/values.yaml deploy/helm/egret/templates/configmap-env.yaml
git commit -m "feat(deploy): chart ranger.enabled toggle (EGRET_RANGER__ env)"
```

---

### Task 2: `just` recipes — Ranger-on deploy + runner

**Files:**
- Modify: `justfile`

- [ ] **Step 1: Add the recipes**

Insert into `justfile` after the `e2e-kind` recipe:

```make
# Like deploy-kind-oidc but ALSO Ranger-on — provisioned Trino engines bind Ranger
# (egret embeds the Ranger admin at /service). For the governance enforcement e2e.
deploy-kind-governance: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m \
      --set oidc.enabled=true --set ingress.enabled=true --set ranger.enabled=true
    @echo "egret (ranger-on): http://egret.localtest.me"

# Opt-in governance enforcement e2e (mask + row-filter on a live Trino query).
e2e-kind-governance: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      npx playwright test governance-kind
```

- [ ] **Step 2: Verify the recipes parse**

Run: `just --list`
Expected: shows `deploy-kind-governance` and `e2e-kind-governance`.

- [ ] **Step 3: Commit**

```bash
git add justfile
git commit -m "chore(e2e): add deploy-kind-governance + e2e-kind-governance recipes"
```

---

### Task 3: Helpers — query runner + policy helpers

**Files:**
- Modify: `frontend/tests/e2e/helpers.ts`

- [ ] **Step 1: Append the helpers**

Add to `frontend/tests/e2e/helpers.ts`:

```ts
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
```

- [ ] **Step 2: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add frontend/tests/e2e/helpers.ts
git commit -m "test(e2e): add runQuery + Ranger policy helpers"
```

---

### Task 4: Governance enforcement spec

**Files:**
- Create: `frontend/tests/e2e/governance-kind.spec.ts`

- [ ] **Step 1: Write the spec**

Create `frontend/tests/e2e/governance-kind.spec.ts`:

```ts
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
    const username = (await (await request.get("/api/me")).json()).username as string;
    const T = "iceberg.demo.report"; // catalog.schema.table (Trino catalog is `iceberg`)
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
          resources: R({ catalog: ["iceberg"], schema: ["*"], table: ["*"], column: ["*"] }),
          // NOTE: exact access strings for DDL/DML are finalized live (Task 6); "all"
          // is the broad grant. If the service-def rejects it, use the explicit list.
          items: [{ users: [username], accesses: ["all"], effect: "allow" }],
        }),
      );
      await new Promise((r) => setTimeout(r, POLL_MS));

      // 3. Seed a governed table with data (some US rows, some not).
      await runQuery(request, h, ws.slug, eng, "CREATE SCHEMA IF NOT EXISTS iceberg.demo");
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
          resources: R({ catalog: ["iceberg"], schema: ["demo"], table: ["report"], column: ["item"] }),
          items: [{ users: [username], maskType: "MASK" }],
        }),
        await createPolicy(request, h, {
          kind: "rowfilter", name: `e2e-rowf-${RUN_ID}`, service,
          resources: R({ catalog: ["iceberg"], schema: ["demo"], table: ["report"] }),
          items: [{ users: [username], filterExpr: "country = 'US'" }],
        }),
      );
      await new Promise((r) => setTimeout(r, POLL_MS));

      // 5. Enforcement: only US rows, and `item` masked (not the raw values).
      const enforced = await runQuery(request, h, ws.slug, eng, `SELECT id, item, country FROM ${T} ORDER BY id`);
      const itemIdx = enforced.columns.findIndex((c) => c.name === "item");
      const countryIdx = enforced.columns.findIndex((c) => c.name === "country");
      expect(enforced.rows.every((r) => r[countryIdx] === "US"), "row-filter: only US rows").toBeTruthy();
      expect(enforced.rows.length, "2 US rows").toBe(2);
      // MASK redacts the value; assert the raw items are gone (exact masked form
      // finalized live in Task 6 — e.g. all-x or a hash).
      const items = enforced.rows.map((r) => r[itemIdx]);
      expect(items, "item is masked, not raw").not.toContain("book");
      expect(items, "item is masked, not raw").not.toContain("lamp");
    } finally {
      for (const id of policyIds) {
        await request.delete(`/api/control-plane/policies/${id}`, { headers: h }).catch(() => {});
      }
      if (engineId) await deleteEngine(request, h, engineId);
    }
  });
});
```

- [ ] **Step 2: Typecheck + gate-skip**

Run: `cd frontend && npm run typecheck && npx playwright test governance-kind`
Expected: typecheck PASS; Playwright reports **1 skipped** (gate unset).

- [ ] **Step 3: Commit**

```bash
git add frontend/tests/e2e/governance-kind.spec.ts
git commit -m "test(e2e): governance enforcement spec (mask + row-filter on Trino)"
```

> Note: the Ranger access-type strings (`all` vs explicit `select`/`create`/`insert`), the exact masked-value form, the poll delay, and the default-deny ordering are the live-tuned parts (Task 6) — expect iteration, like slice 1.

---

### Task 5: Document

**Files:**
- Modify: `frontend/tests/e2e/README.md`

- [ ] **Step 1: Append the subsection**

Add to `frontend/tests/e2e/README.md`:

```markdown
## Governance enforcement e2e (`governance-kind.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), needs a **Ranger-on** deploy — run via `just
e2e-kind-governance` (deploys with `--set ranger.enabled=true`, so provisioned Trino
engines bind Ranger; egret embeds the Ranger admin at `/service`, no separate server).

Provisions a Ranger-bound Trino engine, seeds a governed Iceberg table, creates
access + column-mask + row-filter policies (via `/api/control-plane/policies`,
targeting the admin's own Trino user), and asserts the admin's `SELECT` returns
MASKED `item` values and ONLY `country='US'` rows. The Trino Ranger plugin polls
every ~5s, so policy changes take a few seconds to take effect.
```

- [ ] **Step 2: Commit**

```bash
git add frontend/tests/e2e/README.md
git commit -m "docs(e2e): document governance enforcement e2e"
```

---

### Task 6: Live acceptance (heavy iteration expected)

**Files:** none (fixes land in the spec/helpers/chart from Tasks 1–5).

- [ ] **Step 1: Deploy Ranger-on + run**

Ensure the kind cluster is up. Run: `just e2e-kind-governance`
Expected (goal): the spec PASSES — `1 passed` — the enforced query shows 2 US rows with masked `item`.

- [ ] **Step 2: Tune the enforcement specifics against the live engine**

This is the crown-jewel but flakiest slice; iterate like slice 1. Use the Playwright failure trace + `kubectl logs` on the coordinator + the Ranger plugin cache (`/tmp/ranger-<...>-cache/*.json` in the coordinator pod) to tune:
- **Access strings** — if `accesses:["all"]` is rejected by the Trino service-def or DDL/DML is still denied, switch to the explicit set the plugin expects (e.g. `["select","insert","create","use"]`); confirm from the manual recipe / service-def.
- **Default-deny ordering** — if the seed (CREATE/INSERT) is denied, the allow policies haven't propagated; increase `POLL_MS` or the seed's `runQuery` retry.
- **Masked form** — adjust the assertion to the actual `MASK` output (all-`x`, hash, or `NULL`); assert "not the raw value" robustly.
- **Profile discovery** — if `engineProfileName` doesn't find the profile, inspect `GET /api/control-plane/policies/services` and match on the right field.

Re-run `just e2e-kind-governance` until green. Commit fixes:

```bash
git add frontend/tests/e2e/governance-kind.spec.ts frontend/tests/e2e/helpers.ts
git commit -m "test(e2e): finalize governance enforcement against live Ranger+Trino"
```

- [ ] **Step 3: Confirm unit gates**

Run: `cd frontend && npm run lint && npm run typecheck && npm test`
Expected: all pass (the spec is opt-in, excluded from Vitest).

---

## Self-Review

- **Spec coverage:** chart ranger toggle (Task 1) ✓; ranger-on runner (Task 2) ✓; provision Ranger-bound engine + seed + policies + assert mask/row-filter (Task 4) ✓; query/policy helpers (Task 3) ✓; docs (Task 5) ✓; live acceptance (Task 6) ✓; cleanup in `finally` (Task 4) ✓; default-deny ordering handled (allow → poll → seed → mask/filter → poll → assert) ✓.
- **No placeholders:** every endpoint/DTO pinned from source (policy editor structs, `/policies/services`, `/api/me` username, configmap pattern). The genuinely engine-dependent values (Ranger access strings, masked form, poll timing) are explicitly flagged as Task-6 live-tuning, consistent with slice 1.
- **Type consistency:** `runQuery` / `engineProfileName` / `createPolicy` / slice-1 helpers used in the spec match their signatures.
- **Design deviation (intentional):** single-user enforcement (policies target the admin's own username) per the spec — avoids a second session while still proving mask + row-filter.
