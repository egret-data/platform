import { useQuery } from "@tanstack/react-query";
import type { ApiError } from "@/api/client";
import { type Profile, listProfiles } from "@/api/policies";
import { WarningCallout, profilesQueryKey } from "./externalEngine";

export const STARROCKS_METADATA_TABLES_SENTENCE =
  "StarRocks serves the metadata tables of Iceberg tables ($files, $partitions, $manifests, $snapshots, $history, $refs, $metadata_log_entries) without asking Ranger. Anyone who can log in to this engine can read, for every table in an attached Iceberg catalog, its row counts, partition values, data file paths and each column's lowest and highest values, whatever the profile's grants, masks, row filters and classifications say. egret cannot block this. Give StarRocks logins only to people who may see these values, and serve masked or filtered data through Trino.";

/**
 * The warning shown wherever a StarRocks engine is, or is about to be, bound to
 * a policy profile (spec 2026-11-02-starrocks-metadata-table-warning): StarRocks
 * never asks Ranger about an Iceberg metadata table, so no policy egret compiles
 * reaches them. Every bound profile is affected (a grant restricts everyone it
 * does not name), so the only condition is the engine type and a profile.
 */
export function StarRocksMetadataTablesWarning({ engineType, bound }: { engineType: string; bound: boolean }) {
  if (engineType !== "starrocks" || !bound) return null;
  return <WarningCallout testId="starrocks-metadata-tables">{STARROCKS_METADATA_TABLES_SENTENCE}</WarningCallout>;
}

/**
 * The warning on a provisioned StarRocks engine's page. The engine payload does
 * not carry its profile, so the binding is read from the profile list (as the
 * external engine page does); while that list loads or when it cannot be read,
 * nothing is shown.
 */
export function ProvisionedStarRocksMetadataTablesWarning({ engineId }: { engineId: string }) {
  const profilesQ = useQuery<Profile[], ApiError>({ queryKey: profilesQueryKey, queryFn: listProfiles });
  const bound = !!profilesQ.data?.some((p) => p.engines.some((e) => e.id === engineId));
  return <StarRocksMetadataTablesWarning engineType="starrocks" bound={bound} />;
}
