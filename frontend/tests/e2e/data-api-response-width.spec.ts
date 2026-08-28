import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * Regression for a layout defect no jsdom test can see: the Response section
 * stacked four content widths and left a gutter nothing could ever use.
 *
 * Measured live in Chromium at 1600×1000, sidebar expanded, before the fix —
 * the numbers the 2026-08-28 audit reported, reproduced against the shipped
 * stylesheet:
 *
 *   content box ............................. 1296px
 *   section root (`max-w-5xl`) .............. 1024px, right edge 1288
 *   frozen `Name | Type` table .............. 576px, right edge  840
 *   `Setting | Details | Value` table ....... 1024px, right edge 1288
 *   the two block paragraphs ................ 532px, right edge  796
 *   → 288px of gutter, and a right edge at three different places.
 *
 * And inside the frozen table, the cap was not even buying what it was added
 * for: `w-full` AUTO layout gave `Name` 443.9px of 576, so the type started
 * 444px in. `table-fixed` + `w-72` puts it 288px in at any width.
 *
 * After: every FRAME — the section root, both tables — ends at the same right
 * edge, and the section takes the page like `Config` and `Query` do.
 *
 * **Prose is the one deliberate exception and is asserted as such**, not
 * excluded from the check: `max-w-prose` caps a sentence because a line of
 * running text has a readable measure of its own (`ui-conventions.md` §5.2,
 * "cap the prose, not the column"). A spec that only asserted "the frames
 * agree" would pass on a section that had widened the paragraphs too, which is
 * the obvious wrong fix.
 *
 * **Why this is not a jsdom test.** `EndpointDetail.test.tsx` pins the class
 * names, and that is all it can do: jsdom implements no CSS, so every box is
 * 0×0 and a `max-w-*` is a string. The same assertions pass against the broken
 * and the fixed rendering — what was wrong was the geometry the classes
 * produce. See `docs/en/ui-conventions.md` §10 and the sibling
 * `data-api-script-editor-height.spec.ts`, which exists for this reason.
 *
 * **Three viewports, because the section changes shape twice.** Above `lg` the
 * Details column is on; below it, `hidden lg:table-cell` removes it and the
 * `w-52 lg:w-96` Value column drops back to 208px on purpose (§5.3). 1600 and
 * 1280 are the same regime; 900 is the other one, and the check there is that
 * the change is a NO-OP — the old cap never bound below 1024 either.
 *
 *   EGRET_E2E_DATAAPI_WIDTH=1 npx playwright test data-api-response-width
 */
const ENABLED = process.env.EGRET_E2E_DATAAPI_WIDTH === "1";

/** Widths where the Details column is on, and one below `lg` where it is not. */
const WIDE = [1600, 1280];
const NARROW = 900;

