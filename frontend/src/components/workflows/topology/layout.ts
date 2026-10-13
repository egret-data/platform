import { graphlib, layout as dagreLayout } from "@dagrejs/dagre";
import type { Cluster, ExpansionReason, FlowGraph, Node, NodeState, Relation } from "@/api/generated/model";
import { factLines, nodeFacts } from "@/components/workflows/topology/topologyModel";

/**
 * WHERE EVERY NODE OF A FLOW GRAPH SITS (W7c spec §6.1): a pure function over
 * the server's graph, laid out by `@dagrejs/dagre`, so it is tested without a
 * browser (jsdom has no layout, but this needs none).
 *
 * The graph's contract is the Rust module doc of
 * `crates/egret-workflow/src/graph/mod.rs` and `graph/expand.rs`:
 *
 * - A cluster's `nodes` are the nodes whose INNERMOST cluster it is; a nested
 *   cluster names its container in `parent`. Nodes in no cluster belong to the
 *   flow and are drawn at the top level.
 * - **A `subflow` cluster whose expansion failed** carries a `reason`, has no
 *   nodes and no markers, and its `start`/`end` are the `Subflow` task's own
 *   uid, which stays in its parent cluster. Such a cluster is not drawn as a
 *   box: the reason travels on the task node (`Positioned.expansion`).
 * - A trigger edge is an edge with an endpoint in the `triggers` cluster.
 *
 * A collapsed cluster becomes one node standing in for everything inside it,
 * nested clusters included; an edge with an end inside is rerouted to it, and
 * an edge with both ends inside disappears.
 *
 * Coordinates are absolute, in the graph's own pixels, top-left corner. The
 * returned `nodes` are in LAYOUT ORDER: along the main axis, then across it.
 * A cluster's box starts before its members, so a container always precedes
 * what it contains (which `@xyflow/react` requires of parent nodes), and the
 * keyboard walks this order.
 */

export type Orientation = "LR" | "TB";

export type PositionedKind = "task" | "trigger" | "start" | "end" | "cluster" | "collapsed";

export interface Positioned {
  /** The node's uid, or the cluster's (`cluster:{owner}`) for a box or a collapsed cluster. */
  id: string;
  kind: PositionedKind;
  x: number;
  y: number;
  width: number;
  height: number;
  /** The expanded cluster drawn around this one, or `null` at the flow's level. */
  parent: string | null;
  /** The graph's node, for `task`, `trigger`, `start` and `end`. */
  node?: Node;
  /** The cluster, for `cluster` and `collapsed`. */
  cluster?: Cluster;
  /** Why this `Subflow` task did not expand, when it did not. */
  expansion?: ExpansionReason;
}

export interface Point {
  x: number;
  y: number;
}

export interface RoutedEdge {
  id: string;
  source: string;
  target: string;
  relation: Relation;
  label: string | null;
  /** Part of the trigger chain: drawn dashed. */
  trigger: boolean;
  /** The route, from the source's border to the target's. */
  points: Point[];
  /** Where a label sits, when the edge has one. */
  labelAt?: Point;
}

export interface LayoutOptions {
  orientation: Orientation;
  /**
   * The most members one container hands to dagre; a larger one is laid out by `layered`.
   * Defaults to `DAGRE_LIMIT`; tests lower it to run the fallback on small graphs.
   */
  dagreLimit?: number;
  /** Cluster uids (`cluster:{owner}`) drawn collapsed. */
  collapsed: ReadonlySet<string>;
  /**
   * An execution graph's states. A card's height follows what it says (a state
   * row, iterations, attempts), so the layout reads them; the editor's graph has none.
   */
  states?: Readonly<Record<string, NodeState>>;
}

/**
 * Sizes, in px. A card is `CARD_BASE` (its padding and title row, which holds
 * a 24 px button) plus `LINE` per line of `factLines`; the state row holds a
 * chip, so it takes `STATE_EXTRA` more. At 240 px a card fits a 26-character
 * id at `text-sm` and "2 of 3 iterations done, 1 failed" at `text-xs`.
 */
