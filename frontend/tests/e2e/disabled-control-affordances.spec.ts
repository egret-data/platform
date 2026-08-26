import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * Regression for a defect no jsdom test can see: a DISABLED button that carried
 * the only sentence explaining why it was disabled, and could not be pointed at.
 *
 * `ui/button.tsx` used to set `disabled:cursor-not-allowed` and
 * `disabled:pointer-events-none` in one class string, in that order. The second
 * cancels the first. `pointer-events: none` removes the element from HIT-TESTING,
 * and hit-testing is what decides all three of: which element the `cursor`
 * applies to, which element's `title` the browser opens as a tooltip, and which
 * element matches `:hover`. So the class made the `title` unreachable — on
 * sixteen buttons in the tree that pass a `title` which exists only while they are
 * disabled, and on icon-only buttons that `title` is the ONLY label there is.
 *
 * Measured in Chrome on this branch before the fix, on the Config section's
 * disabled "Validate saved draft" (in viewport, 149x32 at y=622 of a 727px
 * viewport, so not an off-screen artifact):
 *   - `getComputedStyle(btn).pointerEvents` -> "none"
 *   - `document.elementFromPoint` at its own centre -> the wrapping `<div>`,
 *     and the button was absent from `elementsFromPoint` entirely
 *   - Playwright's own `hover()` -> TimeoutError, "element is visible and
 *     stable ... <div class="flex items-center gap-2">…</div> intercepts
 *     pointer events"
 * and on the Query section, disabled "Move up" hit-tested to the container of a
 * DIFFERENT control ("Remove statement") — the pointer did not merely miss, it
 * landed somewhere else.
 *
 * # Why this cannot live in the Vitest suite
 *
 * jsdom has no layout engine and no hit-testing (`docs/en/ui-conventions.md`
 * §10). `getBoundingClientRect` is all zeros and `elementFromPoint` is not
 * meaningfully implemented, so nothing there can distinguish "reachable" from
 * "painted over". A jsdom test asserting `toHaveAttribute("title")` and
 * `toBeDisabled()` passed against the broken build and still passes now.
 *
 * # Two of these assertions would be green for the wrong reason, and are labelled
 *
 * `cursor` is deliberately NOT asserted. `getComputedStyle(btn).cursor` returned
 * `"not-allowed"` BEFORE the fix as well — the class was applied, it simply never
 * rendered, because the pointer never resolved onto the element. An assertion on
 * the computed cursor would therefore pass either way and prove nothing; the
 * honest instrument for "the cursor renders" is the same one as for "the tooltip
 * opens", namely whether hit-testing lands on the button. That is what is
 * asserted.
 *
 * The "Add statement" and "stays inert" checks below are GUARDS, not regression
 * assertions: both already held before the fix (`Add statement` had a local
 * `disabled:pointer-events-auto` override, since removed, and `disabled` has
 * always blocked activation). They exist so the fix cannot go too far — the
 * failure mode of "make it hoverable" is "make it clickable".
 *
 * Opt-in against the plain local dev stack (docker compose + `just serve` +
 * `npm run dev` — see README.md), matching `data-api-statement-strip-overlap.spec.ts`.
 * It provisions its own throwaway connection and endpoint and removes both in a
 * `finally`, because the `default` workspace is shared with other specs. No query
 * ever runs against the connection, so StarRocks reachability is not a
 * precondition. One login for the whole file: the local login route is rate
 * limited to 5 attempts per 5 minutes.
 *
 *   EGRET_E2E_DISABLED_AFFORDANCE=1 npx playwright test disabled-control-affordances
 */
const ENABLED = process.env.EGRET_E2E_DISABLED_AFFORDANCE === "1";

