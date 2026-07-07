import { apiFetch, apiJson } from "./client";

// ── Policies (Ranger-backed / control-plane) ─────────────────────────────────

export interface PolicySummary {
  id: number;
  name: string;
  service: string;
  policy_type: "access" | "masking" | "rowfilter";
  resource_path: string;
  principals: string[];
  permissions: string[];
  is_enabled: boolean;
  description: string;
  labels: string[];
}

export interface ProfileEngine {
  id: string;
  name: string;
  status: string;
}

export interface Profile {
  id: number;
  name: string;
  type: string;
  policy_version: number;
  policy_count: number;
  engines: ProfileEngine[];
}

export function listProfiles(): Promise<Profile[]> {
  return apiJson<Profile[]>("/control-plane/policies/services");
}

export function listPolicies(service: string, type?: string): Promise<PolicySummary[]> {
  const p = new URLSearchParams({ service });
  if (type) p.set("type", type);
  return apiJson<PolicySummary[]>(`/control-plane/policies?${p.toString()}`);
}

// ── Policy editor (Task 4) ────────────────────────────────────────────────

export interface ResourceLevel { values: string[]; isExcludes?: boolean; isRecursive?: boolean; }
export interface AccessItem { users: string[]; groups: string[]; accesses: string[]; effect: "allow" | "deny"; }
export interface AccessPolicyInput {
  name: string;
  service: string;
  isEnabled: boolean;
  resources: { catalog: ResourceLevel; schema: ResourceLevel; table: ResourceLevel; column: ResourceLevel };
  /** "table" (catalog/schema/table/column) or "impersonation" (trinouser). */
  resourceMode: "table" | "impersonation";
  /** The trinouser resource, used only when resourceMode === "impersonation". */
  trinouser: ResourceLevel;
  items: AccessItem[];
  allowExceptions: AccessItem[];
  denyExceptions: AccessItem[];
  isDenyAllElse: boolean;
  description: string;
  labels: string[];
  isOverride: boolean;
}

export interface Principal { kind: "user" | "group"; name: string; label: string; }

export interface ServiceDefResource {
  name: string;
  excludesSupported?: boolean;
  recursiveSupported?: boolean;
  mandatory?: boolean;
}

export interface ServiceDef {
  accessTypes?: { name: string; label?: string }[];
  resources?: ServiceDefResource[];
  [k: string]: unknown;
}

export function getServiceDef(name: string): Promise<ServiceDef> {
  return apiJson<ServiceDef>(`/control-plane/policies/service-def/${encodeURIComponent(name)}`);
}

export function getPolicy(id: number): Promise<Record<string, unknown>> {
  return apiJson<Record<string, unknown>>(`/control-plane/policies/${id}`);
}

export function listPrincipals(q: string, kind?: "user" | "group"): Promise<Principal[]> {
  const p = new URLSearchParams({ q });
  if (kind) p.set("kind", kind);
  return apiJson<Principal[]>(`/control-plane/principals?${p.toString()}`);
}

export interface MaskItem { users: string[]; groups: string[]; maskType: string; valueExpr?: string; }
export interface MaskingPolicyInput {
  kind: "masking";
  name: string; service: string; isEnabled: boolean;
  resources: { catalog: ResourceLevel; schema: ResourceLevel; table: ResourceLevel; column: ResourceLevel };
  items: MaskItem[];
  description: string;
  labels: string[];
  isOverride: boolean;
}
export interface RowFilterItem { users: string[]; groups: string[]; filterExpr: string; }
export interface RowFilterPolicyInput {
  kind: "rowfilter";
  name: string; service: string; isEnabled: boolean;
  resources: { catalog: ResourceLevel; schema: ResourceLevel; table: ResourceLevel };
  items: RowFilterItem[];
  description: string;
  labels: string[];
  isOverride: boolean;
}
export type PolicyInput = (AccessPolicyInput & { kind: "access" }) | MaskingPolicyInput | RowFilterPolicyInput;

export interface ConfigFile { name: string; content: string; }
export function getTrinoConfig(service: string): Promise<ConfigFile[]> {
  return apiJson<ConfigFile[]>(`/control-plane/policies/services/${encodeURIComponent(service)}/trino-config`);
}

/** Seed default "admin has all access" policies (one per Trino resource hierarchy). Idempotent. */
export function seedDefaultPolicies(service: string): Promise<{ created: number }> {
  return apiJson<{ created: number }>(
    `/control-plane/policies/services/${encodeURIComponent(service)}/seed-defaults`,
    { method: "POST" },
  );
}

export const MASK_TYPES = [
  { name: "MASK", label: "Redact" },
  { name: "MASK_SHOW_LAST_4", label: "Show last 4" },
  { name: "MASK_SHOW_FIRST_4", label: "Show first 4" },
  { name: "MASK_HASH", label: "Hash" },
  { name: "MASK_NULL", label: "Nullify" },
  { name: "MASK_NONE", label: "Unmasked" },
  { name: "MASK_DATE_SHOW_YEAR", label: "Date: year only" },
  { name: "CUSTOM", label: "Custom expression" },
] as const;

export function createPolicy(input: PolicyInput): Promise<{ id: number }> {
  return apiJson<{ id: number }>("/control-plane/policies", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
  });
}

export function updatePolicy(id: number, input: PolicyInput): Promise<{ id: number }> {
  return apiJson<{ id: number }>(`/control-plane/policies/${id}`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
  });
}

export function deletePolicy(id: number): Promise<void> {
  return apiFetch(`/control-plane/policies/${id}`, { method: "DELETE" }).then(() => undefined);
}

export function resourceLookup(
  service: string,
  level: "catalog" | "schema" | "table" | "column",
  parents: { catalog?: string; schema?: string; table?: string } = {},
  q = "",
): Promise<string[]> {
  const p = new URLSearchParams({ service, level });
  if (parents.catalog) p.set("catalog", parents.catalog);
  if (parents.schema) p.set("schema", parents.schema);
  if (parents.table) p.set("table", parents.table);
  if (q) p.set("q", q);
  return apiJson<string[]>(`/control-plane/policies/resource-lookup?${p.toString()}`);
}

export function setPolicyEnabled(id: number, enabled: boolean): Promise<void> {
  return apiFetch(`/control-plane/policies/${id}/enabled`, {
    method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ enabled }),
  }).then(() => undefined);
}
