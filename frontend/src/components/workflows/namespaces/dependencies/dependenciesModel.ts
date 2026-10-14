import { graphlib } from "@dagrejs/dagre";
import type { NamespaceDependenciesDto, NamespaceDependencyNodeDto } from "@/api/generated/model";
import type { CanvasBox } from "@/components/workflows/topology/canvas";
import { NODESEP, RANKSEP, TASK_WIDTH, arrange, type Point, type RoutedEdge } from "@/components/workflows/topology/layout";

/**
 * WHERE EVERY FLOW OF A NAMESPACE'S DEPENDENCIES GRAPH SITS (W7d spec §4.2), a pure function over
 * `GET …/namespaces/{ns}/dependencies`, laid out with W7c's `arrange` (dagre, or its layered
 * fallback past `DAGRE_LIMIT` or when dagre throws). The graph is flat: flows, no clusters.
 *
 * - **Groups.** Each connected group of flows is laid out on its own, left to right along its
 *   edges, and the groups are stacked top to bottom, largest first. Flows with no edge at all are
 *   packed into a grid below them, so a namespace of many unrelated flows is not one long column.
 * - **Order.** The returned `nodes` are in LAYOUT ORDER, which the keyboard walks: group by group,
 *   along the main axis then across it, then the grid row by row.
 * - **Edges.** A `subflow` edge is drawn solid and a `flow_trigger` edge dashed (`trigger`), from
 *   the source card's border to the target's. An edge naming a flow the graph does not have is
 *   dropped; a flow calling or triggering itself is a note on its card, not an edge.
 */

/** A flow's card on the canvas. */
export interface FlowBox extends CanvasBox {
  /** The flow's id (the uuid its routes take), which edges name. */
  id: string;
  flow: NamespaceDependencyNodeDto;
  /** What the card says in words, one per line: `flowBadges`, then a flow calling or triggering itself. */
  badges: string[];
}

/** Sizes, in px: a card's padding and title row, then one line each for the namespace, each badge and the dynamic subflows. */
const CARD_BASE = 44;
const LINE = 16;
/** Room around the whole graph, and between groups. */
const MARGIN = 16;
const GROUP_GAP = 48;

/** A flow's states, in words (never colour alone, FE-A11Y-4). */
export function flowBadges(f: NamespaceDependencyNodeDto): string[] {
  const out: string[] = [];
  if (!f.in_namespace) out.push("Outside this namespace");
  if (f.disabled) out.push("Disabled");
  if (f.unparsable) out.push("Unparsable draft");
  return out;
}

/** `Subflow`s whose target is an expression: only a run names their flow, so they draw no edge. */
export function dynamicSubflowsText(n: number): string | null {
  if (n <= 0) return null;
  return n === 1 ? "1 dynamic subflow" : `${n} dynamic subflows`;
}

/** A flow's name as the other workflow pages spell it: `namespace.flow_id`. */
export const flowName = (f: Pick<NamespaceDependencyNodeDto, "namespace" | "flow_id">): string => `${f.namespace}.${f.flow_id}`;

/** What an edge's kind is called, in the list view and the legend. */
export const EDGE_KIND_WORD = { subflow: "Subflow", flow_trigger: "Flow trigger" } as const;

const cardHeight = (f: NamespaceDependencyNodeDto, badges: number): number =>
  CARD_BASE + LINE * (1 + badges + (f.dynamic_subflows > 0 ? 1 : 0));