test.describe("A disabled control can still be pointed at", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DISABLED_AFFORDANCE=1 with the live dev stack up");
  test.setTimeout(120_000);

  test("the disabled button carrying the explanation is the element under the pointer", async ({ page }) => {
    // Leaving a dirtied form raises `beforeunload`; accept it rather than let a
    // navigation below hang on a modal.
    page.on("dialog", (d) => void d.accept());

    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    let dbId: string | undefined;
    let endpointId: string | undefined;

    try {
      const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
        name: `e2e-affordance-${runId}`,
        engine: "starrocks",
        host: "localhost",
        port: 9030,
        options: { user: "root" },
      });
      dbId = db.id as string;

      // MAX_STATEMENTS (`routes/explore/dataapi/detail/Query.tsx`) — a full list is
      // what disables "Add statement" and gives it its `title`.
      const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
        name: "E2E disabled affordance",
        path: `e2e-disabled-affordance-${runId}`,
        database_id: dbId,
        statements: Array.from({ length: 16 }, (_, i) => ({ name: `statement_${i + 1}`, sql: "SELECT 1" })),
      });
      endpointId = endpoint.id as string;

      const base = `/explore/${ws.slug}/data-api/endpoints/${endpointId}`;

      // ---- The regression assertion -------------------------------------------
      // Config's "Validate saved draft" is disabled while the form is dirty, and
      // its `title` is the only thing that says so. It never had a local override,
      // so it is the honest witness: this block FAILS on the pre-fix code.
      await page.goto(`${base}/config`);
      await page.getByRole("textbox", { name: "Description" }).fill(`dirty ${runId}`);

      const validate = page.locator('button[title^="Save your changes before validating"]');
      await expect(validate).toBeDisabled();
      await validate.scrollIntoViewIfNeeded();

      const reach = await validate.evaluate((btn: HTMLButtonElement) => {
        const r = btn.getBoundingClientRect();
        const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
        return {
          // Guards the measurement itself: "not under the pointer" must mean
          // pointer-events, not "scrolled off screen".
          inViewport: r.y >= 0 && r.y + r.height <= window.innerHeight && r.width > 0,
          hitIsButton: hit === btn || btn.contains(hit),
        };
      });
      expect(reach.inViewport, "the disabled Validate button is fully in the viewport when measured").toBe(true);
      expect(
        reach.hitIsButton,
        "elementFromPoint at the disabled Validate button's centre resolves to the button (pre-fix: its wrapping div)",
      ).toBe(true);

      // A real hover, which is what actually opens a `title` tooltip. Pre-fix this
      // threw TimeoutError("...intercepts pointer events") instead.
      await validate.hover();
      const hovered = await page.evaluate(() => {
        const deepest = document.querySelectorAll(":hover");
        const el = deepest[deepest.length - 1] as HTMLElement | undefined;
        return { tag: el?.tagName ?? "none", title: el?.getAttribute("title") ?? "" };
      });
      expect(hovered.tag, "the deepest :hover element is the button itself").toBe("BUTTON");
      expect(
        hovered.title,
        "and it is the element carrying the explanation, so the browser has a tooltip to open",
      ).toContain("Save your changes before validating");

      // ---- Guards: the fix must not have gone further -------------------------
      // Held before the fix too (see the header). `disabled` blocks activation;
      // only `pointer-events` was ever removed.
      await page.evaluate(() => {
        const btn = document.querySelector('button[title^="Save your changes before validating"]') as HTMLButtonElement;
        (window as unknown as { __clicks: number }).__clicks = 0;
        btn.addEventListener("click", () => {
          (window as unknown as { __clicks: number }).__clicks += 1;
        });
      });
      // A real mouse press at the button's centre — not `locator.click()` (which
      // would wait for the button to become enabled) and not a synthetic
      // `dispatchEvent` (which would bypass the browser's own decision).
      const box = await validate.boundingBox();
      await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);
      const afterClick = await page.evaluate(() => ({
        clicks: (window as unknown as { __clicks: number }).__clicks,
        disabled: (document.querySelector('button[title^="Save your changes before validating"]') as HTMLButtonElement)
          .disabled,
      }));
      expect(afterClick.clicks, "a real mouse click at its centre fires no click event").toBe(0);
      expect(afterClick.disabled, "and it is still disabled afterwards").toBe(true);

      // The Data API button the local `disabled:pointer-events-auto` override used
      // to serve. The override is gone; the shared component must now do its job.
      await page.goto(base);
      const addStatement = page.getByRole("button", { name: "Add statement" });
      await expect(addStatement).toBeDisabled();
      const addReach = await addStatement.evaluate((btn: HTMLButtonElement) => {
        const r = btn.getBoundingClientRect();
        const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
        return { hitIsButton: hit === btn || btn.contains(hit), hasTitle: !!btn.getAttribute("title") };
      });
      expect(addReach.hasTitle, "Add statement still explains the cap").toBe(true);
      expect(
        addReach.hitIsButton,
        "Add statement is still reachable after its per-instance override was removed",
      ).toBe(true);
    } finally {
      // Best-effort, matching the idiom in `data-api-statement-strip-overlap.spec.ts`:
      // a failed assertion must not leave the shared `default` workspace holding a
      // throwaway endpoint or connection.
      if (endpointId) {
        await page.request.delete(`/api/ws/${ws.slug}/data-api/endpoints/${endpointId}`, { headers: h }).catch(() => {});
      }
      if (dbId) {
        await page.request.delete(`/api/ws/${ws.slug}/databases/${dbId}`, { headers: h }).catch(() => {});
      }
    }
  });
});
