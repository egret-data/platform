import { useCallback, useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { ChevronDown, ChevronRight, Search, Trash2, Workflow } from "lucide-react";
import { useWorkflowListFlows } from "@/api/generated/workflow/workflow";
import type { FlowListItemDto } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { EmptyState } from "@/components/explore/EmptyState";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { FlowsTrash } from "@/components/workflows/FlowsTrash";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { useCollapsed } from "@/components/explore/folders";
import { flowActivity, StateChip, type FlowActivity } from "@/components/workflows/StateChip";
import {
  buildNamespaceTree,
  EMPTY_SEGMENT_LABEL,
  knownNamespaces,
  namespaceContains,
  type NamespaceNode,
} from "@/components/workflows/namespaceTree";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { ENTITY_LINK_CLASS } from "@/components/ui/entity-link";
import { Forbidden, isForbidden, refusalReason } from "@/routes/Forbidden";
import { ScrollX } from "@/components/ui/scroll-cue";

/**
 * Where the tree's collapsed namespaces are kept, per viewer and per
 * workspace: a preference, never server data (FE-DATA-3). `useCollapsed`
 * wraps every access, so the page works without storage.
 *
 * Exported so the test seeds and reads the key the page uses, rather than a
 * retyped copy that would pass against a page that had stopped reading it.
 * The literal is also the bundle marker `scripts/assert-workflows-lazy.mjs`
 * looks for; renaming it means updating that script.
 */
export const nsCollapseKey = (ws: string) => `egret.wf.ns-collapsed.${ws}`;

/**
 * The State filter: whether a flow runs (the user's ruling, 2026-10-10).
 * `draft` and `live` are both Active, as the chip says; `?state=` carries
 * `active` or `disabled`.
 */
const FLOW_STATES: { value: FlowActivity; label: string }[] = [
  { value: "active", label: "Active" },
  { value: "disabled", label: "Disabled" },
];

/** The table's column count, for the skeleton and the header rows' span. */
const COLS = 5;

/** One namespace's header plus the flows that are its own, in tree order. */
interface Section {
  node: NamespaceNode;
  depth: number;
}

/**
 * The flow list (spec §4): every flow in the workspace, grouped by a
 * collapsible namespace tree built client-side.
 *
 * `GET …/flows` is fetched unfiltered and every filter is applied here. The
 * namespace picker is built from the whole list, and "the workspace has no
 * flows" (`EmptyState`) must stay distinguishable from "the filters match
 * none" — both need the unfiltered collection.
 *
 * With `pinnedNamespace`, as a namespace's Flows tab (W7d spec §4.2), the list
 * is that namespace's flows and its children's (`namespaceContains`, the
 * dot-anchored rule the server's `namespace_prefix` uses), with no page frame
 * (the namespace page owns the `h1`) and no namespace picker. `GET …/flows`
 * filters only by an exact namespace, so the subtree is cut here, from the
 * same cached list the flow page reads.
 */
export default function FlowsList({ pinnedNamespace }: { pinnedNamespace?: string } = {}) {
  const ws = useParams().workspace!;
  const base = `/explore/${ws}/workflows`;
  const pinned = pinnedNamespace !== undefined;

  const query = useWorkflowListFlows(ws, undefined, { query: { queryKey: workflowsKeys.flows(ws) } });
  const allFlows: FlowListItemDto[] | undefined = query.data?.status === 200 ? query.data.data : undefined;
  const flows = useMemo(
    () => (pinnedNamespace === undefined ? allFlows : allFlows?.filter((f) => namespaceContains(pinnedNamespace, f.namespace))),
    [allFlows, pinnedNamespace],
  );
  const hasData = !!flows && flows.length > 0;

  // Filters live in the URL (FE-PAGE-2): a filtered list can be shared and
  // survives a reload. The search box replaces the entry per keystroke; a
  // picker pushes one, so Back undoes it.
  const [params, setParams] = useSearchParams();
  const q = params.get("q") ?? "";
  // On a namespace's tab the namespace is fixed; a `namespace` left in the URL is read as absent.
  const nsFilter = pinned ? "" : (params.get("namespace") ?? "");
  const stateFilter = params.get("state") ?? "";
  const showTrash = !pinned && params.get("view") === "trash";
  const setParam = (key: string, value: string, replace: boolean) =>
    setParams(
      (p) => {
        if (value) p.set(key, value);
        else p.delete(key);
        return p;
      },
      { replace },
    );
  const clearFilters = () =>
    setParams((p) => {
      p.delete("q");
      p.delete("namespace");
      p.delete("state");
      return p;
    });
  const filtering = q.trim() !== "" || nsFilter !== "" || stateFilter !== "";

  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return (flows ?? []).filter(
      (f) =>
        (!needle || f.flow_id.toLowerCase().includes(needle) || f.namespace.toLowerCase().includes(needle)) &&
        (!nsFilter || namespaceContains(nsFilter, f.namespace)) &&
        (!stateFilter || flowActivity(f.state) === stateFilter),
    );
  }, [flows, q, nsFilter, stateFilter]);

  const namespaceOptions = useMemo(() => {
    const known = knownNamespaces(flows ?? []);
    // A shared link can name a namespace that has since emptied; it stays
    // selectable so the picker shows what is filtering the list.
    if (nsFilter && !known.includes(nsFilter)) known.push(nsFilter);
    return known.map((ns) => ({ value: ns, label: ns }));
  }, [flows, nsFilter]);

  const { isCollapsed: isStoredCollapsed, toggle } = useCollapsed(nsCollapseKey(ws));
  // While a filter is engaged the tree shows every match: a search that hits a
  // flow inside a collapsed namespace would otherwise show a header and no row.
  // The stored state is neither read nor written then, and returns when the
  // filters clear. The toggles still work on the results, but what they fold
  // is kept in memory for THIS filter only (`filterSig`): a new search starts
  // fully open again, and nothing about a transient search is persisted.
  //
  // A namespace's Flows tab (pinned) folds in memory too, starting open: the
  // stored state belongs to the workspace tree, where a collapsed ancestor
  // (`company` above `company.team`) would hide the whole tab.
  const transient = filtering || pinned;
  const filterSig = transient ? JSON.stringify([pinnedNamespace ?? null, q, nsFilter, stateFilter]) : "";
  const [filterFolds, setFilterFolds] = useState<{ sig: string; closed: Set<string> }>(() => ({
    sig: "",
    closed: new Set(),
  }));
  const closedNow = filterFolds.sig === filterSig ? filterFolds.closed : null;
  const isCollapsed = useCallback(
    (path: string) => (transient ? (closedNow?.has(path) ?? false) : isStoredCollapsed(path)),
    [transient, closedNow, isStoredCollapsed],
  );
  const onToggle = (path: string) => {
    if (!transient) return toggle(path);
    const next = new Set(closedNow ?? []);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    setFilterFolds({ sig: filterSig, closed: next });
  };
  const sections = useMemo(() => {
    const out: Section[] = [];
    const walk = (nodes: NamespaceNode[], depth: number) => {
      for (const node of nodes) {
        out.push({ node, depth });
        // A collapsed namespace hides its own flows AND every namespace
        // beneath it: its header's count already says how many.
        if (!isCollapsed(node.path)) walk(node.children, depth + 1);
      }
    };
    walk(buildNamespaceTree(filtered), 0);
    return out;
  }, [filtered, isCollapsed]);

  const count = (activity: FlowActivity) => filtered.filter((f) => flowActivity(f.state) === activity).length;

  const content = (
      <ListStates
        isLoading={query.isLoading}
        isError={query.isError}
        error={query.error}
        errorText="The flows could not be loaded. Reload the page to try again."
        hasData={hasData}
        skeletonCols={COLS}
        empty={
          pinned ? (
            <EmptyState
              variant="dashed"
              icon={Workflow}
              title="No flows in this namespace yet"
              description="A flow belongs to the namespace its YAML names. Flows of the namespaces below this one are listed here too."
              action={
                <Button asChild>
                  <Link to={`${base}/flows/new`}>New flow</Link>
                </Button>
              }
            />
          ) : (
          <EmptyState
            variant="dashed"
            icon={Workflow}
            title="No flows yet"
            description="A flow is a set of tasks, written in YAML, that runs when you start it or when a trigger fires."
            action={
              <Button asChild>
                <Link to={`${base}/flows/new`}>New flow</Link>
              </Button>
            }
          />
          )
        }
      >
        <div className="mb-3 flex flex-wrap items-center gap-2">
          <div className="relative max-w-xs flex-1">
            <Label htmlFor="flows-search" className="sr-only">
              Search flows
            </Label>
            <Search
              aria-hidden="true"
              className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground"
            />
            <Input
              id="flows-search"
              className="pl-8"
              placeholder="Search flow id or namespace…"
              value={q}
              onChange={(e) => setParam("q", e.target.value, true)}
            />
          </div>
          {pinned ? null : (
            <FacetSelect
              label="Namespace"
              value={nsFilter || FACET_ALL}
              onChange={(v) => setParam("namespace", v === FACET_ALL ? "" : v, false)}
              options={namespaceOptions}
            />
          )}
          <FacetSelect
            label="State"
            value={stateFilter || FACET_ALL}
            onChange={(v) => setParam("state", v === FACET_ALL ? "" : v, false)}
            options={FLOW_STATES}
          />
        </div>

        <div className="mb-3" data-testid="flows-summary">
          <SummaryStrip
            items={[
              { label: "flows", singular: "flow", value: filtered.length },
              { label: "active", value: count("active") },
              { label: "disabled", value: count("disabled") },
            ]}
          />
        </div>

        {filtered.length === 0 && filtering ? (
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm text-muted-foreground">No flows match the current filters.</p>
            <Button variant="outline" size="sm" onClick={clearFilters}>
              Clear filters
            </Button>
          </div>
        ) : (
          <ScrollX>
            {/* A hand-written <table>, not `FilterableTable` (FE-PAGE-4): the
                list is grouped by a namespace tree, one <tbody> per namespace
                with its header row first — the shape of the grouped Explore
                lists (`ui-conventions.md` §1.5). `FilterableTable` models one
                flat entity and cannot hold a group. */}
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  <th className="py-1">Flow</th>
                  <th>State</th>
                  <th>Version</th>
                  <th>Updated</th>
                  <th>Created by</th>
                </tr>
              </thead>
              {sections.map(({ node, depth }) => {
                const collapsed = isCollapsed(node.path);
                return (
                  <tbody key={node.path}>
                    <NamespaceHeaderRow
                      node={node}
                      depth={depth}
                      collapsed={collapsed}
                      onToggle={() => onToggle(node.path)}
                    />
                    {!collapsed &&
                      node.flows.map((f) => (
                        <tr key={f.id} className="border-t hover:bg-accent">
                          <td className="py-1" style={{ paddingLeft: indent(depth + 1) }}>
                            <Link className={ENTITY_LINK_CLASS} to={`${base}/flows/${f.id}/edit`}>
                              {f.flow_id}
                            </Link>
                          </td>
                          <td>
                            <StateChip kind="flow" state={f.state} />
                          </td>
                          <td className="tabular-nums text-muted-foreground">
                            {/* The latest version, a snapshot to compare and restore; what
                                runs is the saved draft whatever this says. None yet: a draft
                                only, as the flow header says (KI-123). */}
                            {f.latest_version != null ? `v${f.latest_version}` : "Draft"}
                          </td>
                          <td className="text-muted-foreground">
                            <RelativeTime iso={f.updated_at} />
                          </td>
                          <td className="text-muted-foreground">
                            {/* Joined server-side (spec §7.4): null when there is no
                                creator or the account is gone. */}
                            {f.created_by_name ?? "—"}
                          </td>
                        </tr>
                      ))}
                  </tbody>
                );
              })}
            </table>
          </ScrollX>
        )}
      </ListStates>
  );

  if (pinned) return content;
  // The server refused the page's own read: the shared refusal, not a red
  // box with "forbidden" under a title bar that offers actions (KI-231).
  if (!pinned && isForbidden(query.error)) {
    return <Forbidden reason={refusalReason(query.error, "Workflows need the workflow:read permission in this workspace.")} />;
  }

  // The trash (KI-119), as `?view=trash` so it can be linked and Back leaves it (FE-PAGE-2). The
  // switch is not gated on the list having rows: an empty flow list says nothing about the trash.
  const trashToggle = (
    <ListToolbarButton
      icon={Trash2}
      label="Trash"
      active={showTrash}
      filled
      onClick={() =>
        setParams((p) => {
          if (showTrash) p.delete("view");
          else p.set("view", "trash");
          return p;
        })
      }
    />
  );
  if (showTrash) {
    return (
      <ListPageShell title="Trash" toolbar={trashToggle}>
        <FlowsTrash ws={ws} />
      </ListPageShell>
    );
  }

  return (
    <ListPageShell
      title="Flows"
      toolbar={trashToggle}
      action={
        <Button asChild>
          <Link to={`${base}/flows/new`}>New flow</Link>
        </Button>
      }
    >
      {content}
    </ListPageShell>
  );
}

