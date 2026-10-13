import { describe, expect, it } from "vitest";
import { CLUSTER_HEADER, layout, type Positioned } from "@/components/workflows/topology/layout";
import { EMPTY_FLOW, EXPANDED, FLOW, LINE, NESTED, NESTED_FAILED, chain, siblings } from "./fixtures";

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

  describe("geometry: clusters reserve their room", () => {
    const intersects = (a: Positioned, b: Positioned) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
    const ancestors = (nodes: Positioned[], p: Positioned): string[] => {
      const out: string[] = [];
      for (let c = p.parent; c !== null; c = nodes.find((n) => n.id === c)?.parent ?? null) out.push(c);
      return out;
    };
    function assertGeometry(nodes: Positioned[]) {
      const problems: string[] = [];
      const boxes = nodes.filter((n) => n.kind === "cluster");
      const cards = nodes.filter((n) => n.kind !== "cluster");
      // Every member lies inside its cluster's box, below the header.
      for (const n of nodes) {
        if (n.parent === null) continue;
        const box = byId(nodes, n.parent);
        if (n.x < box.x || n.x + n.width > box.x + box.width || n.y < box.y + CLUSTER_HEADER || n.y + n.height > box.y + box.height) {
          problems.push(`${n.id} outside ${box.id}`);
        }
      }
      // Two boxes intersect only when one contains the other.
      for (let i = 0; i < boxes.length; i++)
        for (let j = i + 1; j < boxes.length; j++) {
          const [a, b] = [boxes[i], boxes[j]];
          const nested = ancestors(nodes, a).includes(b.id) || ancestors(nodes, b).includes(a.id);
          if (!nested && intersects(a, b)) problems.push(`box ${a.id} x ${b.id}`);
        }
      // No card or marker overlaps another, nor a box it is not inside.
      for (let i = 0; i < cards.length; i++) {
        for (let j = i + 1; j < cards.length; j++) if (intersects(cards[i], cards[j])) problems.push(`card ${cards[i].id} x ${cards[j].id}`);
        const mine = ancestors(nodes, cards[i]);
        for (const b of boxes) if (!mine.includes(b.id) && intersects(cards[i], b)) problems.push(`card ${cards[i].id} x box ${b.id}`);
      }
      expect(problems).toEqual([]);
    }

    it.each([
      ["three sibling Sequentials", siblings(3)],
      ["sibling Sequentials each holding an If", siblings(3, true)],
      ["the mixed flow", FLOW],
      ["a Sequential holding a Parallel", NESTED],
      ["an expanded Subflow", EXPANDED],
    ])("%s, in both orientations", (_name, graph) => {
      for (const orientation of ["LR", "TB"] as const) {
        const { nodes } = layout(graph, { orientation, collapsed: NONE });
        expect(nodes.filter((n) => n.kind === "cluster").length).toBeGreaterThan(0);
        assertGeometry(nodes);
      }
    });

    it.each([
      ["three sibling Sequentials", siblings(3)],
      ["sibling Sequentials each holding an If", siblings(3, true)],
      ["the mixed flow", FLOW],
    ])("%s, laid out without dagre (the fallback), in both orientations", (_name, graph) => {
      for (const orientation of ["LR", "TB"] as const) {
        const { nodes, edges } = layout(graph, { orientation, collapsed: NONE, dagreLimit: 0 });
        assertGeometry(nodes);
        for (const e of edges) expect(e.points.length).toBeGreaterThanOrEqual(2);
      }
    });

    it("keeps the geometry with one sibling collapsed", () => {
      for (const orientation of ["LR", "TB"] as const) {
        assertGeometry(layout(siblings(3, true), { orientation, collapsed: new Set(["cluster:tasks.p.s2"]) }).nodes);
      }
    });
  });

  it("lays an expanded Subflow's child out inside the Subflow's cluster and routes its edges", () => {
    const { nodes, edges } = layout(EXPANDED, { orientation: "LR", collapsed: NONE });
    expect(byId(nodes, "tasks.call").parent).toBe("cluster:tasks.call");
    expect(byId(nodes, "cluster:tasks.call/tasks.p").parent).toBe("cluster:tasks.call");
    expect(byId(nodes, "tasks.call/tasks.p.x").parent).toBe("cluster:tasks.call/tasks.p");
    expect(byId(nodes, "tasks.call/triggers.hook").parent).toBe("cluster:tasks.call/triggers");
    const pairs = edges.map((e) => `${e.source} -> ${e.target}`);
    expect(pairs).toEqual(
      expect.arrayContaining(["tasks.call -> start:tasks.call/tasks.p", "tasks.call/tasks.p -> tasks.call/tasks.p.x", "end:tasks.call/tasks.p -> end:tasks.call"]),
    );
    for (const e of edges) expect(e.points.length).toBeGreaterThanOrEqual(2);
  });

  it("dashes the trigger chain of an expanded Subflow's child too", () => {
    const { edges } = layout(EXPANDED, { orientation: "LR", collapsed: NONE });
    expect(edges.filter((e) => e.trigger).map((e) => `${e.source} -> ${e.target}`).sort()).toEqual([
      "end:tasks.call/triggers -> tasks.call",
      "start:tasks.call/triggers -> tasks.call/triggers.hook",
      "tasks.call/triggers.hook -> end:tasks.call/triggers",
    ]);
  });

  it("keeps a failed Subflow nested in a cluster inside that cluster, carrying the reason", () => {
    const { nodes } = layout(NESTED_FAILED, { orientation: "TB", collapsed: NONE });
    const call = byId(nodes, "tasks.s.call");
    expect(call.kind).toBe("task");
    expect(call.parent).toBe("cluster:tasks.s");
    expect(call.expansion).toBe("depth");
    expect(nodes.some((n) => n.id === "cluster:tasks.s.call")).toBe(false);
  });

  describe("graphs dagre cannot take", () => {
    it.each(["LR", "TB"] as const)("lays out a chain of 5,000 tasks in %s, in order, without overlap", (orientation) => {
      const axis = orientation === "LR" ? "x" : "y";
      const size = orientation === "LR" ? "width" : "height";
      const { nodes, edges } = layout(chain(5000), { orientation, collapsed: NONE });
      expect(nodes).toHaveLength(5002);
      expect(edges).toHaveLength(5001);
      const ids = ["start:flow", ...Array.from({ length: 5000 }, (_, i) => `tasks.t${i}`), "end:flow"];
      const placed = ids.map((id) => byId(nodes, id));
      // Each one ends before the next begins along the main axis: in order, and no two overlap.
      for (let i = 1; i < placed.length; i++) expect(placed[i][axis]).toBeGreaterThan(placed[i - 1][axis] + placed[i - 1][size]);
      // The returned order is still the layout's.
      expect(nodes.filter((n) => n.kind === "task").map((n) => n.id)).toEqual(ids.slice(1, -1));
    });

    it("the fallback places the same nodes in the same order, with straight routes", () => {
      // Below the limit, a graph dagre can lay out is laid out by dagre: a different route shape.
      const byDagre = layout(LINE, { orientation: "LR", collapsed: NONE });
      const byFallback = layout(LINE, { orientation: "LR", collapsed: NONE, dagreLimit: 0 });
      expect(byFallback.nodes.map((n) => n.id)).toEqual(byDagre.nodes.map((n) => n.id));
      for (const e of byFallback.edges) expect(e.points).toHaveLength(2);
    });
  });
});

