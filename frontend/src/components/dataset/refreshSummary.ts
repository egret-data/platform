import type { DatasetRefresh } from "@/api/datasets";

const names = (xs: string[]) => (xs.length <= 3 ? xs.join(", ") : `${xs.slice(0, 3).join(", ")} and ${xs.length - 3} more`);

/**
 * What a Refresh changed, for its toast (KI-102). The refresh used to answer "Refreshed" whatever
 * it did, while silently resetting every Time switch; it now keeps those, and says which columns
 * the source added or dropped.
 */
export function refreshSummary(r: Pick<DatasetRefresh, "added_columns" | "removed_columns">): string {
  const added = r.added_columns ?? [];
  const removed = r.removed_columns ?? [];
  if (added.length === 0 && removed.length === 0) return "Refreshed: no columns added or removed";
  const parts: string[] = [];
  if (added.length) parts.push(`${added.length} new ${added.length === 1 ? "column" : "columns"} (${names(added)})`);
  if (removed.length) parts.push(`${removed.length} ${removed.length === 1 ? "column" : "columns"} removed (${names(removed)})`);
  return `Refreshed: ${parts.join("; ")}`;
}