/** Left padding for a tree depth: one step per level, plus the chevron's width for flows. */
function indent(depth: number): string {
  return `${0.25 + depth * 1.25}rem`;
}

/**
 * A namespace's header row: chevron, segment, and the number of flows beneath
 * it.
 *
 * The toggle is a native `<button>`, so Enter and Space work without a
 * handler (FE-A11Y-3), and `aria-expanded` says which way it points. Its
 * accessible name is the FULL namespace with the count, because a segment is
 * not unique (`a.x` and `b.x` both show `x`); the visible segment and count
 * are a substring of it, so what is seen is what is spoken.
 *
 * `role="rowheader"` on the spanning cell, as `FolderHeaderRow` does and for
 * the reasons recorded there. `FolderHeaderRow` itself is not reused: it has
 * no depth, no `aria-expanded`, and folder actions this tree has no use for.
 */
function NamespaceHeaderRow({
  node,
  depth,
  collapsed,
  onToggle,
}: {
  node: NamespaceNode;
  depth: number;
  collapsed: boolean;
  onToggle: () => void;
}) {
  const Chevron = collapsed ? ChevronRight : ChevronDown;
  return (
    <tr className="border-t bg-muted/40">
      <td colSpan={COLS} role="rowheader" className="py-1" style={{ paddingLeft: indent(depth) }}>
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded-sm font-medium hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onClick={onToggle}
          aria-expanded={!collapsed}
          aria-label={`${node.path} (${node.total})`}
          title={node.path}
        >
          <Chevron aria-hidden="true" className="h-4 w-4 text-muted-foreground" />
          {node.segment === "" ? (
            <span className="italic text-muted-foreground">{EMPTY_SEGMENT_LABEL}</span>
          ) : (
            node.segment
          )}
          <span className="tabular-nums text-muted-foreground">({node.total})</span>
        </button>
      </td>
    </tr>
  );
}
