import { apiFetch, apiJson } from "./client";

// ── Access control (control-plane) ───────────────────────────────────────────

export interface AccessUser {
  id: string;
  email: string;
  name: string;
  provider: string;
  status: string;
  roles: string[];
  groups: string[];
}

export interface AccessGroup {
  id: string;
  name: string;
  source: string;
  memberCount: number;
  roles: string[];
}

export interface GroupMember {
  id: string;
  email: string;
  name: string;
}

export interface AccessRole {
  id: string;
  scope: string;
  name: string;
  description: string;
  permissionCount: number;
  assigneeCount: number;
}

export interface RoleAssignee {
  kind: "user" | "group";
  name: string;
}

export interface PermissionCatalogGroup {
  group: string;
  permissions: { key: string; description: string; roles: string[] }[];
}

// ── Users ─────────────────────────────────────────────────────────────────

export function listUsers(): Promise<AccessUser[]> {
  return apiJson<AccessUser[]>("/control-plane/users");
}

export function createUser(input: { email: string; name: string; password: string }): Promise<AccessUser> {
  return apiJson<AccessUser>("/control-plane/users", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
  });
}

export function updateUser(id: string, input: { name?: string; status?: string }): Promise<AccessUser> {
  return apiJson<AccessUser>(`/control-plane/users/${encodeURIComponent(id)}`, {
    method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
  });
}

export function assignUserRole(id: string, roleId: string): Promise<void> {
  return apiFetch(`/control-plane/users/${encodeURIComponent(id)}/roles`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ roleId }),
  }).then(() => undefined);
}

export function unassignUserRole(id: string, roleId: string): Promise<void> {
  return apiFetch(`/control-plane/users/${encodeURIComponent(id)}/roles/${encodeURIComponent(roleId)}`, {
    method: "DELETE",
  }).then(() => undefined);
}

// ── Groups ────────────────────────────────────────────────────────────────

export function listGroups(): Promise<AccessGroup[]> {
  return apiJson<AccessGroup[]>("/control-plane/groups");
}

export function createGroup(input: { name: string }): Promise<AccessGroup> {
  return apiJson<AccessGroup>("/control-plane/groups", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
  });
}

export function renameGroup(id: string, name: string): Promise<AccessGroup> {
  return apiJson<AccessGroup>(`/control-plane/groups/${encodeURIComponent(id)}`, {
    method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ name }),
  });
}

export function deleteGroup(id: string): Promise<void> {
  return apiFetch(`/control-plane/groups/${encodeURIComponent(id)}`, { method: "DELETE" }).then(() => undefined);
}

export function getGroupMembers(groupId: string): Promise<GroupMember[]> {
  return apiJson<GroupMember[]>(`/control-plane/groups/${encodeURIComponent(groupId)}/members`);
}

export function addMember(groupId: string, userId: string): Promise<void> {
  return apiFetch(`/control-plane/groups/${encodeURIComponent(groupId)}/members`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ userId }),
  }).then(() => undefined);
}

export function removeMember(groupId: string, userId: string): Promise<void> {
  return apiFetch(
    `/control-plane/groups/${encodeURIComponent(groupId)}/members/${encodeURIComponent(userId)}`,
    { method: "DELETE" },
  ).then(() => undefined);
}

export function assignGroupRole(groupId: string, roleId: string): Promise<void> {
  return apiFetch(`/control-plane/groups/${encodeURIComponent(groupId)}/roles`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ roleId }),
  }).then(() => undefined);
}

export function unassignGroupRole(groupId: string, roleId: string): Promise<void> {
  return apiFetch(
    `/control-plane/groups/${encodeURIComponent(groupId)}/roles/${encodeURIComponent(roleId)}`,
    { method: "DELETE" },
  ).then(() => undefined);
}

// ── Roles ─────────────────────────────────────────────────────────────────

export function listRoles(): Promise<AccessRole[]> {
  return apiJson<AccessRole[]>("/control-plane/access/roles");
}

export function createRole(input: { scope: string; name: string; description: string }): Promise<AccessRole> {
  return apiJson<AccessRole>("/control-plane/access/roles", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
  });
}

export function updateRole(id: string, description: string): Promise<AccessRole> {
  return apiJson<AccessRole>(`/control-plane/access/roles/${encodeURIComponent(id)}`, {
    method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ description }),
  });
}

export function deleteRole(id: string): Promise<void> {
  return apiFetch(`/control-plane/access/roles/${encodeURIComponent(id)}`, { method: "DELETE" }).then(() => undefined);
}

export function setRolePermissions(id: string, permissions: string[]): Promise<void> {
  return apiFetch(`/control-plane/access/roles/${encodeURIComponent(id)}/permissions`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ permissions }),
  }).then(() => undefined);
}

export function getRoleAssignees(roleId: string): Promise<RoleAssignee[]> {
  return apiJson<RoleAssignee[]>(`/control-plane/access/roles/${encodeURIComponent(roleId)}/assignees`);
}

// ── Permission catalog ──────────────────────────────────────────────────────

export function listPermissions(): Promise<PermissionCatalogGroup[]> {
  return apiJson<PermissionCatalogGroup[]>("/control-plane/permissions");
}
