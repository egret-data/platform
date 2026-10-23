import { useEffect, useState } from "react";
import {
  Database,
  Inbox,
  type LucideIcon,
  Mail,
  PanelLeftClose,
  PanelLeftOpen,
  Settings2,
  Share2,
  ShieldCheck,
  Users,
} from "lucide-react";
import { NavLink, Outlet, useParams } from "react-router-dom";

import { useMyWorkspaces } from "@/api/hooks";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { PageContainer } from "@/components/layout/PageContainer";
import { DetailHeader } from "@/components/layout/DetailHeader";
import { tabsListClass, tabsTriggerClass } from "@/components/ui/tabs";

type SettingsItem = { to: string; label: string; icon: LucideIcon };
type SettingsGroup = { heading: string; items: SettingsItem[] };

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
      { to: "members", label: "Members", icon: Users },
      { to: "roles", label: "Roles & permissions", icon: ShieldCheck },
      { to: "requests", label: "Access requests", icon: Inbox },
      { to: "shares", label: "Shared objects", icon: Share2 },
    ],
  },
  {
    heading: "Connections",
    items: [{ to: "engines", label: "Engines", icon: Database }],
  },
  {
    heading: "Workspace",
    items: [
      { to: "notifications", label: "Notifications", icon: Mail },
      { to: "general", label: "General", icon: Settings2 },
    ],
  },
];

const ALL_ITEMS = GROUPS.flatMap((g) => g.items);

const itemActive = "bg-primary/10 font-medium text-primary";
const itemIdle = "text-muted-foreground hover:bg-accent hover:text-foreground";
const focusRing = "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

/** localStorage key remembering whether the settings nav is collapsed to a rail. */
const COLLAPSED_KEY = "egret:ws-settings-nav-collapsed";

function loadCollapsed(): boolean {
  try {
    return localStorage.getItem(COLLAPSED_KEY) === "1";
  } catch {
    return false;
  }
}

/** A single section link — full (icon + label) or, when collapsed, an icon-only
 *  rail button with a tooltip so it stays identifiable. */
function NavItem({ item, collapsed }: { item: SettingsItem; collapsed: boolean }) {
  const Icon = item.icon;
  const link = (
    <NavLink
      to={item.to}
      end
      className={({ isActive }) =>
        collapsed
          ? `flex h-9 w-9 items-center justify-center rounded-md transition-colors ${focusRing} ${
              isActive ? itemActive : itemIdle
            }`
          : `flex items-center gap-2.5 rounded-md px-3 py-2 text-sm transition-colors ${focusRing} ${
              isActive ? itemActive : itemIdle
            }`
      }
    >
      <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
      {collapsed ? (
        <span className="sr-only">{item.label}</span>
      ) : (
        <span className="truncate">{item.label}</span>
      )}
    </NavLink>
  );

  if (!collapsed) return link;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{link}</TooltipTrigger>
      <TooltipContent side="right">{item.label}</TooltipContent>
    </Tooltip>
  );
}

/**
 * Workspace settings shell.
 *
 * A settings-hub layout: a page title on top, then a grouped section nav on the
 * left with the active section rendered in the Outlet on the right. The h1 is
 * the page title (`PageTitle`, 20px), above each section's own heading.
 * The desktop nav collapses to an icon rail (mirroring the main sidebar) to hand
 * the content pane more room; the choice is remembered. On narrow screens the
 * nav becomes a horizontally scrollable tab strip instead.
 */
export default function WorkspaceSettings() {
  const { workspace: slug } = useParams();
  const { data: workspaces } = useMyWorkspaces(true);
  const ws = workspaces?.find((w) => w.slug === slug);

  const [collapsed, setCollapsed] = useState(loadCollapsed);
  useEffect(() => {
    try {
      localStorage.setItem(COLLAPSED_KEY, collapsed ? "1" : "0");
    } catch {
      /* ignore persistence failures (private mode, etc.) */
    }
  }, [collapsed]);

  const gridCols = collapsed
    ? "lg:grid-cols-[3.25rem_minmax(0,1fr)]"
    : "lg:grid-cols-[13rem_minmax(0,1fr)]";

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

      {/* Mobile: horizontal scrollable tab strip (group headings dropped — the
          content pane needs the width more than the grouping does here). */}
      <nav
        className={`-mx-4 mb-5 px-4 lg:hidden ${tabsListClass("underline")}`}
        aria-label="Workspace settings sections"
      >
        {ALL_ITEMS.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end
            className={tabsTriggerClass("underline")}
          >
            {item.label}
          </NavLink>
        ))}
      </nav>

      <div className={`lg:grid lg:gap-8 ${gridCols}`}>
        {/* Desktop: grouped vertical settings nav, collapsible to an icon rail. */}
        <nav
          className={`hidden self-start lg:sticky lg:top-6 lg:flex lg:flex-col ${
            collapsed ? "lg:items-center" : ""
          }`}
          aria-label="Workspace settings sections"
        >
          {GROUPS.map((group, groupIndex) =>
            collapsed ? (
              <div
                key={group.heading}
                className={`space-y-0.5 ${
                  groupIndex > 0 ? "mt-2 w-full border-t border-border pt-2" : ""
                }`}
              >
                {group.items.map((item) => (
                  <NavItem key={item.to} item={item} collapsed />
                ))}
              </div>
            ) : (
              <div key={group.heading} className="mb-5 last:mb-0">
                <p className="mb-1 px-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">
                  {group.heading}
                </p>
                <div className="space-y-0.5">
                  {group.items.map((item) => (
                    <NavItem key={item.to} item={item} collapsed={false} />
                  ))}
                </div>
              </div>
            ),
          )}

          <button
            type="button"
            onClick={() => setCollapsed((c) => !c)}
            aria-label={collapsed ? "Expand settings navigation" : "Collapse settings navigation"}
            aria-pressed={collapsed}
            className={`mt-3 flex items-center gap-2 rounded-md py-2 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground ${focusRing} ${
              collapsed ? "h-9 w-9 justify-center" : "border-t border-border px-3"
            }`}
          >
            {collapsed ? (
              <PanelLeftOpen className="h-4 w-4 shrink-0" aria-hidden="true" />
            ) : (
              <>
                <PanelLeftClose className="h-4 w-4 shrink-0" aria-hidden="true" />
                <span>Collapse</span>
              </>
            )}
          </button>
        </nav>

        <div className="min-w-0 max-w-page" data-settings-pane>
          <Outlet />
        </div>
      </div>
    </PageContainer>
  );
}
