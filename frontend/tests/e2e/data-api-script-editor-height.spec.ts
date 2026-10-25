import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * Regression for a layout defect `Query.test.tsx` cannot see: the statement
 * editor squeezed to nothing.
 *
 * Package S put two paragraphs of prose above the editor inside `#stmt-panel`
 * — one explaining that `build_sql` returns a TEMPLATE, one saying what
 * switching modes discards. Both are correct and both are wanted. But that
 * column is `flex-col` inside a `react-resizable-panels` `Panel` sized as a
 * FRACTION of what is left after the `Connection and parameters` disclosure, and
 * every line of fixed-height prose in it is subtracted from the one item that
 * takes the remainder: the editor.
 *
 * Measured live in Chrome, endpoint with two declared parameters, disclosure
 * expanded, before the fix:
 *
 *   viewport 1600x1300, written SQL ....... editor  100px  (180px at 5774bc77)
 *   viewport 1600x1300, script ............ editor   31px
 *   viewport 1600x1000, script ............ editor    5px, box bottom 880
 *                                           against `#stmt-panel` bottom 807,
 *                                           and `#stmt-panel` was
 *                                           `overflow-y: visible`, so the
 *                                           editor was laid out 73px past the
 *                                           panel that owns it.
 *
 * A five-pixel Monaco is not an editor. The fix is a floor on the editor's box
 * (`min-h-40`) and `overflow-y-auto` on `#stmt-panel` so the prose scrolls
 * instead of the editor paying for it.
 *
 * **Why this is not a jsdom test.** `Query.test.tsx` renders into jsdom, which
 * has no layout engine: `getBoundingClientRect()` returns all zeroes for every
 * element, so a test there reports the same `0` height whether the editor is
 * 5px or 500px, and `getComputedStyle(...).overflowY` never sees a Tailwind
 * class at all. A unit test asserting the editor is IN THE DOCUMENT passes
 * identically against the broken and the fixed rendering — the element was
 * always there; what was wrong was its size. See `docs/en/ui-conventions.md`
 * §10 and the siblings `data-api-statement-strip-overlap.spec.ts` and
 * `data-api-transform-refusal-legibility.spec.ts`, which exist for this reason.
 *
 * **Both modes, and the worst case for each.** The disclosure is expanded
 * throughout, because that is the state that produced the numbers above, and
 * the script mode is measured as well as the written one — script mode carries
 * the extra paragraph, so it is the mode that collapsed first and it is where a
 * future paragraph would collapse it again.
 *
 * No query ever runs: the endpoint's SQL exists only to be shown, so the
 * throwaway StarRocks connection does not have to answer — the same note the
 * sibling specs make.
 *
 *   EGRET_E2E_DATAAPI_STATEMENTS=1 npx playwright test data-api-script-editor-height
 */
const ENABLED = process.env.EGRET_E2E_DATAAPI_STATEMENTS === "1";

/**
 * The floor `Query.tsx` sets on the editor's box, in CSS pixels (`min-h-40`).
 *
 * Asserted as `>=` rather than `===`: at a tall window the editor is far bigger
 * than this and that is the point. The failure this guards against is the
 * editor being SMALLER than a usable box, which is the only direction the
 * squeeze can go.
 */
const FLOOR = 160;

/** Window heights: roomy, a common laptop, and a short one. */
const HEIGHTS = [1300, 1000, 800];

test.describe("Data API — the statement editor keeps a usable height", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DATAAPI_STATEMENTS=1 with the live dev stack up");
  test.setTimeout(180_000);

  test("neither editor collapses when Connection and parameters is expanded", async ({ page }) => {
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    let dbId: string | undefined;
    let endpointId: string | undefined;

    try {
      // A real row in `databases`, never queried — see the header.
      const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
        name: `e2e-editor-height-${runId}`,
        engine: "starrocks",
        host: "localhost",
        port: 9030,
        options: { user: "root" },
      });
      dbId = db.id as string;

      // Two parameters, because the disclosure's height is what steals the
      // editor's, and one parameter row is the smallest disclosure that is not
      // the empty-state sentence.
      const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
        name: "E2E script editor height",
        path: `e2e-editor-height-${runId}`,
        database_id: dbId,
        statements: [{ name: "main", sql: "SELECT 1 AS n\nWHERE region = {{ region }}" }],
        params: [
          { name: "region", type: "string", location: "query", required: false },
          { name: "d", type: "date", location: "query", required: false },
        ],
      });
      endpointId = endpoint.id as string;

      await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}`);

      // The disclosure is a native `<details>`; opening it is what shrinks the
      // panel the editor lives in.
      const strip = page.locator("summary", { hasText: "Connection and parameters" });
      await expect(strip).toBeVisible();
      await strip.click();

      const scripted = page.locator("#stmt-scripted");
      await expect(scripted).toBeVisible();

      /**
       * The editor's laid-out height, and whether its own panel can scroll.
       *
       * `getBoundingClientRect().height` on the `.monaco-editor` root, not the
       * wrapper: the wrapper is what carries the floor, so measuring it would
       * assert the class exists rather than that the browser honoured it.
       */
      const measure = () =>
        page.evaluate(() => {
          const panel = document.querySelector("#stmt-panel") as HTMLElement | null;
          const editor = document.querySelector(".monaco-editor") as HTMLElement | null;
          if (!panel || !editor) return null;
          return {
            editorHeight: Math.round(editor.getBoundingClientRect().height),
            panelOverflowY: getComputedStyle(panel).overflowY,
          };
        });

      for (const height of HEIGHTS) {
        await page.setViewportSize({ width: 1600, height });
        // Let `react-resizable-panels` re-measure and Monaco re-layout.
        await page.waitForTimeout(500);

        for (const mode of ["written SQL", "script"] as const) {
          if (mode === "script") {
            await scripted.check();
          } else {
            await scripted.uncheck();
          }
          await page.waitForTimeout(500);

          const m = await measure();
          expect(m, `${height}px, ${mode}: found #stmt-panel and the editor`).not.toBeNull();
          // The CONSEQUENCE first, deliberately. `overflow-y` below pins the
          // mechanism, but a spec whose first failure is a property name tells
          // the next reader which class went missing rather than what the
          // author would have seen; and a property assertion that runs first
          // can hide a height regression a different mechanism caused.
          expect(
            m!.editorHeight,
            `${height}px, ${mode}: editor laid out at ${m!.editorHeight}px with the ` +
              `Connection and parameters strip expanded`,
          ).toBeGreaterThanOrEqual(FLOOR);
          expect(
            m!.panelOverflowY,
            `${height}px, ${mode}: #stmt-panel must scroll its prose rather than let the ` +
              `editor absorb it, got overflow-y: ${m!.panelOverflowY}`,
          ).not.toBe("visible");
        }
      }
    } finally {
      // Best-effort: a failed assertion must not leave the shared `default`
      // workspace holding a throwaway endpoint or connection.
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
