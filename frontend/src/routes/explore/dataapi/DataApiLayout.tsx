import { Activity, KeyRound, type LucideIcon, Plug } from "lucide-react";
import { NavLink, Outlet, useParams } from "react-router-dom";

import { useMyWorkspaces } from "@/api/hooks";

type Section = {
  to: string;
  label: string;
  icon: LucideIcon;
  /** Whether the NavLink requires an exact pathname match to count as active.
   *  Defaults to true (copied from WorkspaceSettings, where every section is a
   *  leaf). Endpoints is the one exception: `endpoints/:id` is a SIBLING leaf
   *  route (App.tsx), not nested under `endpoints`, so with `end: true` the nav
   *  shows nothing active while viewing an endpoint's detail page. */
  end?: boolean;
};

/**
 * Data API shell.
 *
 * Section nav rather than a tab strip because the three areas are of different
 * natures and more will come (spec §2.2) — the same reason Workspace Settings
 * uses one.
 *
 * NOT collapsible, unlike the settings hub. That hub has nine sections in three
 * groups and earns a rail; three flat sections do not, and a second
 * localStorage-backed collapse key would be a second thing to keep in sync for
 * no gain.
 */
const SECTIONS: Section[] = [
  { to: "endpoints", label: "Endpoints", icon: Plug, end: false },
  { to: "clients", label: "Clients", icon: KeyRound },
  { to: "activity", label: "Activity", icon: Activity },
];

const itemActive = "bg-primary/10 font-medium text-primary";
const itemIdle = "text-muted-foreground hover:bg-accent hover:text-foreground";
const focusRing = "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

/** A single section link — icon + label, active state via `aria-current`. */
function NavItem({ item }: { item: Section }) {
  const Icon = item.icon;
  return (
    <NavLink
      to={item.to}
      end={item.end ?? true}
      className={({ isActive }) =>
        `flex items-center gap-2.5 rounded-md px-3 py-2 text-sm transition-colors ${focusRing} ${
          isActive ? itemActive : itemIdle
        }`
      }
    >
      <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
      <span className="truncate">{item.label}</span>
    </NavLink>
  );
}

/**
 * Data API shell: a page title on top, a flat section nav on the left
 * (Endpoints / Clients / Activity), and the active section rendered in the
 * Outlet on the right. The h1 is a full page title (text-2xl) so it clearly
 * outranks each section's own heading. On narrow screens the nav becomes a
 * horizontally scrollable tab strip instead.
 */
export default function DataApiLayout() {
  const { ws: slug } = useParams();
  const { data: workspaces } = useMyWorkspaces(true);
  const ws = workspaces?.find((w) => w.slug === slug);

  return (
    <div className="mx-auto max-w-6xl p-6">
      <header className="mb-6 space-y-1">
        <h1 className="text-2xl font-semibold leading-tight">Data API</h1>
        <p className="text-sm text-muted-foreground">
          {ws?.name
            ? `Publish parameterized queries as REST endpoints for ${ws.name}.`
            : "Publish parameterized queries as REST endpoints."}
        </p>
      </header>

      {/* Mobile: horizontal scrollable tab strip. */}
      <nav
        className="-mx-6 mb-5 flex gap-1 overflow-x-auto border-b border-border px-6 lg:hidden"
        aria-label="Data API sections"
      >
        {SECTIONS.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.end ?? true}
            className={({ isActive }) =>
              `-mb-px shrink-0 border-b-2 px-3 py-2 text-sm transition-colors ${
                isActive
                  ? "border-primary font-semibold text-foreground"
                  : "border-transparent font-medium text-muted-foreground hover:border-border hover:text-foreground"
              }`
            }
          >
            {item.label}
          </NavLink>
        ))}
      </nav>

      <div className="lg:grid lg:grid-cols-[13rem_minmax(0,1fr)] lg:gap-8">
        {/* Desktop: flat vertical section nav. */}
        <nav
          className="hidden self-start lg:sticky lg:top-6 lg:flex lg:flex-col lg:space-y-0.5"
          aria-label="Data API sections"
        >
          {SECTIONS.map((item) => (
            <NavItem key={item.to} item={item} />
          ))}
        </nav>

        <div className="min-w-0">
          <Outlet />
        </div>
      </div>
    </div>
  );
}
