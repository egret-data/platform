import { Fragment, useRef, type ComponentType, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { AppWindow, BarChart3, Database, FileCode2, Folder, LayoutDashboard, Table2, X } from "lucide-react";
import type { ApiError } from "@/api/client";
import type { BiObject, TableUsage } from "@/api/generated/model";
import { useControlPlaneGovernanceTagUsage } from "@/api/generated/control-plane-governance/control-plane-governance";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { IconButton } from "@/components/ui/icon-button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useCanEditCatalogTags } from "@/routes/governance/ClassificationChip";
import { useRemoveClassification, type RemoveTarget } from "@/routes/governance/RemoveClassificationDialog";
import { pathToUrl } from "@/routes/governance/data/objectPath";
import { ACTIONS_CELL, Section, STACKED_TABLE, TabError } from "@/routes/governance/tabParts";

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

/**
 * One Remove per assignment on the row (spec C4): "Table", then each column,
 * as Attached to lists them. Each line names what it removes; the button's
 * label names the classification and the whole path.
 */
function Removes({
  t,
  tagName,
  onRemove,
}: {
  t: TableUsage;
  tagName: string;
  onRemove: (column: string | undefined, label: string) => void;
}) {
  const base = `${t.catalogName}.${t.schema}.${t.table}`;
  const items: { key: string; column?: string; text: string; label: string }[] = [];
  if (t.onTable) items.push({ key: "", text: "Table", label: base });
  for (const c of [...t.columns].sort(byText)) items.push({ key: c, column: c, text: c, label: `${base}.${c}` });
  return (
    <ul className="space-y-0.5">
      {items.map((i) => (
        <li key={i.key} className="flex items-center justify-end gap-1 max-sm:justify-start">
          <span className={i.column === undefined ? "text-xs" : "font-mono text-xs"}>{i.text}</span>
          <IconButton aria-label={`Remove ${tagName} from ${i.label}`} onClick={() => onRemove(i.column, i.label)}>
            <X aria-hidden className="size-3.5" />
          </IconButton>
        </li>
      ))}
    </ul>
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
 *
 * A viewer who may edit catalog tags (`catalog_registry:update`, or platform
 * admin: `useCanEditCatalogTags`) gets a pinned actions column with one
 * Remove per assignment (spec C4), sent to the row's own `warehouseId`.
 */
export function WhereUsedTab({ tagId }: { tagId: string }) {
  const q = useControlPlaneGovernanceTagUsage(tagId, { query: { retry: false } });
  const canRemove = useCanEditCatalogTags();
  // After a remove, focus lands on the Data heading: the removed row's button, or the whole row, is gone.
  const dataHeading = useRef<HTMLHeadingElement>(null);
  const removal = useRemoveClassification(() => dataHeading.current);

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
  const { tag, data, bi } = q.data.data;
  const remove = (t: TableUsage, column: string | undefined, label: string) => {
    const target: RemoveTarget = { warehouseId: t.warehouseId, schema: t.schema, table: t.table, column, label };
    removal.ask({ tag, target });
  };
  const groups = groupTables(data.tables);
  const readable = [...bi.readable];
  const nothingBi = readable.length === 0 && bi.unreadable.count === 0 && bi.remaining === 0;

  return (
    <div className="space-y-6">
      <Section title="Data" headingRef={dataHeading}>
        {groups.length === 0 ? (
          // The Data page offers Add classification only with catalog_registry:update (FE-ACTION-4).
          <EmptyState
            icon={Table2}
            title="No table or column carries this classification."
            description={
              canRemove
                ? "Open a table or a column and use Add classification."
                : "Attaching needs the catalog_registry:update permission."
            }
            action={
              <Link to="/governance/data" className="text-sm font-medium text-primary hover:underline">
                {canRemove ? "Attach in Data" : "Open Data"}
              </Link>
            }
          />
        ) : (
          groups.map((g) => (
            <div key={g.label} className="space-y-1">
              {/* A long catalog.schema wraps at 390 px instead of running past the card (U3). */}
              <h3 className="break-all font-mono text-sm font-medium">{g.label}</h3>
              <div className="overflow-x-auto rounded-md border">
                {/* Cards below sm (U3): a pinned Remove column covered the table names at 390 px. */}
                <Table aria-label={g.label} className={STACKED_TABLE}>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Table</TableHead>
                      <TableHead>Attached to</TableHead>
                      {canRemove ? (
                        <TableHead className={ACTIONS_CELL}>
                          <span className="sr-only">Actions</span>
                        </TableHead>
                      ) : null}
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
                        <TableCell data-label="Attached to" className="whitespace-normal">
                          <Carries t={t} />
                        </TableCell>
                        {canRemove ? (
                          <TableCell className={`${ACTIONS_CELL} w-auto`}>
                            <Removes t={t} tagName={tag.name} onRemove={(column, label) => remove(t, column, label)} />
                          </TableCell>
                        ) : null}
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            </div>
          ))
        )}
        {removal.dialog}
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
            <Table aria-label="BI objects" className={STACKED_TABLE}>
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
                      <TableCell data-label="Kind">
                        <span className="inline-flex items-center gap-1.5">
                          <Icon className="h-4 w-4 text-muted-foreground" aria-hidden />
                          {k.word}
                        </span>
                      </TableCell>
                      <TableCell data-label="Workspace" className="font-mono text-xs">
                        {o.workspaceSlug}
                      </TableCell>
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
