import type { FlowListItemDto, WorkflowNamespaceDto } from "@/api/generated/model";

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

/**
 * One namespace in the namespaces list's tree (W7d spec §4.1), built from
 * `GET …/namespaces` rows rather than from flows: a namespace can hold only
 * files or KV keys.
 *
 * `label` is the path below the parent node: the last segment, or the full
 * namespace for a root whose parent is not listed (a prefix filter answers
 * `company.team` and below, without `company`). It is `""` for an empty
 * segment (`a.` under `a`); show `EMPTY_SEGMENT_LABEL` then.
 */
export interface NamespaceRowNode {
  label: string;
  path: string;
  row: WorkflowNamespaceDto;
  children: NamespaceRowNode[];
}

/**
 * Builds the namespaces list's tree from the server's flat rows.
 *
 * - A node's parent is its nearest listed ancestor by dots. The server lists
 *   every parent (`own: false`), so that is the direct parent, except at the
 *   top of a filtered list.
 * - Siblings sort by label; the input order is never relied on.
 */
export function buildNamespaceRowTree(rows: WorkflowNamespaceDto[]): NamespaceRowNode[] {
  const byPath = new Map<string, NamespaceRowNode>();
  for (const row of rows) byPath.set(row.namespace, { label: row.namespace, path: row.namespace, row, children: [] });
  const roots: NamespaceRowNode[] = [];
  for (const node of byPath.values()) {
    let parent: NamespaceRowNode | undefined;
    for (let cut = node.path.lastIndexOf("."); cut >= 0 && !parent; cut = node.path.lastIndexOf(".", cut - 1)) {
      parent = byPath.get(node.path.slice(0, cut));
      if (cut === 0) break;
    }
    if (parent) {
      node.label = node.path.slice(parent.path.length + 1);
      parent.children.push(node);
    } else {
      roots.push(node);
    }
  }
  const sort = (nodes: NamespaceRowNode[]) => {
    nodes.sort((a, b) => a.label.localeCompare(b.label));
    for (const n of nodes) sort(n.children);
  };
  sort(roots);
  return roots;
}

/**
 * Each segment of `namespace` with the namespace it ends, for the detail's
 * breadcrumb: `company.team` → `company`, `company.team`. An empty segment is
 * kept (`a..b` → `a`, `a.`, `a..b`), with label `""`.
 */
export function namespaceSegments(namespace: string): { label: string; path: string }[] {
  const parts = namespace.split(".");
  return parts.map((label, i) => ({ label, path: parts.slice(0, i + 1).join(".") }));
}

/**
 * The rows exactly one dot below `namespace`, sorted: the detail's child
 * namespaces. `a.` is a child of `a` (an empty segment), `a.b.c` is not.
 */
export function directChildNamespaces(rows: WorkflowNamespaceDto[], namespace: string): WorkflowNamespaceDto[] {
  return rows
    .filter(
      (r) =>
        r.namespace !== namespace &&
        namespaceContains(namespace, r.namespace) &&
        !r.namespace.slice(namespace.length + 1).includes("."),
    )
    .sort((a, b) => a.namespace.localeCompare(b.namespace));
}
