import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * **The `Try it` tab shows the parameters it is asking the caller to fill.**
 *
 * # Why this cannot be a jsdom test, at all
 *
 * Not "is weaker in jsdom" — cannot be. jsdom implements no CSS layout (§10):
 * every box is 0x0, nothing overflows and nothing scrolls, so `clientHeight`
 * and `scrollHeight` are both 0 and *every* assertion below passes whether the
 * section clips or not. `__tests__/TryIt.test.tsx` pins the CLASS CHAIN that
 * produces the geometry — no scrolling box between the parameter table and the
 * section root, no `Panel` around it — which is the strongest thing that
 * environment can say. This file is the geometry.
 *
 * # What was measured, and what it looked like
 *
 * The section used to carry `Query`'s shape: a vertical `Group` whose two
 * panels held the setup and the result at a fixed `40%` / `60%`. Measured in
 * real Chromium at 1600x1000, the setup pane's `clientHeight` was **230 no
 * matter what it held**:
 *
 *   declared params   clientHeight   scrollHeight   hidden
 *          0              230            277          47
 *          1              230            297          67
 *          2              230            342         112
 *          4              230            432         202
 *          8              230            612         382
 *
 * The first row is the one that settles it: the pane clipped its own content on
 * an endpoint that declares NO parameters, so there was no fixture for which
 * the fraction was right. On a four-parameter endpoint the `Parameter | Type |
 * Value` header row was sliced through the middle and not one parameter row was
 * on screen, while the result pane below it — 389px — held a header and one
 * collapsed caveat.
 *
 * After the change, on the same harness at the same viewport, no element in the
 * section hides anything before a run, and every parameter row is inside the
 * shell's visible box at 0, 1, 2, 4 and 8 parameters. That "before" row is a
 * **negative control that was actually run**, not a recollection: it is the
 * same script against the same dev server at the previous commit.
 *
 * # The traps this file is built to avoid
 *
 * Three specs on this branch have gone green on a broken page, and all three
 * failure modes are reachable from here:
 *
 * 1. **A counted DOM hop.** `parentElement.parentElement` re-points at the HOST
 *    the moment one wrapper `div` is added inside the section, and then the
 *    assertions are about the fieldset. The section root is reached by a WALK
 *    that stops at the child of the host, and landing on the host is a hard
 *    failure.
 * 2. **A `getByRole` name matching by substring.** Playwright matches
 *    accessible names case-insensitively by substring unless told otherwise, and
 *    a fixture named "E2E try it height" would satisfy `name: "Try it"`. Every
 *    heading lookup here is `exact: true`, AND the fixture is named `Wxyz`,
 *    which is not a substring of any tab name — so a dropped `exact` breaks the
 *    run rather than being absorbed by a tidy fixture name.
 * 3. **A degenerate fixture.** One parameter cannot tell a clipped table from a
 *    fitting one: the old pane had room for a header row and most of one row.
 *    This endpoint declares **six**, and that count is asserted BY IDENTITY —
 *    each declared name must resolve to its own row — before anything is
 *    measured, so a fixture that quietly lost its parameters fails here instead
 *    of making every measurement below vacuous.
 *
 * # What has and has not been run against this file
 *
 * **This spec has not been executed against a live seeded stack.** The backend
 * was not running in the authoring environment (Vite on :5173, `/api` proxying
 * to a dead :8080) and standing one up is a Rust build outside a frontend-only
 * task. It compiles and `--list` enumerates it. "It passes" is NOT among the
 * claims made for it — the same limitation `data-api-detail-tab-widths.spec.ts`
 * records, and the first person with the stack up should run it.
 *
 * The geometry claims WERE verified in a real browser, by a throwaway harness
 * driving headless Chromium against the same Vite dev server with the API
 * stubbed at the network layer (`page.route`). Real Chromium, real shipped
 * stylesheet, real components; only the JSON was synthetic. The numbers above
 * and the cross-tab column figures below come from it.
 *
 *   EGRET_E2E_DATAAPI_TRYIT=1 npx playwright test data-api-try-it-height
 */
const ENABLED = process.env.EGRET_E2E_DATAAPI_TRYIT === "1";

/**
 * Six parameters, and six is not decoration.
 *
 * The defect hid 47px on an endpoint with none and 202px on one with four; a
 * fixture small enough to fit inside the old pane would measure the same before
 * and after the fix. Six also keeps the setup under the section's own height at
 * 1600x1000 (measured: the last row's bottom lands at 817 against a visible box
 * ending at 927), so the "every row is on screen" assertion is about the
 * section's layout and not about whether the page happened to be scrolled.
 */
const PARAMS = [
  { name: "region", type: "string", location: "query", required: true },
  { name: "channel", type: "string", location: "query", required: false },
  { name: "segment", type: "string", location: "query", required: false },
  { name: "currency", type: "string", location: "query", required: false },
  { name: "since", type: "date", location: "query", required: false },
  { name: "rowlimit", type: "number", location: "query", required: false },
] as const;

