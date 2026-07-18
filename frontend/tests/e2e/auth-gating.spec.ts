import { test, expect } from "@playwright/test";
import { loginLocalAdmin, loginViaOidcMember } from "./helpers";

/**
 * Auth privilege gate against the in-cluster kind deploy (egret.localtest.me).
 * Opt-in: EGRET_E2E_KIND=1. NOT part of `npm test` / GitHub CI.
 *
 * Proves the control plane is admin-only: a break-glass platform admin reaches it,
 * a JIT-provisioned OIDC member is denied (RequireControlPlaneAccess → Forbidden).
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";

test.describe("auth gating", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.setTimeout(60_000);

  test("break-glass admin reaches the control plane", async ({ page }) => {
    await loginLocalAdmin(page);
    // The Header exposes the Control Plane entry to admins.
    await expect(page.getByRole("link", { name: "Control Plane" })).toBeVisible();
    // The access console renders (not Forbidden) — seeded admin user is listed.
    await page.goto("/control-plane/users");
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByText("admin@egret.local")).toBeVisible();
  });

  test("an OIDC member is denied the control plane", async ({ page }) => {
    await loginViaOidcMember(page);
    await page.goto("/control-plane");
    await expect(page.getByText("403 — Forbidden")).toBeVisible();
  });
});
