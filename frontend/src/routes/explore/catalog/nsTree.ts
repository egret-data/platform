import { type NamespaceDto } from "@/api/catalog";

export interface NsNode {
  /** Last path level, e.g. "2024". */
  name: string;
  /** Full dotted path, e.g. "sale.2024". */
  dotted: string;
  /** Path levels. */
  levels: string[];
  /** True if this namespace was in the input (vs an implied intermediate). */
  exists: boolean;
  children: NsNode[];
}

/**
 * Fold a flat list of namespaces (each as its `levels`) into a nested tree,
 * synthesizing any intermediate parents that weren't listed. Children and roots
 * are sorted by name.
 */
export function buildNamespaceTree(list: NamespaceDto[]): NsNode[] {
  const roots: NsNode[] = [];
  const byPath = new Map<string, NsNode>();

  const ensure = (levels: string[], exists: boolean): NsNode => {
    const dotted = levels.join(".");
    const found = byPath.get(dotted);
    if (found) {
      if (exists) found.exists = true;
      return found;
    }
    const node: NsNode = { name: levels[levels.length - 1], dotted, levels, exists, children: [] };
    byPath.set(dotted, node);
    if (levels.length === 1) {
      roots.push(node);
    } else {
      const parent = ensure(levels.slice(0, -1), false);
      parent.children.push(node);
    }
    return node;
  };

  for (const { levels } of list) {
    if (levels.length > 0) ensure(levels, true);
  }

  const sortRec = (nodes: NsNode[]) => {
    nodes.sort((a, b) => a.name.localeCompare(b.name));
    nodes.forEach((n) => sortRec(n.children));
  };
  sortRec(roots);
  return roots;
}

/** Does `dotted` or any descendant path contain `q` (case-insensitive)? */
export function nodeMatchesFilter(node: NsNode, q: string): boolean {
  if (!q) return true;
  const lc = q.toLowerCase();
  const self = node.dotted.toLowerCase().includes(lc);
  return self || node.children.some((c) => nodeMatchesFilter(c, lc));
}
