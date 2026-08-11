import { test, expect, type Page } from "@playwright/test";
import { loginLocalAdmin } from "./helpers";

/**
 * THE DRAG LISTS IN THE CHART SETTINGS PANEL, DRIVEN FOR REAL.
 *
 * Reordering table columns, cartesian series and pie slices became a `dnd-kit` drag in #470, and
 * nothing in the repo has exercised the drag since. `reorderColumnOrder` and `reorderSeriesOrder`
 * are unit-tested, which pins WHICH STRING lands in the stored order — the part that has regressed
 * repeatedly — but not that a grip is wired to a sensor, that the sensor produces a drop, or that
 * the drop reaches the chart. Every one of those could break with the unit tests still green.
 *
 * IT CANNOT BE A JSDOM TEST. `dnd-kit` resolves a drop by measuring elements: `PointerSensor`
 * needs a real 6px movement and `KeyboardSensor`'s `sortableKeyboardCoordinates` reads
 * `getBoundingClientRect`. jsdom performs no layout, so every rect is zeros and there is no
 * geometry for a collision to be detected against — a "drag" there resolves to nothing and the
 * assertion passes against an unmoved list. Hence a real browser, and hence this file.
 *
 * IT IS A KEYBOARD DRAG, not a simulated mouse one. Both lists register `KeyboardSensor` beside
 * `PointerSensor`, and the keyboard path goes through the same collision detection and the same
 * `onDragEnd` — while being deterministic, instead of depending on how many synthetic mousemoves
 * happen to clear a 6px activation threshold. It also covers the accessibility path, which is the
 * one nobody notices breaking.
 *
 * Opt-in: EGRET_E2E_KIND=1 against the in-cluster kind deploy. NOT part of `npm test`.
 *
 *   EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://localhost:30080 npx playwright test settings-dnd
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";

/** The parity charts this runs against; override when the fixture ids differ. */
const TABLE_CHART = process.env.EGRET_E2E_TABLE_CHART ?? "22ab6537-6efb-4b14-b54b-4461c2006dd6";
const PIE_CHART = process.env.EGRET_E2E_PIE_CHART ?? "031d0a1c-0cb1-4d95-ab9c-6ff2365f56c3";
const WS = process.env.EGRET_E2E_WORKSPACE ?? "demo";

/** The accessible names of the drag grips, in the order they are drawn. */
async function gripOrder(page: Page): Promise<string[]> {
  return page.$$eval('[aria-label^="reorder "]', (els) =>
    els.map((e) => (e.getAttribute("aria-label") ?? "").replace(/^reorder /, "")),
  );
}

/**
 * Two animation frames.
 *
 * `dnd-kit` binds its keyboard move handlers in an effect that runs after the activating Space has
 * been processed and the drag state has painted. An ArrowDown sent in the same tick therefore
 * arrives before anything is listening and is silently dropped — the final Space then "drops" an
 * item that never moved, and the list is unchanged. That looked exactly like a broken product for
 * two runs of this file, and was not: driven by hand, with human-length pauses, the same three
 * keys reorder the list and the rendered table follows.
 *
 * Two frames rather than a sleep because that is the actual dependency, and rather than one
 * because the effect is scheduled from the frame the activation painted in.
 */
async function nextFrames(page: Page): Promise<void> {
  await page.evaluate(
    () => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r()))),
  );
}

/** What `dnd-kit` has last announced, which is how this file knows a step landed. */
async function announcement(page: Page): Promise<string> {
  return page.$$eval("[aria-live]", (rs) =>
    rs.map((r) => (r.textContent ?? "").trim()).filter(Boolean).join(" | "),
  );
}

/**
 * Pick up the grip at `index` with the keyboard, move it one place down, and drop it.
 *
 * Space activates and drops; the arrow keys move.
 *
 * EACH STEP WAITS FOR THE PREVIOUS ONE. Firing the three keys back to back silently does nothing:
 * `dnd-kit` attaches the move handlers when the drag starts, so an ArrowDown sent in the same tick
 * as the activating Space arrives before anything is listening, and the final Space then "drops" an
 * item that never moved — leaving the list in its original order and the test failing as though
 * the product were broken. It is not; this cost a false accusation to learn.
 *
 * The waits are on `dnd-kit`'s own signals — `aria-pressed` for the pickup, its live-region
 * announcement for the move — rather than on sleeps, so this neither flakes nor slows down.
 */
