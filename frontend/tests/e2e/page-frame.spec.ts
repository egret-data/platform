import { test, expect, type Page } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace } from "./helpers";

/**
 * The page frame, measured: one title scale and one left edge across the
 * layout types (KI-214, KI-215; `docs/en/frontend-rules.md` FE-PAGE-6/7).
 *
 * Modelled on `data-api-detail-tab-widths.spec.ts`, and for the same reason it
 * is a browser spec at all: jsdom has no layout, so a class assertion says a
 * root carries `max-w-page`, not that the page renders 960px wide on the
 * list's left edge.
 *
 * One representative page per layout type:
 *
 *   type                 page                         title   width
 *   list                 Explore → Datasets            20px    full
 *   form / editor        Alerts → New alert            20px    960px column
 *   Control Plane        Control Plane → Workspaces    20px    full
 *   Workspace settings   Settings → General            20px    960px pane
 *   compact tool header  SQL Lab                       16px    full
 *
 * Measured before the change on the demo stack at 1440×900: the titles were
 * 18, 24, 24, 24px and an invisible (`sr-only`) `h1`; the content's left edge
 * was 264, 384 (a centred 896px column), 248 and 256px. Every check below
 * failed on that build.
 *
 * Two traps from the sibling spec, avoided the same way:
 *
 * 1. **No counted DOM hops.** The route root is found by a WALK up from the
 *    `h1` to the child of `main`, and landing on `main` itself is a hard
 *    error, never a silent pass.
 * 2. **Count before comparing.** The left-edge claim is made across five
 *    pages; the spec asserts it measured five before it compares them.
 *
 *   EGRET_E2E_PAGE_FRAME=1 npx playwright test page-frame
 *
 * `EGRET_E2E_STORAGE_STATE=<file>` reuses a signed-in session instead of
 * signing in (the shared demo stack limits local sign-ins to five per five
 * minutes).
 */
const ENABLED = process.env.EGRET_E2E_PAGE_FRAME === "1";
const STORAGE_STATE = process.env.EGRET_E2E_STORAGE_STATE;

/** `max-w-page`: the form, editor and settings column. */
const COLUMN = 960;

type Kind = "list" | "form" | "control-plane" | "settings" | "tool";

interface PageCase {
  kind: Kind;
  path: (ws: string) => string;
  title: string;
  /** Computed `font-size` of the visible `h1`. */
  fontSize: "20px" | "16px";
  /** `full`: the root takes the width `main` gives; `column`: a 960px column. */
  width: "full" | "column";
}

const CASES: readonly PageCase[] = [
  { kind: "list", path: (ws) => `/explore/${ws}/datasets`, title: "Datasets", fontSize: "20px", width: "full" },
  { kind: "form", path: (ws) => `/explore/${ws}/alerts/new`, title: "New alert", fontSize: "20px", width: "column" },
  { kind: "control-plane", path: () => `/control-plane/workspaces`, title: "Workspaces", fontSize: "20px", width: "full" },
  { kind: "settings", path: (ws) => `/explore/${ws}/settings/general`, title: "Workspace settings", fontSize: "20px", width: "column" },
  { kind: "tool", path: (ws) => `/explore/${ws}/sql-lab`, title: "SQL Lab", fontSize: "16px", width: "full" },
];

interface Measured {
  error?: string;
  fontSize?: string;
  titleWidth?: number;
  /** Left edge of the title's text box. */
  titleLeft?: number;
  /** The route root's content box (its rect minus padding). */
  contentLeft?: number;
  contentRight?: number;
  /** `main`'s content box: what the route is given. */
  availableRight?: number;
  /** The width the 960px column applies to: the root, or the settings pane. */
  columnWidth?: number;
}

