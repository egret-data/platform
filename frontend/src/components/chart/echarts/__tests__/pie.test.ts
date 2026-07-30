import { describe, it, expect } from "vitest";
import {
  buildPieOption, buildPieLegend, buildPieHole, buildPieHoleGraphic, pieLegendPlacement, resolvePercentVisibility,
  PIE_LEGEND_COLUMN_WIDTH_PX,
} from "../options/pie";
import { SERIES_PALETTE, seriesColor, lightenOklch } from "../palette";
import { calcAvailableDonutSliceLabelLength } from "../sliceLabel";
import { itemTooltip } from "../base";

const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["EU", 10], ["US", 30]];

/**
 * The slices exactly `level` rings out from the centre, in draw order.
 *
 * The pie is ONE `sunburst` series whose `data` is a nested `children` tree, so a ring is a DEPTH
 * in that tree rather than a series of its own. Flattening depth-first is what reproduces the flat
 * per-ring array the old N-independent-`pie`-series version produced: it visits each parent's
 * children consecutively, in the parents' own order, which is exactly the contiguous-per-parent
 * layout `reorderByParent` used to have to construct by hand (and which the sunburst now gets
 * structurally, since ECharts lays a node's children out inside that node's arc).
 */
const ring = (opt: any, level: number): any[] => {
  let nodes: any[] = opt.series[0].data;
  for (let i = 0; i < level; i++) nodes = nodes.flatMap((n: any) => n.children ?? []);
  return nodes;
};

/** How many rings the tree actually draws — its own depth, counted from the data. */
const treeDepth = (nodes: any[]): number =>
  nodes.length === 0 ? 0 : 1 + Math.max(...nodes.map((n: any) => treeDepth(n.children ?? [])));

describe("buildPieOption", () => {
  it("maps rows to {name, value} slices, sorted by value desc", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
    expect(opt.series[0].type).toBe("sunburst");
    // Each datum also carries its own `label` (per-slice fit + contrast colour, see the label
    // describe block below) — picked out here so this test stays about mapping/sorting.
    expect(ring(opt, 0).map((d: any) => ({ name: d.name, value: d.value }))).toEqual([
      { name: "US", value: 30 },
      { name: "EU", value: 10 },
    ]);
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

// The change this file's whole shape follows from: N stacked, independent `pie` series became ONE
// `sunburst`, exactly as Metabase draws its own pies. Independent series cannot express ancestry —
// which is why hovering an outer slice used to change 3 of 55 wedges where Metabase changes 51 of
// 53, and why an outer ring's wedges lined up with their parents' only by coincidence.
describe("buildPieOption — one sunburst series, not N pies", () => {
  const cols3 = [
    { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
    { name: "brand", type: "varchar" }, { name: "m", type: "double" },
  ];
  const rows3 = [
    ["US", "Widgets", "Acme", 10], ["US", "Widgets", "Zenith", 5], ["EU", "Gadgets", "Acme", 3],
  ];

  it("is exactly one series, of type sunburst, whatever the dimension count", () => {
    for (const [c, r, dimensions] of [
      [cols, rows, ["region"]],
      [cols3, rows3, ["region", "category"]],
      [cols3, rows3, ["region", "category", "brand"]],
    ] as const) {
      const opt = buildPieOption(c as any, r as any, { dimensions: [...dimensions], metric: dimensions.length === 1 ? "rev" : "m" }) as any;
      expect(opt.series).toHaveLength(1);
      expect(opt.series[0].type).toBe("sunburst");
    }
  });

  it("nests the rings as a children tree whose depth is the dimension count", () => {
    for (const dimensions of [["region"], ["region", "category"], ["region", "category", "brand"]]) {
      const opt = buildPieOption(cols3, rows3, { dimensions, metric: "m" }) as any;
      expect(treeDepth(ring(opt, 0))).toBe(dimensions.length);
    }
    // ...and the nesting is real parentage, not just depth: US's children are its own categories.
    const two = buildPieOption(cols3, rows3, { dimensions: ["region", "category"], metric: "m" }) as any;
    const us = ring(two, 0).find((d: any) => d.name === "US");
    expect(us.children.map((c: any) => ({ name: c.name, value: c.value }))).toEqual([{ name: "Widgets", value: 15 }]);
  });

  it("focuses a hovered slice's ancestors, and fades the rest by COLOUR rather than opacity", () => {
    const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
    expect(opt.series[0].emphasis).toEqual({ focus: "ancestor" });

    for (const level of [0, 1, 2]) {
      for (const d of ring(opt, level)) {
        // Fading via `itemStyle.opacity` would take the border with it, letting the fill leak
        // through the separator — so the fade lives in the fill colour and opacity stays at 1.
        expect(d.blur.itemStyle.opacity).toBe(1);
        expect(d.blur.itemStyle.color).not.toBe(d.itemStyle.color);
        expect(d.blur.itemStyle.color).toMatch(/^rgba\(\d+, \d+, \d+, 0\.3\)$/);
      }
    }
  });

  it("turns animation off, leaves sorting to the data's own order, and never zooms on a click", () => {
    const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category"], metric: "m" }) as any;
    expect(opt.animation).toBe(false);
    // The key must be PRESENT and undefined: that is what stops ECharts' `sort: 'desc'` default
    // from merging in and silently re-sorting `sortRows: false` / `sliceOrder` away.
    expect("sort" in opt.series[0]).toBe(true);
    expect(opt.series[0].sort).toBeUndefined();
    expect(opt.series[0].nodeClick).toBe(false);
  });

  it("keeps `sortRows: false` order, which ECharts' own sunburst default would have undone", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", sortRows: false }) as any;
    expect(ring(opt, 0).map((d: any) => d.name)).toEqual(["EU", "US"]);
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
    // A sunburst hands its tooltip formatter a root-to-node PATH (entry 0 being ECharts' virtual
    // root) and no `percent` of its own — see the breakdown suite at the end of this file.
    expect(opt.tooltip.formatter({ treePathInfo: [{ name: "" }, { name: "EU" }] })).toContain("25%");
  });
  // Both default ON, resolved here at render time rather than stored — so an already-saved
  // chart with no showLegend/showLabels picks up the fix too. `?? true`, not `|| true`: an
  // explicit `false` must still turn each off, not be treated as "unset".
  it("showLegend defaults on; an explicit false turns it off", () => {
    expect(buildPieLegend(cols, rows, base).map((e) => e.name)).toEqual(["US", "EU"]);
    expect(buildPieLegend(cols, rows, { ...base, showLegend: true }).map((e) => e.name)).toEqual(["US", "EU"]);
    expect(buildPieLegend(cols, rows, { ...base, showLegend: false })).toEqual([]);
  });
  it("showLabels defaults on; an explicit false turns it off", () => {
    expect((buildPieOption(cols, rows, base) as any).series[0].label.show).toBe(true);
    expect((buildPieOption(cols, rows, { ...base, showLabels: true }) as any).series[0].label.show).toBe(true);
    expect((buildPieOption(cols, rows, { ...base, showLabels: false }) as any).series[0].label.show).toBe(false);
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
    expect(slices([["A", -10], ["B", -30]]).map((s: any) => ({ name: s.name, value: s.value }))).toEqual([
      { name: "B", value: 30 },
      { name: "A", value: 10 },
    ]);
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
  it("the Other slice is muted; normal slices take their palette slot", () => {
    const d = slices([["A", 50], ["B", 45], ["C", 2], ["D", 2], ["E", 1]]);
    const other = d.find((s: any) => s.name === "Other").itemStyle.color;
    expect(other).toBeTruthy();
    // Ring 0's palette slots are pinned onto the slice rather than left implicit — a sunburst
    // assigns them by NAME and skips any slice that already has a colour, so an implicit slot
    // would shift the moment one slice carried an override. See the colour-inheritance block.
    expect(d.find((s: any) => s.name === "A").itemStyle.color).toBe(seriesColor(0));
    expect(other).not.toBe(seriesColor(0));
  });
});

// Phase 1b — donut centre total + percent visibility, clean-room from Metabase's
// pie/option.ts (getTotalGraphicOption + getSliceLabel / pie.percent_visibility).
describe("buildPieOption — donut total & percent (Metabase parity 1b)", () => {
  const cols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
  const rows = [["A", 30], ["B", 70]];
  const opt = (extra: Record<string, unknown> = {}) =>
    buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", ...extra }) as any;
  // The label text a slice ACTUALLY renders. Each datum carries its own `label.formatter`, which
  // always overrides the series-level one, so a series-level formatter could never run in the app
  // — these tests used to assert against exactly that dead code path, leaving the real label
  // composition (name / percent / decimals) with no direct test at all. Reading it per-datum also
  // means the percentage under test is the one the chart computed from the fixture, not one the
  // test handed in.
  const labelText = (o: any, name: string) =>
    ring(o, 0).find((d: any) => d.name === name).label.formatter();

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
    expect(labelText(o, "A")).toBe("A");
  });
  it("percentVisibility inside → name + percent", () => {
    const o = opt({ showLabels: true, percentVisibility: "inside" });
    expect(labelText(o, "A")).toBe("A: 30%"); // A is 30 of the 100 this fixture totals
  });
  it("percent without labels shows just the percent", () => {
    // showLabels now defaults on, so it must be explicitly turned off here to isolate
    // percentVisibility's own contribution to `label.show`.
    const thirds = buildPieOption(cols, [["A", 1], ["B", 2]], {
      dimensions: ["d"], metric: "m", percentVisibility: "inside", showLabels: false,
    }) as any;
    expect(thirds.series[0].label.show).toBe(true);
    expect(labelText(thirds, "A")).toBe("33.33%"); // 1/3, rounded to the default 2 decimals
  });
});

// Task 4 — nested rings: pie.dimensions draws up to three nested rings, inner to outer,
// each ring going through buildSlices' own robustness rules (sign handling, sort, Other-fold).
describe("buildPieOption — nested rings (task 4)", () => {
  it("one dimension still renders exactly one series, unchanged", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
    expect(opt.series).toHaveLength(1);
    expect(ring(opt, 0).map((d: any) => ({ name: d.name, value: d.value }))).toEqual([
      { name: "US", value: 30 },
      { name: "EU", value: 10 },
    ]);
    expect(opt.series[0].radius).toEqual(["0%", "70%"]);
  });

  it("two dimensions nest as two rings, the inner one aggregating the outer", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "rev", type: "double" },
    ];
    const rows2 = [
      ["US", "Widgets", 30], ["US", "Gadgets", 10],
      ["EU", "Widgets", 5], ["EU", "Gadgets", 5],
    ];
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "rev" }) as any;
    expect(treeDepth(opt.series[0].data)).toBe(2);
    // inner ring = region totals (US 40, EU 10), sorted desc
    expect(ring(opt, 0).map((d: any) => ({ name: d.name, value: d.value }))).toEqual([
      { name: "US", value: 40 }, { name: "EU", value: 10 },
    ]);
    expect(ring(opt, 1)).toHaveLength(4);
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
    const outer = ring(opt, 1);
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
    const outer = ring(opt, 1);
    expect(outer.map((d: any) => d.name)).toEqual(["A", "C"]);
    expect(outer.map((d: any) => d.value)).toEqual([10, 5]);
  });

  it("the Other fold applies per PARENT, independently, and only when it merges 2+ slices within that parent", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    // Inner ring: R1=995, R2=1000 -- both well above the 2.5% threshold, no Other.
    // Outer ring, judged per PARENT (not ring-wide): R1's own total is 995, threshold cut at
    // ~24.9 -> A(990) big, B(5) is the ONLY small slice under R1, so the merge-2+ rule blocks a
    // fold there and B survives on its own. R2's own total is 1000, threshold cut at 25 ->
    // C(985) big, D(5) E(5) F(5) are 3 small slices under R2 -> R2 gets its OWN Other(15). There
    // is no ring-wide Other merging both parents' small slices together.
    const rows2 = [
      ["R1", "A", 990], ["R1", "B", 5],
      ["R2", "C", 985], ["R2", "D", 5], ["R2", "E", 5], ["R2", "F", 5],
    ];
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" }) as any;
    const inner = ring(opt, 0);
    expect(inner.map((d: any) => d.name)).toEqual(["R2", "R1"]);
    expect(inner.some((d: any) => d.name === "Other")).toBe(false);

    const outer = ring(opt, 1);
    // Contiguous per parent, in the parent's own order (R2 first, then R1): C, R2's Other(15),
    // then A, B (B unfolded — only one small slice under R1).
    expect(outer.map((d: any) => d.name)).toEqual(["C", "Other", "A", "B"]);
    expect(outer.find((d: any) => d.name === "Other")!.value).toBe(15);
    // Exactly one "Other" wedge in the ring, and it belongs to R2 alone — not a ring-wide bucket.
    expect(outer.filter((d: any) => d.name === "Other")).toHaveLength(1);
  });

  it("does not fold the outer ring's Other when only one slice is sub-threshold", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows2 = [["R1", "A", 50], ["R1", "B", 49], ["R1", "C", 1]]; // only C < 2.5% of 100
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" }) as any;
    expect(ring(opt, 1).map((d: any) => d.name)).toEqual(["A", "B", "C"]);
  });

  it("a null outer dimension value does not break or drop its parent's slice", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows2 = [["R1", null, 10], ["R2", "X", 5]];
    expect(() => buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" })).not.toThrow();
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "cat"], metric: "m" }) as any;
    const inner = ring(opt, 0);
    expect(inner).toHaveLength(2);
    expect(inner.map((d: any) => ({ name: d.name, value: d.value }))).toEqual(
      expect.arrayContaining([{ name: "R1", value: 10 }, { name: "R2", value: 5 }]),
    );
  });

  // The rings no longer state their own radii: there is ONE [inner, outer] band and ECharts'
  // sunburstLayout splits it evenly across the tree's own depth (`rPerLevel = (r - r0) / levels`).
  // So what this can still pin is the band itself — where the hole starts and where the outer edge
  // lands — plus the depth that band is divided by.
  it("three dimensions draw three rings inside one radius band, starting at Metabase's 1/4 hole", () => {
    const cols3 = [
      { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
      { name: "brand", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows3 = [
      ["US", "Widgets", "Acme", 10], ["US", "Widgets", "Zenith", 5], ["EU", "Gadgets", "Acme", 3],
    ];
    const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
    expect(treeDepth(opt.series[0].data)).toBe(3);
    // Metabase's threeRingInnerRadiusRatio (1/4) is unconditional once there are 2+ rings, even
    // with `donut` off — see the "Metabase ring geometry" describe block below.
    expect(opt.series[0].radius).toEqual(["17.5%", "70%"]);
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
    expect(treeDepth(three.series[0].data)).toBe(3);
    // Metabase's threeRingInnerRadiusRatio (1/4 of the 70%-outer-edge scale this file uses) — see
    // the "Metabase ring geometry" describe block below for why this replaced the old flat 45%.
    expect(three.series[0].radius).toEqual(["17.5%", "70%"]);
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
    expect(ring(opt, 0).reduce((t: number, s: any) => t + s.value, 0)).toBe(8);
    expect(ring(opt, 1).reduce((t: number, s: any) => t + s.value, 0)).toBe(3);
    expect(opt.aria.label.description).toContain("cat ring totals 3");
    expect(opt.aria.label.description).toContain("not 8");
  });

  it("does not add a divergence note when every ring's total matches", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
    expect(opt.aria.label.description).not.toContain("Note:");
  });
});

