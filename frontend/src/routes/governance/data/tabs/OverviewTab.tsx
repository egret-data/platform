import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { Columns3, Folder, Table2 } from "lucide-react";
import { useControlPlaneGovernanceObjectLineage } from "@/api/generated/control-plane-governance/control-plane-governance";
import { usePermListRules } from "@/api/generated/perm/perm";
import type { BrowseResponse, GovernanceCatalog, ResourceTagDto, RuleView, ViewLineageInfo } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { formatRuleKind } from "@/routes/governance/permFormat";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { KIND_ICON, type NodeKind } from "@/routes/governance/data/CatalogTree";
import { ruleSelects, tagKeysOn } from "@/routes/governance/data/coverage";
import {
  assignmentsOf,
  capitalise,
  engineTypeLabel,
  enginesOf,
  tagReach,
  type BrowseResult,
  type CatalogState,
  type TagReach,
  type TagsResult,
} from "@/routes/governance/data/objectData";
import { pathToUrl, type ObjectPath } from "@/routes/governance/data/objectPath";
import { CatalogGate, Section, TabError } from "@/routes/governance/data/tabs/TabStates";

const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name);

/**
 * An object's Overview tab (spec §2.2): a catalog's schemas, attached engines
 * and workspaces; a schema's tables and views; a table's columns with their
 * type, tags and the masking rules that select them, plus, for a view or materialized view, its security mode,
 * whether its owner is cleared and its lineage state.
 *
 * Each read has its own loading, empty and error state; a level browse could
 * not list shows browse's reason, never an empty list.
 */
export function OverviewTab({
  path,
  kind,
  catalog,
  browse,
  tags,
}: {
  path: ObjectPath;
  kind: NodeKind | null;
  catalog: CatalogState;
  browse: BrowseResult;
  tags: TagsResult;
}) {
  return (
    <CatalogGate catalog={catalog}>
      {(c) =>
        path.schema === undefined ? (
          <CatalogOverview catalog={c} catalogState={catalog} browse={browse} />
        ) : path.object === undefined ? (
          <Level browse={browse} what="tables and views">
            {(body) => <ObjectList path={path} items={body.items ?? []} />}
          </Level>
        ) : (
          <div className="space-y-6">
            {kind === "view" || kind === "materialized_view" ? (
              <ViewFacts catalogId={c.id} schema={path.schema} table={path.object} />
            ) : null}
            <Section title="Columns">
              <Level browse={browse} what="columns">
                {(body) => (
                  <Columns
                    path={path}
                    catalog={c}
                    columns={body.columns ?? []}
                    tags={tags}
                    taggable={c.warehouseId != null}
                  />
                )}
              </Level>
            </Section>
          </div>
        )
      }
    </CatalogGate>
  );
}

/** One browse level's states: loading, error, not listed, then its body. */
function Level({
  browse,
  what,
  children,
}: {
  browse: BrowseResult;
  what: string;
  children: (body: BrowseResponse) => ReactNode;
}) {
  if (browse.isError) return <TabError what={what} onRetry={() => void browse.refetch()} />;
  if (browse.data?.status !== 200) return <ListSkeleton rows={4} cols={2} />;
  const body = browse.data.data;
  if (body.notListed) {
    return (
      <Alert>
        <AlertDescription>{body.notListed.detail}</AlertDescription>
      </Alert>
    );
  }
  return <>{children(body)}</>;
}

