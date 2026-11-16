/**
 * Service accounts + credentials API (SA3) — thin adapter over the generated
 * client (`./generated/control-plane-access/control-plane-access`, tag
 * `control-plane-access`), following the `policies.ts` pattern: plain async
 * helpers call the generated fetch fn and unwrap orval's `{ status, data,
 * headers }` envelope, returning `.data`.
 */
import {
  controlPlaneAccessAddServiceAccountGroup,
  controlPlaneAccessAddServiceAccountRole,
  controlPlaneAccessCreateCredential,
  controlPlaneAccessCreateServiceAccount,
  controlPlaneAccessDeleteServiceAccount,
  controlPlaneAccessDisableServiceAccount,
  controlPlaneAccessEnableServiceAccount,
  controlPlaneAccessGetServiceAccount,
  controlPlaneAccessListCredentials,
  controlPlaneAccessListServiceAccounts,
  controlPlaneAccessRemoveServiceAccountGroup,
  controlPlaneAccessRemoveServiceAccountRole,
  controlPlaneAccessRevokeCredential,
  controlPlaneAccessRotateCredential,
  controlPlaneAccessSetCredentialExpiry,
  getControlPlaneAccessGetServiceAccountQueryKey,
  getControlPlaneAccessListCredentialsQueryKey,
  getControlPlaneAccessListServiceAccountsQueryKey,
} from "./generated/control-plane-access/control-plane-access";
import type {
  AddGroupBody,
  AddRoleBody,
  ControlPlaneAccessListServiceAccountsParams,
  CreateCredentialBody,
  CreateServiceAccountBody,
  SetExpiryBody,
} from "./generated/model";

export type {
  AddGroupBody,
  AddRoleBody,
  Audience,
  CatalogBinding,
  ControlPlaneAccessListServiceAccountsParams,
  CreateCredentialBody,
  CreatedCredential,
  CreateServiceAccountBody,
  Credential,
  CredentialStatus,
  ServiceAccountDetailDto,
  ServiceAccountDto,
  SetExpiryBody,
} from "./generated/model";

// ── Query keys ───────────────────────────────────────────────────────────────

export const serviceAccountsQueryKey = (
  params?: ControlPlaneAccessListServiceAccountsParams,
) => getControlPlaneAccessListServiceAccountsQueryKey(params);
export const serviceAccountQueryKey = (id: string) =>
  getControlPlaneAccessGetServiceAccountQueryKey(id);
export const credentialsQueryKey = (id: string) =>
  getControlPlaneAccessListCredentialsQueryKey(id);

// ── Service accounts ─────────────────────────────────────────────────────────

export async function listServiceAccounts(
  params?: ControlPlaneAccessListServiceAccountsParams,
) {
  return (await controlPlaneAccessListServiceAccounts(params)).data;
}

export async function getServiceAccount(id: string) {
  return (await controlPlaneAccessGetServiceAccount(id)).data;
}

export async function createServiceAccount(body: CreateServiceAccountBody) {
  return (await controlPlaneAccessCreateServiceAccount(body)).data;
}

export async function disableServiceAccount(id: string) {
  return (await controlPlaneAccessDisableServiceAccount(id)).data;
}

export async function enableServiceAccount(id: string) {
  return (await controlPlaneAccessEnableServiceAccount(id)).data;
}

export async function deleteServiceAccount(id: string) {
  return (await controlPlaneAccessDeleteServiceAccount(id)).data;
}

export async function addServiceAccountGroup(id: string, body: AddGroupBody) {
  return (await controlPlaneAccessAddServiceAccountGroup(id, body)).data;
}

export async function removeServiceAccountGroup(id: string, groupId: string) {
  return (await controlPlaneAccessRemoveServiceAccountGroup(id, groupId)).data;
}

export async function addServiceAccountRole(id: string, body: AddRoleBody) {
  return (await controlPlaneAccessAddServiceAccountRole(id, body)).data;
}

export async function removeServiceAccountRole(id: string, roleId: string) {
  return (await controlPlaneAccessRemoveServiceAccountRole(id, roleId)).data;
}

// ── Credentials ──────────────────────────────────────────────────────────────

export async function listCredentials(id: string) {
  return (await controlPlaneAccessListCredentials(id)).data;
}

export async function createCredential(id: string, body: CreateCredentialBody) {
  return (await controlPlaneAccessCreateCredential(id, body)).data;
}

export async function rotateCredential(id: string, cid: string) {
  return (await controlPlaneAccessRotateCredential(id, cid)).data;
}

export async function revokeCredential(id: string, cid: string) {
  return (await controlPlaneAccessRevokeCredential(id, cid)).data;
}

export async function setCredentialExpiry(
  id: string,
  cid: string,
  body: SetExpiryBody,
) {
  return (await controlPlaneAccessSetCredentialExpiry(id, cid, body)).data;
}
