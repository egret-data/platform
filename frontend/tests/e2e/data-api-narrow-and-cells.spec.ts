import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * Two layout defects the 2026-08-29 live pass found, neither of which any jsdom
 * test can see and neither of which the 2026-08-28 audit could have: the first
 * needs a real stylesheet to compute an overflow, and the second needs a
 * viewport narrow enough to wrap a flex row.
 *
 * **1. Prose inside a table cell overflowed its column.**
 * The shared `TableCell` is `whitespace-nowrap`, and `TryIt` puts THREE prose
 * blocks in the Service-account row's Value cell. Measured live in Chromium at
 * 1440×1100 before the first fix: the 64-character `Caveat` summary laid out at
 * 388px inside a 320px cell, so the sentence was clipped mid-phrase on screen
 * ("…does not prove a consumer can call" and then nothing) AND the whole
 * `Setting | Details | Value` table overflowed its wrapper — scrollWidth 1092
 * against clientWidth 1016 — putting a horizontal scrollbar under a settings
 * table on a desktop viewport.
 *
 * **This spec used to measure only one of the three, and passed over the other
 * two.** The first fix put `whitespace-normal` on `Caveat`'s `<summary>`, and
 * this test measured the summary — CLOSED, on the default happy path. It never
 * set `details.open`, so the ~200-character disclosure BODY was never laid out;
 * and it never drove the Service-account fetch to empty or to a failure, so
 * neither the "no service accounts yet" sentence nor the destructive `Alert`
 * carrying the server's message was ever on screen. All three inherited
 * `nowrap` from the same place, and a guard that measures one of them reports
 * green while the other two clip. So the fix moved to the source —
 * `ConfigTable`'s Value cell sets `whitespace-normal` — and this test now walks
 * all three states of that one cell:
 *
 *   a. the caveat closed (as before), then OPENED, measuring its body;
 *   b. the Service-account list intercepted to `[]`, measuring the empty note;
 *   c. the Service-account list intercepted to a `500`, measuring the `Alert`.
 *
 * (b) and (c) are the ones that need nothing clicked: an author whose workspace
 * has no accounts, or whose fetch failed, meets the clipped sentence on arrival.
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

/** The first words of the empty-workspace sentence in the same cell. */
const EMPTY_NOTE = "This workspace has no service accounts yet";

/** The message the intercepted `500` carries. Long on purpose: it is the server's
 *  own text, this cell renders it verbatim, and a short one would fit 320px and
 *  make the (c) measurement pass for a reason that is not the fix. */
const FETCH_ERROR =
  "the workspace service-account directory is temporarily unavailable, please retry";

/**
 * Every block `TryIt` puts in the Service-account row's Value cell, measured
 * against that cell. Runs IN THE PAGE, so it closes over nothing in this module
 * and takes everything it needs as one argument.
 *
 * `scrollWidth` vs `clientWidth` rather than a bounding box, and that is the
 * whole reason the earlier shape could not have caught this: under
 * `table-layout: fixed` a cell is exactly as wide as its `<th>` says, so an
 * overflowing block child keeps the cell's width and only its TEXT spills. The
 * box measures identical whether the text wraps or not; the scroll area does
 * not.
 */
function measureValueCell(opts: { caveat: string; emptyNote: string; open: boolean }) {
  const summary = [...document.querySelectorAll("summary")].find(
    (s) => s.textContent?.trim() === opts.caveat,
  );
  if (!summary) return null;

  const details = summary.closest("details") as HTMLDetailsElement | null;
  // Opened here rather than by clicking: a click would also scroll and focus,
  // and the claim is about layout, not about the toggle (which `TryIt.test.tsx`
  // already asserts). Reading `scrollWidth` below forces the layout.
  if (details && opts.open) details.open = true;

  const cellEl = summary.closest("td");
  // The `overflow-x-auto` wrapper the shared `Table` renders around every table.
  // If the table is wider than this, a settings table scrolls.
  const wrapper = summary.closest("table")?.parentElement ?? null;

  const over = (el: Element | null | undefined) =>
    el ? { scroll: el.scrollWidth, client: el.clientWidth } : null;

  return {
    cellWidth: cellEl ? Math.round(cellEl.getBoundingClientRect().width) : null,
    detailsOpen: details?.open ?? null,
    summary: over(summary),
    // `:scope > div` is the disclosure BODY — the sibling after `<summary>`.
    body: over(details?.querySelector(":scope > div")),
    // The empty note and the failed-fetch alert are two arms of one branch, so
    // at most one of them is ever on screen; the caller asserts the one it drove.
    emptyNote: over(
      cellEl
        ? [...cellEl.querySelectorAll("p")].find((el) =>
            el.textContent?.startsWith(opts.emptyNote),
          )
        : null,
    ),
    alert: over(cellEl?.querySelector('[role="alert"]')),
    wrapper: over(wrapper),
  };
}

