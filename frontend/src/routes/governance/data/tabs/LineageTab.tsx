import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { TriangleAlert } from "lucide-react";
import {
  useControlPlaneGovernanceListCatalogs,
  useControlPlaneGovernanceObjectLineage,
} from "@/api/generated/control-plane-governance/control-plane-governance";
import type { GovernanceCatalog, LineageReader, LineageTableRef, ViewLineageInfo } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { KIND_ICON } from "@/routes/governance/data/CatalogTree";
import { capitalise, type CatalogState } from "@/routes/governance/data/objectData";
import { pathToUrl, type ObjectPath } from "@/routes/governance/data/objectPath";
import { CatalogGate } from "@/routes/governance/data/tabs/TabStates";
import { Section, TabError } from "@/routes/governance/tabParts";

/** The empty reader list of a complete answer. Also this module's bundle marker (`scripts/assert-governance-data-lazy.mjs`). */
export const NO_READERS = "No view or materialized view reads this table";

const MATCHED_BY: Record<LineageReader["matchedBy"], string> = {
  lineage: "Recorded lineage",
  baseTable: "Base table",
  text: "Matched by definition text, may over-report",
};

/**
 * An object's Lineage tab (spec §2.2): the views and materialized views that
 * read it, and, for a view or materialized view, the tables it reads from
 * with its security mode and lineage state. One read, `GET /governance/lineage`,
 * shared with the Overview tab's view facts through the query cache.
 *
 * It never claims more than egret knows: `readersIncomplete` is shown as a
 * warning whenever the list may miss readers, and an empty list is only
 * called empty when that is absent. A reader whose catalog is not in the
 * catalog list is named without a link.
 */
export function LineageTab({ path, catalog }: { path: ObjectPath; catalog: CatalogState }) {
  return <CatalogGate catalog={catalog}>{(c) => <Lineage path={path} catalog={c} />}</CatalogGate>;
}

function Lineage({ path, catalog }: { path: ObjectPath; catalog: GovernanceCatalog }) {
  const lineage = useControlPlaneGovernanceObjectLineage(
    catalog.id,
    { schema: path.schema ?? "", table: path.object ?? "" },
    // Fresh for a while, so opening this tab right after Overview reuses its answer instead of asking again.
    { query: { retry: false, staleTime: 30_000 } },
  );
  const catalogsQ = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  const known = new Set(catalogsQ.data?.status === 200 ? catalogsQ.data.data.map((c) => c.name) : []);

  if (lineage.isError) {
    const detail = lineage.error instanceof ApiError ? lineage.error.message : undefined;
    return <TabError what="lineage" detail={detail} onRetry={() => void lineage.refetch()} />;
  }
  // The page's own catalog gate already failed if the list did; wait for it here too, so a reader never shows unlinked and then linked.
  if (lineage.data?.status !== 200 || catalogsQ.data?.status !== 200) {
    return <ListSkeleton rows={4} cols={3} />;
  }
  const { readers, readersIncomplete, view } = lineage.data.data;
  const linkable = (name: string) => known.has(name);

  return (
    <div className="space-y-6">
      {view !== undefined ? <ViewSection view={view} linkable={linkable} /> : null}
      <Section title="Read by">
        {readersIncomplete ? (
          <Alert>
            <TriangleAlert aria-hidden />
            <AlertDescription>{`This list may be incomplete. ${readersIncomplete.detail}`}</AlertDescription>
          </Alert>
        ) : null}
        <Readers readers={readers} incomplete={!!readersIncomplete} linkable={linkable} />
      </Section>
    </div>
  );
}

function CatalogName({ name, linkable }: { name: string; linkable: (name: string) => boolean }) {
  return linkable(name) ? (
    <Link to={pathToUrl({ catalog: name })} className="text-primary underline-offset-2 hover:underline">
      {name}
    </Link>
  ) : (
    <span>{name}</span>
  );
}

