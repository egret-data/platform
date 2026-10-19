import { Navigate, useLocation, useParams } from "react-router-dom";
import { usePolicyAdminListProfiles } from "@/api/generated/policy-admin/policy-admin";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { TabError } from "@/routes/governance/tabParts";
import { landingEngine } from "@/routes/governance/engines/PolicyRedirect";
import { ENGINES_BASE, profileParam } from "@/routes/governance/engines/engineUrl";
import { encodeSegment } from "@/routes/governance/data/objectPath";
import {
  FrameHeader,
  FrameTabs,
  NotFound,
  OriginBadge,
  PROFILE_TABS,
  TypeBadge,
  type TabContext,
} from "@/routes/governance/engines/PageFrame";
import { EngineTabBody } from "@/routes/governance/engines/tabBodies";
import { useProfilePolicies } from "@/routes/governance/engines/profilePolicies";
import { useProfileOverlaps } from "@/routes/governance/engines/tabs/OverlapsTab";

export const NO_ENGINE_BOUND = "No engine is bound to this profile.";

/**
 * A profile's page (EP13, EP15): `/governance/engines/profiles/:service`. A
 * profile bound to an engine has no page of its own: this replaces the URL
 * with the engine's page (the first engine by name for a legacy shared
 * profile, Q3), keeping the query and hash. An unbound profile gets the engine
 * page's frame without Engine permissions and Plugin. The header renders from
 * the URL while the list loads or fails.
 */
export default function ProfilePage() {
  const name = profileParam(useParams().service);
  const { search, hash } = useLocation();
  const profilesQ = usePolicyAdminListProfiles();
  const dto =
    profilesQ.data?.status === 200 ? (profilesQ.data.data.find((p) => p.name === name) ?? null) : undefined;
  const tagService = dto?.tag_service ?? null;
  const policies = useProfilePolicies(dto && dto.engines.length === 0 ? name : null, tagService);
  const overlaps = useProfileOverlaps(dto && dto.engines.length === 0 ? name : null);

  if (dto && dto.engines.length > 0) {
    const engineId = landingEngine(dto);
    return <Navigate to={{ pathname: `${ENGINES_BASE}/${encodeSegment(engineId ?? "")}`, search, hash }} replace />;
  }

  let body;
  if (profilesQ.isError) {
    body = <TabError what="list of policy profiles" onRetry={() => void profilesQ.refetch()} />;
  } else if (dto === undefined) {
    body = <ListSkeleton rows={4} cols={3} />;
  } else if (dto === null) {
    body = <NotFound sentence={`No policy profile named ${name}.`} />;
  } else {
    const ctx: TabContext = {
      subject: { profile: name },
      engine: null,
      grants: null,
      reach: null,
      profile: { name, type: dto.type, dto, tagService },
      policies,
    };
    const tabs = PROFILE_TABS.filter((t) => t !== "migrate" || policies.hasHandWritten);
    body = (
      <FrameTabs
        tabs={tabs}
        badges={overlaps.pairs ? { overlaps: overlaps.pairs.length } : undefined}
        render={(tab) => <EngineTabBody tab={tab} ctx={ctx} />}
      />
    );
  }

  return (
    <div className="space-y-6 p-4">
      <FrameHeader
        label="Profile"
        title={name}
        facts={
          dto ? (
            <>
              <TypeBadge type={dto.type} />
              <OriginBadge origin={dto.origin} />
              <span>{NO_ENGINE_BOUND}</span>
            </>
          ) : null
        }
      />
      {body}
    </div>
  );
}
