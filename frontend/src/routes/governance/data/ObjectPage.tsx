import { useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import {
  useControlPlaneGovernanceBrowse,
  useControlPlaneGovernanceListCatalogs,
  useControlPlaneGovernanceObjectTags,
} from "@/api/generated/control-plane-governance/control-plane-governance";
import type { GrantView } from "@/api/generated/model";
import { GrantDialog } from "@/routes/governance/GrantDialog";
import type { NodeKind } from "@/routes/governance/data/CatalogTree";
import { ObjectHeader, type HeaderKind, type HeaderTags } from "@/routes/governance/data/ObjectHeader";
import { ObjectTabs, type ObjectLevel } from "@/routes/governance/data/ObjectTabs";
import { assignmentsOf, enginesOf, scopeOf, type CatalogState, type ObjectKind, type TagsResult } from "@/routes/governance/data/objectData";
import { urlToPath } from "@/routes/governance/data/objectPath";
import { DetailsTab } from "@/routes/governance/data/tabs/DetailsTab";
import { OverviewTab } from "@/routes/governance/data/tabs/OverviewTab";
import { LineageTab } from "@/routes/governance/data/tabs/LineageTab";
import { MasksFiltersTab } from "@/routes/governance/data/tabs/MasksFiltersTab";
import { PermissionsTab } from "@/routes/governance/data/tabs/PermissionsTab";

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
 * where its kind comes from; and, for an object in an egret catalog, its
 * tags (no other catalog can carry them, F9, so none is asked).
 *
 * The page owns the one `GrantDialog`: the header's Grant access opens it on
 * this object's scope, the Permissions tab's Edit on an existing grant.
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
    { query: { enabled: !!found && found.warehouseId != null && level === "object", retry: false } },
  );

  let kind: NodeKind | null = level === "object" ? null : level;
  if (level === "object" && parent.data?.status === 200) {
    const item = parent.data.data.items?.find((i) => i.name === path.object);
    if (item && OBJECT_KINDS.includes(item.kind)) kind = item.kind;
  }
  // An object's kind is "loading" only while something that can still tell it is on its way.
  const headerKind: HeaderKind =
    kind ??
    (catalog.status === "error" || catalog.status === "missing" || parent.isError || parent.data?.status === 200
      ? "unknown"
      : "loading");

  const [grantDialog, setGrantDialog] = useState<{ existing: GrantView | null } | null>(null);
  const onGrant = found ? () => setGrantDialog({ existing: null }) : undefined;

  return (
    <div className="space-y-4">
      <ObjectHeader
        path={path}
        kind={headerKind}
        engines={enginesOf(catalog, browse)}
        tags={headerTags(level, catalog, tags)}
        onGrant={onGrant}
      />
      <ObjectTabs
        level={level}
        panels={{
          overview: <OverviewTab path={path} kind={kind} catalog={catalog} browse={browse} tags={tags} />,
          permissions: (
            <PermissionsTab
              path={path}
              kind={kind}
              catalog={catalog}
              onEdit={(existing) => setGrantDialog({ existing })}
            />
          ),
          masks: <MasksFiltersTab path={path} kind={kind} catalog={catalog} tags={tags} />,
          lineage: <LineageTab path={path} catalog={catalog} />,
          details: <DetailsTab path={path} catalog={catalog} />,
        }}
      />
      <GrantDialog
        open={grantDialog !== null}
        onOpenChange={(open) => {
          if (!open) setGrantDialog(null);
        }}
        existing={grantDialog?.existing ?? null}
        initialScope={found ? scopeOf(path, found.id) : undefined}
      />
    </div>
  );
}

/** The header's tag section: only an object in an egret catalog has one (tags are table and column only). */
function headerTags(level: ObjectLevel, catalog: CatalogState, tags: TagsResult): HeaderTags {
  if (level !== "object") return null;
  if (catalog.status === "error" || catalog.status === "missing") return null;
  if (catalog.status === "ok" && catalog.catalog.warehouseId == null) return null;
  if (tags.isError) return { state: "error" };
  const assignments = assignmentsOf(tags);
  if (assignments === null) return null;
  if (assignments === undefined) return { state: "loading" };
  return { state: "ok", tags: assignments.filter((a) => a.column === "").map((a) => a.tag) };
}
