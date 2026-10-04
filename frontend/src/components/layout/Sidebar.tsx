import { useState } from "react";
import { NavLink, useLocation, useParams } from "react-router-dom";
import {
  Activity, ChevronDown, ChevronRight, Compass, EyeOff,
  FolderKanban, Gauge, HardDrive, KeyRound, Library, ListChecks, type LucideIcon,
  PanelLeftClose, PanelLeftOpen, Plug, ScrollText, Search, Server, Settings, Share2, ShieldCheck,
  Tags, UserCheck, UserCog, Users, UsersRound,
} from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useAuth } from "@/auth/useAuth";
import { appForPath } from "@/auth/appAccess";
import { useWorkspaceFeatures } from "@/hooks/useWorkspaceFeatures";
import { EXPLORE_MODULES } from "@/lib/exploreModules";

/** `perm` is the fine-grained read permission that unlocks this item in the
 * Control Plane or Governance; items with no `perm` (e.g. Workspaces, Audit log)
 * are platform_admin-only. `feature` is the workspace entitlement key (e.g.
 * `apps`) that unlocks an Explore item; items with no `feature` are
 * unaffected by entitlements. */
type NavItem = { label: string; icon: LucideIcon; perm?: string; feature?: string; to?: string };
type NavGroup = { heading: string; items: NavItem[] };

const EXPLORE_GROUPS: NavGroup[] = [
  {
    heading: "Explore",
    // The modules + their order/route come from EXPLORE_MODULES, the
    // single source of truth shared with useDefaultExplorePath — see there.
    items: EXPLORE_MODULES.map((m) => ({ label: m.label, icon: m.icon, feature: m.key, to: m.path })),
  },
  {
    heading: "Data API",
    // Its own top-level group, not an item nested under another group's
    // shell: three sibling sections plus a detail route behave like a
    // module, the same reason Charts and Datasets are each their own group
    // rather than items under a shared shell. Every item carries
    // `feature: "data_api"` so the whole group
    // hides when the workspace lacks the entitlement — see
    // docs/en/specs/2026-08-16-data-api-p5-design.md §2.1.
    items: [
      { label: "Endpoints", icon: Plug, to: "data-api/endpoints", feature: "data_api" },
      { label: "Clients", icon: KeyRound, to: "data-api/clients", feature: "data_api" },
      { label: "Activity", icon: Activity, to: "data-api/activity", feature: "data_api" },
    ],
  },
  {
    heading: "Manage",
    items: [
      // Engine connections live inside Workspace settings → Connections now,
      // so they're reached from there rather than duplicated at top level.
      { label: "Workspace settings", icon: Settings, to: "settings/members" },
    ],
  },
];

// Governance = the data-plane rules a data steward manages (SP2: split out of the
// Control Plane). Headings must stay distinct from every other app's headings —
// collapsed-group state is one localStorage key across all three apps.
const GOVERNANCE_GROUPS: NavGroup[] = [
  // SP4b-2: data-plane permission UI, steward-facing first, then admin-facing —
  // "Data permissions" alone (not "Permissions") is deliberate: Control Plane →
  // Access → Permissions already names the RBAC permission *catalog*, and these
  // pages are about data access instead. Order per the design spec's D4: Data
  // permissions, Mask & filter rules, Explain access, then the existing Policies.
  {
    heading: "Data permissions",
    items: [{ label: "Data permissions", icon: UserCheck, perm: "policy:read", to: "data-permissions" }],
  },
  {
    heading: "Mask & filter rules",
    items: [{ label: "Mask & filter rules", icon: EyeOff, perm: "policy:read", to: "mask-filter-rules" }],
  },
  {
    heading: "Explain access",
    items: [{ label: "Explain access", icon: Search, perm: "policy:read", to: "explain-access" }],
  },
  {
    heading: "Policies",
    items: [{ label: "Policies", icon: ShieldCheck, perm: "policy:read", to: "policies" }],
  },
  {
    heading: "Classification",
    items: [{ label: "Classifications", icon: Tags, perm: "tag:read", to: "classifications" }],
  },
  {
    heading: "Lineage",
    // Platform-wide (cross-workspace), so it lives here rather than inside a single
    // workspace. Admin-only until the real feature + its permission land.
    items: [{ label: "Data Lineage", icon: Share2, to: "data-lineage" }],
  },
];

