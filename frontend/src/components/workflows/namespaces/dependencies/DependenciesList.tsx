import { Link } from "react-router-dom";
import type { NamespaceDependenciesDto, NamespaceDependencyEdgeDto, NamespaceDependencyNodeDto } from "@/api/generated/model";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { EDGE_KIND_WORD, dynamicSubflowsText, flowBadges, flowName } from "@/components/workflows/namespaces/dependencies/dependenciesModel";

/**
 * THE LIST VIEW of a namespace's dependencies (W7d spec §4.2): the canvas's accessible equivalent,
 * one row per flow in the server's order (namespace, then flow id), each naming the flows it calls
 * or triggers and the flows that call or trigger it, with the kind of each edge in words.
 *
 * W7c's `TopologyListView` is a treegrid nested by cluster; this graph has no clusters, and its
 * content is a flat table with an edge list per row, so it is `ui/table` (FE-PAGE-4), with one link
 * per flow (the keyboard reaches each with Tab).
 */
export function DependenciesList({ deps, flowHref }: { deps: NamespaceDependenciesDto; flowHref: (id: string) => string }) {
  const byId = new Map(deps.nodes.map((f) => [f.id, f]));
  const out = new Map<string, NamespaceDependencyEdgeDto[]>();
  const into = new Map<string, NamespaceDependencyEdgeDto[]>();
  for (const e of deps.edges) {
    if (!byId.has(e.source) || !byId.has(e.target)) continue;
    out.set(e.source, [...(out.get(e.source) ?? []), e]);
    into.set(e.target, [...(into.get(e.target) ?? []), e]);
  }
  const ends = (edges: NamespaceDependencyEdgeDto[] | undefined, end: "source" | "target") =>
    (edges ?? []).map((e) => `${flowName(byId.get(e[end]) as NamespaceDependencyNodeDto)} (${EDGE_KIND_WORD[e.kind]})`);

  return (
    <div className="h-full overflow-auto rounded-md border">
      <Table aria-label="Flows of the namespace and their dependencies">
        <TableHeader>
          <TableRow>
            <TableHead>Flow</TableHead>
            <TableHead>Namespace</TableHead>
            <TableHead>Calls or triggers</TableHead>
            <TableHead>Called or triggered by</TableHead>
            <TableHead>Notes</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {deps.nodes.map((f) => {
            const dynamic = dynamicSubflowsText(f.dynamic_subflows);
            const notes = [...flowBadges(f), ...(dynamic ? [dynamic] : [])];
            return (
              <TableRow key={f.id} data-flow={f.id} className={f.in_namespace ? undefined : "text-muted-foreground"}>
                <TableCell>
                  <Link className="font-mono text-sm font-medium hover:underline" to={flowHref(f.id)}>
                    {f.flow_id}
                  </Link>
                </TableCell>
                <TableCell className="text-sm">{f.namespace}</TableCell>
                <TableCell className="text-sm">
                  <EdgeList items={ends(out.get(f.id), "target")} />
                </TableCell>
                <TableCell className="text-sm">
                  <EdgeList items={ends(into.get(f.id), "source")} />
                </TableCell>
                <TableCell className="text-xs text-muted-foreground tabular-nums">
                  {notes.map((n) => (
                    <span key={n} className="block">
                      {n}
                    </span>
                  ))}
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}

function EdgeList({ items }: { items: string[] }) {
  if (items.length === 0) return null;
  return (
    <ul className="grid gap-0.5">
      {items.map((t) => (
        <li key={t} className="font-mono text-xs">
          {t}
        </li>
      ))}
    </ul>
  );
}
