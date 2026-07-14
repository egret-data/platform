import { Database, Inbox, type LucideIcon, Settings2, Share2, ShieldCheck, Users } from "lucide-react";
import { NavLink, Outlet, useParams } from "react-router-dom";

import { useMyWorkspaces } from "@/api/hooks";

type SettingsItem = { to: string; label: string; icon: LucideIcon };
type SettingsGroup = { heading: string; items: SettingsItem[] };

/**
 * Workspace settings sections, grouped by mental model rather than listed flat:
 * "Access" (who's in the workspace and what they can do) is the day-to-day work,
 * "Connections" is infrastructure wiring, "Workspace" is the identity of the
 * space itself. Grouping keeps the six sections scannable and leaves obvious
 * homes for future settings.
 */
const GROUPS: SettingsGroup[] = [
  {
    heading: "Access",
    items: [
      { to: "members", label: "Members", icon: Users },
      { to: "roles", label: "Roles & permissions", icon: ShieldCheck },
      { to: "requests", label: "Access requests", icon: Inbox },
      { to: "shares", label: "Shared objects", icon: Share2 },
    ],
  },
  {
    heading: "Connections",
    items: [{ to: "engines", label: "Engines", icon: Database }],
  },
  {
    heading: "Workspace",
    items: [{ to: "general", label: "General", icon: Settings2 }],
  },
];

const ALL_ITEMS = GROUPS.flatMap((g) => g.items);

const itemBase =
  "flex items-center gap-2.5 rounded-md px-3 py-2 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
const itemActive = "bg-primary/10 font-medium text-primary";
const itemIdle = "text-muted-foreground hover:bg-accent hover:text-foreground";

/**
 * Workspace settings shell.
 *
 * A settings-hub layout: a page title on top, then a grouped section nav on the
 * left with the active section rendered in the Outlet on the right. The h1 is a
 * full page title (text-2xl) so it clearly outranks each section's own heading.
 * On narrow screens the left nav collapses into a horizontally scrollable tab
 * strip so the content still gets the full width.
 */
export default function WorkspaceSettings() {
  const { workspace: slug } = useParams();
  const { data: workspaces } = useMyWorkspaces(true);
  const ws = workspaces?.find((w) => w.slug === slug);

  return (
    <div className="mx-auto max-w-6xl px-6 py-6">
      <header className="mb-6 space-y-1">
        <h1 className="text-2xl font-semibold leading-tight">Workspace settings</h1>
        <p className="text-sm text-muted-foreground">
          {ws?.name
            ? `Manage members, access, and configuration for ${ws.name}.`
            : "Manage members, access, and this workspace’s configuration."}
        </p>
      </header>

      {/* Mobile: horizontal scrollable tab strip (group headings dropped — the
          content pane needs the width more than the grouping does here). */}
      <nav
        className="-mx-6 mb-5 flex gap-1 overflow-x-auto border-b border-border px-6 lg:hidden"
        aria-label="Workspace settings sections"
      >
        {ALL_ITEMS.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end
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
        {/* Desktop: grouped vertical settings nav. */}
        <nav
          className="hidden self-start lg:sticky lg:top-6 lg:block"
          aria-label="Workspace settings sections"
        >
          {GROUPS.map((group) => (
            <div key={group.heading} className="mb-5 last:mb-0">
              <p className="mb-1 px-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">
                {group.heading}
              </p>
              <div className="space-y-0.5">
                {group.items.map((item) => {
                  const Icon = item.icon;
                  return (
                    <NavLink
                      key={item.to}
                      to={item.to}
                      end
                      className={({ isActive }) =>
                        `${itemBase} ${isActive ? itemActive : itemIdle}`
                      }
                    >
                      <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
                      <span className="truncate">{item.label}</span>
                    </NavLink>
                  );
                })}
              </div>
            </div>
          ))}
        </nav>

        <div className="min-w-0">
          <Outlet />
        </div>
      </div>
    </div>
  );
}
