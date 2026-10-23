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
 * The `EXPLORE_MODULES` keys cover the Explore group (Data Apps' `apps` among them); `data_api`
 * and `workflows` are gated the same way but each on its own Sidebar group (`Data API`,
 * `Workflows`), not a module, so neither is part of `EXPLORE_MODULES` itself — which also keeps
 * them out of the landing-page choice in `useDefaultExplorePath`. Both still belong here because
 * this list has to name every key the backend can return.
 */
export const ALL_FEATURE_KEYS: string[] = [...EXPLORE_MODULES.map((m) => m.key), "data_api", "workflows"];

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
 * — the same order Sidebar renders), or Workspace settings when
 * every module is switched off (so a workspace with e.g. SQL Lab disabled
 * doesn't redirect straight into a page `RequireFeature` will immediately
 * bounce off).
 */
export function useDefaultExplorePath(): string {
  const enabled = useWorkspaceFeatures();
  const first = EXPLORE_MODULES.find((m) => enabled.includes(m.key));
  // `settings`, not one section: its index opens the first section this
  // reader may open (`SettingsIndex`).
  return first ? first.path : "settings";
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

/**
 * Whether the caller holds `perm` in the current route's workspace, for
 * leaving out a sidebar or settings entry they cannot open (KI-231).
 *
 * The answer comes from `/me/workspaces` → `permissions`, which the server
 * computes with the same PDP decision the page's routes ask; the routes stay
 * the gate. Like `useWorkspaceFeatures` this is OPTIMISTIC: while the list is
 * loading, for a workspace not in it, or from a server that does not send
 * `permissions`, every permission reads as held, so an entry is only ever
 * hidden on the server's own word that it would refuse. A `perm` of
 * `undefined` (an entry with no gate) is always held.
 */
export function useWorkspacePermission(): (perm: string | undefined) => boolean {
  const { workspace } = useParams();
  const { data } = useMyWorkspaces(true);
  const held = data?.find((w) => w.slug === workspace)?.permissions;
  return (perm) => perm === undefined || held === undefined || held.includes(perm);
}
