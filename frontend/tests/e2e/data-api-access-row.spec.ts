import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * `Access`, after Task 2: **a row that fits, and an identity you can read.**
 *
 * # What was wrong, measured rather than recalled
 *
 * Real Chromium, the shipped stylesheet, seven service accounts, 1600×1000, on
 * the tree this task started from (`82ce049e` — i.e. with Task 1's `max-w-5xl`
 * already off, so this is not a complaint about the cap):
 *
 *   available 1296     columns 192 / 960 / 144
 *   row heights        79, 79, 79, 79, 69, 79, 79   (543px of table)
 *   the row's actions  TWO lines — a 32px/13px button, then a 16px/12px link
 *   every address cut  246, 311, 419, 303, 289, 296, 311px against 176px
 *
 * Three defects in one cell layout. The actions were **stacked**, which is
 * what made every row 79px and made `Try it` — the control the section's own
 * prose tells the reader to use — the smallest and faintest thing in the row.
 * The identity column was `w-48`, i.e. 176px of content, on the page whose
 * subject *is* which identity may invoke. And removing the cap had moved the
 * mutation 272px further from the account it acts on, which the row layout is
 * what answers.
 *
 * After, same harness and same fixtures:
 *
 *   columns 384 / 704 / 208
 *   row heights        69, 61, 61, 69, 69, 61, 61   (451px — 92px less)
 *   the row's actions  ONE line, both 32px, both 13px
 *   six of seven addresses fit; the 58-character outlier keeps `title`
 *
 * # Why this cannot be a jsdom test
 *
 * Not "is weaker in jsdom" — cannot be. jsdom implements no CSS
 * (`docs/en/ui-conventions.md` §10), so a row height, a column width, a
 * truncation and a font size are all 0 or absent, and a test asserting any of
 * them passes with the fix and without it. `AccessSection.test.tsx` holds the
 * part that is real in a DOM — which elements are siblings, in what order, and
 * what each one is — plus a handful of class canaries that stop a silent
 * revert. The claims are here.
 *
 * # The two traps the sibling specs went green on
 *
 * 1. **A counted DOM hop.** `parentElement.parentElement` re-points at the host
 *    the moment one wrapper `div` is added, and the assertion then compares a
 *    thing with itself. Everything here is reached by a query from the row or a
 *    WALK that fails loudly when it overshoots.
 * 2. **A `getByRole` name matched by substring.** Playwright matches
 *    accessible names case-insensitively by substring unless told otherwise,
 *    and `data-api-detail-tab-widths.spec.ts` died on its own fixture being
 *    called "E2E response width". Every lookup here passes `exact: true`, and
 *    the fixture is named `Wxyz` — not a substring of `Access`, `Try it`, or
 *    `Service account`.
 *
 * And the general one: **assert the count before iterating.** A loop over a
 * selector that stopped matching asserts nothing and reports green, so the
 * seven rows are counted before anything is claimed about them.
 *
 * # Why the accounts are intercepted and the endpoint is real
 *
 * The endpoint is created for real, because the route has to resolve and the
 * six tabs have to mount. The service-account list, the grants and the invoke
 * probe are `page.route`d, for the reason `data-api-narrow-and-cells.spec.ts`
 * gives for its own interception: the shared workspace's accounts are whatever
 * somebody seeded, and this spec's whole subject is a column width against
 * ADDRESS LENGTH. Creating seven accounts would leave them behind; deleting
 * the workspace's own would be destructive and racy. The addresses used are
 * the exact form the backend synthesises
 * (`crates/egret-dataapi/src/routes.rs`: `svc.<slug>@<ws>.service.egret.local`),
 * so they are realistic in the only way that matters here — length.
 *
 * # What has and has not been run against this file
 *
 * **This spec has not been executed against a live seeded stack.** When it was
 * written the backend was not running in the authoring environment (Vite on
 * :5173, `/api` proxying to a dead :8080) and standing one up is a Rust build
 * outside a frontend-only task. It compiles and `--list` enumerates it, and it
 * is committed as the regression artifact — "it passes" is NOT among the
 * claims made for it.
 *
 * Every geometric claim in it WAS verified in a real browser, by a throwaway
 * harness driving headless Chromium against the same Vite dev server with the
 * API stubbed at the network layer — real Chromium, real shipped stylesheet,
 * real components, synthetic JSON — and with a negative control: the
 * pre-change component was restored, the page re-measured, and the 79px rows,
 * the two-line actions and the seven cut addresses reappeared.
 *
 *   EGRET_E2E_DATAAPI_ACCESS_ROW=1 npx playwright test data-api-access-row
 */
