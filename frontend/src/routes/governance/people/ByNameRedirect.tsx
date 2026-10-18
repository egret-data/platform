import { Link, Navigate, useParams, useSearchParams } from "react-router-dom";
import { usePermListPeople } from "@/api/generated/perm/perm";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { TabError } from "@/routes/governance/tabParts";
import { PEOPLE_BASE, principalParam, principalUrl } from "@/routes/governance/people/peopleUrl";

/**
 * `/governance/people/users/by-name/:username` (spec P1): grant principals
 * name users by username, so links from a grant come here. It looks the
 * username up (`GET /perm/people?kind=user&username=`, service accounts
 * included) and replaces itself with the user's page, keeping `?tab=`.
 *
 * Only a byte-exact match redirects (final review I2). The route compares
 * case-insensitively (`users.username` is citext), but engines compare names
 * byte for byte, so a stored ref `Bob` matches nobody even while `bob` exists;
 * opening bob's page would show a grant reaching someone it does not reach.
 * Such a near match is said, with links to Unmatched names and bob's page.
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
  const rows = found.data.data.items;
  const row = rows.find((r) => r.username === username);
  if (row) return <Navigate to={principalUrl({ kind: "user", userId: row.key }, search.get("tab") ?? undefined)} replace />;
  const near = rows.find((r) => !!r.username);
  return (
    <div className="space-y-4 p-4">
      <div className="space-y-1">
        <Link to={PEOPLE_BASE} className="text-sm text-muted-foreground hover:underline">
          People & groups
        </Link>
        <h1 className="break-all text-lg font-semibold">{username}</h1>
      </div>
      <Alert>
        <AlertDescription className="space-y-1">
          {near?.username ? (
            <>
              <p>
                {`“${username}” matches the user ${near.username} only case-insensitively; engines compare names byte for byte, so this name matches nobody.`}
              </p>
              <p className="flex flex-wrap gap-x-4">
                <Link to={`${PEOPLE_BASE}?kind=unmatched`} className="text-primary hover:underline">
                  Unmatched names
                </Link>
                <Link to={principalUrl({ kind: "user", userId: near.key })} className="text-primary hover:underline">
                  {`${near.username}'s page`}
                </Link>
              </p>
            </>
          ) : (
            <p>{`No user has the username ${username}.`}</p>
          )}
        </AlertDescription>
      </Alert>
    </div>
  );
}
