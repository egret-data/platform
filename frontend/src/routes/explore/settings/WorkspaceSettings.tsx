import { NavLink, Outlet } from "react-router-dom";

const subNavLinkClass = ({ isActive }: { isActive: boolean }) =>
  `px-3 py-1.5 text-sm font-medium rounded-md transition-colors ${
    isActive
      ? "bg-accent text-accent-foreground"
      : "text-muted-foreground hover:bg-accent/50 hover:text-foreground"
  }`;

export default function WorkspaceSettings() {
  return (
    <div className="flex flex-col gap-6 p-6">
      <h1 className="text-xl font-semibold">Workspace settings</h1>

      <nav className="flex gap-1 border-b border-border pb-0">
        <NavLink to="catalog" className={subNavLinkClass}>
          Catalog access
        </NavLink>
        <NavLink to="storage" className={subNavLinkClass}>
          Storage
        </NavLink>
      </nav>

      <div>
        <Outlet />
      </div>
    </div>
  );
}
