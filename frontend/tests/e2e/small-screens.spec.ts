import { test, expect, type Page } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace } from "./helpers";

/**
 * Small screens, measured (KI-233, KI-234; `docs/en/frontend-rules.md`
 * FE-PAGE-10), and the refusal inside the shell (KI-230, FE-STATE-5).
 *
 * Modelled on `page-frame.spec.ts`: a browser spec because jsdom has no
 * layout. A class assertion says a table container can scroll, not that the
 * column past the edge has a cue a reader can see.
 *
 *   width  page                         claim
 *   768    Explore → Datasets           the sidebar is the 64px rail; no sideways document scroll
 *   390    Explore → Dashboards         a table wider than its room shades its clipped edge
 *   390    Alerts → New alert           every section tab is inside the viewport (the strip wraps)
 *   390    Workspace settings           the section strip fits, or shades its clipped edge
 *   390    any Explore page             the workspace picker shows the whole workspace name
 *
 * Before the change, on the demo stack: the sidebar was 224px at 768px, the
 * dashboards table ran past the edge with no cue, the alert editor's
 * Notification tab sat past the right edge, and the picker showed "De" for
 * "Default".
 *
 *   EGRET_E2E_SMALL_SCREENS=1 npx playwright test small-screens
 *
 * `EGRET_E2E_STORAGE_STATE=<file>` reuses a signed-in admin session (the shared
 * demo stack limits local sign-ins). `EGRET_E2E_MEMBER_STORAGE_STATE=<file>`,
 * a session of a workspace member with no Control Plane permission, adds the
 * refusal check.
 */
const ENABLED = process.env.EGRET_E2E_SMALL_SCREENS === "1";
const STORAGE_STATE = process.env.EGRET_E2E_STORAGE_STATE;
const MEMBER_STATE = process.env.EGRET_E2E_MEMBER_STORAGE_STATE;

/** How far the document scrolls sideways: 0 when nothing pushes it wider than the window. */
const documentOverflow = (page: Page) =>
  page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);

const settle = async (page: Page, title: string) => {
  await expect(page.getByRole("heading", { level: 1, name: title, exact: true })).toBeVisible({ timeout: 30_000 });
  await page.waitForTimeout(400);
};

test.describe("Small screens — the rail, scrolling tables, wrapping tabs", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_SMALL_SCREENS=1 with a live stack up");
  test.use(STORAGE_STATE ? { storageState: STORAGE_STATE } : {});
  test.setTimeout(180_000);

  test("768px: the sidebar is the icon rail and the document does not scroll sideways", async ({ page }) => {
    await page.setViewportSize({ width: 768, height: 1024 });
    if (!STORAGE_STATE) await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    await page.goto(`/explore/${ws.slug}/datasets`);
    await settle(page, "Datasets");

    const rail = await page.getByTestId("app-sidebar").evaluate((el) => el.getBoundingClientRect().width);
    expect(rail, "sidebar width at 768px").toBeLessThanOrEqual(64);
    // Forced, so there is nothing to toggle.
    await expect(page.getByRole("button", { name: "Toggle sidebar" })).toHaveCount(0);
    expect(await documentOverflow(page), "document overflow at 768px").toBeLessThanOrEqual(0);
  });

  test("390px: tables shade the clipped edge, tabs stay in view, the picker shows the name", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    if (!STORAGE_STATE) await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);

    // Dashboards: the table is wider than 390px of room. It scrolls inside its
    // container, and the shade on the right edge says so.
    await page.goto(`/explore/${ws.slug}/dashboards`);
    await settle(page, "Dashboards");
    const table = await page.locator("[data-slot=table-container]").first().evaluate((el) => ({
      scrolls: el.scrollWidth > el.clientWidth + 1,
      cue: !!el.parentElement?.querySelector("[data-scroll-cue=end]"),
    }));
    expect(table.scrolls, "the dashboards table is wider than its room at 390px").toBe(true);
    expect(table.cue, "a shade marks the clipped right edge").toBe(true);
    expect(await documentOverflow(page), "document overflow on Dashboards").toBeLessThanOrEqual(0);

    // The picker: the whole workspace name, not "De".
    const picker = await page.getByRole("combobox", { name: "Workspace" }).evaluate((el) => {
      const value = el.querySelector("span") ?? el;
      return { text: value.textContent?.trim() ?? "", clipped: value.scrollWidth > value.clientWidth + 1 };
    });
    expect(picker.text.length, "the picker shows a name").toBeGreaterThan(2);
    expect(picker.clipped, `the picker's "${picker.text}" is not cut`).toBe(false);

    // The alert editor: every section tab inside the viewport.
    await page.goto(`/explore/${ws.slug}/alerts/new`);
    await settle(page, "New alert");
    const tabs = await page.getByRole("tab").evaluateAll((els) =>
      els.map((el) => ({ name: el.textContent?.trim() ?? "", right: el.getBoundingClientRect().right })),
    );
    expect(tabs.length, "the alert editor has section tabs").toBeGreaterThan(3);
    const vw = page.viewportSize()!.width;
    expect(tabs.filter((t) => t.right > vw + 0.5).map((t) => t.name), "tabs past the right edge").toEqual([]);

    // Workspace settings: the section strip fits, or shades its clipped edge.
    await page.goto(`/explore/${ws.slug}/settings/general`);
    await settle(page, "Workspace settings");
    const strip = await page
      .getByRole("navigation", { name: "Workspace settings sections" })
      .first()
      .evaluate((el) => ({
        overflows: el.scrollWidth > el.clientWidth + 1,
        cue: !!el.parentElement?.querySelector("[data-scroll-cue]"),
      }));
    expect(!strip.overflows || strip.cue, "the settings strip fits or shows a shade").toBe(true);
    expect(await documentOverflow(page), "document overflow on Workspace settings").toBeLessThanOrEqual(0);
  });
});

test.describe("Refusal inside the shell", () => {
  test.skip(!ENABLED || !MEMBER_STATE, "opt-in: EGRET_E2E_SMALL_SCREENS=1 and EGRET_E2E_MEMBER_STORAGE_STATE");
  test.use(MEMBER_STATE ? { storageState: MEMBER_STATE } : {});

  for (const width of [390, 768]) {
    test(`${width}px: a member refused Control Plane keeps the header and gets a way out`, async ({ page }) => {
      await page.setViewportSize({ width, height: 844 });
      await page.goto("/control-plane");
      await settle(page, "You don't have access to this page.");
      await expect(page.getByTestId("app-header")).toBeVisible();
      await expect(page.getByRole("link", { name: /^Open / })).toBeVisible();
      expect(await documentOverflow(page), `document overflow at ${width}px`).toBeLessThanOrEqual(0);
    });
  }
});
