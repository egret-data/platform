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
/** A label's box on a `choice` edge; dagre keeps it clear of nodes. */
const LABEL_WIDTH = 64;
const LABEL_HEIGHT = 20;

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

  const g = new graphlib.Graph({ compound: true, multigraph: true });
  g.setGraph({
    rankdir: opts.orientation,
    nodesep: 28,
    ranksep: 44,
    edgesep: 14,
    marginx: 16,
    marginy: 16,
  });
  g.setDefaultEdgeLabel(() => ({}));

  const cardHeight = (n: Node): number => {
    if (n.node !== "task" && n.node !== "trigger") return MARKER_SIZE;
    const facts = nodeFacts(n, opts.states, expansion.get(n.uid));
    return CARD_BASE + LINE * factLines(facts) + (facts.state ? STATE_EXTRA : 0);
  };
  const collapsedHeight = CARD_BASE + (opts.states ? LINE + STATE_EXTRA : 0);
  const items = new Map<string, Omit<Positioned, "x" | "y" | "width" | "height">>();

  // Clusters: an expanded one is a compound node; a collapsed one is a plain node.
  for (const c of drawn.values()) {
    const outer = standIn(parentOf(c.uid));
    if (outer !== null) continue;
    const parent = parentOf(c.uid);
    if (opts.collapsed.has(c.uid)) {
      g.setNode(c.uid, { width: TASK_WIDTH, height: collapsedHeight });
      items.set(c.uid, { id: c.uid, kind: "collapsed", parent, cluster: c });
    } else {
      g.setNode(c.uid, {});
      items.set(c.uid, { id: c.uid, kind: "cluster", parent, cluster: c });
    }
  }
  for (const [uid, item] of items) if (item.parent !== null) g.setParent(uid, item.parent);

  for (const n of graph.nodes) {
    if (nodeRep(n.uid) !== n.uid || items.has(n.uid)) continue;
    const marker = n.node === "start" || n.node === "end";
    g.setNode(n.uid, { width: marker ? MARKER_SIZE : TASK_WIDTH, height: cardHeight(n) });
    const parent = clusterOf.get(n.uid) ?? null;
    if (parent !== null) g.setParent(n.uid, parent);
    items.set(n.uid, { id: n.uid, kind: n.node, parent, node: n, expansion: expansion.get(n.uid) });
  }

  const triggerNodes = new Set(drawn.get("cluster:triggers")?.nodes ?? []);
  const kept: Omit<RoutedEdge, "points" | "labelAt">[] = [];
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
    const trigger = triggerNodes.has(e.source) || triggerNodes.has(e.target);
    kept.push({ id, source, target, relation: e.relation, label, trigger });
    // Only a `choice` edge shows its label; later edges of a branch repeat it for the record.
    const shown = e.relation === "choice" && label !== null;
    g.setEdge(source, target, shown ? { width: LABEL_WIDTH, height: LABEL_HEIGHT, labelpos: "c" } : {}, id);
  }

  dagreLayout(g);

  const boxes = new Map<string, Box>();
  for (const id of items.keys()) {
    const d = g.node(id);
    boxes.set(id, { x: d.x - d.width / 2, y: d.y - d.height / 2, width: d.width, height: d.height });
  }
  // A cluster's box holds its members with room for its header, innermost first.
  const depth = (id: string) => {
    let n = 0;
    for (let p = items.get(id)?.parent ?? null; p !== null; p = items.get(p)?.parent ?? null) n++;
    return n;
  };
  const expanded = [...items.values()].filter((i) => i.kind === "cluster").sort((a, b) => depth(b.id) - depth(a.id));
  for (const c of expanded) {
    const members = [...items.values()].filter((i) => i.parent === c.id).map((i) => boxes.get(i.id) as Box);
    if (members.length === 0) continue;
    const left = Math.min(...members.map((m) => m.x)) - CLUSTER_PAD;
    const top = Math.min(...members.map((m) => m.y)) - CLUSTER_PAD - CLUSTER_HEADER;
    const right = Math.max(...members.map((m) => m.x + m.width)) + CLUSTER_PAD;
    const bottom = Math.max(...members.map((m) => m.y + m.height)) + CLUSTER_PAD;
    const own = boxes.get(c.id) as Box;
    const x = Math.min(left, own.x);
    const y = Math.min(top, own.y);
    boxes.set(c.id, {
      x,
      y,
      width: Math.max(right, own.x + own.width) - x,
      height: Math.max(bottom, own.y + own.height) - y,
    });
  }

  const main = opts.orientation === "LR" ? "x" : "y";
  const cross = opts.orientation === "LR" ? "y" : "x";
  const nodes: Positioned[] = [...items.values()].map((i) => {
    const b = boxes.get(i.id) as Box;
    const p: Positioned = { ...i, x: b.x, y: b.y, width: b.width, height: b.height };
    if (p.expansion === undefined) delete p.expansion;
    return p;
  });
  nodes.sort((a, b) => a[main] - b[main] || a[cross] - b[cross] || depth(a.id) - depth(b.id));

  const edges: RoutedEdge[] = kept.map((e) => {
    const d = g.edge({ v: e.source, w: e.target, name: e.id });
    const routed: RoutedEdge = { ...e, points: ((d.points ?? []) as Point[]).map(({ x, y }) => ({ x, y })) };
    if (e.relation === "choice" && e.label !== null && typeof d.x === "number" && typeof d.y === "number") {
      routed.labelAt = { x: d.x, y: d.y };
    }
    return routed;
  });

  return { nodes, edges };
}