/** Measure one page. Runs in the browser. */
const measure = (page: Page, title: string, kind: Kind) =>
  page.evaluate(
    ({ title, kind }): Measured => {
      const round = (n: number) => Math.round(n);
      const main = document.querySelector("main");
      if (!main) return { error: "no <main>" };
      const h1s = [...main.querySelectorAll("h1")].filter((h) => h.textContent?.trim() === title);
      if (h1s.length !== 1) return { error: `expected one h1 "${title}" in main, found ${h1s.length}` };
      const h1 = h1s[0]!;

      // A WALK to the child of `main`, never a counted hop.
      let root: HTMLElement | null = h1;
      while (root && root.parentElement !== main) root = root.parentElement;
      if (!root) return { error: "the walk from the h1 never reached main's child" };
      if (root === main) return { error: "the walk overshot onto main" };

      const box = (el: HTMLElement) => {
        const r = el.getBoundingClientRect();
        const s = getComputedStyle(el);
        return {
          left: r.left + parseFloat(s.paddingLeft) + parseFloat(s.borderLeftWidth),
          right: r.right - parseFloat(s.paddingRight) - parseFloat(s.borderRightWidth),
        };
      };
      const rootBox = box(root);
      const mainBox = box(main);

      // The settings frame keeps its section nav at the list's left edge and
      // caps the PANE beside it: that pane is the column on this page.
      let column: HTMLElement = root;
      if (kind === "settings") {
        const pane = main.querySelector<HTMLElement>("[data-settings-pane]");
        if (!pane) return { error: "no [data-settings-pane]" };
        column = pane;
      }
      const columnBox = box(column);

      const range = document.createRange();
      range.selectNodeContents(h1);
      const text = range.getBoundingClientRect();
      return {
        fontSize: getComputedStyle(h1).fontSize,
        titleWidth: round(h1.getBoundingClientRect().width),
        titleLeft: round(text.left),
        contentLeft: round(rootBox.left),
        contentRight: round(rootBox.right),
        availableRight: round(mainBox.right),
        columnWidth: round(columnBox.right - columnBox.left),
      };
    },
    { title, kind },
  );

test.describe("Page frame — one title scale, one left edge", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_PAGE_FRAME=1 with a live stack up");
  test.use(STORAGE_STATE ? { storageState: STORAGE_STATE } : {});
  test.setTimeout(180_000);

  test("lists, forms, Control Plane, settings and tools share the title scale and the left edge", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    if (!STORAGE_STATE) await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);

    const lefts: { kind: Kind; left: number }[] = [];
    for (const c of CASES) {
      await page.goto(c.path(ws.slug));
      // `exact: true`: Playwright matches names by substring otherwise.
      await expect(page.getByRole("heading", { level: 1, name: c.title, exact: true })).toBeVisible({
        timeout: 30_000,
      });
      await page.waitForTimeout(400);

      const m = await measure(page, c.title, c.kind);
      expect(m.error ?? null, `${c.kind}: measured`).toBeNull();

      // KI-214: a visible title on the agreed scale — not `sr-only`, not 18 or 24px.
      expect(m.titleWidth!, `${c.kind}: the h1 is visible`).toBeGreaterThan(1);
      expect(m.fontSize, `${c.kind}: title size`).toBe(c.fontSize);

      // KI-215: full-width pages take what `main` gives; column pages are 960px.
      if (c.width === "full") {
        expect(
          Math.abs(m.contentRight! - m.availableRight!),
          `${c.kind}: full width (root ${m.contentRight} vs main ${m.availableRight})`,
        ).toBeLessThanOrEqual(1);
      } else {
        expect(m.columnWidth, `${c.kind}: a ${COLUMN}px column`).toBeLessThanOrEqual(COLUMN);
        expect(m.columnWidth, `${c.kind}: a ${COLUMN}px column, not a narrower one`).toBeGreaterThanOrEqual(
          COLUMN - 1,
        );
      }
      lefts.push({ kind: c.kind, left: m.contentLeft! });
    }

    // Count before comparing.
    expect(lefts.length, "measured every layout type").toBe(CASES.length);
    const spread = Math.max(...lefts.map((l) => l.left)) - Math.min(...lefts.map((l) => l.left));
    expect(
      spread,
      `every layout type starts at one left edge — ${lefts.map((l) => `${l.kind}=${l.left}`).join(", ")}`,
    ).toBeLessThanOrEqual(1);
  });
});