const ENABLED = process.env.EGRET_E2E_DATAAPI_ACCESS_ROW === "1";

/**
 * Seven accounts, named so their synthesised addresses span the realistic band
 * this column has to hold — and NOT seven of the same length, which is the
 * degenerate fixture that cannot tell a column wide enough from one that is
 * not (§8).
 *
 * `decommissioned-2025-reporting` is deliberately the outlier: its address is
 * 58 characters and does NOT fit, which is what `title` is still for. The spec
 * asserts it separately rather than pretending the column holds everything.
 */
const NAMES = [
  "aaaaa",
  "kind-smoke-bot",
  "decommissioned-2025-reporting",
  "reporting-bot",
  "billing-bot",
  "stranger-bot",
  "nightly-export",
] as const;

/** The one whose address is expected NOT to fit — named, so a rename cannot make the split silent. */
const OUTLIER = "decommissioned-2025-reporting";

/** What `w-48` gave this column: 192px minus the shared `TableCell`'s `px-2`. */
const OLD_CONTENT_WIDTH = 176;

/**
 * Everything this spec measures, read in ONE page evaluation.
 *
 * Two things about how it reads widths.
 *
 * `scrollWidth` cannot be used for the address once it FITS: `truncate` sets
 * `overflow: hidden`, so a clipped element reports its content width and an
 * unclipped one reports the box. After the fix they are equal, and the "would
 * the old column have cut this?" guard would read 368 for every row and pass
 * vacuously. So the natural width is measured by laying the same string out in
 * a detached span with the element's own computed font — the number the column
 * has to beat, independent of the column.
 *
 * Lines are counted as DISTINCT vertical centres, not by comparing `top`: the
 * two controls are the same height today, but a variant change that made one
 * shorter would still be one line, and `top` would report two.
 */
function measure(outlier: string) {
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

  const rows = [...document.querySelectorAll<HTMLElement>('[data-testid^="access-row-"]')];
  if (rows.length === 0) return { error: "no Access rows on screen" as const };

  const table = rows[0]!.closest("table");
  if (!table) return { error: "the rows are not in a table" as const };
  // The `overflow-x-auto` wrapper the shared `Table` renders. A WALK is not
  // needed here (it is one known parent), but landing on nothing is an error
  // rather than a skipped assertion.
  const wrapper = table.parentElement;
  if (!wrapper) return { error: "the table has no scroll wrapper" as const };

  return {
    error: null,
    tableScroll: { scroll: wrapper.scrollWidth, client: wrapper.clientWidth },
    columns: [...table.querySelectorAll("th")].map((th) => round(th.getBoundingClientRect().width)),
    rows: rows.map((row) => {
      const cells = [...row.querySelectorAll<HTMLElement>("td")];
      const identity = cells[0]!;
      const actionCell = cells[2]!;
      const address = identity.querySelector<HTMLElement>("div.font-mono")!;
      const controls = [...actionCell.querySelectorAll<HTMLElement>("a,button")];
      const box = (el: Element) => el.getBoundingClientRect();
      return {
        height: round(box(row).height),
        address: {
          text: address.textContent ?? "",
          natural: natural(address),
          available: address.clientWidth,
          isOutlier: (address.textContent ?? "").includes(outlier),
        },
        actions: {
          labels: controls.map((c) => c.textContent?.trim() ?? ""),
          tags: controls.map((c) => c.tagName),
          // Distinct vertical centres === lines.
          lines: new Set(controls.map((c) => round(box(c).top + box(c).height / 2))).size,
          overflows: actionCell.scrollWidth > actionCell.clientWidth,
          paint: controls.map((c) => {
            const s = getComputedStyle(c);
            return {
              label: c.textContent?.trim() ?? "",
              tag: c.tagName,
              fontSize: s.fontSize,
              height: round(box(c).height),
              borderWidth: s.borderTopWidth,
              background: s.backgroundColor,
              left: round(box(c).left),
            };
          }),
        },
      };
    }),
    // §5.2's deliberate exception to §5.1's one width: a frame may span
    // whatever it is given, a line of running text may not.
    prose: [...document.querySelectorAll<HTMLElement>("main p.max-w-prose")].map((p) => ({
      width: round(p.getBoundingClientRect().width),
      right: round(p.getBoundingClientRect().right),
    })),
    sectionRight: (() => {
      // A WALK from the section's own `h2` up to the child of the host, never
      // a counted hop — see the file header, trap 1.
      const heading = [...document.querySelectorAll("h2")].find(
        (h) => h.textContent?.trim() === "Access",
      );
      const routeRoot = document.querySelector<HTMLElement>("main .p-4");
      if (!heading || !routeRoot) return null;
      const host = routeRoot.querySelector<HTMLElement>("fieldset") ?? routeRoot;
      let root: HTMLElement | null = heading.parentElement;
      while (root && root.parentElement !== host) root = root.parentElement;
      if (!root || root === host || root.tagName === "FIELDSET") return null;
      return round(root.getBoundingClientRect().right);
    })(),
  };
}

