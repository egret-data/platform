import { useMemo } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { FolderTree } from "lucide-react";
import { useWorkflowListNamespaces } from "@/api/generated/workflow/workflow";
import type { WorkflowNamespaceDto } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
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
  const lines = useMemo(() => {
    const out: { node: NamespaceRowNode; depth: number }[] = [];
    const walk = (nodes: NamespaceRowNode[], depth: number) => {
      for (const node of nodes) {
        out.push({ node, depth });
        walk(node.children, depth + 1);
      }
    };
    walk(buildNamespaceRowTree(filtered), 0);
    return out;
  }, [filtered]);

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
          <Table data-testid="namespaces-table">
            {/* A parent's row is not its subtree's total, as the Overview says too. */}
            <TableCaption className="mt-0 mb-2 caption-top text-left">{OWN_COUNTS_NOTE}</TableCaption>
            <TableHeader>
              <TableRow>
                <TableHead>Namespace</TableHead>
                <TableHead className="text-right">Flows</TableHead>
                <TableHead className="text-right">Files</TableHead>
                <TableHead className="text-right">KV keys</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {lines.map(({ node, depth }) => (
                <NamespaceRow key={node.path} node={node} depth={depth} ws={ws} />
              ))}
            </TableBody>
          </Table>
        )}
      </ListStates>
    </ListPageShell>
  );
}

/**
 * One namespace: its label, indented by depth, linking to its detail, and its
 * own counts.
 *
 * The link's accessible name is the FULL namespace, because a segment is not
 * unique (`a.x` and `b.x` both show `x`); the visible label is its suffix, so
 * what is seen is what is spoken. An empty segment shows
 * `EMPTY_SEGMENT_LABEL`, which the name then carries too.
 */
function NamespaceRow({ node, depth, ws }: { node: NamespaceRowNode; depth: number; ws: string }) {
  const parentOnly = !node.row.own;
  const empty = node.label === "";
  const count = (n: number) => (
    <TableCell className={`text-right tabular-nums ${parentOnly ? "text-muted-foreground" : ""}`}>{n}</TableCell>
  );
  return (
    <TableRow data-depth={depth}>
      <TableCell style={{ paddingLeft: `${0.5 + depth * 1.25}rem` }}>
        <span className="inline-flex flex-wrap items-center gap-2">
          <Link
            to={namespaceHref(ws, node.path)}
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
      </TableCell>
      {count(node.row.flows)}
      {count(node.row.files)}
      {count(node.row.kv_keys)}
    </TableRow>
  );
}
