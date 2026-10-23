import {
  Database,
  Inbox,
  type LucideIcon,
  Mail,
  Settings2,
  Share2,
  ShieldCheck,
  Users,
} from "lucide-react";
import { Navigate, NavLink, Outlet, useParams } from "react-router-dom";

import { useMyWorkspaces } from "@/api/hooks";
import { useWorkspacePermission } from "@/hooks/useWorkspaceFeatures";
import { PageContainer } from "@/components/layout/PageContainer";
import { DetailHeader } from "@/components/layout/DetailHeader";
import { tabsListClass, tabsTriggerClass } from "@/components/ui/tabs";
import { ScrollStrip } from "@/components/ui/scroll-cue";

/** `perm`: the workspace permission the section's routes require (the
 *  server's `PAGE_PERMISSIONS`); a section the reader cannot open is left out
 *  of the nav (KI-231). The routes stay the gate: a typed URL still gets the
 *  section's own refusal. */
type SettingsItem = { to: string; label: string; icon: LucideIcon; perm?: string };
type SettingsGroup = { heading: string; items: SettingsItem[] };

/** Members, roles, requests, shares, engine connections and notification
 *  channels are all `workspace:manage_members` on the server. */
const MANAGE = "workspace:manage_members";

/**
 * Workspace settings sections, grouped by mental model rather than listed flat:
 * "Access" (who's in the workspace and what they can do) is the day-to-day work,
 * "Connections" is infrastructure wiring, "Workspace" is the identity of the
 * space itself. Grouping keeps the six sections scannable and leaves obvious
 * homes for future settings.
 */
const GROUPS: SettingsGroup[] = [
  {
    heading: "Access",
    items: [
      { to: "members", label: "Members", icon: Users, perm: MANAGE },
      { to: "roles", label: "Roles & permissions", icon: ShieldCheck, perm: MANAGE },
      { to: "requests", label: "Access requests", icon: Inbox, perm: MANAGE },
      { to: "shares", label: "Shared objects", icon: Share2, perm: MANAGE },
    ],
  },
  {
    heading: "Connections",
    items: [{ to: "engines", label: "Engines", icon: Database, perm: MANAGE }],
  },
  {
    heading: "Workspace",
    items: [
      { to: "notifications", label: "Notifications", icon: Mail, perm: MANAGE },
      { to: "general", label: "General", icon: Settings2 },
    ],
  },
];

const itemActive = "bg-primary/10 font-medium text-primary";
const itemIdle = "text-muted-foreground hover:bg-accent hover:text-foreground";
const focusRing = "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

/** The sections this reader may open, groups left empty dropped. */
function useVisibleGroups(): SettingsGroup[] {
  const holds = useWorkspacePermission();
  return GROUPS.map((g) => ({ ...g, items: g.items.filter((i) => holds(i.perm)) })).filter(
    (g) => g.items.length > 0,
  );
}

/** `/settings` itself: the first section this reader may open. General has no
 *  gate, so there always is one. It waits for the workspace list: before it,
 *  every section reads as open and a member would land on a refusal. */
export function SettingsIndex() {
  const { isLoading } = useMyWorkspaces(true);
  const first = useVisibleGroups()[0]?.items[0];
  if (isLoading) return null;
  return <Navigate to={first?.to ?? "general"} replace />;
}

/** A single section link: icon and label. */
function NavItem({ item }: { item: SettingsItem }) {
  const Icon = item.icon;
  return (
    <NavLink
      to={item.to}
      end
      className={({ isActive }) =>
        `flex items-center gap-2.5 rounded-md px-3 py-2 text-sm transition-colors ${focusRing} ${
          isActive ? itemActive : itemIdle
        }`
      }
    >
      <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
      <span className="truncate">{item.label}</span>
    </NavLink>
  );
}

/**
 * Workspace settings shell.
 *
 * A settings-hub layout: a page title on top, then a grouped section nav on the
 * left with the active section rendered in the Outlet on the right. The h1 is
 * the page title (`PageTitle`, 20px), above each section's own heading.
 * Below 1024px the nav becomes a horizontally scrollable tab strip. The
 * sections a reader cannot open are left out (KI-231).
 */
export default function WorkspaceSettings() {
  const { workspace: slug } = useParams();
  const { data: workspaces } = useMyWorkspaces(true);
  const ws = workspaces?.find((w) => w.slug === slug);
  const groups = useVisibleGroups();

  return (
    // The settings frame starts at the list's left edge; the section pane beside
    // the nav is the 960px column (FE-PAGE-6). No back link: the section nav
    // takes its place (FE-PAGE-8).
    <PageContainer>
      <DetailHeader
        className="mb-6"
        title="Workspace settings"
        description={
          ws?.name
            ? `Manage members, access, and configuration for ${ws.name}.`
            : "Manage members, access, and this workspace’s configuration."
        }
      />

      {/* Below 1024px: one scrolling tab strip (group headings dropped, the
          pane needs the width more than the grouping does), with a shade on
          the clipped edge (KI-233). */}
      <ScrollStrip
        className={`mb-5 lg:hidden ${tabsListClass("underline")}`}
        aria-label="Workspace settings sections"
      >
        {groups.flatMap((g) => g.items).map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end
            className={tabsTriggerClass("underline")}
          >
            {item.label}
          </NavLink>
        ))}
      </ScrollStrip>

      <div className="lg:grid lg:grid-cols-[13rem_minmax(0,1fr)] lg:gap-8">
        {/* 1024px and up: the grouped section nav. It has no Collapse of its
            own: the app sidebar's is the one on this page (KI-005). */}
        <nav
          className="hidden self-start lg:sticky lg:top-6 lg:flex lg:flex-col"
          aria-label="Workspace settings sections"
        >
          {groups.map((group) => (
            <div key={group.heading} className="mb-5 last:mb-0">
              <p className="mb-1 px-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">
                {group.heading}
              </p>
              <div className="space-y-0.5">
                {group.items.map((item) => (
                  <NavItem key={item.to} item={item} />
                ))}
              </div>
            </div>
          ))}
        </nav>

        <div className="min-w-0 max-w-page" data-settings-pane>
          <Outlet />
        </div>
      </div>
    </PageContainer>
  );
}
