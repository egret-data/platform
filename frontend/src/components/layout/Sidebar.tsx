import { NavLink, useLocation, useParams } from "react-router-dom";
import {
  Activity,
  BarChart3,
  Boxes,
  Database,
  FolderKanban,
  LayoutDashboard,
  type LucideIcon,
  NotebookText,
  PanelLeftClose,
  PanelLeftOpen,
  ShieldCheck,
  SquareTerminal,
  Table2,
  Users,
} from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

type NavItem = { label: string; icon: LucideIcon };

const EXPLORE: NavItem[] = [
  { label: "SQL Lab", icon: SquareTerminal },
  { label: "Datasets", icon: Table2 },
  { label: "Charts", icon: BarChart3 },
  { label: "Dashboards", icon: LayoutDashboard },
  { label: "Notebooks", icon: NotebookText },
  { label: "Catalog", icon: Boxes },
];

const ADMIN: NavItem[] = [
  { label: "Databases", icon: Database },
  { label: "Catalog", icon: Boxes },
  { label: "Policies", icon: ShieldCheck },
  { label: "Users", icon: Users },
  { label: "Workspaces", icon: FolderKanban },
  { label: "Monitoring", icon: Activity },
];

const slug = (s: string) => s.toLowerCase().replace(/\s+/g, "-");

interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
}

export function Sidebar({ collapsed, onToggle }: SidebarProps) {
  const isAdmin = useLocation().pathname.startsWith("/admin");
  const { workspace } = useParams();
  const items = isAdmin ? ADMIN : EXPLORE;
  const base = isAdmin ? "/admin" : `/explore/${workspace ?? ""}`;

  const linkClass = ({ isActive }: { isActive: boolean }) =>
    `flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors ${
      collapsed ? "justify-center" : ""
    } ${
      isActive
        ? "bg-accent text-accent-foreground"
        : "text-muted-foreground hover:bg-accent/50 hover:text-foreground"
    }`;

  return (
    <nav
      className={`flex shrink-0 flex-col gap-1 border-r border-border bg-card p-3 transition-[width] duration-200 ${
        collapsed ? "w-16" : "w-56"
      }`}
    >
      {!collapsed && (
        <p className="px-3 pb-1 text-xs font-medium uppercase tracking-wider text-muted-foreground">
          {isAdmin ? "Administration" : "Explore"}
        </p>
      )}

      {items.map(({ label, icon: Icon }) => {
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