async function keyboardDragDown(page: Page, index: number): Promise<void> {
  const grip = page.locator('[aria-label^="reorder "]').nth(index);
  const name = (await grip.getAttribute("aria-label"))?.replace(/^reorder /, "") ?? "";

  await grip.focus();
  await page.keyboard.press("Space");
  await expect(grip).toHaveAttribute("aria-pressed", "true");
  await nextFrames(page);

  // The pickup announcement ALSO reads "was moved over droppable area <itself>", so waiting for
  // that string proves nothing. What proves the arrow landed is the droppable being a DIFFERENT
  // item — which is the whole difference between a move and a no-op.
  await page.keyboard.press("ArrowDown");
  await expect
    .poll(() => announcement(page), { message: `moving ${name}` })
    .toMatch(new RegExp(`was moved over droppable area (?!${name}\\b)`));

  await page.keyboard.press("Space");
  await expect
    .poll(() => announcement(page), { message: `dropping ${name}` })
    .toContain("was dropped over droppable area");
}

/**
 * ONE LOGIN FOR THE WHOLE FILE. Sign-in is rate limited to 5 attempts per 5 minutes, so a
 * `loginLocalAdmin` in each test spends the budget in a single run and the last tests fail at
 * `waitForURL` — a login timeout wearing the costume of a product bug. (It cost a wrong diagnosis
 * here before the restructure.) One authenticated page is shared; the tests only read and reorder,
 * and none of them saves, so they do not interfere.
 */
test.describe("chart settings drag lists", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.describe.configure({ mode: "serial" });
  test.setTimeout(90_000);

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await loginLocalAdmin(page);
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("a table column can be dragged down the list, and the table follows", async () => {
    await page.goto(`/explore/${WS}/charts/${TABLE_CHART}?tab=data`);
    await page.waitForSelector('[aria-label^="reorder "]');

    const before = await gripOrder(page);
    expect(before.length).toBeGreaterThan(1);

    // The rendered table is the point: a list widget that reorders itself while the chart keeps
    // its old column order would look right and be broken, and is exactly what a green unit test
    // cannot tell apart.
    const headersBefore = await page.$$eval("table thead th", (th) =>
      th.map((h) => (h.textContent ?? "").trim()),
    );

    await keyboardDragDown(page, 0);

    const expected = [before[1], before[0], ...before.slice(2)];
    await expect
      .poll(() => gripOrder(page), { message: "the list must reorder" })
      .toEqual(expected);

    await expect
      .poll(
        async () =>
          page.$$eval("table thead th", (th) => th.map((h) => (h.textContent ?? "").trim())),
        { message: "the rendered table must follow the list" },
      )
      .toEqual([headersBefore[1], headersBefore[0], ...headersBefore.slice(2)]);
  });

  test("the drag is undoable by dragging back, leaving the chart as it was", async () => {
    // Guards the reorder being one-way — a swap that cannot be swapped back is a stored-order bug
    // that a single drag cannot see. It also leaves the shared fixture unchanged, which matters:
    // nothing here presses Update, so nothing is persisted, but the next test should not depend on
    // that.
    await page.goto(`/explore/${WS}/charts/${TABLE_CHART}?tab=data`);
    await page.waitForSelector('[aria-label^="reorder "]');

    const before = await gripOrder(page);
    await keyboardDragDown(page, 0);
    await expect.poll(() => gripOrder(page)).toEqual([before[1], before[0], ...before.slice(2)]);

    // INDEX 0 AGAIN, not 1. The first drag swapped the top pair, so the item that moved now sits
    // AT index 1 — dragging index 1 down would carry it further down the list, which is what this
    // line did at first: `[created_at, title, id, …]` rather than the original order. Swapping the
    // top pair a second time is what puts both back.
    await keyboardDragDown(page, 0);
    await expect.poll(() => gripOrder(page)).toEqual(before);
  });

  test("a pie slice can be dragged down the list", async () => {
    // The second list, and a different component: pie rows are `SortableRow` render-props holding
    // several controls, where the grip is deliberately the ONLY drag handle. If the handle were
    // spread onto the row instead, this still passes — but clicking the colour swatch would start
    // a drag, which is why that is asserted separately below.
    await page.goto(`/explore/${WS}/charts/${PIE_CHART}?tab=display`);
    await page.waitForSelector('[aria-label^="reorder "]');

    const before = await gripOrder(page);
    expect(before.length).toBeGreaterThan(1);

    await keyboardDragDown(page, 0);
    await expect.poll(() => gripOrder(page)).toEqual([before[1], before[0], ...before.slice(2)]);
  });

  test("clicking a pie row's colour swatch opens the picker instead of starting a drag", async () => {
    await page.goto(`/explore/${WS}/charts/${PIE_CHART}?tab=display`);
    await page.waitForSelector('[aria-label^="reorder "]');

    const before = await gripOrder(page);
    const swatch = page.locator('button[aria-label^="color "]').first();
    await swatch.click();

    // The palette is the proof the click did what it says; the unchanged order is the proof it did
    // not also pick the row up.
    await expect(page.locator('[role="dialog"] button[aria-label^="#"]').first()).toBeVisible();
    expect(await gripOrder(page)).toEqual(before);
  });
});
