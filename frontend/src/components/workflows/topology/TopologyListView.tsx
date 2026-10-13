import { useRef, useState, type KeyboardEvent } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { Cluster, FlowGraph, NodeState } from "@/api/generated/model";
import { IconButton } from "@/components/ui/icon-button";
import { StateChip } from "@/components/workflows/StateChip";
import type { Positioned } from "@/components/workflows/topology/layout";
import { clusterOwner } from "@/components/workflows/topology/TopologyNodes";
import {
  CLUSTER_WORD,
  REFUSAL_NOTE,
  attemptsText,
  clusterIcon,
  clusterKindOf,
  durationText,
  iterationsText,
  nodeFacts,
  shortType,
  typeIcon,
  type TaskNode,
  type TriggerNode,
} from "@/components/workflows/topology/topologyModel";
import { cn } from "@/lib/utils";

/**
 * THE LIST VIEW (W7c spec §6.1): the canvas's accessible equivalent, the same
 * nodes and states as a `treegrid` nested by cluster.
 *
 * Its ARIA is the Gantt's (`executions/gantt/GanttView.tsx`): one tab stop,
 * arrow keys between rows, Home and End, Enter or Space selects, Right and
 * Left expand and collapse a row with children, and Left on a child goes to
 * its parent. Each row says its depth (`aria-level`), its place among its
 * siblings (`aria-posinset`, `aria-setsize`) and, with children, whether it is
 * expanded. Collapsing here and on the canvas is one state, keyed by cluster.
 *
 * A flowable's row is its own task with the cluster's members below it; the
 * triggers cluster's row is "Triggers". Markers have no row. A failed
 * `Subflow` expansion is not a cluster here either: its task is a row like any
 * other, with the reason in its notes.
 */

interface Row {
  /** The node's uid; for the triggers cluster, the cluster's. */
  uid: string;
  node: TaskNode | TriggerNode | null;
  cluster: Cluster | null;
  parent: string | null;
  depth: number;
  children: Row[];
}

/** Columns: task, then state and duration on an execution, then notes. */
const COLUMNS_WITH_STATES = "grid grid-cols-[minmax(12rem,1.4fr)_7.5rem_6.5rem_minmax(12rem,1.6fr)] items-center";
const COLUMNS = "grid grid-cols-[minmax(12rem,1fr)_minmax(12rem,1.4fr)] items-center";

function buildTree(graph: FlowGraph, order: readonly Positioned[]): Row[] {
  const rank = new Map(order.map((p, i) => [p.id, i]));
  const nodes = new Map(graph.nodes.map((n) => [n.uid, n]));
  const drawn = graph.clusters.filter((c) => !c.reason);
  const clusterOf = new Map<string, string>();
  for (const c of drawn) for (const n of c.nodes) clusterOf.set(n, c.uid);
  const rowOfCluster = new Map<string, Row>();

  for (const c of drawn) {
    const owner = clusterOwner(c);
    const ownerNode = owner ? nodes.get(owner) : undefined;
    rowOfCluster.set(c.uid, {
      uid: owner ?? c.uid,
      node: ownerNode && (ownerNode.node === "task" || ownerNode.node === "trigger") ? ownerNode : null,
      cluster: c,
      parent: null,
      depth: 0,
      children: [],
    });
  }
  const top: Row[] = [];
  const place = (row: Row, container: string | null) => {
    const parent = container ? rowOfCluster.get(container) : undefined;
    if (parent) {
      row.parent = parent.uid;
      parent.children.push(row);
    } else top.push(row);
  };
  for (const c of drawn) place(rowOfCluster.get(c.uid) as Row, c.parent ?? null);
  for (const n of graph.nodes) {
    if (n.node !== "task" && n.node !== "trigger") continue;
    if (drawn.some((c) => clusterOwner(c) === n.uid)) continue;
    place({ uid: n.uid, node: n, cluster: null, parent: null, depth: 0, children: [] }, clusterOf.get(n.uid) ?? null);
  }

  const rankOf = (r: Row) => rank.get(r.cluster ? r.cluster.uid : r.uid) ?? Number.MAX_SAFE_INTEGER;
  const sort = (rows: Row[], depth: number) => {
    rows.sort((a, b) => rankOf(a) - rankOf(b));
    for (const r of rows) {
      r.depth = depth;
      sort(r.children, depth + 1);
    }
  };
  sort(top, 0);
  return top;
}

