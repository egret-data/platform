import { test, expect } from "@playwright/test";
import { loginLocalAdmin, csrf, postJson } from "./helpers";

/**
 * Control-plane access console against the in-cluster kind deploy. Opt-in
 * (EGRET_E2E_KIND=1), NOT in `npm test` / GitHub CI. As the break-glass admin:
 * the console pages load seeded data, a group CRUD round-trip works through the UI,
 * and an audited mutation (role:assign) shows in the audit log.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

test.describe("control-plane admin", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.setTimeout(90_000);

  test.beforeEach(async ({ page }) => {
    await loginLocalAdmin(page);
  });

  test("access console pages load with seeded data", async ({ page }) => {
    await page.goto("/control-plane/users");
    await expect(page.getByText("admin@egret.local")).toBeVisible();

    await page.goto("/control-plane/roles");
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByText(/platform_admin/i).first()).toBeVisible();

    await page.goto("/control-plane/permissions");
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByText("group:create").first()).toBeVisible();
  });

  test("RBAC group CRUD round-trip via the UI", async ({ page }) => {
    const name = `e2e-grp-${RUN_ID}`;
    await page.goto("/control-plane/groups");

    // Create.
    await page.getByRole("button", { name: "New group" }).click();
    await page.locator("#new-group-name").fill(name);
    await page.getByRole("button", { name: "Create" }).click();
    await expect(page.getByText(name)).toBeVisible();

    // Delete: open the row's actions menu → Delete → type "delete" → confirm.
    await page.getByRole("button", { name: `Actions for ${name}` }).click();
    await page.getByRole("menuitem", { name: /Delete/ }).click();
    await page.getByPlaceholder("delete").fill("delete");
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    await expect(page.getByText(name)).toHaveCount(0);
  });

  test("audit log records an audited mutation (role:assign)", async ({ page, context }) => {
    // Trigger an audited mutation via the API (group create/delete are NOT audited;
    // role:assign is). page.request shares the logged-in admin session.
    const h = await csrf(context);
    const grp = await postJson(page.request, "/api/control-plane/groups", h, {
      name: `e2e-audit-${RUN_ID}`,
    });
    const roles = (await (await page.request.get("/api/control-plane/access/roles")).json()) as Array<{
      id: string;
    }>;
    expect(roles.length, "at least one role exists").toBeGreaterThan(0);
    const assign = await page.request.post(`/api/control-plane/groups/${grp.id}/roles`, {
      headers: h,
      data: { role_id: roles[0].id },
    });
    expect(assign.ok(), `assign role → ${assign.status()} ${await assign.text()}`).toBeTruthy();

    // The audit log (UI) surfaces the role:assign entry. Filter by action, allowing
    // a brief propagation delay via toPass.
    await page.goto("/control-plane/audit-log");
    await page.locator("#audit-action").fill("role:assign");
    await expect(async () => {
      await expect(page.getByText("role:assign").first()).toBeVisible({ timeout: 3_000 });
    }).toPass({ timeout: 30_000 });

    // Cleanup.
    await page.request.delete(`/api/control-plane/groups/${grp.id}`, { headers: h }).catch(() => {});
  });
});
