import { Link, Navigate, useParams, useSearchParams } from "react-router-dom";
import { usePermListPeople } from "@/api/generated/perm/perm";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { TabError } from "@/routes/governance/tabParts";
import { PEOPLE_BASE, principalParam, principalUrl } from "@/routes/governance/people/peopleUrl";

/**
 * `/governance/people/users/by-name/:username` (spec P1): grant principals
 * name users by username, so links from a grant come here. It looks the
 * username up (`GET /perm/people?kind=user&username=`, exact, service accounts
 * included) and replaces itself with the user's page, keeping `?tab=`.
 */
export default function ByNameRedirect() {
  const username = principalParam(useParams().username);
  const [search] = useSearchParams();
  const found = usePermListPeople({ kind: "user", username, limit: 1 });

  if (found.isError) {
    return (
      <div className="p-4">
        <TabError what="user" onRetry={() => void found.refetch()} />
      </div>
    );
  }
  if (found.data?.status !== 200) {
    return (
      <div className="p-4">
        <ListSkeleton rows={2} cols={1} />
      </div>
    );
  }
  const row = found.data.data.items[0];
  if (row) return <Navigate to={principalUrl({ kind: "user", userId: row.key }, search.get("tab") ?? undefined)} replace />;
  return (
    <div className="space-y-4 p-4">
      <div className="space-y-1">
        <Link to={PEOPLE_BASE} className="text-sm text-muted-foreground hover:underline">
          People & groups
        </Link>
        <h1 className="break-all text-lg font-semibold">{username}</h1>
      </div>
      <Alert>
        <AlertDescription>{`No user has the username ${username}.`}</AlertDescription>
      </Alert>
    </div>
  );
}