function Readers({
  readers,
  incomplete,
  linkable,
}: {
  readers: LineageReader[];
  incomplete: boolean;
  linkable: (name: string) => boolean;
}) {
  if (readers.length === 0) {
    return <p className="text-sm text-muted-foreground">{incomplete ? "No reader is known." : NO_READERS}</p>;
  }
  return (
    <Table aria-label="Readers">
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Kind</TableHead>
          <TableHead>Catalog</TableHead>
          <TableHead>Schema</TableHead>
          <TableHead>Found by</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {readers.map((r) => {
          const { icon: Icon, label } = KIND_ICON[r.kind];
          return (
            <TableRow key={`${r.catalog}.${r.schema}.${r.name}.${r.engine ?? ""}`}>
              <TableCell className="break-all font-medium">
                <span className="inline-flex items-center gap-2">
                  <Icon aria-hidden className="size-4 shrink-0 text-muted-foreground" />
                  <ObjectName catalog={r.catalog} schema={r.schema} name={r.name} linkable={linkable} />
                </span>
              </TableCell>
              <TableCell>{label}</TableCell>
              <TableCell className="break-all">
                <CatalogName name={r.catalog} linkable={linkable} />
              </TableCell>
              <TableCell className="break-all">{r.schema}</TableCell>
              <TableCell>{MATCHED_BY[r.matchedBy] + (r.engine ? ` (${r.engine})` : "")}</TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

/** An object's name, linked to its page when its catalog is one the portal lists. */
function ObjectName({
  catalog,
  schema,
  name,
  linkable,
}: {
  catalog: string;
  schema: string;
  name: string;
  linkable: (name: string) => boolean;
}) {
  if (!linkable(catalog)) return <span>{name}</span>;
  return (
    <Link to={pathToUrl({ catalog, schema, object: name })} className="text-primary underline-offset-2 hover:underline">
      {name}
    </Link>
  );
}

function ViewSection({ view, linkable }: { view: ViewLineageInfo | null; linkable: (name: string) => boolean }) {
  if (!view) {
    return (
      <Section title="Reads from">
        <p className="text-sm text-muted-foreground">egret has no lineage recorded for this object.</p>
      </Section>
    );
  }
  return (
    <>
      <Section title="Reads from">
        <Bases view={view} linkable={linkable} />
      </Section>
      <Section title="View">
        <Facts view={view} />
      </Section>
    </>
  );
}

function Bases({ view, linkable }: { view: ViewLineageInfo; linkable: (name: string) => boolean }) {
  if (view.lineageState !== "computed") {
    return <p className="text-sm text-muted-foreground">The tables this view reads are not known.</p>;
  }
  if (view.bases.length === 0) return <p className="text-sm text-muted-foreground">No base tables are recorded for this view.</p>;
  return (
    <Table aria-label="Base tables">
      <TableHeader>
        <TableRow>
          <TableHead>Table</TableHead>
          <TableHead>Catalog</TableHead>
          <TableHead>Schema</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {view.bases.map((b: LineageTableRef) => (
          <TableRow key={`${b.catalog}.${b.schema}.${b.table}`}>
            <TableCell className="break-all font-medium">
              <ObjectName catalog={b.catalog} schema={b.schema} name={b.table} linkable={linkable} />
            </TableCell>
            <TableCell className="break-all">
              <CatalogName name={b.catalog} linkable={linkable} />
            </TableCell>
            <TableCell className="break-all">{b.schema}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function Facts({ view }: { view: ViewLineageInfo }) {
  const mode =
    view.securityMode === "unknown"
      ? `Unknown${view.securityModeError ? `: ${view.securityModeError}` : ""}`
      : view.securityMode.toUpperCase();
  const cleared = view.ownerCleared === true ? "Yes" : view.ownerCleared === false ? "No" : "Not known";
  let state: ReactNode;
  if (view.lineageState === "computed") state = "Recorded: egret read the view's definition and recorded the tables it reads.";
  else if (view.lineageState === "missing") state = "Not recorded: egret has no lineage for this view yet.";
  else {
    // The detail is the backend's text, which may start lowercase; it follows a full stop here.
    state = `Failed: egret could not work out what this view reads.${view.lineageDetail ? ` ${capitalise(view.lineageDetail)}` : ""}`;
  }
  return (
    <dl aria-label="View facts" className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-6 gap-y-2 text-sm">
      <dt className="text-muted-foreground">Security mode</dt>
      <dd>{mode}</dd>
      {view.securityMode === "definer" ? (
        <>
          <dt className="text-muted-foreground">Owner</dt>
          <dd className="break-all">{view.owner ?? "None"}</dd>
        </>
      ) : null}
      <dt className="text-muted-foreground">Owner cleared</dt>
      <dd>{cleared}</dd>
      <dt className="text-muted-foreground">Lineage</dt>
      <dd>{state}</dd>
    </dl>
  );
}