// Bug fix — a nested ring's Other threshold folds within each PARENT's own children, not
// across the whole ring. Before this fix, buildSlices judged every slice at a given ring depth
// against that RING's total, all at once, regardless of which parent it belonged to — so a ring
// with many parents (each individually fine) could still see its small-but-real slices, scattered
// across every parent, merged into one "Other" wedge that belonged to none of them (it carried no
// parentKey, so reorderByParent's "no resolvable group" fallback shoved it to the very end of the
// ring instead of sitting inside any parent's arc). Ring 0 is unaffected: its items have no
// parentKey (one implicit parent, the whole chart), so grouping by parentKey there degenerates
// to the old ring-wide behaviour — see the "single-ring pie is unchanged" tests above.
describe("buildPieOption — per-parent Other fold (bug fix)", () => {
  const cols2 = [
    { name: "grp", type: "varchar" }, { name: "item", type: "varchar" }, { name: "m", type: "double" },
  ];
  // Three parents, each totalling 1000 (so ring 0 never folds — no parent is remotely
  // sub-threshold of the 3000 grand total):
  //  P1: two big (A1 800, A2 180) + two small (b1 10, b2 10) -- 2 small -> P1 gets its OWN Other(20).
  //  P2: two big (C1 700, C2 290) + ONE small (c_small 10) -- only 1 small -> merge-2+ rule
  //      blocks the fold, c_small survives as itself (this is test 3).
  //  P3: two big (D1 900, D2 70) + two small (d1 15, d2 15) -- 2 small -> P3 gets its OWN
  //      Other(30), a DIFFERENT value from P1's (this is what proves test 4: each Other sums only
  //      its own parent's small children, not another parent's).
  const rows2 = [
    ["P1", "A1", 800], ["P1", "A2", 180], ["P1", "b1", 10], ["P1", "b2", 10],
    ["P2", "C1", 700], ["P2", "C2", 290], ["P2", "c_small", 10],
    ["P3", "D1", 900], ["P3", "D2", 70], ["P3", "d1", 15], ["P3", "d2", 15],
  ];
  const outer = () =>
    ring(buildPieOption(cols2, rows2, { dimensions: ["grp", "item"], metric: "m" }) as any, 1);

  it("each parent with 2+ sub-threshold children gets its OWN Other, and there is no chart-wide Other merging every parent's small slices together", () => {
    const data = outer();
    const others = data.filter((d: any) => d.name === "Other");
    // Two Others (P1's and P3's) — never one ring-wide bucket collapsing all five originally-small
    // slices (b1, b2, c_small, d1, d2) into a single wedge with no parent.
    expect(others).toHaveLength(2);
    expect(others.map((o: any) => o.value).sort((a: number, b: number) => a - b)).toEqual([20, 30]);
  });

  it("a parent with only ONE sub-threshold child does not get an Other — the merge-2+ rule holds per parent", () => {
    const data = outer();
    expect(data.map((d: any) => d.name)).toContain("c_small");
    expect(data.find((d: any) => d.name === "c_small").value).toBe(10);
    // P2 contributes exactly its 3 real children, no Other of its own.
    const p2Names = ["C1", "C2", "c_small"];
    expect(data.filter((d: any) => p2Names.includes(d.name))).toHaveLength(3);
  });

  it("each Other sums only its own parent's small children, not another parent's", () => {
    const data = outer();
    const p1Other = data.find((d: any) => d.name === "Other" && d.value === 20); // b1 + b2
    const p3Other = data.find((d: any) => d.name === "Other" && d.value === 30); // d1 + d2
    expect(p1Other).toBeTruthy();
    expect(p3Other).toBeTruthy();
    // Neither Other is 60 (the old ring-wide bug: b1+b2+c_small+d1+d2) or any other cross-parent mix.
    expect(data.filter((d: any) => d.name === "Other").map((d: any) => d.value)).not.toContain(60);
  });

  // `otherKeyFor` gives each parent's "Other" bucket its OWN key, and that is still load-bearing
  // under the sunburst even though the nesting is structural now: the key is how a ring's slices
  // find WHICH parent-ring node to hang off (groupByParent). Collapse the two Others onto one key
  // and both of them claim the same child list — every folded-away grandchild appears under both.
  //
  // Only visible with three rings AND two parents that each fold: with two rings the Other buckets
  // have no children to mis-attribute, which is why the tests above stayed green when this was
  // deliberately broken.
  it("keeps each parent's Other bucket's own grandchildren under that Other, not under another parent's", () => {
    const cols3 = [
      { name: "region", type: "varchar" }, { name: "product", type: "varchar" },
      { name: "channel", type: "varchar" }, { name: "m", type: "double" },
    ];
    // Each region totals 1020, so ring 0 never folds. Within each region, the two 10-value
    // products are sub-threshold against their own parent's 1020, so each region gets its OWN
    // Other(20) at ring 1 — and each of those Others has its own channels at ring 2.
    const rows3 = [
      ["R1", "P1", "x", 1000],
      ["R1", "a", "A1", 6], ["R1", "a", "A2", 4],
      ["R1", "b", "B1", 10],
      ["R2", "P2", "y", 1000],
      ["R2", "c", "C1", 10],
      ["R2", "d", "D1", 10],
    ];
    const opt = buildPieOption(cols3, rows3, {
      dimensions: ["region", "product", "channel"], metric: "m",
    }) as any;

    const others = ring(opt, 1).filter((d: any) => d.name === "Other");
    expect(others).toHaveLength(2);
    const childNames = others.map((o: any) => o.children.map((c: any) => c.name).sort());
    expect(childNames).toContainEqual(["A1", "A2", "B1"]); // R1's folded products' channels
    expect(childNames).toContainEqual(["C1", "D1"]); // R2's
    // ...and nothing is claimed twice: 1 + 3 + 1 + 2 channels, no more.
    expect(ring(opt, 2)).toHaveLength(7);
  });

  it("each Other sits contiguous with that parent's other children, in the ring's ordering", () => {
    const data = outer().map((d: any) => d.name);
    // P1 (A1, A2, Other), then P2 (C1, C2, c_small), then P3 (D1, D2, Other) — each Other directly
    // adjacent to its own parent's real children, never separated out to the end of the ring.
    expect(data).toEqual(["A1", "A2", "Other", "C1", "C2", "c_small", "D1", "D2", "Other"]);
  });

  // Task 6 reproduction — the live bug: a 3 x 4 x 3 nested pie (e.g. region -> product ->
  // channel). The outer (channel) ring has 36 slices; each is only ~4-42% of its own (region,
  // product) parent's total (well above the 2.5% threshold there) but a much smaller sliver
  // (as little as ~1.6%) of the RING's 12000 grand total. The old ring-wide fold judged every
  // channel slice against the ring total and swallowed the smallest ones into one parentless
  // "Other"; the fix judges each channel against its OWN (region, product) parent's total, where
  // none of them are actually small.
  it("a 3 x 4 x 3 pie produces no ring-wide Other on the outer ring — every real slice survives", () => {
    const cols3 = [
      { name: "region", type: "varchar" }, { name: "product", type: "varchar" },
      { name: "channel", type: "varchar" }, { name: "m", type: "double" },
    ];
    const regions = ["R1", "R2", "R3"];
    const products = ["P1", "P2", "P3", "P4"];
    // Every (region, product) parent totals 1000, split 500/310/190 across 3 channels — each
    // channel is 19-50% of ITS OWN parent (nowhere near sub-threshold there), but 500/12000=4.2%,
    // 310/12000=2.58%, and 190/12000=1.58% of the ring's grand total: under the old ring-wide
    // rule, every "190" channel slice (12 of them, a third of the ring) would have been below the
    // 2.5% threshold and folded into one chart-wide Other belonging to no parent.
    const rows3: unknown[][] = [];
    for (const region of regions) {
      for (const product of products) {
        rows3.push([region, product, "C1", 500]);
        rows3.push([region, product, "C2", 310]);
        rows3.push([region, product, "C3", 190]);
      }
    }
    const opt = buildPieOption(cols3, rows3, {
      dimensions: ["region", "product", "channel"], metric: "m", sliceThreshold: 2.5,
    }) as any;
    expect(treeDepth(opt.series[0].data)).toBe(3);
    const outerRing = ring(opt, 2);
    // No ring-wide "Other" wedge...
    expect(outerRing.some((d: any) => d.name === "Other")).toBe(false);
    // ...and every one of the 36 real slices survived.
    expect(outerRing).toHaveLength(36);
    const values = outerRing.map((d: any) => d.value).sort((a: number, b: number) => a - b);
    expect(values.filter((v: number) => v === 500)).toHaveLength(12);
    expect(values.filter((v: number) => v === 310)).toHaveLength(12);
    expect(values.filter((v: number) => v === 190)).toHaveLength(12);
  });
});

// Task 5 — ring colour inheritance: an outer ring's slice takes its parent slice's own
// colour, lightened one OKLab step per ring, instead of an unrelated palette colour (Metabase
// getRingColorAlias).
//
// Ring 0's own colours are now PINNED explicitly rather than left to ECharts' palette. They used
// to be left implicit (no `itemStyle` at all on an un-overridden slice), which a `pie` series
// makes safe — it colours by datum INDEX. A `sunburst` colours a top-level node by NAME and skips
// the palette entirely for a node that already has a colour, so one `sliceColors` override on
// ring 0 would have consumed no slot and slid every later slice's colour up by one, while the
// React legend and the label-contrast pass (both computing `seriesColor(i)` from the ring index)
// went on naming the old ones.
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
    const inner = ring(opt, 0); // [US(40), EU(10)] — the palette, pinned onto each slice
    expect(inner.map((d: any) => d.itemStyle.color)).toEqual([seriesColor(0), seriesColor(1)]);

    const outer = ring(opt, 1);
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
    const inner = ring(opt, 0);
    expect(inner.find((d: any) => d.name === "US").itemStyle.color).toBe("#123456");

    const outer = ring(opt, 1);
    const widgetsUnderUS = outer.find((d: any) => d.name === "Widgets" && d.value === 30);
    // Must lighten from the OVERRIDE ("#123456"), not from seriesColor(0) — the palette
    // colour US would otherwise have gotten.
    expect(widgetsUnderUS.itemStyle.color).toBe(lightenOklch("#123456", 1));
    expect(widgetsUnderUS.itemStyle.color).not.toBe(lightenOklch(seriesColor(0), 1));
  });

  // Chart settings are JSONB and never whitelisted server-side, so a slice colour can arrive as
  // anything at all. resolveColor is this codebase's one validator for a colour from a less
  // trusted source; an override that skipped it could paint a slice with a value the renderer
  // can't draw, which getTextColorForBackground would then silently mis-contrast a label against.
  it("a slice colour override the renderer can't draw is rejected, leaving the palette slot", () => {
    const opt = buildPieOption(cols2, rows2, {
      dimensions: ["region"], metric: "rev", sliceColors: { US: "banana", EU: "rgb(1,2,3)" },
    }) as any;
    const inner = ring(opt, 0);
    // Not a colour: no override at all, so the slice keeps the palette slot for its own index.
    expect(inner.find((d: any) => d.name === "US").itemStyle.color).toBe(seriesColor(0));
    // A valid non-hex colour still comes through verbatim.
    expect(inner.find((d: any) => d.name === "EU").itemStyle.color).toBe("rgb(1,2,3)");
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
    const inner = ring(opt, 0);
    const other = inner.find((d: any) => d.name === "Other");
    expect(other).toBeTruthy();
    expect(other.value).toBe(40);
    const otherColor = other.itemStyle.color;

    const outer = ring(opt, 1);
    const x = outer.find((d: any) => d.name === "X");
    const y1 = outer.find((d: any) => d.name === "Y1");
    expect(x).toBeTruthy();
    expect(y1).toBeTruthy();
    expect(x.itemStyle.color).toBe(lightenOklch(otherColor, 1));
    expect(y1.itemStyle.color).toBe(lightenOklch(otherColor, 1));
  });
});

