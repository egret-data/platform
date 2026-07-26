import {
  BarChart3,
  LayoutDashboard,
  NotebookText,
  SquareTerminal,
  Table2,
  type LucideIcon,
} from "lucide-react";

/**
 * The five explore modules gated by a workspace's feature entitlements
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
  { key: "notebooks", label: "Notebooks", icon: NotebookText, path: "notebooks" },
];
