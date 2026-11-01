/**
 * Policies (Ranger-backed / control-plane) API — MIGRATED to the orval-generated
 * client (E0·P4d-3).
 *
 * Thin adapter over `./generated/policy-admin/policy-admin` (tag `policy-admin`),
 * following the P4c/P4d pattern: the plain async helpers keep their old
 * names/signatures so call sites and the module-mocking tests don't churn. Each
 * helper calls the generated fetch fn and unwraps orval's `{ status, data,
 * headers }` envelope.
 *
 * Most policy domain types stay FE-local:
 *   - The three `*PolicyInput` editor types form a `kind`-discriminated union.
 *     The spec publishes `AccessPolicyInput`/`MaskingPolicyInput`/
 *     `RowFilterPolicyInput` only as loose *vocabulary* (every field optional),
 *     and create/update take a bare `Object` body, so we keep the strict FE union
 *     and cast at the adapter boundary (like P4d-1's ChartDataBody).
 *   - `getServiceDef`/`getPolicy` return verbatim jsonb (`Object` in the spec),
 *     so we keep the FE `ServiceDef`/`Record<string, unknown>` shapes and cast.
 *   - `PolicySummary.policy_type` and `Principal.kind` are narrow FE unions the
 *     spec widens to `string`; aliasing would churn call sites.
 * `Profile`/`ProfileEngine` map cleanly and are re-exported from the generated
 * model.
 */
import {
  policyAdminCreatePolicy,
  policyAdminCreateProfile,
  policyAdminDeletePolicyHandler,
  policyAdminGetPolicy,
  policyAdminGetServiceDef,
  policyAdminListPolicies,
  policyAdminListPrincipals,
  policyAdminListProfiles,
  policyAdminResourceLookup,
  policyAdminSeedDefaults,
  policyAdminSetPolicyEnabled,
  policyAdminTrinoConfig,
  policyAdminUpdatePolicy,
} from "./generated/policy-admin/policy-admin";
import {
  getPermClassifyServicePoliciesQueryKey,
  permClassifyServicePolicies,
  permMigrateServicePolicies,
  permUndoServicePolicyMigration,
} from "./generated/perm/perm";
import type {
  ClassifiedPolicy,
  CreatedPolicyId,
  CreatedProfile,
  MigrateResponse,
  UndoResponse,
  PolicyAdminCreatePolicyBody,
  PolicyAdminUpdatePolicyBody,
  ProfileDto,
  ProfileEngine,
} from "./generated/model";
import { ApiError } from "./client";

// ── Policies (Ranger-backed / control-plane) ─────────────────────────────────

// FE-local: the spec widens `policy_type` to a bare `string`; keep the union.
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
  /** Who owns the row (migration 0180). The server refuses edits, toggles and
   *  deletes of an `infra` row, so the profile page withholds those controls. */
  managed_by: "operator" | "compiler" | "infra";
}

// Re-export generated DTOs under the app's existing names.
export type { CreatedPolicyId, CreatedProfile, ProfileEngine };
// The generated ProfileDto declares `tag_service` (and, since DG #4a Q1,
// `policy_counts` and each engine's `origin` and `type`).
export type Profile = ProfileDto;

export async function listProfiles(): Promise<Profile[]> {
  return (await policyAdminListProfiles()).data as Profile[];
}

export async function listPolicies(service: string, type?: string): Promise<PolicySummary[]> {
  return (await policyAdminListPolicies({ service, type: type || undefined })).data as PolicySummary[];
}

/** Create a new Ranger service (policy profile) — the manual analogue of Trino's auto-bind. */
export async function createProfile(name: string, type: "trino" | "starrocks"): Promise<CreatedProfile> {
  return (await policyAdminCreateProfile({ name, type })).data as CreatedProfile;
}

// ── Policy editor (Task 4) ────────────────────────────────────────────────

