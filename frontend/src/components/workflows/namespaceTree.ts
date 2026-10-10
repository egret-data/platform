import type { FlowListItemDto } from "@/api/generated/model";

/**
 * One namespace in the flow list's tree (spec §4).
 *
 * `flows` holds only the flows whose namespace IS `path`; a namespace's
 * descendants carry their own. `total` counts both, so a collapsed header can
 * still say how much it is hiding.
 */
export interface NamespaceNode {
  /** The last dotted segment: `team` for `company.team`. */
  segment: string;
  /** The full namespace: `company.team`. */
  path: string;
  flows: FlowListItemDto[];
  children: NamespaceNode[];
  total: number;
}

/**
 * Builds the namespace tree from a flat flow list, client-side, splitting each
 * namespace on `.`.
 *
 * - An intermediate namespace no flow lives in directly still gets a node, so
 *   `a.b.c` alone yields `a` → `a.b` → `a.b.c`.
 * - Siblings sort by segment and a node's own flows by `flow_id`. The input
 *   order is never relied on: `GET …/flows` promises none.
 */
export function buildNamespaceTree(flows: FlowListItemDto[]): NamespaceNode[] {
  const root: NamespaceNode = { segment: "", path: "", flows: [], children: [], total: 0 };
  const byPath = new Map<string, NamespaceNode>();

  const nodeFor = (path: string): NamespaceNode => {
    const existing = byPath.get(path);
    if (existing) return existing;
    const cut = path.lastIndexOf(".");
    const parent = cut < 0 ? root : nodeFor(path.slice(0, cut));
    const node: NamespaceNode = { segment: path.slice(cut + 1), path, flows: [], children: [], total: 0 };
    parent.children.push(node);
    byPath.set(path, node);
    return node;
  };

  for (const f of flows) nodeFor(f.namespace).flows.push(f);

  const finish = (node: NamespaceNode): number => {
    node.flows.sort((a, b) => a.flow_id.localeCompare(b.flow_id));
    node.children.sort((a, b) => a.segment.localeCompare(b.segment));
    node.total = node.flows.length + node.children.reduce((n, c) => n + finish(c), 0);
    return node.total;
  };
  finish(root);
  return root.children;
}

/**
 * Whether `namespace` is `parent` or lies beneath it. Segment-anchored:
 * `companyx` is not inside `company`.
 */
export function namespaceContains(parent: string, namespace: string): boolean {
  return namespace === parent || namespace.startsWith(`${parent}.`);
}

/**
 * Every namespace a picker can offer: each flow's namespace and each of its
 * ancestors, once, sorted. An ancestor is offered because choosing it selects
 * its whole subtree (`namespaceContains`).
 */
export function knownNamespaces(flows: FlowListItemDto[]): string[] {
  const out = new Set<string>();
  for (const f of flows) {
    const parts = f.namespace.split(".");
    for (let i = 1; i <= parts.length; i++) out.add(parts.slice(0, i).join("."));
  }
  return [...out].sort((a, b) => a.localeCompare(b));
}

/**
 * What a header shows for an empty segment. The backend's namespace pattern
 * (`^[a-z0-9][a-z0-9._-]*`, `spec/validate.rs`) admits `a..b` and `a.`, whose
 * nodes have segment `""`; without this the header would be a bare chevron
 * and a count. A leading dot cannot happen.
 *
 * Also a bundle-test marker (`scripts/assert-workflows-lazy.mjs`): the one
 * string literal in this module that survives minification.
 */
export const EMPTY_SEGMENT_LABEL = "(empty segment)";
