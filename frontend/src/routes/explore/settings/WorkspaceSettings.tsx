import { NavLink, Outlet } from "react-router-dom";

const TABS = [
  { to: "members", label: "Members" },
  { to: "roles", label: "Roles & permissions" },
  { to: "shares", label: "Shared objects" },
  { to: "requests", label: "Requests" },
  { to: "engines", label: "Engines" },
  { to: "general", label: "General" },
];

/**
 * Workspace settings shell: a page title + a section tab strip, with the active
 * section rendered in the Outlet. The h1 is a full page title (text-2xl) so it
 * clearly outranks each section's own heading; each tab's page provides its own
 * section header + actions.
 */
export default function WorkspaceSettings() {
  return (
    <div className="mx-auto max-w-6xl p-6">
      <div className="space-y-1">
        <h1 className="text-2xl font-semibold leading-tight">Workspace settings</h1>
        <p className="text-sm text-muted-foreground">
          Manage members, access, engine connections, and this workspace&rsquo;s configuration.
        </p>
      </div>

      <nav
        className="mb-6 mt-4 flex flex-wrap gap-1 border-b border-border"
        aria-label="Workspace settings sections"
      >
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
