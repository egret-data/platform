import type { ReactNode } from "react";
import { Link, useParams } from "react-router-dom";
import { ExternalLink } from "lucide-react";
import type { ApiError } from "@/api/client";
import { useControlPlaneGovernanceEnginePlugin } from "@/api/generated/control-plane-governance/control-plane-governance";
import { usePermListEngineGrants } from "@/api/generated/perm/perm";
import { usePolicyAdminListProfiles } from "@/api/generated/policy-admin/policy-admin";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { TabError } from "@/routes/governance/tabParts";
import {
  Banner,
  ENGINE_TABS,
  EXTERNAL_SENTENCE,
  Fact,
  FrameHeader,
  FrameTabs,
  NotFound,
  OriginBadge,
  sharedSentence,
  TypeBadge,
  type FrameProfile,
  type TabContext,
} from "@/routes/governance/engines/PageFrame";
import { EngineTabBody } from "@/routes/governance/engines/tabBodies";
import { useProfilePolicies } from "@/routes/governance/engines/profilePolicies";

export const NEEDS_ENGINE_READ = "Plugin state needs the engine:read permission.";

/**
 * An engine's page (spec §6): `/governance/engines/:id`, `policy:read`.
 * Engine, profile and reach come from `GET /perm/engines/{id}/grants` (spec
 * premise 9); the profile's type, origin and tag service from the profile
 * list; the plugin version from the plugin route, only with `engine:read`
 * (EP19). Each read fails on its own: a failing grants read shows its error
 * in the tabs that need it and keeps the tab strip (FE-STATE-1).
 */
export default function EnginePage() {
  const { id = "" } = useParams();
  const can = usePolicyPermissions();
  const engineRead = can("engine:read");
  const grantsQ = usePermListEngineGrants(id);
  const profilesQ = usePolicyAdminListProfiles();
  const pluginQ = useControlPlaneGovernanceEnginePlugin(id, { query: { enabled: engineRead } });

  const grants = grantsQ.data?.status === 200 ? grantsQ.data.data : null;
  const engine = grants?.engine ?? null;
  const profileName = grants?.profile?.name ?? null;
  const dto =
    profileName && profilesQ.data?.status === 200
      ? (profilesQ.data.data.find((p) => p.name === profileName) ?? null)
      : null;
  const tagService = dto?.tag_service ?? null;
  const policies = useProfilePolicies(profileName, tagService);

  if (grantsQ.isError && (grantsQ.error as unknown as ApiError | null)?.status === 404) {
    return (
      <div className="space-y-6 p-4">
        <FrameHeader label="Engine" title="Engine" facts={<span className="break-all font-mono">{id}</span>} />
        <NotFound sentence="No engine with this id." />
      </div>
    );
  }

  const profile: FrameProfile | null =
    engine && profileName ? { name: profileName, type: engine.type, dto, tagService } : null;
  const ctx: TabContext = {
    subject: { engineId: id },
    engine,
    grants,
    reach: grants?.reach ?? null,
    profile,
    policies,
  };

  let pluginFact: ReactNode;
  if (!engineRead) pluginFact = <span>{NEEDS_ENGINE_READ}</span>;
  else if (pluginQ.isError) pluginFact = <span>The plugin state could not be loaded.</span>;
  else if (pluginQ.data?.status === 200) {
    const p = pluginQ.data.data.plugin;
    pluginFact = (
      <>
        <span>{p.version ? `Ranger plugin ${p.version}` : "Ranger plugin version unknown"}</span>
        {p.modelled ? null : <span>{p.note ? `Not modelled: ${p.note}` : "Not modelled."}</span>}
      </>
    );
  } else pluginFact = <Skeleton className="h-4 w-28" />;

  const facts = engine ? (
    <>
      <TypeBadge type={engine.type} />
      <OriginBadge origin={engine.origin} />
      <Fact label="Profile">{profileName ?? "No profile"}</Fact>
      {pluginFact}
    </>
  ) : (
    // Until the engine is read (or when it cannot be), the page names it by the URL's id.
    <span className="break-all font-mono">{id}</span>
  );

  const others = grants
    ? grants.reach.engines
        .filter((e) => e.id !== id)
        .map((e) => e.name)
        .sort((a, b) => a.localeCompare(b))
    : [];

  const tabs = ENGINE_TABS.filter((t) => t !== "migrate" || policies.hasHandWritten);

  const body = (tab: (typeof ENGINE_TABS)[number]): ReactNode => {
    if (tab !== "plugin") {
      if (grantsQ.isError) return <TabError what="engine" onRetry={() => void grantsQ.refetch()} />;
      if (!grants) return <ListSkeleton rows={4} cols={3} />;
    }
    return <EngineTabBody tab={tab} ctx={ctx} />;
  };

  return (
    <div className="space-y-6 p-4">
      <FrameHeader
        label="Engine"
        title={engine?.name ?? "Engine"}
        facts={facts}
        actions={
          engineRead ? (
            <Button size="sm" variant="outline" asChild>
              <Link to={`/control-plane/engines/${encodeURIComponent(id)}`}>
                <ExternalLink className="mr-1 h-4 w-4" aria-hidden />
                Open in Control plane
              </Link>
            </Button>
          ) : null
        }
      />
      {grants?.reach.sharedProfile && profileName && others.length > 0 ? (
        <Banner>{sharedSentence(profileName, others)}</Banner>
      ) : null}
      {engine?.origin === "external" ? <Banner>{EXTERNAL_SENTENCE}</Banner> : null}
      <FrameTabs tabs={tabs} render={body} />
    </div>
  );
}
