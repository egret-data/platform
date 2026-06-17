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
  ShieldCheck,
  SquareTerminal,
  Table2,
  Users,
} from "lucide-react";

type NavItem = { label: string; icon: LucideIcon };

const EXPLORE: NavItem[] = [
  { label: "SQL Lab", icon: SquareTerminal },
  { label: "Datasets", icon: Table2 },
  { label: "Charts", icon: BarChart3 },
  { label: "Dashboards", icon: LayoutDashboard },
  { label: "Notebooks", icon: NotebookText },
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

export function Sidebar() {
  const isAdmin = useLocation().pathname.startsWith("/admin");
  const { workspace } = useParams();
  const items = isAdmin ? ADMIN : EXPLORE;
  const base = isAdmin ? "/admin" : `/explore/${workspace ?? ""}`;

  return (
    <nav className="flex w-56 shrink-0 flex-col gap-1 border-r border-border bg-card p-3">
      <p className="px-3 pb-1 text-xs font-medium uppercase tracking-wider text-muted-foreground">
        {isAdmin ? "Administration" : "Explore"}
      </p>
      {items.map(({ label, icon: Icon }) => (
        <NavLink
          key={label}
          to={`${base}/${slug(label)}`}
          className={({ isActive }) =>
            `flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors ${
              isActive
                ? "bg-accent text-accent-foreground"
                : "text-muted-foreground hover:bg-accent/50 hover:text-foreground"
            }`
          }
        >
          <Icon className="h-4 w-4 shrink-0" />
          {label}
        </NavLink>
      ))}
    </nav>
  );
}
