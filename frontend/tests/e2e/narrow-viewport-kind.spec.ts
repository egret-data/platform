import { test, expect } from "@playwright/test";
import { loginLocalAdmin } from "./helpers";

/**
 * W1 acceptance: at a phone viewport the dashboard page must not scroll sideways, and every
 * header action must stay reachable.
 *
 * Opt-in: EGRET_E2E_KIND=1 against the in-cluster kind deploy. NOT part of `npm test` — jsdom
 * has no layout engine, so `scrollWidth` there is always 0 and this assertion is meaningless
 * outside a real browser.
 *
 *   EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://localhost:30080 npx playwright test narrow-viewport
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const DASHBOARD =
  process.env.EGRET_E2E_DASHBOARD ?? "/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb";

/** Elements sticking out past the viewport, minus any with a scroll-clipping ancestor — those
 *  scroll internally and cannot drive the document's own scrollWidth. */
async function overflowReport(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    const clipped = (el: Element) => {
      for (let p = el.parentElement; p; p = p.parentElement) {
        const ox = getComputedStyle(p).overflowX;
        if (ox === "hidden" || ox === "auto" || ox === "scroll") return true;
      }
      return false;
    };
    const offenders: { tag: string; label: string | null; text: string; right: number }[] = [];
    for (const el of Array.from(document.querySelectorAll("*"))) {
      const r = el.getBoundingClientRect();
      if (r.width > 0 && r.right > window.innerWidth + 0.5 && !clipped(el)) {
        offenders.push({
          tag: el.tagName.toLowerCase(),
          label: el.getAttribute("aria-label"),
          text: (el.textContent ?? "").trim().slice(0, 32),
          right: Math.round(r.right),
        });
      }
    }
    return {
      vw: window.innerWidth,
      scrollW: document.documentElement.scrollWidth,
      offenders: offenders.slice(0, 10),
    };
  });
}

test.describe("dashboard at a phone viewport", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.setTimeout(90_000);

  test("the document does not scroll sideways at 390px", async ({ page }) => {
    await loginLocalAdmin(page);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(DASHBOARD);
    await page.waitForSelector(".react-grid-layout");

    const report = await overflowReport(page);
    // Reported rather than just asserted: when this fails, the offender list is the whole
    // diagnosis, and a bare `expect(scrollW).toBe(vw)` throws it away.
    expect(report.offenders, JSON.stringify(report.offenders)).toEqual([]);
    expect(report.scrollW).toBe(report.vw);
  });

  test("every dashboard header action stays inside the viewport at 390px", async ({ page }) => {
    // Clipping the row instead of wrapping it would satisfy the scrollWidth check above while
    // leaving Fullscreen unreachable — neither visible nor scrollable-to. This is the check
    // that tells those two outcomes apart.
    await loginLocalAdmin(page);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(DASHBOARD);
    await page.waitForSelector(".react-grid-layout");

    for (const label of ["add bookmark", "remove bookmark", "auto-refresh", "fullscreen", "dashboard info"]) {
      const el = page.locator(`[aria-label="${label}"]`);
      if ((await el.count()) === 0) continue; // bookmark renders under one of two labels
      await expect(el.first()).toBeVisible();
      const box = await el.first().boundingBox();
      expect(box, `${label} has no box`).not.toBeNull();
      expect(box!.x + box!.width, `${label} runs past the right edge`).toBeLessThanOrEqual(390.5);
    }
  });

  test("the desktop viewport is unchanged", async ({ page }) => {
    await loginLocalAdmin(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(DASHBOARD);
    await page.waitForSelector(".react-grid-layout");

    const report = await overflowReport(page);
    expect(report.scrollW).toBe(report.vw);
    // The wordmark and the full "Control Plane" label are desktop-only affordances; their
    // presence here is what proves the narrow rule did not leak upward.
    await expect(page.getByRole("link", { name: "Control Plane" })).toBeVisible();
    await expect(page.locator('[data-testid="app-header"]').getByText("Egret", { exact: true })).toBeVisible();
  });
});
