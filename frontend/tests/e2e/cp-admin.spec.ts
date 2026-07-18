import { test, expect, type Page } from "@playwright/test";
import { loginLocalAdmin, csrf, postJson } from "./helpers";

/**
 * Control-plane access console against the in-cluster kind deploy. Opt-in
 * (EGRET_E2E_KIND=1), NOT in `npm test` / GitHub CI. As the break-glass admin:
 * the console pages load seeded data, a group CRUD round-trip works through the UI,
 * and an audited mutation (role:assign) shows in the audit log.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

// Serial + one shared login: the login endpoint is rate-limited (5 attempts /
// 5 min per ip+email), so a per-test `beforeEach` login would trip it when the
// suite is re-run. Log in ONCE and reuse the authenticated page across tests.
test.describe.configure({ mode: "serial" });

test.describe("control-plane admin", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.setTimeout(90_000);

  let page: Page;
  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await loginLocalAdmin(page);
  });
  test.afterAll(async () => {
    await page.close();
  });

  test("access console pages load with seeded data", async () => {
    await page.goto("/control-plane/users");
    await expect(page.getByText("admin@egret.local")).toBeVisible();

    await page.goto("/control-plane/roles");
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByText(/platform_admin/i).first()).toBeVisible();

    await page.goto("/control-plane/permissions");
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByText("group:create").first()).toBeVisible();
  });

  test("RBAC group CRUD round-trip via the UI", async () => {
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

  test("audit log records an audited mutation (role:assign)", async () => {
    // Trigger an audited mutation via the API (group create/delete are NOT audited;
    // role:assign is). page.request shares the logged-in admin session.
    const h = await csrf(page.context());
    const grp = await postJson(page.request, "/api/control-plane/groups", h, {
      name: `e2e-audit-${RUN_ID}`,
    });
    const roles = (await (await page.request.get("/api/control-plane/access/roles")).json()) as Array<{
      id: string;
    }>;
    expect(roles.length, "at least one role exists").toBeGreaterThan(0);
    const assign = await page.request.post(`/api/control-plane/groups/${grp.id}/roles`, {
      headers: h,
      data: { roleId: roles[0].id },
    });
    expect(assign.ok(), `assign role → ${assign.status()} ${await assign.text()}`).toBeTruthy();

    // The audit log (UI) surfaces the role:assign entry. `emit` is fire-and-forget
    // (mpsc → a batching background writer), so the entry lands a few seconds later;
    // reload + refilter each retry so the query refetches until it appears.
    await expect(async () => {
      await page.goto("/control-plane/audit-log");
      await page.locator("#audit-action").fill("role:assign");
      await expect(page.getByText("role:assign").first()).toBeVisible({ timeout: 3_000 });
    }).toPass({ timeout: 30_000, intervals: [2_000] });

    // Cleanup.
    await page.request.delete(`/api/control-plane/groups/${grp.id}`, { headers: h }).catch(() => {});
  });
});
