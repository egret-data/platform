import { useEffect, useId, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQueries, type UseQueryResult } from "@tanstack/react-query";
import {
  ArrowRight,
  ChevronDown,
  ChevronRight,
  Database,
  Eye,
  Folder,
  Layers,
  type LucideIcon,
  RotateCw,
  Search,
  Table2,
} from "lucide-react";
import {
  getControlPlaneGovernanceBrowseQueryOptions,
  useControlPlaneGovernanceListCatalogs,
  type ControlPlaneGovernanceBrowseQueryError,
  type ControlPlaneGovernanceBrowseQueryResult,
} from "@/api/generated/control-plane-governance/control-plane-governance";
import type { BrowseKind, BrowseResponse, GovernanceCatalog } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { Button } from "@/components/ui/button";
import { IconButton } from "@/components/ui/icon-button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableRow } from "@/components/ui/table";
import { catalogKindLabel } from "@/lib/catalogKinds";
import { parseDottedPath, pathToUrl, type ObjectPath } from "@/routes/governance/data/objectPath";

/**
 * What a level that browse could not answer says. Also this module's bundle
 * marker (`scripts/assert-governance-data-lazy.mjs`).
 */
export const LEVEL_FAILED = "This level could not be listed. Retry, or reload the page.";

export type NodeKind = "catalog" | BrowseKind;

/**
 * Why an empty level read through an engine is not a plain fact (D1, D2): an
 * engine can answer an empty list for a name it cannot match, the same answer
 * it gives for a level that holds nothing, so egret cannot tell the two apart.
 * Measured: Trino lists a mixed-case Postgres schema `MixS` as `mixs` and
 * then lists nothing in it; StarRocks lists nothing in a schema whose name
 * holds a backslash.
 */
export const ENGINE_EMPTY_DETAIL =
  "An engine can return an empty list for names it cannot match (for example mixed-case names in Trino JDBC catalogs).";

/**
 * What an empty level says: `No <what>` when egret listed it, and
 * `<engine> listed no <what>` with {@link ENGINE_EMPTY_DETAIL} when an engine
 * did (spec §2.5: a listing that may have failed says so).
 */
export function emptyLevel(source: BrowseResponse["source"], what: string): { title: string; detail?: string } {
  if (source?.kind === "engine") return { title: `${source.name} listed no ${what}`, detail: ENGINE_EMPTY_DETAIL };
  return { title: `No ${what}` };
}

type BrowseResult = UseQueryResult<ControlPlaneGovernanceBrowseQueryResult, ControlPlaneGovernanceBrowseQueryError>;

/** Each kind's icon and name, as the tree and an object's header show it. */
export const KIND_ICON: Record<NodeKind, { icon: LucideIcon; label: string }> = {
  catalog: { icon: Database, label: "Catalog" },
  schema: { icon: Folder, label: "Schema" },
  table: { icon: Table2, label: "Table" },
  view: { icon: Eye, label: "View" },
  materialized_view: { icon: Layers, label: "Materialized view" },
};

/** A catalog, schema or object of the tree. */
interface TreeNode {
  /** Unique across the tree: the encoded path (names may hold dots, so a dotted path is not unique). */
  key: string;
  kind: NodeKind;
  name: string;
  path: ObjectPath;
  /** Only catalogs and schemas have a next level. */
  expandable: boolean;
  /** A catalog's source label. */
  source?: string;
}

/** What an expanded node shows in place of children it could not list, or while it lists them. */
type StatusKind = "loading" | "error" | "empty" | "notListed";

/** One visible row of the treegrid, in tree order. */
type Line =
  | { type: "node"; key: string; node: TreeNode; depth: number; parent: string | null; pos: number; size: number }
  | { type: "status"; key: string; status: StatusKind; text: string; owner: TreeNode; depth: number; parent: string };

/** The key of a catalog or a schema: its names, each encoded, joined by `/`. */
function keyOf(p: ObjectPath): string {
  return [p.catalog, p.schema, p.object].filter((s): s is string => s !== undefined).map(encodeURIComponent).join("/");
}

/** The catalog and schema an expanded key names. */
function levelOf(key: string): { catalog: string; schema?: string } {
  const [catalog, schema] = key.split("/").map(decodeURIComponent);
  return schema === undefined ? { catalog } : { catalog, schema };
}

/** What a person reads and types for `p`: its names joined by dots. */
export function dottedPath(p: ObjectPath): string {
  return [p.catalog, p.schema, p.object].filter((s): s is string => s !== undefined).join(".");
}

