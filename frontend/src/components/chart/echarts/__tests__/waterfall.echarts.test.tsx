import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import { EChartsRenderer } from "../EChartsRenderer";
import { buildWaterfallOption } from "../options/waterfall";

/**
 * The waterfall against REAL ECharts, not against the option it emits.
 *
 * Every other waterfall test asserts the option's SHAPE — `series[0].data` is this array, the bar
 * values are those. None of them can see what ECharts then draws, and for a waterfall that is the
 * whole question: the bars are two stacked series, a transparent base that floats the visible
 * delta, so the drawn geometry is ECharts' stacking rule applied to those numbers rather than the
 * numbers themselves.
 *
 * ECharts' default `stackStrategy` is `"samesign"`: a value stacks on the running sum of values
 * with the SAME SIGN, and the other sign starts again from zero. The moment a running total goes
 * below zero the base turns negative while the delta stays positive, the two stop sharing a
 * stack, and the bar is drawn from the axis instead of from the step it continues. No option
 * assertion can catch that; this one reads the rendered SVG.
 */

const cols = [{ name: "m", type: "varchar" }, { name: "delta", type: "double" }];

/** Running total: 10, then -20, then -15 — it crosses zero at the second step. */
const crossesZero = [["A", 10], ["B", -30], ["C", 5]];

const SIZE = 400;

beforeEach(() => {
  // jsdom lays nothing out, and ECharts refuses to size itself without both of these — the same
  // fiction EChartsRenderer.echarts.test.tsx sets up, and the only one in this file.
  Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => ({ width: SIZE, height: SIZE, top: 0, left: 0, right: SIZE, bottom: SIZE, x: 0, y: 0, toJSON() {} }),
  });
  for (const prop of ["clientWidth", "clientHeight"]) {
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, value: SIZE });
  }
});

afterEach(() => {
  for (const prop of ["getBoundingClientRect", "clientWidth", "clientHeight"]) {
    delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop];
  }
});

/** Without the Total bar, so the drawn shapes are exactly the steps under test. Its own geometry
 *  is covered separately. */
function draw(rows: unknown[][]) {
  const option = buildWaterfallOption(cols, rows, { xAxis: "m", metric: "delta", showTotal: false });
  const { container } = render(<EChartsRenderer option={{ ...option, animation: false }} />);
  return container;
}

/** Every corner of an SVG path built from `M`/`L`/`l` segments, in user space.
 *
 *  Read off the path's own `d` because jsdom implements no SVG geometry: `getBBox` does not exist
 *  and `getBoundingClientRect` returns zeroes. ECharts writes a bar as `M x y l dx dy l dx dy …`,
 *  and those **`l` commands are RELATIVE** — reading each number pair as an absolute corner turns a
 *  bar 86px tall into one whose top is at -86, which is exactly the false failure this replaced.
 */
function corners(d: string): { x: number; y: number }[] {
  const tokens = d.match(/[MLlm]|-?\d+(?:\.\d+)?/g) ?? [];
  const out: { x: number; y: number }[] = [];
  let cmd = "M";
  let x = 0;
  let y = 0;
  for (let i = 0; i < tokens.length; i++) {
    if (/[MLlm]/.test(tokens[i])) { cmd = tokens[i]; continue; }
    const dx = Number(tokens[i]);
    const dy = Number(tokens[++i]);
    const relative = cmd === "l" || cmd === "m";
    x = relative ? x + dx : dx;
    y = relative ? y + dy : dy;
    out.push({ x, y });
  }
  return out;
}

/** The visible delta bars, left to right. The transparent base carries no fill to match on —
 *  ECharts writes `transparent` out as `fill="none"`, the same as a gridline's. */
function bars(container: HTMLElement) {
  return [...container.querySelectorAll("path")]
    .filter((p) => {
      const f = p.getAttribute("fill");
      return f != null && f !== "none" && f !== "transparent";
    })
    .map((p) => {
      const pts = corners(p.getAttribute("d") ?? "");
      return {
        x: Math.round(Math.min(...pts.map((c) => c.x))),
        top: Math.round(Math.min(...pts.map((c) => c.y))),
        bottom: Math.round(Math.max(...pts.map((c) => c.y))),
      };
    })
    .filter((b) => Number.isFinite(b.x) && b.bottom > b.top)
    .sort((a, b) => a.x - b.x);
}

describe("waterfall, drawn", () => {
  it("draws each bar spanning the step it continues, even below zero", () => {
    const drawn = bars(draw(crossesZero));
    expect(drawn).toHaveLength(3);

    // A rises 0 → +10, B falls +10 → -20, C rises -20 → -15. Each bar therefore SHARES an edge
    // with the one before it, and that is what pins the geometry:
    const [a, b, c] = drawn;
    expect(b.top).toBe(a.top);        // B hangs from +10, where A stopped
    expect(c.bottom).toBe(b.bottom);  // C stands on -20, where B stopped
    // …and C is drawn entirely below the zero line, which is A's floor.
    expect(c.top).toBeGreaterThan(a.bottom);

    // Under ECharts' default same-sign stacking all three are drawn UP from zero instead, so B and
    // C would share A's floor and B's top would be nowhere near A's.
    expect(b.bottom).toBeGreaterThan(a.bottom);
  });

  it("draws a rising series as a staircase", () => {
    const drawn = bars(draw([["A", 10], ["B", 20], ["C", 5]]));
    expect(drawn).toHaveLength(3);
    // Each bar starts where the previous one stopped: its floor IS the previous ceiling.
    expect(drawn[1].bottom).toBe(drawn[0].top);
    expect(drawn[2].bottom).toBe(drawn[1].top);
  });

  // Metabase's Total rises from zero to the running total, so it is as tall as the whole journey
  // rather than as tall as the last step — and it is drawn by DEFAULT, which egret's was not.
  it("draws the Total from zero, and draws it without being asked", () => {
    const option = buildWaterfallOption(cols, [["A", 10], ["B", 20], ["C", 5]], { xAxis: "m", metric: "delta" });
    const { container } = render(<EChartsRenderer option={{ ...option, animation: false }} />);
    const drawn = bars(container);
    expect(drawn).toHaveLength(4);
    const [a, , , totalBar] = drawn;
    expect(totalBar.bottom).toBe(a.bottom);              // stands on the zero line, like the first step
    expect(totalBar.top).toBe(drawn[2].top);             // and reaches the final running total
  });
});
