/**
 * Access-control (control-plane) API — MIGRATED to the orval-generated client
 * (E0·P4d-3).
 *
 * Thin adapter over `./generated/control-plane-access/control-plane-access`
 * (tag `control-plane-access`), following the P4c/P4d pattern: the plain
 * async helpers keep their old names/signatures so call sites and the
 * module-mocking page tests (`vi.mock("@/api/access")`) don't churn. Each helper
 * calls the generated fetch fn and unwraps orval's `{ status, data, headers }`
 * envelope to the same body the hand-written `apiJson`/`apiFetch` returned.
 *
 * Types are re-exported under the app's existing names. `AccessUser` maps to the
 * generated `ControlPlaneUserDto` (the backend renamed the DTO in P4b-3 to avoid
 * colliding with the workspace `UserDto`). `RoleAssignee` stays FE-local because
 * the spec types its `kind` as a bare `string`; aliasing would widen the FE
 * `"user" | "group"` union and churn call sites.
 */
import {
  controlPlaneAccessAddGroupMember,
  controlPlaneAccessAssignGroupRole,
  controlPlaneAccessAssignUserRole,
  controlPlaneAccessCreateGroup,
  controlPlaneAccessCreateRole,
  controlPlaneAccessCreateUser,
  controlPlaneAccessDeleteGroup,
  controlPlaneAccessDeleteRole,
  controlPlaneAccessGetEffectiveAccess,
  controlPlaneAccessGetRole,
  controlPlaneAccessListGroupMembers,
  controlPlaneAccessListGroups,
  controlPlaneAccessListPermissions,
  controlPlaneAccessListRoleAssignees,
  controlPlaneAccessListRoles,
  controlPlaneAccessListUsers,
  controlPlaneAccessRemoveGroupMember,
  controlPlaneAccessRenameGroup,
  controlPlaneAccessSetRolePermissions,
  controlPlaneAccessUnassignGroupRole,
  controlPlaneAccessUnassignUserRole,
  controlPlaneAccessUpdateRole,
  controlPlaneAccessUpdateUser,
} from "./generated/control-plane-access/control-plane-access";
import type {
  ControlPlaneUserDto,
  EffectiveAccess,
  GroupDto,
  GroupMemberDto,
  PermissionGroupDto,
  RoleDto,
} from "./generated/model";

// ── Access control (control-plane) ───────────────────────────────────────────

// Re-export generated DTOs under the names the app already imports.
export type AccessUser = ControlPlaneUserDto;
export type AccessGroup = GroupDto;
export type GroupMember = GroupMemberDto;
export type AccessRole = RoleDto;
export type PermissionCatalogGroup = PermissionGroupDto;

// Effective-access report (roles/permissions/workspaces/objects/denies for a principal).
export type { EffectiveAccess };

// FE-local: the spec types `kind` as a bare `string`; keep the narrow union.
export interface RoleAssignee {
  kind: "user" | "group";
  name: string;
}

// ── Users ─────────────────────────────────────────────────────────────────

export async function listUsers(): Promise<AccessUser[]> {
  return (await controlPlaneAccessListUsers()).data as AccessUser[];
}

export async function createUser(input: { email: string; name: string; password: string }): Promise<AccessUser> {
  return (await controlPlaneAccessCreateUser(input)).data as AccessUser;
}

export async function updateUser(id: string, input: { name?: string; status?: string }): Promise<AccessUser> {
  return (await controlPlaneAccessUpdateUser(id, input)).data as AccessUser;
}

export async function assignUserRole(id: string, roleId: string): Promise<void> {
  await controlPlaneAccessAssignUserRole(id, { roleId });
}

export async function unassignUserRole(id: string, roleId: string): Promise<void> {
  await controlPlaneAccessUnassignUserRole(id, roleId);
}

// ── Groups ────────────────────────────────────────────────────────────────

export async function listGroups(): Promise<AccessGroup[]> {
  return (await controlPlaneAccessListGroups()).data as AccessGroup[];
}

export async function createGroup(input: { name: string }): Promise<AccessGroup> {
  return (await controlPlaneAccessCreateGroup(input)).data as AccessGroup;
}

export async function renameGroup(id: string, name: string): Promise<AccessGroup> {
  // The endpoint returns 204 (no body); the return value is not consumed by
  // callers (they invalidate on success), so cast through `unknown` to keep the
  // long-standing `Promise<AccessGroup>` signature.
  return (await controlPlaneAccessRenameGroup(id, { name })).data as unknown as AccessGroup;
}

export async function deleteGroup(id: string): Promise<void> {
  await controlPlaneAccessDeleteGroup(id);
}

export async function getGroupMembers(groupId: string): Promise<GroupMember[]> {
  return (await controlPlaneAccessListGroupMembers(groupId)).data as GroupMember[];
}

export async function addMember(groupId: string, userId: string): Promise<void> {
  await controlPlaneAccessAddGroupMember(groupId, { userId });
}

export async function removeMember(groupId: string, userId: string): Promise<void> {
  await controlPlaneAccessRemoveGroupMember(groupId, userId);
}

export async function assignGroupRole(groupId: string, roleId: string): Promise<void> {
  await controlPlaneAccessAssignGroupRole(groupId, { roleId });
}

export async function unassignGroupRole(groupId: string, roleId: string): Promise<void> {
  await controlPlaneAccessUnassignGroupRole(groupId, roleId);
}

// ── Roles ─────────────────────────────────────────────────────────────────

export async function listRoles(): Promise<AccessRole[]> {
  return (await controlPlaneAccessListRoles()).data as AccessRole[];
}

export async function getRole(id: string): Promise<AccessRole> {
  return (await controlPlaneAccessGetRole(id)).data as AccessRole;
}

export async function createRole(input: {
  scope: string;
  name: string;
  description: string;
  parentRoleId?: string | null;
}): Promise<AccessRole> {
  return (await controlPlaneAccessCreateRole(input)).data as AccessRole;
}

export async function updateRole(
  id: string,
  description: string,
  parentRoleId?: string | null,
): Promise<AccessRole> {
  return (await controlPlaneAccessUpdateRole(id, { description, parentRoleId })).data as AccessRole;
}

export async function deleteRole(id: string): Promise<void> {
  await controlPlaneAccessDeleteRole(id);
}

export async function setRolePermissions(id: string, permissions: string[]): Promise<void> {
  await controlPlaneAccessSetRolePermissions(id, { permissions });
}

export async function getRoleAssignees(roleId: string): Promise<RoleAssignee[]> {
  return (await controlPlaneAccessListRoleAssignees(roleId)).data as RoleAssignee[];
}

// ── Permission catalog ──────────────────────────────────────────────────────

export async function listPermissions(): Promise<PermissionCatalogGroup[]> {
  return (await controlPlaneAccessListPermissions()).data as PermissionCatalogGroup[];
}

// ── Effective access ──────────────────────────────────────────────────────────

export async function getEffectiveAccess(
  principalType: "user" | "group",
  principalId: string,
): Promise<EffectiveAccess> {
  return (
    await controlPlaneAccessGetEffectiveAccess({
      principal_type: principalType,
      principal_id: principalId,
    })
  ).data as EffectiveAccess;
}
