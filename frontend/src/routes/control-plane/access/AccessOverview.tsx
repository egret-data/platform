import type { ComponentType, ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { ArrowRight, Compass, ShieldCheck, UserCog, Users } from "lucide-react";

import { listGroups, listRoles, listUsers } from "@/api/access";
import { useControlPlaneAuditListAudit } from "@/api/generated/control-plane-audit/control-plane-audit";
import { useAuth } from "@/auth/useAuth";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { timeAgo } from "@/lib/relativeTime";
import { cn } from "@/lib/utils";

/**
 * egret's access model, top to bottom — the conceptual scaffolding that ties the
 * otherwise-flat menu items together: identity feeds RBAC, which combines with
 * per-object grants, and the data plane enforces classification rules in-engine.
 */
const ACCESS_LAYERS: {
  icon: ComponentType<{ className?: string }>;
  name: string;
  desc: string;
  to: string;
  cta: string;
}[] = [
  {
    icon: Users,
    name: "Identity",
    desc: "Who — users & groups, synced from OIDC.",
    to: "/control-plane/users",
    cta: "Users",
  },
  {
    icon: UserCog,
    name: "Roles & permissions",
    desc: "RBAC — which actions each role grants.",
    to: "/control-plane/roles",
    cta: "Roles",
  },
  {
    icon: Compass,
    name: "Object grants",
    desc: "Per-object access (Cedar) — who can open a dataset, chart or dashboard.",
    to: "/control-plane/access-explorer",
    cta: "Explore",
  },
  {
    icon: ShieldCheck,
    name: "Data plane",
    desc: "Column masking & row filters by tag (Ranger), enforced in the query engine.",
    to: "/control-plane/policies",
    cta: "Policies",
  },
];

/** A count is "unknown" while its query is loading or has failed. */
type Loadable<T> = { status: "loading" } | { status: "error" } | { status: "ok"; value: T };

function fromQuery<T, R>(
  q: { isLoading: boolean; isError: boolean; data: T | undefined },
  map: (data: T) => R,
): Loadable<R> {
  if (q.isLoading) return { status: "loading" };
  if (q.isError || q.data === undefined) return { status: "error" };
  return { status: "ok", value: map(q.data) };
}

const QUICK_LINKS = [
  { to: "/control-plane/users", label: "Users" },
  { to: "/control-plane/groups", label: "Groups" },
  { to: "/control-plane/roles", label: "Roles" },
  { to: "/control-plane/permissions", label: "Permissions" },
  { to: "/control-plane/access-explorer", label: "Access Explorer" },
  { to: "/control-plane/policies", label: "Policies" },
  { to: "/control-plane/classifications", label: "Classifications" },
  { to: "/control-plane/audit-log", label: "Audit log" },
];

/**
 * Control Plane → Access control → Overview.
 *
 * The landing page for the access-control section: at-a-glance identity/role
 * counts, posture warnings, a platform-admin-only recent-changes feed, and
 * quick links into the rest of the section. FE-only aggregation over
 * existing endpoints — no new backend surface. Each panel degrades on its
 * own; a failed sub-query never blanks the page.
 */
export default function AccessOverview() {
  const { me } = useAuth();
  const isPlatformAdmin = !!me?.platform_roles.includes("platform_admin");

  const usersQuery = useQuery({
    queryKey: ["access-overview", "users"],
    queryFn: listUsers,
    retry: false,
  });
  const groupsQuery = useQuery({
    queryKey: ["access-overview", "groups"],
    queryFn: listGroups,
    retry: false,
  });
  const rolesQuery = useQuery({
    queryKey: ["access-overview", "roles"],
    queryFn: listRoles,
    retry: false,
  });

  const users = fromQuery(usersQuery, (data) => ({
    total: data.length,
    active: data.filter((u) => u.status === "active").length,
  }));
  const groups = fromQuery(groupsQuery, (data) => ({ total: data.length }));
  const roles = fromQuery(rolesQuery, (data) => ({
    total: data.length,
    platform: data.filter((r) => r.scope === "platform").length,
    workspace: data.filter((r) => r.scope === "workspace").length,
    zeroAssignee: data.filter((r) => r.assigneeCount === 0),
  }));
  const platformAdmins = fromQuery(usersQuery, (data) => data.filter((u) => u.roles.includes("platform_admin")).length);

  const platformAdminCount = platformAdmins.status === "ok" ? platformAdmins.value : 0;
  const highlightAdmins = platformAdmins.status === "ok" && platformAdminCount >= 3;

  const warnings: { key: string; message: string; to: string }[] = [];
  if (roles.status === "ok") {
    for (const r of roles.value.zeroAssignee) {
      warnings.push({
        key: `role-${r.id}`,
        message: `Role ${r.name} has no assignees`,
        to: "/control-plane/roles",
      });
    }
  }
  if (highlightAdmins) {
    warnings.push({
      key: "platform-admins",
      message: `Review platform admins (${platformAdminCount})`,
      to: "/control-plane/users",
    });
  }

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Access &amp; governance</h1>
        <p className="text-sm text-muted-foreground">
          Who can access what, through which layer — plus the posture issues worth your
          attention.
        </p>
      </div>

      <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
        <KpiTile
          label="Users"
          value={renderCount(users, (v) => v.total)}
          sub={users.status === "ok" ? `${users.value.active} active` : undefined}
        />
        <KpiTile label="Groups" value={renderCount(groups, (v) => v.total)} />
        <KpiTile
          label="Roles"
          value={renderCount(roles, (v) => v.total)}
          sub={roles.status === "ok" ? `${roles.value.platform} platform · ${roles.value.workspace} workspace` : undefined}
        />
        <KpiTile
          label="Platform admins"
          value={renderCount(platformAdmins, (v) => v)}
          highlight={highlightAdmins}
        />
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">How access works</CardTitle>
        </CardHeader>
        <CardContent>
          <ol className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
            {ACCESS_LAYERS.map((layer, i) => (
              <li key={layer.name} className="relative rounded-lg border p-3">
                <div className="flex items-center gap-2">
                  <span className="grid h-8 w-8 shrink-0 place-items-center rounded-md bg-primary/10 text-primary">
                    <layer.icon className="h-4 w-4" />
                  </span>
                  <span className="text-sm font-medium leading-tight">{layer.name}</span>
                </div>
                <p className="mt-2 text-xs text-muted-foreground">{layer.desc}</p>
                <Link
                  to={layer.to}
                  className="mt-2 inline-flex items-center gap-1 text-xs font-medium text-primary underline-offset-2 hover:underline"
                >
                  {layer.cta}
                  <ArrowRight className="h-3 w-3" />
                </Link>
                {i < ACCESS_LAYERS.length - 1 && (
                  <ArrowRight
                    className="absolute -right-2.5 top-1/2 hidden h-4 w-4 -translate-y-1/2 text-border lg:block"
                    aria-hidden
                  />
                )}
              </li>
            ))}
          </ol>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Warnings</CardTitle>
        </CardHeader>
        <CardContent>
          {warnings.length === 0 ? (
            <p className="text-sm text-muted-foreground">No issues found.</p>
          ) : (
            <ul className="space-y-2">
              {warnings.map((w) => (
                <li key={w.key} className="text-sm">
                  <Link to={w.to} className="text-foreground underline-offset-2 hover:underline">
                    {w.message}
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      {isPlatformAdmin ? <RecentChangesPanel /> : null}

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Quick links</CardTitle>
        </CardHeader>
        <CardContent>
          <ul className="flex flex-wrap gap-3">
            {QUICK_LINKS.map((l) => (
              <li key={l.to}>
                <Link
                  to={l.to}
                  className="rounded-md border px-3 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
                >
                  {l.label}
                </Link>
              </li>
            ))}
          </ul>
        </CardContent>
      </Card>
    </div>
  );
}

/** Render a KPI value: "…" while loading, "—" on error, else the mapped value. */
function renderCount<T>(loadable: Loadable<T>, map: (v: T) => ReactNode): ReactNode {
  if (loadable.status === "loading") return "…";
  if (loadable.status === "error") return "—";
  return map(loadable.value);
}

function KpiTile({
  label,
  value,
  sub,
  highlight,
}: {
  label: string;
  value: ReactNode;
  sub?: ReactNode;
  highlight?: boolean;
}) {
  return (
    <Card className={highlight ? "border-highlight/50" : undefined}>
      <CardContent className="space-y-1 py-4">
        <p className="text-xs uppercase tracking-wide text-muted-foreground">{label}</p>
        <p className={cn("text-2xl font-semibold tabular-nums", highlight && "text-highlight")}>
          {value}
        </p>
        {sub ? <p className="text-xs text-muted-foreground">{sub}</p> : null}
      </CardContent>
    </Card>
  );
}

/**
 * Platform-admin-only recent-changes feed. Kept as a separate component so
 * the audit hook is only invoked when this panel is actually mounted —
 * non-admins never issue the request.
 */
function RecentChangesPanel() {
  const auditQuery = useControlPlaneAuditListAudit(
    { limit: 8 },
    { query: { retry: false } },
  );
  const page = auditQuery.data?.status === 200 ? auditQuery.data.data : undefined;

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">Recent changes</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        {auditQuery.isLoading ? (
          <p className="text-sm text-muted-foreground">…</p>
        ) : auditQuery.isError || !page ? (
          <p className="text-sm text-muted-foreground">—</p>
        ) : page.entries.length === 0 ? (
          <p className="text-sm text-muted-foreground">No recent changes.</p>
        ) : (
          <ul className="space-y-2">
            {page.entries.map((e) => (
              <li key={e.id} className="flex flex-wrap items-baseline gap-x-2 text-sm">
                <span className="font-medium">{e.action}</span>
                <span className="text-muted-foreground">{e.resourceType}</span>
                {e.actorUserId ? (
                  <span className="text-xs text-muted-foreground" title={e.actorUserId}>
                    by {e.actorUserId.slice(0, 8)}
                  </span>
                ) : null}
                <span className="ml-auto text-xs text-muted-foreground">{timeAgo(e.ts)}</span>
              </li>
            ))}
          </ul>
        )}
        <Link
          to="/control-plane/audit-log"
          className="inline-block text-xs text-muted-foreground underline-offset-2 hover:underline"
        >
          View audit log
        </Link>
      </CardContent>
    </Card>
  );
}
