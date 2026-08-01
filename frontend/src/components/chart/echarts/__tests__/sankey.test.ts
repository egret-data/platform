import { describe, it, expect } from "vitest";
import { buildSankeyOption, sankeyProblem, hasCyclicFlow, MAX_SANKEY_NODES } from "../options/sankey";
import { SERIES_PALETTE } from "../palette";

const cols = [{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "v", type: "double" }];
const rows = [["A", "X", 10], ["A", "Y", 5], ["B", "X", 3]];
const cfg = { source: "s", target: "t", metric: "v" };
const series = (opt: any) => opt.series[0];

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
    const opt = buildSankeyOption(cols, rows, cfg) as any;
    const fills = series(opt).data.map((n: any) => n.itemStyle.color);
    expect(fills).toEqual([SERIES_PALETTE[0], SERIES_PALETTE[1], SERIES_PALETTE[2], SERIES_PALETTE[3]]);
    for (const f of fills) expect(SERIES_PALETTE).toContain(f);
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
