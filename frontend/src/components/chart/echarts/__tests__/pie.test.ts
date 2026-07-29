import { describe, it, expect } from "vitest";
import { buildPieOption } from "../options/pie";
import { SERIES_PALETTE, seriesColor, lightenOklch } from "../palette";
import { calcAvailableDonutSliceLabelLength } from "../sliceLabel";

const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["EU", 10], ["US", 30]];

describe("buildPieOption", () => {
  it("maps rows to {name, value} slices, sorted by value desc", () => {
    const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
    expect(opt.series[0].type).toBe("pie");
    // Each datum also carries its own `label` (fix 1b/2 — per-slice fit + contrast colour, see
    // the "slice label fit & colour" describe block below) — picked out here so this test stays
    // about mapping/sorting, not label content.
    expect(opt.series[0].data.map((d: any) => ({ name: d.name, value: d.value }))).toEqual([
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
  // Both default ON, resolved here at render time rather than stored — so an already-saved
  // chart with no showLegend/showLabels picks up the fix too. `?? true`, not `|| true`: an
  // explicit `false` must still turn each off, not be treated as "unset".
  it("showLegend defaults on; an explicit false turns it off", () => {
    expect((buildPieOption(cols, rows, base) as any).legend).toMatchObject({ type: "scroll" });
    expect((buildPieOption(cols, rows, { ...base, showLegend: true }) as any).legend).toMatchObject({ type: "scroll" });
    expect((buildPieOption(cols, rows, { ...base, showLegend: false }) as any).legend).toBeUndefined();
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
    // showLabels now defaults on, so it must be explicitly turned off here to isolate
    // percentVisibility's own contribution to `label.show`.
    const o = opt({ percentVisibility: "inside", showLabels: false });
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
    expect(opt.series[0].data.map((d: any) => ({ name: d.name, value: d.value }))).toEqual([
      { name: "US", value: 30 },
      { name: "EU", value: 10 },
    ]);
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
    const inner = opt.series[0].data;
    expect(inner.map((d: any) => d.name)).toEqual(["R2", "R1"]);
    expect(inner.some((d: any) => d.name === "Other")).toBe(false);

    const outer = opt.series[1].data;
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
    // Metabase's threeRingInnerRadiusRatio (1/4) is unconditional once there are 2+ rings, even
    // with `donut` off — see the "Metabase ring geometry" describe block below.
    assertContiguousRadii(opt.series, 17.5);
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
    // Metabase's threeRingInnerRadiusRatio (1/4 of the 70%-outer-edge scale this file uses) — see
    // the "Metabase ring geometry" describe block below for why this replaced the old flat 45%.
    assertContiguousRadii(three.series, 17.5);
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
    (buildPieOption(cols2, rows2, { dimensions: ["grp", "item"], metric: "m" }) as any).series[1].data;

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
    expect(opt.series).toHaveLength(3);
    const outerRing = opt.series[2].data;
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

  // Chart settings are JSONB and never whitelisted server-side, so a slice colour can arrive as
  // anything at all. resolveColor is this codebase's one validator for a colour from a less
  // trusted source; an override that skipped it could paint a slice with a value the renderer
  // can't draw, which getTextColorForBackground would then silently mis-contrast a label against.
  it("a slice colour override the renderer can't draw is rejected, leaving the palette slot", () => {
    const opt = buildPieOption(cols2, rows2, {
      dimensions: ["region"], metric: "rev", sliceColors: { US: "banana", EU: "rgb(1,2,3)" },
    }) as any;
    const inner = opt.series[0].data;
    // Not a colour: no override at all, so ECharts' own palette slot still applies.
    expect(inner.find((d: any) => d.name === "US").itemStyle).toBeUndefined();
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
    expect(opt.series[0].data.map((d: any) => d.name)).toEqual(["United States", "EU"]);
  });

  it("a hidden slice is absent and the remaining percentages re-base", () => {
    const rows = [["A", 10], ["B", 30], ["C", 60]];
    const opt = buildPieOption(cols, rows, {
      dimensions: ["d"], metric: "m", hiddenSlices: ["B"],
    }) as any;
    const data = opt.series[0].data;
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
    const inner = opt.series[0].data;
    expect(inner.map((d: any) => d.name)).toEqual(["R2"]);
    const outer = opt.series[1].data;
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
    expect(opt.series[0].data.map((d: any) => d.name)).toEqual(["A", "B", "C"]);
  });

  it("decimals changes what a percentage label shows", () => {
    // showLabels defaults on now, so it's turned off explicitly to isolate the percent text
    // (otherwise the name would be prefixed too, which isn't what this test is about).
    const rows = [["A", 10], ["B", 20]];
    const withDefault = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", percentVisibility: "inside", showLabels: false }) as any;
    const withZero = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", percentVisibility: "inside", decimals: 0, showLabels: false }) as any;
    expect(withDefault.series[0].label.formatter({ name: "A", percent: 33.333 })).toBe("33.33%");
    expect(withZero.series[0].label.formatter({ name: "A", percent: 33.333 })).toBe("33%");
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
    const data = opt.series[0].data;
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
    const inner = opt.series[0].data;
    expect(inner.map((d: any) => d.name)).toEqual(["EU"]);
    const outer = opt.series[1].data;
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

  describe("fix 2 — a 1-degree gap between slices, in the chart's own background", () => {
    it("every ring gets a 1-degree padAngle, one ring included", () => {
      const one = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
      expect(one.series[0].padAngle).toBe(1);

      const three = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
      for (const s of three.series) expect(s.padAngle).toBe(1);
    });

    it("a single ring carries no extra border — padAngle alone is the gap", () => {
      const one = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
      expect(one.series[0].itemStyle?.borderWidth).toBeFalsy();
    });

    it("two rings use Metabase's twoRingBorderWidth (1), coloured like the chart background", () => {
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m" }) as any;
      for (const s of opt.series) {
        expect(s.itemStyle.borderWidth).toBe(1);
        // jsdom can't resolve var(--card); resolveColor falls back to a concrete colour rather
        // than ever leaking the literal "var(...)" to the SVG renderer.
        expect(s.itemStyle.borderColor).not.toContain("var(");
      }
    });

    it("three rings use Metabase's threeRingBorderWidth (0.3)", () => {
      const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
      for (const s of opt.series) expect(s.itemStyle.borderWidth).toBe(0.3);
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
      for (const s of opt.series) {
        expect(s.label.fontSize).toBe(12);
        expect(s.label.fontWeight).toBe(700);
        expect(s.label.position).toBe("inside");
      }
    });

    it("degrades small/overlapping slices by hiding them (ECharts labelLayout.hideOverlap), not overlapping into mush", () => {
      const opt = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m", showLabels: true }) as any;
      for (const s of opt.series) expect(s.labelLayout).toEqual({ hideOverlap: true });
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

    it("a 3-ring pie's labels use radial rotation; a 1-ring pie's do not", () => {
      const one = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev" }) as any;
      expect(one.series[0].label.rotate).toBe(0);

      const three = buildPieOption(cols3, rows3, { dimensions: ["region", "category", "brand"], metric: "m" }) as any;
      for (const s of three.series) expect(s.label.rotate).toBe("radial");
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
      const emea = opt.series[0].data.find((d: any) => d.name === "EMEA");
      expect(emea.label.formatter()).toBe("EMEA");
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
      const tiny = opt.series[0].data.find((d: any) => d.name === "Tiny");
      expect(tiny.label.formatter()).toBe(" "); // Metabase's own "nothing to show" sentinel
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
      const label = opt.series[0].data.find((d: any) => d.name === longName).label.formatter();
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
    it("the SAME slice's label survives in full at a large container size and truncates at a small one — size is the only variable", () => {
      const longName = "A very long region name that will not fit";
      const cfg = { dimensions: ["d"], metric: "m", showLabels: true, donut: true };
      const rows2 = [[longName, 55], ["Other region", 45]];

      const small = buildPieOption(dCols, rows2, cfg, stubMeasure, { width: 300, height: 300 }) as any;
      const smallLabel = small.series[0].data.find((d: any) => d.name === longName).label.formatter();
      expect(smallLabel).not.toBeNull();
      expect(smallLabel).not.toBe(longName);
      expect(smallLabel?.endsWith("…")).toBe(true);

      const large = buildPieOption(dCols, rows2, cfg, stubMeasure, { width: 3000, height: 3000 }) as any;
      const largeLabel = large.series[0].data.find((d: any) => d.name === longName).label.formatter();
      expect(largeLabel).toBe(longName);
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
      const ring0 = opt.series[0].data;
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
      const labelOf = (name: string) => opt.series[0].data.find((d: any) => d.name === name).label.formatter();

      // A row with no dimension value has nothing to fit — an empty string would otherwise sail
      // through the "already fits" fast path (it fits ANY width) and render as a content-less "".
      expect(labelOf("")).toBe(" ");

      // A slice with real but sub-ellipsis room truncates to a bare "…", which identifies nothing
      // — the MIN_TRUNCATED_LABEL_CHARS floor drops it instead.
      expect(labelOf("Tiny")).toBe(" ");

      // ...and the floor is not just dropping everything that doesn't fit whole: a slice with room
      // for a real multi-character prefix still gets one.
      expect(labelOf("Alpha")).toBe("Alp…");

      for (const d of opt.series[0].data) {
        const text = d.label.formatter();
        if (text === " ") continue; // the one legitimate "nothing to show" sentinel
        expect(text.trim()).not.toBe("");
        expect(text).not.toBe("…");
      }
    });

    it("each slice's label colour contrasts with ITS OWN fill, not one fixed colour for the whole ring", () => {
      const opt = buildPieOption(cols, rows, {
        dimensions: ["region"], metric: "rev",
        sliceColors: { EU: "#111111", US: "#eeeeee" }, // one very dark fill, one very light fill
      }) as any;
      const eu = opt.series[0].data.find((d: any) => d.name === "EU");
      const us = opt.series[0].data.find((d: any) => d.name === "US");
      expect(eu.label.color).not.toBe(us.label.color);
    });
  });

  describe("fix 4 — the legend lists only the first dimension's slices", () => {
    it("a two-ring pie's legend carries only ring 0's names, not ring 1's", () => {
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m", showLegend: true }) as any;
      expect(opt.legend.data).toEqual(["US", "EU"]);
      // "Widgets"/"Gadgets" are ring-1-only names here and must not appear in the legend list.
      expect(opt.legend.data).not.toContain("Widgets");
      expect(opt.legend.data).not.toContain("Gadgets");
    });

    it("a one-ring pie's legend is unaffected — it only ever had one ring's names anyway", () => {
      const opt = buildPieOption(cols, rows, { dimensions: ["region"], metric: "rev", showLegend: true }) as any;
      expect(opt.legend.data).toEqual(["US", "EU"]);
    });

    it("the legend is still absent when showLegend is explicitly off", () => {
      // showLegend now defaults on — this must set it to false explicitly to actually test
      // the off case (a bare config, with no showLegend at all, now shows a legend instead).
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m", showLegend: false }) as any;
      expect(opt.legend).toBeUndefined();
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
});

// Reversal — an outer ring's slice percentage is a share of its PARENT slice, not of that
// ring's own total (see the comment above buildPieOption's `series` map for the full
// before/after reasoning: this was the deliberate share-of-ring decision, reversed after a real
// three-ring chart showed it reads as noise). Ring 0 has no parent, so its share of the chart
// already IS its share of its own ring — nothing changes there, and it must stay untouched.
//
// ECharts computes `percent` once per SERIES (i.e. per ring) with no per-datum override, so the
// parent-relative number for level > 0 is precomputed here and stashed on the datum itself as
// `parentPercent`; both the label formatter (below) and the shared tooltip formatter (base.ts,
// see base.test.ts) prefer it over ECharts' own `percent` whenever a datum carries one.
describe("buildPieOption — outer-ring percentage is a share of its parent (reversal)", () => {
  it("a single-ring pie's percentage is unchanged: share of the ring, i.e. share of the whole", () => {
    const cols = [{ name: "d", type: "varchar" }, { name: "m", type: "double" }];
    const rows = [["A", 25], ["B", 75]];
    const opt = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m" }) as any;
    const a = opt.series[0].data.find((d: any) => d.name === "A");
    // Ring 0 never carries a parentPercent — the formatters fall back to ECharts' own percent.
    expect(a.parentPercent).toBeUndefined();
    expect(opt.tooltip.formatter({ name: "A", value: 25, percent: 25, data: a })).toContain("25%");
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
    const outer = opt.series[1].data;
    const c1Items = outer.filter((d: any) => d.name === "C1" && d.value === 500);
    expect(c1Items).toHaveLength(12);
    for (const item of c1Items) expect(item.parentPercent).toBeCloseTo(50, 9); // NOT 4.1(6)%
    // The tooltip formatter actually reaches 50% when fed the real datum, not the ring-share number.
    const tooltip = opt.tooltip.formatter({ name: "C1", value: 500, percent: 4.17, data: c1Items[0] });
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
    const [, middle, outer] = opt.series;

    // Middle ring (product): share of its own region — NOT of the 3000 grand total.
    expect(middle.data.find((d: any) => d.name === "P1").parentPercent).toBeCloseTo((1500 / 2000) * 100, 9); // 75%, not 50%
    expect(middle.data.find((d: any) => d.name === "P3").parentPercent).toBeCloseTo(100, 9); // R2's only product

    // Outer ring (channel): share of its own PRODUCT — not of region (60%) or the grand total (40%).
    expect(outer.data.find((d: any) => d.name === "C1").parentPercent).toBeCloseTo((1200 / 1500) * 100, 9); // 80%
    expect(outer.data.find((d: any) => d.name === "C4").parentPercent).toBeCloseTo((400 / 1000) * 100, 9); // 40%
  });

  it("decimals still controls the displayed precision at every level", () => {
    const cols2 = [
      { name: "segment", type: "varchar" }, { name: "channel", type: "varchar" }, { name: "m", type: "double" },
    ];
    const rows2 = [["S1", "C1", 1], ["S1", "C2", 2]]; // C1 = 1/3 of its segment = 33.333...%
    const withDefault = buildPieOption(cols2, rows2, { dimensions: ["segment", "channel"], metric: "m" }) as any;
    const withZero = buildPieOption(cols2, rows2, { dimensions: ["segment", "channel"], metric: "m", decimals: 0 }) as any;
    expect(withDefault.series[1].data.find((d: any) => d.name === "C1").parentPercent).toBeCloseTo(33.33, 9);
    expect(withZero.series[1].data.find((d: any) => d.name === "C1").parentPercent).toBe(33);
  });

  it('a parent\'s own "Other" bucket reports its share of that same parent', () => {
    const cols2 = [
      { name: "grp", type: "varchar" }, { name: "item", type: "varchar" }, { name: "m", type: "double" },
    ];
    // P1 totals 1000: two big (800, 180) + two small (10, 10) -> P1's OWN Other = 20 = 2% of P1.
    const rows2 = [["P1", "A1", 800], ["P1", "A2", 180], ["P1", "b1", 10], ["P1", "b2", 10]];
    const opt = buildPieOption(cols2, rows2, { dimensions: ["grp", "item"], metric: "m" }) as any;
    const other = opt.series[1].data.find((d: any) => d.name === "Other");
    expect(other.value).toBe(20);
    expect(other.parentPercent).toBeCloseTo(2, 9); // 20 / 1000 (P1's own total)
  });
});
