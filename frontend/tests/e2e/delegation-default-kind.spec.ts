import { test, expect, type APIRequestContext } from "@playwright/test";
import {
  loginLocalAdmin,
  csrf,
  firstWorkspace,
  provisionStorageAndWarehouse,
  provisionTrinoEngine,
  waitForEngineReady,
  assignEngineToWorkspace,
  runQuery,
  postJson,
  deleteEngine,
} from "./helpers";

/**
 * Default-delegation e2e (Task 5, spec 2026-07-21) against a RANGER-ON kind deploy
 * that has `catalog.public_url` configured (`deploy-kind-governance` sets
 * `EGRET_CATALOG__PUBLIC_URL` unconditionally via configmap-env.yaml — no extra
 * `--set` needed).
 *
 * Backend Tasks 1-4 made per-user JWT delegation the DEFAULT for both Trino and
 * StarRocks: an OMITTED `delegationMode` now resolves to `"jwt"` whenever the
 * platform can mint/validate engine JWTs (`has_platform_jwks`, ⟺ `catalog.public_url`
 * is set), degrading to `"impersonate"` only when it can't (or, for StarRocks, when
 * the FE image predates `authentication_jwt`). This test proves that resolution
 * live: provision a Trino AND a StarRocks engine with NO `delegationMode` field in
 * the create body at all, and assert the engine-detail GET surfaces the RESOLVED
 * mode as `"jwt"` — then runs a query through each to prove the jwt path is not
 * just persisted but actually live end-to-end.
 *
 * Per-user query scope: this test drives the query as the (local) platform admin
 * rather than standing up a second OIDC identity + hand-rolled Ranger masking/
 * row-filter policies. The admin's Trino/StarRocks username is still minted into a
 * PER-USER engine JWT by egret's own machine-IdP (never a shared secret) — see
 * `ranger-auto-admin-kind.spec.ts` (auto-seeded per-admin-user Ranger policy) and
 * `sts-broker-kind.spec.ts` (egret-minted per-user JWT proof + the SSO admin-vs-
 * member masking/row-filter parity test). Re-proving that SAME enforcement
 * machinery here would duplicate those specs without adding coverage; this test's
 * unique contribution is the DEFAULT-RESOLUTION assertion (step 2 below) plus a
 * basic successful query proving the default-resolved jwt path actually works, not
 * just that it round-trips through the API. Full masking assertion: SKIPPED here
 * (see above) — covered elsewhere for the jwt-delegation path.
 *
 * Opt-in (EGRET_E2E_KIND=1) against a ranger-on kind deploy; NOT in `npm test` /
 * GitHub CI. Run via `just e2e-kind-delegation-default`.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

/**
 * Provision a small (1 FE, 1 CN) StarRocks engine wired to the warehouse with NO
 * `delegationMode` / `tlsEnabled` in the body at all — the whole point of this
 * test is proving the platform DEFAULT (not an explicit opt-in, unlike
 * `provisionStarRocksEngineJwt` in helpers.ts) resolves to jwt. The default image
 * (4.1+) is JWT-capable, so no `feImage` override is needed.
 */
async function provisionStarRocksEngineDefault(
  req: APIRequestContext,
  h: Record<string, string>,
  runId: string,
  warehouseId: string,
): Promise<string> {
  const engine = await postJson(req, "/api/control-plane/engines", h, {
    name: `e2e-sr-def-${runId}`,
    warehouseId,
    kind: "starrocks",
    coordinatorReplicas: 1,
    workerReplicas: 1,
    cpuMin: "250m",
    cpuMax: "1",
    // A StarRocks FE needs ~2Gi+ heap; 4Gi is the value proven to bring the FE up
    // live for a delegation-enabled StarRocks 4.1 engine (see helpers.ts).
    memory: "4Gi",
    starrocks: {
      feReplicas: 1,
      cnReplicas: 1,
      // NO delegationMode, NO tlsEnabled — the default-resolution path under test.
    },
  });
  return engine.id as string;
}

/**
 * Poll `GET /engines/{id}` until the stored spec surfaces a non-empty resolved
 * delegation mode at the given DTO path, or throw with the last-seen value. The
 * resolution actually happens synchronously in the create handler (before
 * provisioning kicks off), so this should resolve on the first read — poll anyway
 * per the task's async-safety margin.
 */
async function waitForResolvedDelegationMode(
  req: APIRequestContext,
  engineId: string,
  engineKind: "trino" | "starrocks",
  timeoutMs = 30_000,
): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  let last = "";
  while (Date.now() < deadline) {
    const res = await req.get(`/api/control-plane/engines/${engineId}`);
    expect(res.ok(), `GET engine ${engineId} → ${res.status()}`).toBeTruthy();
    const body = await res.json();
    last =
      engineKind === "trino"
        ? (body.spec?.trinoConfig?.delegationMode ?? "")
        : (body.spec?.starrocksConfig?.delegation?.mode ?? "");
    if (last) return last;
    await new Promise((r) => setTimeout(r, 2_000));
  }
  throw new Error(
    `engine ${engineId} (${engineKind}) never surfaced a resolved delegation mode within ${timeoutMs}ms (last: ${JSON.stringify(last)})`,
  );
}

