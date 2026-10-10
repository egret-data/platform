import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * Regression for a layout defect `Statements.test.tsx` cannot see.
 *
 * `Query.tsx`'s statement tab strip and its "Add statement" button used to share
 * one `overflow-x-auto` flex row. The OUTER row clipped/scrolled; the `[role=tablist]`
 * strip inside it did not, so once enough statements pushed the strip past the
 * button's start, the strip's own children painted outside the strip's box and over
 * the button — invisibly to jsdom, which has no layout engine at all (see
 * `docs/en/ui-conventions.md` §10, "Reading the code does not settle layout"). The
 * existing jsdom test (`Statements.test.tsx`, "stops offering a new statement at
 * the limit, and says why") asserts `toHaveAttribute("disabled")` and the `title`
 * string, and it passed against the broken layout — it has no way to know the
 * element carrying that title was unreachable on screen.
 *
 * Measured live in Chrome at 1600px with 16 statements before the fix:
 * `tablist.clientWidth` 1147 vs `scrollWidth` 1620, and `document.elementFromPoint`
 * at the button's own centre returned `<span role="tab">statement_13</span>`, not
 * the button — at every `scrollLeft` tried, because the button scrolled together
 * with the strip that was covering it.
 *
 * This spec is the only place in the tree that can catch a regression here: it
 * drives a real browser (Playwright, not jsdom) so the browser's actual layout and
 * hit-testing run, and it asserts the exact thing that was wrong —
 * `elementFromPoint` at the button's centre must resolve to the button — at four
 * viewport widths and at both ends of the strip's scroll range. A test that only
 * checked `toBeVisible()` or the `disabled`/`title` attributes (the jsdom shape)
 * would pass whether or not the button is actually reachable, exactly like the
 * pre-existing unit test did; this one does not.
 *
 * Opt-in against the plain local dev stack (docker compose + `just serve` +
 * `npm run dev` — see README.md), NOT the in-cluster kind deploy, matching
 * `workspace-features.spec.ts` / `embedded-catalog.spec.ts`. It provisions its own
 * throwaway StarRocks connection and endpoint (no assigned engine is required in
 * the seeded workspace, unlike `runSqlInLab`'s SQL Lab path) and deletes both in a
 * `finally`, because the suite's `default` workspace is shared with other specs.
 * No query ever runs against the connection — the endpoint's statements exist only
 * to populate the tab strip — so StarRocks reachability is not a precondition.
 *
 *   EGRET_E2E_DATAAPI_STATEMENTS=1 npx playwright test data-api-statement-strip-overlap
 */
const ENABLED = process.env.EGRET_E2E_DATAAPI_STATEMENTS === "1";

const VIEWPORTS = [1280, 1440, 1600, 1920];

test.describe("Data API — the statement strip never covers Add statement", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DATAAPI_STATEMENTS=1 with the live dev stack up");
  test.setTimeout(120_000);

  test("elementFromPoint at the button's centre returns the button, at every width and scroll end", async ({
    page,
  }) => {
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    let dbId: string | undefined;
    let endpointId: string | undefined;

    try {
      // A real row in `databases`, not a working connection — nothing here ever
      // queries it, so StarRocks does not need to actually answer.
      const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
        name: `e2e-overlap-${runId}`,
        engine: "starrocks",
        host: "localhost",
        port: 9030,
        options: { user: "root" },
      });
      dbId = db.id as string;

      // MAX_STATEMENTS (crates/egret-dataapi-api/src/lib.rs) — the cap the "Add
      // statement" button hits, which is what gives it a `title` to carry.
      const statements = Array.from({ length: 16 }, (_, i) => ({
        name: `statement_${i + 1}`,
        sql: "SELECT 1",
      }));
      const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
        name: "E2E overlap regression",
        path: `e2e-overlap-regression-${runId}`,
        database_id: dbId,
        statements,
      });
      endpointId = endpoint.id as string;

      await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}`);
      const tablist = page.getByRole("tablist", { name: "Statements" });
      await expect(tablist).toBeVisible();
      await expect(tablist.getByRole("tab")).toHaveCount(16);

      for (const width of VIEWPORTS) {
        await page.setViewportSize({ width, height: 900 });

        const result = await page.evaluate(() => {
          const btn = Array.from(document.querySelectorAll("button")).find((b) =>
            (b.textContent ?? "").trim().startsWith("Add statement"),
          ) as HTMLButtonElement | undefined;
          const strip = document.querySelector('[role="tablist"]') as HTMLElement | null;
          if (!btn || !strip) return { atZero: false, atMax: false, missing: true };

          const hitsButton = (scrollLeft: number) => {
            strip.scrollLeft = scrollLeft;
            const r = btn.getBoundingClientRect();
            const el = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
            return el === btn;
          };

          const atZero = hitsButton(0);
          const atMax = hitsButton(strip.scrollWidth);
          strip.scrollLeft = 0;
          return { atZero, atMax, missing: false };
        });

        expect(result.missing, `${width}px: found both the strip and the button`).toBe(false);
        expect(result.atZero, `${width}px, scrollLeft=0: elementFromPoint at the button's centre is the button`).toBe(
          true,
        );
        expect(
          result.atMax,
          `${width}px, scrollLeft=max: elementFromPoint at the button's centre is the button`,
        ).toBe(true);
      }
    } finally {
      // Best-effort, matching `deleteEngine`'s idiom elsewhere in this suite — a
      // failed assertion above must not leave the shared `default` workspace
      // holding a throwaway endpoint or connection.
      if (endpointId) {
        await page.request.delete(`/api/ws/${ws.slug}/data-api/endpoints/${endpointId}`, { headers: h }).catch(() => {});
      }
      if (dbId) {
        await page.request.delete(`/api/ws/${ws.slug}/databases/${dbId}`, { headers: h }).catch(() => {});
      }
    }
  });
});
