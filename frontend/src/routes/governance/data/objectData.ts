import type { UseQueryResult } from "@tanstack/react-query";
import type {
  ControlPlaneGovernanceBrowseQueryError,
  ControlPlaneGovernanceBrowseQueryResult,
  ControlPlaneGovernanceObjectTagsQueryError,
  ControlPlaneGovernanceObjectTagsQueryResult,
} from "@/api/generated/control-plane-governance/control-plane-governance";
import type { BrowseEngine, GovernanceCatalog, ResourceTagDto } from "@/api/generated/model";

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