// Control Plane = system administration. Grouped by a mental model, not a feature
// list: Overview (posture hub) → Identity (who) → Access (what they can do/see) →
// Infrastructure (the platform itself) → System (the platform's own records).
const CONTROL_PLANE_GROUPS: NavGroup[] = [
  {
    heading: "Overview",
    items: [{ label: "Access overview", icon: Gauge, perm: "user:read", to: "access-overview" }],
  },
  {
    heading: "Identity",
    items: [
      { label: "Users", icon: Users, perm: "user:read" },
      { label: "Groups", icon: UsersRound, perm: "group:read" },
      { label: "Roles", icon: UserCog, perm: "role:read" },
    ],
  },
  {
    heading: "Access",
    items: [
      { label: "Access Explorer", icon: Compass, perm: "role:read", to: "access-explorer" },
      { label: "Permissions", icon: ListChecks, perm: "role:read" },
    ],
  },
  {
    heading: "Infrastructure",
    items: [
      { label: "Workspaces", icon: FolderKanban },
      { label: "Engines", icon: Server, perm: "engine:read" },
      { label: "Catalogs", icon: Library, perm: "catalog_registry:read" },
      { label: "Storage", icon: HardDrive, perm: "storage:read" },
      { label: "Secrets", icon: KeyRound, perm: "secret:read" },
    ],
  },
  {
    heading: "System",
    // API-layer authorization decisions and admin mutations — a system log, not a
    // data access log, so it stays here and not in Governance.
    items: [{ label: "Audit log", icon: ScrollText }],
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
  const app = appForPath(useLocation().pathname);
  const { workspace } = useParams();
  const { me } = useAuth();
  const isPlatformAdmin = me?.platform_roles.includes("platform_admin") ?? false;
  const permissions = me?.permissions ?? [];
  const enabledFeatures = useWorkspaceFeatures();

  // Governance and Control Plane items are permission-gated: platform_admin sees
  // everything, everyone else sees only items whose `perm` they hold. Items with
  // no `perm` are platform_admin-only. Explore items are entitlement-gated: an item
  // with a `feature` only shows when the workspace has it switched on
  // (entitlements apply to everyone, admins included — no bypass). Either way,
  // groups left with no visible items are dropped entirely (heading included).
  const permissionGroups =
    app === "governance" ? GOVERNANCE_GROUPS : app === "control-plane" ? CONTROL_PLANE_GROUPS : null;
  const groups = permissionGroups
    ? permissionGroups.map((group) => ({
        ...group,
        items: group.items.filter(
          (item) => isPlatformAdmin || (item.perm !== undefined && permissions.includes(item.perm)),
        ),
      })).filter((group) => group.items.length > 0)
    : EXPLORE_GROUPS.map((group) => ({
        ...group,
        items: group.items.filter((item) => !item.feature || enabledFeatures.includes(item.feature)),
      })).filter((group) => group.items.length > 0);
  const base =
    app === "governance" ? "/governance" : app === "control-plane" ? "/control-plane" : `/explore/${workspace ?? ""}`;

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

  const renderItem = ({ label, icon: Icon, to }: NavItem) => {
    const path = `${base}/${to ?? slug(label)}`;
    if (collapsed) {
      return (
        <Tooltip key={label}>
          <TooltipTrigger asChild>
            <NavLink to={path} className={linkClass}>
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
        to={path}
        className={linkClass}
      >
        <Icon className="h-4 w-4 shrink-0" />
        {label}
      </NavLink>
    );
  };

  return (
    <nav
      data-testid="app-sidebar"
      data-collapsed={collapsed ? "true" : "false"}
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
