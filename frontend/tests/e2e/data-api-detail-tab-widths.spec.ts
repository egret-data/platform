import { test, expect } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * The claim `docs/en/ui-conventions.md` §5.1 makes after its reversal: **the
 * six tabs of the endpoint detail route agree on a width.**
 *
 * # Why this cannot be a jsdom test, at all
 *
 * Not "is weaker in jsdom" — cannot be. jsdom implements no CSS (§10), so
 * every box is 0×0 and `expectSectionLeavesWidthToThePage` compares a STRING.
 * Six passing string comparisons say six roots carry no `max-w-*`; they do not
 * say the six render at the same width, because a section can reach a
 * different width through a flex parent, a grid track, a child that overflows
 * its container, or a `w-*` further down. The rule is about geometry across
 * SIBLING ROUTES, and only a browser has both.
 *
 * Measured before the change on the deployed build at 1600×1000 with seeded
 * data — the numbers §5.1 records, and the reason the caps came off:
 *
 *   tab        available   widest block
 *   Query        1296          1296
 *   Config       1296          1296
 *   Response     1296          1296
 *   Access       1296          1024   ← `max-w-5xl`
 *   Try it       1296          1024   ← `max-w-5xl`
 *   Versions     1296          1024   ← `max-w-5xl`
 *
 * A 272px jump on three of the five tab changes.
 *
 * # Two ways the sibling spec went green on a broken page
 *
 * Both were found live in `data-api-response-width.spec.ts`, and this file is
 * built so neither can recur rather than merely remembering them:
 *
 * 1. **A counted DOM hop.** That spec first reached the section root with
 *    `parentElement.parentElement`. ONE extra wrapper `div` inside a section
 *    re-points that at the HOST — and then `root === available` is
 *    tautologically true and the spec passes on a section that has regressed.
 *    Here the root is reached by a WALK that stops at the child of the host,
 *    and landing on the host is a hard failure (`root.tagName === "FIELDSET"`),
 *    never a silent pass. Same rule as `__tests__/sectionWidth.ts`.
 *
 * 2. **A `getByRole` name that matched by substring.** Playwright matches
 *    accessible names case-insensitively by SUBSTRING unless told otherwise.
 *    `name: "Response"` also matched that spec's own fixture, `E2E response
 *    width`, and the run died on a strict-mode violation before measuring
 *    anything. Every heading lookup here passes `exact: true`, AND the fixture
 *    is named `Wxyz` — a string that is not a substring of any of the six tab
 *    names, so an accidental loose match would break the run instead of being
 *    absorbed by a tidy fixture name.
 *
 * And the general one: **assert the count before iterating.** A loop over a
 * selector that has stopped matching asserts nothing and reports green — so
 * this spec checks it measured SIX tabs before comparing them, and fails if
 * any single tab could not be measured.
 *
 * # What "widest block" means here
 *
 * The right edge of the widest FRAME the section renders — its own root and
 * every table it contains. Right edges rather than widths, for the reason the
 * sibling spec gives: two blocks of equal width at different offsets satisfy a
 * width check and still read as ragged.
 *
 * `p.max-w-prose` is excluded on purpose and is not an oversight: §5.2 caps a
 * line of running text at its own readable measure, which is the ONE thing
 * §5.1's "one width" does not govern. That exclusion is itself asserted — a
 * spec that only compared frames would pass on a section that had widened its
 * paragraphs too, which is the obvious wrong fix.
 *
 * # What has and has not been run against this file
 *
 * **This spec has not yet been executed against a live seeded stack.** When it
 * was written the backend was not running in the authoring environment (Vite
 * on :5173, `/api` proxying to a dead :8080), and standing one up is a Rust
 * build outside a frontend-only task. It compiles, `--list` enumerates it, and
 * it is committed as the regression artifact — but "it passes" is NOT among
 * the claims made for it, and the first person with the stack up should run it
 * before trusting it.
 *
 * The geometry claim itself WAS verified in a real browser, by a throwaway
 * harness that drove headless Chromium against the same Vite dev server with
 * the API stubbed at the network layer (`page.route`). Real Chromium, real
 * shipped stylesheet, real components; only the JSON was synthetic. Viewport
 * 1600×1000. The numbers below are RIGHT EDGES, not widths:
 *
 *   after  — Query 1560, Config 1560, Response 1560, Access 1560, Try it 1560, Versions 1560
 *   before — Query 1560, Config 1560, Response 1560, Access 1288, Try it 1288, Versions 1288
 *
 * An earlier revision of this comment glossed the 1560 as "wider than the
 * deployed 1296 because the harness has no seeded sidebar". **That was wrong,
 * and re-measuring is what showed it.** The content box measures L=264,
 * R=1560, WIDTH=**1296** on all six tabs — the deployed figure exactly. 1560
 * and 1288 are right edges in viewport coordinates and they tie to §5.4b's own
 * table: a root capped at `max-w-5xl` starts at 264 and ends at 264+1024=1288.
 * The harness REPRODUCES the deployed geometry rather than approximating it,
 * which is a stronger claim than the gloss made — and the same arithmetic is
 * what caught §5.4b's 1576 as the one wrong number in that table.
 *
 * The "before" row is a **negative control**, not a recollection: the three
 * `max-w-5xl`s were put back, the page re-measured, and the 272px split
 * reappeared exactly where §5.1's table says it was. Without that control the
 * "after" row would be six numbers agreeing for reasons unknown — which is
 * what a harness that cannot see the defect also produces.
 *
 *   EGRET_E2E_DATAAPI_WIDTH=1 npx playwright test data-api-detail-tab-widths
 */
const ENABLED = process.env.EGRET_E2E_DATAAPI_WIDTH === "1";

/**
 * The six tabs, by URL segment and by the `h2` each renders.
 *
 * The index route is `Query` (`App.tsx` mounts it with `index`), so its
 * segment is the empty string.
 */
const TABS = [
  { segment: "", heading: "Query" },
  { segment: "/config", heading: "Config" },
  { segment: "/response", heading: "Response" },
  { segment: "/access", heading: "Access" },
  { segment: "/try", heading: "Try it" },
  { segment: "/versions", heading: "Versions" },
] as const;

test.describe("Data API — the six detail tabs are on one width", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DATAAPI_WIDTH=1 with the live dev stack up");
  test.setTimeout(240_000);

  test("every tab's widest frame ends at the same right edge as the page gives it", async ({ page }) => {
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    let dbId: string | undefined;
    let endpointId: string | undefined;

    try {
      // A real row in `databases`, never queried: these six sections only READ
      // the endpoint and its versions, so the throwaway connection never has
      // to answer. Same note as the sibling specs.
      const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
        name: `e2e-tab-widths-${runId}`,
        engine: "starrocks",
        host: "localhost",
        port: 9030,
        options: { user: "root" },
      });
      dbId = db.id as string;

      // `Wxyz`: deliberately NOT descriptive. The shell renders the endpoint's
      // own name as an `h2` beside each section's, so a fixture named
      // "E2E tab widths" would be a substring match away from satisfying
      // `name: "Try it"`-style lookups if `exact` were ever dropped. A name
      // sharing no substring with Query/Config/Response/Access/Try it/Versions
      // turns that regression into a failure rather than a green run.
      const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
        name: `Wxyz ${runId}`,
        path: `e2e-tab-widths-${runId}`,
        database_id: dbId,
        statements: [{ name: "main", sql: "SELECT 1 AS total_revenue" }],
      });
      endpointId = endpoint.id as string;

      const base = `/explore/${ws.slug}/data-api/endpoints/${endpointId}`;

      await page.setViewportSize({ width: 1600, height: 1000 });

      /**
       * The right edge the route gives a section, the right edge of the
       * section's own root, and of every FRAME inside it — plus the prose,
       * separately, because it is the deliberate exception.
       */
      const measure = (headingText: string) =>
        page.evaluate((wanted) => {
          const round = (n: number) => Math.round(n);
          const right = (el: Element) => round(el.getBoundingClientRect().right);

          // The SECTION's `h2`, not the shell's. The shell renders the
          // endpoint's name as an `h2` too, so this matches on exact text.
          const heading = [...document.querySelectorAll("h2")].find(
            (h2) => h2.textContent?.trim() === wanted,
          );
          if (!heading) return { error: `no h2 with exact text "${wanted}"` };

          const routeRoot = document.querySelector<HTMLElement>("main .p-4");
          if (!routeRoot) return { error: "no route root (main .p-4)" };
          const host = routeRoot.querySelector<HTMLElement>("fieldset") ?? routeRoot;

          // A WALK, never `parentElement.parentElement` — see the file header,
          // trap 1. Landing on the host is a hard error below.
          let root: HTMLElement | null = heading.parentElement;
          while (root && root.parentElement !== host) root = root.parentElement;
          if (!root) return { error: `walk from "${wanted}" never reached the host` };
          if (root === host || root.tagName === "FIELDSET") {
            return { error: `walk from "${wanted}" overshot onto the host` };
          }

          const hostStyle = getComputedStyle(host);
          const available = round(
            host.getBoundingClientRect().right - parseFloat(hostStyle.paddingRight),
          );

          // Every frame the section renders: its own root, and every table at
          // any depth. Tables are the blocks that actually pin a right edge;
          // `div`s inherit theirs from the root.
          const frames: { what: string; right: number }[] = [
            { what: "section root", right: right(root) },
          ];
          [...root.querySelectorAll("table")].forEach((t, i) => {
            // A table inside a horizontally-scrolling wrapper reports the
            // width of its CONTENT, not of the space it was given — it can
            // legitimately exceed `available` at narrow viewports (§5.3). At
            // 1600 none of the six scroll, and the assertion below is that
            // they do not: a table wider than the page is a real defect here.
            frames.push({ what: `table ${i}`, right: right(t) });
          });

          return {
            available,
            frames,
            proseRights: [...root.querySelectorAll("p.max-w-prose")].map(right),
          };
        }, headingText);

      const measured: { tab: string; available: number; widest: number }[] = [];

      for (const { segment, heading } of TABS) {
        await page.goto(`${base}${segment}`);
        // `exact: true` — see the file header, trap 2.
        await expect(
          page.getByRole("heading", { name: heading, level: 2, exact: true }),
        ).toBeVisible({ timeout: 30_000 });
        // The tab's own content, not the previous tab's still on screen.
        await page.waitForTimeout(400);

        const m = await measure(heading);
        expect(m, `${heading}: measured`).toBeTruthy();
        expect(
          "error" in m ? m.error : null,
          `${heading}: resolved its section root`,
        ).toBeNull();
        if ("error" in m) throw new Error(`${heading}: ${m.error}`);

        // Count before comparing: a section with no frames would otherwise
        // contribute a vacuous pass to the cross-tab check below.
        expect(m.frames.length, `${heading}: found at least the section root`).toBeGreaterThan(0);

        for (const f of m.frames) {
          expect(
            f.right,
            `${heading}: ${f.what} does not overflow the page`,
          ).toBeLessThanOrEqual(m.available);
        }

        // §5.1, per tab: the section takes the width it is given.
        //
        // 1px of slack, and the reasoning is worth stating because the sibling
        // spec uses exact equality. These are two INDEPENDENTLY ROUNDED edges:
        // `available` subtracts a parsed `padding-right` from one fractional
        // rect, `root.right` rounds a different fractional rect. Sub-pixel
        // layout — a fractional scrollbar, a zoom level, a font metric that
        // shifts a flex line — can leave them 1 apart while the section is
        // taking exactly the width it was given. This file has never executed
        // against a live stack, so exact equality here would be a spurious
        // failure waiting for its first run rather than a real finding. The
        // defect being guarded is 272px; 1px of tolerance cannot hide it.
        const root = m.frames[0]!;
        expect(
          Math.abs(root.right - m.available),
          `${heading}: the section leaves no gutter (root ${root.right} vs available ${m.available})`,
        ).toBeLessThanOrEqual(1);

        // The exception, asserted rather than skipped (§5.2): prose stops at
        // its own reading measure while the frames take the page.
        //
        // COUNTED FIRST, and that is not boilerplate here. An earlier revision
        // of this file left the loop below unguarded on the reasoning that
        // "not every tab has capped prose" — which made it a `for` over a
        // possibly-empty list, the exact vacuous-pass this file's own header
        // warns about, in the file that warns about it. It is now false that a
        // tab might have none: every one of the six section intros carries
        // `max-w-prose` (§5.1), so an intro losing its cap makes this list
        // shorter and fails HERE rather than passing silently.
        expect(
          m.proseRights.length,
          `${heading}: found the section's capped prose (its intro at minimum)`,
        ).toBeGreaterThan(0);
        for (const r of m.proseRights) {
          expect(r, `${heading}: prose keeps its own reading measure`).toBeLessThan(root.right);
        }

        measured.push({
          tab: heading,
          available: m.available,
          widest: Math.max(...m.frames.map((f) => f.right)),
        });
      }

      // The count guard for the OUTER loop: six tabs, or the cross-tab claim
      // below is made about fewer than it says.
      expect(measured.length, "measured all six tabs").toBe(TABS.length);

      // The claim itself, and the one no jsdom test can make: the six agree.
      //
      // Reported as a SPREAD rather than a pairwise walk, so one failure
      // message carries every tab and its number instead of dying on the first
      // mismatched pair — and so the number in the message is the thing §5.1 is
      // about (how far apart the tabs are) rather than an index.
      //
      // Same 1px tolerance and same reasoning as the per-tab check above: six
      // independently rounded edges, a defect measured in hundreds of pixels.
      // The spread was 272 before this rule; 1px cannot hide that.
      const edges = measured.map((m) => m.widest);
      const spread = Math.max(...edges) - Math.min(...edges);
      expect(
        spread,
        `all six tabs end at the same right edge — measured: ${measured
          .map((m) => `${m.tab}=${m.widest}/${m.available}`)
          .join(", ")}`,
      ).toBeLessThanOrEqual(1);
    } finally {
      // Best-effort: a failed assertion must not leave the shared workspace
      // holding a throwaway endpoint or connection.
      if (endpointId) {
        await page.request
          .delete(`/api/ws/${ws.slug}/data-api/endpoints/${endpointId}`, { headers: h })
          .catch(() => {});
      }
      if (dbId) {
        await page.request
          .delete(`/api/ws/${ws.slug}/databases/${dbId}`, { headers: h })
          .catch(() => {});
      }
    }
  });
});
