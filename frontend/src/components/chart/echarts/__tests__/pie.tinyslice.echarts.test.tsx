import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import { EChartsRenderer } from "../EChartsRenderer";
import { buildPieOption } from "../options/pie";

/**
 * WHAT A TINY SLICE ACTUALLY GETS DRAWN AS, against real ECharts.
 *
 * The claim under test is about the wedge on screen — "is there enough of it to put a pointer on"
 * — and no assertion about the option's own numbers can see it: the option states a VALUE, and
 * the angle that value turns into is ECharts' arithmetic (`sunburstLayout`'s
 * `unitRadian = 2π / rootSum`, applied to every node at every depth with no per-parent
 * renormalisation). So this file measures the rendered SVG sector paths instead.
 *
 * jsdom lays nothing out, so the host's size is supplied by hand — the same fiction the sibling
 * pie.hole.echarts.test.tsx makes.
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

/** The rings' centre, read off the FIRST drawn sector: ECharts' default `startAngle: 90,
 * clockwise: true` puts it at 12 o'clock, so its path opens `M cx (cy - r) A r r ...`. Derived
 * rather than taken from the option, so nothing here trusts a number the option stated. */
function ringCentre(container: HTMLElement): { x: number; y: number } {
  const d = container.querySelector("svg path")!.getAttribute("d")!;
  const m = /^M([\d.-]+) ([\d.-]+)A([\d.-]+) /.exec(d);
  if (!m) throw new Error(`unexpected sector path: ${d}`);
  return { x: parseFloat(m[1]), y: parseFloat(m[2]) + parseFloat(m[3]) };
}

/**
 * A drawn sector's angular width, in radians.
 *
 * zrender writes a sector as `M x0 y0 A rx ry rot largeArc sweep x1 y1 …` — both of those points
 * sit on the OUTER arc, so the angle between them (about the rings' centre) is the wedge's own
 * span. `acos` only resolves [0, π], which is what the large-arc flag then disambiguates: it is
 * set exactly when the sector is wider than a half turn.
 */
function sectorSpan(d: string, centre: { x: number; y: number }): number {
  const m = /^M([\d.-]+) ([\d.-]+)A([\d.-]+) ([\d.-]+) ([\d.-]+) ([01]) ([01]) ([\d.-]+) ([\d.-]+)/.exec(d);
  if (!m) throw new Error(`unexpected sector path: ${d}`);
  const v0 = { x: parseFloat(m[1]) - centre.x, y: parseFloat(m[2]) - centre.y };
  const v1 = { x: parseFloat(m[8]) - centre.x, y: parseFloat(m[9]) - centre.y };
  const largeArc = m[6] === "1";
  const cos = (v0.x * v1.x + v0.y * v1.y) / (Math.hypot(v0.x, v0.y) * Math.hypot(v1.x, v1.y));
  const theta = Math.acos(Math.min(1, Math.max(-1, cos)));
  return largeArc ? 2 * Math.PI - theta : theta;
}

/** Every drawn sector's angular width, in ECharts' own draw order — which for a sunburst is the
 * data tree depth-first, pre-order (so ring 0's first slice, then its whole subtree, then ring 0's
 * second slice, …). */
function sectorSpans(container: HTMLElement): number[] {
  const centre = ringCentre(container);
  return [...container.querySelectorAll("svg path")]
    .map((p) => p.getAttribute("d")!)
    .filter((d) => d.includes("A"))
    .map((d) => sectorSpan(d, centre));
}

const FULL_TURN = 2 * Math.PI;
/** Metabase's OTHER_SLICE_MIN_PERCENTAGE. */
const MIN_FRACTION = 0.005;

describe("a slice too small to point at", () => {
  const columns = [{ name: "r", type: "varchar" }, { name: "n", type: "double" }];
  // 1 in 1001 — 0.0999% of the total, a fifth of the floor.
  const rows: unknown[][] = [["A", 1000], ["B", 1]];
  const cfg = { dimensions: ["r"], metric: "n", sliceThreshold: 0 };

  it("is drawn at the floor rather than as a sub-pixel sliver", () => {
    const { container } = render(
      <EChartsRenderer option={(size) => buildPieOption(columns, rows, cfg, undefined, size)} />,
    );
    const [a, b] = sectorSpans(container);

    // Untouched, this drew 1/1001 of a turn — 0.36°, about 1.1px of arc at this radius, which is
    // narrower than the slice border painted over it.
    expect(b / FULL_TURN).toBeGreaterThan(0.9 * MIN_FRACTION);
    // The floor is a floor, not a redistribution: nothing else grows, and the circle still closes.
    expect(a + b).toBeCloseTo(FULL_TURN, 6);
    expect(a / FULL_TURN).toBeGreaterThan(0.99);
  });

  it("leaves a slice above the floor exactly where it was", () => {
    // 20/1020 = 1.96%, comfortably above 0.5% — its arc is its own share of the turn, untouched.
    const { container } = render(
      <EChartsRenderer
        option={(size) => buildPieOption(columns, [["A", 1000], ["B", 20]], cfg, undefined, size)}
      />,
    );
    const [, b] = sectorSpans(container);
    expect(b / FULL_TURN).toBeCloseTo(20 / 1020, 6);
  });
});

describe("the floor is ring 0's alone", () => {
  const columns = [
    { name: "r", type: "varchar" }, { name: "c", type: "varchar" }, { name: "n", type: "double" },
  ];
  const cfg = { dimensions: ["r", "c"], metric: "n", sliceThreshold: 0 };

  it("does not inflate a tiny slice in an outer ring", () => {
    // A is 99.8% of the chart; its own child `tiny` is 1/1000 of A, far below the floor. Metabase
    // floors ring 0 only, because inflating an outer ring's slices makes them overlap their
    // neighbours — there is no spare arc inside a parent to take the extra from.
    const rows: unknown[][] = [["A", "big", 999], ["A", "tiny", 1], ["B", "only", 1000]];
    const { container } = render(
      <EChartsRenderer option={(size) => buildPieOption(columns, rows, cfg, undefined, size)} />,
    );
    // Draw order: A, A>big, A>tiny, B, B>only.
    const [, , tiny] = sectorSpans(container);
    expect(tiny / FULL_TURN).toBeCloseTo(1 / 2000, 6);
  });

  it("keeps an inflated parent's children filling its drawn arc", () => {
    // B is 2 in 1002 — floored. Its two children are the whole of it, so between them they must
    // still span exactly the arc B is DRAWN with; anything else leaves a wedge of B with nothing
    // nested in it, which reads as missing data rather than as a magnified slice.
    const rows: unknown[][] = [["A", "x", 600], ["A", "y", 400], ["B", "x", 1], ["B", "y", 1]];
    const { container } = render(
      <EChartsRenderer option={(size) => buildPieOption(columns, rows, cfg, undefined, size)} />,
    );
    // Draw order: A, A>x, A>y, B, B>x, B>y.
    const [a, ax, ay, b, bx, by] = sectorSpans(container);
    expect(b / FULL_TURN).toBeGreaterThan(0.9 * MIN_FRACTION);
    expect(bx + by).toBeCloseTo(b, 6);
    // ...and in their own true proportion, 1:1. Five places, not six: these two arcs are ~0.9° of
    // a 127px radius, and zrender writes path coordinates to four decimals, so the angle recovered
    // from them carries about that much noise.
    expect(bx).toBeCloseTo(by, 5);
    // The untouched parent is unaffected in both respects.
    expect(ax + ay).toBeCloseTo(a, 6);
    expect(ax / a).toBeCloseTo(0.6, 6);
  });
});
