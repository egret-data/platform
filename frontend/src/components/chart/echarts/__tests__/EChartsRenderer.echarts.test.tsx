import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import { EChartsRenderer } from "../EChartsRenderer";

/**
 * EChartsRenderer against REAL ECharts — no `vi.mock("echarts/core")`.
 *
 * Its sibling EChartsRenderer.test.tsx mocks the library, so it can only assert what this
 * component ASKS ECharts to do: that `setOption` was called with one argument rather than two,
 * that `dispatchAction` was called with a particular payload. Those are the right assertions for
 * wiring, and they are all a mock can see.
 *
 * They are not enough for the two mechanisms the pie's hover depends on, because both are claims
 * about what ECharts does IN RESPONSE:
 *
 *  - a merge-mode `setOption({ graphic })` updates the existing elements by id, rather than
 *    appending a second set or silently doing nothing;
 *  - it does not, in doing so, discard the emphasis/blur state ECharts is holding — which is the
 *    failure this file exists to catch, since it would leave the chart looking un-hovered while
 *    every unit test still passed.
 *
 * ECharts' SVG renderer needs no canvas, so it runs here for real: the assertions below read the
 * rendered SVG, the same thing a browser would draw. jsdom does no layout, so the element's size
 * is supplied by hand — that is the only fiction in this file.
 */

/** A minimal sunburst: two ring-0 slices, each with children. Same shape as the pie's option
 * (`emphasis.focus: "ancestor"`, animation off), reduced to what these mechanisms need.
 *
 * Every node states its own colour, so a test can say WHICH slices stayed lit rather than only
 * how many — ECharts moves the emphasised element to the end of the DOM, so position tells you
 * nothing. */
const COLORS = { A: "#111111", a1: "#222222", a2: "#333333", B: "#444444", b1: "#555555" };
const sunburst = (graphicText: string) => ({
  animation: false,
  series: [
    {
      type: "sunburst",
      radius: [30, 120],
      nodeClick: false,
      sort: undefined,
      label: { show: false },
      emphasis: { focus: "ancestor" as const },
      data: [
        {
          name: "A", value: 60, itemStyle: { color: COLORS.A },
          children: [
            { name: "a1", value: 40, itemStyle: { color: COLORS.a1 } },
            { name: "a2", value: 20, itemStyle: { color: COLORS.a2 } },
          ],
        },
        {
          name: "B", value: 40, itemStyle: { color: COLORS.B },
          children: [{ name: "b1", value: 40, itemStyle: { color: COLORS.b1 } }],
        },
      ],
    },
  ],
  // Two text elements with stable ids — exactly how options/pie.ts states the hole.
  graphic: [
    { id: "pie-total-value", type: "text", left: "center", top: "43%", style: { text: graphicText, fontSize: 22 } },
    { id: "pie-total-label", type: "text", left: "center", top: "53%", style: { text: "Total", fontSize: 14 } },
  ],
});

const hole = (n: number, txt: string) => ({
  graphic: [
    { id: "pie-total-value", type: "text", left: "center", top: "43%", style: { text: String(n), fontSize: 22 } },
    { id: "pie-total-label", type: "text", left: "center", top: "53%", style: { text: txt, fontSize: 14 } },
  ],
});

/** Every `<text>` the chart drew, in document order. The hole's two come first (the `graphic`
 * component renders under the series), so `slice(0, 2)` IS the hole. */
const texts = (c: HTMLElement) => [...c.querySelectorAll("svg text")].map((t) => t.textContent);

/** How many slices are blurred — ECharts renders a blurred wedge with a reduced `fill-opacity`,
 * which is exactly what a real browser shows (measured on the deployed chart: 48 of 51 wedges
 * dimmed while pointing at an outer slice). */
const blurred = (c: HTMLElement) =>
  [...c.querySelectorAll("svg path")].filter((p) => {
    const o = p.getAttribute("fill-opacity");
    return o != null && parseFloat(o) < 1;
  }).length;

/** `dispatchAction` does not repaint synchronously — zrender flushes the SVG on its next frame.
 * Everything that asserts on a highlight has to let that happen first. */
const settle = () => new Promise((r) => setTimeout(r, 20));

const SIZE = 400;

