/**
 * System-managed secrets (KI-207): the ones egret mints for an engine or a
 * catalog (`engine-internal-*`, `engine-monitor-*`, `engine-tls-*`,
 * `catalog-client-*`, the StarRocks root and Trino delegator passwords). The
 * server sets `system_managed` when it creates them; it refuses to rename,
 * rotate or delete them, and no picker offers them.
 */
interface MaybeSystem {
  system_managed?: boolean;
}

/** The secrets a person may pick for a connection, a catalog or an engine. */
export function pickableSecrets<T extends MaybeSystem>(secrets: readonly T[]): T[] {
  return secrets.filter((s) => s.system_managed !== true);
}

/** Why a system secret has no rename, rotate or delete. */
export const SYSTEM_SECRET_REASON =
  "Managed by egret: it is rotated and deleted with the engine or catalog that created it.";
