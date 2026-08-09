/** One line of Metabase's info-sidebar attribution: "August 1, 2026 by Ref User".
 *
 *  Returns null when there is no date. `created_at` is NULL for every dashboard predating
 *  migration 0106, which deliberately does not invent one; a row rendered as "— by Ref User"
 *  would read as a fact about the dashboard rather than a gap in what was recorded.
 *
 *  The name is dropped rather than defaulted when absent: `created_by` can be NULL because the
 *  user was hard-deleted (0093 sets the FK NULL to preserve the content), and "by Unknown" is a
 *  worse answer than the date on its own. */
export function attributionLine(iso: string | null, name: string | null): string | null {
  if (!iso) return null;
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  const date = d.toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
  return name ? `${date} by ${name}` : date;
}

/** One timeline sentence, in Metabase's wording. `kind` is the RAW value from the API
 *  (`baseline` | `edit` | `revert`, migration 0107), never a rendered label. */
export function revisionSummary(kind: string, name: string | null): string {
  const who = name ?? "Someone";
  switch (kind) {
    case "baseline":
      return `${who} created this`;
    case "edit":
      return `${who} edited this`;
    case "revert":
      return `${who} reverted to an earlier version`;
    default:
      return `${who} changed this`;
  }
}
