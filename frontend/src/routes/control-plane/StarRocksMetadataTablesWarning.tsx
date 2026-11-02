import { useQuery } from "@tanstack/react-query";
import type { ApiError } from "@/api/client";
import { type Profile, listProfiles } from "@/api/policies";
import { WarningCallout, profilesQueryKey } from "./externalEngine";

export const STARROCKS_METADATA_TABLES_SENTENCE =
  "Official StarRocks images serve the metadata tables of Iceberg tables ($files, $partitions, $manifests, $snapshots, $history, $refs, $metadata_log_entries) without asking Ranger, and apply no mask or row filter to prepared statements (Data API endpoints with parameters, clients that prepare statements on the server). Anyone who can log in to this engine can read, for every table in an attached Iceberg catalog, its row counts, partition values, data file paths and each column's lowest and highest values, and through a prepared statement every row and column in the clear, whatever the profile's grants, masks, row filters and classifications say. Run the engine on egret's patched image egret-starrocks-fe (the default for new engines), or give StarRocks logins only to people who may see these values and serve masked or filtered data through Trino.";

/** egret's patched StarRocks FE image: `<N.N.N>-egret.<N>`, with or without its digest. */
const PATCHED_STARROCKS_FE = /^ghcr\.io\/egret-data\/egret-starrocks-fe:\d+\.\d+\.\d+-egret\.\d+(@sha256:[0-9a-f]{64})?$/;

/**
 * Whether `image` is egret's patched StarRocks FE, which closes both holes the warning
 * describes. Anything else, including an engine whose image egret does not know, is
 * treated as unpatched.
 */
export function isPatchedStarRocksImage(image: string | null | undefined): boolean {
  return !!image && PATCHED_STARROCKS_FE.test(image.trim());
}

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
 * nothing is shown. An engine on egret's patched image gets no warning.
 */
export function ProvisionedStarRocksMetadataTablesWarning({ engineId, image }: { engineId: string; image?: string }) {
  const profilesQ = useQuery<Profile[], ApiError>({ queryKey: profilesQueryKey, queryFn: listProfiles });
  const bound = !!profilesQ.data?.some((p) => p.engines.some((e) => e.id === engineId));
  if (isPatchedStarRocksImage(image)) return null;
  return <StarRocksMetadataTablesWarning engineType="starrocks" bound={bound} />;
}
