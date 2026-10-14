import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { ChevronDown, ChevronRight, FolderTree } from "lucide-react";
import { useWorkflowListNamespaces } from "@/api/generated/workflow/workflow";
import type { WorkflowNamespaceDto } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
import { IconButton } from "@/components/ui/icon-button";
import { Table, TableBody, TableCaption, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { EmptyState } from "@/components/explore/EmptyState";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import {
  buildNamespaceRowTree,
  EMPTY_SEGMENT_LABEL,
  namespaceContains,
  type NamespaceRowNode,
} from "@/components/workflows/namespaceTree";
import { namespaceHref, OWN_COUNTS_NOTE } from "@/components/workflows/namespaces/namespaceTabs";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** The table's column count, for the skeleton. */
const COLS = 4;

/** One visible row of the treegrid, in tree order. */
interface Line {
  node: NamespaceRowNode;
  depth: number;
  /** The parent row's path; null for a root. */
  parent: string | null;
  /** 1-based place among its siblings, and their number (`aria-posinset`, `aria-setsize`). */
  pos: number;
  size: number;
}

/**
 * What a parent-only node says beside its name, so "muted" is not carried by
 * colour alone (FE-A11Y-4). Also this module's bundle marker
 * (`scripts/assert-workflows-lazy.mjs`).
 */
export const PARENT_ONLY_TITLE =
  "Nothing lives in this namespace itself; it is listed because a namespace below it holds flows, files or keys.";

/**
 * The namespaces list (W7d spec §4.1), `namespaces`: every namespace of the
 * workspace as a tree by dots, each with its own flows, files and KV keys
 * counts. A parent-only namespace (`own: false`) is muted and says so.
 *
 * `GET …/namespaces` is read unfiltered and the filter is applied here, as the
 * flow list does: the picker needs every namespace, and "the workspace has no
 * namespace" (`EmptyState`) must stay distinguishable from "the filter
 * matches none" (FE-STATE-3). The filter is the server's rule
 * (`namespaceContains`, dot-anchored), kept in the URL as `?prefix=`
 * (FE-PAGE-2). It is a picker because the namespaces are known (FE-FORM-3).
 *
 * The table is a WAI-ARIA treegrid in row focus mode (spec
 * `2026-10-14-editor-completion-and-tree-a11y-design.md` §3): one row takes
 * Tab (roving `tabIndex`); Up/Down move, Home/End jump, Right expands or
 * enters, Left collapses or goes to the parent, Enter opens the namespace.
 * The folded set is kept in memory only: not a selection anyone would share.
 */
export default function NamespacesList() {
  const ws = useParams().workspace!;
  const base = `/explore/${ws}/workflows`;

  const query = useWorkflowListNamespaces(ws, undefined, { query: { queryKey: workflowsKeys.namespaces(ws) } });
  const rows: WorkflowNamespaceDto[] | undefined = query.data?.status === 200 ? query.data.data : undefined;

  const [params, setParams] = useSearchParams();
  const prefix = params.get("prefix") ?? "";
  const setPrefix = (value: string) =>
    setParams((p) => {
      if (value) p.set("prefix", value);
      else p.delete("prefix");
      return p;
    });

  const filtered = useMemo(
    () => (rows ?? []).filter((r) => !prefix || namespaceContains(prefix, r.namespace)),
    [rows, prefix],
  );
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const lines = useMemo(() => {
    const out: Line[] = [];
    const walk = (nodes: NamespaceRowNode[], depth: number, parent: string | null) => {
      nodes.forEach((node, i) => {
        out.push({ node, depth, parent, pos: i + 1, size: nodes.length });
        // A collapsed row hides every row beneath it.
        if (!collapsed.has(node.path)) walk(node.children, depth + 1, node.path);
      });
    };
    walk(buildNamespaceRowTree(filtered), 0, null);
    return out;
  }, [filtered, collapsed]);
  const setOpen = (path: string, open: boolean) =>
    setCollapsed((prev) => {
      if (prev.has(path) === !open) return prev;
      const next = new Set(prev);
      if (open) next.delete(path);
      else next.add(path);
      return next;
    });

  // Roving focus: the row that takes Tab is the focused one while it is shown, else the first.
  const [focus, setFocus] = useState<string | null>(null);
  const tabStop = lines.some((l) => l.node.path === focus) ? focus : (lines[0]?.node.path ?? null);
  const refs = useRef(new Map<string, HTMLTableRowElement>());
  // Every move targets a row already on screen, so it is focused at once: no
  // deferred "focus later" flag that could outlive the move and pull focus
  // back from wherever the user went on the next re-render.
  const moveFocus = (path: string | null | undefined) => {
    if (!path) return;
    setFocus(path);
    refs.current.get(path)?.focus();
  };
  // Whether a row detached in this commit held keyboard focus at that moment.
  // Recorded at removal (React detaches a ref before it removes the node), not
  // as a standing "focus is in the grid" flag: a user who Tabbed or clicked
  // away has left the grid even though no blur said where to. Inline ref
  // callbacks also detach surviving rows each render, so the flag is reset on
  // every effect run and only read when the focused row is really gone.
  const removedHadFocus = useRef(false);
  const prevLines = useRef(lines);
  // A refetch or a filter can remove the focused row, which drops focus to
  // `<body>`. If that row held focus, focus goes to the nearest row above the
  // one that vanished, else the first.
  useEffect(() => {
    const before = prevLines.current;
    prevLines.current = lines;
    if (!focus || !removedHadFocus.current || lines.some((l) => l.node.path === focus)) return;
    const active = document.activeElement;
    if (active && active !== document.body) return;
    const shown = new Set(lines.map((l) => l.node.path));
    const was = before.findIndex((l) => l.node.path === focus);
    let target = lines[0]?.node.path;
    for (let k = was - 1; k >= 0; k--) {
      if (shown.has(before[k].node.path)) {
        target = before[k].node.path;
        break;
      }
    }
    if (!target) return;
    setFocus(target);
    refs.current.get(target)?.focus();
  }, [lines, focus]);
  // After every commit, and after the effect above has read it.
  useEffect(() => {
    removedHadFocus.current = false;
  });
  const navigate = useNavigate();
  const onRowKeyDown = (e: KeyboardEvent<HTMLTableRowElement>, i: number) => {
    // A modified key is someone else's: Alt/Cmd+Left is the browser's Back,
    // Cmd/Ctrl+Enter opens a link elsewhere (the row's link takes the click).
    if (e.altKey || e.metaKey || e.ctrlKey) return;
    const { node, parent } = lines[i];
    const hasChildren = node.children.length > 0;
    const open = hasChildren && !collapsed.has(node.path);
    switch (e.key) {
      case "ArrowDown":
        moveFocus(lines[i + 1]?.node.path);
        break;
      case "ArrowUp":
        moveFocus(lines[i - 1]?.node.path);
        break;
      case "Home":
        moveFocus(lines[0]?.node.path);
        break;
      case "End":
        moveFocus(lines[lines.length - 1]?.node.path);
        break;
      case "ArrowRight":
        if (hasChildren && !open) setOpen(node.path, true);
        else if (open) moveFocus(lines[i + 1]?.node.path);
        break;
      case "ArrowLeft":
        if (open) setOpen(node.path, false);
        else moveFocus(parent);
        break;
      case "Enter":
        navigate(namespaceHref(ws, node.path));
        break;
      default:
        return;
    }
    e.preventDefault();
  };
  const captionId = useId();
  const helpId = useId();

  const options = useMemo(() => {
    const known = (rows ?? []).map((r) => r.namespace).sort((a, b) => a.localeCompare(b));
    // A shared link can name a namespace that has since emptied; it stays selectable.
    if (prefix && !known.includes(prefix)) known.push(prefix);
    return known.map((ns) => ({ value: ns, label: ns }));
  }, [rows, prefix]);

  return (
    <ListPageShell title="Namespaces">
      <ListStates
        isLoading={query.isLoading}
        isError={query.isError}
        error={query.error}
        errorText="The namespaces could not be loaded. Reload the page to try again."
        hasData={!!rows && rows.length > 0}
        skeletonCols={COLS}
        empty={
          <EmptyState
            variant="dashed"
            icon={FolderTree}
            title="No namespaces yet"
            description="A namespace appears when a flow, a namespace file or a KV key is created in it. Its parents by dots are listed too."
            action={
              <Button asChild>
                <Link to={`${base}/flows`}>Go to flows</Link>
              </Button>
            }
          />
        }
      >
        <div className="mb-3 flex flex-wrap items-center gap-2">
          <FacetSelect
            label="Namespace"
            value={prefix || FACET_ALL}
            onChange={(v) => setPrefix(v === FACET_ALL ? "" : v)}
            options={options}
          />
        </div>

        <div className="mb-3" data-testid="namespaces-summary">
          <SummaryStrip items={[{ label: "namespaces", singular: "namespace", value: filtered.length }]} />
        </div>

        {filtered.length === 0 ? (
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm text-muted-foreground">No namespaces match the current filter.</p>
            <Button variant="outline" size="sm" onClick={() => setPrefix("")}>
              Clear filter
            </Button>
          </div>
        ) : (
          <>
            <p id={helpId} className="sr-only">
              Arrow keys move through the namespaces. Right expands, Left collapses, and Enter opens a namespace.
            </p>
            <Table
              data-testid="namespaces-table"
              role="treegrid"
              aria-label="Namespaces"
              aria-describedby={`${captionId} ${helpId}`}
            >
              {/* A parent's row is not its subtree's total, as the Overview says too. */}
              <TableCaption id={captionId} className="mt-0 mb-2 caption-top text-left">
                {OWN_COUNTS_NOTE}
              </TableCaption>
              <TableHeader>
                <TableRow>
                  <TableHead>Namespace</TableHead>
                  <TableHead className="text-right">Flows</TableHead>
                  <TableHead className="text-right">Files</TableHead>
                  <TableHead className="text-right">KV keys</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {lines.map((line, i) => (
                  <NamespaceRow
                    key={line.node.path}
                    line={line}
                    ws={ws}
                    open={!collapsed.has(line.node.path)}
                    tabStop={tabStop === line.node.path}
                    rowRef={(el) => {
                      if (el) {
                        refs.current.set(line.node.path, el);
                        return;
                      }
                      const prev = refs.current.get(line.node.path);
                      if (prev && document.activeElement === prev) removedHadFocus.current = true;
                      refs.current.delete(line.node.path);
                    }}
                    onFocus={() => setFocus(line.node.path)}
                    onKeyDown={(e) => onRowKeyDown(e, i)}
                    onToggle={() => {
                      setOpen(line.node.path, collapsed.has(line.node.path));
                      moveFocus(line.node.path);
                    }}
                  />
                ))}
              </TableBody>
            </Table>
          </>
        )}
      </ListStates>
    </ListPageShell>
  );
}

/**
 * One namespace: a treegrid row with its label, indented by depth, linking to
 * its detail, and its own counts.
 *
 * The link's accessible name is the FULL namespace, because a segment is not
 * unique (`a.x` and `b.x` both show `x`); the visible label is its suffix, so
 * what is seen is what is spoken. An empty segment shows
 * `EMPTY_SEGMENT_LABEL`, which the name then carries too. The link takes no
 * Tab stop: the row does, and its Enter follows the link.
 *
 * A row with children has a chevron that folds it, for the mouse; it takes no
 * Tab stop either, as Right and Left do the same.
 */
function NamespaceRow({
  line,
  ws,
  open,
  tabStop,
  rowRef,
  onFocus,
  onKeyDown,
  onToggle,
}: {
  line: Line;
  ws: string;
  open: boolean;
  tabStop: boolean;
  rowRef: (el: HTMLTableRowElement | null) => void;
  onFocus: () => void;
  onKeyDown: (e: KeyboardEvent<HTMLTableRowElement>) => void;
  onToggle: () => void;
}) {
  const { node, depth } = line;
  const parentOnly = !node.row.own;
  const empty = node.label === "";
  const hasChildren = node.children.length > 0;
  const Chevron = open ? ChevronDown : ChevronRight;
  const count = (n: number) => (
    <TableCell role="gridcell" className={`text-right tabular-nums ${parentOnly ? "text-muted-foreground" : ""}`}>
      {n}
    </TableCell>
  );
  return (
    <TableRow
      ref={rowRef}
      data-depth={depth}
      aria-level={depth + 1}
      aria-posinset={line.pos}
      aria-setsize={line.size}
      aria-expanded={hasChildren ? open : undefined}
      tabIndex={tabStop ? 0 : -1}
      onFocus={onFocus}
      onKeyDown={onKeyDown}
      className="outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
    >
      <TableCell role="gridcell" style={{ paddingLeft: `${0.5 + depth * 1.25}rem` }}>
        <span className="inline-flex items-center gap-1">
          {/* A leaf keeps the chevron's slot, so siblings' labels line up. */}
          {hasChildren ? (
            <IconButton
              tabIndex={-1}
              aria-label={`${open ? "Collapse" : "Expand"} ${node.path}`}
              onClick={onToggle}
            >
              <Chevron aria-hidden className="size-4" />
            </IconButton>
          ) : (
            <span aria-hidden className="size-6 shrink-0" />
          )}
          <span className="inline-flex flex-wrap items-center gap-2">
            <Link
              to={namespaceHref(ws, node.path)}
              tabIndex={-1}
              aria-label={empty ? `${node.path} ${EMPTY_SEGMENT_LABEL}` : node.path}
              title={node.path}
              className={`rounded-sm hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
                parentOnly ? "text-muted-foreground" : "font-medium"
              } ${empty ? "italic" : ""}`}
            >
              {empty ? EMPTY_SEGMENT_LABEL : node.label}
            </Link>
            {parentOnly ? (
              <span className="text-xs text-muted-foreground" title={PARENT_ONLY_TITLE}>
                Parent only
              </span>
            ) : null}
          </span>
        </span>
      </TableCell>
      {count(node.row.flows)}
      {count(node.row.files)}
      {count(node.row.kv_keys)}
    </TableRow>
  );
}
