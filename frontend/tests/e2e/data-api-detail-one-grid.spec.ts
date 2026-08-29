import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * `Response` and `Versions` after the detail-tabs pass: **one grid down the
 * tab, and a history row whose freed width went to the column that was
 * cutting.**
 *
 * # What this file is for
 *
 * `docs/en/ui-conventions.md` §10: reading the code does not settle layout,
 * and jsdom implements no CSS — a column width, a truncation, a shared
 * baseline and a right edge are all 0 or absent there. The sibling jsdom
 * tests hold what IS real in a DOM (which element is inside which table, in
 * what order, with what `colspan`); the numbers are here.
 *
 * # The measurements this replaces an argument with
 *
 * Real Chromium at 1600×1000, content box 1296, against a five-version
 * fixture, on the tree this task started from (`9fd2e493`):
 *
 *   Versions columns  128 / 688 / 224 / 128 / 128
 *   `Who` cut a 33-character address on two of five rows (231px vs 224)
 *   `Note` was handed 688px and holds "—" on a row with no note
 *   Publish sat on the SECTION TITLE's line, under the shell's own Save
 *
 * After:
 *
 *   Versions columns  128 / 592 / 320 / 128 / 128
 *   the same addresses fit; `Note` is still the unsized remainder
 *   Publish shares a baseline with the `History` heading
 *
 * And on `Response`, the tab that carried four block shapes: ONE
 * `Setting | Details | Value` grid, with the transform's editors in a row of
 * it that spans the whole table.
 *
 * # The traps this branch's specs have gone green on, and what is done here
 *
 * 1. **A counted DOM hop.** `parentElement.parentElement` re-points the moment
 *    a wrapper `div` is added. Everything here is a query from a named
 *    element, or a WALK that returns an error when it overshoots.
 *
 *    **This file shipped one anyway, and it was green on its own defect.** The
 *    Versions title guard read `title.parentElement`, and at `9fd2e493` the
 *    `h2` sat in a bare inner `<div>` with the paragraph while Publish was that
 *    div's SIBLING in the flex row — so the hop landed somewhere that held no
 *    button before OR after. It is resolved by identity now (walk both
 *    headings up until they are siblings, then ask whether the title's block
 *    contains Publish), and the old form was re-run against the restored base
 *    structure to confirm it passes there while the new one fails.
 * 2. **`getByRole` matching a name by SUBSTRING.** Every lookup passes
 *    `exact: true`, and the fixture is named `Wxyz` — not a substring of
 *    `Response`, `Versions`, `History` or `Who`.
 * 3. **A count guard that a non-empty population satisfies for free.** Every
 *    loop asserts its length against the FIXTURE's length first, and the
 *    address guard asserts that the fixture's longest address genuinely would
 *    NOT have fitted the old column — a fixture of short names cannot tell a
 *    wide column from a narrow one (§8's degenerate fixture).
 * 4. **A class that jsdom happily asserts while it is inert.** Nothing here
 *    reads `className`; every claim is a measured property.
 *
 * # What has and has not been run against this file
 *
 * **It has been executed against a live seeded stack (kind, 2026-08-29), and
 * the first run FAILED — on this file, not on the page.** Its user-list
 * intercept named a URL that does not exist, so it matched nothing and the
 * `Who` column fell back to a truncated id; see the route site for the full
 * account and for the guard that now makes a dead intercept fail AT the
 * intercept. Until that run it had never executed at all: the backend was not
 * running in the authoring environment (Vite on :5173, `/api` proxying to a
 * dead :8080), so it was committed as the regression artifact with "it passes"
 * NOT among its claims.
 *
 * Every number above WAS measured in a real browser, by a throwaway harness
 * driving headless Chromium against the same Vite dev server with the API
 * stubbed at the network layer — real Chromium, real shipped stylesheet, real
 * components, synthetic JSON — with the removed `max-w-5xl` put back as a
 * negative control and the pre-change columns reproduced.
 *
 *   EGRET_E2E_DATAAPI_ONE_GRID=1 npx playwright test data-api-detail-one-grid
 */
const ENABLED = process.env.EGRET_E2E_DATAAPI_ONE_GRID === "1";

/**
 * Five versions, and the notes span the realistic band on purpose: none, short,
 * medium, and one long enough that no finite column holds it. A fixture of
 * five identical notes cannot tell "Note keeps the remainder" from "Note was
 * given a width".
 *
 * The authors are two addresses of very different length. `LONG_AUTHOR` is
 * 33 characters — the one the old `w-56` column cut, and the reason this
 * fixture is not five copies of `admin@egret.local`.
 */
const LONG_AUTHOR = "priyanka.venkataraman@egret.local";
const NOTES = [
  "",
  "Added the region filter",
  "Cache TTL 60s, and the rate limit raised to 20 qps for the reporting bot",
  "Renamed total -> total_revenue after the finance team asked for it; this changes every consumer's key so it is a breaking change and the old key is gone",
  "First publish",
] as const;

/** What `w-56` gave the `Who` column: 224px minus the shared `TableCell`'s `px-2`. */
const OLD_WHO_CONTENT_WIDTH = 208;

/** The four sized columns of the history table, in px: Version, Who, When, Actions. */
const SIZED_COLUMNS_AT_XL = 128 + 320 + 128 + 128;

/**
 * The walk from a section's own `h2` up to the child of the scrolling host,
 * written out INSIDE each measure function rather than shared.
 *
 * `page.evaluate` ships the evaluated function's own source to the browser and
 * nothing else, so a helper called from inside one is `undefined` there — a
 * `ReferenceError` at the first assertion, which reads exactly like a missing
 * element. It is duplicated on purpose; the alternative is a spec that cannot
 * run.
 */

/**
 * Everything the `Versions` tab claims, in one page evaluation.
 *
 * `scrollWidth` is not used to decide whether an address fits: `truncate` sets
 * `overflow: hidden`, so a clipped cell reports its content and an unclipped
 * one reports its box, and after the fix the two are equal — the guard would
 * read the same number either way and pass vacuously. The natural width is
 * laid out in a detached span with the cell's own computed font instead, which
 * is the number the column has to beat and is independent of the column.
 */
function measureVersions(longAuthor: string) {
  const round = (n: number) => Math.round(n);
  const natural = (el: Element) => {
    const c = getComputedStyle(el);
    const span = document.createElement("span");
    span.style.cssText = `position:absolute;visibility:hidden;white-space:pre;font:${c.font};letter-spacing:${c.letterSpacing}`;
    span.textContent = el.textContent ?? "";
    document.body.appendChild(span);
    const w = round(span.getBoundingClientRect().width);
    span.remove();
    return w;
  };
  /** The right edge of the last painted TEXT in a row, ignoring the action cell. */
  const lastTextRight = (row: HTMLElement, cells: HTMLElement[]) => {
    let right = 0;
    for (let i = 0; i < cells.length - 1; i++) {
      const walker = document.createTreeWalker(cells[i]!, NodeFilter.SHOW_TEXT);
      let node: Node | null;
      while ((node = walker.nextNode())) {
        if (!node.nodeValue?.trim()) continue;
        const range = document.createRange();
        range.selectNodeContents(node);
        for (const box of range.getClientRects()) {
          // Clip to the cell: a truncated text node reports its UNCLIPPED rect.
          if (box.width > 0) right = Math.max(right, Math.min(box.right, cells[i]!.getBoundingClientRect().right));
        }
      }
    }
    return round(right || row.getBoundingClientRect().left);
  };

  const sectionRootOf = (title: string): HTMLElement | null => {
    const heading = [...document.querySelectorAll("h2")].find((h) => h.textContent?.trim() === title);
    const routeRoot = document.querySelector<HTMLElement>("main .p-4");
    if (!heading || !routeRoot) return null;
    const host = routeRoot.querySelector<HTMLElement>("fieldset") ?? routeRoot;
    let node: HTMLElement | null = heading.parentElement;
    while (node && node.parentElement !== host) node = node.parentElement;
    if (!node || node === host || node.tagName === "FIELDSET") return null;
    return node;
  };

  const root = sectionRootOf("Versions");
  if (!root) return { error: "no Versions section root" as const };
  const rows = [...root.querySelectorAll<HTMLElement>('[data-testid^="version-row-"]')];
  if (rows.length === 0) return { error: "no version rows on screen" as const };
  const table = rows[0]!.closest("table");
  if (!table) return { error: "the rows are not in a table" as const };
  const wrapper = table.parentElement;
  if (!wrapper) return { error: "the table has no scroll wrapper" as const };

  const history = [...root.querySelectorAll("h3")].find((h) => h.textContent?.trim() === "History");
  const publish = [...root.querySelectorAll("button")].find(
    (b) => b.textContent?.trim() === "Publish saved draft",
  );
  const title = [...document.querySelectorAll("h2")].find((h) => h.textContent?.trim() === "Versions");
  if (!history || !publish || !title) return { error: "History, Publish or the title is missing" as const };

  const centre = (el: Element) => {
    const b = el.getBoundingClientRect();
    return round(b.top + b.height / 2);
  };

  return {
    error: null,
    sectionRight: round(root.getBoundingClientRect().right),
    tableRight: round(table.getBoundingClientRect().right),
    tableScroll: { scroll: wrapper.scrollWidth, client: wrapper.clientWidth },
    columns: [...table.querySelectorAll("th")].map((th) => ({
      name: (th.textContent ?? "").trim(),
      width: round(th.getBoundingClientRect().width),
    })),
    heading: {
      // A shared line is a shared vertical CENTRE, not an equal `top`: the
      // button is taller than the heading, so `top` reports two lines for one.
      historyCentre: centre(history),
      publishCentre: centre(publish),
      publishAboveTable: publish.getBoundingClientRect().bottom <= table.getBoundingClientRect().top,
      // The block that holds the section title must not hold Publish — that is
      // the sibling shape, and it is resolved by IDENTITY.
      //
      // This read `title.parentElement.querySelectorAll("button")` first, and
      // that was trap #1 in this file's own header, green on the very defect it
      // names: at `9fd2e493` the `h2` sat in a bare inner `<div>` with the
      // paragraph and Publish was that div's SIBLING inside the flex row, so
      // the hop landed on a div that never held a button either way. Proven,
      // not reasoned: with the base structure restored, the old assertion
      // passes and this one fails.
      titleBlockHoldsPublish: (() => {
        const chain = (el: HTMLElement) => {
          const out: HTMLElement[] = [];
          for (let n: HTMLElement | null = el; n; n = n.parentElement) out.push(n);
          return out;
        };
        const ct = chain(title as HTMLElement);
        const ch = chain(history as HTMLElement);
        const common = ct.find((n) => ch.includes(n));
        if (!common) return null;
        const titleBlock = ct.find((n) => n.parentElement === common);
        return titleBlock ? titleBlock.contains(publish) : null;
      })(),
    },
    rows: rows.map((row) => {
      const cells = [...row.querySelectorAll<HTMLElement>("td")];
      const who = cells[2]!;
      const action = row.querySelector<HTMLElement>("td:last-child button");
      return {
        text: (row.innerText ?? "").replace(/\n/g, "|").slice(0, 40),
        who: {
          text: (who.textContent ?? "").trim(),
          natural: natural(who),
          // The CONTENT box, not `clientWidth`: the shared `TableCell` is
          // `px-2`, so `clientWidth` over-reports by 16px and a string that
          // overflows by 10 would read as fitting.
          available: (() => {
            const cs = getComputedStyle(who);
            return round(who.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight));
          })(),
          isLong: (who.textContent ?? "").trim() === longAuthor,
        },
        lastTextRight: lastTextRight(row, cells),
        actionLeft: action ? round(action.getBoundingClientRect().left) : null,
      };
    }),
  };
}