// Task 6 — slice curation: rename / hide / reorder the FIRST dimension's slices only
// (Metabase's pie.rows). All three key on the slice's RAW dimension value, never its display
// text, so renaming never breaks a colour override, a hide, or a manual position.
describe("buildPieOption — slice curation (task 6)", () => {
  const cols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];

  it("a renamed slice shows its new name in the chart data", () => {
    const rows = [["EU", 10], ["US", 30]];
    const opt = buildPieOption(cols, rows, {
      dimensions: ["d"], metric: "m", sliceNames: { US: "United States" },
    }) as any;
    expect(ring(opt, 0).map((d: any) => d.name)).toEqual(["United States", "EU"]);
  });

  it("a hidden slice is absent and the remaining percentages re-base", () => {
    const rows = [["A", 10], ["B", 30], ["C", 60]];
    const opt = buildPieOption(cols, rows, {
      dimensions: ["d"], metric: "m", hiddenSlices: ["B"],
    }) as any;
    const data = ring(opt, 0);
    expect(data.map((d: any) => d.name)).toEqual(["C", "A"]);
    const total = data.reduce((t: number, d: any) => t + d.value, 0);
    // Must be 70 (A + C), NOT 100 — B's value must not still count toward the total.
    expect(total).toBe(70);
    const c = data.find((d: any) => d.name === "C");
    expect((c.value / total) * 100).toBeCloseTo((60 / 70) * 100, 10);
  });

  it("hiding a parent removes its children too, and the outer ring re-bases as well", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows2 = [
      ["R1", "A", 10], ["R1", "B", 5],
      ["R2", "C", 20], ["R2", "D", 5],
    ];
    const opt = buildPieOption(cols2, rows2, {
      dimensions: ["region", "cat"], metric: "m", hiddenSlices: ["R1"],
    }) as any;
    const inner = ring(opt, 0);
    expect(inner.map((d: any) => d.name)).toEqual(["R2"]);
    const outer = ring(opt, 1);
    expect(outer.map((d: any) => d.name)).toEqual(["C", "D"]);
    const outerTotal = outer.reduce((t: number, d: any) => t + d.value, 0);
    // Must be 25 (C + D), NOT 40 — R1's children (A, B) must not survive their hidden parent.
    expect(outerTotal).toBe(25);
  });

  it("sliceOrder reorders; unlisted slices keep their natural order after the named ones", () => {
    const rows = [["A", 5], ["B", 30], ["C", 10]]; // natural (value-desc) order: B, C, A
    const opt = buildPieOption(cols, rows, {
      dimensions: ["d"], metric: "m", sliceOrder: ["A"],
    }) as any;
    expect(ring(opt, 0).map((d: any) => d.name)).toEqual(["A", "B", "C"]);
  });

  it("decimals changes what a percentage label shows", () => {
    // showLabels defaults on now, so it's turned off explicitly to isolate the percent text
    // (otherwise the name would be prefixed too, which isn't what this test is about).
    // Read from the per-datum formatter — the one that actually renders; A is 10 of 30, i.e.
    // 33.333…%, so the two decimal settings genuinely differ.
    const rows = [["A", 10], ["B", 20]];
    const labelOfA = (o: any) => ring(o, 0).find((d: any) => d.name === "A").label.formatter();
    const withDefault = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", percentVisibility: "inside", showLabels: false }) as any;
    const withZero = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", percentVisibility: "inside", decimals: 0, showLabels: false }) as any;
    expect(labelOfA(withDefault)).toBe("33.33%");
    expect(labelOfA(withZero)).toBe("33%");
  });

  it("renaming does not change identity: colour, hidden and order still key on the raw value", () => {
    const rows = [["A", 10], ["B", 30], ["C", 5]];
    const opt = buildPieOption(cols, rows, {
      dimensions: ["d"], metric: "m",
      sliceNames: { A: "Alpha" },
      sliceColors: { A: "#111111" },
      hiddenSlices: ["C"],
      sliceOrder: ["A", "B"],
    }) as any;
    const data = ring(opt, 0);
    // C is hidden (by its raw value, even though never renamed).
    expect(data.map((d: any) => d.name)).not.toContain("C");
    // A is renamed for display...
    const alpha = data.find((d: any) => d.value === 10);
    expect(alpha.name).toBe("Alpha");
    // ...but its sliceColors entry (keyed on raw "A") still applies...
    expect(alpha.itemStyle.color).toBe("#111111");
    // ...and sliceOrder (keyed on raw "A") still puts it first, ahead of B (value 30), even
    // though natural value-desc order would put B first.
    expect(data.map((d: any) => d.value)).toEqual([10, 30]);
  });

  it("curation applies to ring 0 only — a value that also appears in ring 1 is unaffected there", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "m", type: "double" },
    ];
    // "US" is a ring-0 (region) value we hide, but it ALSO appears as a ring-1 (category) value
    // nested under a different, non-hidden region (EU) — that ring-1 "US" must survive untouched.
    const rows2 = [
      ["EU", "US", 10],
      ["US", "Widgets", 20],
    ];
    const opt = buildPieOption(cols2, rows2, {
      dimensions: ["region", "category"], metric: "m", hiddenSlices: ["US"],
    }) as any;
    const inner = ring(opt, 0);
    expect(inner.map((d: any) => d.name)).toEqual(["EU"]);
    const outer = ring(opt, 1);
    expect(outer.map((d: any) => d.name)).toEqual(["US"]);
    expect(outer.find((d: any) => d.name === "US").value).toBe(10);
  });
});

