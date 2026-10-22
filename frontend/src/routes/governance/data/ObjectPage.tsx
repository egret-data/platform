import { useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import {
  useControlPlaneGovernanceBrowse,
  useControlPlaneGovernanceListCatalogs,
  useControlPlaneGovernanceObjectTags,
} from "@/api/generated/control-plane-governance/control-plane-governance";
import type { GrantView, TagDto } from "@/api/generated/model";
import { GrantDialog } from "@/routes/governance/GrantDialog";
import { useCanEditCatalogTags } from "@/routes/governance/ClassificationChip";
import { useRemoveClassification } from "@/routes/governance/RemoveClassificationDialog";
import { AddTagDialog, TAGS_EGRET_ONLY, type TagTarget } from "@/routes/governance/data/AddTagDialog";
import { dottedPath, type NodeKind } from "@/routes/governance/data/CatalogTree";
import { ColumnPanel } from "@/routes/governance/data/ColumnPanel";
import { ObjectHeader, type HeaderKind, type HeaderTags } from "@/routes/governance/data/ObjectHeader";
import { ObjectTabs, type ObjectLevel } from "@/routes/governance/data/ObjectTabs";
import { assignmentsOf, enginesOf, scopeOf, type CatalogState, type ObjectKind, type TagsResult } from "@/routes/governance/data/objectData";
import { urlToPath } from "@/routes/governance/data/objectPath";
import { sameName } from "@/routes/governance/data/coverage";
import { DetailsTab } from "@/routes/governance/data/tabs/DetailsTab";
import { OverviewTab } from "@/routes/governance/data/tabs/OverviewTab";
import { LineageTab } from "@/routes/governance/data/tabs/LineageTab";
import { MasksFiltersTab } from "@/routes/governance/data/tabs/MasksFiltersTab";
import { PermissionsTab } from "@/routes/governance/data/tabs/PermissionsTab";
import { WhoCanReadDialog } from "@/routes/governance/data/WhoCanReadDialog";

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
 * this object's scope, the Permissions tab's Edit on an existing grant. It
 * also owns the header's Who can read? (`WhoCanReadDialog`, on this object)
 * and Add tag (`AddTagDialog`, on a table, view or materialized view; on one
 * outside egret's catalogs the button shows disabled with F9's reason), and
 * the column panel `?column=` opens (`ColumnPanel`), whose own dialogs ask
 * about the column.
 */
export default function ObjectPage() {
  const [search, setSearch] = useSearchParams();
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

  // The object as its schema's listing names it: the URL may differ in case,
  // which the engines and egret's tags ignore; the listing's spelling is the source's.
  const siblings = level === "object" && parent.data?.status === 200 ? (parent.data.data.items ?? []) : [];
  const item = siblings.find((i) => i.name === path.object) ?? siblings.find((i) => sameName(i.name, path.object ?? ""));
  let kind: NodeKind | null = level === "object" ? null : level;
  if (item && OBJECT_KINDS.includes(item.kind)) kind = item.kind;
  // An object's kind is "loading" only while something that can still tell it is on its way.
  const headerKind: HeaderKind =
    kind ??
    (catalog.status === "error" || catalog.status === "missing" || parent.isError || parent.data?.status === 200
      ? "unknown"
      : "loading");

  const [grantDialog, setGrantDialog] = useState<{ existing: GrantView | null } | null>(null);
  const onGrant = found ? () => setGrantDialog({ existing: null }) : undefined;

  const [whoCanRead, setWhoCanRead] = useState(false);
  const onWhoCanRead = found ? () => setWhoCanRead(true) : undefined;

  const [addTag, setAddTag] = useState<TagTarget | null>(null);
  const onAddTag =
    found && level === "object"
      ? () => {
          if (found.warehouseId == null || path.schema === undefined || path.object === undefined) return;
          setAddTag({
            catalog: found.name,
            catalogId: found.id,
            warehouseId: found.warehouseId,
            schema: path.schema,
            // Written with the source's spelling, as the column panel writes a column's.
            table: item?.name ?? path.object,
            assigned: (assignmentsOf(tags) ?? []).filter((a) => a.column === "").map((a) => a.tag.id),
            // For "Attach to columns instead" (spec C5): the columns as the source lists them.
            columns: browse.data?.status === 200 ? (browse.data.data.columns ?? []).map((c) => c.name) : undefined,
          });
        }
      : undefined;

  // Remove on the header's own classifications (spec C4), with the coordinate as stored.
  const canEditTags = useCanEditCatalogTags();
  // After a remove, focus lands on the header's Add classification: the removed chip's button is gone.
  const removal = useRemoveClassification(() => document.querySelector<HTMLElement>('[data-object-action="onAddTag"]'));
  const warehouseId = found?.warehouseId;
  const onRemoveTag =
    canEditTags && warehouseId != null && level === "object"
      ? (tag: TagDto) => {
          const a = (assignmentsOf(tags) ?? []).find((x) => x.column === "" && x.tag.id === tag.id);
          if (!a) return;
          removal.ask({
            tag,
            target: { warehouseId, schema: a.schema, table: a.table, label: dottedPath(path) },
          });
        }
      : undefined;

  const closeColumn = () =>
    setSearch((prev) => {
      const params = new URLSearchParams(prev);
      params.delete("column");
      return params;
    });
  // The header and its dialogs are about the object, never the column the panel shows.
  const objectPath = { catalog: path.catalog, schema: path.schema, object: path.object };

  return (
    <div className="space-y-4">
      <ObjectHeader
        path={path}
        kind={headerKind}
        engines={enginesOf(catalog, browse)}
        tags={headerTags(level, catalog, tags)}
        onGrant={onGrant}
        onAddTag={onAddTag}
        onWhoCanRead={onWhoCanRead}
        onRemoveTag={onRemoveTag}
        disabledReasons={found && found.warehouseId == null ? { onAddTag: TAGS_EGRET_ONLY } : undefined}
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
      <WhoCanReadDialog open={whoCanRead} onOpenChange={setWhoCanRead} path={objectPath} catalogId={catalogId} />
      {removal.dialog}
      <AddTagDialog
        target={addTag}
        onOpenChange={(open) => {
          if (!open) setAddTag(null);
        }}
      />
      {level === "object" ? (
        <ColumnPanel path={path} catalog={catalog} browse={browse} tags={tags} onClose={closeColumn} />
      ) : null}
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