/**
 * Where a catalog is read from (spec §2.2): an egret catalog from its
 * warehouse, any other through an engine attached to it. Which engine, and
 * whether one can be asked at all, is browse's answer per level; the tree says
 * only how the catalog is reached.
 */
export function sourceLabel(c: GovernanceCatalog): string {
  if (c.source === "egret") return "Iceberg · egret";
  return `${catalogKindLabel(c.connectorType)} · via engine`;
}

const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name);

/**
 * The catalog tree of the Data area (spec §2.2): the registry's catalogs, and
 * under each, listed lazily through the browse API, its schemas and then their
 * tables, views and materialized views. A level is read only once its parent
 * is expanded. A level that could not be listed shows browse's reason and a
 * way to go to a known path, never an empty branch.
 *
 * The search box filters the rows that are loaded and shown; it is not a
 * search of the catalogs, and it is kept in memory, not in the URL: what it
 * filters is this viewer's own expanded tree, which a link cannot reproduce.
 * The expanded set is in memory too, and opens itself down to the object the
 * URL names.
 *
 * The tree is a WAI-ARIA treegrid in row focus mode, as the workflows
 * namespaces list (`routes/workflows/NamespacesList.tsx`): one row takes Tab
 * (roving `tabIndex`); Up/Down move, Home/End jump, Right expands or enters,
 * Left collapses or goes to the parent, Enter opens the node (on a row that
 * says a level was not listed, Enter goes to the path box; on a failed level,
 * it retries).
 */
