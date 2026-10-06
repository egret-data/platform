import { test, expect } from "@playwright/test";
import { loginLocalAdmin, csrf, postJson as json } from "./helpers";

/**
 * Embedded-catalog end-to-end (opt-in — needs the live stack + a reserved,
 * initially-empty RustFS bucket; NOT in CI). Proves the machine loop:
 *   provision warehouse → role → grant → Catalog Client
 *   → /oauth/token (mini-IdP) → /catalog/v1 (embedded lakekeeper) → egret guard.
 *
 * Enable with `EGRET_E2E_CATALOG=1`. The bucket (`EGRET_E2E_BUCKET`, default
 * `e2e`) must exist and hold NO prefix-less warehouse — each run uses a unique
 * `key_prefix`, so repeated runs on a reserved bucket don't overlap.
 */
const ENABLED = process.env.EGRET_E2E_CATALOG === "1";
const BUCKET = process.env.EGRET_E2E_BUCKET ?? "e2e";
const S3_ENDPOINT = process.env.EGRET_E2E_S3_ENDPOINT ?? "http://localhost:9000";
const S3_KEY = process.env.EGRET_E2E_S3_KEY ?? "egret";
const S3_SECRET = process.env.EGRET_E2E_S3_SECRET ?? "egretsecret";

test.describe("embedded catalog", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_CATALOG=1 with the live stack + a reserved bucket");

  test("provision → token → catalog; grant gates writes", async ({ page, context }) => {
    await loginLocalAdmin(page);
    // `page.request` shares the logged-in session cookie (the standalone `request`
    // fixture does not).
    const request = page.request;
    const h = await csrf(context);
    const run = Date.now();

    // Platform admin: storage → warehouse (unique prefix) → role → client.
    const storage = await json(request, "/api/control-plane/storage", h, {
      name: `e2e-store-${run}`,
      provider: "s3",
      region: "us-east-1",
      bucket: BUCKET,
      endpoint: S3_ENDPOINT,
      key_prefix: `run-${run}`,
      access_key: S3_KEY,
      path_style: true,
      new_secret: { name: `e2e-sec-${run}`, value: S3_SECRET },
    });
    const me = await (await request.get("/api/me/workspaces")).json();
    const workspaceId = me[0].id;

    const warehouse = await json(request, "/api/control-plane/warehouses", h, {
      name: `lakehouse-${run}`,
      workspaceId,
      storageConnectionId: storage.id,
    });
    expect(warehouse.lakekeeperWarehouseId, "warehouse was provisioned in lakekeeper").toBeTruthy();

    const role = await json(request, `/api/control-plane/warehouses/${warehouse.id}/roles`, h, {
      name: "engine",
      consumerType: "engine",
    });
    // A second role with NO grants, to prove the deny path.
    const roleNoGrant = await json(request, `/api/control-plane/warehouses/${warehouse.id}/roles`, h, {
      name: "nogrant",
      consumerType: "engine",
    });
    // add_grant returns 201 with an empty body — don't parse it as JSON.
    const grantRes = await request.post(`/api/control-plane/roles/${role.id}/grants`, {
      headers: h,
      data: { securableType: "catalog", privilege: "write" },
    });
    expect(grantRes.ok(), `add grant → ${grantRes.status()} ${await grantRes.text()}`).toBeTruthy();

    const mkClient = async (roleId: string) =>
      json(request, `/api/control-plane/warehouses/${warehouse.id}/clients`, h, {
        name: `c-${roleId.slice(0, 8)}`,
        consumerType: "engine",
        workspaceId,
        catalogRoleId: roleId,
      });
    const client = await mkClient(role.id);
    const clientNoGrant = await mkClient(roleNoGrant.id);

    // Machine path: client credentials → egret JWT.
    const tokenRes = await request.post("/oauth/token", {
      form: {
        grant_type: "client_credentials",
        client_id: client.clientId,
        client_secret: client.clientSecret,
      },
    });
    expect(tokenRes.ok()).toBeTruthy();
    const token = (await tokenRes.json()).access_token as string;
    const bearer = { Authorization: `Bearer ${token}` };

    // Warehouse discovery works and is scoped to this warehouse.
    const cfg = await request.get(`/catalog/v1/config?warehouse=${warehouse.name}`, { headers: bearer });
    expect(cfg.status(), await cfg.text()).toBe(200);
    const prefix = (await cfg.json()).defaults.prefix as string;

    // Write path (has the write grant): create a namespace → 200.
    const ns = await request.post(`/catalog/v1/${prefix}/namespaces`, {
      headers: { ...bearer, "Content-Type": "application/json" },
      data: { namespace: [`e2e_${run}`], properties: {} },
    });
    expect(ns.status(), await ns.text()).toBe(200);

    // Deny path: the no-grant client cannot read the data plane → 403.
    const denyTokenRes = await request.post("/oauth/token", {
      form: {
        grant_type: "client_credentials",
        client_id: clientNoGrant.clientId,
        client_secret: clientNoGrant.clientSecret,
      },
    });
    const denyToken = (await denyTokenRes.json()).access_token as string;
    const denied = await request.get(`/catalog/v1/${prefix}/namespaces`, {
      headers: { Authorization: `Bearer ${denyToken}` },
    });
    expect(denied.status(), "a role with no grants is forbidden on the data plane").toBe(403);
  });
});
