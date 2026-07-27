import { test, expect } from "@playwright/test";
import { loginLocalAdmin, csrf, postJson, firstWorkspace, addWorkspaceMember } from "./helpers";

/**
 * User impersonation e2e against the ordinary dev stack (docker compose +
 * `just serve` + `npm run dev` — see README.md), NOT the in-cluster kind
 * deploy: pure browser UI plus a couple of control-plane API calls, so it
 * follows `workspace-features.spec.ts`'s pattern rather than the `*-kind`
 * specs. Opt-in via `EGRET_E2E_IMPERSONATION=1`.
 *
 * Proves the headline: a platform admin impersonates another user from
 * Control Plane → Users, and:
 *   1. the persistent amber banner appears, naming the target;
 *   2. the app's identity resolves to the target — the user menu reads
 *      "Signed in as <target>", `GET /api/auth/me` carries the target's
 *      email/roles plus the `impersonating` overlay, and a target-only view
 *      (no Control Plane link, since the seeded target holds no
 *      control-plane permission) differs from the admin's own;
 *   3. clicking Exit on the banner removes it and reverts identity/`me` back
 *      to the admin.
 *
 * Login is rate-limited (5 attempts / 5 min per ip+email) — this spec signs
 * in ONCE (`loginLocalAdmin`, the local break-glass platform admin) and drives
 * everything else — seeding the target, starting/exiting the impersonation,
 * and every assertion — off that one session.
 *
 * The suite's only ready-made non-admin fixtures are OIDC identities
 * (`tester@egret.local` / `member-sso@egret.local`), which need a real
 * Keycloak login to prove anything about — not needed here, since starting
 * impersonation only requires the target's id, not a session of theirs. So
 * this seeds a local, no-CP-permission member via the same control-plane API
 * `local-user-mgmt-kind.spec.ts` uses (`POST /api/control-plane/users`), then
 * adds it to the shared seeded workspace as `ws_editor` (matching the role
 * used for a plain member elsewhere in the suite) so the impersonated session
 * lands somewhere real in Explore rather than bouncing off `RequireWorkspace`.
 *
 * Why the assertions run from `/explore/{ws}`, not Control Plane → Users
 * (where the Impersonate action lives): `RequireControlPlaneAccess` sits
 * OUTSIDE `AppShell` in `App.tsx` — it swaps in a bare `<Forbidden/>` (no
 * `AppShell`, no banner) the instant `me` no longer carries a control-plane
 * permission, which happens the moment the impersonation overlay resolves
 * `me` to the seeded (permission-less) target. Staying on the Users page
 * would race the banner against that guard tearing the whole shell down; the
 * target's own `ws_editor` grant keeps `RequireWorkspace` satisfied on
 * `/explore/{ws}`, so `AppShell` (and the banner inside it) renders there
 * instead.
 *
 * Cleanup: exit any still-active impersonation AND delete the seeded target
 * user in a `finally`, so a failed run can't leave the admin's session stuck
 * impersonating (impersonation auto-expires in 30 min, but exit explicitly
 * rather than rely on that) nor leak the seeded user into later runs.
 */
const ENABLED = process.env.EGRET_E2E_IMPERSONATION === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;
const ADMIN_EMAIL = process.env.EGRET_E2E_EMAIL ?? "admin@egret.local";
const ADMIN_PASSWORD = process.env.EGRET_E2E_PASSWORD ?? "Admin12345!";

