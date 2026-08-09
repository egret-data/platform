/** A value the URL cannot hold. `useDashboardParams.setValue` deletes exactly these, so all three
 *  spellings are one state — Metabase's `isParameterValueEmpty` (FilterApplyToast/utils.ts) omits
 *  `""` because its parameter values live in Redux, not the query string. */
const isEmpty = (v: unknown) =>
  v === undefined || v === null || v === "" || (Array.isArray(v) && v.length === 0);

/** How many filters differ between what the cards are showing and what the user has drafted.
 *  Ported from Metabase's `getFilterChangeDescription` counting loop. Values are compared by
 *  serialisation rather than by reference: a drafted array is a new array every keystroke. */
export function countChangedFilters(
  applied: Record<string, unknown>,
  draft: Record<string, unknown>,
): number {
  let changed = 0;
  for (const slug of new Set([...Object.keys(applied), ...Object.keys(draft)])) {
    const a = applied[slug];
    const d = draft[slug];
    const ae = isEmpty(a);
    const de = isEmpty(d);
    if (ae && de) continue;
    if (ae !== de) { changed++; continue; }
    if (JSON.stringify(a) !== JSON.stringify(d)) changed++;
  }
  return changed;
}

/** Metabase's `ngettext(msgid`${n} filter changed`, `${n} filters changed`, n)`. */
export function describeFilterChanges(count: number): string {
  return count === 1 ? "1 filter changed" : `${count} filters changed`;
}