test.describe("Data API — the Access row fits, and its identity is readable", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DATAAPI_ACCESS_ROW=1 with the live dev stack up");
  test.setTimeout(180_000);

  test("both row actions share a line, and a real address fits its column", async ({ page }) => {
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    let dbId: string | undefined;
    let endpointId: string | undefined;

    try {
      // A real row in `databases`, never queried — this section only READS, so
      // the throwaway connection never has to answer. Same note as the sibling
      // specs.
      const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
        name: `e2e-access-row-${runId}`,
        engine: "starrocks",
        host: "localhost",
        port: 9030,
        options: { user: "root" },
      });
      dbId = db.id as string;

      // `Wxyz`, for the reason the tab-widths spec gives: the shell renders the
      // endpoint's name beside the section's, and a descriptive fixture name is
      // one dropped `exact` away from satisfying a lookup it should not.
      const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
        name: `Wxyz ${runId}`,
        path: `e2e-access-row-${runId}`,
        database_id: dbId,
        statements: [{ name: "main", sql: "SELECT 1 AS total" }],
      });
      endpointId = endpoint.id as string;

      const accounts = NAMES.map((n, i) => ({
        id: `e2e-sa-${i}`,
        name: n,
        description: "",
        email: `svc.${n}@${ws.slug}.service.egret.local`,
        created_at: "2026-01-01T00:00:00Z",
      }));
      // Mixed invoke states on purpose: one row per state the actions cell has
      // (Revoke / Grant invoke / no mutation at all). A uniform fixture would
      // measure one arm of a three-way branch and report on all three.
      const grantedIds = new Set(["e2e-sa-0", "e2e-sa-3"]);
      const inheritedIds = new Set(["e2e-sa-4"]);

      await page.route(`**/api/ws/${ws.slug}/data-api/service-accounts`, (route) =>
        route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(accounts) }),
      );
      await page.route(
        `**/api/ws/${ws.slug}/data-api/endpoints/${endpointId}/grants`,
        (route) =>
          route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify(
              [...grantedIds].map((id) => ({
                id: `g-${id}`,
                service_account_id: id,
                service_account_name: accounts.find((a) => a.id === id)!.name,
                created_at: "2026-02-01T00:00:00Z",
              })),
            ),
          }),
      );
      // The invoke probe: a non-empty `paths` is the backend saying this
      // account may invoke, cascade included. Answered per account so the
      // "inherited from the workspace" row is a real state and not a guess.
      await page.route(`**/api/ws/${ws.slug}/data-api/openapi?**`, (route) => {
        const sa = new URL(route.request().url()).searchParams.get("service_account_id") ?? "";
        const yes = grantedIds.has(sa) || inheritedIds.has(sa);
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            openapi: "3.0.3",
            servers: [{ url: "https://egret.test/data-api/v1" }],
            paths: yes ? { "/x": { get: {} } } : {},
          }),
        });
      });

      // 1600×1000 is where the defect was measured and where §5.1's table of
      // tab widths was taken, so the numbers here are comparable to those.
      await page.setViewportSize({ width: 1600, height: 1000 });
      await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}/access`);
      await expect(
        page.getByRole("heading", { name: "Access", level: 2, exact: true }),
      ).toBeVisible({ timeout: 30_000 });
      await expect(page.getByTestId("access-row-e2e-sa-0")).toBeVisible();
      // The probes settle a render later; the badge is what they decide.
      await expect(page.getByText("Inherited from the workspace", { exact: true })).toBeVisible();

      const m = await page.evaluate(measure, OUTLIER);
      expect(m.error, "the Access table was measured").toBeNull();
      if (m.error) throw new Error(m.error);

      // The count guard for every loop below.
      expect(m.rows.length, "all seven accounts are on screen").toBe(NAMES.length);
      expect(
        m.tableScroll.scroll,
        "the table does not scroll sideways at 1600",
      ).toBeLessThanOrEqual(m.tableScroll.client);

      // ── 1. The two actions share a line ───────────────────────────────────
      let rowsWithTwoActions = 0;
      for (const row of m.rows) {
        expect(
          row.actions.overflows,
          `${row.address.text}: the actions cell does not overflow its column`,
        ).toBe(false);
        expect(
          row.actions.lines,
          `${row.address.text}: its actions [${row.actions.labels.join(", ")}] share one line`,
        ).toBe(1);
        if (row.actions.labels.length === 2) rowsWithTwoActions += 1;
      }
      // The vacuity guard: "one line" is trivially true of a row with one
      // control, and the inherited row genuinely has one. Six of the seven must
      // have two, or this section measured nothing.
      expect(
        rowsWithTwoActions,
        "six rows really do carry two actions (otherwise 'one line' is vacuous)",
      ).toBe(6);

      // ── 2. Try it is a peer of the mutation, and quieter ──────────────────
      const twoActionRow = m.rows.find((r) => r.actions.labels.length === 2)!;
      const link = twoActionRow.actions.paint.find((p) => p.tag === "A")!;
      const button = twoActionRow.actions.paint.find((p) => p.tag === "BUTTON")!;
      expect(link.label, "the link is Try it").toBe("Try it");
      // A PEER: same text size and same height, so it is not the smallest thing
      // in the row. Before: 12px/16px against 13px/32px.
      expect(link.fontSize, "Try it reads at the mutation's size").toBe(button.fontSize);
      expect(link.height, "Try it has the mutation's hit area").toBe(button.height);
      // …and QUIETER: no border and no fill, against the mutation's outline.
      expect(parseFloat(button.borderWidth), "the mutation carries a border").toBeGreaterThan(0);
      expect(parseFloat(link.borderWidth), "Try it carries none").toBe(0);
      expect(link.background, "Try it has no resting fill").toMatch(/rgba\(0, 0, 0, 0\)|transparent/);

      // ── 3. Try it sits at one offset down the page ───────────────────────
      // `min-w-28` on both mutations, measured: `Revoke` is 78px and
      // `Grant invoke` 104px, so without a floor the link to their left moves
      // 26px between rows — on the screen whose job is reading down a column.
      // Restricted to rows that HAVE a mutation; the inherited row's single
      // action takes the outer edge, which is §4.3 (do not hold a slot empty
      // for symmetry) and is deliberate.
      const linkLefts = new Set(
        m.rows
          .filter((r) => r.actions.labels.length === 2)
          .map((r) => r.actions.paint.find((p) => p.tag === "A")!.left),
      );
      expect([...linkLefts], "Try it is at the same offset on every row that has a mutation").toHaveLength(1);

      // ── 4. A real address fits the column ─────────────────────────────────
      let fitted = 0;
      for (const row of m.rows) {
        if (row.address.isOutlier) {
          // Named and asserted, not skipped: this is the case `title` still
          // exists for, and a spec that quietly excluded it would be claiming
          // the column holds everything.
          expect(
            row.address.natural,
            `${row.address.text}: the outlier really is longer than the column`,
          ).toBeGreaterThan(row.address.available);
          continue;
        }
        // The vacuity guard, per row: an address that would have fitted `w-48`
        // proves nothing about widening the column.
        expect(
          row.address.natural,
          `${row.address.text}: would NOT have fitted the old w-48 column`,
        ).toBeGreaterThan(OLD_CONTENT_WIDTH);
        expect(
          row.address.natural,
          `${row.address.text}: fits the column it is in`,
        ).toBeLessThanOrEqual(row.address.available);
        fitted += 1;
      }
      expect(fitted, "six of the seven addresses fit").toBe(6);

      // ── 5. The prose is capped, though the section is not ────────────────
      expect(m.sectionRight, "the section root was resolved by the walk").not.toBeNull();
      expect(m.prose.length, "the section's standing prose is capped").toBeGreaterThan(0);
      for (const p of m.prose) {
        expect(p.right, "capped prose stops short of the section").toBeLessThan(m.sectionRight!);
        // A measure, not merely "narrower than the page": `max-w-prose` is 65ch
        // and lands near 532px here. 700 is loose enough to survive a font
        // change and tight enough to fail a cap that was deleted.
        expect(p.width, "capped prose keeps a reading measure").toBeLessThan(700);
      }
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
