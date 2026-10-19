import { decodeSegment, encodeSegment } from "@/routes/governance/data/objectPath";

/**
 * Where the Engine & policy area's pages live (spec §2):
 * `/governance/engines` (index), `/governance/engines/:id` (an engine),
 * `/governance/engines/profiles/:service` (a profile, which redirects to its
 * engine when bound), and the policy editor under either, at
 * `…/advanced/policies/:kind/new` or `…/:kind/:policyId` (Q2: the kind stays
 * in the URL, because the editor builds its form from it before the policy
 * loads). `?tab=` selects the page's tab.
 *
 * Statically importable, outside the lazy area's bundle guard (as
 * `people/peopleUrl.ts` is): `PolicyRedirect`, the Overview and Control Plane
 * link into the area through here.
 *
 * A profile name is one path segment, encoded as the Data area encodes its
 * names (`objectPath.ts`): a `/`, `%` or space stays in its segment and a
 * dot-only name survives the router. Read it back with `profileParam`.
 */
export const ENGINES_BASE = "/governance/engines";

/** The tabs an engine page has (spec §6); a profile page has all but `engine-permissions` and `plugin`. */
export type EngineTab = "engine-permissions" | "policies" | "overlaps" | "migrate" | "plugin" | "advanced";

/** The editor's policy kinds, as the old editor routes and `PolicyEditorPage` name them. */
export type PolicyKind = "access" | "masking" | "rowfilter";

/** Which page an editor URL hangs under: a bound engine's, or an unbound profile's. */
export type EditorSubject = { engineId: string } | { profile: string };

function withQuery(path: string, query?: Record<string, string | undefined>): string {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(query ?? {})) if (v !== undefined) params.set(k, v);
  const q = params.toString();
  return q ? `${path}?${q}` : path;
}

/** The URL of engine `id`'s page, on `tab` when given, with extra query keys (`grant`, `type`, …). */
export function engineUrl(id: string, tab?: EngineTab, extra?: Record<string, string | undefined>): string {
  return withQuery(`${ENGINES_BASE}/${encodeSegment(id)}`, { tab, ...extra });
}

/** The URL of profile `name`'s page, on `tab` when given. */
export function profileUrl(name: string, tab?: EngineTab, extra?: Record<string, string | undefined>): string {
  return withQuery(`${ENGINES_BASE}/profiles/${encodeSegment(name)}`, { tab, ...extra });
}

/** The policy editor for `subject`: a new policy of `kind`, or policy `policyId`. */
export function editorUrl(subject: EditorSubject, kind: string, policyId?: string | number): string {
  const base =
    "engineId" in subject
      ? `${ENGINES_BASE}/${encodeSegment(subject.engineId)}`
      : `${ENGINES_BASE}/profiles/${encodeSegment(subject.profile)}`;
  const leaf = policyId === undefined ? "new" : encodeSegment(String(policyId));
  return `${base}/advanced/policies/${encodeSegment(kind)}/${leaf}`;
}

/** A profile route param as the router hands it back, with a dot-only name restored. */
export function profileParam(param: string | undefined): string {
  return decodeSegment(param ?? "");
}