export function CatalogTree({ selected }: { selected: ObjectPath | null }) {
  // The Governance catalog list, not the registry's: it is readable under the
  // same permissions as this area (`policy:read` or `tag:read`).
  const catalogsQ = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  const catalogs = useMemo(
    () => (catalogsQ.data?.status === 200 ? [...catalogsQ.data.data].sort(byName) : undefined),
    [catalogsQ.data],
  );
  const idByName = useMemo(() => new Map((catalogs ?? []).map((c) => [c.name, c.id])), [catalogs]);

  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const setOpen = (key: string, open: boolean) =>
    setExpanded((prev) => {
      if (prev.has(key) === open) return prev;
      const next = new Set(prev);
      if (open) next.add(key);
      else next.delete(key);
      return next;
    });

  // Open the tree down to the object the URL names. Only opens: a node the
  // user folded stays folded until the URL moves somewhere under it again.
  // Adjusted while rendering, when the selection changes (React's "storing
  // information from previous renders"), not in an effect.
  const selectedKey = selected ? keyOf(selected) : null;
  const [openedFor, setOpenedFor] = useState<string | null>(null);
  if (selected && selectedKey !== openedFor) {
    setOpenedFor(selectedKey);
    const keys = [keyOf({ catalog: selected.catalog })];
    if (selected.schema !== undefined && selected.object !== undefined) {
      keys.push(keyOf({ catalog: selected.catalog, schema: selected.schema }));
    }
    if (!keys.every((k) => expanded.has(k))) setExpanded(new Set([...expanded, ...keys]));
  }

  // One browse query per expanded level whose catalog is in the registry.
  const levels = useMemo(
    () => [...expanded].map((key) => ({ key, ...levelOf(key) })).filter((l) => idByName.has(l.catalog)),
    [expanded, idByName],
  );
  const results = useQueries({
    queries: levels.map((l) =>
      getControlPlaneGovernanceBrowseQueryOptions(
        idByName.get(l.catalog)!,
        l.schema === undefined ? undefined : { schema: l.schema },
        { query: { retry: false } },
      ),
    ),
  });
  const resultByKey = new Map<string, BrowseResult>();
  levels.forEach((l, i) => resultByKey.set(l.key, results[i]));

  const [filter, setFilter] = useState("");
  const needle = filter.trim().toLocaleLowerCase();

  const lines = buildLines(catalogs ?? [], expanded, resultByKey, needle);

  // Roving focus: the row that takes Tab is the focused one while it is shown, else the selected, else the first.
  const [focus, setFocus] = useState<string | null>(null);
  const shown = (key: string | null) => key !== null && lines.some((l) => l.key === key);
  const tabStop = shown(focus) ? focus : shown(selectedKey) ? selectedKey : (lines[0]?.key ?? null);
  const refs = useRef(new Map<string, HTMLTableRowElement>());
  const moveFocus = (key: string | null | undefined) => {
    if (!key) return;
    setFocus(key);
    refs.current.get(key)?.focus();
  };
  // As in `NamespacesList`: when a refetch or the filter removes the row that
  // held focus, focus goes to the nearest row above it that is still shown.
  const removedHadFocus = useRef(false);
  const prevLines = useRef(lines);
  useEffect(() => {
    const before = prevLines.current;
    prevLines.current = lines;
    if (!focus || !removedHadFocus.current || lines.some((l) => l.key === focus)) return;
    const active = document.activeElement;
    if (active && active !== document.body) return;
    const now = new Set(lines.map((l) => l.key));
    const was = before.findIndex((l) => l.key === focus);
    let target = lines[0]?.key;
    for (let k = was - 1; k >= 0; k--) {
      if (now.has(before[k].key)) {
        target = before[k].key;
        break;
      }
    }
    if (!target) return;
    setFocus(target);
    refs.current.get(target)?.focus();
  }, [lines, focus]);
  useEffect(() => {
    removedHadFocus.current = false;
  });

  const navigate = useNavigate();
  const goBox = useRef<HTMLInputElement>(null);
  const [goText, setGoText] = useState("");
  const [goError, setGoError] = useState<string | null>(null);
  const startPath = (prefix: string) => {
    setGoText(prefix);
    setGoError(null);
    goBox.current?.focus();
  };
  const onGo = (e: FormEvent) => {
    e.preventDefault();
    const p = parseDottedPath(goText);
    if (!p) {
      setGoError("Enter a path such as catalog.schema.table.");
      return;
    }
    // Before the list arrives the name cannot be checked here; the page it opens says what it finds.
    if (catalogs && !idByName.has(p.catalog)) {
      setGoError(`No catalog is named “${p.catalog}”.`);
      return;
    }
    setGoError(null);
    navigate(pathToUrl(p));
  };

  const onRowKeyDown = (e: KeyboardEvent<HTMLTableRowElement>, i: number) => {
    // A modified key is someone else's: Alt/Cmd+Left is the browser's Back.
    if (e.altKey || e.metaKey || e.ctrlKey) return;
    const line = lines[i];
    const node = line.type === "node" ? line.node : null;
    const open = !!node?.expandable && expanded.has(line.key);
    switch (e.key) {
      case "ArrowDown":
        moveFocus(lines[i + 1]?.key);
        break;
      case "ArrowUp":
        moveFocus(lines[i - 1]?.key);
        break;
      case "Home":
        moveFocus(lines[0]?.key);
        break;
      case "End":
        moveFocus(lines[lines.length - 1]?.key);
        break;
      case "ArrowRight":
        if (node?.expandable && !open) setOpen(line.key, true);
        else if (open) moveFocus(lines[i + 1]?.key);
        break;
      case "ArrowLeft":
        if (open) setOpen(line.key, false);
        else moveFocus(line.parent);
        break;
      case "Enter":
        if (node) navigate(pathToUrl(node.path));
        else if (line.type === "status" && line.status === "notListed") startPath(`${dottedPath(line.owner.path)}.`);
        else if (line.type === "status" && line.status === "error") void resultByKey.get(line.owner.key)?.refetch();
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const searchId = useId();
  const goId = useId();
  const goErrorId = useId();
  const helpId = useId();

  return (
    <div className="flex min-h-0 flex-col gap-3">
      <form onSubmit={onGo} className="space-y-1" noValidate>
        <Label htmlFor={goId} className="sr-only">
          Go to path
        </Label>
        <div className="flex items-center gap-1">
          <Input
            id={goId}
            ref={goBox}
            value={goText}
            onChange={(e) => {
              setGoText(e.target.value);
              setGoError(null);
            }}
            placeholder="Go to catalog.schema.table"
            autoComplete="off"
            spellCheck={false}
            aria-invalid={goError ? true : undefined}
            aria-describedby={goError ? goErrorId : undefined}
            className="h-8 font-mono text-xs"
          />
          <IconButton type="submit" aria-label="Go" title="Go" className="size-8">
            <ArrowRight aria-hidden className="size-4" />
          </IconButton>
        </div>
        {goError ? (
          <p id={goErrorId} className="text-xs text-destructive">
            {goError}
          </p>
        ) : null}
      </form>

      <div className="relative">
        <Label htmlFor={searchId} className="sr-only">
          Filter loaded items
        </Label>
        <Search aria-hidden className="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          id={searchId}
          type="search"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter loaded items"
          className="h-8 pl-8 text-sm"
        />
      </div>

      {catalogsQ.isError ? (
        <p role="alert" className="text-sm text-destructive">
          The catalogs could not be loaded. Reload the page to try again.
        </p>
      ) : catalogsQ.isLoading || !catalogs ? (
        <div className="space-y-2" aria-busy="true" aria-label="Loading catalogs">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-6 w-full" />
          ))}
        </div>
      ) : catalogs.length === 0 ? (
        <EmptyState
          icon={Database}
          title="No catalogs"
          description="The catalog registry is empty. An administrator adds catalogs in Control Plane, Catalogs."
        />
      ) : lines.length === 0 ? (
        <div className="space-y-2">
          <p className="text-sm text-muted-foreground">No loaded item matches the filter.</p>
          <Button variant="outline" size="sm" onClick={() => setFilter("")}>
            Clear filter
          </Button>
        </div>
      ) : (
        <>
          <p id={helpId} className="sr-only">
            Arrow keys move through the catalogs. Right expands, Left collapses, and Enter opens an item.
          </p>
          {/* A treegrid, the namespaces list's pattern: one column, so no header row. */}
          <Table role="treegrid" aria-label="Catalogs" aria-describedby={helpId} className="table-fixed">
            <TableBody>
              {lines.map((line, i) => (
                <TreeRow
                  key={line.key}
                  line={line}
                  open={expanded.has(line.key)}
                  selected={line.key === selectedKey}
                  tabStop={tabStop === line.key}
                  rowRef={(el) => {
                    if (el) {
                      refs.current.set(line.key, el);
                      return;
                    }
                    const prev = refs.current.get(line.key);
                    if (prev && document.activeElement === prev) removedHadFocus.current = true;
                    refs.current.delete(line.key);
                  }}
                  onFocus={() => setFocus(line.key)}
                  onKeyDown={(e) => onRowKeyDown(e, i)}
                  onToggle={() => {
                    setOpen(line.key, !expanded.has(line.key));
                    moveFocus(line.key);
                  }}
                  onGoToPath={(prefix) => startPath(prefix)}
                  onRetry={() => {
                    if (line.type === "status") void resultByKey.get(line.owner.key)?.refetch();
                  }}
                />
              ))}
            </TableBody>
          </Table>
        </>
      )}
    </div>
  );
}

