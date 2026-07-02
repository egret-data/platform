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
