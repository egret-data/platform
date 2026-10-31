import type { AccessUser } from "@/api/access";
import { holdsPlatformAdmin } from "@/components/access/effectiveRoles";

/**
 * Why disabling `user` would be refused as the last active platform admin, read
 * from the users list, or `null`. It counts platform_admin held directly or
 * through a group (`effectiveRoles`), as the server's guard does; the server's
 * 409 stays the final word.
 */
export function lastActiveAdminReason(user: AccessUser | null, users: AccessUser[]): string | null {
  const activeAdmin = (u: AccessUser) => u.status === "active" && holdsPlatformAdmin(u);
  if (!user || !activeAdmin(user)) return null;
  return users.some((u) => u.id !== user.id && activeAdmin(u))
    ? null
    : "No other active user holds platform_admin, and the last active platform admin cannot be disabled.";
}
