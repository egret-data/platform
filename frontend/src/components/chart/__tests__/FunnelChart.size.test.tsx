import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, act } from "@testing-library/react";
import { FunnelChart } from "../FunnelChart";

vi.mock("../echarts/EChartsRenderer", () => ({
  EChartsRenderer: () => <div data-testid="echarts" />,
}));

/**
 * The funnel's typography on a small card.
 *
 * ## How this measures, and why the measurement is honest
 *
 * jsdom does not lay out: `getBoundingClientRect()` returns zeros and no stylesheet is loaded, so
 * neither a box width nor a Tailwind class's effect can be read here. An assertion phrased as
 * "the later steps' name boxes are wider than 40px" would go green against a component that
 * rendered nothing at all. (PR 05 shipped exactly that mistake in `sizeAware.test.ts`, where a
 * gauge case passed only because jsdom's text measurer returns 0.) The pixel-width criterion is
 * therefore NOT asserted here — it belongs to the browser check in Task 5 of this plan.
 *
 * What IS assertable at unit level is the thing that drives those widths: the font sizes the
 * component actually applies. Metabase expresses its own funnel typography as a cascade of `em`
 * multipliers in `FunnelNormal.styled.tsx`, and this port does the same — but as INLINE styles,
 * which jsdom does report verbatim and which win over any class in a browser too. `appliedFontSizePx`
 * below folds that cascade the way a browser would: walk from the leaf up to the funnel's root,
 * multiply every inline `em` on the way, and scale by the root's own size. So these assertions read
 * the real applied value, not a class name — a class assertion passes for a class that does nothing.
 *
 * The one number the walker cannot read from the DOM is the root's own font size, which comes from
 * Tailwind's `text-sm`. It is stated as a constant instead — the single stated assumption in this
 * file — and every expectation below was then confirmed against a real Chromium render of this
 * component at these exact card sizes, so the constant is measured rather than assumed.
 *
 * ## egret's root is 13px, Metabase's is 14px
 *
 * egret's design tokens set `--text-sm: 0.8125rem` (`src/index.css`), so the same `em` cascade
 * lands 13/14 lower here than it does in Metabase. What ports is the MULTIPLIER, not the pixel:
 *
 * | element          | multiplier wide | multiplier narrow | Metabase px     | egret px        |
 * |------------------|-----------------|-------------------|-----------------|-----------------|
 * | step-1 value     | 1.72            | 1.29              | 24.08 / 18.06   | 22.36 / 16.77   |
 * | metric name      | 1.1825          | 0.86              | 16.555 / 12.04  | 15.3725 / 11.18 |
 * | share percentage | 1.15            | 0.85              | 16.1 / 11.9     | 14.95 / 11.05   |
 * | step name        | 1               | 1                 | 13 / 13         | 13 / 13         |
 *
 * The px columns are both measured, Metabase's on its own dashboard card and egret's in Chromium.
 * The right-hand column is what this file asserts; the difference between the two is the funnel
 * root's own size and is not what this task changed.
 */
const ROOT_FONT_PX = 13;

/** egret's rendering of Metabase's funnel typography, in px — the multiplier table above folded
 * against egret's own 13px root, and confirmed in Chromium at these card sizes. */
const FONT_PX = {
  wide: { stepOneValue: 22.36, metricName: 15.3725, sharePercent: 14.95 },
  narrow: { stepOneValue: 16.77, metricName: 11.18, sharePercent: 11.05 },
};

/**
 * The font size a browser would compute for `el`, in px, by folding the inline `em` font-sizes
 * from `el` up to `root` inclusive.
 *
 * Throws on an inline font-size that is not in `em`: a port that started stating px directly would
 * silently break the cascade (a px size stops inheriting scale from its ancestors), and this
 * walker must not quietly fold it into a number that happens to look right.
 */
function appliedFontSizePx(el: HTMLElement, root: HTMLElement): number {
  let factor = 1;
  let node: HTMLElement | null = el;
  let reachedRoot = false;
  while (node) {
    const size = node.style.fontSize;
    if (size) {
      const em = /^([\d.]+)em$/.exec(size);
      if (!em) throw new Error(`inline font-size "${size}" is not an em; the cascade cannot be folded`);
      factor *= Number(em[1]);
    }
    if (node === root) {
      reachedRoot = true;
      break;
    }
    node = node.parentElement;
  }
  if (!reachedRoot) throw new Error("element is not inside the funnel root");
  return Math.round(factor * ROOT_FONT_PX * 1e4) / 1e4;
}

