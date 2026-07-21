import { test, expect, type APIRequestContext } from "@playwright/test";
import { loginViaOidcMember } from "./helpers";

/**
 * Keycloak back-channel logout e2e (BCL Task 3) against a kind deploy.
 *
 * Tasks 1-2 built `POST /api/auth/oidc/backchannel-logout` — egret validates a
 * Keycloak logout token (keying on the `sub` claim, not `sid`) and revokes the
 * user's live sessions. This spec proves the headline end-to-end: force-logging
 * a user out THROUGH KEYCLOAK (not egret) still kills their egret session,
 * because Keycloak calls back to that endpoint.
 *
 * The realm-JSON config (`backchannel.logout.url` / `.session.required` on the
 * `egret` client — see deploy/helm/egret/files/egret-realm.json and its
 * deploy/keycloak/ twin) only takes effect on a FRESH Keycloak import. The kind
 * deploy this e2e runs against was already up before that config landed, so
 * this test wires the same two attributes onto the live client via the
 * Keycloak admin REST API at setup time, making it self-contained (no
 * re-deploy required to exercise the real flow).
 *
 * `member-sso`'s Keycloak user id is pinned in the realm JSON
 * (`a5510000-0000-4000-8000-000000000002`) specifically so this test (and any
 * other) can target it without a lookup-by-username round trip.
 *
 * The Keycloak admin API (used to wire the client + force-logout) is reached
 * over LOOPBACK, not the ingress host: Keycloak's default `sslRequired=external`
 * rejects the admin-cli password grant over plain HTTP from a non-loopback
 * client ("HTTPS required"), but exempts 127.0.0.1. So the browser OIDC login
 * uses the ingress base URL while these admin calls target
 * `EGRET_E2E_KC_ADMIN_URL` (default `http://127.0.0.1:18080`, a
 * `kubectl port-forward` the recipe sets up). This weakens no Keycloak config.
 *
 * Opt-in (EGRET_E2E_KIND=1); NOT in `npm test` / GitHub CI. Run via
 * `just e2e-kind-backchannel-logout`.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";

const KEYCLOAK_ADMIN_USER = process.env.EGRET_E2E_KEYCLOAK_ADMIN_USER ?? "admin";
const KEYCLOAK_ADMIN_PASSWORD = process.env.EGRET_E2E_KEYCLOAK_ADMIN_PASSWORD ?? "admin";

// Keycloak admin-API origin. Loopback so `sslRequired=external` is satisfied over
// HTTP (see the file docstring). Absolute URLs override the request context's
// ingress baseURL. This Keycloak serves under a `/auth` relative path.
const KC_ADMIN = (process.env.EGRET_E2E_KC_ADMIN_URL ?? "http://127.0.0.1:18080") + "/auth";

// Fixed in deploy/keycloak/egret-realm.json (and its helm copy) precisely so tests
// can target `member-sso` without a lookup-by-username round trip.
const MEMBER_SSO_USER_ID = "a5510000-0000-4000-8000-000000000002";

// Must be reachable BY KEYCLOAK in-cluster — the egret Service (backend-service.yaml:
// name `egret`, port 8080) in the `egret-system` namespace, same host
// `EGRET_CATALOG__PUBLIC_URL` uses. The OIDC router is nested at `/api/auth/oidc`.
const BACKCHANNEL_LOGOUT_URL =
  "http://egret.egret-system.svc.cluster.local:8080/api/auth/oidc/backchannel-logout";

/** Fetch an admin access token from Keycloak's `master` realm (admin-cli, password grant). */
async function getKeycloakAdminToken(request: APIRequestContext): Promise<string> {
  const res = await request.post(`${KC_ADMIN}/realms/master/protocol/openid-connect/token`, {
    form: {
      grant_type: "password",
      client_id: "admin-cli",
      username: KEYCLOAK_ADMIN_USER,
      password: KEYCLOAK_ADMIN_PASSWORD,
    },
  });
  expect(res.ok(), `keycloak admin token → ${res.status()} ${await res.text()}`).toBeTruthy();
  return ((await res.json()) as { access_token: string }).access_token;
}

/**
 * Point the `egret` client's backchannel-logout URL at the in-cluster egret Service,
 * live, via the Keycloak admin API — see the file docstring for why this can't just
 * rely on the realm-JSON import on an already-running deploy.
 */
async function wireBackchannelLogout(request: APIRequestContext, adminToken: string): Promise<void> {
  const authHeaders = { Authorization: `Bearer ${adminToken}` };

  const list = await request.get(`${KC_ADMIN}/admin/realms/egret/clients?clientId=egret`, {
    headers: authHeaders,
  });
  expect(list.ok(), `find egret client → ${list.status()} ${await list.text()}`).toBeTruthy();
  const clients = (await list.json()) as Array<Record<string, unknown>>;
  expect(clients.length, "the egret client exists in the egret realm").toBeGreaterThan(0);
  const client = clients[0];

  const attributes = { ...((client.attributes as Record<string, string>) ?? {}) };
  attributes["backchannel.logout.url"] = BACKCHANNEL_LOGOUT_URL;
  attributes["backchannel.logout.session.required"] = "false";

  const put = await request.put(`${KC_ADMIN}/admin/realms/egret/clients/${client.id}`, {
    headers: { ...authHeaders, "Content-Type": "application/json" },
    data: { ...client, attributes },
  });
  expect(put.ok(), `wire backchannel-logout url → ${put.status()} ${await put.text()}`).toBeTruthy();
}

test.describe("Keycloak backchannel-logout revokes the egret session", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against a kind deploy");
  test.setTimeout(90_000);

  test("Keycloak force-logout kills member-sso's egret session", async ({ page, request }) => {
    const adminToken = await getKeycloakAdminToken(request);
    await wireBackchannelLogout(request, adminToken);

    // Establish a real egret session for member-sso through the actual OIDC flow.
    await loginViaOidcMember(page);
    const meBefore = await page.request.get("/api/auth/me");
    expect(meBefore.status(), "member-sso has a live egret session after login").toBe(200);

    // Force-logout THROUGH KEYCLOAK (not egret) — this is what makes Keycloak fire
    // the back-channel logout call to the URL wired above.
    const logout = await request.post(
      `${KC_ADMIN}/admin/realms/egret/users/${MEMBER_SSO_USER_ID}/logout`,
      { headers: { Authorization: `Bearer ${adminToken}` } },
    );
    expect(logout.ok(), `force logout → ${logout.status()} ${await logout.text()}`).toBeTruthy();

    // Back-channel delivery is async (Keycloak → egret over the in-cluster Service)
    // — poll for a few seconds rather than asserting immediately.
    await expect(async () => {
      const me = await page.request.get("/api/auth/me");
      expect(me.status(), "egret session is revoked after the Keycloak force-logout").toBe(401);
    }).toPass({ timeout: 15_000, intervals: [1_000] });
  });
});
