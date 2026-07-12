import { useState } from "react";
import { NavLink, useLocation, useParams } from "react-router-dom";
import {
  BarChart3, Bell, Boxes, ChevronDown, ChevronRight, Database,
  FolderKanban, HardDrive, KeyRound, LayoutDashboard, ListChecks, type LucideIcon,
  NotebookText, PanelLeftClose, PanelLeftOpen, Plug, ScrollText, Server, Share2, ShieldCheck,
  SquareTerminal, Table2, UserCog, Users, UsersRound, Waypoints, Workflow,
} from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useAuth } from "@/auth/useAuth";

/** `perm` is the fine-grained read permission that unlocks this item in the
 * Control Plane; items with no `perm` (e.g. Workspaces) are platform_admin-only. */
type NavItem = { label: string; icon: LucideIcon; perm?: string };
type NavGroup = { heading: string; items: NavItem[] };

const EXPLORE_GROUPS: NavGroup[] = [
  {
    heading: "Explore",
    items: [
      { label: "SQL Lab", icon: SquareTerminal },
      { label: "Datasets", icon: Table2 },
      { label: "Charts", icon: BarChart3 },
      { label: "Dashboards", icon: LayoutDashboard },
      { label: "Notebooks", icon: NotebookText },
    ],
  },
  {
    heading: "Automate",
    items: [
      { label: "Workflows", icon: Workflow },
      { label: "Alerts", icon: Bell },
    ],
  },
  {
    heading: "Data Engineering",
    items: [
      { label: "Data API", icon: Plug },
      { label: "Pipelines", icon: Waypoints },
    ],
  },
  {
    heading: "Data Governance",
    items: [
      { label: "Data Lineage", icon: Share2 },
    ],
  },
  {
    heading: "Manage",
    items: [
      { label: "Engines", icon: Database },
    ],
  },
];

// Control Plane = global-admin only; platform-level concerns: workspaces,
// engine provisioning, and user role/workspace assignment.
const CONTROL_PLANE_GROUPS: NavGroup[] = [
  {
    heading: "Control Plane",
    items: [
      { label: "Workspaces", icon: FolderKanban },
      { label: "Engines", icon: Server, perm: "engine:read" },
      { label: "Warehouses", icon: Boxes, perm: "warehouse:read" },
      { label: "Storage", icon: HardDrive, perm: "storage:read" },
      { label: "Secrets", icon: KeyRound, perm: "secret:read" },
    ],
  },
  {
    heading: "Access control",
    items: [
      { label: "Users", icon: Users, perm: "user:read" },
      { label: "Groups", icon: UsersRound, perm: "group:read" },
      { label: "Roles", icon: UserCog, perm: "role:read" },
      { label: "Permissions", icon: ListChecks, perm: "role:read" },
    ],
  },
  {
    heading: "Data Governance",
    items: [
      { label: "Policies", icon: ShieldCheck, perm: "policy:read" },
      { label: "Audit log", icon: ScrollText },
    ],
  },
];

const slug = (s: string) => s.toLowerCase().replace(/\s+/g, "-");

/** localStorage key holding the headings of collapsed sidebar groups. */
const COLLAPSED_GROUPS_KEY = "egret:sidebar-collapsed-groups";

function loadCollapsedGroups(): Set<string> {
  try {
    const raw = localStorage.getItem(COLLAPSED_GROUPS_KEY);
    return new Set(raw ? (JSON.parse(raw) as string[]) : []);
  } catch {
    return new Set();
  }
}

interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
}