// Ring geometry, gaps, labels and legend — closing the four gaps against Metabase's own
// constants.ts (frontend/src/metabase/visualizations/echarts/pie/constants.ts):
//   slice.innerRadiusRatio 3/5, twoRingInnerRadiusRatio 2/5, threeRingInnerRadiusRatio 1/4;
//   slice.borderProportion 360 (a 1deg gap); twoRingBorderWidth 1, threeRingBorderWidth 0.3;
//   slice.multiRingFontSize 12, slice.label.fontWeight 700; total.valueFontSize 22,
//   total.valueFontSizeSm 17, total.labelFontSize 14, total.fontWeight 700.
describe("buildPieOption — Metabase ring geometry, gaps and labels", () => {
  const cols2 = [
    { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "m", type: "double" },
  ];
  const rows2 = [
    ["US", "Widgets", 30], ["US", "Gadgets", 10],
    ["EU", "Widgets", 5], ["EU", "Gadgets", 5],
  ];
  const cols3 = [
    { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
    { name: "brand", type: "varchar" }, { name: "m", type: "double" },
  ];
  const rows3 = [
    ["US", "Widgets", "Acme", 10], ["US", "Widgets", "Zenith", 5], ["EU", "Gadgets", "Acme", 3],
  ];

  describe("fix 1 — the hole shrinks as rings are added", () => {
    it("one ring keeps today's exact hole behaviour: donut off = 0%, donut on = 45%", () => {
      const off = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
      expect(off.series[0].radius[0]).toBe("0%");
      const on = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", donut: true }) as any;
      expect(on.series[0].radius[0]).toBe("45%");
    });

    it("two rings get Metabase's 2/5 hole, unconditionally — even with donut off", () => {
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m" }) as any;
      expect(opt.series[0].radius[0]).toBe("28%"); // 70 * 2/5
    });

    it("three rings get Metabase's 1/4 hole, unconditionally — even with donut off", () => {
      const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
      expect(opt.series[0].radius[0]).toBe("17.5%"); // 70 * 1/4
    });

    it("the hole ratio is the same whether donut is explicitly on for a multi-ring pie", () => {
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m", donut: true }) as any;
      expect(opt.series[0].radius[0]).toBe("28%");
    });
  });

  // Slices are separated by a shared BORDER STROKE, never by an angular gap.
  //
  // This used to set ECharts' `padAngle: 1`, on a reading of Metabase's
  // `borderProportion: 360 // 1 degree` as an angular pad. It isn't one — `getBorderWidth` returns
  // `(π × innerSideLength) / 360`, a stroke WIDTH (the arc length one degree subtends at the pie's
  // own radius). Measured against a real Metabase on the same 3-ring data: Metabase's outer-ring
  // slices touch (0 gaps), egret's had 36, median 3.96px — because a constant ANGLE opens a gap
  // that grows with radius, and because a pad removes real area from each slice where a shared
  // stroke does not.
  describe("slice separation — a shared border stroke, and no angular gap anywhere", () => {
    it("sets no padAngle at any ring count", () => {
      for (const opt of [
        buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any,
        buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m" }) as any,
        buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any,
      ]) {
        expect(opt.series[0].padAngle).toBeUndefined();
        expect(JSON.stringify(opt)).not.toContain("padAngle");
      }
    });

    it("two AND three rings both use a flat borderWidth of 1", () => {
      // Metabase's own `twoRingBorderWidth: 1` / `threeRingBorderWidth: 0.3` constants are DEAD in
      // its source — `getBorderWidth` reads neither, and returns a flat 1 for every ring count
      // above one. The 0.3 this used to draw at three rings came from the dead constant.
      for (const [c, r, dimensions] of [
        [cols2, rows2, ["region", "category"]],
        [cols3, rows3, ["region", "category", "brand"]],
      ] as const) {
        const opt = buildPieOption(c as any, r as any, { dimensions: [...dimensions], metric: "m" }) as any;
        for (let level = 0; level < dimensions.length; level++) {
          for (const d of ring(opt, level)) expect(d.itemStyle.borderWidth).toBe(1);
        }
      }
    });

    it("a single ring derives its border width from the pie's own diameter", () => {
      // Metabase's `(π × innerSideLength) / 360` — the arc length one degree subtends. A 400px host
      // draws a 0.7 * 400 = 280px pie, so the stroke is π * 280 / 360.
      const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }, undefined, {
        width: 400, height: 400,
      }) as any;
      for (const d of ring(opt, 0)) expect(d.itemStyle.borderWidth).toBeCloseTo((Math.PI * 280) / 360, 9);
    });

    it("colours the border like the chart's own background, never leaking a raw var()", () => {
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m" }) as any;
      // jsdom can't resolve var(--card); resolveColor falls back to a concrete colour rather
      // than ever leaking the literal "var(...)" to the SVG renderer.
      expect(opt.series[0].itemStyle.borderColor).toBeTruthy();
      expect(opt.series[0].itemStyle.borderColor).not.toContain("var(");
    });
  });

  describe("fix 3 — slice labels, sized and weighted like Metabase, degrading gracefully", () => {
    it("a single ring uses a font size in Metabase's 14-20 range, weight 700, drawn on the slice", () => {
      const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", showLabels: true }) as any;
      const label = opt.series[0].label;
      expect(label.fontSize).toBeGreaterThanOrEqual(14);
      expect(label.fontSize).toBeLessThanOrEqual(20);
      expect(label.fontWeight).toBe(700);
      expect(label.position).toBe("inside");
    });

    it("multi-ring pies use Metabase's multiRingFontSize (12), weight 700, on every ring", () => {
      const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m", showLabels: true }) as any;
      const label = opt.series[0].label;
      expect(label.fontSize).toBe(12);
      expect(label.fontWeight).toBe(700);
      expect(label.position).toBe("inside");
      // Every label was already measured against its own slice; ECharts must not truncate or wrap
      // it a second time against the box it computes for a rotated label.
      expect(label.overflow).toBe("none");
    });

    it("degrades small/overlapping slices by hiding them (ECharts labelLayout.hideOverlap), not overlapping into mush", () => {
      const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m", showLabels: true }) as any;
      expect(opt.series[0].labelLayout).toEqual({ hideOverlap: true });
    });
  });

  // Fix 1a/1b/2 — the real anti-overlap mechanism (labelLayout.hideOverlap above is only a
  // backstop): multi-ring labels run radial, each slice's label is measured against its own
  // slice's geometry and truncated-or-dropped, and each slice's label colour contrasts with its
  // own fill rather than one fixed colour for the whole chart. A stub measurer (`s.length * 7`)
  // is injected as buildPieOption's 4th argument so expected numbers are obvious, per the
  // ground rules — real text measurement is exercised for its own sake in sliceLabel.test.ts.
  describe("fix 1a/1b/2 — slice labels are radial, fit their own slice, and contrast their own fill", () => {
    const stubMeasure = (s: string) => s.length * 7;

    // Rotation is per DATUM now, not per series — one series covers every ring, and a sunburst's
    // own `label.rotate` default is "radial", so a single-ring pie has to say 0 on each slice or
    // it would silently start drawing its labels along the radius.
    it("a 3-ring pie's labels use radial rotation; a 1-ring pie's do not", () => {
      const one = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
      for (const d of ring(one, 0)) expect(d.label.rotate).toBe(0);

      const three = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
      for (const level of [0, 1, 2]) {
        for (const d of ring(three, level)) expect(d.label.rotate).toBe("radial");
      }
    });

    const dCols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
    // A stand-in "real container" size — buildPieOption has no DOM access, so every geometry-
    // sensitive test below passes this explicitly instead of relying on any built-in guess (see
    // the regression-guard and 530px-sunburst tests further down for why that distinction
    // matters). 300x300 here specifically reproduces the exact pixel math this file's tests
    // already relied on before containerSize existed as an explicit input.
    const containerSize = { width: 300, height: 300 };

    // The POINT of fix 1a, not just its cosmetic half. `label.rotate: "radial"` (asserted above)
    // only tells ECharts how to DRAW the text; what actually makes a narrow multi-ring slice able
    // to hold a label is that its available space is MEASURED radially — bounded by the ring's own
    // thickness rather than by the inner chord the text would have to span horizontally. So this
    // picks a slice where the two branches disagree outright: the arc is narrow enough that a
    // horizontal label is rejected (and truncation can't rescue it either), while the ring is
    // thick enough that a radial one fits in full.
    it("measures a multi-ring label radially, not just rotates it: a slice a horizontal label could not fit keeps its label in full", () => {
      const rCols = [
        { name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "m", type: "double" },
      ];
      // sortRows: false keeps "EMEA" first so its arc starts at 12 o'clock, well clear of
      // isNearXAxis's ±π/8 band around 3 and 9 o'clock (where the horizontal branch is ALSO
      // allowed the donut thickness, and the two branches would agree). sliceThreshold: 0 keeps
      // the 20/360 slice out of the "Other" fold. Two rings -> holeStart 28%, ring 0 spans
      // 28-49% of a 530px container's radius: inner 74.2px, outer 129.85px.
      const rRows = [["EMEA", "x", 20], ["Rest", "y", 340]];
      const opt = buildPieOption(
        rCols,
        rRows,
        { dimensions: ["region", "cat"], metric: "m", showLabels: true, sortRows: false, sliceThreshold: 0 },
        stubMeasure,
        { width: 530, height: 530 },
      ) as any;

      // The premise, stated against the geometry itself: for THIS slice's ring and arc, a
      // horizontal label has less room than "EMEA" needs (4 * 7 = 28px) and a radial one has more.
      const ringInner = 28 * 2.65;
      const ringOuter = 49 * 2.65;
      const arc = (20 / 360) * 2 * Math.PI; // "EMEA"'s nominal share of the ring
      const emeaWidth = stubMeasure("EMEA");
      expect(calcAvailableDonutSliceLabelLength(ringInner, ringOuter, 0, arc, 12, "horizontal"))
        .toBeLessThan(emeaWidth);
      expect(calcAvailableDonutSliceLabelLength(ringInner, ringOuter, 0, arc, 12, "radial"))
        .toBeGreaterThan(emeaWidth);

      // ...and the chart keeps the label, which is only possible via the radial branch: measured
      // horizontally there isn't even room for a 3-character truncation, so it would be dropped.
      const emea = ring(opt, 0).find((d: any) => d.name === "EMEA");
      expect(emea.label.formatter()).toBe("EMEA");
    });

    // The angular scale a label is measured against is the one ECharts' sunburstLayout uses:
    // `unitRadian = 2π / rootSum` for EVERY node, so an outer slice's arc is its share of the
    // WHOLE chart, nested inside its parent's. Measuring it as a share of its own PARENT instead
    // (which is what N independent pie series did — each ring angled by its own total) hands the
    // fit pass arcs that can be wildly too wide: a lone child fills 100% of its parent but may be a
    // sliver of the chart. Nothing pinned this, so it is pinned on exactly that slice.
    it("measures an outer slice's arc as its share of the whole chart, not of its parent", () => {
      const aCols = [
        { name: "d", type: "varchar" }, { name: "c", type: "varchar" }, { name: "m", type: "double" },
      ];
      // "Only" is 100% of its parent "Small", and 0.1% of the chart. Two rings in a 400px host:
      // ring 1 spans 98px-140px, so a radial label needs an inner chord wider than the 12px font.
      // Its real 0.36-degree arc gives 0.6px; its parent-relative "arc" would give a full circle.
      const opt = buildPieOption(
        aCols,
        [["Big", "B1", 999], ["Small", "Only", 1]],
        { dimensions: ["d", "c"], metric: "m", showLabels: true },
        stubMeasure,
        { width: 400, height: 400 },
      ) as any;
      const outer = ring(opt, 1);
      expect(outer.find((d: any) => d.name === "Only").label.formatter()).toBe(" ");
      // ...and the sibling that really is most of the chart keeps its label, so this isn't just
      // "everything on the outer ring is dropped".
      expect(outer.find((d: any) => d.name === "B1").label.formatter()).toBe("B1");
    });

    it("a slice too small for its label gets NO label, rather than a bare ellipsis", () => {
      // donut: true so this ring actually has a hole — calcAvailableDonutSliceLabelLength is a
      // DONUT formula (see the guard in buildPieOption); a plain, no-hole single ring skips the
      // fit check entirely (a separate, deliberate exception — see "showLabels: false" test
      // below for that case instead). A ~0.1%-of-the-ring sliver's available space is far too
      // small for even a truncated "Tiny" — sliceThreshold: 0 keeps it out of the "Other" fold
      // so it stays its own slice.
      const rows = [["Tiny", 1], ["Big", 999]];
      const opt = buildPieOption(
        dCols,
        rows,
        { dimensions: ["d"], metric: "m", showLabels: true, sliceThreshold: 0, donut: true },
        stubMeasure,
        containerSize,
      ) as any;
      const tiny = ring(opt, 0).find((d: any) => d.name === "Tiny");
      expect(tiny.label.formatter()).toBe(" "); // the "nothing to show" sentinel
    });

    it("a label that almost fits is truncated with an ellipsis rather than dropped", () => {
      // Two donut slices sharing the ring roughly evenly, so each gets a wide (not sliver) arc
      // with real, nonzero available space — long enough that a moderately long name doesn't
      // fit whole, short enough that a truncated prefix + ellipsis does.
      const longName = "A very long region name that will not fit";
      const opt = buildPieOption(
        dCols,
        [[longName, 55], ["Other region", 45]],
        { dimensions: ["d"], metric: "m", showLabels: true, donut: true },
        stubMeasure,
        containerSize,
      ) as any;
      const label = ring(opt, 0).find((d: any) => d.name === longName).label.formatter();
      expect(label).not.toBeNull();
      expect(label).not.toBe(" ");
      expect(label?.endsWith("…")).toBe(true);
      expect(label?.length).toBeLessThan(longName.length);
    });

    it("showLabels: false still suppresses everything, per-slice fitting included", () => {
      const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", showLabels: false }, stubMeasure) as any;
      expect(opt.series[0].label.show).toBe(false);
    });

    // Regression guard for the sizing bug: buildPieOption used to convert its abstract 0-100%
    // radii to pixels against a GUESSED, fixed 300px reference container (
    // NOMINAL_CONTAINER_MIN_DIMENSION_PX) instead of the chart's real rendered size — so on any
    // container meaningfully bigger than 300px (a real ~530px card, say), every label was
    // measured against far less room than it actually had, truncating text that had plenty of
    // space to render in full. Same data, same slice, same everything else — container size is
    // the ONLY variable — so a real fix must show a full label at a large size where the exact
    // same slice truncates at a small one.
    //
    // The "large" size is 1000px rather than the 3000px this used to use, and the name is shorter:
    // the diameter is capped at 550px now (Metabase's DIMENSIONS.maxSideLength), so past ~786px of
    // host every size draws the same circle and a 41-character label can never fit a single ring's
    // horizontal band whatever the host does. That IS the cap working, and the test says the same
    // thing about a label the capped geometry can actually hold.
    it("the SAME slice's label survives in full at a large container size and truncates at a small one — size is the only variable", () => {
      const longName = "Long region";
      const cfg = { dimensions: ["d"], metric: "m", showLabels: true, donut: true };
      const rows2 = [[longName, 55], ["Other region", 45]];

      const small = buildPieOption(dCols, rows2, cfg, stubMeasure, { width: 300, height: 300 }) as any;
      const smallLabel = ring(small, 0).find((d: any) => d.name === longName).label.formatter();
      expect(smallLabel).not.toBeNull();
      expect(smallLabel).not.toBe(longName);
      expect(smallLabel?.endsWith("…")).toBe(true);

      const large = buildPieOption(dCols, rows2, cfg, stubMeasure, { width: 1000, height: 1000 }) as any;
      const largeLabel = ring(large, 0).find((d: any) => d.name === longName).label.formatter();
      expect(largeLabel).toBe(longName);
    });

    // Metabase's pie is 550px in a 735px host — an ABSOLUTE cap (`DIMENSIONS.maxSideLength`), not
    // a ratio, alongside a 12px-a-side padding floor (`DIMENSIONS.padding.side`). Without it a
    // full-page card drew a circle as big as the card, which stops adding information past a point
    // and just pushes the labels apart.
    it("caps the diameter at 550px on a big host, and leaves a smaller host alone", () => {
      const outer = (size: { width: number; height: number }) =>
        (buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }, undefined, size) as any)
          .series[0].radius[1];

      // 1200px host: 0.7 * 1200 = 840px of diameter uncapped -> capped to 550, i.e. r = 275.
      expect(outer({ width: 1200, height: 1200 })).toBe(275);
      // ...and a host wider than it is tall is capped off its SHORTER side, like every other
      // radius in this file.
      expect(outer({ width: 4000, height: 1000 })).toBe(275);

      // 600px host: 0.7 * 600 = 420 < 550, so nothing is capped and the fraction still rules.
      expect(outer({ width: 600, height: 600 })).toBe(210);

      // The padding floor only binds on a host small enough that 12px a side is more than the
      // fraction already leaves: below 80px, here.
      expect(outer({ width: 60, height: 60 })).toBe((60 - 24) / 2);
    });

    // Realistic reproduction of the reported live bug: a 3-ring sunburst (region -> segment ->
    // channel) rendered in a ~530px card (the real measured size from EChartsRenderer's
    // ResizeObserver, not a guess) — every ring-0 (region) label, including "LATAM", must
    // survive IN FULL. Before the fix, the same chart measured against a fixed 300px reference
    // and truncated "LATAM" down to "L…".
    it("a realistic 3-ring sunburst at ~530px keeps ring 0's region labels in full, LATAM included", () => {
      const cols3 = [
        { name: "region", type: "varchar" }, { name: "segment", type: "varchar" },
        { name: "channel", type: "varchar" }, { name: "revenue", type: "double" },
      ];
      const regions = ["EMEA", "LATAM", "APAC", "NA"];
      const segments = ["Enterprise", "SMB", "Consumer"];
      const channels = ["Direct", "Partner", "Online"];
      const rows3: unknown[][] = [];
      let v = 1000;
      for (const r of regions) {
        for (const s of segments) {
          for (const c of channels) {
            rows3.push([r, s, c, v]);
            v += 37;
          }
        }
      }
      const opt = buildPieOption(
        cols3,
        rows3,
        { dimensions: ["region", "segment", "channel"], metric: "revenue", donut: true, showLabels: true },
        undefined, // real (canvas-fallback-to-average-glyph-width) measurer — the real app path
        { width: 530, height: 530 },
      ) as any;
      const ring0 = ring(opt, 0);
      for (const region of regions) {
        const label = ring0.find((d: any) => d.name === region).label.formatter();
        expect(label).toBe(region);
      }
    });

    // Ground rule 3 — no surviving label is ever whitespace-only or a bare ellipsis: every label
    // is either its full text, a real multi-character truncation, or the "nothing to show"
    // sentinel — never "", never "…" alone, never a truncation with no real characters.
    //
    // This test used to run against a big, cramped, many-slice 3-ring sunburst and sweep every
    // label, on the assumption that SOME slice in there would land in each dangerous band. None
    // did: deleting the MIN_TRUNCATED_LABEL_CHARS floor outright left the whole suite green,
    // because no slice in that fixture ever had `0 < availableSpace <= ellipsisWidth` (the only
    // band that yields a bare ellipsis) and no fixture row had a blank dimension value at all. A
    // sweep only guards what its fixture actually reaches, so this now names the two cases and
    // engineers a slice into each, with the deterministic 7px-per-character measurer so the
    // bands are exact rather than hoped for.
    //
    // Geometry: a single-ring donut (radii 45%-70%) in a 300px container -> inner 67.5px, outer
    // 105px, fontSize 14. Labels there are HORIZONTAL, so a slice's room is its inner chord —
    // which, unlike the radial branch's donut thickness (always > 2 * fontSize or 0), can land
    // anywhere, sub-ellipsis-width included. "Tiny" is 19/1000 of the ring: a ~6.8-degree arc,
    // ~8px of chord, ~4px after the label padding — under the 7px ellipsis, so truncation can
    // only return a bare "…", which the floor must turn into no label at all.
    it("drops a label with nothing left to say: a blank dimension value, and a slice too narrow for even an ellipsis", () => {
      const dc = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
      const opt = buildPieOption(
        dc,
        [["", 500], ["Alpha", 481], ["Tiny", 19]],
        { dimensions: ["d"], metric: "m", donut: true, showLabels: true, sliceThreshold: 0 },
        stubMeasure,
        { width: 300, height: 300 },
      ) as any;
      const labelOf = (name: string) => ring(opt, 0).find((d: any) => d.name === name).label.formatter();

      // A row with no dimension value has nothing to fit — an empty string would otherwise sail
      // through the "already fits" fast path (it fits ANY width) and render as a content-less "".
      expect(labelOf("")).toBe(" ");

      // A slice with real but sub-ellipsis room truncates to a bare "…", which identifies nothing
      // — the MIN_TRUNCATED_LABEL_CHARS floor drops it instead.
      expect(labelOf("Tiny")).toBe(" ");

      // ...and the floor is not just dropping everything that doesn't fit whole: a slice with room
      // for a real multi-character prefix still gets one.
      expect(labelOf("Alpha")).toBe("Alp…");

      for (const d of ring(opt, 0)) {
        const text = d.label.formatter();
        if (text === " ") continue; // the one legitimate "nothing to show" sentinel
        expect(text.trim()).not.toBe("");
        expect(text).not.toBe("…");
      }
    });

    // SLICE_LABEL_PADDING (Metabase's DIMENSIONS.slice.label.padding) subtracts a 2px margin from
    // each side of the space a label is allowed, so text never butts up against its slice's own
    // border. It IS load-bearing — it changes which labels survive whole — but nothing pinned
    // that: setting it to 0 changed no assertion, which would have let a future reader delete it
    // as decorative. This nails it to an outcome, with a label engineered to sit exactly in the
    // 4px the padding takes away.
    //
    // The radial branch makes the arithmetic exact: a multi-ring label's room is the ring's
    // thickness outright, no chord/rectangle minimum involved. Two rings in a 400px container ->
    // radii 28%-70% split in two, so ring 0 spans 28%-49% = 56px to 98px: exactly 42px thick, and
    // "ABCDEF" is exactly 42px under the 7px-per-character measurer.
    it("keeps the slice label clear of its own border: a label that fits the ring exactly is still truncated", () => {
      const pCols = [
        { name: "d", type: "varchar" }, { name: "c", type: "varchar" }, { name: "m", type: "double" },
      ];
      const opt = buildPieOption(
        pCols,
        [["ABCDEF", "x", 50], ["Zed", "y", 50]],
        { dimensions: ["d", "c"], metric: "m", showLabels: true },
        stubMeasure,
        { width: 400, height: 400 },
      ) as any;

      const ringThickness = (49 - 28) * (400 / 2 / 100);
      expect(stubMeasure("ABCDEF")).toBe(ringThickness); // fits the bare geometry to the pixel

      // ...and is still cut back, because the padding is subtracted before the fit is judged: 38px
      // of usable room leaves 31px once the ellipsis is paid for, i.e. four characters.
      const label = ring(opt, 0).find((d: any) => d.name === "ABCDEF").label.formatter();
      expect(label).toBe("ABCD…");

      // A shorter label in the same ring is untouched — the padding trims the margin, not the text.
      expect(ring(opt, 0).find((d: any) => d.name === "Zed").label.formatter()).toBe("Zed");
    });

    // The arc a label is measured against is the slice's FULL nominal share. There used to be a
    // `padAngle` compensation here — half a degree taken off each end, because `padAngle: 1` made
    // every drawn slice a degree narrower than its share. The pad is gone (slices are separated by
    // a shared border stroke, which removes no area at all), so the nominal arc IS the drawn arc
    // and the compensation would now under-measure every label.
    //
    // Pinned on a slice engineered to sit exactly across the boundary: a 1/34 share of a
    // (diameter-capped) 550px donut leaves just enough room for "EMEA", and one degree less does
    // not — not even enough for a 3-character truncation. So the old pad accounting would drop
    // this label and measuring the true arc keeps it.
    it("measures a label against its full nominal arc, with nothing taken off for a gap", () => {
      const dCols2 = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
      const opt = buildPieOption(
        dCols2,
        [["Rest", 33], ["EMEA", 1]], // 1/34 of the ring = 10.59 degrees
        { dimensions: ["d"], metric: "m", donut: true, showLabels: true, sliceThreshold: 0 },
        stubMeasure,
        { width: 1800, height: 1800 }, // capped to a 550px pie: r 275px, hole 45/70 of it
      ) as any;
      expect(opt.series[0].padAngle).toBeUndefined();

      // The premise, stated against the geometry rather than assumed. EMEA is the last slice, so
      // its arc ends at 12 o'clock.
      const outerPx = 275;
      const innerPx = outerPx * (45 / 70);
      const arc = (1 / 34) * 2 * Math.PI;
      const oneDegree = Math.PI / 180;
      const room = (start: number, end: number) =>
        calcAvailableDonutSliceLabelLength(innerPx, outerPx, start, end, 14, "horizontal") - 4;
      expect(room(2 * Math.PI - arc, 2 * Math.PI)).toBeGreaterThanOrEqual(stubMeasure("EMEA"));
      // A degree narrower — what the pad compensation used to hand the fit pass — is short of both
      // the whole label AND the 3-character floor a truncation would have to clear.
      expect(room(2 * Math.PI - arc + oneDegree / 2, 2 * Math.PI - oneDegree / 2))
        .toBeLessThan(stubMeasure("EMEA"));

      expect(ring(opt, 0).find((d: any) => d.name === "EMEA").label.formatter()).toBe("EMEA");
    });

    // The angle model above starts every ring at 12 o'clock and runs clockwise because that is
    // what ECharts does by default (`startAngle: 90, clockwise: true`) and this file never says
    // otherwise. Nothing enforced that: a future change setting either would leave the fit maths
    // silently measuring the wrong slices. Setting them isn't forbidden — it just has to come
    // with a matching change to the derivation, which this test makes impossible to skip.
    it("never overrides ECharts' default startAngle/clockwise, which the label-fit angles assume", () => {
      const charts = [
        buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any,
        buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m" }) as any,
        buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any,
      ];
      for (const chart of charts) {
        for (const s of chart.series) {
          expect(s.startAngle).toBeUndefined();
          expect(s.clockwise).toBeUndefined();
        }
      }
    });

    it("each slice's label colour contrasts with ITS OWN fill, not one fixed colour for the whole ring", () => {
      const opt = buildPieOption(cols, rows, {
        dimensions: ["region"], metric: "rev",
        sliceColors: { EU: "#111111", US: "#eeeeee" }, // one very dark fill, one very light fill
      }) as any;
      const eu = ring(opt, 0).find((d: any) => d.name === "EU");
      const us = ring(opt, 0).find((d: any) => d.name === "US");
      expect(eu.label.color).not.toBe(us.label.color);
      // ...and each is the READABLE ink for its own fill. Pinned to the concrete values because
      // "they differ" alone passed even when the label was being painted in the slice's own fill
      // colour — two different fills give two different labels either way. jsdom can't resolve
      // var(), so resolveColor hands back COLOR_FALLBACK.text / .textInverse.
      expect(eu.label.color).toBe("#ffffff"); // white on near-black
      expect(us.label.color).toBe("#111827"); // near-black on near-white
      expect(eu.label.color).not.toBe(eu.itemStyle.color);
      expect(us.label.color).not.toBe(us.itemStyle.color);
    });
  });

  describe("fix 4 — the legend lists only the first dimension's slices", () => {
    it("a two-ring pie's legend carries only ring 0's names, not ring 1's", () => {
      const names = buildPieLegend(cols2, rows2, {
        dimensions: ["region", "category"], metric: "m", showLegend: true,
      }).map((e) => e.name);
      expect(names).toEqual(["US", "EU"]);
      // "Widgets"/"Gadgets" are ring-1-only names here and must not appear in the legend list.
      expect(names).not.toContain("Widgets");
      expect(names).not.toContain("Gadgets");
    });

    it("a one-ring pie's legend is unaffected — it only ever had one ring's names anyway", () => {
      const names = buildPieLegend(cols, rows, { dimensions: ["region"], metric: "rev", showLegend: true })
        .map((e) => e.name);
      expect(names).toEqual(["US", "EU"]);
    });

    it("the legend is still absent when showLegend is explicitly off", () => {
      // showLegend now defaults on — this must set it to false explicitly to actually test
      // the off case (a bare config, with no showLegend at all, now shows a legend instead).
      expect(buildPieLegend(cols2, rows2, {
        dimensions: ["region", "category"], metric: "m", showLegend: false,
      })).toEqual([]);
    });

    // The swatch is the whole point of a key: it must be the colour the ring-0 slice is actually
    // drawn in, including a per-slice override and the muted "Other" tint — the legend is no
    // longer inside the canvas, so nothing else keeps the two in step.
    it("each entry carries its slice's own render colour", () => {
      const entries = buildPieLegend(cols, rows, { dimensions: ["region"], metric: "rev" });
      expect(entries.map((e) => e.color)).toEqual([seriesColor(0), seriesColor(1)]);
      const overridden = buildPieLegend(cols, rows, {
        dimensions: ["region"], metric: "rev", sliceColors: { US: "#123456" },
      });
      expect(overridden.find((e) => e.name === "US")!.color).toBe("#123456");
    });
  });

  describe("centre total keeps working under the new hole ratios (Metabase's total.* constants)", () => {
    it("uses Metabase's labelFontSize (14) and bolds the caption too, one ring", () => {
      const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", donut: true, showTotal: true }) as any;
      const [value, label] = opt.graphic;
      expect(value.style.fontSize).toBe(22);
      expect(value.style.fontWeight).toBe(700);
      expect(label.style.fontSize).toBe(14);
      expect(label.style.fontWeight).toBe(700); // previously unset — Metabase bolds the caption too
    });

    it("shrinks the value to Metabase's valueFontSizeSm (17) under the tightest (three-ring) hole", () => {
      const opt = buildPieOption(cols3, rows3, {
        dimensions: ["region", "category", "brand"], metric: "m", donut: true, showTotal: true,
      }) as any;
      const [value] = opt.graphic;
      expect(value.style.fontSize).toBe(17);
    });

    it("still renders at the regular size under the two-ring hole", () => {
      const opt = buildPieOption(cols2, rows2, {
        dimensions: ["region", "category"], metric: "m", donut: true, showTotal: true,
      }) as any;
      const [value] = opt.graphic;
      expect(value.style.fontSize).toBe(22);
    });
  });

  // Measured against a real Metabase (question 7, "Product breakdown", a genuine three-level
  // sunburst) at 1440x920: Metabase renders `2,852` over `Total` in the hole; egret rendered
  // nothing there, because `showTotal` defaulted off. An empty hole in the middle of three
  // rings is the single thing that made the chart read as unfinished — a one-ring donut's
  // hole is small and reads as deliberate, a sunburst's is a large void asking to be filled.
  //
  // Resolved at RENDER time, never written into stored settings (this program's standing
  // rule), and `??` not `||`, so an explicit `false` still turns it off.
  describe("a multi-ring pie fills its hole with the total by default", () => {
    it("two rings show the total with nothing stored at all", () => {
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m" }) as any;
      expect(opt.graphic).toBeTruthy();
      expect(opt.graphic.map((g: any) => g.style?.text)).toEqual(["50", "Total"]);
    });

    it("three rings show it too", () => {
      const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
      expect(opt.graphic).toBeTruthy();
      expect(opt.graphic.map((g: any) => g.style?.text)).toEqual(["18", "Total"]);
    });

    it("an explicit false still empties the hole", () => {
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m", showTotal: false }) as any;
      expect(opt.graphic).toBeFalsy();
    });

    it("multi-ring does not need donut set — the hole is there either way", () => {
      const off = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m", donut: false }) as any;
      expect(off.graphic).toBeTruthy();
    });

    // The single-ring side is deliberately untouched: a plain pie has no hole to write in,
    // and a one-ring donut's hole stays empty unless it is asked for.
    it("a single-ring donut still defaults to an empty hole", () => {
      const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", donut: true }) as any;
      expect(opt.graphic).toBeFalsy();
    });

    it("a single-ring pie with no hole never draws a total, even asked to", () => {
      const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", showTotal: true }) as any;
      expect(opt.graphic).toBeFalsy();
    });
  });

  // Metabase's legend reads `Doohickey 23.88%` — name AND share per ring-0 slice — because its
  // sample sunburst is configured `"pie.percent_visibility": "legend"`. egret's config had no
  // such value at all, so its legend could only ever be names.
  describe("the legend can carry each ring-0 slice's percentage", () => {
    // rows2 totals 50 at ring 0: US 40 (80%), EU 10 (20%).
    const legendPercent = (cfg: any, name: string) =>
      buildPieLegend(cols2, rows2, cfg).find((e) => e.name === name)?.percent;

    it("percentVisibility legend puts name and share beside each ring-0 entry", () => {
      const entries = buildPieLegend(cols2, rows2, {
        dimensions: ["region", "category"], metric: "m", percentVisibility: "legend",
      });
      expect(entries.map((e) => e.name)).toEqual(["US", "EU"]);
      expect(entries.map((e) => e.percent)).toEqual(["80%", "20%"]);
    });

    it("legend does NOT also put the percent on the slice", () => {
      const opt = buildPieOption(cols2, rows2, {
        dimensions: ["region", "category"], metric: "m", percentVisibility: "legend",
      }) as any;
      expect(ring(opt, 0).find((d: any) => d.name === "US").label.formatter()).toBe("US");
    });

    // "both" finally means what it says. Before "legend" existed it could only draw the inside
    // half, so the value promised something the renderer had no way to deliver.
    it("both puts the percent in the legend AND on the slice", () => {
      const cfg = { dimensions: ["region", "category"], metric: "m", percentVisibility: "both" };
      expect(legendPercent(cfg, "US")).toBe("80%");
      const opt = buildPieOption(cols2, rows2, cfg as any) as any;
      expect(ring(opt, 0).find((d: any) => d.name === "US").label.formatter()).toBe("US: 80%");
    });

    it("inside leaves the legend as plain names", () => {
      const entries = buildPieLegend(cols2, rows2, {
        dimensions: ["region", "category"], metric: "m", percentVisibility: "inside",
      });
      expect(entries.map((e) => e.percent)).toEqual([undefined, undefined]);
    });

    it("the legend percent honours the configured decimals, like the slice labels", () => {
      const entries = buildPieLegend(cols3, rows3, {
        dimensions: ["region", "category", "brand"], metric: "m", percentVisibility: "legend", decimals: 3,
      });
      // US 15 of 18 = 83.333...%
      expect(entries.find((e) => e.name === "US")!.percent).toBe("83.333%");
    });

    it("a renamed ring-0 slice still gets its percentage", () => {
      const entries = buildPieLegend(cols2, rows2, {
        dimensions: ["region", "category"], metric: "m", percentVisibility: "legend",
        sliceNames: { US: "United States" },
      });
      expect(entries.map((e) => e.name)).toContain("United States");
      expect(entries.find((e) => e.name === "United States")!.percent).toBe("80%");
    });

    it("no legend at all means no entries to trip over", () => {
      expect(buildPieLegend(cols2, rows2, {
        dimensions: ["region", "category"], metric: "m", percentVisibility: "legend", showLegend: false,
      })).toEqual([]);
    });

    describe("the default", () => {
      it("a multi-ring pie shows legend percentages with nothing stored", () => {
        const cfg = { dimensions: ["region", "category"], metric: "m" };
        expect(legendPercent(cfg, "US")).toBe("80%");
        // …and keeps the slices themselves uncluttered, which is the point of choosing the
        // legend over the arcs on a sunburst.
        const opt = buildPieOption(cols2, rows2, cfg) as any;
        expect(ring(opt, 0).find((d: any) => d.name === "US").label.formatter()).toBe("US");
      });

      it("a single-ring pie is untouched: no percentages anywhere", () => {
        const cfg = { dimensions: ["region"], metric: "rev" };
        expect(buildPieLegend(cols, rows, cfg).map((e) => e.percent)).toEqual([undefined, undefined]);
        const opt = buildPieOption(cols, rows, cfg) as any;
        expect(ring(opt, 0).find((d: any) => d.name === "US").label.formatter()).toBe("US");
      });

      it("an explicit off still wins over the multi-ring default", () => {
        expect(legendPercent({
          dimensions: ["region", "category"], metric: "m", percentVisibility: "off",
        }, "US")).toBeUndefined();
      });

      // Settings come from a JSONB column that is never whitelisted server-side, so an
      // unrecognised value has to degrade rather than be trusted — drawing nothing is the safe
      // reading of "I don't know what this means".
      it("an unknown stored value degrades to off rather than throwing or guessing", () => {
        const cfg = {
          dimensions: ["region", "category"], metric: "m", percentVisibility: "everywhere" as never,
        };
        expect(legendPercent(cfg, "US")).toBeUndefined();
        const opt = buildPieOption(cols2, rows2, cfg) as any;
        expect(ring(opt, 0).find((d: any) => d.name === "US").label.formatter()).toBe("US");
      });
    });
  });
});