function CatalogOverview({
  catalog,
  catalogState,
  browse,
}: {
  catalog: GovernanceCatalog;
  catalogState: CatalogState;
  browse: BrowseResult;
}) {
  const engines = enginesOf(catalogState, browse);
  const workspaces = [...catalog.workspaces].sort(byName);
  return (
    <div className="space-y-6">
      <Section title="Schemas">
        <Level browse={browse} what="schemas">
          {(body) => {
            const schemas = [...(body.items ?? [])].sort(byName);
            if (schemas.length === 0) return <EmptyState icon={Folder} title="No schemas" />;
            return (
              <ul aria-label="Schemas" className="flex flex-wrap gap-2">
                {schemas.map((s) => (
                  <li key={s.name}>
                    <Link
                      to={pathToUrl({ catalog: catalog.name, schema: s.name })}
                      className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-sm hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      {s.name}
                    </Link>
                  </li>
                ))}
              </ul>
            );
          }}
        </Level>
      </Section>

      <Section title="Attached engines">
        {engines.length === 0 ? (
          <p className="text-sm text-muted-foreground">Not attached to any engine</p>
        ) : (
          <Table aria-label="Attached engines">
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Type</TableHead>
                <TableHead>Origin</TableHead>
                <TableHead>Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {engines.map((e) => (
                <TableRow key={e.name}>
                  <TableCell className="font-medium">{e.name}</TableCell>
                  <TableCell>{e.type ? engineTypeLabel(e.type) : "—"}</TableCell>
                  <TableCell>{e.origin ? capitalise(e.origin) : "—"}</TableCell>
                  {/* An external engine's status is a placeholder (migration 0184). */}
                  <TableCell>{e.origin === "external" ? "—" : e.status ? capitalise(e.status) : "—"}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Section>

      <Section title="Workspaces">
        {workspaces.length === 0 ? (
          <p className="text-sm text-muted-foreground">Not assigned to any workspace</p>
        ) : (
          <ul aria-label="Workspaces" className="flex flex-wrap gap-1">
            {workspaces.map((w) => (
              <li key={w.id}>
                <Badge variant="outline" title={w.slug}>
                  {w.name}
                </Badge>
              </li>
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}

function ObjectList({ path, items }: { path: ObjectPath; items: NonNullable<BrowseResponse["items"]> }) {
  const objects = [...items].sort(byName);
  if (objects.length === 0) return <EmptyState icon={Table2} title="No tables or views" />;
  return (
    <Table aria-label="Tables and views">
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Kind</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {objects.map((o) => {
          const { icon: Icon, label } = KIND_ICON[o.kind];
          return (
            <TableRow key={o.name}>
              <TableCell>
                <Link
                  to={pathToUrl({ catalog: path.catalog, schema: path.schema, object: o.name })}
                  className="rounded-sm font-medium hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  {o.name}
                </Link>
              </TableCell>
              <TableCell>
                <span className="inline-flex items-center gap-1 text-muted-foreground">
                  <Icon aria-hidden className="size-4" />
                  {label}
                </span>
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

/**
 * A table's columns in the order the source returns them. The Tags column
 * shows each column's own tags, then the tags on its table marked "from
 * table" (a column inherits its table's tags). It is left out when the
 * catalog cannot carry tags (`taggable` false: not an egret catalog, whose
 * tags the page never asks for), and says so when the tags could not be read.
 *
 * The Masking rules column (shown with `policy:read`, which `GET /perm/rules`
 * needs) lists each column's mask rules by name and kind: those whose path
 * selects the column, and those whose tag the column carries itself or
 * through its table (`ruleSelects`, `tagKeysOn`). F10: which rules select a
 * column, never who sees what; only explain evaluates.
 */
function Columns({
  path,
  catalog,
  columns,
  tags,
  taggable,
}: {
  path: ObjectPath;
  catalog: GovernanceCatalog;
  columns: NonNullable<BrowseResponse["columns"]>;
  tags: TagsResult;
  taggable: boolean;
}) {
  const canReadRules = usePolicyPermissions()("policy:read");
  const rulesQ = usePermListRules({ query: { enabled: canReadRules, retry: false } });
  if (columns.length === 0) return <EmptyState icon={Columns3} title="No columns returned" />;
  const assignments = assignmentsOf(tags);
  const showTags = taggable && assignments !== null;
  const onTable = (assignments ?? []).filter((a) => a.column === "");
  const onColumn = (name: string) => (assignments ?? []).filter((a) => a.column === name);
  const masks =
    rulesQ.data?.status === 200 ? rulesQ.data.data.filter((r) => r.kind.type === "mask") : undefined;
  const reach = tagReach(path, catalog, tags);
  return (
    <div className="space-y-2">
      {showTags && tags.isError ? <TabError what="tags" onRetry={() => void tags.refetch()} /> : null}
      {canReadRules && rulesQ.isError ? (
        <TabError what="masking rules" onRetry={() => void rulesQ.refetch()} />
      ) : null}
      <Table aria-label="Columns">
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Type</TableHead>
            {showTags ? <TableHead>Tags</TableHead> : null}
            {canReadRules ? <TableHead>Masking rules</TableHead> : null}
          </TableRow>
        </TableHeader>
        <TableBody>
          {columns.map((col) => (
            <TableRow key={col.name}>
              <TableCell className="font-medium">{col.name}</TableCell>
              <TableCell className="font-mono text-xs">{col.type}</TableCell>
              {showTags ? (
                <TableCell>
                  {assignments === undefined ? (
                    tags.isError ? (
                      <span className="text-muted-foreground">—</span>
                    ) : (
                      <span className="sr-only">Loading tags</span>
                    )
                  ) : (
                    <ColumnTags column={col.name} own={onColumn(col.name)} inherited={onTable} />
                  )}
                </TableCell>
              ) : null}
              {canReadRules ? (
                <TableCell className="whitespace-normal">
                  {rulesQ.isError ? (
                    <span className="text-muted-foreground">—</span>
                  ) : masks === undefined || reach.status === "loading" ? (
                    <span className="sr-only">Loading masking rules</span>
                  ) : (
                    <ColumnMasks
                      column={col.name}
                      rules={masks.filter((r) => selectsColumn(r, { ...path, column: col.name }, catalog.id, reach))}
                      tagRulesUnknown={reach.status === "error" && masks.some((r) => r.selector.type === "tag")}
                    />
                  )}
                </TableCell>
              ) : null}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

function ColumnTags({ column, own, inherited }: { column: string; own: ResourceTagDto[]; inherited: ResourceTagDto[] }) {
  if (own.length === 0 && inherited.length === 0) return <span className="text-muted-foreground">—</span>;
  return (
    <ul aria-label={`Tags of ${column}`} className="flex flex-wrap gap-1">
      {own.map((a) => (
        <li key={`own-${a.tag.id}`}>
          <Badge variant="secondary" title={a.tag.key}>
            {a.tag.name}
          </Badge>
        </li>
      ))}
      {inherited.map((a) => (
        <li key={`table-${a.tag.id}`}>
          <Badge variant="outline" title={a.tag.key}>
            {a.tag.name}
            <span className="font-normal text-muted-foreground">{" from table"}</span>
          </Badge>
        </li>
      ))}
    </ul>
  );
}

/** Whether mask rule `r` selects the column `p`, with the tags it can find there. */
function selectsColumn(r: RuleView, p: ObjectPath, catalogId: string, reach: TagReach): boolean {
  const keys = reach.status === "ok" ? tagKeysOn(reach.assignments, p) : [];
  return ruleSelects(r, p, catalogId, keys) !== null;
}

function ColumnMasks({ column, rules, tagRulesUnknown }: { column: string; rules: RuleView[]; tagRulesUnknown: boolean }) {
  const unknown = tagRulesUnknown ? <span className="text-xs text-muted-foreground">Tag rules not checked</span> : null;
  if (rules.length === 0) return unknown ?? <span className="text-muted-foreground">—</span>;
  const sorted = [...rules].sort((a, b) => a.name.localeCompare(b.name));
  return (
    <div className="space-y-1">
      <ul aria-label={`Masking rules of ${column}`} className="space-y-0.5">
        {sorted.map((r) => (
          <li key={r.id} className="text-sm break-words">
            {r.name}
            <span className="text-xs text-muted-foreground">{` ${formatRuleKind(r)}`}</span>
          </li>
        ))}
      </ul>
      {unknown}
    </div>
  );
}

/**
 * A view's or materialized view's facts from the lineage route. egret records
 * them for its own catalogs' Trino views; for anything else the route has no
 * `view`, and this says so rather than guessing.
 */
function ViewFacts({ catalogId, schema, table }: { catalogId: string; schema: string; table: string }) {
  const lineage = useControlPlaneGovernanceObjectLineage(catalogId, { schema, table }, { query: { retry: false } });
  let body: ReactNode;
  if (lineage.isError) {
    body = <TabError what="view's security mode and lineage" onRetry={() => void lineage.refetch()} />;
  } else if (lineage.data?.status !== 200) {
    body = <ListSkeleton rows={3} cols={2} />;
  } else if (!lineage.data.data.view) {
    body = <p className="text-sm text-muted-foreground">egret has no security mode or lineage recorded for this object.</p>;
  } else {
    body = <FactList view={lineage.data.data.view} />;
  }
  return <Section title="Security and lineage">{body}</Section>;
}

function FactList({ view }: { view: ViewLineageInfo }) {
  const mode =
    view.securityMode === "unknown"
      ? `Unknown${view.securityModeError ? `: ${view.securityModeError}` : ""}`
      : view.securityMode.toUpperCase();
  const cleared = view.ownerCleared === true ? "Yes" : view.ownerCleared === false ? "No" : "Not known";
  const state =
    view.lineageState === "computed"
      ? "Recorded"
      : view.lineageState === "missing"
        ? "Not recorded"
        : `Failed${view.lineageDetail ? `: ${view.lineageDetail}` : ""}`;
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
