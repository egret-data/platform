import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import { EChartsRenderer } from "../EChartsRenderer";
import { buildPieOption } from "../options/pie";
import { COLOR_FALLBACK, LABEL_HALO_WIDTH } from "../palette";

/**
 * WHETHER THE OUTLINE IS ACTUALLY DRAWN, against real ECharts — not what the option asks for.
 *
 * `getTextHaloForBackground` is unit-tested next door (palette.test.ts) across the whole
 * SERIES_PALETTE × ring levels × both themes; what those tests cannot see is the step after it.
 * `textBorderColor`/`textBorderWidth` are ECharts label properties, and the claim being made here
 * is that ECharts turns them into a stroke on the glyph — spread into the wrong object, dropped by
 * a state merge, or overridden per-datum, they would vanish with every unit test still green. That
 * is the same class of gap the ChartRenderer seam tests exist for: the builder was never the part
 * that broke.
 *
 * So this file reads the rendered SVG. zrender writes a text outline as `stroke` + `stroke-width`
 * on the `<text>` element, with `paint-order="stroke"` so the outline is painted UNDER the glyph
 * rather than eating into it from both sides.
 *
 * jsdom resolves no CSS custom properties, so `resolveColor` yields the non-DOM fallbacks —
 * `COLOR_FALLBACK.text` (#111827) and `.textInverse` (#ffffff). Both failing fills fail against
 * that pair too, so the behaviour under test is reachable here; which of the two inks each fill
 * takes is palette.test.ts's business, against the real oklch tokens of both themes.
 *
 * jsdom lays nothing out either, so the host's size is supplied by hand — the same single fiction
 * the sibling `pie.hole.echarts.test.tsx` and `EChartsRenderer.echarts.test.tsx` already make.
 */

const W = 700;
const H = 600;

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => ({ width: W, height: H, top: 0, left: 0, right: W, bottom: H, x: 0, y: 0, toJSON() {} }),
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, value: W });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, value: H });
});

afterEach(() => {
  for (const prop of ["getBoundingClientRect", "clientWidth", "clientHeight"]) {
    delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop];
  }
});

const columns = [
  { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "n", type: "double" },
];
/** Two rings. APAC is the largest region, so it takes palette slot 0 — and at ring 0 that is drawn
 * as the DARK variant, `#227FD2`: one of the exactly two fills in the whole palette that no ink
 * clears. Nothing here forces a colour; the failing fill arises from the ring rule itself. */
const rows: unknown[][] = [
  ["NA", "Widgets", 5000], ["NA", "Gadgets", 3000],
  ["EU", "Widgets", 4000], ["EU", "Gadgets", 2000],
  ["APAC", "Widgets", 6000], ["APAC", "Gadgets", 2437],
];
const cfg = { dimensions: ["region", "category"], metric: "n" };

interface DrawnLabel {
  text: string;
  ink: string | null;
  halo: string | null;
  haloWidth: string | null;
  paintOrder: string | null;
}

function draw(config: Record<string, unknown> = cfg, cols = columns, data = rows) {
  const { container } = render(
    <EChartsRenderer option={(size) => buildPieOption(cols, data, config, undefined, size)} />,
  );
  const labels: DrawnLabel[] = [...container.querySelectorAll("svg text")].map((t) => ({
    text: t.textContent ?? "",
    ink: t.getAttribute("fill"),
    halo: t.getAttribute("stroke"),
    haloWidth: t.getAttribute("stroke-width"),
    paintOrder: t.getAttribute("paint-order"),
  }));
  const fills = [...container.querySelectorAll("svg path")].map((p) => p.getAttribute("fill"));
  return { labels, fills };
}

const find = (labels: DrawnLabel[], text: string) => labels.filter((l) => l.text === text);

describe("a slice label whose fill no ink can serve", () => {
  it("is drawn with a 2px outline in the ink it did NOT take", () => {
    const { labels, fills } = draw();
    // The premise: this chart really does draw the unservable fill.
    expect(fills).toContain("#227fd2");

    const [apac] = find(labels, "APAC");
    expect(apac).toBeDefined();
    // Near-black ink (its better option at 4.253, still short of 4.5) — outlined in white.
    expect(apac.ink).toBe(COLOR_FALLBACK.text);
    expect(apac.halo).toBe(COLOR_FALLBACK.textInverse);
    expect(apac.halo).not.toBe(apac.ink);
    expect(apac.haloWidth).toBe(String(LABEL_HALO_WIDTH));
  });

  it("has the outline painted UNDER the glyph, so it separates the text without thinning it", () => {
    const [apac] = find(draw().labels, "APAC");
    expect(apac.paintOrder).toBe("stroke");
  });

  it("does the same for #7172AD, the accent any 8-series chart reaches", () => {
    // The other failing fill, and the one that is NOT sunburst-only: accent7's own base level,
    // which any chart with 8+ categories lands on. Forced through `sliceColors` on a SINGLE-ring
    // pie — a single ring draws its palette slot unshaded, so the fill is #7172AD itself; on a
    // nested chart the same override would be taken at ring 0's dark variant instead.
    const single = [{ name: "d", type: "varchar" }, { name: "n", type: "double" }];
    const { labels, fills } = draw(
      { dimensions: ["d"], metric: "n", sliceColors: { AAA: "#7172AD" } },
      single,
      [["AAA", 5000], ["BBB", 4000], ["CCC", 3000]],
    );
    expect(fills).toContain("#7172AD");
    const [aaa] = find(labels, "AAA");
    // White ink here (4.456 against this fill, its better option) — so the outline is near-black,
    // the opposite way round from the #227FD2 case above. The rule follows the measurement.
    expect(aaa.ink).toBe(COLOR_FALLBACK.textInverse);
    expect(aaa.halo).toBe(COLOR_FALLBACK.text);
    expect(aaa.haloWidth).toBe(String(LABEL_HALO_WIDTH));
    // and only that slice — its two neighbours take palette slots an ink already clears
    expect(labels.filter((l) => l.halo != null).map((l) => l.text)).toEqual(["AAA"]);
  });
});

describe("every other label", () => {
  it("is drawn with no outline at all — the halo is not a look change", () => {
    const { labels } = draw();
    const outlined = labels.filter((l) => l.halo != null).map((l) => l.text);
    expect(outlined).toEqual(["APAC"]);
  });

  it("includes a genuinely dark fill whose white ink clears on its own", () => {
    // EU takes palette slot 2 at ring 0, i.e. `#8a5eb0` — dark enough that white reads at well
    // over 4.5:1 against it. It is the case a "dark fill ⇒ outline" shortcut would get wrong.
    const { labels, fills } = draw();
    expect(fills).toContain("#8a5eb0");
    const [eu] = find(labels, "EU");
    expect(eu.ink).toBe(COLOR_FALLBACK.textInverse);
    expect(eu.halo).toBeNull();
  });

  it("leaves the hole's own readout bare, since it sits on no fill", () => {
    const { labels } = draw();
    for (const line of [...find(labels, "22,437"), ...find(labels, "Total")]) {
      expect(line.halo, line.text).toBeNull();
    }
  });
});