describe("resolvePercentVisibility", () => {
  it("passes every known value through, at any ring count", () => {
    for (const v of ["off", "legend", "inside", "both"] as const) {
      expect(resolvePercentVisibility(v, 1)).toBe(v);
      expect(resolvePercentVisibility(v, 3)).toBe(v);
    }
  });

  it("defaults to the legend once there is more than one ring, and off below that", () => {
    expect(resolvePercentVisibility(undefined, 0)).toBe("off");
    expect(resolvePercentVisibility(undefined, 1)).toBe("off");
    expect(resolvePercentVisibility(undefined, 2)).toBe("legend");
    expect(resolvePercentVisibility(undefined, 3)).toBe("legend");
  });

  it("degrades an unknown stored value to off, not to the default", () => {
    expect(resolvePercentVisibility("legend ", 3)).toBe("off");
    expect(resolvePercentVisibility("", 3)).toBe("off");
    expect(resolvePercentVisibility("true", 3)).toBe("off");
  });
});

// An outer ring's slice percentage is a share of its PARENT slice, not of that ring's own total
// (see the comment on `slicePercent` in pie.ts: this was a deliberate share-of-ring decision,
// reversed after a real three-ring chart showed it reads as noise). Ring 0 has no parent, so its
// share of the whole chart is what it reports.
//
// EVERY slice states its own `slicePercent`, ring 0 included — it is no longer an override on top
// of ECharts' number, because a `sunburst` series has none. Only `PieSeriesModel.getDataParams`
// adds `percent`, and only it honours `percentPrecision` (which this option therefore no longer
// sets). Both the label and the shared tooltip formatter (base.ts, see base.test.ts) read the
// datum's own number.
describe("buildPieOption — every slice states its own percentage, of its own parent", () => {
  it("a single-ring pie reports each slice's share of the whole chart", () => {
    const cols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
    const rows = [["A", 25], ["B", 75]];
    const opt = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m" }) as any;
    const a = ring(opt, 0).find((d: any) => d.name === "A");
    expect(a.slicePercent).toBeCloseTo(25, 9);
    // ...and the tooltip reads it off the tree. A sunburst hands the formatter no `percent` at
    // all, only the root-to-node path.
    expect(opt.tooltip.formatter({ treePathInfo: [{ name: "" }, { name: "A" }] })).toContain("25%");
  });

  it("never sets percentPrecision — a pie-series option a sunburst ignores outright", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", decimals: 3 }) as any;
    expect(opt.series[0].percentPrecision).toBeUndefined();
    // The rounding it used to delegate is done here instead, at the chart's own `decimals`.
    const two = buildPieOption(
      [{ name: "d", type: "varchar" }, { name: "m", type: "double" }],
      [["A", 1], ["B", 2]],
      { dimensions: ["d"], metric: "m", decimals: 3 },
    ) as any;
    expect(ring(two, 0).find((d: any) => d.name === "A").slicePercent).toBe(33.333);
  });

  it("two rings: an outer slice's percentage is its value over its PARENT's total, not the ring's", () => {
    const cols2 = [
      { name: "segment", type: "varchar" }, { name: "channel", type: "varchar" }, { name: "m", type: "double" },
    ];
    // 12 segments, each totalling 1000 (500 + 310 + 190) -> the channel ring's own total is
    // 12000. Worked example from the spec: a 500 channel slice is 500/12000 = 4.17% of the ring
    // but 500/1000 = 50% of its own segment.
    const segments = Array.from({ length: 12 }, (_, i) => `S${i + 1}`);
    const rows2: unknown[][] = [];
    for (const seg of segments) rows2.push([seg, "C1", 500], [seg, "C2", 310], [seg, "C3", 190]);
    const opt = buildPieOption(cols2, rows2, { dimensions: ["segment", "channel"], metric: "m" }) as any;
    const outer = ring(opt, 1);
    const c1Items = outer.filter((d: any) => d.name === "C1" && d.value === 500);
    expect(c1Items).toHaveLength(12);
    for (const item of c1Items) expect(item.slicePercent).toBeCloseTo(50, 9); // NOT 4.1(6)%
    // The tooltip formatter actually reaches 50% when fed the real path, not the ring-share number.
    const tooltip = opt.tooltip.formatter({ treePathInfo: [{ name: "" }, { name: "S1" }, { name: "C1" }] });
    expect(tooltip).toContain("50%");
    expect(tooltip).not.toContain("4.17%");
  });

  it("three rings: each level is a share of its OWN parent, not of ring 0 or the grand total", () => {
    const cols3 = [
      { name: "region", type: "varchar" }, { name: "product", type: "varchar" },
      { name: "channel", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows3 = [
      ["R1", "P1", "C1", 1200], ["R1", "P1", "C2", 300], // P1 = 1500
      ["R1", "P2", "C3", 500],                            // P2 = 500; R1 = 2000
      ["R2", "P3", "C4", 400], ["R2", "P3", "C5", 600],   // P3 = 1000; R2 = 1000
    ];
    const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "product", "channel"], metric: "m" }) as any;
    const middle = ring(opt, 1);
    const outer = ring(opt, 2);

    // Middle ring (product): share of its own region — NOT of the 3000 grand total.
    expect(middle.find((d: any) => d.name === "P1").slicePercent).toBeCloseTo((1500 / 2000) * 100, 9); // 75%, not 50%
    expect(middle.find((d: any) => d.name === "P3").slicePercent).toBeCloseTo(100, 9); // R2's only product

    // Outer ring (channel): share of its own PRODUCT — not of region (60%) or the grand total (40%).
    expect(outer.find((d: any) => d.name === "C1").slicePercent).toBeCloseTo((1200 / 1500) * 100, 9); // 80%
    expect(outer.find((d: any) => d.name === "C4").slicePercent).toBeCloseTo((400 / 1000) * 100, 9); // 40%
  });

  it("decimals still controls the displayed precision at every level", () => {
    const cols2 = [
      { name: "segment", type: "varchar" }, { name: "channel", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows2 = [["S1", "C1", 1], ["S1", "C2", 2]]; // C1 = 1/3 of its segment = 33.333...%
    const withDefault = buildPieOption(cols2, rows2, { dimensions: ["segment", "channel"], metric: "m" }) as any;
    const withZero = buildPieOption(cols2, rows2, { dimensions: ["segment", "channel"], metric: "m", decimals: 0 }) as any;
    expect(ring(withDefault, 1).find((d: any) => d.name === "C1").slicePercent).toBeCloseTo(33.33, 9);
    expect(ring(withZero, 1).find((d: any) => d.name === "C1").slicePercent).toBe(33);
  });

  it('a parent\'s own "Other" bucket reports its share of that same parent', () => {
    const cols2 = [
      { name: "grp", type: "varchar" }, { name: "item", type: "varchar" }, { name: "m", type: "double" },
    ];
    // P1 totals 1000: two big (800, 180) + two small (10, 10) -> P1's OWN Other = 20 = 2% of P1.
    const rows2 = [["P1", "A1", 800], ["P1", "A2", 180], ["P1", "b1", 10], ["P1", "b2", 10]];
    const opt = buildPieOption(cols2, rows2, { dimensions: ["grp", "item"], metric: "m" }) as any;
    const other = ring(opt, 1).find((d: any) => d.name === "Other");
    expect(other.value).toBe(20);
    expect(other.slicePercent).toBeCloseTo(2, 9); // 20 / 1000 (P1's own total)
  });
});

