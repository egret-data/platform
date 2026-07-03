import { apiFetch, apiJson } from "./client";

// ── Storage (global / control-plane) ─────────────────────────────────────────

/** A global storage connection. `secret_ref` is the id of a global named secret. */
export interface Storage {
  id: string;
  name: string;
  purpose: string | null;
  provider: string;
  endpoint: string | null;
  region: string;
  bucket: string;
  key_prefix: string | null;
  access_key: string;
  path_style: boolean;
  secret_ref: string | null;
  created_at: string;
}

export interface StorageInput {
  name: string;
  purpose?: string | null;
  provider: string;
  endpoint?: string | null;
  region: string;
  bucket: string;
  key_prefix?: string | null;
  access_key: string;
  path_style: boolean;
  // Reference an existing global secret, or mint a new one inline. A `new_secret`
  // takes precedence over `secret_ref` on the backend.
  secret_ref?: string;
  new_secret?: { name: string; value: string };
}

export function listStorage(): Promise<Storage[]> {
  return apiJson<Storage[]>("/control-plane/storage");
}

export function getStorage(id: string): Promise<Storage> {
  return apiJson<Storage>(`/control-plane/storage/${id}`);
}

export function createStorage(input: StorageInput): Promise<Storage> {
  return apiJson<Storage>("/control-plane/storage", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export function updateStorage(id: string, input: StorageInput): Promise<Storage> {
  return apiJson<Storage>(`/control-plane/storage/${id}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function deleteStorage(id: string): Promise<void> {
  await apiFetch(`/control-plane/storage/${id}`, { method: "DELETE" });
}

// ── Secrets (global / control-plane) ─────────────────────────────────────────

/** A global named secret. Never carries the plaintext value. */
export interface Secret {
  id: string;
  name: string;
  description: string | null;
  created_at: string;
  // Number of connections referencing this secret. 0 = safe to delete.
  used_by: number;
}

export interface SecretInput {
  name: string;
  description?: string | null;
  // Write-only — never returned by the API.
  value: string;
}

export interface SecretUpdate {
  name?: string;
  description?: string | null;
  // Write-only. Omit/blank to keep the existing value; a value rotates it.
  value?: string;
}

export function listSecrets(): Promise<Secret[]> {
  return apiJson<Secret[]>("/control-plane/secrets");
}

export function createSecret(input: SecretInput): Promise<Secret> {
  return apiJson<Secret>("/control-plane/secrets", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export function updateSecret(id: string, input: SecretUpdate): Promise<Secret> {
  return apiJson<Secret>(`/control-plane/secrets/${id}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function deleteSecret(id: string): Promise<void> {
  await apiFetch(`/control-plane/secrets/${id}`, { method: "DELETE" });
}

// ── Warehouses (embedded catalog / control-plane) ────────────────────────────

/** A catalog warehouse (per workspace/domain). */
export interface Warehouse {
  id: string;
  name: string;
  workspaceId: string;
  storageConnId: string;
  lakekeeperWarehouseId: string | null;
}

export interface CreateWarehouseInput {
  name: string;
  workspaceId: string;
  storageConnectionId: string;
}

export function listWarehouses(): Promise<Warehouse[]> {
  return apiJson<Warehouse[]>("/control-plane/warehouses");
}

export function getWarehouse(id: string): Promise<Warehouse> {
  return apiJson<Warehouse>(`/control-plane/warehouses/${id}`);
}

export function createWarehouse(input: CreateWarehouseInput): Promise<Warehouse> {
  return apiJson<Warehouse>("/control-plane/warehouses", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function deleteWarehouse(id: string): Promise<void> {
  await apiFetch(`/control-plane/warehouses/${id}`, { method: "DELETE" });
}

// ── Catalog roles (keyed by consumer-type) ───────────────────────────────────

export interface CatalogRole {
  id: string;
  name: string;
  consumerType: string;
}

export interface CreateRoleInput {
  name: string;
  consumerType: string;
}

export function listRoles(warehouseId: string): Promise<CatalogRole[]> {
  return apiJson<CatalogRole[]>(`/control-plane/warehouses/${warehouseId}/roles`);
}

export function createRole(
  warehouseId: string,
  input: CreateRoleInput,
): Promise<CatalogRole> {
  return apiJson<CatalogRole>(`/control-plane/warehouses/${warehouseId}/roles`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

// ── Grants (on a catalog role) ───────────────────────────────────────────────

export interface Grant {
  id: string;
  securableType: string;
  namespace: string[] | null;
  tableName: string | null;
  privilege: string;
}

export interface GrantInput {
  securableType: string;
  namespace?: string[] | null;
  tableName?: string | null;
  privilege: string;
}

export function listGrants(roleId: string): Promise<Grant[]> {
  return apiJson<Grant[]>(`/control-plane/roles/${roleId}/grants`);
}

export async function addGrant(roleId: string, input: GrantInput): Promise<void> {
  await apiFetch(`/control-plane/roles/${roleId}/grants`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function revokeGrant(grantId: string): Promise<void> {
  await apiFetch(`/control-plane/grants/${grantId}`, { method: "DELETE" });
}

// ── Catalog Clients (per-consumer machine credentials) ───────────────────────

export interface CatalogClient {
  id: string;
  name: string;
  consumerType: string;
  catalogRoleId: string;
  clientId: string;
}

/** Returned ONCE on client creation — carries the one-time client secret. */
export interface CatalogClientCreated {
  id: string;
  clientId: string;
  clientSecret: string;
}

export interface CreateClientInput {
  name: string;
  consumerType: string;
  workspaceId: string;
  catalogRoleId: string;
}

export function listClients(warehouseId: string): Promise<CatalogClient[]> {
  return apiJson<CatalogClient[]>(`/control-plane/warehouses/${warehouseId}/clients`);
}

export function createClient(
  warehouseId: string,
  input: CreateClientInput,
): Promise<CatalogClientCreated> {
  return apiJson<CatalogClientCreated>(
    `/control-plane/warehouses/${warehouseId}/clients`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    },
  );
}

export async function revokeClient(id: string): Promise<void> {
  await apiFetch(`/control-plane/clients/${id}`, { method: "DELETE" });
}
