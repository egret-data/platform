import type { AccessUser } from "@/api/access";

/** One platform role a user holds, and every source it comes from. */
export interface RoleHolding {
  name: string;
  /** Granted to the user directly (removable on the user's page). */
  direct: boolean;
  /** The groups it is held through, in the server's order. */
  groups: string[];
}

/**
 * The platform roles `user` holds, directly or through a group (KI-260), one
 * entry per role name, in the server's order (by name). Read from
 * `effectiveRoles`, which the server computes: the frontend never joins groups
 * to roles itself.
 */
export function roleHoldings(user: AccessUser): RoleHolding[] {
  const byName = new Map<string, RoleHolding>();
  for (const r of user.effectiveRoles) {
    const h = byName.get(r.name) ?? { name: r.name, direct: false, groups: [] };
    if (r.group == null) h.direct = true;
    else h.groups.push(r.group);
    byName.set(r.name, h);
  }
  return [...byName.values()];
}

/** Where a role comes from, as a phrase: "direct", "via group a", "direct and via groups a, b". */
export function holdingSource(h: RoleHolding): string {
  const via =
    h.groups.length === 0 ? "" : `via ${h.groups.length === 1 ? "group" : "groups"} ${h.groups.join(", ")}`;
  if (h.direct) return via ? `direct and ${via}` : "direct";
  return via;
}

/** Whether `user` holds platform_admin, directly or through a group. */
export function holdsPlatformAdmin(user: AccessUser): boolean {
  return user.effectiveRoles.some((r) => r.name === "platform_admin");
}