// Measured side by side against a real Metabase (docker, sample question 7 "Product breakdown",
// a genuine 3-level sunburst) at 1440x920: Metabase drew 69px-thick rings in a 735x735 host,
// egret 58px in an 830x661 one, and Metabase's legend was a LEFT COLUMN where egret's was a
// bottom row.
//
// The arithmetic behind the layout choice, stated once here because every test below is a
// consequence of it. A pie's outer radius is a percentage of `min(width, height) / 2` (ECharts'
// own convention), so its DIAMETER is a fraction of the host's shorter side:
//
//   bottom row : 0.70 * min(w, h)                   — what this file already drew, unchanged
//   side column: 0.85 * min(w - 190, h)             — the box a 190px legend column leaves
//
// and the layout that draws the bigger circle wins. On the measured 830x661 host that is the
// column (0.85 * min(640, 661) = 544px, against the row's 0.70 * 661 = 462.7px), which lifts the
// rings from 58px to 68px — the gap this was measured against. On a narrow, tall host the same
// arithmetic says the opposite (360x720: a row draws 252px, a column 144.5px), because taking
// 190px of width from a 360px host costs far more than a legend row costs in height.
describe("a pie's legend goes beside it when that leaves the rings more room", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["EU", 10], ["US", 30]];
  const base = { dimensions: ["region"], metric: "rev" };
  // The real measured host from the Metabase comparison, and a host shaped the opposite way.
  const wideShort = { width: 830, height: 661 };
  const narrowTall = { width: 360, height: 720 };
  /** The pie's outer radius in PIXELS — the unit the option uses once the real size is known. */
  const outerRadiusPx = (opt: any) => opt.series[0].radius[1] as number;

  it("a wide, short host puts the legend in a column beside the chart", () => {
    expect(pieLegendPlacement(wideShort)).toBe("side");
  });

  it("a narrow, tall host keeps the legend below", () => {
    expect(pieLegendPlacement(narrowTall)).toBe("bottom");
  });

  // The assertion that makes the threshold load-bearing rather than decorative: on each host the
  // layout chosen is the one that draws the BIGGER circle. Without this, the choice could be
  // inverted and every other test here would still pass.
  it("each host gets whichever layout draws the bigger pie", () => {
    const row = (s: { width: number; height: number }) => 0.7 * Math.min(s.width, s.height);
    const column = (s: { width: number; height: number }) =>
      0.85 * Math.min(s.width - PIE_LEGEND_COLUMN_WIDTH_PX, s.height);
    // 830x661: column 544px, row 462.7px -> column.
    expect(column(wideShort)).toBeCloseTo(544, 1);
    expect(column(wideShort)).toBeGreaterThan(row(wideShort));
    expect(pieLegendPlacement(wideShort)).toBe("side");
    // 360x720: row 252px, column 144.5px -> row.
    expect(row(narrowTall)).toBeCloseTo(252, 1);
    expect(row(narrowTall)).toBeGreaterThan(column(narrowTall));
    expect(pieLegendPlacement(narrowTall)).toBe("bottom");
  });

  // The documented "no DOM" path: with no real measurement there is no aspect to judge, so
  // nothing moves — the same answer a not-yet-laid-out (0x0) host gives.
  it("an unknown container size keeps the legend below", () => {
    expect(pieLegendPlacement(undefined)).toBe("bottom");
    expect(pieLegendPlacement({ width: 0, height: 0 })).toBe("bottom");
  });

  // The legend is React markup beside the canvas now, so the chart's own box already excludes
  // it: the pie simply fills whatever it is given, at the fraction that layout asks for. Nothing
  // is shifted and nothing is reserved inside the option any more.
  it("the side layout fills its box at 0.85 and stays centred in it", () => {
    const opt = buildPieOption(cols, rows, { ...base, legendPlacement: "side" }, undefined, {
      width: wideShort.width - PIE_LEGEND_COLUMN_WIDTH_PX, height: wideShort.height,
    }) as any;
    // 0.85 * min(640, 661) = 544px of diameter — the number the Metabase comparison asked for,
    // and comfortably under the 550px cap, so the fraction is still what decides it.
    expect(outerRadiusPx(opt) * 2).toBeCloseTo(544, 6);
    expect(opt.series[0].radius[0]).toBe(0); // no donut here
    expect(opt.series[0].center).toEqual(["50%", "50%"]);
  });

  it("the bottom layout's geometry is untouched", () => {
    const opt = buildPieOption(cols, rows, { ...base, legendPlacement: "bottom" }, undefined, narrowTall) as any;
    expect(outerRadiusPx(opt) * 2).toBeCloseTo(0.7 * 360, 6);
    expect(opt.series[0].center).toEqual(["50%", "48%"]);
  });

  it("an unstated placement is the bottom one — today's geometry, unchanged", () => {
    const opt = buildPieOption(cols, rows, base, undefined, wideShort) as any;
    expect(outerRadiusPx(opt) * 2).toBeCloseTo(0.7 * 661, 6);
    expect(opt.series[0].center).toEqual(["50%", "48%"]);
  });

  // No legend, no column to make room for: a chart with `showLegend: false` keeps exactly the
  // geometry it has today even if something upstream still hands it the side placement.
  it("a hidden legend reserves nothing", () => {
    const opt = buildPieOption(
      cols, rows, { ...base, showLegend: false, legendPlacement: "side" }, undefined, wideShort,
    ) as any;
    expect(outerRadiusPx(opt) * 2).toBeCloseTo(0.7 * 661, 6);
    expect(opt.series[0].center).toEqual(["50%", "48%"]);
  });

  // With no measured size at all there are no pixels to state, so the radii stay in ECharts' own
  // percent-of-the-shorter-side units — the geometry this file drew before the diameter cap. A
  // 0x0 host counts as unknown too: EChartsRenderer measures once on mount, and a not-yet-laid-out
  // element reads 0x0, where pixel radii of 0 would draw nothing at all until the first resize.
  it("falls back to percent radii when the container size is unknown or not yet laid out", () => {
    expect((buildPieOption(cols, rows, base) as any).series[0].radius).toEqual(["0%", "70%"]);
    expect((buildPieOption(cols, rows, { ...base, donut: true }) as any).series[0].radius).toEqual(["45%", "70%"]);
    expect((buildPieOption(cols, rows, base, undefined, { width: 0, height: 0 }) as any).series[0].radius)
      .toEqual(["0%", "70%"]);
  });

  // The whole point of this commit: ECharts' `legend` component does not work with a `sunburst`
  // series, so the legend had to leave the canvas before the series type can change. Nothing in
  // the option may re-introduce one.
  it("the built option carries no ECharts legend at all", () => {
    for (const cfg of [base, { ...base, legendPlacement: "side" as const }, { ...base, showLegend: true }]) {
      expect(buildPieOption(cols, rows, cfg, undefined, wideShort)).not.toHaveProperty("legend");
    }
  });

  // The donut's centre total is a `graphic` text element positioned against the CANVAS. With
  // nothing shifted any more it goes back to the plain `left: "center"` it always used.
  it("the donut's centre total stays centred on the canvas", () => {
    const opt = buildPieOption(
      cols, rows, { ...base, donut: true, showTotal: true, legendPlacement: "side" }, undefined, wideShort,
    ) as any;
    const [value, caption] = opt.graphic;
    expect(value).toMatchObject({ left: "center", top: "43%" });
    expect(caption).toMatchObject({ left: "center", top: "53%" });
    expect(value.x).toBeUndefined();
  });
});

