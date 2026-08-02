import { describe, it, expect } from "vitest";
import type { EChartsSize } from "../EChartsRenderer";
import {
  buildSankeyOption, sankeyProblem, hasCyclicFlow, MAX_SANKEY_NODES,
  type SankeyBuildConfig,
} from "../options/sankey";
import { SERIES_PALETTE } from "../palette";
import { colorsForKeys } from "../keyColors";

const cols = [{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "v", type: "double" }];
const rows = [["A", "X", 10], ["A", "Y", 5], ["B", "X", 3]];
const cfg = { source: "s", target: "t", metric: "v" };
const series = (opt: any) => opt.series[0];

const buildSankey = (rws: unknown[][]) => buildSankeyOption(cols, rws, cfg) as any;
/** Node names in the order the builder emits them — source then target, row by row, first seen. */
const nodeNamesOf = (opt: any): string[] => series(opt).data.map((n: any) => n.name);
const nodeColorsOf = (opt: any): string[] => series(opt).data.map((n: any) => n.itemStyle.color);
const colorOf = (opt: any, name: string): string =>
  series(opt).data.find((n: any) => n.name === name).itemStyle.color;

/**
 * NINE nodes, one more than the palette has colours.
 *
 * That count is the point: the colours used to be `seriesColor(i)`, i.e. `i % 8`, so on a chart
 * whose own ceiling is 150 nodes the ninth was drawn in the first one's blue — two nodes, one ink,
 * on the one diagram read entirely by following a colour from column to column.
 *
 * THE EMISSION ORDER IS LOAD-BEARING AND MUST NOT BE SORTED. `colorsForKeys` builds its Record in
 * an order of its own (sorted, on the hash path; preferred names first, on the harmony path this
 * fixture takes), so a fixture whose nodes are already in that order makes a by-NAME lookup and a
 * by-POSITION one return the identical sequence — and the tests named for the distinction pass
 * against either. That is not hypothetical: it is exactly how two of PR 04's cartesian tests
 * shipped blind. `Pending` is a preferred name (`PREFERRED_COLORS.error`), so `colorsForKeys`
 * assigns it BEFORE the other eight and its entry leads the Record while the node sits fourth —
 * which is what makes `Object.values(...)[i]` disagree with `colors[name]` here. Reordering these
 * rows, or renaming `Pending`, re-blinds the tests below; `NINE_NODES` is asserted so it cannot
 * happen quietly.
 */
const nineNodeRows: unknown[][] = [
  ["Widget", "Shipped", 40],
  ["Doohickey", "Pending", 30],
  ["Gizmo", "Returned", 20],
  ["Gadget", "Delivered", 10],
  ["Widget", "Refunded", 5],
];
const NINE_NODES = [
  "Widget", "Shipped", "Doohickey", "Pending", "Gizmo", "Returned", "Gadget", "Delivered", "Refunded",
];