export interface ResourceLevel { values: string[]; isExcludes?: boolean; isRecursive?: boolean; }
export interface AccessItem {
  users: string[];
  groups: string[];
  roles: string[];
  accesses: string[];
  effect: "allow" | "deny";
  delegateAdmin?: boolean;
}
export interface AccessPolicyInput {
  name: string;
  service: string;
  isEnabled: boolean;
  /** Generic resource-type → value map, e.g. `{ catalog: {...}, schema: {...} }` or `{ role: {...} }`. */
  resources: Record<string, ResourceLevel>;
  /** Additional resource sets (Ranger's "additionalResources") — same shape as `resources`. */
  additional: Record<string, ResourceLevel>[];
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
  /** Name of the parent resource type, e.g. "schema" for "table". Absent for root types (catalog, role, …). */
  parent?: string;
  /** Depth in the resource hierarchy (0 = root). */
  level?: number;
  /** Access types allowed when this resource is the policy's leaf (e.g. queryid → only "sqlinfo"). */
  accessTypeRestrictions?: string[];
  /** Whether a policy may validly stop the resource chain at this type (vs. requiring a child). */
  isValidLeaf?: boolean;
  uiHint?: string;
}

export interface ServiceDef {
  accessTypes?: { name: string; label?: string }[];
  resources?: ServiceDefResource[];
  /** Def for masking policies — its own resource hierarchy (e.g. catalog/schema/table/column) and mask-type list. */
  dataMaskDef?: { resources?: ServiceDefResource[]; maskTypes?: { name: string; label?: string }[] };
  /** Def for row-filter policies — its own resource hierarchy (e.g. catalog/schema/table). */
  rowFilterDef?: { resources?: ServiceDefResource[] };
  [k: string]: unknown;
}

export async function getServiceDef(name: string): Promise<ServiceDef> {
  return (await policyAdminGetServiceDef(name)).data as ServiceDef;
}

export async function getPolicy(id: number): Promise<Record<string, unknown>> {
  return (await policyAdminGetPolicy(id)).data as Record<string, unknown>;
}

export async function listPrincipals(q: string, kind?: "user" | "group"): Promise<Principal[]> {
  return (await policyAdminListPrincipals({ q, kind: kind || undefined })).data as Principal[];
}

export interface MaskItem { users: string[]; groups: string[]; maskType: string; valueExpr?: string; }
export interface MaskingPolicyInput {
  kind: "masking";
  name: string; service: string; isEnabled: boolean;
  resources: Record<string, ResourceLevel>;
  items: MaskItem[];
  description: string;
  labels: string[];
  isOverride: boolean;
}
export interface RowFilterItem { users: string[]; groups: string[]; filterExpr: string; }
export interface RowFilterPolicyInput {
  kind: "rowfilter";
  name: string; service: string; isEnabled: boolean;
  resources: Record<string, ResourceLevel>;
  items: RowFilterItem[];
  description: string;
  labels: string[];
  isOverride: boolean;
}
export type PolicyInput = (AccessPolicyInput & { kind: "access" }) | MaskingPolicyInput | RowFilterPolicyInput;

export interface ConfigFile { name: string; content: string; }
/** The coordinator files, and what makes them not work as rendered (KI-149). */
export interface TrinoConfig { files: ConfigFile[]; warnings: string[]; }
export async function getTrinoConfig(service: string): Promise<TrinoConfig> {
  return (await policyAdminTrinoConfig(service)).data as TrinoConfig;
}

