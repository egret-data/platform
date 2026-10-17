import type { UseQueryResult } from "@tanstack/react-query";
import type {
  ControlPlaneGovernanceBrowseQueryError,
  ControlPlaneGovernanceBrowseQueryResult,
  ControlPlaneGovernanceObjectTagsQueryError,
  ControlPlaneGovernanceObjectTagsQueryResult,
} from "@/api/generated/control-plane-governance/control-plane-governance";
import type { BrowseEngine, GovernanceCatalog, ResourceTagDto, Scope } from "@/api/generated/model";
import type { ObjectPath } from "@/routes/governance/data/objectPath";

/**
 * What an object's page reads, shared by its header and its tabs.
 *
 * Every read is its own query with its own state, so one that fails leaves the
 * others standing (spec §2.5): the header always has the name and path the
 * URL gives it, whatever the API answers.
 */

/** The catalog the URL names, as the Governance catalog list knows it. */
export type CatalogState =
  | { status: "loading" }
  | { status: "error"; retry: () => void }
  | { status: "missing"; name: string }
  | { status: "ok"; catalog: GovernanceCatalog };

export type BrowseResult = UseQueryResult<ControlPlaneGovernanceBrowseQueryResult, ControlPlaneGovernanceBrowseQueryError>;
export type TagsResult = UseQueryResult<ControlPlaneGovernanceObjectTagsQueryResult, ControlPlaneGovernanceObjectTagsQueryError>;

/** An engine the catalog is attached to: from browse when it answered, else only its name. */
export type EngineInfo = Pick<BrowseEngine, "name"> & Partial<Omit<BrowseEngine, "name">>;

/** The object kinds a schema lists. */
export type ObjectKind = "table" | "view" | "materialized_view";

const ENGINE_TYPE_LABELS: Record<string, string> = {
  trino: "Trino",
  starrocks: "StarRocks",
  risingwave: "RisingWave",
};

export function engineTypeLabel(type: string): string {
  return ENGINE_TYPE_LABELS[type] ?? type;
}

/** `running` → `Running`. */
export function capitalise(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1);
}

/**
 * The engines to show: browse's answer for this level when it has one (it
 * carries type, origin and status), otherwise the names the catalog list
 * holds, so a failed browse never empties the header. Sorted by name.
 */
export function enginesOf(catalog: CatalogState, browse: BrowseResult): EngineInfo[] {
  const fromBrowse = browse.data?.status === 200 ? browse.data.data.engines : undefined;
  const engines: EngineInfo[] =
    fromBrowse ?? (catalog.status === "ok" ? catalog.catalog.engines.map((name) => ({ name })) : []);
  return [...engines].sort((a, b) => a.name.localeCompare(b.name));
}

/** The tag assignments of a tags answer, or `null` when the catalog cannot carry tags. */
export function assignmentsOf(tags: TagsResult): ResourceTagDto[] | null | undefined {
  if (tags.data?.status !== 200) return undefined;
  return tags.data.data.supported ? (tags.data.data.assignments ?? []) : null;
}

/**
 * The tags a rule can find on an object, as the page knows them: `none` when
 * nothing there can carry a tag (a catalog or a schema, or an object in a
 * catalog that is not egret's, whose tags the page never asks for, F9), else
 * the tag read's own state.
 */
export type TagReach =
  | { status: "none" }
  | { status: "loading" }
  | { status: "error"; retry: () => void }
  | { status: "ok"; assignments: ResourceTagDto[] };

export function tagReach(path: ObjectPath, catalog: GovernanceCatalog, tags: TagsResult): TagReach {
  if (path.schema === undefined || path.object === undefined || catalog.warehouseId == null) return { status: "none" };
  if (tags.isError) return { status: "error", retry: () => void tags.refetch() };
  const assignments = assignmentsOf(tags);
  if (assignments === null) return { status: "none" };
  if (assignments === undefined) return { status: "loading" };
  return { status: "ok", assignments };
}

/** The grant or rule scope that names exactly this object (never its column). */
export function scopeOf(path: ObjectPath, catalogId: string): Scope {
  const scope: Scope = { catalog_id: catalogId };
  if (path.schema !== undefined) {
    scope.schema = path.schema;
    if (path.object !== undefined) scope.table = path.object;
  }
  return scope;
}