/** Registered ResizeObserver callbacks, so a spec can deliver a size the way the browser would. */
const observed: { cb: (entries: { contentRect: { width: number; height: number } }[]) => void }[] = [];
let realResizeObserver: typeof globalThis.ResizeObserver;

beforeEach(() => {
  observed.length = 0;
  realResizeObserver = globalThis.ResizeObserver;
  globalThis.ResizeObserver = class {
    constructor(private cb: (entries: { contentRect: { width: number; height: number } }[]) => void) {}
    observe() {
      observed.push({ cb: this.cb });
    }
    unobserve() {}
    disconnect() {}
  } as unknown as typeof globalThis.ResizeObserver;
});

afterEach(() => {
  globalThis.ResizeObserver = realResizeObserver;
});

/**
 * The Metabase harness's own funnel data. The step names are deliberately of four different
 * lengths and the values of three different digit counts, so nothing here can be right by
 * coincidence of equal-width text — see the note on the fixture at the bottom of this file.
 */
const cols = [{ name: "step", type: "varchar" }, { name: "sessions", type: "bigint" }];
const rows = [
  ["Visited", 131320],
  ["Added to cart", 56280],
  ["Started checkout", 21105],
  ["Purchased", 9840],
];

/** Renders the funnel and then reports `width` x `height` to it, the way a browser's
 * ResizeObserver would once the card had been laid out. */
function renderAt(width: number, height: number) {
  const view = render(<FunnelChart columns={cols} rows={rows} dimension="step" metric="sessions" />);
  act(() => {
    for (const o of observed) o.cb([{ contentRect: { width, height } }]);
  });
  const root = view.container.querySelector<HTMLElement>("[data-funnel-root]")!;
  const q = (sel: string) => root.querySelector<HTMLElement>(sel)!;
  return {
    root,
    sizes: {
      stepOneValue: appliedFontSizePx(q("[data-funnel-value]"), root),
      metricName: appliedFontSizePx(q("[data-funnel-metric]"), root),
      sharePercent: appliedFontSizePx(q("[data-funnel-share]"), root),
      // EVERY step's name, not the first one `querySelector` happens to return. The first column
      // is the one whose type deliberately changes, so a spec that looked only there would miss a
      // scale leaking into the later steps' names — which is exactly what it did until a mutant
      // that scaled only the later heads survived.
      stepNames: [...root.querySelectorAll<HTMLElement>("[data-funnel-step-name]")]
        .map((el) => appliedFontSizePx(el, root)),
      infoBlock: appliedFontSizePx(q("[data-funnel-info]"), root),
      spacerBlock: appliedFontSizePx(q("[data-funnel-spacer]"), root),
    },
  };
}

/** The card measured on egret's own dashboard, whose steps 2-4 were crushed to 19px. */
const EGRET_DASHBOARD_CARD = { width: 184, height: 234 };
/** Comfortably past the flip in both directions — a card no one would call small. */
const ROOMY_CARD = { width: 640, height: 400 };

