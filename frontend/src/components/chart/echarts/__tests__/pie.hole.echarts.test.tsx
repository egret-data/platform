import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import { EChartsRenderer } from "../EChartsRenderer";
import { buildPieHole, buildPieHoleGraphic, buildPieOption } from "../options/pie";

/**
 * WHERE THE HOLE'S TEXT ACTUALLY LANDS, against real ECharts — not what the option asks for.
 *
 * The option is a set of `graphic` layout instructions (`left: "center"`, a `top` inside a group);
 * whether they put the total in the MIDDLE OF THE RINGS is a claim about what ECharts does with
 * them, and the sibling unit tests cannot see it. That is exactly the gap the live comparison
 * found: measured against a real Metabase at 1440x920, egret's two lines sat 50.9px apart with
 * their block ~10px below the rings' own centre, where Metabase's sit 22px apart on it.
 *
 * So this file measures the rendered SVG the same way that comparison did:
 *   - the rings' centre is DERIVED from the drawn geometry (the first ring-0 sector starts at 12
 *     o'clock — ECharts' default `startAngle: 90, clockwise: true` — so its path opens
 *     `M cx cy-r A r r`, giving the centre without trusting any option value);
 *   - each line's centre is read off its own `transform` + baseline offset.
 * Nothing here reads back the numbers the option stated, which is the point.
 *
 * jsdom lays nothing out, so the host's size is supplied by hand — the only fiction, and the same
 * one EChartsRenderer.echarts.test.tsx already makes.
 */

const W = 600;
const H = 520;

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
const rows: unknown[][] = [
  ["NA", "Widgets", 5000], ["NA", "Gadgets", 3000],
  ["EU", "Widgets", 4000], ["EU", "Gadgets", 2000],
  ["APAC", "Widgets", 6000], ["APAC", "Gadgets", 2437],
];
const cfg = { dimensions: ["region", "category"], metric: "n" };

/** The rings' centre, read off the FIRST drawn sector: it starts at 12 o'clock, so its path
 * begins `M cx (cy - r) A r r ...`. */
function ringCentre(container: HTMLElement): { x: number; y: number } {
  const d = container.querySelector("svg path")!.getAttribute("d")!;
  const m = /^M([\d.-]+) ([\d.-]+)A([\d.-]+) /.exec(d);
  if (!m) throw new Error(`unexpected sector path: ${d}`);
  return { x: parseFloat(m[1]), y: parseFloat(m[2]) + parseFloat(m[3]) };
}

/** The hole's two lines, as { text, x, y } CENTRES. The `graphic` component renders under the
 * series, so the first two `<text>`s ARE the hole (the same convention the sibling
 * EChartsRenderer.echarts.test.tsx reads it by). zrender writes a plain text element as
 * `transform="translate(x y)"` with the baseline pushed to the middle of its own line box (`y`
 * attribute, `dominant-baseline: central`), and `text-anchor: middle` puts the translated x at
 * the string's horizontal middle. */
function holeText(container: HTMLElement) {
  return [...container.querySelectorAll("svg text")]
    .slice(0, 2)
    .map((t) => {
      const tf = /translate\(([\d.-]+) ([\d.-]+)\)/.exec(t.getAttribute("transform") ?? "");
      return {
        text: t.textContent,
        x: tf ? parseFloat(tf[1]) : NaN,
        y: (tf ? parseFloat(tf[2]) : NaN) + parseFloat(t.getAttribute("y") ?? "0"),
      };
    });
}

describe("the pie's total, measured on the rendered chart", () => {
  it("draws both lines on the rings' own centre line", () => {
    const { container } = render(
      <EChartsRenderer option={(size) => buildPieOption(columns, rows, cfg, undefined, size)} />,
    );
    const centre = ringCentre(container);
    // The rings really are where the option says: dead centre of the canvas.
    expect(centre).toEqual({ x: W / 2, y: H / 2 });

    const [value, label] = holeText(container);
    expect(value.text).toBe("22,437");
    expect(label.text).toBe("Total");
    // THE HORIZONTAL FAULT: 22px off-centre, live. Both lines share the rings' centre exactly.
    expect(value.x).toBe(centre.x);
    expect(label.x).toBe(centre.x);
  });

  it("stacks them 22px apart, straddling that centre the way Metabase's do", () => {
    const { container } = render(
      <EChartsRenderer option={(size) => buildPieOption(columns, rows, cfg, undefined, size)} />,
    );
    const centre = ringCentre(container);
    const [value, label] = holeText(container);
    // Measured off a live Metabase at 1440x920: value dy -9, label dy +13, i.e. 22px apart.
    // egret drew -28 / +22.9 — 50.9px apart, and low.
    expect(value.y - centre.y).toBeCloseTo(-9, 6);
    expect(label.y - centre.y).toBeCloseTo(13, 6);
    expect(label.y - value.y).toBeCloseTo(22, 6);
  });

  // The hover path re-states the hole as a merge-mode partial option, which re-runs ECharts'
  // layout for those elements against a DIFFERENT string. A readout that drifted off-centre as
  // the pointer moved would be invisible to every unit test, since the option's own numbers
  // never change.
  it("keeps the readout centred when a hover replaces the total", () => {
    const hole = buildPieHole(columns, rows, cfg);
    const { container } = render(
      <EChartsRenderer
        option={(size) => buildPieOption(columns, rows, cfg, undefined, size)}
        overlay={(size) => ({ graphic: buildPieHoleGraphic(hole, cfg, { name: "APAC", value: 8437, fromLegend: false }, size) })}
      />,
    );
    const centre = ringCentre(container);
    const [value, label] = holeText(container);
    expect([value.text, label.text]).toEqual(["8,437", "APAC"]);
    expect(value.x).toBe(centre.x);
    expect(label.x).toBe(centre.x);
    expect(label.y - value.y).toBeCloseTo(22, 6);
  });
});
