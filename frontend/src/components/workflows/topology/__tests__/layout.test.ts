import { describe, expect, it } from "vitest";
import { layout, type Positioned } from "@/components/workflows/topology/layout";
import { EMPTY_FLOW, FLOW, LINE, NESTED } from "./fixtures";

const NONE: ReadonlySet<string> = new Set();
const byId = (nodes: Positioned[], id: string) => {
  const n = nodes.find((p) => p.id === id);
  if (!n) throw new Error(`no positioned ${id}`);
  return n;
};
const centre = (p: Positioned, axis: "x" | "y") => (axis === "x" ? p.x + p.width / 2 : p.y + p.height / 2);

describe("layout", () => {
  it.each([
    ["LR", "x"],
    ["TB", "y"],
  ] as const)("in %s puts the flow's start before every task along the main axis", (orientation, axis) => {
    const { nodes } = layout(FLOW, { orientation, collapsed: NONE });
    const flowStart = centre(byId(nodes, "start:flow"), axis);
    const tasks = nodes.filter((n) => n.kind === "task");
    expect(tasks.length).toBeGreaterThan(5);
    for (const t of tasks) expect(centre(t, axis)).toBeGreaterThan(flowStart);
  });

  it("follows the edges, not the order of the nodes array", () => {
    const { nodes } = layout(LINE, { orientation: "LR", collapsed: NONE });
    const xs = ["tasks.one", "tasks.two", "tasks.three"].map((id) => byId(nodes, id).x);
    expect(xs[0]).toBeLessThan(xs[1]);
    expect(xs[1]).toBeLessThan(xs[2]);
    // The returned order is the layout's order, so the keyboard can walk it.
    const ids = nodes.filter((n) => n.kind === "task").map((n) => n.id);
    expect(ids).toEqual(["tasks.one", "tasks.two", "tasks.three"]);
  });

  it("draws an expanded cluster around its members, before them in the returned order", () => {
    const { nodes } = layout(FLOW, { orientation: "LR", collapsed: NONE });
    const box = byId(nodes, "cluster:tasks.p");
    expect(box.kind).toBe("cluster");
    for (const id of ["tasks.p", "tasks.p.x", "tasks.p.y", "start:tasks.p", "end:tasks.p"]) {
      const n = byId(nodes, id);
      expect(n.parent).toBe("cluster:tasks.p");
      expect(n.x).toBeGreaterThanOrEqual(box.x);
      expect(n.y).toBeGreaterThanOrEqual(box.y);
      expect(n.x + n.width).toBeLessThanOrEqual(box.x + box.width);
      expect(n.y + n.height).toBeLessThanOrEqual(box.y + box.height);
      expect(nodes.indexOf(n)).toBeGreaterThan(nodes.indexOf(box));
    }
  });

  it("hides a collapsed cluster's nodes and reroutes their edges to the cluster", () => {
    const { nodes, edges } = layout(FLOW, { orientation: "LR", collapsed: new Set(["cluster:tasks.p"]) });
    const ids = new Set(nodes.map((n) => n.id));
    for (const hidden of ["tasks.p", "tasks.p.x", "tasks.p.y", "start:tasks.p", "end:tasks.p"]) expect(ids.has(hidden)).toBe(false);
    expect(byId(nodes, "cluster:tasks.p").kind).toBe("collapsed");
    const pairs = edges.map((e) => `${e.source} -> ${e.target}`);
    expect(pairs).toContain("tasks.a -> cluster:tasks.p");
    expect(pairs).toContain("cluster:tasks.p -> start:tasks.check");
    // No edge is left pointing at a hidden node, and the cluster's inner edges are gone, not looped.
    for (const e of edges) {
      expect(ids.has(e.source)).toBe(true);
      expect(ids.has(e.target)).toBe(true);
      expect(e.source).not.toBe(e.target);
    }
  });

  it("collapsing an outer cluster hides the clusters nested in it", () => {
    const { nodes, edges } = layout(NESTED, { orientation: "TB", collapsed: new Set(["cluster:tasks.s"]) });
    expect(nodes.map((n) => n.id).sort()).toEqual(["cluster:tasks.s", "end:flow", "start:flow", "tasks.after"]);
    expect(edges.map((e) => `${e.source} -> ${e.target}`).sort()).toEqual([
      "cluster:tasks.s -> tasks.after",
      "start:flow -> cluster:tasks.s",
      "tasks.after -> end:flow",
    ]);
  });

  it("nests a cluster's box inside its parent cluster's box", () => {
    const { nodes } = layout(NESTED, { orientation: "TB", collapsed: NONE });
    const outer = byId(nodes, "cluster:tasks.s");
    const inner = byId(nodes, "cluster:tasks.s.p");
    expect(inner.parent).toBe("cluster:tasks.s");
    expect(inner.x).toBeGreaterThan(outer.x);
    expect(inner.y).toBeGreaterThan(outer.y);
    expect(inner.x + inner.width).toBeLessThan(outer.x + outer.width);
    expect(inner.y + inner.height).toBeLessThan(outer.y + outer.height);
  });

  it("draws a failed Subflow expansion as its task node, carrying the reason, with no box and no markers", () => {
    const { nodes, edges } = layout(FLOW, { orientation: "LR", collapsed: NONE });
    const call = byId(nodes, "tasks.call");
    expect(call.kind).toBe("task");
    expect(call.expansion).toBe("not_found");
    expect(call.parent).toBeNull();
    expect(nodes.some((n) => n.id === "cluster:tasks.call")).toBe(false);
    // Its `start`/`end` are the task's own uid, not markers: exactly one node carries it.
    expect(nodes.filter((n) => n.id === "tasks.call")).toHaveLength(1);
    expect(edges.map((e) => `${e.source} -> ${e.target}`)).toEqual(expect.arrayContaining(["tasks.b -> tasks.call", "tasks.call -> tasks.call2"]));
    // A Subflow nobody expanded has no reason.
    expect(byId(nodes, "tasks.call2").expansion).toBeUndefined();
  });

  it("marks the trigger chain's edges, and only those", () => {
    const { edges } = layout(FLOW, { orientation: "LR", collapsed: NONE });
    const trig = edges.filter((e) => e.trigger).map((e) => `${e.source} -> ${e.target}`).sort();
    expect(trig).toEqual(["end:triggers -> start:flow", "start:triggers -> triggers.daily", "triggers.daily -> end:triggers"]);
    const collapsed = layout(FLOW, { orientation: "LR", collapsed: new Set(["cluster:triggers"]) }).edges;
    expect(collapsed.filter((e) => e.trigger).map((e) => `${e.source} -> ${e.target}`)).toEqual(["cluster:triggers -> start:flow"]);
  });

  it("routes every edge through at least two points and places a choice label", () => {
    const { edges } = layout(FLOW, { orientation: "TB", collapsed: NONE });
    for (const e of edges) expect(e.points.length).toBeGreaterThanOrEqual(2);
    const then = edges.find((e) => e.relation === "choice" && e.label === "then");
    expect(then?.labelAt).toEqual({ x: expect.any(Number), y: expect.any(Number) });
  });

  it("lays out an empty graph without throwing", () => {
    expect(layout({ nodes: [], edges: [], clusters: [] }, { orientation: "LR", collapsed: NONE })).toEqual({ nodes: [], edges: [] });
    expect(layout(EMPTY_FLOW, { orientation: "TB", collapsed: NONE }).nodes).toHaveLength(2);
  });

  it("ignores an edge whose end is not a node", () => {
    const broken = { ...LINE, edges: [...LINE.edges, { source: "tasks.one", target: "tasks.ghost", relation: "sequential" as const }] };
    expect(layout(broken, { orientation: "LR", collapsed: NONE }).edges.some((e) => e.target === "tasks.ghost")).toBe(false);
  });
});