test.describe("Data API — prose fits its cell, and a divider keeps its neighbour", () => {
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

      // ── 1. Every prose block in the Value cell ────────────────────────────
      // One viewport for all three states: 1440px is where the live pass
      // measured the clip, and it is the width at which the Value column is
      // `lg:w-80` (320px) — wide enough that nothing here is a narrow-screen
      // artefact, narrow enough that ~200 unbreakable characters cannot fit.
      await page.setViewportSize({ width: 1440, height: 1100 });
      const tryUrl = `/explore/${ws.slug}/data-api/endpoints/${endpointId}/try`;
      const accountsUrl = `**/api/ws/${ws.slug}/data-api/service-accounts`;

      // Every helper asserts FOUND before it asserts FITS: a selector that
      // matched nothing would make the width assertion vacuously true, and the
      // whole reason this section was rewritten is that the old shape reported
      // green over prose it never laid out.
      type Box = { scroll: number; client: number } | null;
      const fits = (box: Box, what: string) => {
        expect(box, `${what} is on screen`).not.toBeNull();
        // `text-overflow` is `clip` here, so an overflow is text a reader simply
        // cannot see — there is no ellipsis to tell them something is missing.
        expect(box!.scroll, `${what} lays out inside its own box`).toBeLessThanOrEqual(
          box!.client,
        );
      };
      const tableFits = (m: { wrapper: Box }, what: string) => {
        expect(m.wrapper, `${what}: the settings table is on screen`).not.toBeNull();
        expect(
          m.wrapper!.scroll,
          `${what}: the settings table does not overflow its wrapper`,
        ).toBe(m.wrapper!.client);
      };

      // (a) The caveat, closed and then open. ────────────────────────────────
      await page.goto(tryUrl);
      await expect(page.getByRole("heading", { name: "Try it", level: 2, exact: true })).toBeVisible();

      const closed = await page.evaluate(measureValueCell, {
        caveat: CAVEAT,
        emptyNote: EMPTY_NOTE,
        open: false,
      });
      // Found, not merely searched for: a renamed summary would otherwise make
      // every assertion below vacuous.
      expect(closed, `the "${CAVEAT}" caveat is on the Try it screen`).not.toBeNull();
      expect(closed!.detailsOpen, "the caveat starts closed").toBe(false);
      // The vacuity guard for everything below, and the same shape as the 320px
      // wrap guard in part 2: these are all "does the prose fit the column",
      // which passes trivially if the column ever stops being narrow. Measured
      // 304px at `w-52 lg:w-80` minus padding; the blocks it holds are 498px,
      // 807px and 1103px unwrapped.
      expect(
        closed!.cellWidth,
        "the Value column is still the narrow one (otherwise these checks are vacuous)",
      ).toBeLessThan(400);
      fits(closed!.summary, "the closed caveat summary");
      tableFits(closed!, "caveat closed");

      const opened = await page.evaluate(measureValueCell, {
        caveat: CAVEAT,
        emptyNote: EMPTY_NOTE,
        open: true,
      });
      // The guard on the guard: measuring a body that is still collapsed would
      // report a 0×0 box and pass. This is the line the previous shape lacked.
      expect(opened!.detailsOpen, "the disclosure really is open").toBe(true);
      fits(opened!.body, "the open caveat body");
      tableFits(opened!, "caveat open");

      // (b) No service accounts — nothing clicked. ───────────────────────────
      // Intercepted rather than seeded: the shared workspace HAS accounts, and
      // deleting them to observe this would be destructive and racy. The route
      // is the real one the generated client calls.
      await page.route(accountsUrl, (route) =>
        route.fulfill({ status: 200, contentType: "application/json", body: "[]" }),
      );
      await page.goto(tryUrl);
      await expect(page.getByRole("heading", { name: "Try it", level: 2, exact: true })).toBeVisible();
      await expect(page.getByText(EMPTY_NOTE, { exact: false })).toBeVisible();

      const empty = await page.evaluate(measureValueCell, {
        caveat: CAVEAT,
        emptyNote: EMPTY_NOTE,
        open: false,
      });
      expect(empty, "the caveat is still on screen with no accounts").not.toBeNull();
      fits(empty!.emptyNote, "the no-service-accounts sentence");
      tableFits(empty!, "no service accounts");

      // (c) The fetch failed — also nothing clicked. ─────────────────────────
      // A FAILED fetch is not an empty workspace, and `TryIt` renders the
      // server's own message here verbatim, so this cell holds text no
      // front-end author chose the length of.
      await page.unroute(accountsUrl);
      await page.route(accountsUrl, (route) =>
        route.fulfill({
          status: 500,
          contentType: "application/json",
          body: JSON.stringify({ error: FETCH_ERROR }),
        }),
      );
      await page.goto(tryUrl);
      await expect(page.getByRole("heading", { name: "Try it", level: 2, exact: true })).toBeVisible();
      await expect(page.getByRole("alert")).toBeVisible();

      const failed = await page.evaluate(measureValueCell, {
        caveat: CAVEAT,
        emptyNote: EMPTY_NOTE,
        open: false,
      });
      expect(failed, "the caveat is still on screen after a failed fetch").not.toBeNull();
      fits(failed!.alert, "the failed-fetch alert");
      tableFits(failed!, "failed fetch");

      await page.unroute(accountsUrl);

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
