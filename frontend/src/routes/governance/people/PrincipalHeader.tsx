import { useId, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { ExternalLink, KeyRound } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import type { PageKind, Principal } from "@/routes/governance/people/principalData";
import { PEOPLE_BASE } from "@/routes/governance/people/peopleUrl";
import { BackLink } from "@/components/layout/DetailHeader";
import { PageTitle } from "@/components/layout/PageTitle";

const KIND_LABEL: Record<PageKind, string> = { user: "User", group: "Group", role: "Role" };

/** The Control Plane read each kind's page there needs. */
const MANAGE_PERM: Record<PageKind, string> = { user: "user:read", group: "group:read", role: "role:read" };

/**
 * Where Control Plane manages the principal. A user's page is by id; groups
 * and roles are listed by name there, but their pages are by id, which the
 * People reads do not carry, so the link opens the list.
 */
function manageUrl(kind: PageKind, key: string): string {
  if (kind === "user") return `/control-plane/users/${encodeURIComponent(key)}`;
  return kind === "group" ? "/control-plane/groups" : "/control-plane/roles";
}

export const NO_USERNAME_SENTENCE =
  "No username yet, so no engine knows this user. egret allocates one at their first query, notebook or engine token, or when a grant naming one of their groups or workspaces is compiled.";

const PUBLIC_SENTENCE = "Everyone. Engines resolve public themselves; it has no members to list.";

function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** `3 members (1 service account)`. */
function withServiceAccounts(base: string, serviceAccounts: number): string {
  return serviceAccounts > 0 ? `${base} (${count(serviceAccounts, "service account", "service accounts")})` : base;
}

function Facts({ principal }: { principal: Principal }): ReactNode {
  if (principal.kind === "user") {
    const u = principal.data;
    return (
      <>
        {u.email ? <li className="break-all">{u.email}</li> : null}
        <li>
          {u.username ? (
            <>
              <span className="text-muted-foreground">Username </span>
              <span className="break-all font-mono">{u.username}</span>
            </>
          ) : (
            <span className="text-muted-foreground">{NO_USERNAME_SENTENCE}</span>
          )}
        </li>
        <li>{u.status === "active" ? "Active" : u.status === "disabled" ? "Disabled" : u.status}</li>
        {u.serviceAccount ? (
          <li>
            <Badge variant="outline">Service account</Badge>
          </li>
        ) : null}
      </>
    );
  }
  if (principal.kind === "group") {
    const g = principal.data;
    if (g.isPublic) return <li className="text-muted-foreground">{PUBLIC_SENTENCE}</li>;
    return (
      <>
        <li>{g.source === "oidc" ? "OIDC: memberships change at login" : g.source === "local" ? "Local" : g.source}</li>
        <li className="tabular-nums">{withServiceAccounts(count(g.members.length, "member", "members"), g.serviceAccounts)}</li>
      </>
    );
  }
  const r = principal.data;
  return (
    <>
      <li>{`Exists in ${r.scopes.join(", ")}`}</li>
      <li className="tabular-nums">{withServiceAccounts(count(r.holders.length, "holder", "holders"), r.serviceAccounts)}</li>
    </>
  );
}

/**
 * A principal page's header (spec §2's table). It renders from the URL alone
 * while the read loads or fails, so the way back and Manage in Control Plane
 * work either way; the facts appear once the principal is read.
 */
export function PrincipalHeader({
  kind,
  keyName,
  principal,
  onGrant,
  grantBlocked,
}: {
  kind: PageKind;
  keyName: string;
  principal: Principal | null;
  /** Grant access (P14); offered with `policy:update` once the principal is read. */
  onGrant?: () => void;
  /** Why Grant access is disabled, said beside it (FE-ACTION-4). */
  grantBlocked?: string;
}) {
  const can = usePolicyPermissions();
  const blockedId = useId();
  const title = principal?.kind === "user" ? principal.data.name || principal.data.email || keyName : keyName;
  return (
    <header className="space-y-2">
      <div>
        <BackLink to={PEOPLE_BASE}>People & groups</BackLink>
      </div>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-xs uppercase text-muted-foreground">{KIND_LABEL[kind]}</p>
          <PageTitle tabIndex={-1} className="break-all outline-none">
            {title}
          </PageTitle>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {onGrant && can("policy:update") ? (
            <>
              <Button
                size="sm"
                onClick={onGrant}
                disabled={grantBlocked !== undefined}
                aria-describedby={grantBlocked ? blockedId : undefined}
              >
                <KeyRound aria-hidden />
                Grant access
              </Button>
              {grantBlocked ? (
                <span id={blockedId} className="text-xs text-muted-foreground">
                  {grantBlocked}
                </span>
              ) : null}
            </>
          ) : null}
          {can(MANAGE_PERM[kind]) ? (
            <Button variant="outline" size="sm" asChild>
              <Link to={manageUrl(kind, keyName)}>
                <ExternalLink aria-hidden />
                Manage in Control Plane
              </Link>
            </Button>
          ) : null}
        </div>
      </div>
      {principal ? (
        <ul role="list" className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
          <Facts principal={principal} />
        </ul>
      ) : null}
    </header>
  );
}
