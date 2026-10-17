import { Link, useParams, useSearchParams } from "react-router-dom";
import {
  useControlPlaneGovernanceBrowse,
  useControlPlaneGovernanceListCatalogs,
  useControlPlaneGovernanceObjectTags,
} from "@/api/generated/control-plane-governance/control-plane-governance";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import type { NodeKind } from "@/routes/governance/data/CatalogTree";
import { ObjectHeader, type HeaderTags } from "@/routes/governance/data/ObjectHeader";
import { ObjectTabs, type ObjectLevel } from "@/routes/governance/data/ObjectTabs";
import { assignmentsOf, enginesOf, type CatalogState, type ObjectKind, type TagsResult } from "@/routes/governance/data/objectData";
import { urlToPath } from "@/routes/governance/data/objectPath";
import { DetailsTab } from "@/routes/governance/data/tabs/DetailsTab";
import { OverviewTab } from "@/routes/governance/data/tabs/OverviewTab";

const OBJECT_KINDS: readonly string[] = ["table", "view", "materialized_view"] satisfies ObjectKind[];

/**
 * An object's page (spec §2.2): a catalog, a schema, or a table, view or
 * materialized view, named by the URL. The header, then the level's tabs.
 *
 * The header never waits on the API: its name and path come from the URL,
 * and each thing it reads (the catalog, its engines, the object's kind and
 * tags) has its own state, so one failing call never blanks it or the tabs
 * (spec §2.5).
 *
 * Reads, each shared through the query cache with the tree and the tabs:
 * the Governance catalog list (the catalog's id, warehouse, workspaces and
 * engine names); browse at this level (the engines' details, and what the
 * Overview lists); browse at the parent schema, for an object, which is
 * where its kind comes from; and, for an object, its tags.
 */
export default function ObjectPage() {
  const [search] = useSearchParams();
  const path = urlToPath(useParams(), search);
  const level: ObjectLevel = path.schema === undefined ? "catalog" : path.object === undefined ? "schema" : "object";

  const catalogsQ = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  const found = catalogsQ.data?.status === 200 ? catalogsQ.data.data.find((c) => c.name === path.catalog) : undefined;
  const catalog: CatalogState = catalogsQ.isError
    ? { status: "error", retry: () => void catalogsQ.refetch() }
    : catalogsQ.data?.status !== 200
      ? { status: "loading" }
      : found
        ? { status: "ok", catalog: found }
        : { status: "missing", name: path.catalog };
  const catalogId = found?.id ?? "";

  // The same parameters the tree passes, so the two share one cache entry.
  const levelParams =
    path.schema === undefined
      ? undefined
      : path.object === undefined
        ? { schema: path.schema }
        : { schema: path.schema, table: path.object };
  const browse = useControlPlaneGovernanceBrowse(catalogId, levelParams, {
    query: { enabled: !!found, retry: false },
  });
  const parent = useControlPlaneGovernanceBrowse(
    catalogId,
    { schema: path.schema ?? "" },
    { query: { enabled: !!found && level === "object", retry: false } },
  );
  const tags = useControlPlaneGovernanceObjectTags(
    catalogId,
    { schema: path.schema ?? "", table: path.object ?? "" },
    { query: { enabled: !!found && level === "object", retry: false } },
  );

  let kind: NodeKind | null = level === "object" ? null : level;
  if (level === "object" && parent.data?.status === 200) {
    const item = parent.data.data.items?.find((i) => i.name === path.object);
    if (item && OBJECT_KINDS.includes(item.kind)) kind = item.kind;
  }

  return (
    <div className="space-y-4">
      <ObjectHeader path={path} kind={kind} engines={enginesOf(catalog, browse)} tags={headerTags(level, catalog, tags)} />
      <ObjectTabs
        level={level}
        panels={{
          overview: <OverviewTab path={path} kind={kind} catalog={catalog} browse={browse} tags={tags} />,
          permissions: <ElsewhereNote what="grants" to="/governance/data-permissions" page="Data permissions" />,
          masks: <ElsewhereNote what="mask and row-filter rules" to="/governance/mask-filter-rules" page="Mask & filter rules" />,
          lineage: <p className="text-sm text-muted-foreground">Lineage is not shown on this page yet.</p>,
          details: <DetailsTab path={path} catalog={catalog} />,
        }}
      />
    </div>
  );
}

/** The header's tag section: only an object in an egret catalog has one (tags are table and column only). */
function headerTags(level: ObjectLevel, catalog: CatalogState, tags: TagsResult): HeaderTags {
  if (level !== "object") return null;
  if (catalog.status === "error" || catalog.status === "missing") return null;
  if (tags.isError) return { state: "error" };
  const assignments = assignmentsOf(tags);
  if (assignments === null) return null;
  if (assignments === undefined) return { state: "loading" };
  return { state: "ok", tags: assignments.filter((a) => a.column === "").map((a) => a.tag) };
}

/**
 * A tab whose list still lives on its own Governance page. The link shows
 * only to a viewer who can open that page (`policy:read`).
 */
function ElsewhereNote({ what, to, page }: { what: string; to: string; page: string }) {
  const can = usePolicyPermissions();
  return (
    <p className="text-sm text-muted-foreground">
      {`The ${what} that reach this object are not listed on this page yet.`}
      {can("policy:read") ? (
        <>
          {" "}
          <Link to={to} className="text-foreground underline underline-offset-2 hover:no-underline">
            {`Open ${page}`}
          </Link>
        </>
      ) : null}
    </p>
  );
}