describe("buildSankeyOption", () => {
  it("builds unique nodes and weighted links", () => {
    const opt = buildSankeyOption(cols, rows, cfg) as any;
    expect(series(opt).type).toBe("sankey");
    expect(series(opt).data.map((n: any) => n.name).sort()).toEqual(["A", "B", "X", "Y"]);
    expect(series(opt).links).toEqual([
      { source: "A", target: "X", value: 10 },
      { source: "A", target: "Y", value: 5 },
      { source: "B", target: "X", value: 3 },
    ]);
  });

  /**
   * Each node states its OWN colour. Left to itself ECharts INTERPOLATES along the palette array
   * for a sankey rather than picking discrete entries — measured live on seven nodes it drew
   * #7EB967, #F3AF6C, #F6BF6A, #CA8AAA and #61A8B5, five colours in no design system. Metabase's
   * nodes measure as clean accents.
   */
  it("colours each node with a palette accent, not something between two", () => {
    const opt = buildSankey(rows);
    // Emitted source-then-target, so NOT alphabetical — which is what makes the by-name lookup
    // below distinguishable from a positional one. See `nineNodeRows`.
    expect(nodeNamesOf(opt)).toEqual(["A", "X", "Y", "B"]);
    const fills = nodeColorsOf(opt);
    expect(fills).toEqual(nodeNamesOf(opt).map((n) => colorsForKeys(["A", "X", "Y", "B"])[n]));
    for (const f of fills) expect(SERIES_PALETTE).toContain(f);
  });

  /**
   * A node's colour comes from a hash of its NAME, as Metabase's does — so a category is the same
   * colour on the flow diagram as on every other card of the dashboard that plots it, and it does
   * not change the moment a row is added that introduces an earlier node.
   */
  it("keys a node's colour to its name, not the order it was met in", () => {
    const opt = buildSankey(nineNodeRows);
    expect(nodeNamesOf(opt)).toEqual(NINE_NODES); // the emission order this pins against
    expect(colorOf(opt, "Widget")).toBe(colorsForKeys(NINE_NODES).Widget);
    expect(nodeColorsOf(opt)).toEqual(NINE_NODES.map((n) => colorsForKeys(NINE_NODES)[n]));
  });

  /**
   * Nine nodes, nine colours. `seriesColor(i)` was `i % 8`, so node nine took node one's blue on a
   * diagram whose own limit is 150 nodes; past eight keys `colorsForKeys` widens the harmony
   * (each accent plus its dark variant) rather than starting the palette over.
   */
  it("gives nine nodes nine distinct colours", () => {
    const opt = buildSankey(nineNodeRows);
    expect(nodeNamesOf(opt)).toHaveLength(9);
    expect(new Set(nodeColorsOf(opt)).size).toBe(9);
  });

  it("drops non-positive / non-finite links", () => {
    const opt = buildSankeyOption(cols, [["A", "X", 10], ["A", "Y", 0], ["B", "X", -2]], cfg) as any;
    expect(series(opt).links.map((l: any) => l.source)).toEqual(["A"]);
  });

  it("empty when source / target / metric missing", () => {
    expect(series(buildSankeyOption(cols, rows, {}) as any).links).toEqual([]);
  });

  /**
   * An edge belongs to two nodes, so colouring it has to pick ONE. egret drew a `"gradient"`, which
   * belongs to neither and cannot be followed with the eye — the single thing a sankey is read for.
   * Metabase's default is the source; its gray is a fixed neutral, not a palette accent.
   */
  it("colours edges by an end, not with a gradient", () => {
    expect(series(buildSankeyOption(cols, rows, cfg) as any).lineStyle.color).toBe("source");
    expect(series(buildSankeyOption(cols, rows, { ...cfg, edgeColor: "target" }) as any).lineStyle.color).toBe("target");
    expect(series(buildSankeyOption(cols, rows, { ...cfg, edgeColor: "gray" }) as any).lineStyle.color).toBe("#81898e");
  });

  it("draws edges at Metabase's opacity and curve", () => {
    const s = series(buildSankeyOption(cols, rows, cfg) as any);
    expect(s.lineStyle.opacity).toBe(0.2);
    expect(s.lineStyle.curveness).toBe(0.5);
  });

  it("takes the node alignment from the config, defaulting left", () => {
    expect(series(buildSankeyOption(cols, rows, cfg) as any).nodeAlign).toBe("left");
    expect(series(buildSankeyOption(cols, rows, { ...cfg, nodeAlign: "justify" }) as any).nodeAlign).toBe("justify");
  });

  // A sankey's layout IS the answer — which node sits at which depth, in what order. Dragging one
  // apart only destroys that, so Metabase pins them; ECharts leaves them draggable by default.
  it("pins the nodes", () => {
    expect(series(buildSankeyOption(cols, rows, cfg) as any).draggable).toBe(false);
  });

  // A node label lands on top of the edges leaving it; the halo is what keeps it readable there.
  it("haloes the node labels", () => {
    const s = series(buildSankeyOption(cols, rows, cfg) as any);
    expect(s.label.textBorderWidth).toBe(3);
    expect(s.label.textBorderColor).toBeTruthy();
  });

  it("shows edge labels only when asked, and formats them", () => {
    expect(series(buildSankeyOption(cols, rows, cfg) as any).edgeLabel.show).toBe(false);
    const on = series(buildSankeyOption(cols, rows, { ...cfg, showEdgeLabels: true }) as any);
    expect(on.edgeLabel.show).toBe(true);
    expect(on.edgeLabel.formatter({ value: 1234 })).toBe("1,234");
    const compact = series(buildSankeyOption(cols, rows, { ...cfg, showEdgeLabels: true, labelFormatting: "compact" }) as any);
    expect(compact.edgeLabel.formatter({ value: 1234 })).toBe("1.2K");
  });
});