beforeEach(() => {
  // jsdom lays nothing out. ECharts reads clientWidth/clientHeight on init and
  // getBoundingClientRect thereafter; without both it refuses to size the canvas at all.
  Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => ({ width: SIZE, height: SIZE, top: 0, left: 0, right: SIZE, bottom: SIZE, x: 0, y: 0, toJSON() {} }),
  });
  for (const prop of ["clientWidth", "clientHeight"]) {
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, value: SIZE });
  }
});

afterEach(() => {
  document.documentElement.classList.remove("dark");
  for (const prop of ["getBoundingClientRect", "clientWidth", "clientHeight"]) {
    delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop];
  }
});

describe("EChartsRenderer against real ECharts — the overlay", () => {
  it("draws the option's own graphic when there is no overlay", () => {
    const { container } = render(<EChartsRenderer option={sunburst("22,437")} />);
    expect(texts(container).slice(0, 2)).toEqual(["22,437", "Total"]);
    expect(container.querySelectorAll("svg path").length).toBe(5); // 2 ring-0 + 3 children
  });

  // THE MECHANISM. A merge-mode apply has to reach the elements the option already stated, by id.
  // Appending instead would leave the resting total drawn underneath the readout; no-oping would
  // leave the hole stale — the two failure modes that are indistinguishable from a mock's point of
  // view, since both look like a `setOption` call that was made.
  it("re-states the hole in place, without adding a second one", () => {
    const { container } = render(
      <EChartsRenderer option={sunburst("22,437")} overlay={hole(503, "Organic")} />,
    );
    const all = texts(container);
    expect(all.slice(0, 2)).toEqual(["503", "Organic"]);
    // Two hole texts, and no more — an append would show four.
    expect(all.filter((t) => t === "22,437")).toHaveLength(0);
    expect(all).toHaveLength(2);
  });

  it("follows the overlay as the pointer moves from slice to slice", () => {
    const { container, rerender } = render(
      <EChartsRenderer option={sunburst("22,437")} overlay={hole(503, "Organic")} />,
    );
    rerender(<EChartsRenderer option={sunburst("22,437")} overlay={hole(929, "Affiliate")} />);
    expect(texts(container).slice(0, 2)).toEqual(["929", "Affiliate"]);
    rerender(<EChartsRenderer option={sunburst("22,437")} overlay={hole(8177, "LATAM")} />);
    expect(texts(container).slice(0, 2)).toEqual(["8177", "LATAM"]);
    expect(texts(container)).toHaveLength(2); // still no accumulation after three moves
  });

  // A theme flip re-applies the whole option with notMerge, which resets `graphic` to whatever the
  // option says. Without the re-statement in `applyOption` the hole would snap back to the resting
  // total in the middle of a hover.
  it("keeps the readout across a full notMerge re-apply", async () => {
    const { container } = render(
      <EChartsRenderer option={sunburst("22,437")} overlay={hole(503, "Organic")} />,
    );
    document.documentElement.classList.add("dark");
    await new Promise((r) => setTimeout(r, 0));
    expect(texts(container).slice(0, 2)).toEqual(["503", "Organic"]);
  });
});

