import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Database, Eye, Plus, Table2, Trash2 } from "lucide-react";
import { useNamespaceContents } from "@/api/catalog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { type NsNode } from "./nsTree";

export const MAX_VISIBLE = 200;

/** All ancestor dotted paths of `ns` (including `ns` itself), or empty. */
function seedFromNs(ns: string | null): Set<string> {
  const set = new Set<string>();
  if (!ns) return set;
  const levels = ns.split(".");
  levels.forEach((_, i) => set.add(levels.slice(0, i + 1).join(".")));
  return set;
}

/**
 * Collect the dotted paths of every node that matches `q` itself or has a
 * descendant that matches, so per-node visibility checks are O(1) lookups
 * instead of re-walking the subtree on every render.
 */
function collectMatches(nodes: NsNode[], q: string, out: Set<string>): boolean {
  let anyHere = false;
  for (const node of nodes) {
    const childMatch = collectMatches(node.children, q, out);
    const selfMatch = node.dotted.toLowerCase().includes(q);
    if (selfMatch || childMatch) {
      out.add(node.dotted);
      anyHere = true;
    }
  }
  return anyHere;
}

export function NamespaceTree({
  ws,
  wid,
  tree,
  selectedNs,
  selectedTable,
  onSelectTable,
  onDeleteNamespace,
  onNewNamespace,
}: {
  ws: string;
  wid: string;
  tree: NsNode[];
  selectedNs: string | null;
  selectedTable: string | null;
  onSelectTable: (ns: string, table: string) => void;
  onDeleteNamespace: (dotted: string) => void;
  onNewNamespace: () => void;
}): JSX.Element {
  const [filter, setFilter] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(() => seedFromNs(selectedNs));

  // Keep the deep-link's ancestors expanded when navigating to a new ns while
  // mounted (e.g. clicking a breadcrumb or a fresh deep-link).
  useEffect(() => {
    if (!selectedNs) return;
    setExpanded((prev) => {
      const next = new Set(prev);
      for (const path of seedFromNs(selectedNs)) next.add(path);
      return next;
    });
  }, [selectedNs]);

  const toggle = (dotted: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(dotted)) next.delete(dotted);
      else next.add(dotted);
      return next;
    });
  };

  // Precompute, once per (tree, filter), the set of dotted paths that match the
  // filter themselves or have a matching descendant — so the per-node expand and
  // child-visibility checks below are O(1) lookups rather than subtree re-walks.
  const hasFilter = filter.trim().length > 0;
  const matchSet = useMemo(() => {
    const set = new Set<string>();
    if (hasFilter) collectMatches(tree, filter.trim().toLowerCase(), set);
    return set;
  }, [tree, filter, hasFilter]);

  // A node is effectively-expanded if explicitly expanded, or (when filtering)
  // it has a descendant match so the match is revealed.
  const isExpanded = (node: NsNode): boolean => {
    if (expanded.has(node.dotted)) return true;
    if (hasFilter && node.children.some((c) => matchSet.has(c.dotted))) return true;
    return false;
  };

  const visibleRoots = hasFilter ? tree.filter((n) => matchSet.has(n.dotted)) : tree;

  return (
    <div className="rounded border border-border">
      <div className="flex items-center justify-between border-b border-border px-3 py-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">
        <span>Namespaces</span>
        <Button
          variant="ghost"
          size="icon"
          className="h-6 w-6 text-muted-foreground"
          onClick={onNewNamespace}
          aria-label="New namespace"
        >
          <Plus className="h-4 w-4" />
        </Button>
      </div>
      <div className="border-b border-border p-2">
        <Input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter namespaces…"
          className="h-8"
        />
      </div>
      <ul className="p-1">
        {visibleRoots.map((node) => (
          <TreeNode
            key={node.dotted}
            ws={ws}
            wid={wid}
            node={node}
            depth={0}
            filter={filter}
            matchSet={matchSet}
            hasFilter={hasFilter}
            isExpanded={isExpanded}
            onToggle={toggle}
            selectedNs={selectedNs}
            selectedTable={selectedTable}
            onSelectTable={onSelectTable}
            onDeleteNamespace={onDeleteNamespace}
          />
        ))}
      </ul>
    </div>
  );
}