/**
 * A deterministic stub measurer, the same convention `sliceLabel.test.ts` uses: 7px per character
 * whatever font is asked for. Every expected number below is arithmetic on it, so the assertions
 * say what the builder computed rather than what some rasteriser happened to return.
 */
const stubMeasure = (s: string) => s.length * 7;

/** The card the defect was measured on: labels drawn at x=158 on a 166px svg, reaching 197-221. */
const CARD: EChartsSize = { width: 166, height: 184 };
/** Wide enough that the 25% cap never binds, so the reservation is the raw label width. */
const ROOMY: EChartsSize = { width: 800, height: 400 };

/**
 * THREE levels, and every width in it is load-bearing — this fixture exists to make wrong
 * implementations disagree with the right one rather than accidentally match it:
 *
 *  - "Doohickey Assembly" (18 chars, 126px) is a LEVEL-0 node and the WIDEST name in the fixture.
 *    An implementation that reserves room for the widest label ANYWHERE reserves 126; the right
 *    one reserves 63. First-column labels are drawn INTO the diagram and need no gutter at all.
 *  - "Warehouse Hub" (13, 91px) sits in the MIDDLE level, so "widest non-root" disagrees too.
 *  - The last column is Shipped (7) / Returned (8) / Delivered (9): three DIFFERENT widths, with
 *    the widest one LAST — so "reserve the widest" and "reserve the first" disagree.
 *  - "Cancelled Order" (15, 105px) is a level-1 SINK. `nodeAlign: "justify"` pushes a node with no
 *    outputs to the right edge, so it joins the reserved set there and not under `"left"` — which
 *    is the one thing distinguishing "last level" from "last drawn column".
 */
const labelRoomRows: unknown[][] = [
  ["Doohickey Assembly", "Warehouse Hub", 40],
  ["Widget", "Warehouse Hub", 10],
  ["Widget", "Cancelled Order", 5],
  ["Warehouse Hub", "Shipped", 30],
  ["Warehouse Hub", "Returned", 10],
  ["Warehouse Hub", "Delivered", 20],
];
const LAST_COLUMN = ["Shipped", "Returned", "Delivered"];

const labelRoom = (size?: EChartsSize, over: Partial<SankeyBuildConfig> = {}) =>
  series(buildSankeyOption(cols, labelRoomRows, { ...cfg, ...over }, size, stubMeasure) as any);

/**
 * Measured live at 166x184: the right-hand labels started at x=158 — 8px from the edge — and ran to
 * 197-221, i.e. 30-55px PAST it, because `right` was the string `"8%"` (13px here) and no label
 * width or overflow was set anywhere. Metabase reserves the room instead: its layout adds the
 * widest last-column label to the right padding, capped at 25% of the width, and truncates those
 * labels to the cap when it binds (`getSankeyLayout`, `MAX_RIGHT_LABEL_LENGTH_PERCENT`).
 */