// Clicking a legend entry toggled that ring-0 slice off before this commit — ECharts' own
// `legend.selectedMode` defaults to `true`, and a live check (a real ECharts instance, SVG
// renderer, `legendToggleSelect` on "US") confirmed it: ring 0's US sector disappeared, the
// remaining slices re-partitioned the full circle, the legend swatch greyed out, and every OTHER
// ring was left exactly as it was. With the legend out of the canvas, that has to be done here.
describe("a toggled-off legend entry drops its ring-0 slice", () => {
  const cols2 = [
    { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "m", type: "double" },
  ];
  const rows2 = [["US", "Widgets", 40], ["EU", "Gadgets", 10]];
  const cfg = { dimensions: ["region", "category"], metric: "m", sliceThreshold: 0 };

  it("removes exactly that slice from ring 0", () => {
    const opt = buildPieOption(cols2, rows2, { ...cfg, toggledOffSlices: ["US"] }) as any;
    expect(ring(opt, 0).map((d: any) => d.name)).toEqual(["EU"]);
  });

  // CHANGED BY THE SUNBURST, deliberately. As N independent `pie` series, a toggled-off ring-0
  // slice took nothing with it: its children stayed drawn in the outer rings as orphan arcs, sitting
  // under nothing. A tree cannot express that — children belong to their parent — so hiding a slice
  // now hides its whole family, which is what the reader was asking for anyway.
  it("takes the hidden slice's children with it, instead of leaving them as orphan arcs", () => {
    const opt = buildPieOption(cols2, rows2, { ...cfg, toggledOffSlices: ["US"] }) as any;
    expect(ring(opt, 1).map((d: any) => d.name)).toEqual(["Gadgets"]); // "Widgets" was US's child
  });

  // The survivors keep their own colours. A re-deal would otherwise slide every survivor up a
  // palette slot and the React swatches — built from the FULL ring — would then name a colour the
  // chart no longer draws.
  it("keeps each surviving slice's colour rather than re-dealing the palette", () => {
    const opt = buildPieOption(cols2, rows2, { ...cfg, toggledOffSlices: ["US"] }) as any;
    expect(ring(opt, 0)[0].itemStyle.color).toBe(seriesColor(1));
    expect(buildPieLegend(cols2, rows2, cfg).find((e) => e.name === "EU")!.color).toBe(seriesColor(1));
  });

  // Also matching the ECharts legend: neither the hole's total nor the legend's own percentages
  // re-based on what was left showing.
  it("does not re-base the centre total or the legend percentages", () => {
    const opt = buildPieOption(cols2, rows2, { ...cfg, showTotal: true, toggledOffSlices: ["US"] }) as any;
    expect(opt.graphic.map((g: any) => g.style?.text)).toEqual(["50", "Total"]);
    expect(buildPieLegend(cols2, rows2, cfg).map((e) => e.percent)).toEqual(["80%", "20%"]);
  });

  it("nothing toggled off leaves the option byte-for-byte as it was", () => {
    const plain = buildPieOption(cols2, rows2, cfg) as any;
    const empty = buildPieOption(cols2, rows2, { ...cfg, toggledOffSlices: [] }) as any;
    expect(ring(empty, 0).map((d: any) => d.name)).toEqual(ring(plain, 0).map((d: any) => d.name));
    expect(ring(empty, 0).map((d: any) => d.itemStyle.color)).toEqual(
      ring(plain, 0).map((d: any) => d.itemStyle.color),
    );
    expect(ring(plain, 0).map((d: any) => d.itemStyle.color)).toEqual([seriesColor(0), seriesColor(1)]);
  });
});

