import { describe, expect, it } from "vitest";
import { dynamicSubflowsText, flowBadges, layoutDependencies, type FlowBox } from "@/components/workflows/namespaces/dependencies/dependenciesModel";
import { DEPS, chainOf, edge, flow } from "./fixtures";

const byName = (nodes: FlowBox[], name: string) => {
  const found = nodes.find((n) => n.flow.flow_id === name);
  if (!found) throw new Error(`no node ${name}`);
  return found;
};

const overlaps = (a: FlowBox, b: FlowBox) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

describe("layoutDependencies", () => {
  it("places every flow once, left to right along its calls and triggers, with no two cards overlapping", () => {
    const { nodes } = layoutDependencies(DEPS);
    expect(nodes.map((n) => n.id).sort()).toEqual(["f 5", "f1", "f2", "f3", "f4"]);
    expect(byName(nodes, "caller").x).toBeLessThan(byName(nodes, "extract").x);
    expect(byName(nodes, "extract").x).toBeLessThan(byName(nodes, "load").x);
    expect(byName(nodes, "sync").x).toBeLessThan(byName(nodes, "load").x);
    for (const a of nodes) for (const b of nodes) if (a !== b) expect(overlaps(a, b), `${a.id} overlaps ${b.id}`).toBe(false);
  });

  it("walks the connected flows first, along the layout, then the flows with no edge", () => {
    const order = layoutDependencies(DEPS).nodes.map((n) => n.flow.flow_id);
    // dagre ranks `sync` beside `extract`, one rank before `load`, which both reach.
    expect(order[0]).toBe("caller");
    expect(new Set(order.slice(1, 3))).toEqual(new Set(["extract", "sync"]));
    expect(order.slice(3)).toEqual(["load", "broken"]);
  });

  it("routes subflow edges solid and flow_trigger edges dashed, from one card's border to the other's", () => {
    const { nodes, edges } = layoutDependencies(DEPS);
    const call = edges.find((e) => e.source === "f 5" && e.target === "f1");
    const trig = edges.find((e) => e.source === "f1" && e.target === "f2");
    expect(call?.trigger).toBe(false);
    expect(trig?.trigger).toBe(true);
    const extract = byName(nodes, "extract");
    const first = call?.points[0];
    const last = call?.points[call.points.length - 1];
    expect(first?.x).toBeCloseTo(byName(nodes, "caller").x + byName(nodes, "caller").width, 0);
    expect(last?.x).toBeCloseTo(extract.x, 0);
    expect(edges).toHaveLength(3);
  });

  it("drops an edge naming a flow it does not have, and draws a flow calling itself as a note, not an edge", () => {
    const { nodes, edges } = layoutDependencies({
      nodes: [flow("a", "ns", "a"), flow("b", "ns", "b")],
      edges: [edge("a", "a", "subflow"), edge("a", "zz", "subflow"), edge("b", "b", "flow_trigger"), edge("a", "b", "subflow")],
      unparsable: 0,
    });
    expect(edges.map((e) => `${e.source}>${e.target}`)).toEqual(["a>b"]);
    expect(nodes.find((n) => n.id === "a")?.badges).toContain("Calls itself");
    expect(nodes.find((n) => n.id === "b")?.badges).toContain("Triggers itself");
  });

  it("packs flows with no edge into a grid rather than one long column", () => {
    const deps = { nodes: Array.from({ length: 100 }, (_, i) => flow(`s${i}`, "ns", `s${i}`)), edges: [], unparsable: 0 };
    const { nodes } = layoutDependencies(deps);
    const xs = new Set(nodes.map((n) => n.x));
    const ys = new Set(nodes.map((n) => n.y));
    expect(xs.size).toBeGreaterThan(5);
    expect(ys.size).toBeGreaterThan(5);
    for (let i = 0; i < nodes.length; i++) for (let j = i + 1; j < nodes.length; j++) expect(overlaps(nodes[i], nodes[j])).toBe(false);
  });

  it("lays out 2,000 chained flows without dagre's recursion: every card placed, in order", () => {
    const { nodes, edges } = layoutDependencies(chainOf(2000));
    expect(nodes).toHaveLength(2000);
    expect(edges).toHaveLength(1999);
    expect(nodes.every((n) => Number.isFinite(n.x) && Number.isFinite(n.y))).toBe(true);
    // Each call one rank further along.
    for (let i = 1; i < nodes.length; i++) expect(nodes[i].x).toBeGreaterThan(nodes[i - 1].x);
    expect(nodes[1999].flow.flow_id).toBe("flow_1999");
  });

  it("an empty graph is empty", () => {
    expect(layoutDependencies({ nodes: [], edges: [], unparsable: 0 })).toEqual({ nodes: [], edges: [] });
  });
});

describe("what a flow's card says", () => {
  it("names each of its states in words", () => {
    expect(flowBadges(flow("x", "o", "x", { in_namespace: false, disabled: true, unparsable: true }))).toEqual([
      "Outside this namespace",
      "Disabled",
      "Unparsable draft",
    ]);
    expect(flowBadges(flow("x", "o", "x"))).toEqual([]);
  });

  it("counts dynamic subflows with its own plural", () => {
    expect(dynamicSubflowsText(0)).toBeNull();
    expect(dynamicSubflowsText(1)).toBe("1 dynamic subflow");
    expect(dynamicSubflowsText(12)).toBe("12 dynamic subflows");
  });

  it("is as tall as the lines it holds", () => {
    const { nodes } = layoutDependencies(DEPS);
    // extract: namespace only; sync: namespace, Disabled, dynamic subflows.
    expect(byName(nodes, "sync").height - byName(nodes, "extract").height).toBe(32);
  });
});
