import { ApiError } from "@/api/client";
import type { GroupPrincipal, Membership, RolePrincipal, UserPrincipal } from "@/api/generated/model";
import type { WorkspaceNames } from "@/routes/governance/permFormat";
import { parseUuid } from "@/routes/governance/people/reach";
import { usePermReadGroup, usePermReadRole, usePermReadUser } from "@/api/generated/perm/perm";

export type PageKind = "user" | "group" | "role";

export type Principal =
  | { kind: "user"; data: UserPrincipal }
  | { kind: "group"; data: GroupPrincipal }
  | { kind: "role"; data: RolePrincipal };

export type PrincipalState =
  | { status: "loading" }
  | { status: "missing" }
  | { status: "error"; retry: () => void }
  | { status: "ok"; principal: Principal };

/**
 * The principal a People page shows, read from the server
 * (`GET /perm/people/{users,groups,roles}/…`). Only the kind's own read runs.
 * A 404 is `missing`: the server has no user with this id, or no group or role
 * with this name.
 */
export function usePrincipal(kind: PageKind, key: string): PrincipalState {
  const opts = (k: PageKind) => ({ query: { enabled: kind === k } });
  const user = usePermReadUser(key, opts("user"));
  const group = usePermReadGroup(key, opts("group"));
  const role = usePermReadRole(key, opts("role"));
  const q = kind === "user" ? user : kind === "group" ? group : role;
  if (q.isError) {
    if (q.error instanceof ApiError && q.error.status === 404) return { status: "missing" };
    return { status: "error", retry: () => void q.refetch() };
  }
  if (kind === "user" && user.data?.status === 200) return { status: "ok", principal: { kind, data: user.data.data } };
  if (kind === "group" && group.data?.status === 200) return { status: "ok", principal: { kind, data: group.data.data } };
  if (kind === "role" && role.data?.status === 200) return { status: "ok", principal: { kind, data: role.data.data } };
  return { status: "loading" };
}

/** What a page calls the principal: a user's name (else email), a group's or role's name. */
export function principalTitle(principal: Principal | null, key: string): string {
  if (principal?.kind === "user") return principal.data.name || principal.data.email || key;
  return key;
}

/**
 * The workspaces' names the principal's own read carries (its `standsFor`
 * and memberships), by id, compared as uuids as `reach.ts` compares
 * `workspace_members` refs. Another workspace reads by its id.
 */
export function workspaceNamesOf(principal: Principal): WorkspaceNames {
  const names = new Map<string, string>();
  const add = (id: string, name: string) => {
    const key = parseUuid(id);
    if (key) names.set(key, name);
  };
  for (const s of principal.data.standsFor) {
    if (s.via.kind === "workspace" || s.via.kind === "group_workspace") add(s.via.id, s.via.name);
  }
  const memberships: Membership[] = principal.kind === "role" ? [] : principal.data.workspaces;
  for (const m of memberships) if (m.scope?.type === "workspace") add(m.scope.id, m.scope.name);
  return (id) => {
    const key = parseUuid(id);
    return key ? names.get(key) : undefined;
  };
}