test.describe("default-on jwt delegation on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against a ranger-on kind deploy");
  // Full engine-provisioning lifecycle — StarRocks FE cold-start is slow. Keep
  // serial — a single kind node runs hot.
  test.describe.configure({ mode: "serial" });
  test.setTimeout(900_000);

  test("default-provisioned Trino resolves to jwt delegation and serves a query", async ({
    page,
    context,
  }) => {
    await loginLocalAdmin(page);
    const request = page.request;
    const h = await csrf(context);
    let engineId: string | undefined;

    try {
      const ws = await firstWorkspace(request);
      const { warehouseId } = await provisionStorageAndWarehouse(request, h, RUN_ID, ws.id);

      // NO trinoConfig / delegationMode in the body at all — the default path.
      engineId = await provisionTrinoEngine(request, h, RUN_ID, warehouseId);

      // The CORE assertion: an omitted delegationMode resolved to "jwt" because
      // this deploy has catalog.public_url configured (has_platform_jwks == true).
      const mode = await waitForResolvedDelegationMode(request, engineId, "trino");
      expect(mode, "default (no delegationMode) Trino provision resolves to jwt").toBe("jwt");

      await waitForEngineReady(request, engineId);
      await assignEngineToWorkspace(request, h, engineId, ws.id);
      const eng = `e2e-trino-${RUN_ID}`;

      // Basic per-user proof: the admin's own SELECT succeeds solely off the
      // egret-auto-seeded admin Ranger policy (no hand-created access policy) and
      // the jwt-delegated coordinator — proving the DEFAULT-resolved jwt path is
      // live end-to-end, not just persisted in the stored spec.
      const table = `egret.delegdefault.probe_${RUN_ID.replace(/[^a-zA-Z0-9_]/g, "_")}`;
      await runQuery(request, h, ws.slug, eng, "CREATE SCHEMA IF NOT EXISTS egret.delegdefault");
      await runQuery(request, h, ws.slug, eng, `CREATE TABLE IF NOT EXISTS ${table} (id bigint, item varchar)`);
      await runQuery(request, h, ws.slug, eng, `INSERT INTO ${table} VALUES (1,'apple'),(2,'banana')`);

      const out = await runQuery(request, h, ws.slug, eng, `SELECT id, item FROM ${table} ORDER BY id`);
      const idIdx = out.columns.findIndex((c) => c.name === "id");
      const itemIdx = out.columns.findIndex((c) => c.name === "item");
      expect(out.rows.length, "admin query returns both seeded rows via the jwt-delegated coordinator").toBe(2);
      expect(out.rows.map((r) => String(r[idIdx]))).toEqual(["1", "2"]);
      expect(out.rows.map((r) => r[itemIdx])).toEqual(["apple", "banana"]);
    } finally {
      if (engineId) await deleteEngine(request, h, engineId);
    }
  });

  test("default-provisioned StarRocks resolves to jwt delegation and serves a query", async ({
    page,
    context,
  }) => {
    await loginLocalAdmin(page);
    const request = page.request;
    const h = await csrf(context);
    const runId = `${RUN_ID}-sr`;
    let engineId: string | undefined;

    try {
      const ws = await firstWorkspace(request);
      const { warehouseId } = await provisionStorageAndWarehouse(request, h, runId, ws.id);

      // NO delegationMode / tlsEnabled in the body at all — the default path.
      engineId = await provisionStarRocksEngineDefault(request, h, runId, warehouseId);

      // The CORE assertion: an omitted delegationMode resolved to "jwt" (StarRocks
      // FE image defaults to 4.1+, which is JWT-capable, so nothing steers it to
      // impersonate).
      const mode = await waitForResolvedDelegationMode(request, engineId, "starrocks");
      expect(mode, "default (no delegationMode) StarRocks provision resolves to jwt").toBe("jwt");

      await waitForEngineReady(request, engineId);
      await assignEngineToWorkspace(request, h, engineId, ws.id);
      const eng = `e2e-sr-def-${runId}`;

      // Basic per-user proof, via SQL Lab (same mechanism the Trino case above
      // uses): the admin's own SELECT succeeds solely off the egret-auto-seeded
      // admin Ranger policy (no hand-created access policy) and the
      // jwt-delegated coordinator — proving the DEFAULT-resolved jwt path is
      // live end-to-end, not just persisted in the stored spec.
      //
      // The StarRocks JWT security integration (CREATE SECURITY INTEGRATION +
      // authentication_chain="native,egret_oidc") is bootstrapped ASYNCHRONOUSLY
      // after the FE pod reports "running" (waitForEngineReady only gates on the
      // pod phase). Until it lands, the FE's chain is just "native", so the
      // per-user JWT login for `preferred_username` falls to native auth and is
      // rejected ("Access denied ... using password: NO"). `runQuery`'s own
      // `toPass` retry (300s budget) already re-submits on exactly this kind of
      // transient failure, so no extra retry loop is needed here.
      const table = `egret.delegdefault_sr.probe_${runId.replace(/[^a-zA-Z0-9_]/g, "_")}`;
      await runQuery(request, h, ws.slug, eng, "CREATE SCHEMA IF NOT EXISTS egret.delegdefault_sr");
      await runQuery(request, h, ws.slug, eng, `CREATE TABLE IF NOT EXISTS ${table} (id bigint, item varchar)`);
      await runQuery(request, h, ws.slug, eng, `INSERT INTO ${table} VALUES (1,'apple'),(2,'banana')`);

      const out = await runQuery(request, h, ws.slug, eng, `SELECT id, item FROM ${table} ORDER BY id`);
      const idIdx = out.columns.findIndex((c) => c.name === "id");
      const itemIdx = out.columns.findIndex((c) => c.name === "item");
      expect(out.rows.length, "admin query returns both seeded rows via the jwt-delegated coordinator").toBe(2);
      expect(out.rows.map((r) => String(r[idIdx]))).toEqual(["1", "2"]);
      expect(out.rows.map((r) => r[itemIdx])).toEqual(["apple", "banana"]);
    } finally {
      if (engineId) await deleteEngine(request, h, engineId);
    }
  });
});
