import { describe, it, expect } from "vitest";
import { buildPieOption } from "../options/pie";
import { SERIES_PALETTE, seriesColor, lightenOklch } from "../palette";

const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["EU", 10], ["US", 30]];

describe("buildPieOption", () => {
  it("maps rows to {name, value} slices, sorted by value desc", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
    expect(opt.series[0].type).toBe("pie");
    expect(opt.series[0].data).toEqual([{ name: "US", value: 30 }, { name: "EU", value: 10 }]);
    expect(opt.series[0].radius).toEqual(["0%", "70%"]);
  });
  it("donut sets an inner radius", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", donut: true }) as any;
    expect(opt.series[0].radius).toEqual(["45%", "70%"]);
  });
  it("empty when dimension or metric missing", () => {
    expect((buildPieOption(cols, rows, {}) as any).series[0].data).toEqual([]);
  });
});

// Behaviors ported from Metabase's pie unit specs (visualizations/echarts/pie/*),
// scoped to what egret's flat buildPieOption supports: palette, percent tooltip,
// legend + label toggles. (Metabase-only: "Other" grouping, negative handling,
// donut centre total, percent-visibility modes — tracked as fidelity gaps.)
describe("buildPieOption — Metabase-parity", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["EU", 10], ["US", 30]];
  const base = { dimensions: ["region"], metric: "rev" };

  it("uses the categorical palette", () => {
    expect((buildPieOption(cols, rows, base) as any).color).toEqual(SERIES_PALETTE);
  });
  it("item tooltip carries a percent formatter", () => {
    const opt = buildPieOption(cols, rows, base) as any;
    expect(opt.tooltip.trigger).toBe("item");
    expect(opt.tooltip.formatter({ name: "EU", value: 10, percent: 25 })).toContain("25%");
  });
  it("showLegend toggles the legend", () => {
    expect((buildPieOption(cols, rows, { ...base, showLegend: true }) as any).legend).toMatchObject({ type: "scroll" });
    expect((buildPieOption(cols, rows, base) as any).legend).toBeUndefined();
  });
  it("showLabels toggles slice labels", () => {
    expect((buildPieOption(cols, rows, { ...base, showLabels: true }) as any).series[0].label.show).toBe(true);
    expect((buildPieOption(cols, rows, base) as any).series[0].label.show).toBe(false);
  });
});