test.describe("Data API — the Response section is on one measure", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DATAAPI_WIDTH=1 with the live dev stack up");
  test.setTimeout(180_000);

  test("every frame shares the page's right edge, and only the prose stops short", async ({ page }) => {
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    let dbId: string | undefined;
    let endpointId: string | undefined;

    try {
      // A real row in `databases`, never queried — the section only READS the
      // endpoint and its versions, so the throwaway connection never has to
      // answer. Same note as the sibling specs.
      const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
        name: `e2e-response-width-${runId}`,
        engine: "starrocks",
        host: "localhost",
        port: 9030,
        options: { user: "root" },
      });
      dbId = db.id as string;

      const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
        name: "E2E response width",
        path: `e2e-response-width-${runId}`,
        database_id: dbId,
        statements: [{ name: "main", sql: "SELECT 1 AS total_revenue" }],
      });
      endpointId = endpoint.id as string;

      await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}/response`);
      await expect(page.getByRole("heading", { name: "Response", level: 2 })).toBeVisible();

      /**
       * The right edge of each frame, of the two paragraphs, and of the space
       * the section is given.
       *
       * Right EDGES rather than widths: "one measure" is a claim about where
       * things end, and two blocks with equal widths at different offsets
       * would satisfy a width check while still reading as ragged.
       */
      const measure = () =>
        page.evaluate(() => {
          const round = (n: number) => Math.round(n);
          const right = (el: Element | null | undefined) =>
            el ? round(el.getBoundingClientRect().right) : null;
          const heading = [...document.querySelectorAll("h2")].find(
            (h2) => h2.textContent?.trim() === "Response",
          );
          if (!heading) return null;
          // The section's own root, walked up from its heading — never
          // `querySelector`, so an interposed wrapper breaks the walk instead
          // of being silently walked past.
          const root = heading.parentElement?.parentElement as HTMLElement | undefined;
          if (!root) return null;
          const host = root.parentElement as HTMLElement;
          const hostStyle = getComputedStyle(host);
          const tables = [...root.querySelectorAll("table")];
          const settings = tables.find(
            (t) => t.querySelector("th")?.textContent?.trim() === "Setting",
          );
          const frozen = tables.find(
            (t) => t.querySelector("th")?.textContent?.trim() === "Name",
          );
          const nameHead = frozen?.querySelector("th");
          const typeHead = frozen?.querySelectorAll("th")[1];
          const paragraphs = [...root.querySelectorAll(":scope > div > p.max-w-prose")];
          return {
            available: round(host.getBoundingClientRect().right - parseFloat(hostStyle.paddingRight)),
            root: right(root),
            settingsTable: right(settings),
            frozenTable: right(frozen),
            // How far into the frozen table the type sits — the thing the
            // removed `max-w-xl` was supposed to be keeping small.
            typeOffset:
              typeHead && frozen
                ? round(
                    typeHead.getBoundingClientRect().left - frozen.getBoundingClientRect().left,
                  )
                : null,
            nameWidth: nameHead ? round(nameHead.getBoundingClientRect().width) : null,
            valueWidth: settings
              ? round(
                  ([...settings.querySelectorAll("th")].find(
                    (th) => th.textContent?.trim() === "Value",
                  )?.getBoundingClientRect().width ?? 0),
                )
              : null,
            paragraphRights: paragraphs.map((p) => right(p)),
          };
        });

      for (const width of WIDE) {
        await page.setViewportSize({ width, height: 1000 });
        await page.waitForTimeout(300);
        const m = await measure();
        expect(m, `${width}px: found the Response section`).not.toBeNull();

        // One measure: the section takes what it is given, and both tables end
        // where it does. `toBeCloseTo`-style slack is deliberately NOT used —
        // these are the same box model, so they are equal or the claim is
        // false.
        expect(m!.root, `${width}px: the section leaves no gutter`).toBe(m!.available);
        expect(m!.settingsTable, `${width}px: the settings table's right edge`).toBe(m!.root);
        expect(m!.frozenTable, `${width}px: the frozen list's right edge`).toBe(m!.root);

        // The exception, asserted rather than skipped.
        for (const r of m!.paragraphRights) {
          expect(r, `${width}px: prose keeps its own reading measure`).toBeLessThan(m!.root!);
        }

        // The frozen list's own grid: `Name` is the settings table's first
        // column, so the type sits beside the name at any width.
        expect(m!.nameWidth, `${width}px: Name is w-72`).toBe(288);
        expect(m!.typeOffset, `${width}px: the type sits beside the name`).toBe(288);
        // `w-96` above `lg` — the other half of removing the cap (§5.2).
        expect(m!.valueWidth, `${width}px: Value took the width, not Details`).toBe(384);
      }

      // Below `lg`: the widening must NOT apply. The old cap did not bind here
      // either, so this regime is unchanged by the whole change — which is the
      // claim, and it is only checkable in a browser.
      await page.setViewportSize({ width: NARROW, height: 1000 });
      await page.waitForTimeout(300);
      const narrow = await measure();
      expect(narrow, `${NARROW}px: found the Response section`).not.toBeNull();
      expect(narrow!.valueWidth, `${NARROW}px: Value stays at the shared w-52`).toBe(208);
      expect(narrow!.root, `${NARROW}px: still no gutter`).toBe(narrow!.available);
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
