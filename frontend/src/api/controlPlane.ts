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

// ── Polaris connection (global / control-plane) ──────────────────────────────

/** External Apache Polaris catalog connection. Never carries the client secret. */
export interface PolarisConnection {
  baseUrl: string;
  kcTokenUrl: string;
  kcClientId: string;
  kcScope?: string;
}

/** Write payload for the Polaris connection — includes the write-only client secret. */
export interface PolarisConnectionInput {
  baseUrl: string;
  kcTokenUrl: string;
  kcClientId: string;
  kcClientSecret: string;
  kcScope?: string;
}

export interface PolarisHealth {
  ok: boolean;
  error?: string;
}

export function getPolarisConnection(): Promise<PolarisConnection | null> {
  return apiJson<PolarisConnection | null>("/control-plane/polaris/connection");
}

export function putPolarisConnection(input: PolarisConnectionInput): Promise<PolarisConnection> {
  return apiJson<PolarisConnection>("/control-plane/polaris/connection", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export function polarisHealth(): Promise<PolarisHealth> {
  return apiJson<PolarisHealth>("/control-plane/polaris/connection/health");
}

// ── Polaris catalogs (global / control-plane) ────────────────────────────────

export interface Catalog {
  name: string;
  catalogType: string;
}

export interface CreateCatalogInput {
  name: string;
  storageConnectionId: string;
}

export function listCatalogs(): Promise<Catalog[]> {
  return apiJson<Catalog[]>("/control-plane/polaris/catalogs");
}

export function createCatalog(input: CreateCatalogInput): Promise<Catalog> {
  return apiJson<Catalog>("/control-plane/polaris/catalogs", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export function getCatalog(name: string): Promise<Catalog> {
  return apiJson<Catalog>(`/control-plane/polaris/catalogs/${encodeURIComponent(name)}`);
}

export async function deleteCatalog(name: string): Promise<void> {
  await apiFetch(`/control-plane/polaris/catalogs/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
}

// ── Polaris RBAC (global / control-plane) ────────────────────────────────────

export interface Principal {
  name: string;
}

/** Returned once on principal creation — carries the one-time credentials. */
export interface PrincipalWithCreds {
  name: string;
  clientId: string;
  clientSecret: string;
}

export interface PolarisRole {
  name: string;
}

export interface GrantBody {
  kind: "catalog" | "namespace" | "table" | "view";
  privilege: string;
  namespace?: string[];
  table?: string;
  view?: string;
}

export function listPrincipals(): Promise<Principal[]> {
  return apiJson<Principal[]>("/control-plane/polaris/principals");
}

export function createPrincipal(name: string): Promise<PrincipalWithCreds> {
  return apiJson<PrincipalWithCreds>("/control-plane/polaris/principals", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name }),
  });
}

export async function deletePrincipal(name: string): Promise<void> {
  await apiFetch(`/control-plane/polaris/principals/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
}

export function listPrincipalRoles(): Promise<PolarisRole[]> {
  return apiJson<PolarisRole[]>("/control-plane/polaris/principal-roles");
}

export async function createPrincipalRole(name: string): Promise<void> {
  await apiFetch("/control-plane/polaris/principal-roles", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name }),
  });
}

export function listCatalogRoles(catalog: string): Promise<PolarisRole[]> {
  return apiJson<PolarisRole[]>(
    `/control-plane/polaris/catalogs/${encodeURIComponent(catalog)}/catalog-roles`,
  );
}

export async function createCatalogRole(catalog: string, name: string): Promise<void> {
  await apiFetch(
    `/control-plane/polaris/catalogs/${encodeURIComponent(catalog)}/catalog-roles`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    },
  );
}

export async function assignPrincipalRole(principal: string, role: string): Promise<void> {
  await apiFetch(
    `/control-plane/polaris/principals/${encodeURIComponent(principal)}/principal-roles`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ role }),
    },
  );
}

export async function assignCatalogRole(
  principalRole: string,
  catalog: string,
  catalogRole: string,
): Promise<void> {
  await apiFetch(
    `/control-plane/polaris/principal-roles/${encodeURIComponent(principalRole)}/catalog-roles/${encodeURIComponent(catalog)}`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ catalogRole }),
    },
  );
}

export async function addGrant(
  catalog: string,
  catalogRole: string,
  grant: GrantBody,
): Promise<void> {
  await apiFetch(
    `/control-plane/polaris/catalogs/${encodeURIComponent(catalog)}/catalog-roles/${encodeURIComponent(catalogRole)}/grants`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(grant),
    },
  );
}

export async function revokeGrant(
  catalog: string,
  catalogRole: string,
  grant: GrantBody,
): Promise<void> {
  await apiFetch(
    `/control-plane/polaris/catalogs/${encodeURIComponent(catalog)}/catalog-roles/${encodeURIComponent(catalogRole)}/grants:revoke`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(grant),
    },
  );
}

// ── Workspace catalog assignment (control-plane) ─────────────────────────────

export interface Assignment {
  catalogName: string;
  principalName: string;
}

export function listAssignments(ws: string): Promise<Assignment[]> {
  return apiJson<Assignment[]>(
    `/control-plane/workspaces/${encodeURIComponent(ws)}/catalog-assignments`,
  );
}

export async function assignCatalog(ws: string, catalog: string): Promise<void> {
  await apiFetch(
    `/control-plane/workspaces/${encodeURIComponent(ws)}/catalog-assignments`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ catalog }),
    },
  );
}

export async function unassignCatalog(ws: string, catalog: string): Promise<void> {
  await apiFetch(
    `/control-plane/workspaces/${encodeURIComponent(ws)}/catalog-assignments/${encodeURIComponent(catalog)}`,
    { method: "DELETE" },
  );
}