/** Everything the `Response` tab claims, in one page evaluation. */
function measureResponse() {
  const round = (n: number) => Math.round(n);
  const sectionRootOf = (title: string): HTMLElement | null => {
    const heading = [...document.querySelectorAll("h2")].find((h) => h.textContent?.trim() === title);
    const routeRoot = document.querySelector<HTMLElement>("main .p-4");
    if (!heading || !routeRoot) return null;
    const host = routeRoot.querySelector<HTMLElement>("fieldset") ?? routeRoot;
    let node: HTMLElement | null = heading.parentElement;
    while (node && node.parentElement !== host) node = node.parentElement;
    if (!node || node === host || node.tagName === "FIELDSET") return null;
    return node;
  };

  const root = sectionRootOf("Response");
  if (!root) return { error: "no Response section root" as const };

  const settingHeads = [...root.querySelectorAll("th")].filter(
    (th) => (th.textContent ?? "").trim() === "Setting",
  );
  if (settingHeads.length !== 1) {
    return { error: `expected exactly one settings grid, found ${settingHeads.length}` as const };
  }
  const grid = settingHeads[0]!.closest("table");
  if (!grid) return { error: "the Setting header is not in a table" as const };

  const toggle = root.querySelector<HTMLElement>("#resp-transform-enabled");
  const editor = root.querySelector<HTMLElement>('[aria-label="Response transform script"], .monaco-editor');
  const download = [...root.querySelectorAll("button")].find(
    (b) => b.textContent?.trim() === "Download OpenAPI",
  );
  const openapiHeading = [...root.querySelectorAll("h3")].find(
    (h) => h.textContent?.trim() === "OpenAPI document",
  );
  if (!toggle || !download || !openapiHeading) {
    return { error: "the toggle, the download button or the OpenAPI heading is missing" as const };
  }

  const editorCell = editor?.closest("td") ?? null;
  const centre = (el: Element) => {
    const b = el.getBoundingClientRect();
    return round(b.top + b.height / 2);
  };

  return {
    error: null,
    sectionRight: round(root.getBoundingClientRect().right),
    // The toggle is IN that one grid — measured as the same element, not as
    // "some table exists".
    toggleIsInTheGrid: toggle.closest("table") === grid,
    gridWidth: round(grid.getBoundingClientRect().width),
    editorRow: editorCell
      ? { width: round(editorCell.getBoundingClientRect().width), inTheGrid: editorCell.closest("table") === grid }
      : null,
    openapi: {
      headingCentre: centre(openapiHeading),
      buttonCentre: centre(download),
      // A `border-t` that actually paints, read as a property rather than as
      // a class — a `border-*` utility with no `border-t-width` is inert.
      //
      // The block is reached by a WALK from the heading up to the direct child
      // of the section root, never by `parentElement.parentElement` — one
      // wrapper `div` inside the block re-points that at the heading LINE,
      // which has no border of its own, and the assertion would then fail for
      // a reason that has nothing to do with the rule. (The same hop in the
      // other direction is what made this file's Versions guard vacuous.)
      blockBorderTop: (() => {
        let block: HTMLElement | null = openapiHeading as HTMLElement;
        while (block && block.parentElement !== root) block = block.parentElement;
        if (!block || block === root) return null;
        const cs = getComputedStyle(block);
        return { width: cs.borderTopWidth, style: cs.borderTopStyle };
      })(),
    },
    // The section's TOP-LEVEL table frames, by right edge. §5.1's one measure:
    // frames share the page's right edge, prose does not.
    //
    // Nested tables are excluded, and that is not a convenience: `ReturnsEditor`
    // renders a `<Table className="max-w-xl">` INSIDE a cell of the settings
    // grid, and §5.1/§5.5 keep that cap deliberately — a row of four form
    // controls stretched wide puts the delete button an arm's length from the
    // row it acts on. A blanket `querySelectorAll("table")` would report that
    // deliberate 576px frame as a violation of the rule that exempts it.
    frameRights: [...root.querySelectorAll("table")]
      .filter((t) => !t.parentElement?.closest("table"))
      .map((t) => round(t.getBoundingClientRect().right)),
    nestedFrames: [...root.querySelectorAll("table")].filter((t) => t.parentElement?.closest("table")).length,
    proseRights: [...root.querySelectorAll<HTMLElement>("p.max-w-prose")].map((p) => ({
      width: round(p.getBoundingClientRect().width),
      right: round(p.getBoundingClientRect().right),
    })),
  };
}