// Phase 1a — pie data robustness, clean-room from Metabase's pie/model/index.ts
// (negative/zero handling) + constants.ts (SLICE_THRESHOLD = 2.5%) + "Other" grouping.
describe("buildPieOption — data robustness (Metabase parity)", () => {
  const cols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
  const slices = (rows: unknown[][], extra: Record<string, unknown> = {}) =>
    (buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", ...extra }) as any).series[0].data;

  it("drops zero-value slices", () => {
    expect(slices([["A", 10], ["B", 0], ["C", 5]]).map((s: any) => s.name)).toEqual(["A", "C"]);
  });
  it("drops negative slices when signs are mixed", () => {
    expect(slices([["A", 10], ["B", -3], ["C", 5]]).map((s: any) => s.name)).toEqual(["A", "C"]);
  });
  it("uses absolute values when every slice is negative", () => {
    expect(slices([["A", -10], ["B", -30]])).toEqual([{ name: "B", value: 30 }, { name: "A", value: 10 }]);
  });
  it("sorts slices by value descending by default", () => {
    expect(slices([["A", 5], ["B", 30], ["C", 10]]).map((s: any) => s.name)).toEqual(["B", "C", "A"]);
  });
  it("preserves input order when sortRows is false", () => {
    expect(slices([["A", 5], ["B", 30]], { sortRows: false }).map((s: any) => s.name)).toEqual(["A", "B"]);
  });
  it("groups sub-threshold slices (<2.5%) into a single Other slice", () => {
    const d = slices([["A", 50], ["B", 45], ["C", 2], ["D", 2], ["E", 1]]); // C,D,E each <2.5% of 100
    expect(d.map((s: any) => s.name)).toEqual(["A", "B", "Other"]);
    expect(d.find((s: any) => s.name === "Other").value).toBe(5);
  });
  it("does not create Other for a single sub-threshold slice", () => {
    const d = slices([["A", 50], ["B", 49], ["C", 1]]); // only C <2.5%
    expect(d.map((s: any) => s.name)).toEqual(["A", "B", "C"]);
  });
  it("the threshold is configurable and 0 disables Other grouping", () => {
    const d = slices([["A", 50], ["B", 45], ["C", 2], ["D", 2], ["E", 1]], { sliceThreshold: 0 });
    expect(d.map((s: any) => s.name)).toEqual(["A", "B", "C", "D", "E"]);
  });
  it("the Other slice is muted; normal slices defer to the palette", () => {
    const d = slices([["A", 50], ["B", 45], ["C", 2], ["D", 2], ["E", 1]]);
    expect(d.find((s: any) => s.name === "Other").itemStyle.color).toBeTruthy();
    expect(d.find((s: any) => s.name === "A").itemStyle).toBeUndefined();
  });
});

// Phase 1b — donut centre total + percent visibility, clean-room from Metabase's
// pie/option.ts (getTotalGraphicOption + getSliceLabel / pie.percent_visibility).
describe("buildPieOption — donut total & percent (Metabase parity 1b)", () => {
  const cols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
  const rows = [["A", 30], ["B", 70]];
  const opt = (extra: Record<string, unknown> = {}) =>
    buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", ...extra }) as any;
  const labelText = (o: any, p: { name: string; percent: number }) => o.series[0].label.formatter(p);

  it("renders a centre total only for donut + showTotal", () => {
    expect(opt({ showTotal: true }).graphic).toBeFalsy();           // not a donut
    expect(opt({ donut: true }).graphic).toBeFalsy();               // showTotal off
    expect(opt({ donut: true, showTotal: true }).graphic).toBeTruthy();
  });
  it("centre total shows the formatted grand total + a Total caption", () => {
    const g = opt({ donut: true, showTotal: true, metricTypes: { m: "double" }, columnSettings: { m: { prefix: "$", decimals: 0 } } }).graphic;
    const texts = g.map((e: any) => e.style?.text);
    expect(texts).toContain("$100");
    expect(texts).toContain("Total");
  });
  it("percentVisibility defaults off → label is the name only", () => {
    const o = opt({ showLabels: true });
    expect(o.series[0].label.show).toBe(true);
    expect(labelText(o, { name: "A", percent: 30 })).toBe("A");
  });
  it("percentVisibility inside → name + percent", () => {
    const o = opt({ showLabels: true, percentVisibility: "inside" });
    expect(labelText(o, { name: "A", percent: 30 })).toBe("A: 30%");
  });
  it("percent without labels shows just the percent", () => {
    const o = opt({ percentVisibility: "inside" });
    expect(o.series[0].label.show).toBe(true);
    expect(labelText(o, { name: "A", percent: 33.333 })).toBe("33.33%");
  });
});

// Task 4 — nested rings: pie.dimensions draws up to three nested rings, inner to outer,
// each ring going through buildSlices' own robustness rules (sign handling, sort, Other-fold).
describe("buildPieOption — nested rings (task 4)", () => {
  it("one dimension still renders exactly one series, unchanged", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
    expect(opt.series).toHaveLength(1);
    expect(opt.series[0].data).toEqual([{ name: "US", value: 30 }, { name: "EU", value: 10 }]);
    expect(opt.series[0].radius).toEqual(["0%", "70%"]);
  });

  it("two dimensions render two series, at different radii, inner first", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "rev", type: "double" },
    ];
    const rows2 = [
      ["US", "Widgets", 30], ["US", "Gadgets", 10],
      ["EU", "Widgets", 5], ["EU", "Gadgets", 5],
    ];
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "rev" }) as any;
    expect(opt.series).toHaveLength(2);
    const [inner, outer] = opt.series;
    expect(parseFloat(inner.radius[0])).toBeLessThan(parseFloat(outer.radius[0]));
    expect(parseFloat(inner.radius[1])).toBeLessThanOrEqual(parseFloat(outer.radius[1]));
    // inner ring = region totals (US 40, EU 10), sorted desc
    expect(inner.data.map((d: any) => ({ name: d.name, value: d.value }))).toEqual([
      { name: "US", value: 40 }, { name: "EU", value: 10 },
    ]);
  });

  it("the outer ring's slices are contiguous per parent, in the parent's own order", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "rev", type: "double" },
    ];
    // Region totals: EU=21, US=8 -> parent order EU, US.
    // A naive global sort-by-value on the outer ring would interleave: EU Widgets(20), US Widgets(5), US Gadgets(3), EU Gadgets(1).
    const rows2 = [
      ["US", "Widgets", 5], ["US", "Gadgets", 3],
      ["EU", "Widgets", 20], ["EU", "Gadgets", 1],
    ];
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "rev" }) as any;
    const outer = opt.series[1].data;
    expect(outer.map((d: any) => d.name)).toEqual(["Widgets", "Gadgets", "Widgets", "Gadgets"]);
    expect(outer.map((d: any) => d.value)).toEqual([20, 1, 5, 3]);
  });

  it("buildSlices' sign/zero rules apply to the outer ring exactly like the inner", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    // Outer ring values across both parents: 10, -5 (mixed sign -> dropped), 5, 0 (dropped).
    const rows2 = [["R1", "A", 10], ["R1", "B", -5], ["R2", "C", 5], ["R2", "D", 0]];
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" }) as any;
    const outer = opt.series[1].data;
    expect(outer.map((d: any) => d.name)).toEqual(["A", "C"]);
    expect(outer.map((d: any) => d.value)).toEqual([10, 5]);
  });

  it("the Other fold applies per ring, independently, and only when it merges 2+ slices", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    // Inner ring: R1=995, R2=1000 -- both well above the 2.5% threshold, no Other.
    // Outer ring (total 2000, threshold cut at 50): R1 -> A(990) big, B(5) small;
    // R2 -> C(985) big, D(5) E(5) F(5) small. 4 small slices ring-wide -> one Other(20).
    const rows2 = [
      ["R1", "A", 990], ["R1", "B", 5],
      ["R2", "C", 985], ["R2", "D", 5], ["R2", "E", 5], ["R2", "F", 5],
    ];
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" }) as any;
    const inner = opt.series[0].data;
    expect(inner.map((d: any) => d.name)).toEqual(["R2", "R1"]);
    expect(inner.some((d: any) => d.name === "Other")).toBe(false);

    const outer = opt.series[1].data;
    expect(outer.map((d: any) => d.name)).toEqual(["C", "A", "Other"]);
    expect(outer.find((d: any) => d.name === "Other").value).toBe(20);
  });

  it("does not fold the outer ring's Other when only one slice is sub-threshold", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows2 = [["R1", "A", 50], ["R1", "B", 49], ["R1", "C", 1]]; // only C < 2.5% of 100
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" }) as any;
    expect(opt.series[1].data.map((d: any) => d.name)).toEqual(["A", "B", "C"]);
  });

  it("a null outer dimension value does not break or drop its parent's slice", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows2 = [["R1", null, 10], ["R2", "X", 5]];
    expect(() => buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" })).not.toThrow();
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" }) as any;
    const inner = opt.series[0].data;
    expect(inner).toHaveLength(2);
    expect(inner.map((d: any) => ({ name: d.name, value: d.value }))).toEqual(
      expect.arrayContaining([{ name: "R1", value: 10 }, { name: "R2", value: 5 }]),
    );
  });

  // Asserts a ring's radii are strictly increasing, non-overlapping (each ring's outer edge is
  // exactly the next ring's inner edge), and together span [holeStart, 70%] with no gap.
  function assertContiguousRadii(series: { radius: [string, string] }[], holeStart: number) {
    const bounds = series.map((s) => s.radius.map((r) => parseFloat(r)));
    expect(bounds[0][0]).toBe(holeStart);
    for (let i = 0; i < bounds.length; i++) {
      expect(bounds[i][0]).toBeLessThan(bounds[i][1]); // each ring itself has positive thickness
      if (i > 0) expect(bounds[i][0]).toBe(bounds[i - 1][1]); // contiguous: no gap, no overlap
    }
    expect(bounds[bounds.length - 1][1]).toBe(70); // spans the full outer edge
  }

  it("three dimensions render three series at three strictly-increasing, non-overlapping radii", () => {
    const cols3 = [
      { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
      { name: "brand", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows3 = [
      ["US", "Widgets", "Acme", 10], ["US", "Widgets", "Zenith", 5], ["EU", "Gadgets", "Acme", 3],
    ];
    const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
    expect(opt.series).toHaveLength(3);
    assertContiguousRadii(opt.series, 0);
  });

  it("donut hollows the centre with one ring, and with three rings the centre stays clear", () => {
    const one = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", donut: true }) as any;
    expect(one.series[0].radius[0]).toBe("45%");

    const cols3 = [
      { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
      { name: "brand", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows3 = [
      ["US", "Widgets", "Acme", 10], ["US", "Widgets", "Zenith", 5], ["EU", "Gadgets", "Acme", 3],
    ];
    const three = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m", donut: true }) as any;
    expect(three.series).toHaveLength(3);
    assertContiguousRadii(three.series, 45);
  });

  it("names each ring's series after its own dimension, so a click can be attributed to the right ring", () => {
    const cols3 = [
      { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
      { name: "brand", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows3 = [
      ["US", "Widgets", "Acme", 10], ["US", "Widgets", "Zenith", 5], ["EU", "Gadgets", "Acme", 3],
    ];
    const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
    expect(opt.series.map((s: any) => s.name)).toEqual(["region", "category", "brand"]);
  });

  it("says so in the aria description when a ring's total diverges from the first ring's", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    // Region totals: R1 = 2 + (-7) = -5, R2 = 1 + (-4) = -3 -- BOTH negative -> the inner ring
    // abs-converts them (5, 3; total 8). The outer ring's own items are NOT all negative (2 and 1
    // are positive) -> mixed-sign drop instead of abs-convert -> keeps only 2 and 1 (total 3).
    const rows2 = [["R1", "A", 2], ["R1", "B", -7], ["R2", "C", 1], ["R2", "D", -4]];
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" }) as any;
    expect(opt.series[0].data.reduce((t: number, s: any) => t + s.value, 0)).toBe(8);
    expect(opt.series[1].data.reduce((t: number, s: any) => t + s.value, 0)).toBe(3);
    expect(opt.aria.label.description).toContain("cat ring totals 3");
    expect(opt.aria.label.description).toContain("not 8");
  });

  it("does not add a divergence note when every ring's total matches", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
    expect(opt.aria.label.description).not.toContain("Note:");
  });
});

// Task 5 — ring colour inheritance: an outer ring's slice takes its parent slice's own
// colour, lightened one OKLab step per ring, instead of an unrelated palette colour (Metabase
// getRingColorAlias). Single-ring pies are untouched — see the "unchanged" test in the
// nested-rings block above, which asserts ring 0 still carries no itemStyle at all.
describe("buildPieOption — ring colour inheritance (task 5)", () => {
  const cols2 = [
    { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "rev", type: "double" },
  ];
  const rows2 = [
    ["US", "Widgets", 30], ["US", "Gadgets", 10],
    ["EU", "Widgets", 5], ["EU", "Gadgets", 5],
  ];

  it("an outer slice inherits its parent's colour, lightened one step", () => {
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "rev" }) as any;
    const inner = opt.series[0].data; // [US(40), EU(10)] — implicit palette, no itemStyle
    expect(inner.map((d: any) => d.itemStyle)).toEqual([undefined, undefined]);

    const outer = opt.series[1].data;
    const usColor = seriesColor(inner.findIndex((d: any) => d.name === "US"));
    const euColor = seriesColor(inner.findIndex((d: any) => d.name === "EU"));
    const widgetsUnderUS = outer.find((d: any) => d.name === "Widgets" && d.value === 30);
    const widgetsUnderEU = outer.find((d: any) => d.name === "Widgets" && d.value === 5);
    expect(widgetsUnderUS.itemStyle.color).toBe(lightenOklch(usColor, 1));
    expect(widgetsUnderEU.itemStyle.color).toBe(lightenOklch(euColor, 1));
  });

  it("a sliceColors override wins, and its children lighten from the override, not the palette colour it replaced", () => {
    const opt = buildPieOption(cols2, rows2, {
      dimensions: ["region", "category"], metric: "rev", sliceColors: { US: "#123456" },
    }) as any;
    const inner = opt.series[0].data;
    expect(inner.find((d: any) => d.name === "US").itemStyle.color).toBe("#123456");

    const outer = opt.series[1].data;
    const widgetsUnderUS = outer.find((d: any) => d.name === "Widgets" && d.value === 30);
    // Must lighten from the OVERRIDE ("#123456"), not from seriesColor(0) — the palette
    // colour US would otherwise have gotten.
    expect(widgetsUnderUS.itemStyle.color).toBe(lightenOklch("#123456", 1));
    expect(widgetsUnderUS.itemStyle.color).not.toBe(lightenOklch(seriesColor(0), 1));
  });

  it("a ring's children whose parent folded into the PARENT ring's Other bucket inherit Other's own muted colour, lightened", () => {
    const cols3 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    // Ring0 (region) totals: R1=960 (big), R2=20 (small), R3=100-80=20 (small) — R2 and R3 fold
    // into ring0's Other (two small slices). At ring1 (region,cat), R3's row is split into a big
    // positive (Y1=100) and a negative (Y2=-80, dropped by the sign filter): R3-Y1 survives ring1
    // on its own merits (100 is well above ring1's own threshold), and R2-X(20) is the ONLY
    // below-threshold item left at ring1 (one small slice doesn't trigger a fold there) — so both
    // survive as distinct outer slices, each with a parent that only exists via ring0's Other.
    const rows3 = [
      ["R1", "A", 960],
      ["R2", "X", 20],
      ["R3", "Y1", 100],
      ["R3", "Y2", -80],
    ];
    const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "cat"], metric: "m" }) as any;
    const inner = opt.series[0].data;
    const other = inner.find((d: any) => d.name === "Other");
    expect(other).toBeTruthy();
    expect(other.value).toBe(40);
    const otherColor = other.itemStyle.color;

    const outer = opt.series[1].data;
    const x = outer.find((d: any) => d.name === "X");
    const y1 = outer.find((d: any) => d.name === "Y1");
    expect(x).toBeTruthy();
    expect(y1).toBeTruthy();
    expect(x.itemStyle.color).toBe(lightenOklch(otherColor, 1));
    expect(y1.itemStyle.color).toBe(lightenOklch(otherColor, 1));
  });
});
