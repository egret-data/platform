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
    const rows = [["A", 10], ["B", 20]];
    const withDefault = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", percentVisibility: "inside" }) as any;
    const withZero = buildPieOption(cols, rows, { dimensions: ["d"], metric: "m", percentVisibility: "inside", decimals: 0 }) as any;
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

    it("the legend is still absent when showLegend is off", () => {
      const opt = buildPieOption(cols2, rows2, { dimensions: ["region", "category"], metric: "m" }) as any;
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
