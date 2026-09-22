import type { Me } from "@/api/hooks";

/**
 * Who may enter each top-level app, and which app a URL belongs to.
 *
 * The single source for `Header` (which entries to show), `Sidebar` (which
 * nav to render) and the area guards in `guards.tsx`. There used to be two
 * copies of the Control Plane list (Header + guards); both missed `tag:read`,
 * so a tag-only user could not reach Classifications at all.
 *
 * Typed `readonly string[]` rather than `as const`: `includes(p)` on a tuple of
 * literals rejects a plain `string` argument.
 */
export const CONTROL_PLANE_PERMS: readonly string[] = [
  "engine:read",
  "warehouse:read",
  // D3: `catalog_registry:read` gates the Catalogs section. It joins this list
  // in the SAME change that adds the section, and `warehouse:read` only LEAVES
  // it in SP3c-2, once `warehouse_admin` has been re-gated. Between the two
  // releases a user may hold either key and is locked out of nothing.
  "catalog_registry:read",
  "storage:read",
  "secret:read",
  "user:read",
  "group:read",
  "role:read",
  "workspace:read",
];

export const GOVERNANCE_PERMS: readonly string[] = ["policy:read", "tag:read"];

function isAdminOrHoldsAny(me: Me | null | undefined, perms: readonly string[]): boolean {
  if (!me) return false;
  if (me.platform_roles.includes("platform_admin")) return true;
  return me.permissions.some((p) => perms.includes(p));
}

/** platform_admin, or any Control Plane read permission. */
export function canEnterControlPlane(me: Me | null | undefined): boolean {
  return isAdminOrHoldsAny(me, CONTROL_PLANE_PERMS);
}

/** platform_admin, or any Governance read permission. */
export function canEnterGovernance(me: Me | null | undefined): boolean {
  return isAdminOrHoldsAny(me, GOVERNANCE_PERMS);
}

export type AppArea = "explore" | "governance" | "control-plane";

/** Segment-exact: `/governance-reports` is not Governance. */
export function appForPath(pathname: string): AppArea {
  if (pathname === "/governance" || pathname.startsWith("/governance/")) return "governance";
  if (pathname === "/control-plane" || pathname.startsWith("/control-plane/")) return "control-plane";
  return "explore";
}