/**
 * The visible rows, in tree order. With a filter, a node shows when its name
 * matches or a shown node below it does. A node shown because it matches
 * keeps its whole loaded subtree, status rows included (loading, failed, not
 * listed, empty), so an expanded row is never shown with nothing under it
 * (spec §2.2: never an empty branch). A node kept only as a match's parent
 * shows just the path down to that match.
 */
function buildLines(
  catalogs: GovernanceCatalog[],
  expanded: ReadonlySet<string>,
  results: Map<string, BrowseResult>,
  needle: string,
): Line[] {
  const childrenOf = (node: TreeNode): { nodes: TreeNode[] } | { status: StatusKind; text: string } | null => {
    if (!node.expandable || !expanded.has(node.key)) return null;
    const r = results.get(node.key);
    if (!r || r.isError) {
      return r
        ? { status: "error", text: LEVEL_FAILED }
        : { status: "loading", text: "Loading" };
    }
    if (r.isPending || r.data?.status !== 200) return { status: "loading", text: "Loading" };
    const body: BrowseResponse = r.data.data;
    if (body.notListed) return { status: "notListed", text: body.notListed.detail };
    const items = [...(body.items ?? [])].sort(byName);
    if (items.length === 0) {
      const empty = emptyLevel(body.source, node.kind === "catalog" ? "schemas" : "tables or views");
      return { status: "empty", text: empty.detail ? `${empty.title}. ${empty.detail}` : empty.title };
    }
    return {
      nodes: items.map((item) => {
        const path: ObjectPath =
          node.kind === "catalog"
            ? { catalog: node.path.catalog, schema: item.name }
            : { catalog: node.path.catalog, schema: node.path.schema, object: item.name };
        return { key: keyOf(path), kind: item.kind, name: item.name, path, expandable: item.kind === "schema" && node.kind === "catalog" };
      }),
    };
  };

  const out: Line[] = [];
  /** Pushes the shown lines of `nodes` under `filter`; returns whether any node was shown. */
  const walk = (nodes: TreeNode[], depth: number, parent: string | null, filter: string): boolean => {
    const shownHere: { node: TreeNode; below: Line[] }[] = [];
    for (const node of nodes) {
      const below: Line[] = [];
      const kids = childrenOf(node);
      const matches = !filter || node.name.toLocaleLowerCase().includes(filter);
      let childShown = false;
      if (kids && "nodes" in kids) {
        const save = out.length;
        childShown = walk(kids.nodes, depth + 1, node.key, matches ? "" : filter);
        below.push(...out.splice(save));
      } else if (kids) {
        below.push({
          type: "status",
          key: `${node.key}#status`,
          status: kids.status,
          text: kids.text,
          owner: node,
          depth: depth + 1,
          parent: node.key,
        });
      }
      if (matches || childShown) shownHere.push({ node, below });
    }
    shownHere.forEach(({ node, below }, i) => {
      out.push({ type: "node", key: node.key, node, depth, parent, pos: i + 1, size: shownHere.length });
      out.push(...below);
    });
    return shownHere.length > 0;
  };
  walk(
    catalogs.map((c) => ({
      key: keyOf({ catalog: c.name }),
      kind: "catalog" as const,
      name: c.name,
      path: { catalog: c.name },
      expandable: true,
      source: sourceLabel(c),
    })),
    0,
    null,
    needle,
  );
  return out;
}