function flatten(rows: Row[], collapsed: ReadonlySet<string>, out: Row[] = []): Row[] {
  for (const r of rows) {
    out.push(r);
    if (r.children.length > 0 && !(r.cluster && collapsed.has(r.cluster.uid))) flatten(r.children, collapsed, out);
  }
  return out;
}

export interface TopologyListViewProps {
  graph: FlowGraph;
  /** The layout of the whole graph (nothing collapsed): rows follow its order. */
  order: readonly Positioned[];
  states?: Readonly<Record<string, NodeState>>;
  collapsed: ReadonlySet<string>;
  onToggle: (clusterUid: string, open?: boolean) => void;
  selectedUid: string | null;
  onSelect: (uid: string) => void;
  now: number;
}

export function TopologyListView({ graph, order, states, collapsed, onToggle, selectedUid, onSelect, now }: TopologyListViewProps) {
  const tree = buildTree(graph, order);
  const rows = flatten(tree, collapsed);
  const [focusUid, setFocusUid] = useState<string | null>(selectedUid);
  const rowEls = useRef(new Map<string, HTMLDivElement>());
  const tabStop = rows.some((r) => r.uid === focusUid) ? focusUid : (rows[0]?.uid ?? null);
  const columns = states ? COLUMNS_WITH_STATES : COLUMNS;

  const siblingsOf = (r: Row): Row[] => {
    if (r.parent === null) return tree;
    const parent = rows.find((p) => p.uid === r.parent) ?? flatten(tree, new Set()).find((p) => p.uid === r.parent);
    return parent?.children ?? [];
  };
  const focusRow = (uid: string) => {
    setFocusUid(uid);
    rowEls.current.get(uid)?.focus();
  };
  const activate = (r: Row) => {
    if (r.node) onSelect(r.uid);
    else if (r.cluster) onToggle(r.cluster.uid);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>, r: Row, i: number) => {
    const isCollapsed = r.cluster !== null && collapsed.has(r.cluster.uid);
    const hasChildren = r.children.length > 0 && r.cluster !== null;
    switch (e.key) {
      case "ArrowDown":
        if (rows[i + 1]) focusRow(rows[i + 1].uid);
        break;
      case "ArrowUp":
        if (rows[i - 1]) focusRow(rows[i - 1].uid);
        break;
      case "Home":
        if (rows[0]) focusRow(rows[0].uid);
        break;
      case "End":
        if (rows.length) focusRow(rows[rows.length - 1].uid);
        break;
      case "ArrowRight":
        if (hasChildren && isCollapsed) onToggle((r.cluster as Cluster).uid, true);
        break;
      case "ArrowLeft":
        if (hasChildren && !isCollapsed) onToggle((r.cluster as Cluster).uid, false);
        else if (r.parent !== null) focusRow(r.parent);
        break;
      case "Enter":
      case " ":
        activate(r);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  return (
    <div className="h-full overflow-auto rounded-md border">
      {/* The canvas's accessible equivalent; the Gantt's treegrid pattern, not a table (FE-PAGE-4 does not apply to a tree). */}
      <div role="treegrid" aria-label="Tasks of the flow" aria-rowcount={rows.length + 1} className="min-w-[36rem] text-sm">
        <div role="row" aria-rowindex={1} className={cn(columns, "h-8 border-b bg-muted text-xs text-muted-foreground")}>
          <div role="columnheader" className="px-3">
            Task
          </div>
          {states ? (
            <>
              <div role="columnheader" className="px-2">
                State
              </div>
              <div role="columnheader" className="px-2">
                Duration
              </div>
            </>
          ) : null}
          <div role="columnheader" className="px-2">
            Notes
          </div>
        </div>
        {rows.map((r, i) => {
          const siblings = siblingsOf(r);
          const isCollapsed = r.cluster !== null && collapsed.has(r.cluster.uid);
          const hasChildren = r.children.length > 0 && r.cluster !== null;
          const selected = r.uid === selectedUid;
          const node = r.node;
          const expansion = node ? graph.clusters.find((c) => c.reason && c.start === node.uid)?.reason : undefined;
          const facts = node ? nodeFacts(node, states, expansion ?? undefined) : null;
          const icon = node
            ? typeIcon(node, node.node === "task" ? clusterKindOf(node.uid, graph.clusters) : undefined)
            : clusterIcon((r.cluster as Cluster).kind);
          const Icon = icon.icon;
          const name = node ? (node.node === "task" ? node.task_id : node.trigger_id) : CLUSTER_WORD[(r.cluster as Cluster).kind];
          const state = facts?.state;
          const notes = [
            ...(facts?.badges.filter((b) => b !== "Not run") ?? []),
            ...(facts?.refusal ? [`${REFUSAL_NOTE} (${facts.refusal})`] : []),
            ...(facts?.expansion ? [facts.expansion] : []),
            ...(state?.iterations ? [iterationsText(state.iterations)] : []),
            ...(state && attemptsText(state) ? [attemptsText(state) as string] : []),
          ];
          return (
            <div
              key={r.uid}
              role="row"
              data-uid={r.uid}
              aria-rowindex={i + 2}
              aria-level={r.depth + 1}
              aria-posinset={siblings.indexOf(r) + 1}
              aria-setsize={siblings.length}
              aria-expanded={hasChildren ? !isCollapsed : undefined}
              aria-selected={selected}
              tabIndex={r.uid === tabStop ? 0 : -1}
              ref={(el) => {
                if (el) rowEls.current.set(r.uid, el);
                else rowEls.current.delete(r.uid);
              }}
              onFocus={() => setFocusUid(r.uid)}
              onClick={() => activate(r)}
              onKeyDown={(e) => onKeyDown(e, r, i)}
              className={cn(
                columns,
                "min-h-9 cursor-pointer border-b border-l-2 last:border-b-0 hover:bg-muted",
                "focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none focus-visible:ring-inset",
                selected ? "border-l-primary bg-muted" : "border-l-transparent",
                facts?.dimmed && "text-muted-foreground",
              )}
            >
              <div role="gridcell" className="flex min-w-0 items-center gap-1 py-1 pr-2" style={{ paddingLeft: `calc(${r.depth} * 1rem + 0.25rem)` }}>
                {hasChildren ? (
                  <IconButton
                    tabIndex={-1}
                    aria-label={`${isCollapsed ? "Expand" : "Collapse"} ${name}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      onToggle((r.cluster as Cluster).uid);
                    }}
                    className="size-5"
                  >
                    {isCollapsed ? <ChevronRight aria-hidden className="size-4" /> : <ChevronDown aria-hidden className="size-4" />}
                  </IconButton>
                ) : (
                  <span aria-hidden className="w-5 shrink-0" />
                )}
                <Icon data-icon={icon.name} aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
                <span className="truncate font-mono text-xs" title={r.uid}>
                  {name}
                </span>
                {node ? <span className="shrink-0 text-xs text-muted-foreground">{shortType(node.type)}</span> : null}
              </div>
              {states ? (
                <>
                  <div role="gridcell" className="px-2">
                    {state ? <StateChip state={state.state} kind="execution" /> : state === null ? <span className="text-xs">Not run</span> : null}
                  </div>
                  <div role="gridcell" className="px-2 tabular-nums">
                    {state ? (durationText(state, now) ?? "—") : "—"}
                  </div>
                </>
              ) : null}
              <div role="gridcell" className="flex min-w-0 flex-col px-2 py-1 text-xs text-muted-foreground">
                {notes.map((n) => (
                  <span key={n} className="tabular-nums">
                    {n}
                  </span>
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
