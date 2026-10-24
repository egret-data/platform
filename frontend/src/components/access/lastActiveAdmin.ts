import type { AccessUser } from "@/api/access";

/**
 * Why disabling `user` would be refused as the last active platform admin, read
 * from the users list, or `null`. It sees direct roles only (`roles` on the
 * DTO): an admin through a group is not counted, so it can be wrong either way
 * when group admins exist, and the server's 409 stays the final word.
 */
export function lastActiveAdminReason(user: AccessUser | null, users: AccessUser[]): string | null {
  const activeAdmin = (u: AccessUser) => u.status === "active" && u.roles.includes("platform_admin");
  if (!user || !activeAdmin(user)) return null;
  return users.some((u) => u.id !== user.id && activeAdmin(u))
    ? null
    : "No other active user holds platform_admin directly, and the last active platform admin cannot be disabled.";
}