/**
 * One row. A node's link is named by its full dotted path, because a name is
 * not unique across the tree (two schemas can each hold `orders`); the visible
 * label is the name alone. The link and the chevron take no Tab stop: the row
 * does, and its keys do what they do.
 */
function TreeRow({
  line,
  open,
  selected,
  tabStop,
  rowRef,
  onFocus,
  onKeyDown,
  onToggle,
  onGoToPath,
  onRetry,
}: {
  line: Line;
  open: boolean;
  selected: boolean;
  tabStop: boolean;
  rowRef: (el: HTMLTableRowElement | null) => void;
  onFocus: () => void;
  onKeyDown: (e: KeyboardEvent<HTMLTableRowElement>) => void;
  onToggle: () => void;
  onGoToPath: (prefix: string) => void;
  onRetry: () => void;
}) {
  const indent = { paddingLeft: `${0.25 + line.depth * 1}rem` };
  const rowProps = {
    ref: rowRef,
    "aria-level": line.depth + 1,
    tabIndex: tabStop ? 0 : -1,
    onFocus,
    onKeyDown,
  };
  const rowClass =
    "border-0 outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset";

  if (line.type === "status") {
    const prefix = `${dottedPath(line.owner.path)}.`;
    return (
      <TableRow {...rowProps} className={`${rowClass} hover:bg-transparent`}>
        <TableCell role="gridcell" aria-label={dottedPath(line.owner.path)} style={indent} className="py-1 whitespace-normal">
          <div className="ml-7 text-xs text-muted-foreground">
            {line.status === "loading" ? (
              <span className="flex items-center gap-2">
                <Skeleton className="h-4 w-28" />
                <span className="sr-only">Loading</span>
              </span>
            ) : line.status === "notListed" ? (
              <div className="space-y-1">
                <p>{line.text}</p>
                <Button variant="link" size="sm" tabIndex={-1} className="h-auto p-0 text-xs" onClick={() => onGoToPath(prefix)}>
                  Go to path
                </Button>
              </div>
            ) : line.status === "error" ? (
              <div className="space-y-1">
                <p className="text-destructive">{line.text}</p>
                <Button variant="link" size="sm" tabIndex={-1} className="h-auto p-0 text-xs" onClick={onRetry}>
                  <RotateCw aria-hidden className="size-3" />
                  Retry
                </Button>
              </div>
            ) : (
              <p>{line.text}</p>
            )}
          </div>
        </TableCell>
      </TableRow>
    );
  }

  const { node } = line;
  const { icon: KindIcon, label: kindLabel } = KIND_ICON[node.kind];
  const Chevron = open ? ChevronDown : ChevronRight;
  const full = dottedPath(node.path);
  return (
    <TableRow
      {...rowProps}
      aria-posinset={line.pos}
      aria-setsize={line.size}
      aria-expanded={node.expandable ? open : undefined}
      aria-selected={selected}
      className={`${rowClass} ${selected ? "bg-accent text-accent-foreground hover:bg-accent" : ""}`}
    >
      <TableCell role="gridcell" aria-label={full} style={indent} className="py-0.5">
        <span className="flex min-w-0 items-center gap-1">
          {node.expandable ? (
            <IconButton tabIndex={-1} aria-label={`${open ? "Collapse" : "Expand"} ${full}`} onClick={onToggle}>
              <Chevron aria-hidden className="size-4" />
            </IconButton>
          ) : (
            <span aria-hidden className="size-6 shrink-0" />
          )}
          <KindIcon role="img" aria-label={kindLabel} className="size-4 shrink-0 text-muted-foreground" />
          <span className="flex min-w-0 flex-col">
            <Link
              to={pathToUrl(node.path)}
              tabIndex={-1}
              aria-label={full}
              title={full}
              className="truncate rounded-sm hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            >
              {node.name}
            </Link>
            {node.source ? <span className="truncate text-xs text-muted-foreground">{node.source}</span> : null}
          </span>
        </span>
      </TableCell>
    </TableRow>
  );
}
