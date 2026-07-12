import { NavLink, Outlet, useParams } from "react-router-dom";
import { Users } from "lucide-react";

const TABS = [{ to: "members", label: "Members", icon: Users }];

export default function WorkspaceSettings() {
  const ws = useParams().workspace!;
  return (
    <div className="p-4">
      <h1 className="mb-4 text-lg font-semibold">Workspace settings</h1>
      <div className="grid grid-cols-[180px_1fr] gap-6">
        <nav className="flex flex-col gap-1">
          {TABS.map((t) => (
            <NavLink
              key={t.to}
              to={`/explore/${ws}/settings/${t.to}`}
              className={({ isActive }) =>
                `flex items-center gap-2 rounded-md px-3 py-2 text-sm font-medium ${isActive ? "bg-muted text-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground"}`
              }
            >
              <t.icon className="h-4 w-4" /> {t.label}
            </NavLink>
          ))}
        </nav>
        <div className="min-w-0"><Outlet /></div>
      </div>
    </div>
  );
}