export function layoutDependencies(deps: NamespaceDependenciesDto, dagreLimit?: number): { nodes: FlowBox[]; edges: RoutedEdge[] } {
  if (deps.nodes.length === 0) return { nodes: [], edges: [] };
  const index = new Map(deps.nodes.map((f, i) => [f.id, i]));

  // Edges between flows the graph has, once each; a self edge becomes a note.
  const selfNotes = new Map<string, string[]>();
  const kept: { id: string; source: string; target: string; trigger: boolean }[] = [];
  const seen = new Set<string>();
  for (const e of deps.edges) {
    if (!index.has(e.source) || !index.has(e.target)) continue;
    if (e.source === e.target) {
      const notes = selfNotes.get(e.source) ?? [];
      const note = e.kind === "subflow" ? "Calls itself" : "Triggers itself";
      if (!notes.includes(note)) notes.push(note);
      selfNotes.set(e.source, notes);
      continue;
    }
    const id = `${e.source} -> ${e.target} ${e.kind}`;
    if (seen.has(id)) continue;
    seen.add(id);
    kept.push({ id, source: e.source, target: e.target, trigger: e.kind === "flow_trigger" });
  }

  const boxes = new Map<string, FlowBox>();
  for (const f of deps.nodes) {
    const badges = [...flowBadges(f), ...(selfNotes.get(f.id) ?? [])];
    boxes.set(f.id, { id: f.id, flow: f, badges, x: 0, y: 0, width: TASK_WIDTH, height: cardHeight(f, badges.length) });
  }

  // Connected groups (union-find, iterative), in the server's order of their first flow.
  const parent = new Map<string, string>(deps.nodes.map((f) => [f.id, f.id]));
  const find = (v: string): string => {
    let r = v;
    while (parent.get(r) !== r) r = parent.get(r) as string;
    for (let c = v; c !== r; ) {
      const next = parent.get(c) as string;
      parent.set(c, r);
      c = next;
    }
    return r;
  };
  for (const e of kept) {
    const a = find(e.source);
    const b = find(e.target);
    if (a !== b) parent.set(b, a);
  }
  const groups = new Map<string, string[]>();
  for (const f of deps.nodes) {
    const r = find(f.id);
    const list = groups.get(r);
    if (list) list.push(f.id);
    else groups.set(r, [f.id]);
  }
  const connected = [...groups.values()].filter((g) => g.length > 1).sort((a, b) => b.length - a.length);
  const lone = [...groups.values()].filter((g) => g.length === 1).map((g) => g[0]);

  const order: FlowBox[] = [];
  const routes = new Map<string, Point[]>();
  let top = MARGIN;
  let widest = 0;
  for (const members of connected) {
    const inGroup = new Set(members);
    const g = new graphlib.Graph({ multigraph: true });
    g.setGraph({ rankdir: "LR", nodesep: NODESEP, ranksep: RANKSEP * 2, edgesep: 14, marginx: 0, marginy: 0 });
    g.setDefaultEdgeLabel(() => ({}));
    for (const id of members) {
      const b = boxes.get(id) as FlowBox;
      g.setNode(id, { width: b.width, height: b.height });
    }
    const here = kept.filter((e) => inGroup.has(e.source));
    for (const e of here) g.setEdge(e.source, e.target, {}, e.id);
    arrange(g, "LR", dagreLimit);

    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const id of members) {
      const d = g.node(id);
      minX = Math.min(minX, d.x - d.width / 2);
      minY = Math.min(minY, d.y - d.height / 2);
      maxX = Math.max(maxX, d.x + d.width / 2);
      maxY = Math.max(maxY, d.y + d.height / 2);
    }
    const dx = MARGIN - minX;
    const dy = top - minY;
    const placed = members.map((id) => {
      const d = g.node(id);
      const b = boxes.get(id) as FlowBox;
      b.x = d.x - d.width / 2 + dx;
      b.y = d.y - d.height / 2 + dy;
      return b;
    });
    placed.sort((a, b) => a.x - b.x || a.y - b.y || (index.get(a.id) ?? 0) - (index.get(b.id) ?? 0));
    order.push(...placed);
    for (const e of here) {
      const d = g.edge({ v: e.source, w: e.target, name: e.id });
      routes.set(e.id, ((d.points ?? []) as Point[]).map(({ x, y }) => ({ x: x + dx, y: y + dy })));
    }
    widest = Math.max(widest, maxX - minX);
    top += maxY - minY + GROUP_GAP;
  }

  // Flows with no edge: a grid, about square, and at least as wide as the widest group.
  if (lone.length > 0) {
    const cell = TASK_WIDTH + NODESEP;
    const columns = Math.min(lone.length, Math.max(Math.ceil(Math.sqrt(lone.length)), Math.floor((widest + NODESEP) / cell)));
    for (let start = 0; start < lone.length; start += columns) {
      const row = lone.slice(start, start + columns).map((id) => boxes.get(id) as FlowBox);
      row.forEach((b, i) => {
        b.x = MARGIN + i * cell;
        b.y = top;
      });
      order.push(...row);
      top += Math.max(...row.map((b) => b.height)) + NODESEP;
    }
  }

  // A route is drawn from the source's border to the target's: dagre's own ends, or `layered`'s.
  const edges: RoutedEdge[] = kept.map((e) => ({
    id: e.id,
    source: e.source,
    target: e.target,
    relation: "sequential",
    label: null,
    trigger: e.trigger,
    points: routes.get(e.id) ?? [],
  }));
  return { nodes: order, edges };
}