describe("sankey node-label room", () => {
  it("reserves the LAST column's widest label in the right gutter", () => {
    const s = labelRoom(ROOMY);
    // 5% of 800 = 40, Metabase's own horizontal padding (floored at 24), plus "Delivered" at 63.
    expect(s.right).toBe(40 + stubMeasure("Delivered"));
    // NOT the widest label on the chart: "Doohickey Assembly" is 126px and sits in the FIRST
    // column, whose labels are drawn into the diagram and cost the gutter nothing.
    expect(s.right).toBeLessThan(40 + stubMeasure("Doohickey Assembly"));
    // Nothing had to be cut at this size, so there is no formatter to cut it.
    expect(s.label.formatter).toBeUndefined();
  });

  it("keeps the last column's labels inside the canvas on the 166px card", () => {
    const s = labelRoom(CARD);
    // The widest last-column label (63) is over the 25% cap (41.5), so the cap is what is reserved,
    // on top of the 24px floor — a 166px card is under the width where 5% reaches 24.
    expect(s.right).toBe(24 + 41.5);
    // ...and the labels are cut to the room reserved for them. 41.5 less the 7px ellipsis leaves
    // 34.5, which holds four characters.
    const drawn = LAST_COLUMN.map((n) => s.label.formatter({ name: n }));
    expect(drawn).toEqual(["Ship…", "Retu…", "Deli…"]);

    // The defect, stated as the reader sees it: the last column's nodes end at `width - right`, and
    // every label drawn from there must finish before the canvas does. It used to finish 30-55px
    // outside.
    const nodeRightEdge = CARD.width - (s.right as number);
    for (const text of drawn) {
      expect(nodeRightEdge + stubMeasure(text)).toBeLessThanOrEqual(CARD.width);
    }
  });

  it("cuts only the column that has to live in the gutter", () => {
    const s = labelRoom(CARD);
    expect(s.label.formatter({ name: "Doohickey Assembly" })).toBe("Doohickey Assembly");
    expect(s.label.formatter({ name: "Warehouse Hub" })).toBe("Warehouse Hub");
  });

  /**
   * A node with no outgoing flow is only on the right EDGE when the alignment puts it there —
   * Metabase's `getMostRightNodes`, whose `shouldIncludeAllNodesWithoutOutputs` is exactly
   * `node_align !== "left"`. Under "left" the sink stays at its own depth with the whole diagram
   * still to its right, and reserving for it would waste a fifth of the canvas.
   */
  it("counts a sink as last-column only when the alignment pushes it there", () => {
    expect(labelRoom(ROOMY).right).toBe(40 + stubMeasure("Delivered"));
    expect(labelRoom(ROOMY, { nodeAlign: "justify" }).right).toBe(40 + stubMeasure("Cancelled Order"));
    expect(labelRoom(ROOMY, { nodeAlign: "right" }).right).toBe(40 + stubMeasure("Cancelled Order"));
  });

  // No measurement means no fitting: the same rule every other size-aware builder follows. Nothing
  // is truncated against a width nobody supplied.
  it("leaves the gutter alone when the caller has no measurement", () => {
    const s = labelRoom(undefined);
    expect(s.right).toBe("8%");
    expect(s.label.formatter).toBeUndefined();
  });
});

describe("sankeyProblem", () => {
  /**
   * Metabase checks these BEFORE rendering. egret checked nothing, so a circular flow reached
   * ECharts and it threw **"Sankey is a DAG, the original data has cycle!"** onto the card in red —
   * the library's own wording, naming an acronym instead of a fix. Seen live before this change.
   */
  it("names a circular flow in words a reader can act on", () => {
    const msg = sankeyProblem(cols, [["A", "B", 1], ["B", "C", 1], ["C", "A", 1]], cfg);
    expect(msg).toMatch(/circular/i);
    expect(msg).not.toMatch(/DAG/);
  });

  it("catches source and target being the same column", () => {
    expect(sankeyProblem(cols, rows, { ...cfg, target: "s" })).toMatch(/two different columns/i);
  });

  it("catches too many nodes, and says how many there are", () => {
    const many = Array.from({ length: MAX_SANKEY_NODES + 5 }, (_, i) => [`n${i}`, `m${i}`, 1]);
    const msg = sankeyProblem(cols, many, cfg);
    expect(msg).toContain(String(MAX_SANKEY_NODES));
    expect(msg).toContain(String((MAX_SANKEY_NODES + 5) * 2));
  });

  it("passes a plain acyclic flow", () => {
    expect(sankeyProblem(cols, rows, cfg)).toBeUndefined();
  });

  it("says nothing until the columns are picked", () => {
    expect(sankeyProblem(cols, rows, { source: "s" })).toBeUndefined();
  });
});

describe("hasCyclicFlow", () => {
  it("finds a loop of any length", () => {
    expect(hasCyclicFlow([["A", "B"], ["B", "A"]], 0, 1)).toBe(true);
    expect(hasCyclicFlow([["A", "B"], ["B", "C"], ["C", "A"]], 0, 1)).toBe(true);
    expect(hasCyclicFlow([["A", "A"]], 0, 1)).toBe(true);
  });

  /**
   * A node reached by two separate routes is a DIAMOND, which a sankey draws happily. Only a node
   * reached again while still on the current descent is a loop — which is why the walk tracks the
   * path separately from everything visited. Treating "visited" as "on the path" rejects this.
   */
  it("does not mistake a diamond for a loop", () => {
    expect(hasCyclicFlow([["A", "B"], ["A", "C"], ["B", "D"], ["C", "D"]], 0, 1)).toBe(false);
  });

  // Iterative, not recursive: a long chain on the render path must not blow the stack.
  it("handles a chain far longer than the call stack", () => {
    const chain = Array.from({ length: 50_000 }, (_, i) => [`n${i}`, `n${i + 1}`]);
    expect(hasCyclicFlow(chain, 0, 1)).toBe(false);
  });
});