export function Sidebar({ collapsed, onToggle }: SidebarProps) {
  const isAdmin = useLocation().pathname.startsWith("/control-plane");
  const { workspace } = useParams();
  const { me } = useAuth();
  const isPlatformAdmin = me?.platform_roles.includes("platform_admin") ?? false;
  const permissions = me?.permissions ?? [];

  // Control-plane items are permission-gated: platform_admin sees everything,
  // everyone else sees only items whose `perm` they hold. Items with no `perm`
  // (Workspaces) are platform_admin-only. Groups left with no visible items
  // are dropped entirely (heading included). Explore items are unchanged.
  const groups = isAdmin
    ? CONTROL_PLANE_GROUPS.map((group) => ({
        ...group,
        items: group.items.filter(
          (item) => isPlatformAdmin || (item.perm !== undefined && permissions.includes(item.perm)),
        ),
      })).filter((group) => group.items.length > 0)
    : EXPLORE_GROUPS;
  const base = isAdmin ? "/control-plane" : `/explore/${workspace ?? ""}`;

  // Per-group collapse (only meaningful when the sidebar itself is expanded).
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(loadCollapsedGroups);

  const toggleGroup = (heading: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(heading)) next.delete(heading);
      else next.add(heading);
      try {
        localStorage.setItem(COLLAPSED_GROUPS_KEY, JSON.stringify([...next]));
      } catch {
        /* ignore quota / unavailable storage */
      }
      return next;
    });
  };

  const linkClass = ({ isActive }: { isActive: boolean }) =>
    `flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors ${
      collapsed ? "justify-center" : ""
    } ${
      isActive
        ? "bg-accent text-accent-foreground"
        : "text-muted-foreground hover:bg-accent/50 hover:text-foreground"
    }`;

  const renderItem = ({ label, icon: Icon }: NavItem) => {
    if (collapsed) {
      return (
        <Tooltip key={label}>
          <TooltipTrigger asChild>
            <NavLink to={`${base}/${slug(label)}`} className={linkClass}>
              <Icon className="h-4 w-4 shrink-0" />
            </NavLink>
          </TooltipTrigger>
          <TooltipContent side="right">{label}</TooltipContent>
        </Tooltip>
      );
    }

    return (
      <NavLink
        key={label}
        to={`${base}/${slug(label)}`}
        className={linkClass}
      >
        <Icon className="h-4 w-4 shrink-0" />
        {label}
      </NavLink>
    );
  };

  return (
    <nav
      className={`flex shrink-0 flex-col gap-1 overflow-y-auto border-r border-border bg-card p-3 transition-[width] duration-200 ${
        collapsed ? "w-16" : "w-56"
      }`}
    >
      {groups.map(({ heading, items }, groupIndex) => {
        // Icon-rail mode: no headings / no per-group collapse, just separated icons.
        if (collapsed) {
          return (
            <div key={heading} className={groupIndex > 0 ? "mt-2" : undefined}>
              {groupIndex > 0 && <hr className="my-1 border-border" />}
              {items.map(renderItem)}
            </div>
          );
        }

        const groupCollapsed = collapsedGroups.has(heading);
        return (
          <div key={heading} className={groupIndex > 0 ? "mt-2" : undefined}>
            <button
              type="button"
              onClick={() => toggleGroup(heading)}
              aria-expanded={!groupCollapsed}
              className="flex w-full items-center justify-between rounded-md px-3 py-1 text-xs font-medium uppercase tracking-wider text-muted-foreground transition-colors hover:text-foreground"
            >
              <span>{heading}</span>
              {groupCollapsed ? (
                <ChevronRight className="h-3.5 w-3.5 shrink-0" />
              ) : (
                <ChevronDown className="h-3.5 w-3.5 shrink-0" />
              )}
            </button>
            {!groupCollapsed && items.map(renderItem)}
          </div>
        );
      })}

      {/* Toggle button pinned to bottom */}
      {collapsed ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              onClick={onToggle}
              aria-label="Toggle sidebar"
              className="mt-auto flex items-center justify-center rounded-md px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
            >
              <PanelLeftOpen className="h-4 w-4 shrink-0" />
            </button>
          </TooltipTrigger>
          <TooltipContent side="right">Expand</TooltipContent>
        </Tooltip>
      ) : (
        <button
          onClick={onToggle}
          aria-label="Toggle sidebar"
          className="mt-auto flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
        >
          <PanelLeftClose className="h-4 w-4 shrink-0" />
          <span>Collapse</span>
        </button>
      )}
    </nav>
  );
}