export const TASK_WIDTH = 240;
const CARD_BASE = 44;
const LINE = 16;
const STATE_EXTRA = 8;
const MARKER_SIZE = 14;
/** Room inside a cluster's box: the header with its title and collapse button, then a margin. */
export const CLUSTER_HEADER = 32;
const CLUSTER_PAD = 14;
/** A cluster's box is at least this wide inside, so its header (word, id, button) fits. */
const CLUSTER_MIN_WIDTH = 160;
/** Room around the whole graph. */
const MARGIN = 16;
/** A label's box on a `choice` edge; dagre keeps it clear of nodes. */
const LABEL_WIDTH = 64;
const LABEL_HEIGHT = 20;

/**
 * DAGRE'S LIMIT. dagre ranks, orders and makes a graph acyclic with RECURSIVE depth-first
 * searches (`dfsFAS`, `longestPath`, `tightTree`, …), one stack frame chain per node along a
 * path, whatever the `ranker`. Measured on 3.1.1 in node 24: a chain of 2,000 nodes overflows the
 * default stack with every ranker, and with half the stack (`--stack-size=500`) 1,200 already
 * does while 800 does not. The server allows 5,000 nodes, all of which can sit in one container
 * (a flat flow, one long `Sequential`). It is slow before it is fatal, too: a 300-node chain
 * takes ~250 ms, a 1,200-node one ~0.9 s, on the main thread.
 *
 * So a container with more than `DAGRE_LIMIT` members skips dagre for `layered` below, as does
 * one dagre throws on. 300 keeps dagre's cost near a quarter second and its depth well inside
 * even a halved stack.
 */
export const DAGRE_LIMIT = 300;
const NODESEP = 28;
const RANKSEP = 44;
/** `layered` reserves no box for an edge label, so its ranks sit far enough apart for one. */
const LAYERED_RANKSEP = 88;

/**
 * A plain layered layout with no recursion, for what dagre cannot take: ranks by longest path
 * (Kahn's order, iterative), each rank ordered once by the mean position of its predecessors,
 * nodes stacked across the rank `NODESEP` apart, ranks `LAYERED_RANKSEP` apart, and every edge a
 * straight segment from border to border. It writes the same fields dagre does (`x`, `y` on
 * nodes; `points`, `x`, `y` on edges), so the caller reads either the same way. Nodes on a cycle
 * (the server draws none) are ranked after the rest rather than dropped.
 */
function layered(g: graphlib.Graph, orientation: Orientation): void {
  const lr = orientation === "LR";
  const along = (v: string): number => (lr ? g.node(v).width : g.node(v).height);
  const across = (v: string): number => (lr ? g.node(v).height : g.node(v).width);
  const nodes = g.nodes();
  const edges = g.edges().filter((e) => e.v !== e.w);
  const indegree = new Map<string, number>(nodes.map((v) => [v, 0]));
  const out = new Map<string, string[]>(nodes.map((v) => [v, []]));
  const into = new Map<string, string[]>(nodes.map((v) => [v, []]));
  for (const e of edges) {
    indegree.set(e.w, (indegree.get(e.w) ?? 0) + 1);
    out.get(e.v)?.push(e.w);
    into.get(e.w)?.push(e.v);
  }
  const rank = new Map<string, number>();
  const queue = nodes.filter((v) => indegree.get(v) === 0);
  for (const v of queue) rank.set(v, 0);
  for (let i = 0; i < queue.length; i++) {
    const v = queue[i];
    for (const w of out.get(v) ?? []) {
      rank.set(w, Math.max(rank.get(w) ?? 0, (rank.get(v) ?? 0) + 1));
      const left = (indegree.get(w) ?? 0) - 1;
      indegree.set(w, left);
      if (left === 0) queue.push(w);
    }
  }
  let last = -1;
  for (const r of rank.values()) last = Math.max(last, r);
  const ordered = new Set(queue);
  for (const v of nodes) if (!ordered.has(v)) rank.set(v, last + 1);

  const layers: string[][] = [];
  for (const v of nodes) (layers[rank.get(v) ?? 0] ??= []).push(v);
  const position = new Map<string, number>();
  let mainAt = 0;
  for (const layer of layers) {
    if (!layer) continue;
    const mean = (v: string) => {
      const known = (into.get(v) ?? []).map((u) => position.get(u)).filter((p): p is number => p !== undefined);
      return known.length ? known.reduce((a, b) => a + b, 0) / known.length : Number.POSITIVE_INFINITY;
    };
    const order = layer.map((v, i) => ({ v, i, m: mean(v) })).sort((a, b) => a.m - b.m || a.i - b.i);
    order.forEach(({ v }, i) => position.set(v, i));
    const thickness = Math.max(...layer.map(along));
    let crossAt = -(order.reduce((sum, { v }) => sum + across(v), 0) + NODESEP * (order.length - 1)) / 2;
    for (const { v } of order) {
      const main = mainAt + thickness / 2;
      const cross = crossAt + across(v) / 2;
      Object.assign(g.node(v), lr ? { x: main, y: cross } : { x: cross, y: main });
      crossAt += across(v) + NODESEP;
    }
    mainAt += thickness + LAYERED_RANKSEP;
  }

  for (const e of g.edges()) {
    const a = g.node(e.v);
    const b = g.node(e.w);
    const forward = (rank.get(e.w) ?? 0) >= (rank.get(e.v) ?? 0) ? 1 : -1;
    const from = lr ? { x: a.x + (forward * a.width) / 2, y: a.y } : { x: a.x, y: a.y + (forward * a.height) / 2 };
    const to = lr ? { x: b.x - (forward * b.width) / 2, y: b.y } : { x: b.x, y: b.y - (forward * b.height) / 2 };
    Object.assign(g.edge(e), { points: [from, to], x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 });
  }
}

interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export function layout(graph: FlowGraph, opts: LayoutOptions): { nodes: Positioned[]; edges: RoutedEdge[] } {
  if (graph.nodes.length === 0) return { nodes: [], edges: [] };

  // Clusters that are drawn: every cluster but a failed `Subflow` expansion.
  const drawn = new Map<string, Cluster>();
  const expansion = new Map<string, ExpansionReason>();
  for (const c of graph.clusters) {
    if (c.reason) expansion.set(c.start, c.reason);
    else drawn.set(c.uid, c);
  }
  const clusterOf = new Map<string, string>();
  for (const c of drawn.values()) for (const n of c.nodes) clusterOf.set(n, c.uid);
  const parentOf = (clusterUid: string): string | null => {
    const p = drawn.get(clusterUid)?.parent ?? null;
    return p !== null && drawn.has(p) ? p : null;
  };

  // The outermost collapsed cluster around a node or cluster, if any: what it is drawn as.
  const standIn = (container: string | null): string | null => {
    let found: string | null = null;
    for (let c = container; c !== null; c = parentOf(c)) if (opts.collapsed.has(c)) found = c;
    return found;
  };
  const nodeRep = (uid: string): string => standIn(clusterOf.get(uid) ?? null) ?? uid;

  const cardHeight = (n: Node): number => {
    if (n.node !== "task" && n.node !== "trigger") return MARKER_SIZE;
    const facts = nodeFacts(n, opts.states, expansion.get(n.uid));
    return CARD_BASE + LINE * factLines(facts) + (facts.state ? STATE_EXTRA : 0);
  };
  const collapsedHeight = CARD_BASE + (opts.states ? LINE + STATE_EXTRA : 0);
  type Item = Omit<Positioned, "x" | "y" | "width" | "height">;
  const items = new Map<string, Item>();
  /** Leaf sizes; an expanded cluster's size comes from laying out its members. */
  const leafSize = new Map<string, { width: number; height: number }>();

  for (const c of drawn.values()) {
    if (standIn(parentOf(c.uid)) !== null) continue;
    const parent = parentOf(c.uid);
    if (opts.collapsed.has(c.uid)) {
      leafSize.set(c.uid, { width: TASK_WIDTH, height: collapsedHeight });
      items.set(c.uid, { id: c.uid, kind: "collapsed", parent, cluster: c });
    } else {
      items.set(c.uid, { id: c.uid, kind: "cluster", parent, cluster: c });
    }
  }
  for (const n of graph.nodes) {
    if (nodeRep(n.uid) !== n.uid || items.has(n.uid)) continue;
    const marker = n.node === "start" || n.node === "end";
    leafSize.set(n.uid, { width: marker ? MARKER_SIZE : TASK_WIDTH, height: cardHeight(n) });
    items.set(n.uid, { id: n.uid, kind: n.node, parent: clusterOf.get(n.uid) ?? null, node: n, expansion: expansion.get(n.uid) });
  }

  // The chain of containers around an item, innermost first, ending with the flow (`null`).
  const chain = (id: string): (string | null)[] => {
    const out: (string | null)[] = [];
    for (let c = items.get(id)?.parent ?? null; ; c = items.get(c)?.parent ?? null) {
      out.push(c);
      if (c === null) return out;
    }
  };
  // The item that stands for `id` among the direct members of `container`.
  const memberOf = (id: string, container: string | null): string => {
    let m = id;
    while ((items.get(m)?.parent ?? null) !== container) m = items.get(m)?.parent as string;
    return m;
  };

  // Every trigger cluster: the flow's, and an expanded Subflow child's (`cluster:{subflow}/triggers`).
  const triggerNodes = new Set([...drawn.values()].filter((c) => c.kind === "triggers").flatMap((c) => c.nodes));
  interface Kept extends Omit<RoutedEdge, "points" | "labelAt"> {
    /** The container whose own layout routes this edge: the innermost one holding both ends. */
    level: string | null;
  }
  const kept: Kept[] = [];
  const seen = new Set<string>();
  for (const e of graph.edges) {
    if (!items.has(e.source) && !clusterOf.has(e.source)) continue;
    if (!items.has(e.target) && !clusterOf.has(e.target)) continue;
    const source = nodeRep(e.source);
    const target = nodeRep(e.target);
    if (source === target || !items.has(source) || !items.has(target)) continue;
    const label = e.label ?? null;
    const id = `${source} -> ${target} ${e.relation} ${label ?? ""}`;
    if (seen.has(id)) continue;
    seen.add(id);
    const around = chain(target);
    const level = chain(source).find((c) => around.includes(c)) ?? null;
    const trigger = triggerNodes.has(e.source) || triggerNodes.has(e.target);
    kept.push({ id, source, target, relation: e.relation, label, trigger, level });
  }

  /*
   * ONE DAGRE LAYOUT PER CONTAINER, innermost first. A cluster is laid out on its own, then sized
   * as its content plus a header and padding, and placed as a plain node in its parent's layout.
   * So the header's room is reserved before anything is placed around it, in either orientation.
   * (dagre's own compound nodes do not: its border nodes are zero-sized dummies, so in LR, where a
   * header lies across the ranks, siblings sat `edgesep` apart and a grown box overlapped the next.)
   * An edge is routed in the innermost container holding both ends, between the members that hold
   * them; the server enters a cluster at its `start:` marker and leaves at its `end:`, so the route
   * is then extended from the box's border to that marker.
   */
  const members = new Map<string | null, string[]>();
  for (const i of items.values()) {
    const list = members.get(i.parent);
    if (list) list.push(i.id);
    else members.set(i.parent, [i.id]);
  }
  /** Positions relative to the container's content origin, and each level's routes. */
  const rel = new Map<string, Box>();
  const routes = new Map<string, { points: Point[]; labelAt?: Point }>();
  const contentSize = new Map<string | null, { width: number; height: number }>();

  const place = (container: string | null): { width: number; height: number } => {
    const g = new graphlib.Graph({ multigraph: true });
    g.setGraph({ rankdir: opts.orientation, nodesep: NODESEP, ranksep: RANKSEP, edgesep: 14, marginx: 0, marginy: 0 });
    g.setDefaultEdgeLabel(() => ({}));
    for (const id of members.get(container) ?? []) {
      g.setNode(id, items.get(id)?.kind === "cluster" ? boxSize(id) : (leafSize.get(id) as { width: number; height: number }));
    }
    const here = kept.filter((e) => e.level === container);
    for (const e of here) {
      // Only a `choice` edge shows its label; later edges of a branch repeat it for the record.
      const shown = e.relation === "choice" && e.label !== null;
      g.setEdge(memberOf(e.source, container), memberOf(e.target, container), shown ? { width: LABEL_WIDTH, height: LABEL_HEIGHT, labelpos: "c" } : {}, e.id);
    }
    if (g.nodeCount() > (opts.dagreLimit ?? DAGRE_LIMIT)) layered(g, opts.orientation);
    else {
      try {
        dagreLayout(g);
      } catch {
        // dagre lays out a copy and writes back only at the end, so `g` is untouched here.
        layered(g, opts.orientation);
      }
    }
    // Shift so the content starts at 0,0: nodes, routes and labels together.
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    const grow = (x0: number, y0: number, x1: number, y1: number) => {
      minX = Math.min(minX, x0);
      minY = Math.min(minY, y0);
      maxX = Math.max(maxX, x1);
      maxY = Math.max(maxY, y1);
    };
    for (const id of g.nodes()) {
      const d = g.node(id);
      grow(d.x - d.width / 2, d.y - d.height / 2, d.x + d.width / 2, d.y + d.height / 2);
    }
    if (minX === Infinity) return { width: 0, height: 0 };
    for (const id of g.nodes()) {
      const d = g.node(id);
      rel.set(id, { x: d.x - d.width / 2 - minX, y: d.y - d.height / 2 - minY, width: d.width, height: d.height });
    }
    for (const e of here) {
      const d = g.edge({ v: memberOf(e.source, container), w: memberOf(e.target, container), name: e.id });
      const route: { points: Point[]; labelAt?: Point } = {
        points: ((d.points ?? []) as Point[]).map(({ x, y }) => ({ x: x - minX, y: y - minY })),
      };
      if (e.relation === "choice" && e.label !== null && typeof d.x === "number" && typeof d.y === "number") {
        route.labelAt = { x: d.x - minX, y: d.y - minY };
      }
      routes.set(e.id, route);
    }
    const size = { width: maxX - minX, height: maxY - minY };
    contentSize.set(container, size);
    return size;
  };
  const sized = new Map<string, { width: number; height: number }>();
  function boxSize(cluster: string): { width: number; height: number } {
    let s = sized.get(cluster);
    if (!s) {
      const c = place(cluster);
      s = { width: Math.max(c.width, CLUSTER_MIN_WIDTH) + 2 * CLUSTER_PAD, height: c.height + CLUSTER_HEADER + 2 * CLUSTER_PAD };
      sized.set(cluster, s);
    }
    return s;
  }
  place(null);

  // Absolute boxes, outermost first: a member sits at its container's content origin plus its offset.
  const boxes = new Map<string, Box>();
  const origin = (container: string | null): Point => {
    if (container === null) return { x: MARGIN, y: MARGIN };
    const b = absolute(container);
    const inner = (contentSize.get(container) ?? { width: 0 }).width;
    // A content narrower than the header's minimum is centred in the box.
    const extra = Math.max(0, CLUSTER_MIN_WIDTH - inner) / 2;
    return { x: b.x + CLUSTER_PAD + extra, y: b.y + CLUSTER_HEADER + CLUSTER_PAD };
  };
  function absolute(id: string): Box {
    let b = boxes.get(id);
    if (!b) {
      const r = rel.get(id) as Box;
      const o = origin(items.get(id)?.parent ?? null);
      b = { x: r.x + o.x, y: r.y + o.y, width: r.width, height: r.height };
      boxes.set(id, b);
    }
    return b;
  }
  for (const id of items.keys()) absolute(id);
  const depth = (id: string) => chain(id).length - 1;
  const centre = (id: string): Point => {
    const b = boxes.get(id) as Box;
    return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
  };

  const main = opts.orientation === "LR" ? "x" : "y";
  const cross = opts.orientation === "LR" ? "y" : "x";
  const nodes: Positioned[] = [...items.values()].map((i) => {
    const b = boxes.get(i.id) as Box;
    const p: Positioned = { ...i, x: b.x, y: b.y, width: b.width, height: b.height };
    if (p.expansion === undefined) delete p.expansion;
    return p;
  });
  nodes.sort((a, b) => a[main] - b[main] || a[cross] - b[cross] || depth(a.id) - depth(b.id));

  const edges: RoutedEdge[] = kept.map(({ level, ...e }) => {
    const o = origin(level);
    const route = routes.get(e.id) ?? { points: [] };
    const points = route.points.map((p) => ({ x: p.x + o.x, y: p.y + o.y }));
    // Into and out of a cluster: from the box's border to the marker inside it.
    if (memberOf(e.source, level) !== e.source) points.unshift(centre(e.source));
    if (memberOf(e.target, level) !== e.target) points.push(centre(e.target));
    const routed: RoutedEdge = { ...e, points };
    if (route.labelAt) routed.labelAt = { x: route.labelAt.x + o.x, y: route.labelAt.y + o.y };
    return routed;
  });

  return { nodes, edges };
}
