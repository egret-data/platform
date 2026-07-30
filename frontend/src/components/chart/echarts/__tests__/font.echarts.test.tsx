import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import { EChartsRenderer } from "../EChartsRenderer";
import { buildPieOption } from "../options/pie";
import { buildCartesianOption } from "../options/cartesian";
import { buildFunnelOption } from "../options/funnel";
import { buildGaugeOption } from "../options/gauge";
import { buildBoxplotOption } from "../options/boxplot";
import { buildSankeyOption } from "../options/sankey";
import { buildWaterfallOption } from "../options/waterfall";
import { buildMapOption } from "../options/map";
import { CHART_FONT_FALLBACK } from "../palette";

/**
 * WHAT TYPEFACE THE CHARTS ARE ACTUALLY DRAWN IN.
 *
 * Measured live: egret's UI is Inter and its charts were `sans-serif` — the browser's default —
 * because nothing in any option ever named a font. Metabase sets one at the option root
 * (`textStyle: { fontFamily: renderingContext.fontFamily }`, pie/option.ts) and its charts are in
 * its own Lato as a result. The rule worth copying is that a chart uses the PRODUCT's font; the
 * font itself is egret's, not Metabase's.
 *
 * Asserted on the rendered SVG rather than on the option, for two reasons: what a chart is drawn
 * in is the claim, and ECharts' inheritance from the root `textStyle` down to a series label or a
 * `graphic` element is its own behaviour, not something an option key can promise. (It is exactly
 * where the hole's total turned out NOT to inherit, which no option-level assertion would have
 * caught.)
 *
 * Under jsdom no custom property resolves, so `var(--font-sans)` degrades to CHART_FONT_FALLBACK —
 * which still starts with Inter, so "the product's font" is testable here. What cannot be tested
 * here is the token lookup itself; that needs a browser.
 */

const W = 600;
const H = 400;

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

/** The `font-family` of every `<text>` the chart drew, de-duplicated. */
function fonts(container: HTMLElement): string[] {
  const found = [...container.querySelectorAll("svg text")].map(
    (t) => /font-family:\s*([^;]+)/.exec(t.getAttribute("style") ?? "")?.[1]?.trim() ?? "",
  );
  expect(found.length, "no text was drawn at all — the assertion would be vacuous").toBeGreaterThan(0);
  return [...new Set(found)];
}

const cols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
const rows: unknown[][] = [["A", 30], ["B", 70]];

const CHART_TYPES: [string, () => unknown][] = [
  ["bar", () => buildCartesianOption(cols, rows, { kind: "bar", xAxis: "d", yMetrics: ["m"], showLabels: true })],
  ["line", () => buildCartesianOption(cols, rows, { kind: "line", xAxis: "d", yMetrics: ["m"], showLabels: true })],
  ["area", () => buildCartesianOption(cols, rows, { kind: "area", xAxis: "d", yMetrics: ["m"] })],
  ["scatter", () => buildCartesianOption(cols, rows, { kind: "scatter", xAxis: "d", yMetrics: ["m"] })],
  ["pie", () => buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", donut: true, showTotal: true })],
  ["funnel", () => buildFunnelOption(cols, rows, { dimension: "d", metric: "m" })],
  ["gauge", () => buildGaugeOption(cols, [[30]], { metric: "m" })],
  ["boxplot", () => buildBoxplotOption(cols, [["A", 1], ["A", 2], ["A", 3], ["B", 4]], { category: "d", value: "m" })],
  ["waterfall", () => buildWaterfallOption(cols, rows, { xAxis: "d", metric: "m" })],
  [
    "sankey",
    () =>
      buildSankeyOption(
        [{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "m", type: "double" }],
        [["A", "B", 10]],
        { source: "s", target: "t", metric: "m" },
      ),
  ],
];

describe("every chart is drawn in the product's own typeface", () => {
  for (const [name, build] of CHART_TYPES) {
    it(`${name}: every string it draws`, () => {
      const { container } = render(<EChartsRenderer option={build() as never} />);
      for (const font of fonts(container)) {
        expect(font, `${name} drew text in ${font}`).toBe(CHART_FONT_FALLBACK);
      }
    });
  }

  // The hole's total is a `graphic` element, not a series label — a different inheritance path,
  // and the one that turned out not to follow the option root at all.
  it("the pie's centre total too, not just its slice labels", () => {
    const { container } = render(
      <EChartsRenderer
        option={(size) => buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", donut: true, showTotal: true }, undefined, size)}
      />,
    );
    const [value, label] = [...container.querySelectorAll("svg text")].slice(0, 2);
    expect(value.textContent).toBe("100");
    expect(label.textContent).toBe("Total");
    for (const el of [value, label]) {
      expect(/font-family:\s*([^;]+)/.exec(el.getAttribute("style") ?? "")?.[1]?.trim()).toBe(CHART_FONT_FALLBACK);
    }
  });

  // The map is the one chart type this file cannot render: an ECharts `map` series draws nothing
  // at all without a registered geojson, so there would be no text to read a font off. It states
  // the font the same way the others do — at the option root, via base.ts — and that is what is
  // checked here, so the list of chart types covered has no silent hole in it.
  it("the map states it at the root too, where it cannot be rendered to check", () => {
    const opt = buildMapOption(
      [{ name: "r", type: "varchar" }, { name: "m", type: "double" }],
      [["VN", 5]],
      { echartsName: "world", region: "r", metric: "m", normalize: (v) => String(v) },
    ) as { textStyle?: { fontFamily?: string } };
    expect(opt.textStyle?.fontFamily).toBe(CHART_FONT_FALLBACK);
  });

  // The point of the whole change, stated as its own assertion so it can't be satisfied by
  // hard-coding something that merely isn't the browser default.
  it("which is Inter, the font the rest of the app is set in — never the browser's default", () => {
    expect(CHART_FONT_FALLBACK.toLowerCase()).toContain("inter");
    expect(CHART_FONT_FALLBACK).not.toBe("sans-serif");
  });
});