describe("FunnelChart typography follows the card's size", () => {
  it("sets the same three sizes Metabase does on a roomy card", () => {
    const { sizes } = renderAt(ROOMY_CARD.width, ROOMY_CARD.height);
    expect(sizes.stepOneValue).toBe(FONT_PX.wide.stepOneValue);
    expect(sizes.metricName).toBe(FONT_PX.wide.metricName);
    expect(sizes.sharePercent).toBe(FONT_PX.wide.sharePercent);
  });

  // The defect this task exists for: on egret's 184px dashboard card every one of these stayed at
  // its full size, so the first column took the width the later three needed and they collapsed
  // onto their 20px floor. Metabase shrinks exactly these three and nothing else.
  it("shrinks the same three, to the same sizes, on the 184px dashboard card", () => {
    const { sizes } = renderAt(EGRET_DASHBOARD_CARD.width, EGRET_DASHBOARD_CARD.height);
    expect(sizes.stepOneValue).toBe(FONT_PX.narrow.stepOneValue);
    expect(sizes.metricName).toBe(FONT_PX.narrow.metricName);
    expect(sizes.sharePercent).toBe(FONT_PX.narrow.sharePercent);
  });

  // Measured across Metabase's own flip: the step's NAME does not move. A port that scaled all of
  // the funnel's typography by one factor would pass the two cases above and fail this one.
  it("leaves every step's name alone at both sizes", () => {
    const wide = renderAt(ROOMY_CARD.width, ROOMY_CARD.height).sizes.stepNames;
    const narrow = renderAt(EGRET_DASHBOARD_CARD.width, EGRET_DASHBOARD_CARD.height).sizes.stepNames;
    expect(wide).toHaveLength(rows.length);
    expect(new Set(wide)).toEqual(new Set([ROOT_FONT_PX]));
    expect(narrow).toEqual(wide);
  });

  // `gridSize.width < 7` at viewport 1440 measures 301px wide (NOT narrow) against 258px (narrow),
  // and seven grid columns is 301.125px — so 301 is the first wide pixel and 300 is the last narrow
  // one. Pinned on both sides, one pixel apart, so a threshold that drifted in either direction is
  // caught rather than being covered by the 43px gap between two real grid sizes.
  it("flips between 300px and 301px of card width, where the grid does", () => {
    expect(renderAt(300, 400).sizes.stepOneValue).toBe(FONT_PX.narrow.stepOneValue);
    expect(renderAt(301, 400).sizes.stepOneValue).toBe(FONT_PX.wide.stepOneValue);
  });

  // Metabase's root padding is `isSmall ? 0.5rem : 1rem`, and `isSmall = isShort || isNarrow` —
  // so a SHORT card tightens the padding while leaving every font size alone. Collapsing the two
  // flags into one would fail this.
  it("tightens the padding on a short card without touching the type", () => {
    const short = renderAt(ROOMY_CARD.width, 200);
    expect(short.root.style.padding).toBe("0.5rem");
    expect(short.sizes.stepOneValue).toBe(FONT_PX.wide.stepOneValue);

    const narrow = renderAt(EGRET_DASHBOARD_CARD.width, EGRET_DASHBOARD_CARD.height);
    expect(narrow.root.style.padding).toBe("0.5rem");

    const roomy = renderAt(ROOMY_CARD.width, ROOMY_CARD.height);
    expect(roomy.root.style.padding).toBe("1rem");
  });

  // The first column ends in an invisible twin of the block the other columns end with, so every
  // band's bottom edge starts at the same y. The twin only reserves the right height if it is set
  // at the same size — which is exactly what Metabase gets wrong upstream (its spacer misses the
  // `isDashboard` override its real block carries, leaving the first column 114.89px against the
  // graph columns' 110.45px). This port does not copy that, at both sizes.
  it("keeps the first column's spacer the same size as the block it stands in for", () => {
    for (const [w, h] of [[ROOMY_CARD.width, ROOMY_CARD.height], [EGRET_DASHBOARD_CARD.width, EGRET_DASHBOARD_CARD.height]]) {
      const { sizes } = renderAt(w, h);
      expect(sizes.spacerBlock).toBe(sizes.infoBlock);
    }
  });

  // A funnel that has never been measured — mounted hidden, or in a container jsdom-like enough to
  // report zero — must not read that as "tiny" and shrink itself. Metabase's own `isNarrow` is
  // false when `gridSize` is absent.
  it("stays at full size until it has actually been measured", () => {
    const view = render(<FunnelChart columns={cols} rows={rows} dimension="step" metric="sessions" />);
    const root = view.container.querySelector<HTMLElement>("[data-funnel-root]")!;
    expect(appliedFontSizePx(root.querySelector<HTMLElement>("[data-funnel-value]")!, root))
      .toBe(FONT_PX.wide.stepOneValue);
    act(() => {
      for (const o of observed) o.cb([{ contentRect: { width: 0, height: 0 } }]);
    });
    expect(appliedFontSizePx(root.querySelector<HTMLElement>("[data-funnel-value]")!, root))
      .toBe(FONT_PX.wide.stepOneValue);
  });

  /*
   * ON THE FIXTURE. Nothing above depends on the step names or the values: a font size is applied
   * from the card's measured width alone, so swapping "Added to cart" for "X" cannot change a
   * single expected number — verified by mutation. The fixture is nonetheless the live harness's
   * own (four names of 7, 13, 16 and 9 characters; values of 6, 5, 5 and 4 digits), because it is
   * the same data the browser check in Task 5 measures, and there the name lengths matter very
   * much — four names of equal length would hide a per-column width bug.
   */
});
