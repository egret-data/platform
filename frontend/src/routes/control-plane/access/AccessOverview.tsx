import type { ComponentType, ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  AlertTriangle,
  ArrowRight,
  CheckCircle2,
  ChevronDown,
  Compass,
  ShieldCheck,
  UserCog,
  Users,
} from "lucide-react";

import { listGroups, listRoles, listUsers } from "@/api/access";
import { useControlPlaneAuditListAudit } from "@/api/generated/control-plane-audit/control-plane-audit";
import { useAuth } from "@/auth/useAuth";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
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
    to: "/governance/engines",
    cta: "Engine & policy",
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
  const groups = fromQuery(groupsQuery, (data) => ({
    total: data.length,
    synced: data.filter((g) => g.source === "oidc").length,
    local: data.filter((g) => g.source !== "oidc").length,
  }));
  const roles = fromQuery(rolesQuery, (data) => ({
    total: data.length,
    platform: data.filter((r) => r.scope === "platform").length,
    workspace: data.filter((r) => r.scope === "workspace").length,
    zeroAssignee: data.filter((r) => r.assigneeCount === 0),
  }));
  const platformAdmins = fromQuery(usersQuery, (data) => data.filter((u) => u.roles.includes("platform_admin")).length);

  const platformAdminCount = platformAdmins.status === "ok" ? platformAdmins.value : 0;
  // Share of accounts that are platform admins. Highlight the tile as a posture
  // concern only when that share is genuinely high (> 10%) — a fixed ">= 3"
  // threshold fired on every real deployment and so meant nothing.
  const userTotal = users.status === "ok" ? users.value.total : 0;
  const adminPct = userTotal > 0 ? Math.round((platformAdminCount / userTotal) * 100) : 0;
  const highlightAdmins =
    platformAdmins.status === "ok" && users.status === "ok" && adminPct > 10;

  const warnings: { key: string; title: string; detail: string; to: string }[] = [];
  if (roles.status === "ok" && roles.value.zeroAssignee.length > 0) {
    // Aggregate rather than list every role — hundreds of unassigned roles would
    // otherwise bury the page in UUID-named lines. One row, linking to Roles.
    const n = roles.value.zeroAssignee.length;
    warnings.push({
      key: "zero-assignee-roles",
      title: `${n} role${n === 1 ? "" : "s"} ${n === 1 ? "has" : "have"} no assignees`,
      detail: "They grant nothing until assigned to a user or group.",
      to: "/control-plane/roles",
    });
  }
  if (highlightAdmins) {
    warnings.push({
      key: "platform-admins",
      title: `${platformAdminCount} platform admins · ${adminPct}% of users`,
      detail: "Review who holds full-platform access and trim if it's too broad.",
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
          to="/control-plane/users"
          value={renderCount(users, (v) => v.total)}
          sub={users.status === "ok" ? `${users.value.active} active` : undefined}
        />
        <KpiTile
          label="Groups"
          to="/control-plane/groups"
          value={renderCount(groups, (v) => v.total)}
          sub={
            groups.status === "ok"
              ? `${groups.value.local} local · ${groups.value.synced} synced`
              : undefined
          }
        />
        <KpiTile
          label="Roles"
          to="/control-plane/roles"
          value={renderCount(roles, (v) => v.total)}
          sub={roles.status === "ok" ? `${roles.value.platform} platform · ${roles.value.workspace} workspace` : undefined}
        />
        <KpiTile
          label="Platform admins"
          to="/control-plane/users"
          value={renderCount(platformAdmins, (v) => v)}
          highlight={highlightAdmins}
          sub={
            platformAdmins.status === "ok" && users.status === "ok" ? (
              highlightAdmins ? (
                // Icon + text, not colour alone, so the "review this" signal
                // survives for colour-blind users (WCAG color-not-only).
                <span
                  className="inline-flex items-center gap-1 font-medium"
                  style={{ color: "var(--highlight)" }}
                >
                  <AlertTriangle className="h-3 w-3" />
                  {adminPct}% of users — review
                </span>
              ) : (
                `${adminPct}% of users`
              )
            ) : undefined
          }
        />
      </div>

      {/* The working area: what needs attention (primary) beside the recent
          real changes (secondary). Two columns on desktop, stacked on mobile.
          Non-admins can't read the audit feed, so they get a full-width panel. */}
      {isPlatformAdmin ? (
        <div className="grid gap-5 lg:grid-cols-3">
          <div className="lg:col-span-2">
            <NeedsAttentionCard warnings={warnings} />
          </div>
          <RecentChangesPanel />
        </div>
      ) : (
        <NeedsAttentionCard warnings={warnings} />
      )}

      {/* Orientation content: open by default so the access model is visible at
          a glance, but collapsible for admins who don't need the reminder. It
          sits last so the actionable panels still lead the page. */}
      <details open className="group rounded-xl border bg-card text-card-foreground shadow-sm">
        <summary className="flex cursor-pointer list-none items-center justify-between gap-2 p-4 text-lg font-semibold [&::-webkit-details-marker]:hidden">
          How access works
          <ChevronDown className="h-5 w-5 text-muted-foreground transition-transform group-open:rotate-180" />
        </summary>
        <div className="px-4 pb-4">
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
        </div>
      </details>
    </div>
  );
}

/**
 * The primary posture panel: an actionable list of things worth the admin's
 * attention (unassigned roles, an over-broad admin set), each row linking to
 * where it's fixed. When there's nothing, a positive "all clear" state rather
 * than a bare line — the absence of problems is itself information.
 */
function NeedsAttentionCard({
  warnings,
}: {
  warnings: { key: string; title: string; detail: string; to: string }[];
}) {
  return (
    <Card className="h-full">
      <CardHeader className="flex-row items-center justify-between gap-2 space-y-0">
        <CardTitle className="text-lg">Needs attention</CardTitle>
        {warnings.length > 0 ? (
          <Badge variant="warning">{warnings.length}</Badge>
        ) : null}
      </CardHeader>
      <CardContent>
        {warnings.length === 0 ? (
          <div className="flex flex-col items-center gap-2 py-8 text-center">
            <span className="grid h-10 w-10 place-items-center rounded-full border">
              <CheckCircle2 className="h-5 w-5" style={{ color: "var(--success)" }} />
            </span>
            <p className="text-sm font-medium">You&apos;re all clear</p>
            <p className="text-xs text-muted-foreground">
              No posture issues need your attention right now.
            </p>
          </div>
        ) : (
          <ul className="space-y-2">
            {warnings.map((w) => (
              <li key={w.key}>
                <Link
                  to={w.to}
                  className="group flex items-start gap-3 rounded-lg border p-3 transition-colors hover:border-primary/40 hover:bg-accent"
                >
                  <span className="mt-0.5 grid h-8 w-8 shrink-0 place-items-center rounded-md border">
                    <AlertTriangle
                      className="h-4 w-4"
                      style={{ color: "var(--warning)" }}
                    />
                  </span>
                  <div className="min-w-0 flex-1">
                    <p className="text-sm font-medium text-foreground">{w.title}</p>
                    <p className="text-xs text-muted-foreground">{w.detail}</p>
                  </div>
                  <span className="mt-0.5 inline-flex shrink-0 items-center gap-1 text-xs font-medium text-primary">
                    Review
                    <ArrowRight className="h-3.5 w-3.5 transition-transform group-hover:translate-x-0.5" />
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * Render a KPI value: a skeleton shimmer while loading (so a slow tile reads as
 * "loading", not "broken", next to siblings that already resolved), "—" on
 * error, else the mapped value.
 */
function renderCount<T>(loadable: Loadable<T>, map: (v: T) => ReactNode): ReactNode {
  if (loadable.status === "loading") return <Skeleton className="h-7 w-14 rounded-md" />;
  if (loadable.status === "error") return "—";
  return map(loadable.value);
}

function KpiTile({
  label,
  value,
  sub,
  to,
  highlight,
}: {
  label: string;
  value: ReactNode;
  sub?: ReactNode;
  /** When set, the whole tile is a link into the matching section. */
  to?: string;
  highlight?: boolean;
}) {
  const card = (
    <Card
      className={cn(
        "h-full transition-colors",
        to && "hover:border-primary/50",
        highlight && "border-highlight/50",
      )}
    >
      <CardContent className="space-y-1 py-4">
        <p className="text-xs uppercase tracking-wide text-muted-foreground">{label}</p>
        <div className={cn("text-2xl font-semibold tabular-nums", highlight && "text-highlight")}>
          {value}
        </div>
        {sub ? <div className="text-xs text-muted-foreground">{sub}</div> : null}
      </CardContent>
    </Card>
  );

  if (!to) return card;
  return (
    <Link
      to={to}
      // The offset is gone entirely now, not merely coloured. `ring-offset-2`
      // widened the gap between the card and its focus ring and filled it with
      // `--tw-ring-offset-color`; left unset that defaulted to `#fff` and painted
      // a 2px WHITE band inside the ring on a dark page, so #415 set it to
      // `ring-offset-background`. That fixed the white, but `--background` is not
      // the surface these cards sit on either — they are a grid of `--card` tiles,
      // and in dark mode `--background` is oklch(0.14) against a card of
      // oklch(0.21), i.e. the band merely changed from too-bright to too-dark.
      // There is no colour that is correct here, because the gap wants to be
      // "whatever is behind the card" and Tailwind can only be told one colour.
      // A flush ring has no gap to fill and abuts the card directly, which is
      // also the boundary `focusIndicatorContrast.test.ts` actually measures. See
      // the long note at the bottom of that file for the rest of the reasoning,
      // including why an offset also cannot coexist with the `ring-inset` that
      // clipping containers need. That file now sweeps the whole tree, so this
      // cannot drift back in unnoticed.
      className="block rounded-xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      {card}
    </Link>
  );
}

/**
 * Platform-admin-only recent-changes feed. Kept as a separate component so
 * the audit hook is only invoked when this panel is actually mounted —
 * non-admins never issue the request.
 */
function RecentChangesPanel() {
  // `kind: "mutation"` filters out the read/authorization-decision events that
  // otherwise flood the feed — "Recent changes" should mean actual changes.
  const auditQuery = useControlPlaneAuditListAudit(
    { limit: 8, kind: "mutation" },
    { query: { retry: false } },
  );
  const page = auditQuery.data?.status === 200 ? auditQuery.data.data : undefined;

  return (
    <Card className="h-full">
      <CardHeader>
        <CardTitle className="text-lg">Recent changes</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        {auditQuery.isLoading ? (
          <div className="space-y-2">
            {Array.from({ length: 3 }).map((_, i) => (
              <Skeleton key={i} className="h-4 w-full" />
            ))}
          </div>
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
                    by {e.actorEmail ?? e.actorUserId.slice(0, 8)}
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
