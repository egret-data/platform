import { Link, useParams } from "react-router-dom";
import type { ApiError } from "@/api/client";
import { usePermListEngineGrants } from "@/api/generated/perm/perm";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { TabError } from "@/routes/governance/tabParts";
import { NotFound } from "@/routes/governance/engines/PageFrame";
import PolicyEditorPage from "@/routes/governance/engines/PolicyEditorPage";
import { engineUrl, profileParam } from "@/routes/governance/engines/engineUrl";

export const NO_PROFILE_TO_EDIT = "This engine downloads no policy profile, so it has no policy to edit.";

/**
 * The editor under an engine's page (spec §2, Q2):
 * `/governance/engines/:id/advanced/policies/:kind/new|:policyId`. The engine
 * names its profile only through `GET /perm/engines/{id}/grants` (spec premise
 * 9), so the editor mounts once that answered: it builds its blank form from
 * the profile on mount.
 */
export function EngineEditorRoute() {
  const { id = "", kind = "", policyId } = useParams();
  const q = usePermListEngineGrants(id);
  const data = q.data?.status === 200 ? q.data.data : null;

  let body;
  if (q.isError) {
    body =
      (q.error as unknown as ApiError | null)?.status === 404 ? (
        <NotFound sentence="No engine with this id." />
      ) : (
        <TabError what="engine" onRetry={() => void q.refetch()} />
      );
  } else if (!data) {
    body = <ListSkeleton rows={6} cols={2} />;
  } else if (!data.profile) {
    body = (
      <Alert>
        <AlertDescription>
          <span>{NO_PROFILE_TO_EDIT} </span>
          <Link to={engineUrl(id)} className="text-primary underline focus-visible:ring-2 focus-visible:ring-ring">
            Back to {data.engine.name}
          </Link>
        </AlertDescription>
      </Alert>
    );
  } else {
    const service = data.profile.name;
    body = (
      <PolicyEditorPage key={`${service}/${kind}/${policyId ?? "new"}`} service={service} subject={{ engineId: id }} />
    );
  }
  return <div className="p-4">{body}</div>;
}

/** The editor under an unbound profile's page: `/governance/engines/profiles/:service/advanced/policies/…`. */
export function ProfileEditorRoute() {
  const { service, kind = "", policyId } = useParams();
  const name = profileParam(service);
  return (
    <div className="p-4">
      <PolicyEditorPage key={`${name}/${kind}/${policyId ?? "new"}`} service={name} subject={{ profile: name }} />
    </div>
  );
}
