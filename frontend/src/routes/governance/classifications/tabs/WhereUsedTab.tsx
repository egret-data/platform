import { Fragment, type ComponentType, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { AppWindow, BarChart3, Database, FileCode2, Folder, LayoutDashboard, Table2 } from "lucide-react";
import type { ApiError } from "@/api/client";
import type { BiObject, TableUsage } from "@/api/generated/model";
import { useControlPlaneGovernanceTagUsage } from "@/api/generated/control-plane-governance/control-plane-governance";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { pathToUrl } from "@/routes/governance/data/objectPath";
import { Section, TabError } from "@/routes/governance/tabParts";

const plural = (n: number, one: string, many: string) => (n === 1 ? one : many);

/** CL13: the BI kinds with a page of their own, and its path segment. Saved queries and folders have none. */
const KIND_PATH: Record<string, string> = {
  dashboard: "dashboards",
  chart: "charts",
  dataset: "datasets",
  app: "apps",
};

const KIND: Record<string, { word: string; icon: ComponentType<{ className?: string }> }> = {
  dashboard: { word: "Dashboard", icon: LayoutDashboard },
  chart: { word: "Chart", icon: BarChart3 },
  dataset: { word: "Dataset", icon: Database },
  app: { word: "App", icon: AppWindow },
  saved_query: { word: "Saved query", icon: FileCode2 },
  folder: { word: "Folder", icon: Folder },
};

const byText = (a: string, b: string) => a.localeCompare(b);

/** Tables grouped by catalog, then schema, each level sorted (the order is the page's, not the server's). */
function groupTables(tables: TableUsage[]): { label: string; rows: TableUsage[] }[] {
  const groups = new Map<string, TableUsage[]>();
  const sorted = [...tables].sort(
    (a, b) => byText(a.catalogName, b.catalogName) || byText(a.schema, b.schema) || byText(a.table, b.table),
  );
  for (const t of sorted) {
    const label = `${t.catalogName}.${t.schema}`;
    groups.set(label, [...(groups.get(label) ?? []), t]);
  }
  return [...groups].map(([label, rows]) => ({ label, rows }));
}

function Carries({ t }: { t: TableUsage }) {
  const columns = [...t.columns].sort(byText);
  const parts: ReactNode[] = [];
  if (t.onTable) parts.push("Table");
  if (columns.length > 0) {
    parts.push(
      <span key="columns">
        {"Columns: "}
        {columns.map((c, i) => (
          <Fragment key={c}>
            {i > 0 ? ", " : null}
            <Link
              to={pathToUrl({ catalog: t.catalogName, schema: t.schema, object: t.table, column: c })}
              className="font-mono text-primary hover:underline"
            >
              {c}
            </Link>
          </Fragment>
        ))}
      </span>,
    );
  }
  return (
    <>
      {parts.map((p, i) => (
        <Fragment key={i}>
          {i > 0 ? " · " : null}
          {p}
        </Fragment>
      ))}
    </>
  );
}

function Note({ children }: { children: ReactNode }) {
  return <p className="text-sm text-muted-foreground">{children}</p>;
}

function BiName({ o }: { o: BiObject }) {
  const path = KIND_PATH[o.kind];
  if (!path) return <span className="font-medium">{o.name}</span>;
  return (
    <Link
      to={`/explore/${encodeURIComponent(o.workspaceSlug)}/${path}/${encodeURIComponent(o.id)}`}
      className="font-medium text-primary hover:underline"
    >
      {o.name}
    </Link>
  );
}

/**
 * Where used (spec §5.1, C1), from route C: the egret-catalog tables and
 * columns carrying the classification, and the BI objects carrying it that
 * this reader may read. Unreadable objects are only counted (Q5); assignments
 * the server could not place (orphaned warehouse, deleted object) and objects
 * past the check cap are each stated, never dropped (CL11, CL12). A failed
 * read is an error with Retry, never an empty sentence.
 */
export function WhereUsedTab({ tagId }: { tagId: string }) {
  const q = useControlPlaneGovernanceTagUsage(tagId, { query: { retry: false } });

  if (q.isError) {
    const message = (q.error as unknown as ApiError | null)?.message;
    return <TabError what="classification's usage" onRetry={() => void q.refetch()} detail={message} />;
  }
  if (q.data?.status !== 200) {
    return (
      <div className="space-y-6">
        <Section title="Data">
          <ListSkeleton rows={3} cols={2} />
        </Section>
        <Section title="BI">
          <ListSkeleton rows={3} cols={3} />
        </Section>
      </div>
    );
  }
  const { data, bi } = q.data.data;
  const groups = groupTables(data.tables);
  const readable = [...bi.readable];
  const nothingBi = readable.length === 0 && bi.unreadable.count === 0 && bi.remaining === 0;

  return (
    <div className="space-y-6">
      <Section title="Data">
        {groups.length === 0 ? (
          <EmptyState icon={Table2} title="No table or column carries this classification." />
        ) : (
          groups.map((g) => (
            <div key={g.label} className="space-y-1">
              <h3 className="font-mono text-sm font-medium">{g.label}</h3>
              <div className="overflow-x-auto rounded-md border">
                <Table aria-label={g.label}>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Table</TableHead>
                      <TableHead>Carries it</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {g.rows.map((t) => (
                      <TableRow key={`${t.catalogId}/${t.schema}/${t.table}`}>
                        <TableCell>
                          <Link
                            to={pathToUrl({ catalog: t.catalogName, schema: t.schema, object: t.table })}
                            className="font-mono text-primary hover:underline"
                          >
                            {t.table}
                          </Link>
                        </TableCell>
                        <TableCell className="whitespace-normal">
                          <Carries t={t} />
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            </div>
          ))
        )}
        {data.orphaned > 0 ? (
          <Note>
            {`${data.orphaned} ${plural(data.orphaned, "assignment is", "assignments are")} in a warehouse no catalog names.`}
          </Note>
        ) : null}
      </Section>

      <Section title="BI">
        {nothingBi ? (
          <EmptyState icon={LayoutDashboard} title="No BI object carries this classification." />
        ) : readable.length > 0 ? (
          <div className="overflow-x-auto rounded-md border">
            <Table aria-label="BI objects">
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Kind</TableHead>
                  <TableHead>Workspace</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {readable.map((o) => {
                  const k = KIND[o.kind] ?? { word: o.kind, icon: Folder };
                  const Icon = k.icon;
                  return (
                    <TableRow key={`${o.kind}/${o.id}`}>
                      <TableCell className="whitespace-normal">
                        <BiName o={o} />
                      </TableCell>
                      <TableCell>
                        <span className="inline-flex items-center gap-1.5">
                          <Icon className="h-4 w-4 text-muted-foreground" aria-hidden />
                          {k.word}
                        </span>
                      </TableCell>
                      <TableCell className="font-mono text-xs">{o.workspaceSlug}</TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          </div>
        ) : null}
        {bi.unreadable.count > 0 ? (
          <Note>{`${bi.unreadable.count} ${plural(bi.unreadable.count, "object", "objects")} you cannot read.`}</Note>
        ) : null}
        {bi.deleted > 0 ? (
          <Note>
            {bi.deleted === 1
              ? "1 assignment points at a deleted object."
              : `${bi.deleted} assignments point at deleted objects.`}
          </Note>
        ) : null}
        {bi.remaining > 0 ? <Note>{`Not all BI objects were checked: ${bi.remaining} remain.`}</Note> : null}
      </Section>
    </div>
  );
}