// Metabase's `getTotalGraphicOption` swaps the hole's contents for whatever the reader is
// pointing at: a slice's value over its name, uppercased when the pointer is on the LEGEND entry
// rather than the slice, and the chart total over "Total" when it is on neither. egret drew only
// the last of those.
//
// The hole is built by its own function rather than inside `buildPieOption` because it has two
// callers: the option (rest state) and React (on hover). Rebuilding the whole option per hover
// would re-run the per-slice `measureText` fit pass at pointer rates — see ChartRenderer.
describe("the pie's hole reads out the slice under the pointer", () => {
  const cols2 = [
    { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
    { name: "m", type: "double" },
  ];
  const rows2 = [["US", "Widgets", 40], ["EU", "Gadgets", 10]];
  const cfg = { dimensions: ["region", "category"], metric: "m" };
  const text = (g: ReturnType<typeof buildPieHoleGraphic>) => (g ?? []).map((e) => e.style.text);
  const hole = () => buildPieHole(cols2, rows2, cfg);

  it("says the chart total over 'Total' when nothing is hovered", () => {
    expect(text(buildPieHoleGraphic(hole(), cfg, null))).toEqual(["50", "Total"]);
  });

  it("says the hovered slice's value over its name", () => {
    expect(text(buildPieHoleGraphic(hole(), cfg, { name: "US", value: 40, fromLegend: false })))
      .toEqual(["40", "US"]);
  });

  it("uppercases the name when the hover came from the legend, not the chart", () => {
    expect(text(buildPieHoleGraphic(hole(), cfg, { name: "US", value: 40, fromLegend: true })))
      .toEqual(["40", "US"]);
    expect(text(buildPieHoleGraphic(hole(), cfg, { name: "Widgets", value: 40, fromLegend: true })))
      .toEqual(["40", "WIDGETS"]);
  });

  // An outer-ring slice reads out exactly the same way — the readout is whatever the pointer is
  // on, at any depth, which is what makes the hole useful on a sunburst rather than a static
  // caption.
  it("reads out an outer-ring slice too, at its own value", () => {
    expect(text(buildPieHoleGraphic(hole(), cfg, { name: "Gadgets", value: 10, fromLegend: false })))
      .toEqual(["10", "Gadgets"]);
  });

  it("formats a hovered value with the metric's own settings, like the total", () => {
    const formatted = { ...cfg, metricTypes: { m: "double" }, columnSettings: { m: { prefix: "$", decimals: 0 } } };
    const model = buildPieHole(cols2, rows2, formatted);
    expect(text(buildPieHoleGraphic(model, formatted, null))[0]).toBe("$50");
    expect(text(buildPieHoleGraphic(model, formatted, { name: "US", value: 40, fromLegend: false }))[0]).toBe("$40");
  });

  it("draws nothing at all where the rest state draws nothing", () => {
    // A single-ring pie with no hole to write in — hovering it must not conjure text into the
    // middle of a solid disc.
    const flat = { dimensions: ["region"], metric: "m" };
    const flatModel = buildPieHole(cols2, rows2, flat);
    expect(buildPieHoleGraphic(flatModel, flat, { name: "US", value: 40, fromLegend: false })).toBeUndefined();
    // …and an explicit `showTotal: false` stays empty on hover too.
    const off = { ...cfg, showTotal: false };
    expect(buildPieHoleGraphic(buildPieHole(cols2, rows2, off), off, { name: "US", value: 40, fromLegend: false }))
      .toBeUndefined();
  });

  // The rest state that ships in the option and the one the hover path produces are the SAME
  // call, so the hole can't end up styled one way at rest and another under the pointer.
  it("is the same thing buildPieOption bakes in for the rest state", () => {
    const opt = buildPieOption(cols2, rows2, cfg) as any;
    expect(opt.graphic).toEqual(buildPieHoleGraphic(hole(), cfg, null));
  });

  // Both elements carry a stable id, which is what lets React re-state JUST the hole as a
  // merge-mode partial option instead of re-applying (and so rebuilding) the whole chart.
  it("gives each line a stable id so it can be re-stated on its own", () => {
    const rest = buildPieHoleGraphic(hole(), cfg, null)!;
    const hovered = buildPieHoleGraphic(hole(), cfg, { name: "US", value: 40, fromLegend: false })!;
    expect(rest.map((e) => e.id)).toEqual(["pie-total-value", "pie-total-label"]);
    expect(hovered.map((e) => e.id)).toEqual(rest.map((e) => e.id));
  });

  // Metabase truncates both lines against `innerRadius * 2` and, when even that leaves them too
  // wide, drops to `valueFontSizeSm` and hides the label. A stub measurer stands in for a real
  // text rasteriser, exactly as the slice-label tests do.
  describe("fitting the text to the hole", () => {
    /** 10px per character, whatever the font. */
    const measure = (t: string) => t.length * 10;
    // A two-ring hole is 2/5 of the outer radius; at 400x400 the circle is 0.7*400 = 280 across,
    // so outer r = 140 and inner r = 56 — a 112px hole, i.e. 11 characters at this stub's scale.
    const size = { width: 400, height: 400 };

    it("truncates a value too wide for the hole", () => {
      const model = { ...hole(), totalText: "1,234,567,890,123" }; // 17 chars = 170px > 112
      const [value] = buildPieHoleGraphic(model, cfg, null, size, measure)!;
      expect(value.style.text).toBe("1,234,567,…");
      expect(value.style.text.length * 10).toBeLessThanOrEqual(112);
    });

    it("keeps a value that fits untouched", () => {
      const [value, label] = buildPieHoleGraphic(hole(), cfg, null, size, measure)!;
      expect(value.style.text).toBe("50");
      expect(label.style.text).toBe("Total");
      expect(value.style.fontSize).toBe(22);
    });

    // Degenerate: at 30x30 the 12px-a-side padding floor leaves a 6px circle, so the hole is
    // 2.4px across — narrower than this stub's own ellipsis. Metabase's escape hatch.
    const tiny = { width: 30, height: 30 };

    it("hides the caption and shrinks the value when the hole is too narrow for both", () => {
      const [value, label] = buildPieHoleGraphic(hole(), cfg, null, tiny, measure)!;
      expect(value.style.text).toBe("…"); // the total is the headline number, so it survives alone
      expect(label.style.text).toBe("");
      expect(value.style.fontSize).toBe(17); // Metabase's valueFontSizeSm
    });

    // A hover readout is transient — showing a name with no value (or the reverse) says less than
    // showing nothing, so it goes entirely. Metabase makes the same split.
    it("drops a hover readout entirely rather than showing half of it", () => {
      const g = buildPieHoleGraphic(hole(), cfg, { name: "US", value: 40, fromLegend: false }, tiny, measure)!;
      expect(g.map((e) => e.style.text)).toEqual(["", ""]);
    });

    // The same "don't guess" rule the slice labels follow: an unmeasured container must not become
    // a guessed-small one, or a perfectly readable total gets ellipsised against a number nobody
    // measured.
    it("never truncates when no real container size is known", () => {
      const model = { ...hole(), totalText: "1,234,567,890,123" };
      const [value] = buildPieHoleGraphic(model, cfg, null, undefined, measure)!;
      expect(value.style.text).toBe("1,234,567,890,123");
    });
  });
});

// Hovering a legend entry has to highlight its slice, which means telling ECharts WHICH datum —
// and a sunburst has one tree, not a series per ring. Metabase dispatches `{ type: "highlight",
// name }`, which ECharts resolves with a linear `indexOfName` scan over the whole tree in
// depth-first pre-order; a sunburst's names are only unique within a parent, and "Other" is a
// name every ring can produce, so the first match is not reliably the ring-0 slice meant. The
// index is derived instead, from the same tree the option builds.
describe("each legend entry points at its own datum in the sunburst's flattened tree", () => {
  const cols3 = [
    { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
    { name: "brand", type: "varchar" }, { name: "m", type: "double" },
  ];
  // Deliberately shaped so both rings fold their own "Other" buckets: name lookup would be
  // ambiguous here, index lookup cannot be.
  const rows3 = [
    ["US", "Widgets", "Acme", 40], ["US", "Widgets", "Bolt", 30], ["US", "Gadgets", "Cog", 20],
    ["US", "Trinkets", "Dot", 1], ["US", "Baubles", "Eft", 1],
    ["EU", "Widgets", "Acme", 25], ["EU", "Gizmos", "Fig", 15],
    ["EU", "Trinkets", "Gnu", 1], ["EU", "Baubles", "Hob", 1],
    // Two regions small enough to fold into ring 0's own "Other" — so the name "Other" exists at
    // more than one depth, which is the case a name lookup gets wrong.
    ["AF", "Widgets", "Acme", 1], ["SA", "Widgets", "Bolt", 1],
  ];
  const cfg = { dimensions: ["region", "category", "brand"], metric: "m" };

  /** The datum list ECharts builds from `series.data`: one virtual root, then depth-first
   * pre-order (SunburstSeries.getInitialData -> Tree.createTree). */
  const flatten = (nodes: any[]): any[] => nodes.flatMap((n: any) => [n, ...flatten(n.children ?? [])]);
  const datumList = (opt: any) => [{ name: "<root>" }, ...flatten(opt.series[0].data)];

  it("every entry's index lands on a datum of that entry's own name", () => {
    const opt = buildPieOption(cols3, rows3, cfg) as any;
    const data = datumList(opt);
    const entries = buildPieLegend(cols3, rows3, cfg);
    expect(entries.length).toBeGreaterThan(1);
    for (const e of entries) {
      expect(e.sliceDataIndex).toBeTypeOf("number");
      expect(data[e.sliceDataIndex!].name).toBe(e.name);
    }
  });

  // The guard the whole approach exists for: a name lookup would resolve ring 0's "Other" to
  // whichever nested "Other" the depth-first scan reached first.
  it("does not point at some deeper ring's slice that shares the name", () => {
    const opt = buildPieOption(cols3, rows3, cfg) as any;
    const data = datumList(opt);
    const names = data.map((d: any) => d.name);
    // The fixture really is ambiguous — otherwise this test proves nothing.
    expect(names.filter((n: string) => n === "Other").length).toBeGreaterThan(1);
    const other = buildPieLegend(cols3, rows3, cfg).find((e) => e.name === "Other")!;
    // The ring-0 "Other" is NOT the first "Other" the scan meets, so a name payload would have
    // lit up a nested one instead.
    expect(other.sliceDataIndex).not.toBe(names.indexOf("Other"));
    expect(data[other.sliceDataIndex!].name).toBe("Other");
    expect(data[other.sliceDataIndex!].value).toBe(other.value);
  });

  it("withholds the index for a toggled-off entry, and re-indexes the ones after it", () => {
    const before = buildPieLegend(cols3, rows3, cfg);
    const off = { ...cfg, toggledOffSlices: [before[0].name] };
    const after = buildPieLegend(cols3, rows3, off);
    expect(after[0].sliceDataIndex).toBeUndefined();
    // The survivors move up to fill the hole the removed subtree left.
    expect(after[1].sliceDataIndex).toBe(1);
    expect(before[1].sliceDataIndex).toBeGreaterThan(1);

    // …and they still land on the right datum in the tree that is actually drawn.
    const data = datumList(buildPieOption(cols3, rows3, off) as any);
    for (const e of after.filter((x) => x.sliceDataIndex != null)) {
      expect(data[e.sliceDataIndex!].name).toBe(e.name);
    }
  });

  // The legend hover reads the slice's value out in the chart's centre, and unlike a chart hover
  // there is no ECharts event to take it from.
  it("carries each entry's raw value", () => {
    const entries = buildPieLegend(cols3, rows3, cfg);
    const opt = buildPieOption(cols3, rows3, cfg) as any;
    expect(entries.map((e) => e.value)).toEqual(ring(opt, 0).map((d: any) => d.value));
  });
});

// egret's tooltip said `Organic: 743 (33.73%)` — what you already clicked on. Metabase's says what
// that is 33.73% OF: the hovered slice's siblings, each with its value and share, with the one
// under the pointer marked. Ported from `getTooltipModel`
// (visualizations/PieChart/use-chart-events.ts).
describe("a pie's tooltip breaks down the slice's siblings", () => {
  const cols3 = [
    { name: "region", type: "varchar" }, { name: "product", type: "varchar" },
    { name: "channel", type: "varchar" }, { name: "m", type: "double" },
  ];
  const rows3 = [
    ["R1", "P1", "Organic", 600], ["R1", "P1", "Paid", 400],   // P1 = 1000
    ["R1", "P2", "Organic", 500],                               // P2 = 500;  R1 = 1500
    ["R2", "P3", "Organic", 300], ["R2", "P3", "Paid", 200],    // P3 = 500;  R2 = 500
  ];
  const cfg = { dimensions: ["region", "product", "channel"], metric: "m" };
  const tip = (path: string[], c: any = cfg, cols = cols3, rws = rows3) =>
    (buildPieOption(cols, rws, c) as any).tooltip.formatter({
      treePathInfo: [{ name: "" }, ...path.map((name) => ({ name }))],
    }) as string;
  /** The table body, as [name, value, percent] triples, ignoring markup. */
  const rowsOf = (html: string) =>
    [...html.matchAll(/<tr[^>]*>(.*?)<\/tr>/g)].map((m) =>
      [...m[1].matchAll(/<td[^>]*>(.*?)<\/td>/g)].map((c) => c[1].replace(/<[^>]*>/g, "")));

  it("lists the hovered slice's siblings, each with its value and share of their parent", () => {
    // Pointing at one channel under P1 shows every channel under P1 — that is the comparison the
    // reader is actually making.
    expect(rowsOf(tip(["R1", "P1", "Organic"]))).toEqual([
      ["Organic", "600", "60%"],
      ["Paid", "400", "40%"],
      ["Total", "1,000", "100%"],
    ]);
  });

  it("marks the slice under the pointer, and only it", () => {
    const html = tip(["R1", "P1", "Paid"]);
    const bold = [...html.matchAll(/<tr style="font-weight:(\d+)">(.*?)<\/tr>/g)]
      .filter((m) => m[1] === "700")
      .map((m) => m[2].replace(/<[^>]*>/g, "").match(/^[A-Za-z]+/)![0]);
    expect(bold).toEqual(["Paid"]);
  });

  // At ring 0 the dimension names the comparison; deeper, the ancestors do — you are looking at
  // channels WITHIN R1 > P1, and the header is the only thing that says so.
  it("heads the table with the dimension at ring 0 and the ancestors below it", () => {
    expect(tip(["R1"])).toContain(">region<");
    expect(tip(["R1", "P1"])).toContain(">R1<");
    expect(tip(["R1", "P1", "Organic"])).toContain(">R1  &gt;  P1<");
  });

  it("uses the column's display title in the header when it has one", () => {
    const titled = { ...cfg, columnSettings: { region: { column_title: "Sales region" } } };
    expect(tip(["R1"], titled)).toContain(">Sales region<");
  });

  // A colour marker only where a colour means one thing. "Organic" is drawn in a different shade
  // under every parent, so a swatch beside it in an outer-ring table would claim a colour it does
  // not consistently have.
  it("shows a colour marker at ring 0 only", () => {
    expect(tip(["R1"])).toContain("background:");
    expect(tip(["R1", "P1"])).not.toContain("background:");
    expect(tip(["R1", "P1", "Organic"])).not.toContain("background:");
  });

  // "Other" is not a category to compare against its neighbours — pointing at it is a question
  // about what went into it.
  it("lists an Other bucket's children rather than its siblings", () => {
    const cols2 = [
      { name: "d", type: "varchar" }, { name: "c", type: "varchar" }, { name: "m", type: "double" },
    ];
    // Three regions small enough to fold into ring 0's Other (37 of 1000), each with one channel.
    const rows2 = [["Big", "Main", 963], ["T1", "Ch1", 15], ["T2", "Ch2", 12], ["T3", "Ch3", 10]];
    const nested = { dimensions: ["d", "c"], metric: "m" };
    expect(rowsOf(tip(["Other"], nested, cols2, rows2))).toEqual([
      ["Ch1", "15", "40.54%"],
      ["Ch2", "12", "32.43%"],
      ["Ch3", "10", "27.03%"],
      ["Total", "37", "100%"],
    ]);
    // …and nothing in it is marked as focused, because the pointer is on the bucket, not a row.
    expect(tip(["Other"], nested, cols2, rows2)).not.toContain('font-weight:700">');
  });

  // At the OUTERMOST ring the slices an Other folded away are not in the tree at all — they are
  // not drawn — so there are no children to list and the sibling comparison is what is left.
  it("falls back to the siblings for an Other bucket with nothing nested in it", () => {
    const cols2 = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
    const rows2 = [["Big", 960], ["Tiny1", 10], ["Tiny2", 15], ["Tiny3", 15]];
    const flat = { dimensions: ["d"], metric: "m" };
    expect(rowsOf(tip(["Other"], flat, cols2, rows2))).toEqual([
      ["Big", "960", "96%"],
      ["Other", "40", "4%"],
      ["Total", "1,000", "100%"],
    ]);
  });

  it("drops the Total row when there is only one sibling to add up", () => {
    // P2 is R1's only product with one channel — nothing to total.
    expect(rowsOf(tip(["R1", "P2", "Organic"]))).toEqual([["Organic", "500", "100%"]]);
  });

  it("formats values with the metric's own settings", () => {
    const money = { ...cfg, metricTypes: { m: "double" }, columnSettings: { m: { prefix: "$", decimals: 0 } } };
    expect(rowsOf(tip(["R1", "P1", "Organic"], money))[0]).toEqual(["Organic", "$600", "60%"]);
  });

  it("honours the chart's own decimal precision on the shares", () => {
    const cols2 = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
    const rows2 = [["A", 1], ["B", 2]];
    const three = { dimensions: ["d"], metric: "m", decimals: 3 };
    expect(rowsOf(tip(["A"], three, cols2, rows2))[1]).toEqual(["A", "1", "33.333%"]);
  });

  // Every name in that table is a DIMENSION VALUE, going into an ECharts tooltip via innerHTML.
  it("escapes dimension values instead of injecting them as markup", () => {
    const cols2 = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
    const rows2 = [["<img src=x onerror=alert(1)>", 60], ["safe", 40]];
    const html = tip(["<img src=x onerror=alert(1)>"], { dimensions: ["d"], metric: "m" }, cols2, rows2);
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
  });

  it("says nothing at all for a path it cannot resolve", () => {
    expect(tip([])).toBe("");
    expect(tip(["nope"])).toBe("");
  });

  // The shared item tooltip is what funnel, map, boxplot and sankey draw; none of them has a
  // sibling structure, and none of their behaviour changes because the pie stopped using it.
  it("leaves the shared item tooltip alone", () => {
    const shared = itemTooltip((v) => String(v), { percent: true });
    expect(shared.formatter({ name: "EU", value: 10, percent: 25 })).toBe("EU: 10 (25%)");
  });
});
