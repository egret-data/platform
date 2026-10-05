import {
  test,
  expect,
  type Page,
  type Browser,
  type BrowserContext,
  type APIRequestContext,
} from "@playwright/test";
import { loginLocalAdmin, csrf, postJson } from "./helpers";

/**
 * Local user management e2e (sub-project 2, Task 5) against a kind deploy.
 *
 * Proves the headline of sub-project 2: disabling a local user revokes their
 * live session IMMEDIATELY — `update_user` in egret-controlplane's
 * `access/mod.rs` calls `st.sessions.revoke_all_for_user(user_id)` in the same
 * request that flips `status: "disabled"`, rather than merely rejecting the
 * user's NEXT login. The test proves this by holding a still-open session
 * (via a raw `GET /api/auth/me`) across the admin's disable call and showing
 * it flips from 200 to 401 with no new request from the user in between.
 *
 * Also covers password reset (`POST /users/{id}/password` revokes old
 * sessions the same way) and hard-delete (`DELETE /users/{id}`, plus a fresh
 * login with the now-gone credentials failing) as cheap follow-ons that reuse
 * the same setup.
 *
 * Opt-in (EGRET_E2E_KIND=1); NOT in `npm test` / GitHub CI. Run via
 * `just e2e-kind-local-user`. The admin logs in via the UI exactly ONCE
 * (`beforeAll`) and that session is shared across all tests in this file —
 * local login is rate-limited 5/5min per ip:email (see
 * sts-broker-kind.spec.ts / e2e-e2e-dataplane-kind memory), so re-running
 * `loginLocalAdmin` per test would risk tripping it.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

/** Admin-side: create a local user via the control-plane. Returns its id. */
async function createLocalUser(
  request: APIRequestContext,
  h: Record<string, string>,
  email: string,
  password: string,
): Promise<string> {
  const created = await postJson(request, "/api/control-plane/users", h, {
    email,
    name: "E2E local-user-mgmt",
    password,
  });
  return created.id as string;
}

/**
 * Log the given local user in through the real `/api/auth/login` endpoint
 * (not the UI) inside a FRESH browser context, so their session cookies never
 * touch the admin session under test elsewhere in the file. Returns the new
 * context (caller must close it) and an `APIRequestContext` that carries the
 * user's session cookie for subsequent calls (e.g. `GET /api/auth/me`).
 */
async function loginAsLocalUser(
  browser: Browser,
  email: string,
  password: string,
): Promise<{ context: BrowserContext; request: APIRequestContext }> {
  const context = await browser.newContext();
  const request = (await context.newPage()).request;
  const res = await request.post("/api/auth/login", { data: { email, password } });
  expect(res.ok(), `login ${email} → ${res.status()} ${await res.text()}`).toBeTruthy();
  return { context, request };
}

test.describe("local user management on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against a kind deploy");
  test.describe.configure({ mode: "serial" });
  test.setTimeout(120_000);

  let page: Page;
  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await loginLocalAdmin(page);
  });
  test.afterAll(async () => {
    await page.close();
  });

  test("disable locks the user out immediately", async ({ browser }) => {
    const request = page.request;
    const h = await csrf(page.context());
    const email = `e2e-lockout-${RUN_ID}@egret.local`;
    const password = "Lockout12345!";
    const userId = await createLocalUser(request, h, email, password);

    const { context: userCtx, request: userReq } = await loginAsLocalUser(browser, email, password);
    try {
      const meBefore = await userReq.get("/api/auth/me");
      expect(meBefore.status(), "user has a live session before disable").toBe(200);

      const patch = await request.patch(`/api/control-plane/users/${userId}`, {
        headers: h,
        data: { status: "disabled" },
      });
      expect(patch.ok(), `disable → ${patch.status()} ${await patch.text()}`).toBeTruthy();

      // Immediate lockout: re-use the SAME still-open session (no new login) —
      // it must now be rejected, proving the disable call revoked it synchronously
      // rather than just blocking the user's next login attempt.
      const meAfter = await userReq.get("/api/auth/me");
      expect(meAfter.status(), "disabled user's live session is revoked immediately").toBe(401);
    } finally {
      await userCtx.close();
    }
  });

  test("password reset invalidates the old session", async ({ browser }) => {
    const request = page.request;
    const h = await csrf(page.context());
    const email = `e2e-pwreset-${RUN_ID}@egret.local`;
    const oldPassword = "OldPass12345!";
    const newPassword = "NewPass12345!";
    const userId = await createLocalUser(request, h, email, oldPassword);

    const { context: userCtx, request: userReq } = await loginAsLocalUser(browser, email, oldPassword);
    try {
      const meBefore = await userReq.get("/api/auth/me");
      expect(meBefore.status(), "user has a live session before reset").toBe(200);

      const reset = await request.post(`/api/control-plane/users/${userId}/password`, {
        headers: h,
        data: { password: newPassword },
      });
      expect(reset.ok(), `reset password → ${reset.status()} ${await reset.text()}`).toBeTruthy();

      const meAfter = await userReq.get("/api/auth/me");
      expect(meAfter.status(), "old session is revoked immediately on password reset").toBe(401);
    } finally {
      await userCtx.close();
    }
  });

  test("delete removes the account", async ({ browser }) => {
    const request = page.request;
    const h = await csrf(page.context());
    const email = `e2e-delete-${RUN_ID}@egret.local`;
    const password = "DeleteMe12345!";
    const userId = await createLocalUser(request, h, email, password);

    const { context: userCtx, request: userReq } = await loginAsLocalUser(browser, email, password);
    try {
      const meBefore = await userReq.get("/api/auth/me");
      expect(meBefore.status(), "user has a live session before delete").toBe(200);

      const del = await request.delete(`/api/control-plane/users/${userId}`, { headers: h });
      expect(del.ok(), `delete → ${del.status()} ${await del.text()}`).toBeTruthy();

      const meAfter = await userReq.get("/api/auth/me");
      expect(meAfter.status(), "deleted user's live session is revoked immediately").toBe(401);

      const reLogin = await userReq.post("/api/auth/login", { data: { email, password } });
      expect(reLogin.status(), "fresh login with the deleted account's creds fails").toBe(401);
    } finally {
      await userCtx.close();
    }
  });
});