function TreeNode({
  ws,
  wid,
  node,
  depth,
  filter,
  matchSet,
  hasFilter,
  isExpanded,
  onToggle,
  selectedNs,
  selectedTable,
  onSelectTable,
  onDeleteNamespace,
}: {
  ws: string;
  wid: string;
  node: NsNode;
  depth: number;
  filter: string;
  matchSet: Set<string>;
  hasFilter: boolean;
  isExpanded: (node: NsNode) => boolean;
  onToggle: (dotted: string) => void;
  selectedNs: string | null;
  selectedTable: string | null;
  onSelectTable: (ns: string, table: string) => void;
  onDeleteNamespace: (dotted: string) => void;
}): JSX.Element {
  const open = isExpanded(node);
  const contents = useNamespaceContents(ws, wid, open ? node.dotted : null);
  const indent = depth * 12 + 8;

  const lc = filter.trim().toLowerCase();
  const matchLeaf = (name: string) => !lc || name.toLowerCase().includes(lc);

  const tables = (contents.data?.tables ?? []).filter(matchLeaf);
  const views = (contents.data?.views ?? []).filter(matchLeaf);
  const leaves = [
    ...tables.map((name) => ({ name, kind: "table" as const })),
    ...views.map((name) => ({ name, kind: "view" as const })),
  ];
  const total = leaves.length;
  const shown = leaves.slice(0, MAX_VISIBLE);

  // When filtering, child namespaces are themselves filtered (O(1) set lookup).
  const childNodes = hasFilter
    ? node.children.filter((c) => matchSet.has(c.dotted))
    : node.children;

  return (
    <li>
      <div className="group flex items-center gap-1" style={{ paddingLeft: indent }}>
        <button
          type="button"
          onClick={() => onToggle(node.dotted)}
          className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-accent/50 hover:text-foreground"
          aria-label={`${open ? "Collapse" : "Expand"} ${node.dotted}`}
        >
          {open ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
        </button>
        <span
          className={`flex min-w-0 flex-1 items-center gap-2 rounded px-2 py-1.5 text-left text-sm ${
            node.exists ? "text-muted-foreground" : "text-muted-foreground/50"
          }`}
        >
          <Database className="h-4 w-4 shrink-0" />
          <span className="truncate font-mono">{node.name}</span>
        </span>
        {node.exists && (
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7 shrink-0 text-muted-foreground opacity-0 hover:text-destructive group-hover:opacity-100"
            onClick={() => onDeleteNamespace(node.dotted)}
            aria-label={`Delete ${node.dotted}`}
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        )}
      </div>

      {open && (
        <ul>
          {childNodes.map((child) => (
            <TreeNode
              key={child.dotted}
              ws={ws}
              wid={wid}
              node={child}
              depth={depth + 1}
              filter={filter}
              matchSet={matchSet}
              hasFilter={hasFilter}
              isExpanded={isExpanded}
              onToggle={onToggle}
              selectedNs={selectedNs}
              selectedTable={selectedTable}
              onSelectTable={onSelectTable}
              onDeleteNamespace={onDeleteNamespace}
            />
          ))}

          {contents.isLoading ? (
            <li style={{ paddingLeft: (depth + 1) * 12 + 8 }} className="py-1">
              <ListSkeleton rows={2} cols={1} />
            </li>
          ) : contents.isError ? (
            <li
              style={{ paddingLeft: (depth + 1) * 12 + 8 }}
              className="py-1 text-sm text-destructive"
            >
              Failed to load tables.
            </li>
          ) : (
            <>
              {shown.map((leaf) =>
                leaf.kind === "table" ? (
                  <li key={`t:${leaf.name}`}>
                    <button
                      type="button"
                      onClick={() => onSelectTable(node.dotted, leaf.name)}
                      style={{ paddingLeft: (depth + 1) * 12 + 8 + 24 }}
                      className={`flex w-full items-center gap-2 rounded py-1.5 pr-2 text-left text-sm transition-colors ${
                        selectedNs === node.dotted && selectedTable === leaf.name
                          ? "bg-accent text-accent-foreground"
                          : "text-muted-foreground hover:bg-accent/50 hover:text-foreground"
                      }`}
                    >
                      <Table2 className="h-4 w-4 shrink-0" />
                      <span className="truncate font-mono">{leaf.name}</span>
                    </button>
                  </li>
                ) : (
                  <li
                    key={`v:${leaf.name}`}
                    style={{ paddingLeft: (depth + 1) * 12 + 8 + 24 }}
                    className="flex items-center gap-2 py-1.5 pr-2 text-sm text-muted-foreground"
                  >
                    <Eye className="h-4 w-4 shrink-0" />
                    <span className="truncate font-mono">{leaf.name}</span>
                  </li>
                ),
              )}
              {total > MAX_VISIBLE && (
                <li
                  style={{ paddingLeft: (depth + 1) * 12 + 8 + 24 }}
                  className="py-1 text-xs text-muted-foreground"
                >
                  Showing {MAX_VISIBLE} of {total} — refine search
                </li>
              )}
            </>
          )}
        </ul>
      )}
    </li>
  );
}
