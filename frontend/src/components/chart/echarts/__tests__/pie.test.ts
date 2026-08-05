import { describe, it, expect } from "vitest";
import {
  buildPieOption, buildPieLegend, buildPieHole, buildPieHoleGraphic, pieLegendPlacement, resolvePercentVisibility,
  PIE_LEGEND_COLUMN_WIDTH_PX, computeMaxPercentDecimals, pieHasSlices, buildPieBreakdown,
  pieSliceDrillPath, pieSliceRows,
} from "../options/pie";
import { SERIES_PALETTE, chartShade, COLOR_FALLBACK } from "../palette";
import { colorsForKeys } from "../keyColors";
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

/**
 * The hole's two lines, in order (value, caption).
 *
 * The pie states them as ONE `graphic` group — Metabase's own construction (option.ts's
 * `getTotalGraphicOption`), adopted because two independently positioned texts could not hold a
 * fixed 22px apart or share the rings' centre; see buildPieHoleGraphic. So anything reading the
 * hole reads the group's children, and `graphic` itself is one element, not two.
 */
const holeLines = (graphic: any): any[] => graphic?.[0]?.children ?? [];
const holeTexts = (graphic: any): (string | undefined)[] =>
  holeLines(graphic).map((e: any) => e.style?.text);

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
  /**
   * Metabase's `getDefaultShowLabels` (shared/settings/pie.ts): `false` once there is a single
   * dimension, `true` above that. A one-ring pie therefore draws NOTHING on its arcs by default
   * and leaves naming to the legend, which is already on and — since the percentage default moved
   * to "legend" — already carries each slice's share beside its name.
   */
  const ringsOf = (dims: string[], extra: Record<string, unknown> = {}) =>
    (buildPieOption(
      [{ name: "region", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "rev", type: "double" }],
      [["EU", "A", 10], ["US", "B", 30]],
      { dimensions: dims, metric: "rev", ...extra },
    ) as any).series[0];

  it("showLabels defaults OFF on a one-ring pie, ON above it; an explicit value always wins", () => {
    expect(ringsOf(["region"]).label.show).toBe(false);
    expect(ringsOf(["region"], { showLabels: true }).label.show).toBe(true);
    expect(ringsOf(["region", "cat"]).label.show).toBe(true);
    expect(ringsOf(["region", "cat"], { showLabels: false }).label.show).toBe(false);
  });

  it("the default one-ring pie still names its slices — in the legend, with their share", () => {
    expect(buildPieLegend(cols, rows, base).map((e) => `${e.name} ${e.percent}`))
      .toEqual(["US 75%", "EU 25%"]);
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
  it("the Other slice is muted; normal slices take the colour their name hashes to", () => {
    const d = slices([["A", 50], ["B", 45], ["C", 2], ["D", 2], ["E", 1]]);
    const other = d.find((s: any) => s.name === "Other").itemStyle.color;
    expect(other).toBeTruthy();
    // Ring 0's palette slots are pinned onto the slice rather than left implicit — a sunburst
    // assigns them by NAME and skips any slice that already has a colour, so an implicit slot
    // would shift the moment one slice carried an override. See the colour-inheritance block.
    const byName = colorsForKeys(["A", "B", "Other"]);
    expect(d.find((s: any) => s.name === "A").itemStyle.color).toBe(byName.A);
    expect(other).not.toBe(byName.A);
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

  it("renders a centre total only where there is a hole to write it in", () => {
    expect(opt({ showTotal: true }).graphic).toBeFalsy();           // not a donut: nowhere to put it
    expect(opt({ donut: true }).graphic).toBeTruthy();              // a hole, and showTotal defaults on
    expect(opt({ donut: true, showTotal: false }).graphic).toBeFalsy();
    expect(opt({ donut: true, showTotal: true }).graphic).toBeTruthy();
  });
  it("centre total shows the formatted grand total + a Total caption", () => {
    const g = opt({ donut: true, showTotal: true, metricTypes: { m: "double" }, columnSettings: { m: { prefix: "$", decimals: 0 } } }).graphic;
    expect(holeTexts(g)).toContain("$100");
    expect(holeTexts(g)).toContain("Total");
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
    // 1/3, at the precision the DATA asks for: under a slice label's two-significant-digit budget
    // (see computeMaxPercentDecimals) neither third needs a decimal at all.
    expect(labelText(thirds, "A")).toBe("33%");
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

// Task 5 — ring colour inheritance: an outer ring's slice takes a VARIANT of its parent slice's
// own colour rather than an unrelated palette colour (Metabase's getRingColorAlias).
//
// THE VARIANTS ARE METABASE'S THREE FIXED LEVELS — inner dark, middle base, outer light — not a
// per-depth lightening ramp. egret used to start at the base and lighten outward in OKLCH, which
// is why a live side-by-side had it disagreeing with Metabase on every ring at once: egret's
// ring-1 blue measured #76C4FF against Metabase's #87BCEC, and its innermost ring was the medium
// base where Metabase's is the deep #227FD2.
//
// Ring 0's own colours are now PINNED explicitly rather than left to ECharts' palette. They used
// to be left implicit (no `itemStyle` at all on an un-overridden slice), which a `pie` series
// makes safe — it colours by datum INDEX. A `sunburst` colours a top-level node by NAME and skips
// the palette entirely for a node that already has a colour, so one `sliceColors` override on
// ring 0 would have consumed no slot and slid every later slice's colour up by one, while the
// React legend and the label-contrast pass (both deriving a colour from the ring INDEX) went on
// naming the old ones. What ring 0 is pinned TO is now the colour each slice's NAME hashes to
// (see the by-name block further down); the variants below still derive from whatever that is.
describe("buildPieOption — ring colour inheritance (task 5)", () => {
  const cols2 = [
    { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "rev", type: "double" },
  ];
  const rows2 = [
    ["US", "Widgets", 30], ["US", "Gadgets", 10],
    ["EU", "Widgets", 5], ["EU", "Gadgets", 5],
  ];

  // The rule, on the shape it was measured on: three rings, dark then base then light. Stated
  // with Metabase's OWN published hexes for the first accent rather than with a call to the
  // derivation under test — this is the assertion that says egret draws the same colours a
  // Metabase sunburst does, so it cannot be satisfied by any self-consistent ramp.
  it("paints the rings dark, base, then light — Metabase's own accent variants", () => {
    const cols3 = [
      { name: "a", type: "varchar" }, { name: "b", type: "varchar" }, { name: "c", type: "varchar" },
      { name: "m", type: "double" },
    ];
    // "H" is not decorative: ring 0's base is the colour its NAME hashes to, and "H" is the
    // shortest name that hashes to accent0 — which is what lets this state Metabase's published
    // accent0 variants rather than a call to the derivation under test.
    const rows3 = [["H", "B", "C", 10]];
    const opt = buildPieOption(cols3, rows3, { dimensions: ["a", "b", "c"], metric: "m" }) as any;
    expect(colorsForKeys(["H"]).H).toBe("#509EE3");
    expect(ring(opt, 0)[0].itemStyle.color.toUpperCase()).toBe("#227FD2"); // accent0-dark
    expect(ring(opt, 1)[0].itemStyle.color.toUpperCase()).toBe("#509EE3"); // accent0
    expect(ring(opt, 2)[0].itemStyle.color.toUpperCase()).toBe("#87BCEC"); // accent0-light
  });

  // Ring 0's own colour is now keyed on the slice's NAME (see the by-name block below); what this
  // pins is that the deeper rings still VARY FROM IT — dark inside base — whatever that base is.
  const REGIONS = ["US", "EU"];

  it("a two-ring pie is the dark variant inside its base, per family", () => {
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "rev" }) as any;
    const inner = ring(opt, 0); // [US(40), EU(10)] — value-desc, so NOT alphabetical
    expect(inner.map((d: any) => d.name)).toEqual(REGIONS);
    const byName = colorsForKeys(REGIONS);
    expect(inner.map((d: any) => d.itemStyle.color)).toEqual(REGIONS.map((r) => chartShade(byName[r])));

    const outer = ring(opt, 1);
    const widgetsUnderUS = outer.find((d: any) => d.name === "Widgets" && d.value === 30);
    const widgetsUnderEU = outer.find((d: any) => d.name === "Widgets" && d.value === 5);
    // The middle ring is the family's base colour itself, undarkened — so a family reads as one
    // hue at two lightnesses rather than as two unrelated colours.
    expect(widgetsUnderUS.itemStyle.color).toBe(byName.US);
    expect(widgetsUnderEU.itemStyle.color).toBe(byName.EU);
  });

  // A single ring is deliberately left as it was: Metabase's own `hasMultipleRings` gate, and the
  // one case where changing the variant would restyle every saved pie in the product.
  it("a single-ring pie keeps the base colours, undarkened", () => {
    const opt = buildPieOption(cols2, rows2, { dimensions: ["region"], metric: "rev" }) as any;
    expect(ring(opt, 0).map((d: any) => d.itemStyle.color)).toEqual(
      REGIONS.map((r) => colorsForKeys(REGIONS)[r]),
    );
  });

  it("a sliceColors override wins, and its rings vary from the override, not the palette colour it replaced", () => {
    const opt = buildPieOption(cols2, rows2, {
      dimensions: ["region", "category"], metric: "rev", sliceColors: { US: "#123456" },
    }) as any;
    const inner = ring(opt, 0);
    // The override is the family's BASE, so ring 0 shows its dark variant — the same treatment
    // the palette gets. (Metabase leaves a non-accent hex flat at every ring instead, which is
    // the one place this deliberately does more than the port.)
    expect(inner.find((d: any) => d.name === "US").itemStyle.color).toBe(chartShade("#123456"));

    const outer = ring(opt, 1);
    const widgetsUnderUS = outer.find((d: any) => d.name === "Widgets" && d.value === 30);
    // Must derive from the OVERRIDE ("#123456"), not from the colour "US" hashes to — the one it
    // would otherwise have gotten.
    expect(widgetsUnderUS.itemStyle.color).toBe("#123456");
    expect(widgetsUnderUS.itemStyle.color).not.toBe(colorsForKeys(REGIONS).US);
  });

  // Chart settings are JSONB and never whitelisted server-side, so a slice colour can arrive as
  // anything at all. resolveColor is this codebase's one validator for a colour from a less
  // trusted source; an override that skipped it could paint a slice with a value the renderer
  // can't draw, which getTextColorForBackground would then silently mis-contrast a label against.
  //
  // It matters twice over now that ring 0's overrides reach `colorsForKeys` as its `existing` map:
  // that function has no opinion about what a colour is, so an unvalidated "banana" would be taken
  // as the slice's pinned colour and written straight onto the datum. Validation happens before the
  // map is built, which is why there is one map and not two applications.
  it("a slice colour override the renderer can't draw is rejected, leaving the hashed colour", () => {
    const opt = buildPieOption(cols2, rows2, {
      dimensions: ["region"], metric: "rev", sliceColors: { US: "banana", EU: "rgb(1,2,3)" },
    }) as any;
    const inner = ring(opt, 0);
    // Not a colour: no override at all, so the slice keeps the colour its own name hashes to.
    expect(inner.find((d: any) => d.name === "US").itemStyle.color)
      .toBe(colorsForKeys(REGIONS, { EU: "rgb(1,2,3)" }).US);
    expect(inner.find((d: any) => d.name === "US").itemStyle.color).not.toBe("banana");
    // A valid non-hex colour still comes through verbatim.
    expect(inner.find((d: any) => d.name === "EU").itemStyle.color).toBe("rgb(1,2,3)");
  });

  /**
   * An override matches by NAME on EVERY ring, not just ring 0 — a value used as a category as
   * well as a region takes it in both places. Long-standing behaviour that had no test of its own,
   * which is why it is pinned here: ring 0's overrides now travel a different route (they are
   * `colorsForKeys`' `existing` map, so they consume their colour as well as claim it), and the
   * deeper rings' straight assignment is all that is left of the original loop. Deleting that loop
   * used to leave the whole suite green.
   */
  it("an override on a name that only appears on an outer ring still reaches it", () => {
    const opt = buildPieOption(cols2, rows2, {
      dimensions: ["region", "category"], metric: "rev", sliceColors: { Widgets: "#123456" },
    }) as any;
    // "Widgets" is a ring-1 value and appears under both parents, so both arcs take it.
    expect(ring(opt, 1).filter((d: any) => d.name === "Widgets").map((d: any) => d.itemStyle.color))
      .toEqual(["#123456", "#123456"]);
    // ...and ring 0, which never carries that name, is untouched.
    expect(ring(opt, 0).map((d: any) => d.itemStyle.color))
      .toEqual(REGIONS.map((r) => chartShade(colorsForKeys(REGIONS)[r])));
  });

  it("a ring's children whose parent folded into the PARENT ring's Other bucket inherit Other's own muted colour", () => {
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
    // Other is a family like any other: its muted grey is the base, so the inner ring shows the
    // grey's dark variant and what nests under it shows the grey itself.
    const muted = COLOR_FALLBACK.axisLabel; // resolveColor("var(--muted-foreground)") under jsdom
    expect(other.itemStyle.color).toBe(chartShade(muted));

    const outer = ring(opt, 1);
    const x = outer.find((d: any) => d.name === "X");
    const y1 = outer.find((d: any) => d.name === "Y1");
    expect(x).toBeTruthy();
    expect(y1).toBeTruthy();
    expect(x.itemStyle.color).toBe(muted);
    expect(y1.itemStyle.color).toBe(muted);
    // ...and NOT one of the palette families, which is the whole point of the Other anchor.
    expect(SERIES_PALETTE).not.toContain(x.itemStyle.color);
  });
});

/**
 * WHICH colour a ring-0 slice is drawn in — Metabase's rule, which is a hash of the slice's NAME
 * rather than its position in the ring. A category is then the same colour on every card of a
 * dashboard that breaks out by it, and it does not change when the sort order does or when another
 * category appears beside it.
 *
 * The ring VARIANTS (dark inner, base, light outer) are a separate question and are pinned by the
 * inheritance block above; all this changes is the base each family varies from.
 */
describe("buildPieOption — a ring-0 slice takes its colour from its name", () => {
  const cols2 = [{ name: "category", type: "varchar" }, { name: "m", type: "double" }];
  /**
   * The four Sample-Database product categories. Ring 0 is emitted value-DESC, so this fixture's
   * emission order is Widget, Doohickey, Gizmo, Gadget — deliberately NOT alphabetical.
   *
   * THE ORDER IS LOAD-BEARING. `colorsForKeys` assigns in sorted key order on the hash path, so its
   * returned Record is in sorted order too: a fixture whose slices are already sorted makes a
   * by-name lookup and a by-position one produce the identical sequence, and a test named for the
   * distinction passes against either. Two of PR 04's cartesian tests shipped exactly that way.
   * `SLICE_ORDER` is asserted below so re-sorting these rows cannot re-blind them quietly.
   *
   * "Gadget" and "Widget" also hash onto the SAME palette slot, so this data exercises the
   * collision walk rather than only the happy path.
   */
  const rows2 = [["Widget", 40], ["Doohickey", 30], ["Gizmo", 20], ["Gadget", 10]];
  const SLICE_ORDER = ["Widget", "Doohickey", "Gizmo", "Gadget"];
  const NAMES = ["Doohickey", "Gadget", "Gizmo", "Widget"];
  const pieCfg = { dimensions: ["category"], metric: "m" };
  const buildPie = (extra: Record<string, unknown> = {}) =>
    buildPieOption(cols2, rows2, { ...pieCfg, ...extra }) as any;
  const ring0ColorOf = (opt: any, name: string) =>
    ring(opt, 0).find((d: any) => d.name === name).itemStyle.color;

  it("colours each slice by its name rather than by its place in the ring", () => {
    const opt = buildPie();
    expect(ring(opt, 0).map((d: any) => d.name)).toEqual(SLICE_ORDER); // the order this pins against
    const byName = colorsForKeys(NAMES);
    expect(ring(opt, 0).map((d: any) => d.itemStyle.color)).toEqual(SLICE_ORDER.map((n) => byName[n]));
  });

  it("keys a slice's colour to its name, and honours sliceColors", () => {
    // The override is itself a palette accent, which is the interesting case: `colorsForKeys`
    // CONSUMES it, so whichever slice would have hashed onto that slot moves along instead of
    // being drawn in the same ink as the pinned one.
    const sliceColors = { Gadget: SERIES_PALETTE[6] };
    const opt = buildPie({ sliceColors });
    expect(ring0ColorOf(opt, "Gadget")).toBe(SERIES_PALETTE[6]);
    expect(ring0ColorOf(opt, "Widget")).toBe(colorsForKeys(NAMES, sliceColors).Widget);
    expect(new Set(ring(opt, 0).map((d: any) => d.itemStyle.color)).size).toBe(4);
  });

  it("keeps a slice's colour when the ring is drawn in a different order", () => {
    const flipped = buildPieOption(cols2, [...rows2].reverse(), { ...pieCfg, sortRows: false }) as any;
    expect(ring(flipped, 0).map((d: any) => d.name)).toEqual([...SLICE_ORDER].reverse());
    for (const name of SLICE_ORDER) {
      expect(ring0ColorOf(flipped, name)).toBe(ring0ColorOf(buildPie(), name));
    }
  });

  /**
   * THE SYNTHETIC "OTHER" BUCKET IS NOT A KEY.
   *
   * Its colour is fixed in `buildSlices` (the muted tint) and nothing ever reads a hashed one for
   * it — but a name handed to `colorsForKeys` costs a SLOT whether or not the answer is used, and
   * "Other" hashes onto the same slot as "Q1" and sorts before it. So including it moved Q1 off
   * accent0 for no reason a reader could see.
   *
   * Whether ring 0 has an Other bucket at all is data-dependent: `buildSlices` folds whatever falls
   * under the threshold. A real category's colour would therefore have changed when some unrelated,
   * SMALLER category crossed into or out of the fold — the exact instability the by-name rule exists
   * to remove, reintroduced in the one place a reader would not look for it.
   *
   * The two fixtures differ only in three tiny rows that fold away. Q1 is 45 in both.
   */
  const otherCols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
  const foldPie = (rws: unknown[][]) =>
    buildPieOption(otherCols, rws, { dimensions: ["d"], metric: "m" }) as any;
  /** C, D and E are each under the 2.5% fold threshold, so ring 0 gains an "Other". */
  const WITH_OTHER: unknown[][] = [["Widget", 50], ["Q1", 45], ["C", 2], ["D", 2], ["E", 1]];
  const WITHOUT_OTHER: unknown[][] = [["Widget", 50], ["Q1", 45]];

  it("does not let the Other bucket take a real slice's colour", () => {
    const folded = foldPie(WITH_OTHER);
    const plain = foldPie(WITHOUT_OTHER);
    // The premise: the fold really did happen in one case and not the other.
    expect(ring(folded, 0).map((d: any) => d.name)).toEqual(["Widget", "Q1", "Other"]);
    expect(ring(plain, 0).map((d: any) => d.name)).toEqual(["Widget", "Q1"]);
    // "Other" and "Q1" hash onto the same slot, and "Other" sorts first — so a Q1 that is still
    // accent0 here is a Q1 the synthetic bucket did not push along.
    expect(colorsForKeys(["Widget", "Q1"]).Q1).toBe(SERIES_PALETTE[0]);
    expect(ring0ColorOf(folded, "Q1")).toBe(SERIES_PALETTE[0]);
    // Stated as an identity too, since that is the property: a category's colour does not depend
    // on whether some smaller, unrelated category folded away.
    expect(ring0ColorOf(folded, "Q1")).toBe(ring0ColorOf(plain, "Q1"));
    expect(ring0ColorOf(folded, "Widget")).toBe(ring0ColorOf(plain, "Widget"));
  });

  it("a sliceColors entry aimed at Other neither repaints it nor costs a real slice its colour", () => {
    // `sliceColors` reaches ring 0 as `colorsForKeys`' `existing` map, where an entry CONSUMES its
    // colour. An "Other" entry must get no further than it did before: the bucket keeps its muted
    // tint (it is not offered to the override at all), and no accent is spent on its behalf.
    const pinned = foldPie(WITH_OTHER.map((r) => r)) as any;
    const withEntry = buildPieOption(otherCols, WITH_OTHER, {
      dimensions: ["d"], metric: "m", sliceColors: { Other: SERIES_PALETTE[0] },
    }) as any;
    expect(ring0ColorOf(withEntry, "Other")).toBe(ring0ColorOf(pinned, "Other"));
    expect(ring0ColorOf(withEntry, "Other")).not.toBe(SERIES_PALETTE[0]);
    expect(ring0ColorOf(withEntry, "Q1")).toBe(SERIES_PALETTE[0]);
  });

  /**
   * ...and a dimension value that genuinely IS the string "Other" is a real category, not the
   * bucket. The two are told apart by the OTHER_KEY sentinel rather than by the name (this file
   * accepts the name collision knowingly), so excluding the bucket from the key set must not
   * silently leave such a slice unpainted.
   */
  it("a real category literally named Other still hashes to a colour", () => {
    const opt = foldPie([["Other", 60], ["Widget", 40]]);
    expect(ring(opt, 0).map((d: any) => d.name)).toEqual(["Other", "Widget"]);
    const byName = colorsForKeys(["Other", "Widget"]);
    expect(ring(opt, 0).map((d: any) => d.itemStyle.color)).toEqual([byName.Other, byName.Widget]);
    expect(SERIES_PALETTE).toContain(ring0ColorOf(opt, "Other"));
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
    // 33.333…%, so the decimal settings genuinely differ.
    const rows = [["A", 10], ["B", 20]];
    const labelOfA = (o: any) => ring(o, 0).find((d: any) => d.name === "A").label.formatter();
    const withDefault = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", percentVisibility: "inside", showLabels: false }) as any;
    const withThree = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", percentVisibility: "inside", decimals: 3, showLabels: false }) as any;
    // Unset asks the data, which (at two significant digits on a slice) wants no decimals here.
    expect(labelOfA(withDefault)).toBe("33%");
    expect(labelOfA(withThree)).toBe("33.333%");
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
      // Value-desc, so [US(30), EU(10)] — and each swatch is the colour that NAME hashes to.
      expect(entries.map((e) => e.name)).toEqual(["US", "EU"]);
      expect(entries.map((e) => e.color)).toEqual(
        ["US", "EU"].map((r) => colorsForKeys(["US", "EU"])[r]),
      );
      const overridden = buildPieLegend(cols, rows, {
        dimensions: ["region"], metric: "rev", sliceColors: { US: "#123456" },
      });
      expect(overridden.find((e) => e.name === "US")!.color).toBe("#123456");
    });
  });

  describe("centre total keeps working under the new hole ratios (Metabase's total.* constants)", () => {
    it("uses Metabase's labelFontSize (14) and bolds the caption too, one ring", () => {
      const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", donut: true, showTotal: true }) as any;
      const [value, label] = holeLines(opt.graphic);
      expect(value.style.fontSize).toBe(22);
      expect(value.style.fontWeight).toBe(700);
      expect(label.style.fontSize).toBe(14);
      expect(label.style.fontWeight).toBe(700); // previously unset — Metabase bolds the caption too
    });

    // RE-BASELINED. A three-ring pie used to START at valueFontSizeSm on the theory that its hole
    // (1/4 of the outer radius) was too tight for 22px — a guess, applied without measuring
    // anything, and measurably wrong on the chart the live comparison was made against: a 66px
    // hole radius holds `22,437` at 22px with ~54px to spare, and Metabase draws it at 22 there.
    // The small size is Metabase's overflow escape hatch (`hasSufficientWidth`), so it is the fit
    // pass below — against the REAL hole — that reaches for it, at any ring count.
    it("keeps the full 22px under the tightest (three-ring) hole, which fits it", () => {
      const opt = buildPieOption(cols3, rows3, {
        dimensions: ["region", "category", "brand"], metric: "m", donut: true, showTotal: true,
      }, undefined, { width: 830, height: 661 }) as any;
      const [value] = holeLines(opt.graphic);
      expect(value.style.fontSize).toBe(22);
    });

    it("still renders at the regular size under the two-ring hole", () => {
      const opt = buildPieOption(cols2, rows2, {
        dimensions: ["region", "category"], metric: "m", donut: true, showTotal: true,
      }) as any;
      const [value] = holeLines(opt.graphic);
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
      expect(holeTexts(opt.graphic)).toEqual(["50", "Total"]);
    });

    it("three rings show it too", () => {
      const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
      expect(opt.graphic).toBeTruthy();
      expect(holeTexts(opt.graphic)).toEqual(["18", "Total"]);
    });

    it("an explicit false still empties the hole", () => {
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m", showTotal: false }) as any;
      expect(opt.graphic).toBeFalsy();
    });

    it("multi-ring does not need donut set — the hole is there either way", () => {
      const off = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m", donut: false }) as any;
      expect(off.graphic).toBeTruthy();
    });

    // A one-ring donut joined it in task 5 — see the task-5 block at the bottom of this file for
    // why "its hole is small enough to read as deliberate" did not survive being measured.
    it("a single-ring donut fills its hole too", () => {
      const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", donut: true }) as any;
      expect(opt.graphic).toBeTruthy();
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

      // A single ring gets the same treatment as of task 5 — see the task-5 block at the bottom
      // of this file. Its arcs stay clean either way; only the legend gains the number. Cleaner
      // still now that `resolveShowLabels` leaves a one-ring pie's arcs bare: the name goes with
      // the percentage into the legend, and the arc gets the "nothing to show" sentinel.
      it("a single-ring pie gets it in the legend as well, and keeps its arcs clean", () => {
        const cfg = { dimensions: ["region"], metric: "rev" };
        expect(buildPieLegend(cols, rows, cfg).map((e) => e.percent)).toEqual(["75%", "25%"]);
        const opt = buildPieOption(cols, rows, cfg) as any;
        expect(ring(opt, 0).find((d: any) => d.name === "US").label.formatter()).toBe(" ");
        // …and asking for labels puts the name back on the arc, without the percentage.
        const withLabels = buildPieOption(cols, rows, { ...cfg, showLabels: true }) as any;
        expect(ring(withLabels, 0).find((d: any) => d.name === "US").label.formatter()).toBe("US");
      });

      it("an explicit off still wins over the default", () => {
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
  it("passes every known value through", () => {
    for (const v of ["off", "legend", "inside", "both"] as const) {
      expect(resolvePercentVisibility(v)).toBe(v);
    }
  });

  // Metabase's `getDefaultPercentVisibility` is `() => "legend"` — a constant with no ring count
  // and no chart in it. The ring count that used to be a parameter here is gone with the branch.
  it("defaults to the legend", () => {
    expect(resolvePercentVisibility(undefined)).toBe("legend");
  });

  it("degrades an unknown stored value to off, not to the default", () => {
    expect(resolvePercentVisibility("legend ")).toBe("off");
    expect(resolvePercentVisibility("")).toBe("off");
    expect(resolvePercentVisibility("true")).toBe("off");
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
    // C2/C3 need a decimal (31.0%/19.0%), so the chart's shared precision pads C1's own round
    // number to match: "50.0%", not the shorter "50%".
    const tooltip = opt.tooltip.formatter({ treePathInfo: [{ name: "" }, { name: "S1" }, { name: "C1" }] });
    expect(tooltip).toContain("50.0%");
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
    const withThree = buildPieOption(cols2, rows2, { dimensions: ["segment", "channel"], metric: "m", decimals: 3 }) as any;
    // Unset asks the data — no third of this chart needs a decimal at two significant digits.
    expect(ring(withDefault, 1).find((d: any) => d.name === "C1").slicePercent).toBe(33);
    expect(ring(withThree, 1).find((d: any) => d.name === "C1").slicePercent).toBeCloseTo(33.333, 9);
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

  // The DIAMETER is untouched (0.70 of the shorter side). What changed is the centre: this layout
  // used to lift the circle 2% "off the row beneath it", and there is no row beneath it inside the
  // canvas any more — see pieGeometry, and the hole tests below for what the lift cost.
  it("the bottom layout's diameter is untouched, and the circle is now dead centre", () => {
    const opt = buildPieOption(cols, rows, { ...base, legendPlacement: "bottom" }, undefined, narrowTall) as any;
    expect(outerRadiusPx(opt) * 2).toBeCloseTo(0.7 * 360, 6);
    expect(opt.series[0].center).toEqual(["50%", "50%"]);
  });

  it("an unstated placement is the bottom one — today's geometry, unchanged", () => {
    const opt = buildPieOption(cols, rows, base, undefined, wideShort) as any;
    expect(outerRadiusPx(opt) * 2).toBeCloseTo(0.7 * 661, 6);
    expect(opt.series[0].center).toEqual(["50%", "50%"]);
  });

  // No legend, no column to make room for: a chart with `showLegend: false` keeps exactly the
  // geometry it has today even if something upstream still hands it the side placement.
  it("a hidden legend reserves nothing", () => {
    const opt = buildPieOption(
      cols, rows, { ...base, showLegend: false, legendPlacement: "side" }, undefined, wideShort,
    ) as any;
    expect(outerRadiusPx(opt) * 2).toBeCloseTo(0.7 * 661, 6);
    expect(opt.series[0].center).toEqual(["50%", "50%"]);
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

  // THE HOLE AND THE RINGS SHARE ONE CENTRE, in every layout — the fault the live comparison
  // found (the total's block sat ~10px low and, on the deployed chart, 22px right of the rings'
  // middle). It is stated once here as a relationship rather than as two numbers that happen to
  // agree: the pie is centred on the canvas, and the hole is the literal `"center"` ECharts
  // resolves to that same point. Where it actually LANDS is measured on a rendered chart in
  // pie.hole.echarts.test.tsx — this test only pins that neither side can drift alone.
  it("the centre total is anchored to the same point as the rings, in either layout", () => {
    for (const legendPlacement of ["side", "bottom"] as const) {
      const opt = buildPieOption(
        cols, rows, { ...base, donut: true, showTotal: true, legendPlacement }, undefined, wideShort,
      ) as any;
      expect(opt.series[0].center).toEqual(["50%", "50%"]);
      expect(opt.graphic).toHaveLength(1);
      expect(opt.graphic[0]).toMatchObject({ type: "group", left: "center", top: "center" });
      // ...and nothing shifts it sideways by hand, which is what a re-introduced legend column
      // would need and is where the 22px went.
      expect(opt.graphic[0].x).toBeUndefined();
      expect(holeLines(opt.graphic).map((e: any) => e.left)).toEqual(["center", "center"]);
    }
  });

  // Metabase's own construction (option.ts's getTotalGraphicOption): the caption's box starts 26px
  // below the value's, which — at 22px and 14px — puts the two lines exactly 22px apart whatever
  // size the chart is. The pair egret drew before was pinned to 43% and 53% of the canvas HEIGHT,
  // so their spacing scaled with the chart: 50.9px apart on the host the comparison was made on.
  it("stacks the two lines Metabase's fixed 26px apart, not a fraction of the canvas", () => {
    const opt = buildPieOption(
      cols, rows, { ...base, donut: true, showTotal: true }, undefined, wideShort,
    ) as any;
    const [value, caption] = holeLines(opt.graphic);
    expect(value.top).toBe(0);
    expect(caption.top).toBe(26);
    // A taller chart moves neither of them: the offsets are pixels inside the group, not
    // percentages of anything.
    const taller = buildPieOption(
      cols, rows, { ...base, donut: true, showTotal: true }, undefined, { width: 830, height: 1200 },
    ) as any;
    expect(holeLines(taller.graphic).map((e: any) => e.top)).toEqual([0, 26]);
  });

  // Metabase's `top: labelText ? 0 : 8`. The group is centred by its own bounding box, so a value
  // left on its own (the too-narrow escape hatch, and a hover readout that lost its name) has to
  // be nudged down or the box still reserves the caption's room and the lone line rides high.
  it("nudges a caption-less value to Metabase's 8px so it centres by itself", () => {
    const tiny = { width: 30, height: 30 }; // the padding floor leaves a hole ~2px across
    const opt = buildPieOption(cols, rows, { ...base, donut: true, showTotal: true }, undefined, tiny) as any;
    const [value, caption] = holeLines(opt.graphic);
    expect(caption.style.text).toBe("");
    expect(value.top).toBe(8);
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
  // chart no longer draws. Keying the colour on the NAME makes that structural rather than careful:
  // "EU" is the same ink whether or not "US" is showing beside it.
  it("keeps each surviving slice's colour rather than re-dealing the palette", () => {
    const opt = buildPieOption(cols2, rows2, { ...cfg, toggledOffSlices: ["US"] }) as any;
    // Two rings, so ring 0 draws each family's dark variant — EU keeps the dark variant of the
    // colour ITS name hashes to, rather than sliding into the one US vacated.
    const euDark = chartShade(colorsForKeys(["US", "EU"]).EU);
    expect(ring(opt, 0)[0].itemStyle.color).toBe(euDark);
    expect(buildPieLegend(cols2, rows2, cfg).find((e) => e.name === "EU")!.color).toBe(euDark);
  });

  // Also matching the ECharts legend: neither the hole's total nor the legend's own percentages
  // re-based on what was left showing.
  it("does not re-base the centre total or the legend percentages", () => {
    const opt = buildPieOption(cols2, rows2, { ...cfg, showTotal: true, toggledOffSlices: ["US"] }) as any;
    expect(holeTexts(opt.graphic)).toEqual(["50", "Total"]);
    expect(buildPieLegend(cols2, rows2, cfg).map((e) => e.percent)).toEqual(["80%", "20%"]);
  });

  it("nothing toggled off leaves the option byte-for-byte as it was", () => {
    const plain = buildPieOption(cols2, rows2, cfg) as any;
    const empty = buildPieOption(cols2, rows2, { ...cfg, toggledOffSlices: [] }) as any;
    expect(ring(empty, 0).map((d: any) => d.name)).toEqual(ring(plain, 0).map((d: any) => d.name));
    expect(ring(empty, 0).map((d: any) => d.itemStyle.color)).toEqual(
      ring(plain, 0).map((d: any) => d.itemStyle.color),
    );
    expect(ring(plain, 0).map((d: any) => d.itemStyle.color)).toEqual(
      ["US", "EU"].map((r) => chartShade(colorsForKeys(["US", "EU"])[r])),
    );
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
  const text = (g: ReturnType<typeof buildPieHoleGraphic>) =>
    (g?.[0]?.children ?? []).map((e) => e.style.text);
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

  // The group and both its lines carry stable ids, which is what lets React re-state JUST the
  // hole as a merge-mode partial option instead of re-applying (and so rebuilding) the whole
  // chart. The group needs one as much as its children do: ECharts matches a merged `graphic`
  // element by id, and an id-less group would be a second one appended over the first.
  it("gives the group and each line a stable id so it can be re-stated on its own", () => {
    const rest = buildPieHoleGraphic(hole(), cfg, null)!;
    const hovered = buildPieHoleGraphic(hole(), cfg, { name: "US", value: 40, fromLegend: false })!;
    expect(rest.map((e) => e.id)).toEqual(["pie-total"]);
    expect(rest[0].children.map((e) => e.id)).toEqual(["pie-total-value", "pie-total-label"]);
    expect(hovered.map((e) => e.id)).toEqual(rest.map((e) => e.id));
    expect(hovered[0].children.map((e) => e.id)).toEqual(rest[0].children.map((e) => e.id));
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
      const [value] = buildPieHoleGraphic(model, cfg, null, size, measure)![0].children;
      expect(value.style.text).toBe("1,234,567,…");
      expect(value.style.text.length * 10).toBeLessThanOrEqual(112);
    });

    it("keeps a value that fits untouched", () => {
      const [value, label] = buildPieHoleGraphic(hole(), cfg, null, size, measure)![0].children;
      expect(value.style.text).toBe("50");
      expect(label.style.text).toBe("Total");
      expect(value.style.fontSize).toBe(22);
    });

    // Degenerate: at 30x30 the 12px-a-side padding floor leaves a 6px circle, so the hole is
    // 2.4px across — narrower than this stub's own ellipsis. Metabase's escape hatch.
    const tiny = { width: 30, height: 30 };

    it("hides the caption and shrinks the value when the hole is too narrow for both", () => {
      const [value, label] = buildPieHoleGraphic(hole(), cfg, null, tiny, measure)![0].children;
      expect(value.style.text).toBe("…"); // the total is the headline number, so it survives alone
      expect(label.style.text).toBe("");
      expect(value.style.fontSize).toBe(17); // Metabase's valueFontSizeSm
    });

    // A hover readout is transient — showing a name with no value (or the reverse) says less than
    // showing nothing, so it goes entirely. Metabase makes the same split.
    it("drops a hover readout entirely rather than showing half of it", () => {
      const g = buildPieHoleGraphic(hole(), cfg, { name: "US", value: 40, fromLegend: false }, tiny, measure)!;
      expect(g[0].children.map((e) => e.style.text)).toEqual(["", ""]);
    });

    // The same "don't guess" rule the slice labels follow: an unmeasured container must not become
    // a guessed-small one, or a perfectly readable total gets ellipsised against a number nobody
    // measured.
    it("never truncates when no real container size is known", () => {
      const model = { ...hole(), totalText: "1,234,567,890,123" };
      const [value] = buildPieHoleGraphic(model, cfg, null, undefined, measure)![0].children;
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
    // One decimal, not two: the chart's own percent precision is computed from the shares it
    // DRAWS (Big at 96%, Other at 3.7%, Main at 100%), and Other's breakdown — which is what this
    // table is — is deliberately not part of that set. See computeMaxPercentDecimals. Ch3's own
    // share happens to round to a whole number, but the shared decimal still pads it: "27.0%",
    // not the shorter "27%" — the whole reason the precision is computed as a maximum at all.
    expect(rowsOf(tip(["Other"], nested, cols2, rows2))).toEqual([
      ["Ch1", "15", "40.5%"],
      ["Ch2", "12", "32.4%"],
      ["Ch3", "10", "27.0%"],
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

// A sub-0.5% ring-0 slice is DRAWN at 0.5% so there is something to point at — Metabase's
// OTHER_SLICE_MIN_PERCENTAGE. What that must not do is change a single number the chart reports.
// The drawn side of this (that the wedge really does come out wider) is measured against real
// ECharts in pie.tinyslice.echarts.test.tsx; this block is about the figures.
describe("the tiny-slice floor changes the drawing, not the numbers", () => {
  const cols2 = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
  // B is 1 in 1001 — 0.0999%, a fifth of the floor. sliceThreshold 0 keeps it out of an "Other"
  // bucket, so the slice under test is the tiny one itself.
  const rows2 = [["A", 1000], ["B", 1]];
  const cfg = { dimensions: ["d"], metric: "m", sliceThreshold: 0 };

  it("draws the floored slice bigger while it keeps its own value", () => {
    const opt = buildPieOption(cols2, rows2, cfg) as any;
    const [a, b] = ring(opt, 0);
    expect(b.rawValue).toBe(1);
    expect(b.value).toBeCloseTo(1001 * 0.005, 9); // the total's 0.5%, which is what gets drawn
    // Untouched slices state one number twice, so nothing downstream has to know which is which.
    expect([a.value, a.rawValue]).toEqual([1000, 1000]);
  });

  it("reports the slice's true share, not its drawn one", () => {
    const [, b] = ring(buildPieOption(cols2, rows2, cfg) as any, 0);
    expect(b.slicePercent).toBe(0.1); // 1/1001, to the default 2 decimals — not 0.5
  });

  it("keeps the label on the true share", () => {
    // `showLabels: true` because this is a ONE-RING pie, whose arcs are bare by default
    // (`resolveShowLabels`) — the name has to be asked for before there is a label to check the
    // share against. Nothing here is about the default.
    const opt = buildPieOption(cols2, rows2, { ...cfg, percentVisibility: "inside", showLabels: true }) as any;
    expect(ring(opt, 0)[1].label.formatter()).toBe("B: 0.1%");
  });

  it("keeps the legend's percent and value true", () => {
    const [, b] = buildPieLegend(cols2, rows2, { ...cfg, percentVisibility: "legend" });
    // 1/1001 — its own share to four decimals (which is what three significant digits costs at
    // this size, see computeMaxPercentDecimals), and nowhere near the 0.5% it is DRAWN at.
    expect(b).toMatchObject({ name: "B", value: 1, percent: "0.0999%" });
  });

  it("keeps the tooltip's value and total true", () => {
    const html = (buildPieOption(cols2, rows2, cfg) as any).tooltip.formatter({
      treePathInfo: [{ name: "" }, { name: "B" }],
    }) as string;
    const cells = [...html.matchAll(/<tr[^>]*>(.*?)<\/tr>/g)].map((m) =>
      [...m[1].matchAll(/<td[^>]*>(.*?)<\/td>/g)].map((c) => c[1].replace(/<[^>]*>/g, "")));
    expect(cells).toEqual([
      ["A", "1,000", "99.9%"],
      ["B", "1", "0.1%"],
      ["Total", "1,001", "100%"],
    ]);
  });

  it("keeps the centre total true", () => {
    const hole = buildPieHole(cols2, rows2, { ...cfg, donut: true, showTotal: true });
    expect(holeTexts(buildPieHoleGraphic(hole, { ...cfg, donut: true, showTotal: true }, null)))
      .toEqual(["1,001", "Total"]);
  });
});

/**
 * HOW MANY DECIMALS A PERCENTAGE GETS, when nobody has said.
 *
 * egret hard-coded 2, so a chart of round quarters read `25%` (the trailing zeros are stripped)
 * but one with a 23.88% slice read `23.88%` in the legend AND on the slice, where Metabase's
 * slice says `23.9%`. Metabase computes the count from the data — `computeMaxDecimalsForValues`,
 * over the percentages actually present — with a different significant-digit budget for each
 * place the number appears: 3 for the legend, 2 for a label drawn on a slice.
 */
describe("computeMaxPercentDecimals — Metabase's computeMaxDecimalsForValues", () => {
  // It is not "the shortest that still looks distinct". It formats each value as a percent under
  // an Intl `maximumSignificantDigits` budget and counts the fraction digits that survive, taking
  // the largest count over the set. Nothing compares one value to another.
  it("counts the fraction digits an Intl percent format keeps under the digit budget", () => {
    expect(computeMaxPercentDecimals([0.25], 3)).toBe(0);      // "25%"
    expect(computeMaxPercentDecimals([0.2388], 3)).toBe(1);    // "23.9%"
    expect(computeMaxPercentDecimals([0.05123], 3)).toBe(2);   // "5.12%"
    expect(computeMaxPercentDecimals([0.005123], 3)).toBe(3);  // "0.512%"
  });

  it("gives a smaller budget fewer decimals for the same value", () => {
    expect(computeMaxPercentDecimals([0.2388], 2)).toBe(0);    // "24%"
    expect(computeMaxPercentDecimals([0.05123], 2)).toBe(1);   // "5.1%"
  });

  it("takes the MAXIMUM over the set, so one small slice lengthens every number", () => {
    expect(computeMaxPercentDecimals([0.2388, 0.0512, 0.71], 3)).toBe(2);
    expect(computeMaxPercentDecimals([0.2388, 0.71], 3)).toBe(1);
  });

  it("is 0 for no values at all", () => {
    expect(computeMaxPercentDecimals([], 3)).toBe(0);
  });
});

describe("a pie picks its percent decimals from the data", () => {
  const cols2 = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
  // Shares 23.88% / 5.12% / 71%. Under the legend's 3-significant-digit budget the 5.12% slice
  // needs two decimals, so every legend number gets two; under a slice label's 2-digit budget it
  // needs only one, so the labels stay a character shorter. That is exactly the split the live
  // Metabase shows: `23.88%` in the legend, `23.9%` on the slice.
  const rows2 = [["A", 2388], ["B", 512], ["C", 7100]];
  // `showLabels: true` throughout: this is a one-ring pie, whose arcs are bare by default
  // (`resolveShowLabels`), and every slice-label case below is about the PRECISION of a label
  // that has to exist first. The legend cases ignore it.
  const cfg = { dimensions: ["d"], metric: "m", showLabels: true };

  it("gives the legend three significant digits' worth", () => {
    const entries = buildPieLegend(cols2, rows2, { ...cfg, percentVisibility: "legend" });
    // C's own share is an exact 71%, but the shared precision (driven by B's 5.12%) is 2 decimals
    // for every entry — "71.00%", not the shorter "71%" a naive formatter would strip it to.
    expect(entries.map((e) => e.percent)).toEqual(["71.00%", "23.88%", "5.12%"]);
  });

  it("gives a slice label two, so it stays shorter than the legend's", () => {
    const opt = buildPieOption(cols2, rows2, { ...cfg, percentVisibility: "inside" }) as any;
    // Same padding rule at the slice's own (1-decimal) precision: "71.0%", not "71%".
    expect(ring(opt, 0).map((d: any) => d.label.formatter())).toEqual(["C: 71.0%", "A: 23.9%", "B: 5.1%"]);
  });

  it("drops decimals entirely when the data doesn't need them", () => {
    const round = [["A", 25], ["B", 75]];
    expect(buildPieLegend(cols2, round, { ...cfg, percentVisibility: "legend" }).map((e) => e.percent))
      .toEqual(["75%", "25%"]);
    const opt = buildPieOption(cols2, round, { ...cfg, percentVisibility: "inside" }) as any;
    expect(ring(opt, 0).map((d: any) => d.slicePercent)).toEqual([75, 25]);
  });

  it("still does what an explicit `decimals` says, in both places", () => {
    const four = { ...cfg, percentVisibility: "both" as const, decimals: 4 };
    // A's share is an exact 23.88% — 4 decimals pads it to "23.8800%", not the shorter "23.88%".
    expect(buildPieLegend(cols2, rows2, four)[1].percent).toBe("23.8800%");
    expect((ring(buildPieOption(cols2, rows2, four) as any, 0))[1].label.formatter()).toBe("A: 23.8800%");
    const none = { ...cfg, percentVisibility: "both" as const, decimals: 0 };
    // decimals: 0 must give "24%", never "24.%" — no stray separator with nothing after it.
    expect(buildPieLegend(cols2, rows2, none)[1].percent).toBe("24%");
    expect((ring(buildPieOption(cols2, rows2, none) as any, 0))[1].label.formatter()).toBe("A: 24%");
  });

  // Metabase's chart-location set is every slice at every depth (getAllSlicePercentages), not just
  // the ring the legend lists — an outer ring's shares are drawn too, and drawn at the same
  // precision.
  it("counts an outer ring's shares too", () => {
    const cols3 = [{ name: "d", type: "varchar" }, { name: "e", type: "varchar" }, { name: "m", type: "double" }];
    // Ring 0 is 80/20 — no decimals needed. A's child `y` is 0.125% of it, which is what pushes
    // every label to two.
    const rows3 = [["A", "x", 799], ["A", "y", 1], ["B", "z", 200]];
    const opt = buildPieOption(cols3, rows3,
      { dimensions: ["d", "e"], metric: "m", sliceThreshold: 0, percentVisibility: "inside" }) as any;
    // z is an exact 100% of its own parent B, but the chart's shared 2-decimal precision (driven
    // by y's 0.13%) pads it too — "100.00%", not the shorter "100%".
    expect(ring(opt, 1).map((d: any) => d.label.formatter())).toEqual(["x: 99.88%", "y: 0.13%", "z: 100.00%"]);
  });

  // ...but it stops at an "Other" bucket, exactly as Metabase's own walk does: it pushes Other's
  // own share and does not descend into the slices folded inside it.
  it("does not descend into an Other bucket's children", () => {
    const cols3 = [{ name: "d", type: "varchar" }, { name: "e", type: "varchar" }, { name: "m", type: "double" }];
    // A is 97%; B and C are 1.5% each, so they fold into one Other worth 3%. Nothing outside that
    // bucket needs a decimal — and `b1`, at 5.333% of Other, would demand one if it counted.
    const rows3 = [["A", "a1", 9700], ["B", "b1", 16], ["B", "b2", 134], ["C", "c1", 150]];
    const opt = buildPieOption(cols3, rows3,
      { dimensions: ["d", "e"], metric: "m", percentVisibility: "inside" }) as any;
    const b1 = ring(opt, 1).find((d: any) => d.name === "b1");
    expect(b1.label.formatter()).toBe("b1: 5%");
  });
});

/**
 * `computeMaxPercentDecimals` takes the MAXIMUM decimal count across a chart's shares specifically
 * so a reader compares like with like down a column (see that function's own header comment) — but
 * that promise only holds if every value actually PRINTS at that count. `parseFloat(p.toFixed(n))`
 * used to strip a value's own trailing zeros, so a chart computed to need 1 decimal (because two of
 * its slices do) would still show a third, round one shorter than its neighbours.
 *
 * Live comparison against Metabase on a real 4-slice pie: Metabase's legend read
 * `23.88% · 26.54% · 15.85% · 33.73%`; egret's read `36.4% · 34.6% · 29%` — that last entry should
 * have been `29.0%`, one decimal like its neighbours, not the shorter round number
 * `parseFloat` was silently producing.
 */
describe("a percentage prints at its computed precision even when it rounds to a trailing zero", () => {
  const cols2 = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
  // A 3-slice chart shaped exactly like the live bug: two slices (36.40%, 34.63%) whose legend
  // precision (3 significant digits) needs 1 decimal, and a third (28.97%) that ROUNDS to an exact
  // 29 at that same precision — the case a naive `parseFloat` strips.
  const rows2 = [["A", 3640], ["B", 3463], ["C", 2897]];
  // `showLabels: true` for the same reason as the describe above: a one-ring pie's arcs are bare
  // by default (`resolveShowLabels`), and the slice-label case here is about the DIGITS a label
  // prints, not about whether it is drawn.
  const cfg = { dimensions: ["d"], metric: "m", showLabels: true };

  it("legend: a value that rounds to a whole number still shows the shared decimal", () => {
    const entries = buildPieLegend(cols2, rows2, { ...cfg, percentVisibility: "legend" });
    expect(entries.map((e) => e.percent)).toEqual(["36.4%", "34.6%", "29.0%"]);
  });

  it("decimals: 0 never leaves a stray separator behind", () => {
    // The guard `pctFormatter` has to satisfy at the OTHER end of the range: `(29).toFixed(0)` is
    // "29", with no decimal point at all, so this must read "29%" and never "29.%".
    const entries = buildPieLegend(cols2, rows2, { ...cfg, percentVisibility: "legend", decimals: 0 });
    expect(entries.map((e) => e.percent)).toEqual(["36%", "35%", "29%"]);
    for (const e of entries) expect(e.percent).not.toMatch(/\.%$/);
  });

  it("a 2-decimal value that rounds evenly still pads: 23.88% stays 23.88%, not 23.9%", () => {
    // The flip side of the same guard: padding must never itself round away a real digit. cols2
    // fixture with 2 significant sig-fig decimals already covered elsewhere (rows2 in the
    // "picks its percent decimals" describe above); this pins the boundary case directly.
    const exact = buildPieLegend(cols2, [["A", 2388], ["B", 512], ["C", 7100]], { ...cfg, percentVisibility: "legend" });
    expect(exact.map((e) => e.percent)).toEqual(["71.00%", "23.88%", "5.12%"]);
  });

  it("legend, slice label and tooltip agree on the same slice's percentage", () => {
    // An explicit `decimals` is the one config where the legend's own (3-sig-fig) budget and the
    // slice/tooltip's (2-sig-fig) budget both give way to the same authoritative number (see
    // `computeMaxPercentDecimals`'s callers: `cfg.decimals ?? computeMaxPercentDecimals(...)` in
    // both places) — so this is the case where all three sites must print IDENTICAL digits for
    // the same slice, not just consistently-padded ones at each site's own precision.
    const withDecimals = { ...cfg, decimals: 1 };
    const legendPct = buildPieLegend(cols2, rows2, { ...withDecimals, percentVisibility: "legend" })
      .find((e) => e.name === "C")!.percent;
    const opt = buildPieOption(cols2, rows2, { ...withDecimals, percentVisibility: "inside" }) as any;
    const tooltipHtml = opt.tooltip.formatter({ treePathInfo: [{ name: "" }, { name: "C" }] }) as string;
    expect(legendPct).toBe("29.0%");
    expect(ring(opt, 0).find((d: any) => d.name === "C").label.formatter()).toBe("C: 29.0%");
    expect(tooltipHtml).toContain("29.0%");
  });
});

// A pie can only show a share of something, so a column of zeros leaves ring 0 empty even though
// the query returned rows — and an empty ring 0 used to reach the canvas as an empty sunburst,
// i.e. nothing at all. ChartRenderer asks this before it draws.
describe("pieHasSlices", () => {
  const cols2 = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
  const cfg = { dimensions: ["d"], metric: "m" };

  it("is false when every value is zero, however many rows there are", () => {
    expect(pieHasSlices(cols2, [["A", 0], ["B", 0], ["C", 0]], cfg)).toBe(false);
    // ...and the chart really would have had nothing to draw.
    expect((buildPieOption(cols2, [["A", 0], ["B", 0]], cfg) as any).series[0].data).toEqual([]);
  });

  it("is false for values a pie cannot take a share of at all", () => {
    expect(pieHasSlices(cols2, [["A", null], ["B", "n/a"]], cfg)).toBe(false);
  });

  it("is true as soon as one slice survives", () => {
    expect(pieHasSlices(cols2, [["A", 0], ["B", 7]], cfg)).toBe(true);
  });

  // An all-negative column is charted on its magnitudes (buildSlices' own rule), so it is NOT the
  // empty case — only a mixed-sign one loses its negatives.
  it("is true for an all-negative column, which the pie draws by magnitude", () => {
    expect(pieHasSlices(cols2, [["A", -3], ["B", -7]], cfg)).toBe(true);
  });

  it("is false with no dimension or metric to build a ring from", () => {
    expect(pieHasSlices(cols2, [["A", 5]], {})).toBe(false);
  });
});

// ---------------------------------------------------------------------------------------------
// The structured alternative to the arcs (buildPieBreakdown).
//
// A sunburst past two levels encodes meaning in COLOUR (family + lightened shade) and POSITION
// (nested inside a specific parent wedge). Neither survives a text summary, which is what the
// chart's `aria` description is — so a screen-reader user got ring 0's flat list and nothing else.
// This is the table that carries the rest of it: every slice, under its parent, with its share OF
// that parent.
describe("buildPieBreakdown", () => {
  // The table and the legend sit on the same page, so a root slice must not read as a DIFFERENT
  // number in each. Measured live at the in-arc budget: `APAC 34.6%` in the legend, `APAC 35%` in
  // the table — they rounded opposite ways.
  //
  // Character equality is NOT the invariant, and asserting it was this test's first mistake. The
  // two precisions are computed over different value sets, exactly as Metabase's legend (ring 0,
  // 3 significant digits) and slice labels (every depth, 2) are: the table carries deep rows whose
  // tiny shares demand more decimals, and Metabase's rule then gives that precision to the whole
  // column so a reader compares like with like. So the table's figure is a REFINEMENT of the
  // legend's — round it back to the legend's precision and you must get the legend's string.
  it("states a root slice's share as a refinement of the legend's, never a different number", () => {
    const cols = [
      { name: "r", type: "varchar" }, { name: "p", type: "varchar" }, { name: "m", type: "double" },
    ];
    // 36.44 / 34.59 / 28.97 of the whole — the live numbers that exposed the split. NO tiny slice
    // anywhere, deliberately: a sub-percent share forces extra decimals at BOTH budgets, which
    // makes them agree by accident and leaves this test unable to fail. (It did exactly that on
    // the first attempt — the mutation back to the in-arc budget survived.) With only sizeable
    // children, the in-arc budget prints whole numbers and the legend's prints one decimal, so
    // the two round apart and the refinement check has something to catch.
    const rows = [
      ["LATAM", "x", 1822], ["LATAM", "y", 1822],
      ["APAC", "x", 1730], ["APAC", "y", 1729],
      ["EMEA", "x", 1449], ["EMEA", "y", 1448],
    ];
    const cfg = { dimensions: ["r", "p"], metric: "m" };
    const legend = buildPieLegend(cols, rows, cfg);
    const table = buildPieBreakdown(cols, rows, cfg);

    for (const entry of legend) {
      const row = table.rows.find((r) => r.depth === 0 && r.name === entry.name);
      expect(row, `no table row for legend entry ${entry.name}`).toBeDefined();
      const legendDecimals = (entry.percent!.match(/\.(\d+)%/)?.[1] ?? "").length;
      const tableDecimals = (row!.share.match(/\.(\d+)%/)?.[1] ?? "").length;
      // The table is at least as precise...
      expect(tableDecimals).toBeGreaterThanOrEqual(legendDecimals);
      // ...and agrees once brought back to the legend's precision. This is what fails when the
      // two round opposite ways, which is the defect measured live.
      const backToLegend = `${parseFloat(row!.share).toFixed(legendDecimals)}%`;
      expect(backToLegend).toBe(entry.percent);
    }
  });

  const cols3 = [
    { name: "region", type: "varchar" }, { name: "product", type: "varchar" },
    { name: "channel", type: "varchar" }, { name: "m", type: "double" },
  ];
  // Each region totals 1020, so ring 0 never folds. Within each region the two 10-value products
  // are sub-threshold against their own parent's 1020, so each region gets its OWN Other(20) at
  // ring 1 — and each of those Others keeps its channels at ring 2.
  const rows3 = [
    ["R1", "P1", "x", 1000],
    ["R1", "a", "A1", 6], ["R1", "a", "A2", 4],
    ["R1", "b", "B1", 10],
    ["R2", "P2", "y", 1000],
    ["R2", "c", "C1", 10],
    ["R2", "d", "D1", 10],
  ];
  const cfg3 = { dimensions: ["region", "product", "channel"], metric: "m", decimals: 1 };

  /** What the CHART itself says this slice's share is — read back out of the tooltip's own HTML,
   * so the two can only agree by actually agreeing. `path` is root-to-slice, the same shape
   * ECharts hands the formatter as `treePathInfo` (minus its virtual root, which is prepended
   * here). Returns [value, share] as the tooltip prints them. */
  const tooltipCells = (opt: any, path: string[]): [string, string] => {
    const html: string = opt.tooltip.formatter({
      treePathInfo: [{ name: "" }, ...path.map((name) => ({ name }))],
    });
    const leaf = path[path.length - 1];
    const row = html.split("<tr").find((r) => r.includes(`>${leaf}</td>`))!;
    const cells = [...row.matchAll(/>([^<>]+)<\/td>/g)].map((m) => m[1]);
    return [cells[1], cells[2]];
  };

  // 1. Every slice, under its correct parent.
  it("exposes every slice of a three-ring chart, each under its own parent", () => {
    const { rows: out } = buildPieBreakdown(cols3, rows3, cfg3);
    expect(out.map((r) => [r.depth, r.parents, r.name])).toEqual([
      [0, [], "R1"],
      [1, ["R1"], "P1"],
      [2, ["R1", "P1"], "x"],
      [1, ["R1"], "Other"],
      [2, ["R1", "Other"], "B1"],
      [2, ["R1", "Other"], "A1"],
      [2, ["R1", "Other"], "A2"],
      [0, [], "R2"],
      [1, ["R2"], "P2"],
      [2, ["R2", "P2"], "y"],
      [1, ["R2"], "Other"],
      [2, ["R2", "Other"], "C1"],
      [2, ["R2", "Other"], "D1"],
    ]);
  });

  it("names each ring by its own dimension, and the value column by the metric", () => {
    const b = buildPieBreakdown(cols3, rows3, {
      ...cfg3, columnSettings: { region: { column_title: "Region" }, m: { column_title: "Revenue" } },
    });
    expect(b.levels).toEqual(["Region", "product", "channel"]);
    expect(b.metric).toBe("Revenue");
  });

  // 2. The share is of the PARENT, and it is the same number the chart's own tooltip prints.
  it("states each slice's share of its parent — the number the tooltip shows for that slice", () => {
    const b = buildPieBreakdown(cols3, rows3, cfg3);
    const share = (name: string, parents: string[]) =>
      b.rows.find((r) => r.name === name && r.parents.join("|") === parents.join("|"))!.share;
    // Ring 0's parent is the whole chart; deeper, it is the wedge the slice sits inside — so B1
    // is half of the Other bucket it is nested in, not 0.5% of the chart.
    expect(share("R1", [])).toBe("50.0%");
    expect(share("P1", ["R1"])).toBe("98.0%");
    expect(share("Other", ["R1"])).toBe("2.0%");
    expect(share("B1", ["R1", "Other"])).toBe("50.0%");
    expect(share("A1", ["R1", "Other"])).toBe("30.0%");

    const opt = buildPieOption(cols3, rows3, cfg3) as any;
    for (const path of [["R1"], ["R1", "P1"], ["R1", "Other", "B1"], ["R2", "P2", "y"]]) {
      const row = b.rows.find(
        (r) => r.name === path[path.length - 1] && r.parents.join("|") === path.slice(0, -1).join("|"),
      )!;
      expect(row.share).toBe(tooltipCells(opt, path)[1]);
    }
  });

  // RE-BASELINED. This used to assert the table printed shares at the SLICE budget, character for
  // character with the tooltip. It no longer does, deliberately: the table sits beside the legend,
  // and at the slice budget the same root slice read `34.6%` in one and `35%` in the other. The
  // table now takes the legend's budget — so it is at least as precise as the tooltip, and equal
  // whenever the two budgets happen to land on the same number of decimals.
  it("is never less precise than the tooltip for the same slice", () => {
    const auto = { dimensions: ["region", "product", "channel"], metric: "m" };
    const b = buildPieBreakdown(cols3, rows3, auto);
    const opt = buildPieOption(cols3, rows3, auto) as any;
    const fromTable = b.rows.find((r) => r.name === "P1")!.share;
    const fromTooltip = tooltipCells(opt, ["R1", "P1"])[1];
    const decimals = (v: string) => (v.match(/\.(\d+)%/)?.[1] ?? "").length;
    expect(decimals(fromTable)).toBeGreaterThanOrEqual(decimals(fromTooltip));
    // Same underlying number, whatever the precision.
    expect(parseFloat(fromTable)).toBeCloseTo(parseFloat(fromTooltip), 1);
  });

  // 3. In step with what is actually drawn.
  it("leaves out a hidden slice and everything nested under it", () => {
    const b = buildPieBreakdown(cols3, rows3, { ...cfg3, hiddenSlices: ["R2"] });
    expect(b.rows.some((r) => r.name === "R2" || r.parents.includes("R2"))).toBe(false);
    expect(b.rows.some((r) => r.name === "P2" || r.name === "y")).toBe(false);
    // ...and the rest re-bases, exactly as the chart does: R1 is now the whole chart.
    expect(b.rows.find((r) => r.name === "R1")!.share).toBe("100.0%");
  });

  it("leaves out a slice the reader has toggled off in the legend", () => {
    const b = buildPieBreakdown(cols3, rows3, { ...cfg3, toggledOffSlices: ["R1"] });
    expect(b.rows.some((r) => r.name === "R1" || r.parents.includes("R1"))).toBe(false);
    expect(b.rows.some((r) => r.name === "R2")).toBe(true);
  });

  it("uses a renamed slice's new name, wherever it appears", () => {
    const b = buildPieBreakdown(cols3, rows3, { ...cfg3, sliceNames: { R1: "Region One" } });
    expect(b.rows.some((r) => r.name === "R1")).toBe(false);
    expect(b.rows.find((r) => r.depth === 0)!.name).toBe("Region One");
    expect(b.rows.find((r) => r.name === "P1")!.parents).toEqual(["Region One"]);
  });

  it("shows each parent's own folded Other as one entry under that parent", () => {
    const b = buildPieBreakdown(cols3, rows3, cfg3);
    const others = b.rows.filter((r) => r.name === "Other");
    expect(others.map((r) => r.parents)).toEqual([["R1"], ["R2"]]);
    // The folded-away products themselves are NOT drawn, so they are not listed either — what is
    // listed under Other is what the chart nests inside it.
    expect(b.rows.some((r) => ["a", "b", "c", "d"].includes(r.name))).toBe(false);
  });

  // 6. Values read the same here as everywhere else the chart states them.
  it("formats a value with the chart's own metric formatter", () => {
    const formatted = { ...cfg3, columnSettings: { m: { prefix: "$", decimals: 0 } } };
    const b = buildPieBreakdown(cols3, rows3, formatted);
    const opt = buildPieOption(cols3, rows3, formatted) as any;
    const row = b.rows.find((r) => r.name === "x")!;
    expect(row.value).toBe(tooltipCells(opt, ["R1", "P1", "x"])[0]);
    expect(row.value).toContain("$");
    // ...and it is the slice's TRUE value, never the inflated one a tiny wedge is drawn at.
    expect(b.rows.find((r) => r.name === "A2")!.value).toBe("$4");
  });

  // The same caveat the chart's aria description carries: a ring whose own total diverges from the
  // chart's, because the zero/negative filter runs independently per ring.
  it("carries the ring-total divergence note", () => {
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "product", type: "varchar" }, { name: "m", type: "double" },
    ];
    // R1 nets to 20 at ring 0; at ring 1 the -10 is dropped outright, so that ring totals 30.
    const b = buildPieBreakdown(cols2, [["R1", "P1", 30], ["R1", "P2", -10]], {
      dimensions: ["region", "product"], metric: "m",
    });
    expect(b.notes).toHaveLength(1);
    expect(b.notes[0]).toContain("product");
    expect(b.notes[0]).toContain("30");
  });

  // 5. A single-ring pie is a flat list, which the chart's `aria` description already reads out in
  // full. A second, redundant description is not an improvement for a screen-reader user — it is
  // the same list twice — so the breakdown does not engage at all below two rings.
  it("produces nothing for a single-ring pie, whose aria description is already the whole story", () => {
    expect(buildPieBreakdown(cols, rows, { dimensions: ["region"], metric: "rev" }).rows).toEqual([]);
    // ...and the chart's own description is still there, listing every slice.
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
    expect(opt.aria.label.description).toContain("US");
    expect(opt.aria.label.description).toContain("EU");
  });

  it("produces nothing when there is no ring to break down at all", () => {
    expect(buildPieBreakdown(cols, rows, {}).rows).toEqual([]);
  });
});

describe("pieSliceDrillPath", () => {
  const SEP = "\u241F";
  const OTHER = "\u0000__other__\u0000";

  it("returns the whole ancestor path in raw values", () => {
    expect(pieSliceDrillPath(["EU", "Widgets", "Direct"].join(SEP))).toEqual(["EU", "Widgets", "Direct"]);
    expect(pieSliceDrillPath("EU")).toEqual(["EU"]);
  });

  // "Other" is a bucket the chart invented. No single column value stands behind it, so a filter
  // built from it matches nothing - an empty result presented as the slice's contents.
  it("refuses an Other bucket, at any ring", () => {
    expect(pieSliceDrillPath(OTHER)).toBeNull();
    expect(pieSliceDrillPath([OTHER, "EU"].join(SEP))).toBeNull();
    expect(pieSliceDrillPath(["EU", OTHER].join(SEP))).toBeNull();
  });

  it("refuses a datum with no key rather than throwing", () => {
    expect(pieSliceDrillPath(undefined)).toBeNull();
  });

  // A real category literally named "Other" is a different string from the sentinel, so it still
  // drills - the collision the sentinel exists to avoid.
  it("still drills a real value that happens to be named Other", () => {
    expect(pieSliceDrillPath("Other")).toEqual(["Other"]);
  });
});

describe("slice labels honour the dimension's column format", () => {
  const COLS = [{ name: "order_date", type: "date" }, { name: "rev", type: "double" }];
  const ROWS = [["2026-01-01", 30], ["2026-02-01", 10]];

  // The pie was the one chart that ignored a dimension's format: a month breakout read
  // "2026-01-01" while the same column in a table or a pivot read the way its setting said.
  it("formats the slice name while leaving the key raw", () => {
    const opt = buildPieOption(COLS, ROWS, {
      dimensions: ["order_date"], metric: "rev",
      dimensionUnits: { order_date: "month" },
      columnSettings: { order_date: { date_style: "MMMM YYYY" } },
    }) as any;
    const data = opt.series[0].data;
    expect(data.map((d: any) => d.name)).toEqual(["January 2026", "February 2026"]);
    // The drill, the colour overrides and the ordering all address the raw value, so it must
    // survive the rename untouched.
    expect(data.map((d: any) => d.key)).toEqual(["2026-01-01", "2026-02-01"]);
  });

  it("leaves an extraction unit's label alone rather than re-formatting a finished word", () => {
    const opt = buildPieOption(
      [{ name: "dow", type: "date" }, { name: "rev", type: "double" }],
      [[3, 30], [5, 10]],
      {
        dimensions: ["dow"], metric: "rev",
        dimensionUnits: { dow: "day_of_week" },
        columnSettings: { dow: { date_style: "MMMM YYYY" } },
      },
    ) as any;
    expect(opt.series[0].data.map((d: any) => d.name)).toEqual(["Wed", "Fri"]);
  });

  it("falls back to the raw value when the column has no format", () => {
    const opt = buildPieOption(COLS, ROWS, {
      dimensions: ["order_date"], metric: "rev", dimensionUnits: { order_date: "month" },
    }) as any;
    expect(opt.series[0].data.map((d: any) => d.name)).toEqual(["2026-01-01", "2026-02-01"]);
  });
});

/**
 * THE RAW/FORMATTED SEAM — the settings panel and the chart must name the same slice.
 *
 * `PieDisplayControl` (viz/simple.tsx) hands the panel `String(row[dimIdx])`, i.e. the RAW
 * dimension values, and every control writes those raw strings into `hiddenSlices`,
 * `sliceOrder`, `sliceNames` and `sliceColors`. A column format then makes the chart DRAW
 * something else, so a curation keyed on the drawn label finds nothing at all: reproduced in
 * the UI on a pie over `order_month` with Date style `January 2024`, where the panel listed
 * `2026-12-01` and the chart drew `December 2026`, and toggling "Show 2026-12-01" off changed
 * nothing. Metabase keys the same four on the raw value too — `getPieRows`/`getPieColors` build
 * every row around `getKeyFromDimensionValue(dimensionValue)` and carry `name: formatter(value)`
 * alongside it, and its own hidden check is `!hiddenSlices.includes(row.key)`.
 *
 * THE FIXTURE IS ONLY ABLE TO SEE THIS BECAUSE ITS TWO VOCABULARIES DIFFER. A format whose
 * output happens to equal its input (Date style `2024-01-31` on an ISO date, say) makes every
 * assertion below pass against the broken code as well — which is exactly the control that
 * identified the bug in the UI. `the fixture speaks two vocabularies` states that premise so it
 * cannot rot away silently.
 */
describe("a curated pie finds the slice the panel named, not the label it drew", () => {
  const COLS = [{ name: "order_month", type: "date" }, { name: "rev", type: "double" }];
  /** What the panel is handed — `String(row[dimIdx])` for `dimensions[0]`. */
  const RAW = ["2026-12-01", "2026-11-01", "2026-10-01"];
  const LABELS = ["December 2026", "November 2026", "October 2026"];
  const ROWS: unknown[][] = [[RAW[0], 60], [RAW[1], 30], [RAW[2], 10]];
  const cfg = {
    dimensions: ["order_month"], metric: "rev",
    dimensionUnits: { order_month: "month" },
    columnSettings: { order_month: { date_style: "MMMM YYYY" } },
  };
  const build = (extra: Record<string, unknown> = {}) =>
    buildPieOption(COLS, ROWS, { ...cfg, ...extra }) as any;

  it("the fixture speaks two vocabularies — no raw value is its own label", () => {
    const data = ring(build(), 0);
    expect(data.map((d: any) => d.key)).toEqual(RAW);
    expect(data.map((d: any) => d.name)).toEqual(LABELS);
    for (const d of data) expect(d.name).not.toBe(d.key);
    // No slice is under the 2.5% fold threshold, so nothing here is an "Other" bucket in
    // disguise: 10/100 is the smallest, and after the hide below 10/40 is smaller still.
    expect(data.map((d: any) => d.value)).toEqual([60, 30, 10]);
  });

  it("hiddenSlices, named by the panel's raw value, hides that slice", () => {
    const data = ring(build({ hiddenSlices: [RAW[0]] }), 0);
    expect(data.map((d: any) => d.name)).toEqual(["November 2026", "October 2026"]);
    // ...and the remaining shares re-base as if December had never been queried.
    expect(data.reduce((t: number, d: any) => t + d.value, 0)).toBe(40);
  });

  it("sliceNames, named by the panel's raw value, renames that slice", () => {
    const data = ring(build({ sliceNames: { [RAW[1]]: "Black Friday" } }), 0);
    expect(data.map((d: any) => d.name)).toEqual(["December 2026", "Black Friday", "October 2026"]);
    // The rename is display-only: the key the drill and the curations address is untouched.
    expect(data.map((d: any) => d.key)).toEqual(RAW);
  });

  it("sliceOrder, named by the panel's raw value, reorders the ring", () => {
    // Natural order is value-desc (Dec 60, Nov 30, Oct 10), so putting October first is a
    // change only an order override can make.
    const data = ring(build({ sliceOrder: [RAW[2]] }), 0);
    expect(data.map((d: any) => d.name)).toEqual(["October 2026", "December 2026", "November 2026"]);
  });

  it("sliceColors, named by the panel's raw value, paints that slice", () => {
    const data = ring(build({ sliceColors: { [RAW[2]]: "#123456" } }), 0);
    expect(data.find((d: any) => d.name === "October 2026").itemStyle.color).toBe("#123456");
  });

  /**
   * And the slices it did NOT pin take the palette as if keyed on the raw value — which is the
   * same question `PieDisplaySection`'s swatch asks (`colorsForKeys(rawSlices, sliceColors)`),
   * so the swatch beside "2026-12-01" is now the ink the chart puts in December's wedge. Both
   * assertions are needed: the override alone can be satisfied while the rest of the ring is
   * still hashed on its drawn label, and then every OTHER swatch in the panel is wrong.
   */
  it("an un-pinned slice takes the palette slot its raw value hashes to", () => {
    const plain = ring(build(), 0);
    expect(plain.map((d: any) => d.itemStyle.color)).toEqual(RAW.map((r) => colorsForKeys(RAW)[r]));
    const pinned = { [RAW[2]]: "#123456" };
    const withPin = ring(build({ sliceColors: pinned }), 0);
    expect(withPin.map((d: any) => d.itemStyle.color))
      .toEqual(RAW.map((r) => colorsForKeys(RAW, pinned)[r]));
  });

  /**
   * AGGREGATION IS UNCHANGED BY ANY OF THE ABOVE, and this is the assertion that says so.
   *
   * `aggregateRing` has always grouped on `parts.join(KEY_SEP)` — the STRINGIFIED RAW values —
   * and only ever passed `labelOf` over the display `name`. So two raw values that format to
   * one label were already two slices, and moving the curations onto the key cannot merge or
   * split anything. Stated here because the opposite (a builder that aggregates on the label)
   * would have made this a rendering change in a settings change's clothes, and the difference
   * is not visible from the curation code alone.
   */
  it("two raw values that format to the same label are still two slices", () => {
    // Both days are in the same month, so `MMMM YYYY` collapses them to one label.
    const data = ring(buildPieOption(COLS, [["2026-10-01", 30], ["2026-10-15", 10]], cfg), 0);
    expect(data.map((d: any) => d.name)).toEqual(["October 2026", "October 2026"]);
    expect(data.map((d: any) => d.key)).toEqual(["2026-10-01", "2026-10-15"]);
    expect(data.map((d: any) => d.value)).toEqual([30, 10]);
  });

  it("an outer ring's own dimension value is raw there too", () => {
    // `sliceColors` has always matched on every ring, not just ring 0 — so the same VALUE
    // appearing as an outer-ring dimension value takes the override there as well. That match
    // has to speak the same vocabulary the panel wrote, which is the raw value, not the label
    // the outer ring happens to be drawn with.
    const cols2 = [
      { name: "region", type: "varchar" }, { name: "order_month", type: "date" },
      { name: "rev", type: "double" },
    ];
    const opt = buildPieOption(cols2, [["EU", RAW[0], 60], ["US", RAW[1], 40]], {
      dimensions: ["region", "order_month"], metric: "rev",
      dimensionUnits: { order_month: "month" },
      columnSettings: { order_month: { date_style: "MMMM YYYY" } },
      sliceColors: { [RAW[0]]: "#654321" },
    }) as any;
    const outer = ring(opt, 1);
    expect(outer.map((d: any) => d.name)).toEqual(["December 2026", "November 2026"]);
    expect(outer.find((d: any) => d.name === "December 2026").itemStyle.color).toBe("#654321");
  });
});

/**
 * RING 0 AS THE SETTINGS PANEL HAS TO SEE IT.
 *
 * `pieSliceRows` is the one derivation `PieDisplayControl` is allowed to have: the panel needs the
 * slices in the order the chart draws them and it needs to know which ones the fold swallowed, and
 * it can work out neither from the slice NAMES it used to be handed. Everything here is asserted
 * against `buildPieOption`'s own output where the two overlap, because a helper that restates the
 * rule instead of sharing it is exactly how the panel and the chart drifted apart in the first
 * place.
 */
describe("pieSliceRows — the panel's view of ring 0", () => {
  const c = [{ name: "category", type: "varchar" }, { name: "m", type: "double" }];
  /** NOT pre-sorted: the rows arrive Gizmo, Widget, Gadget, Doohickey and the pie draws
   *  Widget, Gadget, Gizmo, Doohickey. A fixture arriving value-descending could not tell a list
   *  that sorts from one that echoes its input. */
  const r: unknown[][] = [["Gizmo", 20], ["Widget", 40], ["Gadget", 30], ["Doohickey", 10]];
  const base = { dimensions: ["category"], metric: "m" };
  const keys = (cfg: Record<string, unknown> = {}) => pieSliceRows(c, r, { ...base, ...cfg }).map((s) => s.key);
  const foldedKeys = (cfg: Record<string, unknown> = {}) =>
    pieSliceRows(c, r, { ...base, ...cfg }).filter((s) => s.folded).map((s) => s.key);
  const drawnNames = (cfg: Record<string, unknown> = {}) =>
    (buildPieOption(c, r, { ...base, ...cfg }) as any).series[0].data.map((d: any) => d.name);

  it("reads value-descending, which is the order the chart draws", () => {
    expect(keys()).toEqual(drawnNames());
    expect(keys()).not.toEqual(r.map((row) => row[0])); // …and that is not the row order
  });

  it("follows the chart back to row order when sorting is off", () => {
    // The literal is stated as well as the comparison: both sides of the comparison go through the
    // same sort, so a builder that ignored `sortRows: false` would keep them agreeing with each
    // other while both disagreed with the pie the user asked for.
    expect(keys({ sortRows: false })).toEqual(["Gizmo", "Widget", "Gadget", "Doohickey"]);
    expect(keys({ sortRows: false })).toEqual(drawnNames({ sortRows: false }));
  });

  it("marks the slices the fold swallowed, and puts them where the bucket is drawn", () => {
    const cfg = { sliceThreshold: 26 }; // Gizmo 20% and Doohickey 10% are both under it
    expect(drawnNames(cfg)).toEqual(["Widget", "Gadget", "Other"]);
    expect(keys(cfg)).toEqual(["Widget", "Gadget", "Gizmo", "Doohickey"]);
    expect(foldedKeys(cfg)).toEqual(["Gizmo", "Doohickey"]);
  });

  it("folds nothing when only one slice is under the threshold, as buildSlices does not", () => {
    const cfg = { sliceThreshold: 15 }; // Doohickey alone
    expect(drawnNames(cfg)).toEqual(["Widget", "Gadget", "Gizmo", "Doohickey"]);
    expect(foldedKeys(cfg)).toEqual([]);
  });

  // A hidden slice is dropped before the ring's total is computed, so it cannot be folded — and it
  // stays listed, because its own switch is the only route back to showing it.
  it("keeps a hidden slice listed, unfolded, after the ones the chart draws", () => {
    const cfg = { hiddenSlices: ["Widget"] };
    expect(drawnNames(cfg)).toEqual(["Gadget", "Gizmo", "Doohickey"]);
    expect(keys(cfg)).toEqual(["Gadget", "Gizmo", "Doohickey", "Widget"]);
    expect(foldedKeys(cfg)).toEqual([]);
  });

  // Hiding re-bases the remaining shares, which changes what falls under the threshold. A list that
  // judged the fold before the hide would fold nothing here.
  it("judges the fold on the shares that remain after a hide, as the ring does", () => {
    // At 25%, of 100: Gizmo 20% and Doohickey 10% are both under it, so they fold. Hide Widget and
    // the remaining 60 re-bases them to 33% and 17% — only one is under it now, and one does not
    // make a bucket. A list that folded before the hide would drop Gizmo's row for a chart that
    // draws Gizmo.
    expect(foldedKeys({ sliceThreshold: 25 })).toEqual(["Gizmo", "Doohickey"]);
    const cfg = { hiddenSlices: ["Widget"], sliceThreshold: 25 };
    expect(drawnNames(cfg)).toEqual(["Gadget", "Gizmo", "Doohickey"]);
    expect(foldedKeys(cfg)).toEqual([]);
    expect(keys(cfg)).toEqual(["Gadget", "Gizmo", "Doohickey", "Widget"]);
  });

  it("lists nothing when the chart has no dimension or no metric to draw with", () => {
    expect(pieSliceRows(c, r, { metric: "m" })).toEqual([]);
    expect(pieSliceRows(c, r, { dimensions: ["category"] })).toEqual([]);
    expect(pieSliceRows(c, r, { dimensions: ["nope"], metric: "m" })).toEqual([]);
  });

  /**
   * A TEMPORAL EXTRACTION UNIT KEYS ON THE EXTRACTED WORD, not on the number in the column: the
   * aggregation groups on `temporalLabel(unit, raw)`, so "Wed" IS the slice's raw value as far as
   * every stored curation is concerned. A panel that stringified the column would write
   * `sliceColors["3"]`, which no slice carries.
   *
   * NOTE (PR 06 task 4): this closes the derivation, not the app path. `VizControlProps` carries no
   * temporal units, so `PieDisplayControl` still passes none — threading them is a change to the
   * studio shell's control contract, not to the pie.
   */
  it("keys on the extracted word for a temporal extraction unit, as the ring does", () => {
    const dowCols = [{ name: "dow", type: "bigint" }, { name: "m", type: "double" }];
    const dowRows: unknown[][] = [[3, 20], [1, 40]];
    const cfg = { dimensions: ["dow"], metric: "m", dimensionUnits: { dow: "day_of_week" } };
    expect((buildPieOption(dowCols, dowRows, cfg) as any).series[0].data.map((d: any) => d.name))
      .toEqual(["Mon", "Wed"]);
    expect(pieSliceRows(dowCols, dowRows, cfg).map((s) => s.key)).toEqual(["Mon", "Wed"]);
  });
});

/**
 * PR 06 TASK 5 — the one-ring pie gets the defaults Metabase gives its ONLY pie shape.
 *
 * Metabase has no ring count and no donut switch: `getDefaultPercentVisibility = () => "legend"`
 * and `getDefaultShowTotal = () => true` are both unconditional, and `DIMENSIONS.slice
 * .innerRadiusRatio = 3/5` means every Metabase pie has a hole. egret withheld both from a single
 * ring on reasons measured below to be about the ARCS (which "legend" never touches) and about a
 * hole "small enough to read as deliberate" (egret's is 45/70 = 0.643 of the outer radius, LARGER
 * than the 3/5 = 0.600 hole Metabase writes its total into).
 *
 * The holeless pie is the one shape Metabase cannot draw, and it keeps its empty middle: there is
 * nowhere to write.
 */
describe("a single-ring pie's percentage and total defaults (task 5)", () => {
  // rows totals 40 at ring 0: US 30 (75%), EU 10 (25%).
  const oneRing = { dimensions: ["region"], metric: "rev" };

  it("puts a lone ring's shares in the legend with nothing stored, as Metabase does", () => {
    const entries = buildPieLegend(cols, rows, oneRing);
    expect(entries.map((e) => e.name)).toEqual(["US", "EU"]);
    expect(entries.map((e) => e.percent)).toEqual(["75%", "25%"]);
  });

  it("leaves the arcs themselves uncluttered — the legend is where the number goes", () => {
    // The arcs are now bare outright, not merely percentage-free: `resolveShowLabels` defaults a
    // one-ring pie's slice labels off, so the name goes to the legend with the share rather than
    // being printed twice. `" "` is the builder's "nothing to show" sentinel.
    const opt = buildPieOption(cols, rows, oneRing) as any;
    expect(ring(opt, 0).find((d: any) => d.name === "US").label.formatter()).toBe(" ");
    // …and turning labels on puts the NAME back without dragging the percentage onto the arc.
    const named = buildPieOption(cols, rows, { ...oneRing, showLabels: true }) as any;
    expect(ring(named, 0).find((d: any) => d.name === "US").label.formatter()).toBe("US");
  });

  it("an explicit off still empties the legend of percentages", () => {
    const entries = buildPieLegend(cols, rows, { ...oneRing, percentVisibility: "off" });
    expect(entries.map((e) => e.percent)).toEqual([undefined, undefined]);
  });

  it("fills a one-ring DONUT's hole with the total by default", () => {
    const opt = buildPieOption(cols, rows, { ...oneRing, donut: true }) as any;
    expect(opt.graphic).toBeTruthy();
    expect(holeTexts(opt.graphic)).toEqual(["40", "Total"]);
  });

  it("an explicit false still empties a one-ring donut's hole", () => {
    const opt = buildPieOption(cols, rows, { ...oneRing, donut: true, showTotal: false }) as any;
    expect(opt.graphic).toBeFalsy();
  });

  it("a holeless pie still draws no total — there is nowhere to write it", () => {
    expect((buildPieOption(cols, rows, oneRing) as any).graphic).toBeFalsy();
    expect((buildPieOption(cols, rows, { ...oneRing, showTotal: true }) as any).graphic).toBeFalsy();
  });
});

/**
 * `sliceOrder` IS RING 0'S, AND ONLY RING 0'S.
 *
 * MUTANT: `if (level === 0 && cfg.sliceOrder?.length)` → `if (cfg.sliceOrder?.length)`. It survived
 * the entire suite. `hiddenSlices`' ring-0-only rule is asserted ("curation applies to ring 0 only"
 * above) and `sliceOrder`'s — stated in the same breath, two lines apart, by a comment that says
 * "Ring 0 only" outright — was not.
 *
 * WHY IT MATTERS RATHER THAN BEING A TECHNICALITY. The panel's reorder arrows list the FIRST
 * dimension's values and nothing else, so `sliceOrder` is a vocabulary of ring-0 keys. `Slice.raw`
 * is each ring's own dimension value, though, so a string that happens to occur in both rings —
 * a region also used as a category, a month also used as a label — would be reordered in a ring
 * whose order the reader was never offered a control over, and only in that one.
 *
 * THE FIXTURE NEEDS THREE CHILDREN UNDER ONE PARENT. With two, "moved to the front" and "already
 * at the front" are the same array half the time; with three, the natural (value-desc) order and
 * the overridden one differ in a way no swap can produce by accident. The moved value is the
 * SMALLEST, so nothing but an order override could put it first.
 */
describe("sliceOrder applies to ring 0 and leaves the rings outside it alone", () => {
  const COLS2 = [
    { name: "region", type: "varchar" },
    { name: "category", type: "varchar" },
    { name: "m", type: "double" },
  ];
  // EU's three children are deliberately unequal: value-desc is Y(30), Z(20), X(10).
  const ROWS2: unknown[][] = [
    ["EU", "Y", 30], ["EU", "Z", 20], ["EU", "X", 10], ["US", "W", 40],
  ];
  const build = (extra: Record<string, unknown> = {}) =>
    buildPieOption(COLS2, ROWS2, { dimensions: ["region", "category"], metric: "m", ...extra }) as any;

  it("the fixture draws two rings in the orders the assertions below are about", () => {
    const opt = build();
    expect(ring(opt, 0).map((d: any) => d.name)).toEqual(["EU", "US"]);
    // Depth-first: EU's children in value-desc order, then US's.
    expect(ring(opt, 1).map((d: any) => d.name)).toEqual(["Y", "Z", "X", "W"]);
  });

  it("naming a ring-1 value in sliceOrder does not reorder ring 1", () => {
    const opt = build({ sliceOrder: ["X"] });
    // Unchanged. Under the mutant X leads its parent's run and this reads ["X", "Y", "Z", "W"].
    expect(ring(opt, 1).map((d: any) => d.name)).toEqual(["Y", "Z", "X", "W"]);
  });

  it("and does not disturb ring 0 either — the name is not a ring-0 value at all", () => {
    expect(ring(build({ sliceOrder: ["X"] }), 0).map((d: any) => d.name)).toEqual(["EU", "US"]);
  });

  /**
   * The positive control. Without it, deleting the whole `sliceOrder` block would pass both
   * assertions above — "ring 1 is untouched" is satisfied most cheaply by touching nothing.
   */
  it("but naming a RING-0 value does reorder ring 0, and carries its children with it", () => {
    const opt = build({ sliceOrder: ["US"] });
    expect(ring(opt, 0).map((d: any) => d.name)).toEqual(["US", "EU"]);
    // The children follow their parent because the sunburst nests them, not because ring 1 was
    // re-sorted: US's single child leads, then EU's three in their own untouched order.
    expect(ring(opt, 1).map((d: any) => d.name)).toEqual(["W", "Y", "Z", "X"]);
  });
});
