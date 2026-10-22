import { useState, type ReactNode } from "react";
import type { GrantView, PrincipalSet } from "@/api/generated/model";
import { GrantDialog } from "@/routes/governance/GrantDialog";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { TabError } from "@/routes/governance/tabParts";
import { PrincipalHeader } from "@/routes/governance/people/PrincipalHeader";
import { usePrincipal, type PageKind, type Principal } from "@/routes/governance/people/principalData";
import { PEOPLE_BASE, principalParam } from "@/routes/governance/people/peopleUrl";
import { OverviewTab } from "@/routes/governance/people/tabs/OverviewTab";
import { PermissionsTab } from "@/routes/governance/people/tabs/PermissionsTab";
import { MasksFiltersTab } from "@/routes/governance/people/tabs/MasksFiltersTab";
import { ClearancesTab } from "@/routes/governance/people/tabs/ClearancesTab";
import { CheckAccessTab } from "@/routes/governance/people/tabs/CheckAccessTab";
import { TERM } from "@/routes/governance/glossary";
import { PageContainer } from "@/components/layout/PageContainer";

export type PrincipalTab = "overview" | "permissions" | "masks" | "clearances" | "check";

const LABELS: Record<PrincipalTab, string> = {
  overview: "Overview",
  permissions: "Permissions",
  masks: TERM.rules,
  clearances: "Clearances",
  check: "Check access",
};

/**
 * The tabs of each kind, in order (spec §2's table). `public` has no members,
 * so no Overview, Clearances or Check access.
 */
export const TABS_BY_KIND: Record<PageKind | "public", readonly PrincipalTab[]> = {
  user: ["overview", "permissions", "masks", "clearances", "check"],
  group: ["overview", "permissions", "masks", "clearances", "check"],
  role: ["overview", "permissions", "clearances", "check"],
  public: ["permissions", "masks"],
};

/** `No group named x. Grants naming it match nobody; see Unmatched names.` and the user and role equivalents. */
function Missing({ kind, name }: { kind: PageKind; name: string }) {
  if (kind === "user") {
    return (
      <Alert>
        <AlertDescription>{`No user has the id ${name}.`}</AlertDescription>
      </Alert>
    );
  }
  return (
    <Alert>
      <AlertDescription>
        <p>
          {`No ${kind} named ${name}. Grants naming it match nobody; see `}
          <Link to={`${PEOPLE_BASE}?kind=unmatched`} className="text-primary underline">
            Unmatched names
          </Link>
          .
        </p>
      </AlertDescription>
    </Alert>
  );
}

/**
 * A user's, group's or role's page (spec §2): the header, then Radix tabs.
 * The selected tab is `?tab=` (FE-PAGE-2); the kind's first tab is the
 * absence of the parameter, and a tab the kind does not have shows the first.
 * The header renders from the URL while the principal loads or fails, so its
 * links keep working; only the tabs wait for the read.
 */
export default function PrincipalPage({ kind }: { kind: PageKind }) {
  const params = useParams();
  const key = principalParam(kind === "user" ? params.userId : params.name);
  const state = usePrincipal(kind, key);
  const principal = state.status === "ok" ? state.principal : null;
  // The page's one GrantDialog: Grant access opens it new, with this principal
  // in include; the Permissions tab's Edit opens it on a grant.
  const [grantDialog, setGrantDialog] = useState<{ existing: GrantView | null } | null>(null);
  const include = principal ? includeOf(principal) : null;

  let body: ReactNode;
  if (state.status === "missing") body = <Missing kind={kind} name={key} />;
  else if (state.status === "error") body = <TabError what={kind} onRetry={state.retry} />;
  else if (state.status === "loading") body = <ListSkeleton rows={4} cols={3} />;
  else body = <PrincipalTabs principal={state.principal} onEdit={(existing) => setGrantDialog({ existing })} />;

  return (
    <PageContainer className="space-y-6">
      <PrincipalHeader
        kind={kind}
        keyName={key}
        principal={principal}
        onGrant={principal ? () => setGrantDialog({ existing: null }) : undefined}
        grantBlocked={principal && !include ? GRANT_NEEDS_USERNAME : undefined}
      />
      {body}
      <GrantDialog
        open={grantDialog !== null}
        onOpenChange={(open) => {
          if (!open) setGrantDialog(null);
        }}
        existing={grantDialog?.existing ?? null}
        initialPrincipals={include ?? undefined}
      />
    </PageContainer>
  );
}

export const GRANT_NEEDS_USERNAME = "A grant names a user by username, and this user has none yet.";

/** The principal as a grant's include names it: a user by username (none without one), a group or role by name. */
function includeOf(principal: Principal): PrincipalSet | null {
  if (principal.kind === "user") {
    const username = principal.data.username;
    return username ? { include: [{ kind: "user", ref: username }], exclude: [] } : null;
  }
  return { include: [{ kind: principal.kind, ref: principal.data.name }], exclude: [] };
}

function PrincipalTabs({ principal, onEdit }: { principal: Principal; onEdit: (grant: GrantView) => void }) {
  const [search, setSearch] = useSearchParams();
  const isPublic = principal.kind === "group" && principal.data.isPublic;
  const tabs = TABS_BY_KIND[isPublic ? "public" : principal.kind];
  const asked = search.get("tab");
  const value = tabs.find((t) => t === asked) ?? tabs[0];

  const select = (next: string) =>
    setSearch((prev) => {
      const params = new URLSearchParams(prev);
      if (next === tabs[0]) params.delete("tab");
      else params.set("tab", next);
      return params;
    });

  const panel = (t: PrincipalTab): ReactNode => {
    if (t === "overview") return <OverviewTab principal={principal} />;
    if (t === "permissions") return <PermissionsTab principal={principal} onEdit={onEdit} />;
    if (t === "masks") return <MasksFiltersTab principal={principal} />;
    if (t === "clearances") return <ClearancesTab principal={principal} />;
    return <CheckAccessTab principal={principal} />;
  };

  return (
    <Tabs value={value} onValueChange={select} className="min-w-0">
      {/* Wraps, so every tab is in view at 390 px. */}
      <TabsList className="h-auto max-w-full flex-wrap justify-start">
        {tabs.map((t) => (
          <TabsTrigger key={t} value={t}>
            {LABELS[t]}
          </TabsTrigger>
        ))}
      </TabsList>
      {tabs.map((t) => (
        <TabsContent key={t} value={t} className="mt-4">
          {t === value ? panel(t) : null}
        </TabsContent>
      ))}
    </Tabs>
  );
}
