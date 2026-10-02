import {
  BarChart3,
  Bell,
  AppWindow,
  LayoutDashboard,
  SquareTerminal,
  Table2,
  type LucideIcon,
} from "lucide-react";

/**
 * The explore modules gated by a workspace's feature entitlements
 * (`FeatureDto.key` on the backend), in nav order.
 *
 * This is the SINGLE source of truth for that order plus each module's label,
 * icon, and route: `Sidebar.tsx` (nav items) and `useDefaultExplorePath` (the
 * index/catch-all redirect) both read this same array, so renaming,
 * reordering, or re-routing a module can't silently diverge between "what the
 * nav shows" and "where an empty workspace lands" — they're the same order by
 * construction, not by two lists happening to agree today.
 */
export interface ExploreModule {
  /** The workspace feature entitlement key (matches the backend's `FeatureDto.key`). */
  key: string;
  label: string;
  icon: LucideIcon;
  /** Path segment under `/explore/:workspace/`. */
  path: string;
}

export const EXPLORE_MODULES: ExploreModule[] = [
  { key: "sql_lab", label: "SQL Lab", icon: SquareTerminal, path: "sql-lab" },
  { key: "datasets", label: "Datasets", icon: Table2, path: "datasets" },
  { key: "charts", label: "Charts", icon: BarChart3, path: "charts" },
  { key: "dashboards", label: "Dashboards", icon: LayoutDashboard, path: "dashboards" },
  // Data Apps replace the Jupyter notebook in this menu. The notebook routes still exist until the
  // notebook is removed, but nothing links to them.
  { key: "apps", label: "Apps", icon: AppWindow, path: "apps" },
  // Last in nav order deliberately: Alerts & Reports acts ON the modules above
  // (it queries data and mails charts), so it reads as their consumer — and
  // being last means it can never displace SQL Lab as the default landing page.
  { key: "alerts", label: "Alerts & Reports", icon: Bell, path: "alerts" },
];
