import { useParams } from "react-router-dom";
import { useMyWorkspaces } from "@/api/hooks";
import { EXPLORE_MODULES } from "@/lib/exploreModules";

/**
 * Every workspace feature key that exists, used only to build the "still
 * loading" optimistic fallback below — NOT the same list `useDefaultExplorePath`
 * searches for a landing page (that one deliberately stays scoped to
 * `EXPLORE_MODULES`, so a non-module item can never become the default
 * explore path just by gaining a `feature` key here).
 *
 * The six `EXPLORE_MODULES` keys cover the Explore group; `data_api` is gated
 * the same way but on its own Sidebar group (`Data API`), not a module, so it
 * isn't part of `EXPLORE_MODULES` itself. `workflows` is a feature entitlement
 * key with no nav gate yet — the group that will read it lands in a later
 * package — but it still belongs here because this list has to name every key
 * the backend can return, not just the ones currently wired into the nav.
 * `apps` (Data Apps) is the same: a backend entitlement whose UI lands later.
 */
export const ALL_FEATURE_KEYS: string[] = [...EXPLORE_MODULES.map((m) => m.key), "data_api", "workflows", "apps"];

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

/**
 * The route to send a user to when they land on the workspace root (or an
 * unknown path): the first enabled module in nav order (per `EXPLORE_MODULES`
 * — the same order Sidebar renders), or Workspace settings → Members when
 * every module is switched off (so a workspace with e.g. SQL Lab disabled
 * doesn't redirect straight into a page `RequireFeature` will immediately
 * bounce off).
 */
export function useDefaultExplorePath(): string {
  const enabled = useWorkspaceFeatures();
  const first = EXPLORE_MODULES.find((m) => enabled.includes(m.key));
  return first ? first.path : "settings/members";
}

/**
 * Absolute target for the explore index and catch-all redirects.
 *
 * ABSOLUTE, and that is the whole point. These redirects used to pass
 * `useDefaultExplorePath()` straight to `<Navigate>`, which is relative. Under
 * react-router 6 a relative target inside the `path="*"` route resolved against the
 * splat route's PATTERN (`/explore/:workspace/`) and landed correctly. react-router 7
 * resolves it against the matched LOCATION, so `/explore/ws/unknown` redirected to
 * `/explore/ws/unknown/sql-lab`, which matches `*` again — the path grew a segment per
 * pass until the browser gave up. Building the target from the workspace makes the
 * resolution rule irrelevant.
 */
export function exploreRedirectTo(workspace: string | undefined, defaultPath: string): string {
  return `/explore/${workspace ?? ""}/${defaultPath}`;
}
