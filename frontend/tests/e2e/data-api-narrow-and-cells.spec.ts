import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * Two layout defects the 2026-08-29 live pass found, neither of which any jsdom
 * test can see and neither of which the 2026-08-28 audit could have: the first
 * needs a real stylesheet to compute an overflow, and the second needs a
 * viewport narrow enough to wrap a flex row.
 *
 * **1. A `Caveat` inside a table cell overflowed its column.**
 * The shared `TableCell` is `whitespace-nowrap`, and `TryIt` puts a `Caveat` in
 * the Service-account row's Value cell. Measured live in Chromium at 1440×1100
 * before the fix: the 64-character summary laid out at 388px inside a 320px
 * cell, so the sentence was clipped mid-phrase on screen ("…does not prove a
 * consumer can call" and then nothing) AND the whole
 * `Setting | Details | Value` table overflowed its wrapper — scrollWidth 1092
 * against clientWidth 1016 — putting a horizontal scrollbar under a settings
 * table on a desktop viewport. `Caveat.tsx` now sets `whitespace-normal` on the
 * summary and `min-w-0` on its text.
 *
 * **2. The Endpoints toolbar divider could end a wrapped row.**
 * `ListPageShell`'s action group is `flex flex-wrap`, and the `role="separator"`
 * hairline and the Trash button were siblings in it. Measured live at 320×900
 * before the fix: the first row ended
 * `[bookmarks] [bulk] [new folder] |` with the 1px rule hanging off the end and
 * Trash alone on the next line — a divider with nothing to its right, which
 * reads as a stray mark rather than as a boundary. They are one flex item now,
 * so no width can split them.
 *
 * Both are geometry, and geometry is the one thing `Endpoints.test.tsx` and
 * `TryIt.test.tsx` cannot assert: jsdom implements no CSS, so every box is 0×0
 * and `whitespace-nowrap` is a string. Same reason as the sibling
 * `data-api-response-width.spec.ts`.
 *
 * ONE test, not two, and deliberately: `loginLocalAdmin` is a real login and
 * the local login route is rate-limited to five attempts per five minutes, so a
 * spec file that logs in per test spends that budget on setup.
 *
 *   EGRET_E2E_DATAAPI_NARROW=1 npx playwright test data-api-narrow-and-cells
 */
const ENABLED = process.env.EGRET_E2E_DATAAPI_NARROW === "1";

/** The summary this spec measures. Asserted, so a copy change cannot make the
 *  selector match nothing and the test pass over an empty list. */
const CAVEAT = "A green preview does not prove a consumer can call this endpoint";

test.describe("Data API — a caveat fits its cell, and a divider keeps its neighbour", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DATAAPI_NARROW=1 with the live dev stack up");
  test.setTimeout(180_000);

  test("no settings table scrolls sideways, and no wrap orphans the hairline", async ({ page }) => {
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    let dbId: string | undefined;
    let endpointId: string | undefined;

    try {
      // A real row in `databases`, never queried — both screens only READ, so
      // the throwaway connection never has to answer. Same note as the sibling
      // specs.
      const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
        name: `e2e-narrow-${runId}`,
        engine: "starrocks",
        host: "localhost",
        port: 9030,
        options: { user: "root" },
      });
      dbId = db.id as string;

      const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
        name: "E2E narrow cells",
        path: `e2e-narrow-${runId}`,
        database_id: dbId,
        statements: [{ name: "main", sql: "SELECT 1 AS total" }],
      });
      endpointId = endpoint.id as string;

      // ── 1. The caveat in the Value cell ───────────────────────────────────
      await page.setViewportSize({ width: 1440, height: 1100 });
      await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}/try`);
      await expect(page.getByRole("heading", { name: "Try it", level: 2, exact: true })).toBeVisible();

      const cell = await page.evaluate((caveatText) => {
        const summary = [...document.querySelectorAll("summary")].find(
          (s) => s.textContent?.trim() === caveatText,
        );
        if (!summary) return null;
        const cellEl = summary.closest("td");
        // The `overflow-x-auto` wrapper the shared `Table` renders around every
        // table. If the table is wider than this, a settings table scrolls.
        const table = summary.closest("table");
        const wrapper = table?.parentElement ?? null;
        return {
          summaryScroll: summary.scrollWidth,
          summaryClient: summary.clientWidth,
          cellWidth: cellEl ? Math.round(cellEl.getBoundingClientRect().width) : null,
          wrapperScroll: wrapper?.scrollWidth ?? null,
          wrapperClient: wrapper?.clientWidth ?? null,
        };
      }, CAVEAT);

      // Found, not merely searched for: a renamed summary would otherwise make
      // every assertion below vacuous.
      expect(cell, `the "${CAVEAT}" caveat is on the Try it screen`).not.toBeNull();

      // The sentence is not clipped: `text-overflow` is `clip` on a `<summary>`,
      // so an overflow here is text a reader simply cannot see — there is no
      // ellipsis to tell them something is missing.
      expect(cell!.summaryScroll, "the caveat lays out inside its own box").toBeLessThanOrEqual(
        cell!.summaryClient,
      );

      // And the table it sits in does not scroll sideways because of it.
      expect(cell!.wrapperScroll, "the settings table does not overflow its wrapper").toBe(
        cell!.wrapperClient,
      );

      // ── 2. The toolbar divider at a wrapping width ────────────────────────
      // 320px is the narrowest viewport Chromium will take and the width at
      // which the action group is forced below its natural width — measured, it
      // is where the group falls to 176px and the row breaks after the third
      // icon button. Above roughly 360px the group does not wrap this far and
      // the assertion would hold trivially, which is what the wrap guard below
      // exists to catch.
      await page.setViewportSize({ width: 320, height: 900 });
      await page.goto(`/explore/${ws.slug}/data-api/endpoints`);
      await expect(page.getByRole("heading", { name: "Endpoints", level: 1 })).toBeVisible();
      // Wait for a control that renders only on `hasData`, NOT for Trash: Trash
      // is deliberately ungated, so it is on screen while the list is still
      // loading — and so is the `h1`. Measuring then finds no divider and the
      // "is rendered" guard fires for a timing reason that has nothing to do
      // with the layout. That is not hypothetical: it happened on the first
      // mutation run of this spec, which reported the guard instead of the
      // claim. Bookmarks shares the divider's exact gate.
      await expect(page.getByRole("button", { name: "Bookmarks" })).toBeVisible();

      const toolbar = await page.evaluate(() => {
        const centre = (el: Element | null) =>
          el ? Math.round(el.getBoundingClientRect().top + el.getBoundingClientRect().height / 2) : null;
        const sep = document.querySelector('[role="separator"][aria-orientation="vertical"]');
        const trash = document.querySelector('button[aria-label="Trash"]');
        const bookmarks = document.querySelector('button[aria-label="Bookmarks"]');
        const create = [...document.querySelectorAll("a")].find(
          (a) => a.textContent?.trim() === "+ Endpoint",
        );
        return {
          sep: centre(sep),
          trash: centre(trash),
          bookmarks: centre(bookmarks),
          create: centre(create ?? null),
        };
      });

      // The divider only renders beside the three filter controls, so its
      // absence would make the rest of this vacuous.
      expect(toolbar.sep, "the toolbar divider is rendered").not.toBeNull();
      expect(toolbar.trash, "the Trash button is rendered").not.toBeNull();

      // The guard: this width really does wrap the group. If a future layout
      // stops wrapping here, the divider check below would pass for a reason
      // that has nothing to do with the fix, so say so loudly instead.
      expect(
        toolbar.create,
        "320px still wraps the action group (otherwise this check is vacuous)",
      ).not.toBe(toolbar.bookmarks);

      // The claim: whatever the wrap does, the hairline keeps the control it
      // divides. Vertical centres, not `top` — the divider is `h-5` inside a
      // 36px row, so its top never matched the button's even before the wrap.
      expect(
        toolbar.sep,
        "the divider stays on the same line as the control it divides",
      ).toBe(toolbar.trash);
    } finally {
      // Best-effort: a failed assertion must not leave the shared workspace
      // holding a throwaway endpoint or connection.
      if (endpointId) {
        await page.request
          .delete(`/api/ws/${ws.slug}/data-api/endpoints/${endpointId}`, { headers: h })
          .catch(() => {});
      }
      if (dbId) {
        await page.request.delete(`/api/ws/${ws.slug}/databases/${dbId}`, { headers: h }).catch(() => {});
      }
    }
  });
});
