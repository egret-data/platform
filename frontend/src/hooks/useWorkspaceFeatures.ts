import { useParams } from "react-router-dom";
import { useMyWorkspaces } from "@/api/hooks";

/** The five modules a workspace can be entitled to, in nav order. */
export const ALL_FEATURE_KEYS = ["sql_lab", "datasets", "charts", "dashboards", "notebooks"] as const;
export type FeatureKey = (typeof ALL_FEATURE_KEYS)[number];

/**
 * The module keys the *current route's* workspace has switched on — the
 * client-side read that drives both the Sidebar's item filtering and
 * `RequireFeature`'s route gate.
 *
 * This is deliberately an OPTIMISTIC read, not the real gate: the backend's
 * `WsContext` extractor enforces entitlements on every request regardless of
 * what this returns, so it is safe for the client to be briefly wrong here.
 * Two cases where it can be wrong, both handled elsewhere:
 *   - stale-open (a module got switched off while a page reading this was
 *     already mounted) — the eventual 403 `feature_disabled` from the API
 *     invalidates `my-workspaces` (see `client.ts`), which flows back through
 *     here.
 *   - still loading (the `/me/workspaces` fetch hasn't resolved yet on first
 *     paint) — returning every key here, rather than none, is what stops the
 *     Sidebar and route guards from flashing items/Forbidden away and back.
 */
export function useWorkspaceFeatures(): string[] {
  const { workspace } = useParams();
  const { data } = useMyWorkspaces(true);
  if (data === undefined) return [...ALL_FEATURE_KEYS];
  return data.find((w) => w.slug === workspace)?.features ?? [];
}

/** `path` within `/explore/:workspace/` for each feature, in nav order. */
const FEATURE_PATHS: { feature: FeatureKey; path: string }[] = [
  { feature: "sql_lab", path: "sql-lab" },
  { feature: "datasets", path: "datasets" },
  { feature: "charts", path: "charts" },
  { feature: "dashboards", path: "dashboards" },
  { feature: "notebooks", path: "notebooks" },
];

/**
 * The route to send a user to when they land on the workspace root (or an
 * unknown path): the first enabled module in nav order, or Workspace settings
 * → Members when every module is switched off (so a workspace with e.g. SQL
 * Lab disabled doesn't redirect straight into a page `RequireFeature` will
 * immediately bounce off).
 */
export function useDefaultExplorePath(): string {
  const enabled = useWorkspaceFeatures();
  const first = FEATURE_PATHS.find((f) => enabled.includes(f.feature));
  return first ? first.path : "settings/members";
}
