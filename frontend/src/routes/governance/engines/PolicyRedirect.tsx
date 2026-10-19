import { Navigate, useLocation, useParams } from "react-router-dom";
import { usePolicyAdminListProfiles } from "@/api/generated/policy-admin/policy-admin";
import type { ProfileDto } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { TabError } from "@/routes/governance/tabParts";
import { editorUrl, ENGINES_BASE, engineUrl, profileUrl } from "@/routes/governance/engines/engineUrl";

/**
 * The URLs of the pages Engine & policy replaced (spec §3, EP15, Q3):
 * `/governance/policies/:service`, `…/migrate`, `…/:kind/new` and
 * `…/:kind/:id`. It reads the profile list, finds the profile by name and
 * replaces itself:
 * - bound to an engine → that engine's page (`/governance/engines/<id>/…`);
 *   bound to several (a legacy shared profile) → the first engine by name
 *   (Q3), whose page names the others in its banner;
 * - unbound → the profile page (`/governance/engines/profiles/<name>/…`);
 * - unknown → the index, which says "No policy profile named X." (location
 *   state `missingProfile`, read by `MissingProfileNotice`).
 *
 * The old profile page's type tab (`?tab=access|masking|rowfilter|tags`)
 * becomes the Policies tab's type filter (`&type=`, `tags` → `tag`). Every
 * other query key and the hash are kept (the editor's `?scope=tag` picks
 * By-tag mode). Always `replace`, so Back does not return to the dead URL.
 *
 * Statically imported by `App.tsx`: it imports no page of the lazy area.
 */
export type PolicyRedirectTarget = "profile" | "migrate" | "editor";

/** The location state key the index reads to say which profile name matched nothing. */
export const MISSING_PROFILE_STATE = "missingProfile";

/** The old type tab's value as the Policies tab's `type` filter names it. */
const TYPE_OF_OLD_TAB: Record<string, string> = {
  access: "access",
  masking: "masking",
  rowfilter: "rowfilter",
  tags: "tag",
};

/** The engine an old URL naming `profile` lands on: the first bound engine by name (Q3), or none. */
export function landingEngine(profile: Pick<ProfileDto, "engines">): string | null {
  const byName = [...profile.engines].sort((a, b) =>
    a.name < b.name ? -1 : a.name > b.name ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0,
  );
  return byName[0]?.id ?? null;
}

export default function PolicyRedirect({ to }: { to: PolicyRedirectTarget }) {
  // The router hands the old `:service` param back decoded; the old pages read it raw.
  const { service = "", kind = "", id } = useParams();
  const { search, hash } = useLocation();
  const profiles = usePolicyAdminListProfiles();

  if (profiles.isError) {
    return (
      <div className="p-4">
        <TabError what="list of policy profiles" onRetry={() => void profiles.refetch()} />
      </div>
    );
  }
  if (profiles.data?.status !== 200) {
    return (
      <div className="p-4">
        <ListSkeleton rows={3} cols={2} />
      </div>
    );
  }

  const found = profiles.data.data.find((p) => p.name === service);
  if (!found) {
    return <Navigate to={ENGINES_BASE} replace state={{ [MISSING_PROFILE_STATE]: service }} />;
  }
  const engineId = landingEngine(found);

  const params = new URLSearchParams(search);
  let pathname: string;
  if (to === "editor") {
    pathname = editorUrl(engineId ? { engineId } : { profile: found.name }, kind, id);
  } else {
    const oldTab = params.get("tab");
    params.delete("tab");
    params.delete("type");
    const type = to === "profile" && oldTab ? TYPE_OF_OLD_TAB[oldTab] : undefined;
    const tab = to === "migrate" ? "migrate" : "policies";
    const extra: Record<string, string | undefined> = { type };
    for (const [k, v] of params) extra[k] = v;
    const url = engineId ? engineUrl(engineId, tab, extra) : profileUrl(found.name, tab, extra);
    return <Navigate to={`${url}${hash}`} replace />;
  }
  return <Navigate to={{ pathname, search, hash }} replace />;
}