/** Seed default "admin has all access" policies (one per Trino resource hierarchy). Idempotent. */
export async function seedDefaultPolicies(service: string): Promise<{ created: number }> {
  return (await policyAdminSeedDefaults(service)).data as { created: number };
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

export async function createPolicy(input: PolicyInput): Promise<CreatedPolicyId> {
  // The spec types the body as a loose `Object`; cast the strict FE union.
  return (await policyAdminCreatePolicy(input as unknown as PolicyAdminCreatePolicyBody)).data as CreatedPolicyId;
}

export async function updatePolicy(id: number, input: PolicyInput): Promise<CreatedPolicyId> {
  return (await policyAdminUpdatePolicy(id, input as unknown as PolicyAdminUpdatePolicyBody)).data as CreatedPolicyId;
}

export async function deletePolicy(id: number): Promise<void> {
  await policyAdminDeletePolicyHandler(id);
}

export async function resourceLookup(
  service: string,
  level: string,
  parents: { catalog?: string; schema?: string; table?: string } = {},
  q = "",
): Promise<string[]> {
  return (
    await policyAdminResourceLookup({
      service,
      level,
      catalog: parents.catalog || undefined,
      schema: parents.schema || undefined,
      table: parents.table || undefined,
      q: q || undefined,
    })
  ).data as string[];
}

export async function setPolicyEnabled(id: number, enabled: boolean): Promise<void> {
  await policyAdminSetPolicyEnabled(id, { enabled });
}

// ── D10: the 409 a compiler-owned policy produces ──────────────────────────
// (Who owns a row is read from `PolicySummary.managed_by`, spec #4a premise 1.)

/**
 * A write refused because the SP4a permission-model compiler owns the
 * target policy (D10). `PolicyStoreError::CompilerOwned` surfaces as a 409
 * whose JSON body's `error` field is `PolicyHttpError::Conflict`'s Display
 * text, `"conflict: <reason>"` — the reason itself already names the policy
 * and says to edit the grant/rule instead, so this only strips the
 * "conflict: " wire prefix rather than replacing the server's own
 * explanation with a generic one. Returns `null` for anything that is not
 * this specific refusal, so a caller falls back to its normal error path.
 */
export function compilerOwnedConflict(err: unknown): string | null {
  if (!(err instanceof ApiError) || err.status !== 409) return null;
  const msg = err.message || "";
  if (msg.startsWith("conflict: ")) return msg.slice("conflict: ".length);
  return msg || "This policy is compiled by the permission model and cannot be edited directly.";
}

// ── SP4d-3: classify a profile's policies, migrate them, undo ─────────────
//
// Thin adapters over the generated `perm` client (tag `perm`), in this file's
// style: unwrap orval's envelope, keep the generated DTOs. The blocker
// sentences, the disagreement sentences, the refusal and the notices are the
// SERVER's text — nothing here re-derives or re-words them.

export type {
  ClassifiedPolicy,
  GatedProfile,
  MigrateResponse,
  MigratedPolicy,
  MigrationRecord,
  UndoResponse,
  WireDisagreement,
} from "./generated/model";

/** Every policy of one profile, classified (owner, verdict, blocker sentences). */
export async function classifyServicePolicies(serviceId: number): Promise<ClassifiedPolicy[]> {
  return (await permClassifyServicePolicies(serviceId)).data as ClassifiedPolicy[];
}

/** The key the classification is cached under — orval's own key factory, so a
 *  migrate or undo invalidates exactly what the page reads. */
export function classifyQueryKey(serviceId: number) {
  return getPermClassifyServicePoliciesQueryKey(serviceId);
}

function isMigrateResponse(body: unknown): body is MigrateResponse {
  return (
    typeof body === "object" &&
    body !== null &&
    "equivalent" in body &&
    Array.isArray((body as { disagreements?: unknown }).disagreements)
  );
}

/**
 * Migrate (or, with `dryRun`, prove without writing) the named policies of one
 * profile.
 *
 * **A 422 is an ANSWER, not a failure:** the gate ran and did not prove the
 * migration — its body is the same `MigrateResponse`, carrying the
 * disagreements or the gate's refusal (the probe budget among them). So it
 * resolves, and the caller reads `equivalent` / `refusal` / `disagreements`.
 * Every other non-2xx (400, 403, 404, 409, 500) still throws its `ApiError`.
 */
export async function migrateServicePolicies(
  serviceId: number,
  policyNames: string[],
  dryRun: boolean,
): Promise<MigrateResponse> {
  try {
    return (
      await permMigrateServicePolicies(serviceId, { policy_names: policyNames, dry_run: dryRun })
    ).data as MigrateResponse;
  } catch (e) {
    if (e instanceof ApiError && e.status === 422 && isMigrateResponse(e.body)) return e.body;
    throw e;
  }
}

/** Undo the named migrations (the policies' CURRENT names). */
export async function undoServicePolicyMigration(
  serviceId: number,
  policyNames: string[],
): Promise<UndoResponse> {
  return (await permUndoServicePolicyMigration(serviceId, { policy_names: policyNames }))
    .data as UndoResponse;
}
