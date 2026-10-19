import { compilerOwnedConflict } from "@/api/policies";
import type { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";

/**
 * The policy surface's write vocabulary: who may write, and what a refused
 * write says. Shared by every page of Engine & policy (`routes/governance/engines/`)
 * because they are ONE surface with one backend — SP4b-2 finding 3 was fixed
 * on one page first and another kept the same defect for a whole review
 * round, which is what a second copy of this reasoning buys.
 *
 * **Every control names the key ITS OWN route requires.** These are five
 * different permissions on `crates/egret-policy/src/admin/mod.rs`, not one
 * "can write" flag:
 *
 * | control | route | permission |
 * |---|---|---|
 * | New profile / Create profile | `POST /policies/services` | `policy:create` |
 * | Delete profile | `DELETE /policies/services/{service}` | `policy:delete` |
 * | Seed default policies | `POST …/seed-defaults` | `policy:seed` |
 * | New policy | `POST /policies` | `policy:create` |
 * | Enabled switch, editor Save | `PUT /policies/{id}`, `…/enabled` | `policy:update` |
 * | Delete policy | `DELETE /policies/{id}` | `policy:delete` |
 *
 * Reads (`Download config`, the lists, a policy's body) are `policy:read`,
 * which is what reaches these pages at all, so they are never gated here.
 */
export function usePolicyPermissions(): (permission: string) => boolean {
  const { me } = useAuth();
  const isAdmin = !!me && me.platform_roles.includes("platform_admin");
  // `platform_admin` bypasses inside the PDP, so it passes every key.
  return (permission: string) => isAdmin || !!me?.permissions.includes(permission);
}

/**
 * What a refused write says.
 *
 * A 403's body is the bare word `forbidden` (`PolicyHttpError`/`PermHttpError`
 * both render `{"error":"forbidden"}`), which tells an operator nothing about
 * what to ask for — so the permission is named instead. A compiler-owned 409
 * keeps its own explanation (D10). Anything else passes through unchanged.
 */
export function refusalMessage(e: unknown, needs: string, fallback: string): string {
  const conflict = compilerOwnedConflict(e);
  if (conflict) return conflict;
  const err = e as ApiError;
  if (err?.status === 403) {
    return `This account cannot do that: it needs the ${needs} permission.`;
  }
  return err?.message ?? fallback;
}
