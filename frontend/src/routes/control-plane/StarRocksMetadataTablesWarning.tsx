import { useQuery } from "@tanstack/react-query";
import type { ApiError } from "@/api/client";
import { type Profile, listProfiles } from "@/api/policies";
import type { SecurityUpgradeReason } from "@/api/generated/model";
import { WarningCallout, profilesQueryKey } from "./externalEngine";

export const STARROCKS_METADATA_TABLES_SENTENCE =
  "Unless this engine runs egret-starrocks-fe 4.1.7-egret.4 or later, its StarRocks lets users past this profile's grants, masks, row filters and classifications. Official images serve the metadata tables of Iceberg tables ($files, $partitions, $manifests, $snapshots, $history, $refs, $metadata_log_entries) without asking Ranger and apply no mask or row filter to prepared statements (Data API endpoints with parameters); official images and egret's earlier patched images also apply none through the HTTP SQL API or Arrow Flight SQL, let anyone read object storage through FILES(), and let a masked or filtered user store a table's rows in the clear in a materialized view; images before 4.1.7-egret.4 also show a filtered user the statistics of the rows it hides through EXPLAIN COSTS, and let anyone reach object storage with the cluster's own identity through BROKER LOAD, EXPORT and SELECT INTO OUTFILE without keys. Run the engine on egret-starrocks-fe 4.1.7-egret.4 or later (the default for new engines), or give StarRocks logins only to people who may see every value and serve masked or filtered data through Trino.";

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
 * Whether the server's verdict says this provisioned StarRocks engine's FE image
 * is unpatched, or cannot say it. The rule lives in `egret-perm::engine_images`;
 * the engine DTO carries its result in `securityUpgradeReasons`. A payload
 * without the field (an older server, or no read of the engine) settles nothing.
 */
export function imageVerdict(
  reasons: readonly SecurityUpgradeReason[] | null | undefined,
): "patched" | "unpatched" | "unknown" {
  if (!reasons) return "unknown";
  return reasons.includes("unpatchedImage") ? "unpatched" : "patched";
}

/**
 * The warning on a provisioned StarRocks engine's page. The engine payload does
 * not carry its profile, so the binding is read from the profile list (as the
 * external engine page does); while that list loads or when it cannot be read,
 * nothing is shown.
 *
 * It reads the server's image verdict (`securityUpgradeReasons`):
 * - `unpatchedImage` present: hidden, because the security-upgrade warning
 *   (`StarRocksPostureWarnings.tsx`) names the image and offers the fix, so the
 *   page carries one message, not two;
 * - absent: the image is patched, so there is nothing to warn about;
 * - no verdict at all (a payload without the field): shown while bound, as before.
 */
export function ProvisionedStarRocksMetadataTablesWarning({
  engineId,
  securityUpgradeReasons,
}: {
  engineId: string;
  securityUpgradeReasons?: readonly SecurityUpgradeReason[] | null;
}) {
  const profilesQ = useQuery<Profile[], ApiError>({ queryKey: profilesQueryKey, queryFn: listProfiles });
  const bound = !!profilesQ.data?.some((p) => p.engines.some((e) => e.id === engineId));
  if (imageVerdict(securityUpgradeReasons) !== "unknown") return null;
  return <StarRocksMetadataTablesWarning engineType="starrocks" bound={bound} />;
}
