import { useQuery } from "@tanstack/react-query";
import type { ApiError } from "@/api/client";
import { type Profile, listProfiles } from "@/api/policies";
import { WarningCallout, profilesQueryKey } from "./externalEngine";

export const STARROCKS_METADATA_TABLES_SENTENCE =
  "Unless this engine runs egret-starrocks-fe 4.1.7-egret.3 or later, its StarRocks lets users past this profile's grants, masks, row filters and classifications. Official images serve the metadata tables of Iceberg tables ($files, $partitions, $manifests, $snapshots, $history, $refs, $metadata_log_entries) without asking Ranger and apply no mask or row filter to prepared statements (Data API endpoints with parameters); official images and egret's earlier patched images also apply none through the HTTP SQL API or Arrow Flight SQL, let anyone read object storage through FILES(), and let a masked or filtered user store a table's rows in the clear in a materialized view. Run the engine on egret-starrocks-fe 4.1.7-egret.3 or later (the default for new engines), or give StarRocks logins only to people who may see every value and serve masked or filtered data through Trino.";

/** egret's patched StarRocks FE image: `<N.N.N>-egret.<N>`, with or without its digest. */
const PATCHED_STARROCKS_FE =
  /^ghcr\.io\/egret-data\/egret-starrocks-fe:(\d+)\.(\d+)\.(\d+)-egret\.(\d+)(?:@sha256:[0-9a-f]{64})?$/;

/**
 * The first patch level of each upstream release that closes every hole the warning names
 * (egret-starrocks: 0001 to 0007 on 4.1.7). A later upstream release starts at level 1 with
 * every patch rebased onto it, so any level of it counts.
 */
const FIRST_COMPLETE = { release: [4, 1, 7], level: 3 } as const;

/**
 * Whether `image` is an egret-patched StarRocks FE that closes every hole the warning
 * describes: 4.1.7 from egret.3, or any later release. Anything else, including an earlier
 * patch level and an engine whose image egret does not know, is treated as unpatched.
 */
export function isPatchedStarRocksImage(image: string | null | undefined): boolean {
  const m = image ? PATCHED_STARROCKS_FE.exec(image.trim()) : null;
  if (!m) return false;
  const release = [Number(m[1]), Number(m[2]), Number(m[3])];
  for (let i = 0; i < 3; i++) {
    if (release[i] !== FIRST_COMPLETE.release[i]) return release[i] > FIRST_COMPLETE.release[i];
  }
  return Number(m[4]) >= FIRST_COMPLETE.level;
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
 * nothing is shown. An engine on a complete egret-patched image gets no warning.
 */
export function ProvisionedStarRocksMetadataTablesWarning({ engineId, image }: { engineId: string; image?: string }) {
  const profilesQ = useQuery<Profile[], ApiError>({ queryKey: profilesQueryKey, queryFn: listProfiles });
  const bound = !!profilesQ.data?.some((p) => p.engines.some((e) => e.id === engineId));
  if (isPatchedStarRocksImage(image)) return null;
  return <StarRocksMetadataTablesWarning engineType="starrocks" bound={bound} />;
}