test.describe("Data API — one grid down the tab, and a history row that spends its width", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DATAAPI_ONE_GRID=1 with the live dev stack up");
  test.setTimeout(180_000);

  test("Versions spends the freed width on the column that was cutting, and Publish is on the History line", async ({
    page,
  }) => {
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
      name: `e2e-one-grid-${runId}`,
      engine: "starrocks",
      host: "localhost",
      port: 9030,
      options: { user: "root" },
    });
    // `Wxyz`, for the reason the sibling specs give: the shell renders the
    // endpoint's name beside the section's, and a descriptive fixture name is
    // one dropped `exact` away from satisfying a lookup it should not.
    const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
      name: `Wxyz ${runId}`,
      path: `e2e-one-grid-${runId}`,
      database_id: db.id as string,
      statements: [{ name: "main", sql: "SELECT 1 AS total" }],
    });
    const endpointId = endpoint.id as string;

    // The history is intercepted rather than published for real: publishing
    // five times would need an engine that answers, and this spec's subject is
    // COLUMN WIDTH against note and address length. The shapes are the DTO's.
    const versions = NOTES.map((note, i) => ({
      id: `e2e-ver-${5 - i}`,
      endpoint_id: endpointId,
      version: 5 - i,
      note,
      created_by: i % 2 === 0 ? "e2e-user-1" : "e2e-user-2",
      created_at: `2026-08-0${(i % 8) + 1}T09:00:00Z`,
      is_live: 5 - i === 4,
      snapshot: {},
    }));
    await page.route(`**/api/ws/${ws.slug}/data-api/endpoints/${endpointId}/versions`, (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(versions) }),
    );
    // `/api/control-plane/users`, and the URL is worth a sentence because THIS
    // GLOB WAS WRONG UNTIL THE FILE'S FIRST LIVE RUN (2026-08-29). It read
    // `**/api/access/users` — the name of the frontend MODULE the call comes
    // from (`src/api/access.ts`), not the route it issues; `listUsers()` calls
    // the generated `controlPlaneAccessListUsers`, whose URL builder returns
    // `/api/control-plane/users`. A `page.route` glob that matches nothing does
    // not throw: the real request went through, `Who` fell back to
    // `created_by.slice(0, 8)` (Versions.tsx:307), and the run died 15s later
    // on "element not found" — which reads exactly like the column regression
    // this file exists to catch. On a stubbed harness it could never have been
    // noticed, because the harness answered every request itself.
    //
    // `interceptedUsers` guards the OTHER half, and it is worth being exact
    // about which: re-breaking the glob was run as a mutation, and the run dies
    // at the `LONG_AUTHOR` visibility wait below, NOT here — so this counter is
    // not what catches a rename, and saying so would be the kind of guard this
    // branch keeps finding green on the defect it names. What it does catch is
    // the case the visibility wait CANNOT see: a stack whose real user list
    // happens to contain that address, where the wait passes, the intercept is
    // dead anyway, and every width below is measured against whatever the
    // backend holds rather than against the fixture this file documents.
    let interceptedUsers = 0;
    await page.route("**/api/control-plane/users", (route) => {
      interceptedUsers += 1;
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          { id: "e2e-user-1", email: "admin@egret.local", name: "Admin", status: "active" },
          { id: "e2e-user-2", email: LONG_AUTHOR, name: "P V", status: "active" },
        ]),
      });
    });

    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}/versions`);
    await expect(page.getByRole("heading", { name: "Versions", level: 2, exact: true })).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.getByTestId("version-row-e2e-ver-5")).toBeVisible();
    // The author column resolves through a second query; wait for the value
    // this spec measures rather than for a timeout.
    await expect(page.getByText(LONG_AUTHOR, { exact: true }).first()).toBeVisible();
    // Asserted before the measurements, not after: if the user list were served
    // by the real backend the addresses below would be whoever the stack holds,
    // and every width claim would be about a different fixture than the one
    // this file documents.
    expect(interceptedUsers, "the user-list intercept actually fired").toBeGreaterThan(0);

    const m = await page.evaluate(measureVersions, LONG_AUTHOR);
    expect(m.error, "the history table was measured").toBeNull();
    if (m.error) throw new Error(m.error);

    // Count guards for every loop below.
    expect(m.rows.length, "all five versions are on screen").toBe(NOTES.length);
    expect(m.columns.map((c) => c.name)).toEqual(["Version", "Note", "Who", "When", "Actions"]);
    expect(m.tableScroll.scroll, "the table does not scroll sideways at 1600").toBeLessThanOrEqual(
      m.tableScroll.client,
    );

    // §5.1: the frame takes the page's measure.
    expect(Math.abs(m.tableRight - m.sectionRight), "the table ends at the section's edge").toBeLessThanOrEqual(1);

    const who = m.columns.find((c) => c.name === "Who")!;
    const note = m.columns.find((c) => c.name === "Note")!;

    // `Note` is still the UNSIZED remainder — the four sized columns plus it
    // are the whole table, so a width added to Note would show up here.
    expect(note.width, "Note keeps the remainder").toBe(m.tableScroll.client - SIZED_COLUMNS_AT_XL);

    const longRows = m.rows.filter((r) => r.who.isLong);
    // NON-VACUITY: the fixture must contain an address the OLD column could
    // not hold, or "it fits now" says nothing.
    expect(longRows.length, "the fixture has rows with the long address").toBeGreaterThan(0);
    for (const r of longRows) {
      expect(
        r.who.natural,
        `${r.who.text} is an address the old w-56 column could not hold`,
      ).toBeGreaterThan(OLD_WHO_CONTENT_WIDTH);
      expect(r.who.natural, `${r.who.text} fits the widened column`).toBeLessThanOrEqual(r.who.available);
    }
    expect(who.width, "Who is the widened column, not the remainder").toBe(320);

    // The row's action is beside the row's content, not across a void — the
    // number §5.1 records, asserted as a bound rather than as an equality.
    const withAction = m.rows.filter((r) => r.actionLeft !== null);
    expect(withAction.length, "the fixture has rollback-able rows").toBeGreaterThan(0);
    for (const r of withAction) {
      expect(r.actionLeft! - r.lastTextRight, `${r.text}: the action follows the row's text`).toBeLessThanOrEqual(
        120,
      );
    }

    // The heading treatment: Publish shares the History line, and the section
    // title's line holds no control.
    expect(m.heading.publishCentre, "Publish shares the History line").toBe(m.heading.historyCentre);
    expect(m.heading.publishAboveTable, "…and that line is above the table").toBe(true);
    expect(m.heading.titleBlockHoldsPublish, "the section title's block holds no control").toBe(false);
  });

  test("Response renders one settings grid, with the transform's editors in a row that spans it", async ({
    page,
  }) => {
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
      name: `e2e-one-grid-r-${runId}`,
      engine: "starrocks",
      host: "localhost",
      port: 9030,
      options: { user: "root" },
    });
    const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
      name: `Wxyz ${runId}`,
      path: `e2e-one-grid-r-${runId}`,
      database_id: db.id as string,
      statements: [{ name: "main", sql: "SELECT 1 AS total" }],
      // A transform, so the editor row is on screen — the whole point of the
      // second half of this test. Without it the row does not exist and the
      // measurement would be `null` rather than wrong.
      output: {
        transform: {
          script: "def transform(params, statements):\n    return {}\n",
          returns: [{ name: "total", ty: "double", required: true }],
        },
      },
    });
    const endpointId = endpoint.id as string;

    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}/response`);
    await expect(page.getByRole("heading", { name: "Response", level: 2, exact: true })).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.getByRole("checkbox", { name: "Build the response with a script", exact: true })).toBeVisible();
    // Monaco mounts asynchronously; the editor row's width is what is measured,
    // so wait for the editor rather than for a timeout.
    await expect(page.locator(".monaco-editor").first()).toBeVisible({ timeout: 30_000 });

    const m = await page.evaluate(measureResponse);
    expect(m.error, "the Response section was measured").toBeNull();
    if (m.error) throw new Error(m.error);

    expect(m.toggleIsInTheGrid, "the transform toggle is a row of the one settings grid").toBe(true);
    expect(m.editorRow, "the transform's editors are in a table cell").not.toBeNull();
    expect(m.editorRow!.inTheGrid, "…of that same grid").toBe(true);
    // Full width is a MEASUREMENT here; `colspan` is what the jsdom sibling
    // asserts, and a `colspan` on a table whose columns are hidden is not a
    // span the reader gets.
    expect(
      Math.abs(m.editorRow!.width - m.gridWidth),
      "the editor row spans the whole grid",
    ).toBeLessThanOrEqual(2);

    // The action at the end of the tab: heading and button on one line, under
    // a border that actually paints.
    expect(m.openapi.buttonCentre, "Download OpenAPI shares the heading's line").toBe(
      m.openapi.headingCentre,
    );
    expect(m.openapi.blockBorderTop, "the action block is separated by a real border").toEqual({
      width: "1px",
      style: "solid",
    });

    // §5.1's one measure: every FRAME ends at the section's right edge, and
    // prose is the deliberate exception. Counted before iterating.
    expect(m.frameRights.length, "the section has frames to compare").toBeGreaterThanOrEqual(2);
    // The exclusion above is not vacuous: with a transform on there IS a nested
    // frame, and it is the one §5.5 exempts.
    expect(m.nestedFrames, "the returns grid is the nested frame the filter excludes").toBe(1);
    for (const right of m.frameRights) {
      expect(Math.abs(right - m.sectionRight), "a frame ends at the section's edge").toBeLessThanOrEqual(1);
    }
    expect(m.proseRights.length, "the section has capped prose").toBeGreaterThan(0);
    for (const p of m.proseRights) {
      expect(p.width, "prose stops at a reading measure").toBeLessThanOrEqual(700);
    }
  });
});