test.describe("user impersonation", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_IMPERSONATION=1 with the live dev stack up");
  test.setTimeout(60_000);

  test("platform admin impersonates a seeded member, sees their identity, and exits", async ({
    page,
  }) => {
    await loginLocalAdmin(page);
    const request = page.request; // shares the logged-in session cookie
    const h = await csrf(page.context());

    // Seed a plain local member with no control-plane permission — the target
    // this test impersonates.
    const targetEmail = `e2e-impersonate-${RUN_ID}@egret.local`;
    const target = await postJson(request, "/api/control-plane/users", h, {
      email: targetEmail,
      name: "E2E Impersonation Target",
      password: "TargetPw12345!",
    });
    const targetId = target.id as string;

    const ws = await firstWorkspace(request);
    await addWorkspaceMember(request, h, ws.slug, targetEmail, "ws_editor");

    try {
      // Control Plane → Users → find the seeded target and start impersonation.
      await page.goto("/control-plane/users");
      await expect(page.getByRole("heading", { name: "Users" })).toBeVisible();

      // Narrow to the seeded row via the table's own search (avoids relying on
      // sort order / pagination to surface a freshly-created user).
      await page.getByPlaceholder("Search users…").fill(targetEmail);
      await page.getByRole("button", { name: `Actions for ${targetEmail}` }).click();
      await page.getByRole("menuitem", { name: "Impersonate" }).click();

      await expect(page.getByRole("heading", { name: "Impersonate user" })).toBeVisible();
      await page.getByLabel(/reason/i).fill("E2E impersonation proof");
      await page.getByLabel(/^password$/i).fill(ADMIN_PASSWORD);
      const submit = page.getByRole("button", { name: "Impersonate", exact: true });
      await expect(submit).toBeEnabled();
      await submit.click();

      // The dialog closing is the observable "overlay created" signal — the
      // POST has resolved server-side by the time it's gone.
      await expect(page.getByRole("heading", { name: "Impersonate user" })).toHaveCount(0);

      // `GET /api/auth/me` now resolves to the target — the impersonation
      // overlay carries both identities.
      const meImpersonating = await (await request.get("/api/auth/me")).json();
      expect(meImpersonating.email, "me.email resolves to the target").toBe(targetEmail);
      expect(meImpersonating.user_id, "me.user_id resolves to the target").toBe(targetId);
      expect(
        meImpersonating.platform_roles,
        "the target has no platform roles",
      ).not.toContain("platform_admin");
      expect(meImpersonating.impersonating?.target?.email).toBe(targetEmail);
      expect(meImpersonating.impersonating?.impersonator?.email).toBe(ADMIN_EMAIL);
      expect(meImpersonating.impersonating?.expires_at).toBeTruthy();

      // `RequireControlPlaneAccess` sits outside `AppShell`, so it swaps in a
      // bare Forbidden (no banner) the instant `me` no longer carries a CP
      // permission — which is now, since the overlay resolved `me` to the
      // permission-less target. Move to a page the TARGET can actually reach
      // (`ws_editor` on the seeded workspace) so `AppShell` — and the banner
      // inside it — renders.
      await page.goto(`/explore/${ws.slug}`);

      // 1. The persistent banner names the target.
      const banner = page.getByRole("status");
      await expect(banner).toContainText("Impersonating");
      await expect(banner).toContainText(targetEmail);
      await expect(banner).toContainText(/\d{2}:\d{2} left/);
      const exitButton = banner.getByRole("button", { name: "Exit" });
      await expect(exitButton).toBeVisible();

      // 2. The app's identity reflects the target: the user menu reads
      // "Signed in as <target>" ...
      await page.getByRole("button", { name: "User menu" }).click();
      const userMenu = page.getByRole("menu");
      await expect(userMenu.getByText("Signed in as")).toBeVisible();
      await expect(userMenu.getByText(targetEmail)).toBeVisible();
      await page.keyboard.press("Escape");

      // ... and a target-only view differs from the admin's own: the seeded
      // target holds no control-plane permission, so the Control Plane link
      // (visible to the real admin on every Explore page) is gone.
      await expect(page.getByRole("link", { name: "Control Plane" })).toHaveCount(0);

      // 3. Exit returns to the admin: the banner disappears ...
      await exitButton.click();
      await expect(banner).toHaveCount(0);

      // ... the user menu reverts to the admin ...
      await page.getByRole("button", { name: "User menu" }).click();
      await expect(page.getByRole("menu").getByText(ADMIN_EMAIL)).toBeVisible();
      await page.keyboard.press("Escape");

      // ... and the Control Plane link (admin-only) is back.
      await expect(page.getByRole("link", { name: "Control Plane" })).toBeVisible();

      // `/api/auth/me` confirms the same: back to the admin, no overlay.
      const meAfterExit = await (await request.get("/api/auth/me")).json();
      expect(meAfterExit.email).toBe(ADMIN_EMAIL);
      expect(meAfterExit.platform_roles).toContain("platform_admin");
      expect(meAfterExit.impersonating ?? null).toBeNull();
    } finally {
      // Force-exit any still-active impersonation first (best-effort — a 400
      // "not impersonating" is expected and fine if the Exit click above
      // already succeeded), so a failed assertion above can't leave the
      // admin's session stuck impersonating for a later run.
      await request.post("/api/auth/impersonation/exit", { headers: h }).catch(() => {});
      // Clean up the seeded target user (best-effort).
      await request.delete(`/api/control-plane/users/${targetId}`, { headers: h }).catch(() => {});
    }
  });
});