test.describe("Data API — Try it shows the parameters it asks for", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DATAAPI_TRYIT=1 with the live dev stack up");
  test.setTimeout(240_000);

  test("no parameter row is clipped, and the section's only scroll box is the result", async ({ page }) => {
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    let dbId: string | undefined;
    let endpointId: string | undefined;

    try {
      // A real row in `databases`, never queried: this tab only READS the
      // endpoint until Run is pressed, and Run is not pressed here. Same note as
      // the sibling specs.
      const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
        name: `e2e-tryit-height-${runId}`,
        engine: "starrocks",
        host: "localhost",
        port: 9030,
        options: { user: "root" },
      });
      dbId = db.id as string;

      // `Wxyz`: deliberately NOT descriptive — see trap 2 in the header.
      const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
        name: `Wxyz ${runId}`,
        path: `e2e-tryit-height-${runId}`,
        database_id: dbId,
        statements: [{ name: "main", sql: "SELECT 1 AS total_revenue" }],
        params: PARAMS,
      });
      endpointId = endpoint.id as string;

      await page.setViewportSize({ width: 1600, height: 1000 });
      await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}/try`);
      await expect(
        page.getByRole("heading", { name: "Try it", level: 2, exact: true }),
      ).toBeVisible({ timeout: 30_000 });
      // The tab's own content, not the previous route's still on screen.
      await page.waitForTimeout(400);

      const measured = await page.evaluate(() => {
        const round = (n: number) => Math.round(n);
        const box = (el: Element) => {
          const b = el.getBoundingClientRect();
          return { top: round(b.top), bottom: round(b.bottom), height: round(b.height) };
        };

        // The SECTION's `h2`, not the shell's — the shell renders the
        // endpoint's name as an `h2` too, so this matches on exact text.
        const heading = [...document.querySelectorAll("h2")].find(
          (h2) => h2.textContent?.trim() === "Try it",
        );
        if (!heading) return { error: 'no h2 with exact text "Try it"' };

        const routeRoot = document.querySelector<HTMLElement>("main .p-4");
        if (!routeRoot) return { error: "no route root (main .p-4)" };
        const host = routeRoot.querySelector<HTMLElement>("fieldset") ?? routeRoot;

        // A WALK, never a counted hop — see trap 1.
        let root: HTMLElement | null = heading.parentElement;
        while (root && root.parentElement !== host) root = root.parentElement;
        if (!root) return { error: 'walk from "Try it" never reached the host' };
        if (root === host || root.tagName === "FIELDSET") {
          return { error: 'walk from "Try it" overshot onto the host' };
        }

        // The two halves, by their accessible names rather than by position.
        const named = (name: string) =>
          [...root!.querySelectorAll<HTMLElement>("section[aria-labelledby]")].find(
            (s) => document.getElementById(s.getAttribute("aria-labelledby")!)?.textContent?.trim() === name,
          ) ?? null;
        const setup = named("Run as");
        const result = named("Result");
        if (!setup || !result) return { error: `missing half: setup=${!!setup} result=${!!result}` };

        // The parameter table, resolved from its own heading.
        const h4 = [...root.querySelectorAll("h4")].find(
          (x) => x.textContent?.trim() === "Parameter values",
        );
        if (!h4) return { error: "no 'Parameter values' heading" };
        const table = h4.parentElement?.querySelector("table") ?? null;
        if (!table) return { error: "'Parameter values' introduces no table" };

        const hostBox = host.getBoundingClientRect();
        const rows = [...table.querySelectorAll("thead tr, tbody tr")].map((tr) => ({
          text: (tr as HTMLElement).innerText.replace(/\s+/g, " ").trim(),
          ...box(tr),
          // Inside the host's VISIBLE box, which is what "not clipped" means for
          // a row: the host is the shell's `overflow-auto` fieldset.
          visible:
            tr.getBoundingClientRect().top >= hostBox.top - 1 &&
            tr.getBoundingClientRect().bottom <= hostBox.bottom + 1,
        }));

        // Anything in the section hiding content behind its own scrollbar.
        const hiding: { where: string; cls: string; clientHeight: number; scrollHeight: number }[] = [];
        for (const el of root.querySelectorAll<HTMLElement>("*")) {
          if (el.clientHeight <= 0) continue;
          if (el.scrollHeight - el.clientHeight <= 2) continue;
          const oy = getComputedStyle(el).overflowY;
          if (oy !== "auto" && oy !== "scroll" && oy !== "hidden") continue;
          hiding.push({
            where: result.contains(el) ? "result" : setup.contains(el) ? "setup" : "section",
            cls: el.getAttribute("class") ?? "",
            clientHeight: el.clientHeight,
            scrollHeight: el.scrollHeight,
          });
        }

        // The `Setting | Details | Value` grid's columns, for the cross-tab
        // comparison below.
        const cols = (() => {
          const th = [...root.querySelectorAll("th")].find((t) => t.textContent?.trim() === "Setting");
          if (!th) return null;
          const tbl = th.closest("table")!;
          const pick = (name: string) => {
            const c = [...tbl.querySelectorAll("th")].find((t) => t.textContent?.trim() === name);
            return c ? round(c.getBoundingClientRect().width) : null;
          };
          return { setting: pick("Setting"), details: pick("Details"), value: pick("Value") };
        })();

        return {
          host: { top: round(hostBox.top), bottom: round(hostBox.bottom) },
          setup: box(setup),
          result: box(result),
          rows,
          hiding,
          cols,
        };
      });

      expect("error" in measured ? measured.error : null, "resolved the section").toBeNull();
      if ("error" in measured) throw new Error(measured.error as string);

      // ── The fixture is not degenerate ──────────────────────────────────────
      // By IDENTITY, before anything is measured: each declared parameter has
      // its own row. A count alone would be satisfied by six copies of row 0,
      // and a population that never empties cannot fail (trap 3).
      const bodyRows = measured.rows.slice(1);
      expect(bodyRows.length, "one row per declared parameter, plus the header").toBe(PARAMS.length);
      for (const p of PARAMS) {
        const own = bodyRows.filter((r) => new RegExp(`(^|\\s)${p.name}(\\s|\\*|$)`).test(r.text));
        expect(own.length, `exactly one row for "${p.name}" — rows: ${bodyRows.map((r) => r.text).join(" / ")}`).toBe(1);
      }
      // Distinct rows, not one row measured six times.
      expect(new Set(bodyRows.map((r) => r.top)).size, "the six rows are at six different offsets").toBe(
        PARAMS.length,
      );

      // ── The claim ─────────────────────────────────────────────────────────
      // Every row on screen, header included. The header row is the one the
      // defect sliced through the middle.
      for (const r of measured.rows) {
        expect(
          r.visible,
          `row "${r.text}" is inside the visible box (row ${r.top}-${r.bottom}, host ${measured.host.top}-${measured.host.bottom})`,
        ).toBe(true);
      }

      // Nothing in the section hides content before a run — and if anything ever
      // does, it may only be the result, which holds an unbounded JSON body.
      for (const hEl of measured.hiding) {
        expect(
          hEl.where,
          `something outside the result hides ${hEl.scrollHeight - hEl.clientHeight}px: ${hEl.cls}`,
        ).toBe("result");
      }
      expect(
        measured.hiding.filter((x) => x.where === "setup").length,
        "the setup hides nothing",
      ).toBe(0);

      // The setup takes its own height rather than a share of the section: it
      // ends where its content ends, which is above the result's top.
      expect(measured.setup.bottom, "the setup ends before the result begins").toBeLessThanOrEqual(
        measured.result.top,
      );

      // ── §5.2: the same settings grid as the tab next door ─────────────────
      // A cross-tab geometry claim, which is the kind only a browser can make.
      // `Try it` carried `w-52 lg:w-80` while `Config` and `Response` carried
      // `w-52 lg:w-96`, so the picker sat at 320px and `Details` — the only
      // unsized column under `table-fixed` — held 680, of which 148px could
      // never be used (its sentence is capped at `max-w-prose` and laid out at
      // 532). Measured after: Setting 288 / Details 616 / Value 384 here, and
      // 288 / 624 / 384 on `Response`.
      //
      // **The SIZED columns are what is compared, and the 8px on `Details` is
      // not an oversight.** `Try it` insets both of its halves by `px-1` (a
      // habit from when they were scrolling panels, so a focus ring was not
      // clipped by the scroll box), which makes its table 1288 wide against
      // Response's 1296. `Details` is the unsized remainder, so it absorbs that
      // 8px by construction; asserting it equal would be asserting the two
      // sections have identical padding, which is a different rule this task
      // did not change. `Setting` and `Value` are the ones §5.2 is about — they
      // are set by class and they must agree.
      const tryCols = measured.cols;
      expect(tryCols, "found the Setting | Details | Value grid").not.toBeNull();

      await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}/response`);
      await expect(
        page.getByRole("heading", { name: "Response", level: 2, exact: true }),
      ).toBeVisible({ timeout: 30_000 });
      await page.waitForTimeout(400);
      const responseCols = await page.evaluate(() => {
        const round = (n: number) => Math.round(n);
        const th = [...document.querySelectorAll("th")].find((t) => t.textContent?.trim() === "Setting");
        if (!th) return null;
        const tbl = th.closest("table")!;
        const pick = (name: string) => {
          const c = [...tbl.querySelectorAll("th")].find((t) => t.textContent?.trim() === name);
          return c ? round(c.getBoundingClientRect().width) : null;
        };
        return { setting: pick("Setting"), details: pick("Details"), value: pick("Value") };
      });
      expect(responseCols, "found Response's settings grid").not.toBeNull();
      // Equality, not "wide enough": the point of taking 384 rather than the
      // locally-optimal 448 is that the adjacent tabs render ONE grid. A
      // `toBeGreaterThan(320)` would also pass on 448 and on 512, which is
      // exactly the drift this asserts against.
      expect(
        { setting: tryCols!.setting, value: tryCols!.value },
        `Try it ${JSON.stringify(tryCols)} vs Response ${JSON.stringify(responseCols)}`,
      ).toEqual({ setting: responseCols!.setting, value: responseCols!.value });
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
