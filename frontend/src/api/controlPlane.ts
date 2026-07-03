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

export interface StorageObject {
  key: string;
  size: number;
  lastModified: string | null;
}
export interface StorageListing {
  folders: string[];
  objects: StorageObject[];
}
export function browseStorageObjects(id: string, prefix: string): Promise<StorageListing> {
  return apiJson<StorageListing>(
    `/control-plane/storage/${id}/objects?prefix=${encodeURIComponent(prefix)}`,
  );
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
  description: string | null;
}

export interface CreateRoleInput {
  name: string;
  consumerType: string;
  description?: string | null;
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

export async function deleteRole(roleId: string): Promise<void> {
  await apiFetch(`/control-plane/roles/${roleId}`, { method: "DELETE" });
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

// ── Warehouse browse (embedded catalog) ──────────────────────────────────────

export interface Namespace {
  parts: string[];
  id: string | null;
  protected: boolean;
}
export interface Tabular {
  name: string;
  kind: "table" | "view";
  id: string | null;
  protected: boolean;
}
export interface DeletedTabular {
  id: string;
  name: string;
  namespace: string[];
  kind: "table" | "view";
  deletedAt: string;
  expiration: string;
}
export interface Column {
  name: string;
  type: string;
  required: boolean;
}
export interface PartitionField {
  name: string;
  sourceColumn: string;
  transform: string;
}
export interface SortField {
  column: string;
  transform: string;
  direction: string;
  nullOrder: string;
}
export interface TableSnapshot {
  snapshotId: number;
  parentSnapshotId: number | null;
  timestampMs: number;
  operation: string;
  summary: Record<string, string>;
}
/** Rich table detail: schema + partition/sort (with transforms) + location + properties + snapshots. */
export interface TableDetail {
  location: string;
  formatVersion: number;
  lastUpdatedMs: number;
  currentSnapshotId: number | null;
  snapshotCount: number;
  columns: Column[];
  partitionFields: PartitionField[];
  sortFields: SortField[];
  properties: Record<string, string>;
  snapshots: TableSnapshot[];
}

export function listNamespaces(warehouseId: string): Promise<Namespace[]> {
  return apiJson<Namespace[]>(`/control-plane/warehouses/${warehouseId}/namespaces`);
}
export function listTabulars(warehouseId: string, namespace: string): Promise<Tabular[]> {
  return apiJson<Tabular[]>(
    `/control-plane/warehouses/${warehouseId}/namespaces/${encodeURIComponent(namespace)}/tables`,
  );
}
export function loadTableDetail(
  warehouseId: string,
  namespace: string,
  table: string,
): Promise<TableDetail> {
  return apiJson<TableDetail>(
    `/control-plane/warehouses/${warehouseId}/namespaces/${encodeURIComponent(namespace)}/tables/${encodeURIComponent(table)}`,
  );
}
export function loadViewColumns(
  warehouseId: string,
  namespace: string,
  view: string,
): Promise<Column[]> {
  return apiJson<Column[]>(
    `/control-plane/warehouses/${warehouseId}/namespaces/${encodeURIComponent(namespace)}/views/${encodeURIComponent(view)}`,
  );
}
export interface SearchResult {
  namespace: string[];
  name: string;
  kind: "table" | "view";
}
export interface WarehouseTask {
  queue: string;
  entity: string | null;
  status: string;
}
export function searchTabulars(warehouseId: string, q: string): Promise<SearchResult[]> {
  return apiJson<SearchResult[]>(
    `/control-plane/warehouses/${warehouseId}/search?q=${encodeURIComponent(q)}`,
  );
}
export function listWarehouseTasks(warehouseId: string): Promise<WarehouseTask[]> {
  return apiJson<WarehouseTask[]>(`/control-plane/warehouses/${warehouseId}/tasks`);
}
export interface WarehouseConfig {
  active: boolean;
  deleteProfileSeconds: number | null;
  numTables: number;
  numViews: number;
  statsUpdatedMs: number;
}
export function getWarehouseConfig(warehouseId: string): Promise<WarehouseConfig> {
  return apiJson<WarehouseConfig>(`/control-plane/warehouses/${warehouseId}/config`);
}
export async function setWarehouseStatus(warehouseId: string, active: boolean): Promise<void> {
  await apiFetch(`/control-plane/warehouses/${warehouseId}/status`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ active }),
  });
}
export async function setDeleteProfile(warehouseId: string, seconds: number | null): Promise<void> {
  await apiFetch(`/control-plane/warehouses/${warehouseId}/delete-profile`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ seconds }),
  });
}
export async function setFormatPolicy(warehouseId: string, versions: number[]): Promise<void> {
  await apiFetch(`/control-plane/warehouses/${warehouseId}/format-policy`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ versions }),
  });
}
export async function rotateCredential(
  warehouseId: string,
  accessKey: string,
  secretKey: string,
): Promise<void> {
  await apiFetch(`/control-plane/warehouses/${warehouseId}/storage-credential`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ accessKey, secretKey }),
  });
}

export function listDeletedTabulars(warehouseId: string): Promise<DeletedTabular[]> {
  return apiJson<DeletedTabular[]>(`/control-plane/warehouses/${warehouseId}/deleted-tabulars`);
}
export async function undropTabular(
  warehouseId: string,
  kind: "table" | "view",
  id: string,
): Promise<void> {
  await apiFetch(`/control-plane/warehouses/${warehouseId}/undrop`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ kind, id }),
  });
}
export function setProtection(
  warehouseId: string,
  target: "table" | "view" | "namespace",
  id: string,
  protectedFlag: boolean,
): Promise<{ protected: boolean }> {
  return apiJson<{ protected: boolean }>(
    `/control-plane/warehouses/${warehouseId}/protection`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ target, id, protected: protectedFlag }),
    },
  );
}