describe("EChartsRenderer against real ECharts — highlight", () => {
  /** The wedges NOT dimmed, by the colour each node was given. ECharts moves the emphasised
   * element to the end of the DOM, so identity has to come from the fill, not the position. */
  const lit = (c: HTMLElement) =>
    [...c.querySelectorAll("svg path")]
      .filter((p) => {
        const o = p.getAttribute("fill-opacity");
        return o == null || parseFloat(o) >= 1;
      })
      // The emphasised wedge is redrawn in a lightened shade of its own colour, so it is matched
      // by name rather than by exact fill.
      .map((p) => p.getAttribute("fill"));

  // The legend's half of the two-way hover. A mock can only show the payload was sent; this shows
  // ECharts resolved it to a datum and blurred everything outside that slice's family.
  it("blurs every wedge but the highlighted one, for a ring-0 slice", async () => {
    const { container, rerender } = render(<EChartsRenderer option={sunburst("22,437")} />);
    expect(blurred(container)).toBe(0);

    // dataIndex 1 is ring-0's first slice: ECharts prepends one virtual root, then walks
    // depth-first pre-order — root, A, a1, a2, B, b1.
    rerender(<EChartsRenderer option={sunburst("22,437")} highlightDataIndex={1} />);
    await settle();
    // `focus: "ancestor"` lights the node and its ANCESTORS. A ring-0 slice has none but the
    // virtual root, so it stands alone and the other four dim — which is exactly what the
    // deployed chart does when a legend entry is hovered (50 of 51 dimmed).
    expect(blurred(container)).toBe(4);
    expect(lit(container)).toHaveLength(1);
  });

  it("keeps a hovered outer slice's ancestors lit with it", async () => {
    // dataIndex 5 is `b1`, whose ancestor chain is B -> root. Both b1 and B stay lit; A's family
    // of three dims. The deployed chart shows the same shape three rings deep: 48 of 51 dimmed,
    // the hovered wedge and its two ancestors lit.
    const { container, rerender } = render(<EChartsRenderer option={sunburst("22,437")} />);
    rerender(<EChartsRenderer option={sunburst("22,437")} highlightDataIndex={5} />);
    await settle();
    expect(blurred(container)).toBe(3);
    expect(lit(container)).toHaveLength(2);
  });

  // The reason `PieLegendEntry` carries an index at all. ECharts resolves a `name` payload with
  // `SeriesData.indexOfName`, a depth-first pre-order scan — so with the same name at two depths
  // it finds whichever comes first, which is not the ring-0 slice a legend entry means.
  it("addresses the datum by index, so a name shared across rings cannot mislead it", async () => {
    const OUTER = "#aaaaaa", ROOT_OTHER = "#bbbbbb";
    const ambiguous = {
      animation: false,
      series: [{
        type: "sunburst", radius: [30, 120], nodeClick: false, label: { show: false },
        emphasis: { focus: "ancestor" as const },
        data: [
          { name: "Big", value: 60, itemStyle: { color: "#cccccc" },
            children: [{ name: "Other", value: 60, itemStyle: { color: OUTER } }] },
          { name: "Other", value: 40, itemStyle: { color: ROOT_OTHER },
            children: [{ name: "x", value: 40, itemStyle: { color: "#dddddd" } }] },
        ],
      }],
    };
    // Depth-first pre-order: root(0), Big(1), Other(2, a CHILD of Big), Other(3, ring 0), x(4).
    // `indexOfName("Other")` would return 2 — the wrong one.
    const { container, rerender } = render(<EChartsRenderer option={ambiguous} />);
    rerender(<EChartsRenderer option={ambiguous} highlightDataIndex={3} />);
    await settle();
    expect(blurred(container)).toBe(3);
    // The one still lit is ring 0's "Other", not the nested one that shares its name.
    const still = [...container.querySelectorAll("svg path")].find((p) => {
      const o = p.getAttribute("fill-opacity");
      return o == null || parseFloat(o) >= 1;
    })!;
    expect(still.getAttribute("fill")).not.toBe(OUTER);
  });

  it("lets the highlight go again", async () => {
    const { container, rerender } = render(
      <EChartsRenderer option={sunburst("22,437")} highlightDataIndex={1} />,
    );
    await settle();
    expect(blurred(container)).toBe(4);
    rerender(<EChartsRenderer option={sunburst("22,437")} highlightDataIndex={undefined} />);
    await settle();
    expect(blurred(container)).toBe(0);
  });
});

// THE REGRESSION THIS FILE WAS WRITTEN FOR. Updating the hole means calling `setOption` while the
// chart is mid-hover, and if that discarded ECharts' emphasis state the chart would stop dimming —
// visibly broken, with every mocked test still green.
describe("EChartsRenderer against real ECharts — the overlay and the highlight together", () => {
  it("keeps the chart dimmed while the hole's readout changes underneath it", async () => {
    const { container, rerender } = render(
      <EChartsRenderer option={sunburst("22,437")} overlay={hole(22437, "Total")} highlightDataIndex={1} />,
    );
    await settle();
    expect(blurred(container)).toBe(4);

    rerender(
      <EChartsRenderer option={sunburst("22,437")} overlay={hole(60, "A")} highlightDataIndex={1} />,
    );
    await settle();
    expect(texts(container).slice(0, 2)).toEqual(["60", "A"]);
    expect(blurred(container)).toBe(4); // ...and the dimming is still there
  });

  it("still highlights when the overlay arrives in the same render", async () => {
    const { container, rerender } = render(
      <EChartsRenderer option={sunburst("22,437")} overlay={hole(22437, "Total")} />,
    );
    rerender(
      <EChartsRenderer option={sunburst("22,437")} overlay={hole(40, "B")} highlightDataIndex={4} />,
    );
    await settle();
    expect(texts(container).slice(0, 2)).toEqual(["40", "B"]);
    expect(blurred(container)).toBe(4);
  });
});
