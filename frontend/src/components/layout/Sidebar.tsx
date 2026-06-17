import { NavLink, useLocation, useParams } from "react-router-dom";

const EXPLORE = ["SQL Lab", "Datasets", "Charts", "Dashboards", "Notebooks"];
const ADMIN = ["Databases", "Catalog", "Policies", "Users", "Workspaces", "Monitoring"];
const slug = (s: string) => s.toLowerCase().replace(/\s+/g, "-");

export function Sidebar() {
  const isAdmin = useLocation().pathname.startsWith("/admin");
  const { workspace } = useParams();
  const items = isAdmin ? ADMIN : EXPLORE;
  const base = isAdmin ? "/admin" : `/explore/${workspace ?? ""}`;

  return (
    <nav className="w-56 shrink-0 border-r border-border bg-card p-2">
      {items.map((label) => (
        <NavLink
          key={label}
          to={`${base}/${slug(label)}`}
          className={({ isActive }) =>
            `block rounded px-3 py-2 text-sm ${isActive ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:bg-accent/50"}`
          }
        >
          {label}
        </NavLink>
      ))}
    </nav>
  );
}
