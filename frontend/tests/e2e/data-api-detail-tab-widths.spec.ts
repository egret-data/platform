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
 * **It has been executed against a live seeded stack (kind, 2026-08-29) and it
 * passes** — 5.2s, six tabs, `available = root right = widest frame = 1560` on
 * every one. Until then it had not: the backend was not running in the
 * authoring environment (Vite on :5173, `/api` proxying to a dead :8080), so
 * it was committed as the regression artifact with "it passes" explicitly NOT
 * among its claims. It is now among them.
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
/**
 * Two viewports, and the second one is the point.
 *
 * 1600 is where §5.1's table was taken. 1100 is BELOW the `xl` breakpoint, and
 * until the live check nothing in this branch drove it — while both row fixes
 * are `xl`-gated (`Versions.tsx:228` `w-56 xl:w-80`, `AccessSection.tsx:344`
 * `w-48 xl:w-96`), so under 1280px the tabs render a different page than the
 * one every spec measured. The width rule is supposed to hold at BOTH; that is
 * exactly the claim a single-viewport spec cannot make.
 */
const VIEWPORTS = [1600, 1100] as const;

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

          // The section's INTRO specifically, resolved by identity: the `<p>`
          // beside the section's own `h2`. Not "some capped paragraph in the
          // section" — see the assertion site for why that distinction is the
          // whole point.
          const introEl = heading.parentElement?.querySelector("p") ?? null;

          return {
            available,
            frames,
            // Reported separately from `frames`, which is seeded with the root
            // and therefore can never be empty — see the assertion site.
            tableCount: root.querySelectorAll("table").length,
            intro: introEl
              ? {
                  right: right(introEl),
                  // MEASURED, not read off `className`. `capped` below is a
                  // class assertion in a file whose own header says a class
                  // assertion is not the claim, and it is wrong in two
                  // directions: a later `max-w-none` overrides the cap and the
                  // regex still matches, and a `display:none` intro reports
                  // width 0 and right 0 so the `<` comparison passes too. The
                  // width is what the rule is about; the class is kept only as
                  // a named canary for a silent revert.
                  width: round(introEl.getBoundingClientRect().width),
                  capped: /(?:^|\s)max-w-prose(?:\s|$)/.test(introEl.className),
                }
              : null,
            proseRights: [...root.querySelectorAll("p.max-w-prose")].map(right),
          };
        }, headingText);

      for (const width of VIEWPORTS) {
        await page.setViewportSize({ width, height: 1000 });
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

          // Count before comparing — and count THE THING THAT CAN VANISH.
          //
          // This assertion used to read `m.frames.length > 0`, described as
          // catching "a section with no frames". IT COULD NEVER FAIL: `frames` is
          // seeded unconditionally with the section root a few lines above, so
          // the length is at least 1 by construction, whatever the page did. The
          // state it was meant to catch is the `querySelectorAll("table")` beside
          // that seed matching NOTHING — at which point the loop below compares
          // one box with itself and a tab whose grids had all disappeared would
          // still report green. So the tables are counted separately, and all six
          // tabs render at least one (measured live: Query 2, Config 1,
          // Response 2, Access 1, Try it 2, Versions 1).
          //
          // Same family as the prose guard below and as the two count guards in
          // the sibling specs: a number that a population satisfies for free is
          // not a guard.
          expect(m.frames.length, `${heading}: found at least the section root`).toBeGreaterThan(0);
          expect(
            m.tableCount,
            `${heading}: the section still renders a grid to measure`,
          ).toBeGreaterThan(0);

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
          // taking exactly the width it was given.
          //
          // The slack is kept for that reason ALONE, and the reason it used to
          // give is now false: this file HAS run against a live kind stack, and
          // `available == root.right` held EXACTLY on all six tabs at 1600, 900
          // and 700 — zero gutter, so exact equality would have passed. Kept
          // because the environments this has run in are two, not because it has
          // run in none. The defect guarded is 272px; 1px cannot hide it.
          const root = m.frames[0]!;
          expect(
            Math.abs(root.right - m.available),
            `${heading}: the section leaves no gutter (root ${root.right} vs available ${m.available})`,
          ).toBeLessThanOrEqual(1);

          // The exception, asserted rather than skipped (§5.2): prose stops at
          // its own reading measure while the frames take the page.
          //
          // BY IDENTITY, NOT BY POPULATION — and this file has now got that wrong
          // twice, in opposite ways, which is why the reasoning is written out.
          //
          //  1. First revision: no guard at all on the loop below, because "not
          //     every tab has capped prose". A `for` over a possibly-empty list
          //     asserts nothing — the vacuous pass this file's own header warns
          //     about, in the file that warns about it.
          //  2. Second revision: `proseRights.length > 0`, claiming an intro
          //     losing its cap would shorten the list and fail here. **Also
          //     false.** `Response` and `Access` each render THREE unconditional
          //     `max-w-prose` paragraphs, so deleting the cap from the intro
          //     leaves two behind and the count still passes. Only `Query`, with
          //     exactly one, was ever genuinely guarded — a guard that holds for
          //     one of six looks identical to one that holds for all six.
          //
          // A count over a population can be satisfied by any member of it. The
          // intro is therefore resolved as a specific ELEMENT — the `<p>` beside
          // this section's `h2` — and asserted on directly, so only the thing the
          // rule is about can satisfy it. Same reasoning as
          // `__tests__/sectionWidth.ts`'s `expectSectionIntroCapsItsProse`, which
          // is the jsdom half of this guard.
          expect(m.intro, `${heading}: found the section's intro paragraph`).not.toBeNull();
          // Geometry first: on screen at all, and held to a reading measure. 700
          // is the same bound the sibling specs use — loose enough to survive a
          // font change, tight enough that an uncapped intro (which would take
          // the section's full 1560) fails.
          expect(m.intro!.width, `${heading}: the intro is laid out at all`).toBeGreaterThan(0);
          expect(
            m.intro!.width,
            `${heading}: the intro keeps a reading measure`,
          ).toBeLessThan(700);
          expect(
            m.intro!.right,
            `${heading}: the intro stops short of the section's measure`,
          ).toBeLessThan(root.right);
          // The canary, asserted last and labelled as such — see `measure`.
          expect(m.intro!.capped, `${heading}: the intro still carries max-w-prose (§5.1)`).toBe(true);

          // The remaining capped paragraphs, still asserted — widening them is
          // the obvious wrong fix and would otherwise pass.
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
          `at ${width}px all six tabs end at the same right edge — measured: ${measured
            .map((m) => `${m.tab}=${m.widest}/${m.available}`)
            .join(", ")}`,
        ).toBeLessThanOrEqual(1);
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
        await page.request
          .delete(`/api/ws/${ws.slug}/databases/${dbId}`, { headers: h })
          .catch(() => {});
      }
    }
  });
});
