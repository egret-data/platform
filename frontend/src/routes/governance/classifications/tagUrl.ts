/**
 * Where the Classification area's pages live (spec §2, CL17):
 * `/governance/classifications` (the index) and
 * `/governance/classifications/:id` (a tag's page), whose tab is `?tab=`
 * (`where-used`, the default, is the absence of the parameter).
 *
 * Statically importable, outside the lazy area's bundle guard (as
 * `engines/engineUrl.ts` is): the Engine & policy and People pages link into
 * the area through here.
 */
export const CLASSIFICATIONS_BASE = "/governance/classifications";

/** The tag page's tabs, in order (spec §5). */
export type TagTab = "where-used" | "clearance" | "rules" | "views" | "handwritten";

/** The URL of tag `id`'s page, on `tab` when given. */
export function tagUrl(id: string, tab?: TagTab): string {
  const path = `${CLASSIFICATIONS_BASE}/${encodeURIComponent(id)}`;
  return tab ? `${path}?tab=${tab}` : path;
}
