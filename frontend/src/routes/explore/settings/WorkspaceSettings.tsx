import { NavLink, Outlet } from "react-router-dom";

const TABS = [
  { to: "members", label: "Members" },
  { to: "roles", label: "Roles & permissions" },
  { to: "shares", label: "Shared objects" },
];

export default function WorkspaceSettings() {
  return (
    <div className="p-4">
      <h1 className="mb-4 text-lg font-semibold">Workspace settings</h1>
      <nav className="mb-6 flex gap-1 border-b border-border" aria-label="Workspace settings sections">
        {TABS.map((t) => (
          <NavLink
            key={t.to}
            to={t.to}
            end
            className={({ isActive }) =>
              `-mb-px border-b-2 px-3 py-2 text-sm transition-colors ${
                isActive
                  ? "border-primary font-semibold text-foreground"
                  : "border-transparent font-medium text-muted-foreground hover:border-border hover:text-foreground"
              }`
            }
          >
            {t.label}
          </NavLink>
        ))}
      </nav>
      <div className="min-w-0">
        <Outlet />
      </div>
    </div>
  );
}
