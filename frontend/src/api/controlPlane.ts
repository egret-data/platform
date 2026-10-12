/**
 * Control-plane API (engines · warehouses/catalog · storage · secrets) —
 * MIGRATED to the orval-generated client (E0·P4d-4).
 *
 * Thin adapter over the generated fetch fns for the `control-plane-engines`,
 * `control-plane-catalog`, `control-plane-storage` and `control-plane-secrets`
 * tags, following the P4c/P4d pattern (closest analog: `access.ts`). Every plain
 * async helper keeps its old name/signature so call sites (ControlPlaneEngines /
 * EngineDetail / Warehouses / WarehouseDetail / Storage / Secrets pages) and the
 * module-mocking page tests (`vi.mock("@/api/controlPlane")`) don't churn. Each
 * helper calls the generated fetch fn and unwraps orval's `{ status, data,
 * headers }` envelope to the same body the hand-written `apiJson`/`apiFetch`
 * returned.
 *
 * FE-only types are PRESERVED verbatim: the spec types several engine/catalog
 * fields loosely (`serde_json::Value` → `{ [k]: unknown }` for `trinoConfig`,
 * catalog `config`, query `outputStage`, kube specs), so the richly-typed FE
 * interfaces (`TrinoConfig`, `EngineSpec`, `TableDetail`, `MonitorQuery*`, …) stay
 * here and are cast at the adapter boundary — aliasing the loose generated DTOs
 * would widen these and churn every call site.
 *
 * WebSocket streaming is NOT adopted: pod `logs/follow` and `exec` upgrade to a
 * WebSocket (the generated client cannot do WS) and are opened directly with
 * `new WebSocket(...)` in `PodLogsDialog.tsx` / `PodTerminalDialog.tsx` — outside
 * this module, untouched. Only the bounded `pods/{pod}/logs` GET (JSON `PodLogs`
 * tail) lives here and is migrated to `controlPlaneEnginesPodLogs`.
 */
import {
  controlPlaneCatalogAddGrant,
  controlPlaneCatalogBrowseDeleted,
  controlPlaneCatalogBrowseNamespaces,
  controlPlaneCatalogBrowseSchema,
  controlPlaneCatalogBrowseTables,
  controlPlaneCatalogBrowseView,
  controlPlaneCatalogCreateClient,
  controlPlaneCatalogCreateRole,
  controlPlaneCatalogCreateWarehouse,
  controlPlaneCatalogDeleteRole,
  controlPlaneCatalogDeleteWarehouse,
  controlPlaneCatalogGetWarehouse,
  controlPlaneCatalogListClients,
  controlPlaneCatalogListGrants,
  controlPlaneCatalogListRoles,
  controlPlaneCatalogListTasks,
  controlPlaneCatalogListWarehouses,
  controlPlaneCatalogRevokeClient,
  controlPlaneCatalogRevokeGrant,
  controlPlaneCatalogRotateCredential,
  controlPlaneCatalogSearchTabulars,
  controlPlaneCatalogSetDeleteProfileRoute,
  controlPlaneCatalogSetFormatPolicy,
  controlPlaneCatalogSetProtection,
  controlPlaneCatalogSetWarehouseStatus,
  controlPlaneCatalogUndropTabular,
  controlPlaneCatalogWarehouseConfig,
} from "./generated/control-plane-catalog/control-plane-catalog";
import {
  controlPlaneEnginesCreateEngine,
  controlPlaneEnginesDeleteEngine,
  controlPlaneEnginesGetCluster,
  controlPlaneEnginesGetEngine,
  controlPlaneEnginesGetQuery,
  controlPlaneEnginesKillQuery,
  controlPlaneEnginesListEngines,
  controlPlaneEnginesListQueries,
  controlPlaneEnginesPodLogs,
  controlPlaneEnginesRestartEngine,
  controlPlaneEnginesRotateEngineSecrets,
  controlPlaneEnginesUpdateEngine,
} from "./generated/control-plane-engines/control-plane-engines";
import {
  controlPlaneSecretsCreate,
  controlPlaneSecretsList,
  controlPlaneSecretsRemove,
  controlPlaneSecretsUpdate,
} from "./generated/control-plane-secrets/control-plane-secrets";
import {
  controlPlaneStorageBrowseObjects,
  controlPlaneStorageCreate,
  controlPlaneStorageDetail,
  controlPlaneStorageList,
  controlPlaneStorageRemove,
  controlPlaneStorageUpdate,
} from "./generated/control-plane-storage/control-plane-storage";
import type {
  CreateEngineBody,
  CreateEngineResponse,
  NewEngineCatalog,
} from "./generated/model";
import { ApiError } from "./client";

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
export async function browseStorageObjects(id: string, prefix: string): Promise<StorageListing> {
  return (await controlPlaneStorageBrowseObjects(id, { prefix })).data as StorageListing;
}
export async function listStorage(): Promise<Storage[]> {
  return (await controlPlaneStorageList()).data as Storage[];
}

export async function getStorage(id: string): Promise<Storage> {
  return (await controlPlaneStorageDetail(id)).data as Storage;
}

export async function createStorage(input: StorageInput): Promise<Storage> {
  return (await controlPlaneStorageCreate(input)).data as Storage;
}

export async function updateStorage(id: string, input: StorageInput): Promise<Storage> {
  return (await controlPlaneStorageUpdate(id, input)).data as Storage;
}

export async function deleteStorage(id: string): Promise<void> {
  await controlPlaneStorageRemove(id);
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
  // k8s-style type (Opaque / kubernetes.io/tls / kubernetes.io/basic-auth).
  type: string;
  // Data key names (NOT values).
  keys: string[];
}

export interface SecretInput {
  name: string;
  description?: string | null;
  type?: string;
  data?: Record<string, string>;
  // Back-compat single value (Opaque{value}); prefer `type`+`data`.
  value?: string;
}

export interface SecretUpdate {
  name?: string;
  description?: string | null;
  type?: string;
  data?: Record<string, string>;
  value?: string;
}

export async function listSecrets(): Promise<Secret[]> {
  return (await controlPlaneSecretsList()).data as Secret[];
}

export async function createSecret(input: SecretInput): Promise<Secret> {
  return (await controlPlaneSecretsCreate(input)).data as Secret;
}

export async function updateSecret(id: string, input: SecretUpdate): Promise<Secret> {
  return (await controlPlaneSecretsUpdate(id, input)).data as Secret;
}

export async function deleteSecret(id: string): Promise<void> {
  await controlPlaneSecretsRemove(id);
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

export async function listWarehouses(): Promise<Warehouse[]> {
  return (await controlPlaneCatalogListWarehouses()).data as Warehouse[];
}

export async function getWarehouse(id: string): Promise<Warehouse> {
  return (await controlPlaneCatalogGetWarehouse(id)).data as Warehouse;
}

export async function createWarehouse(input: CreateWarehouseInput): Promise<Warehouse> {
  return (await controlPlaneCatalogCreateWarehouse(input)).data as Warehouse;
}

export async function deleteWarehouse(id: string): Promise<void> {
  await controlPlaneCatalogDeleteWarehouse(id);
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

export async function listRoles(warehouseId: string): Promise<CatalogRole[]> {
  return (await controlPlaneCatalogListRoles(warehouseId)).data as CatalogRole[];
}

export async function createRole(
  warehouseId: string,
  input: CreateRoleInput,
): Promise<CatalogRole> {
  return (await controlPlaneCatalogCreateRole(warehouseId, input)).data as CatalogRole;
}

export async function deleteRole(roleId: string): Promise<void> {
  await controlPlaneCatalogDeleteRole(roleId);
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

export async function listGrants(roleId: string): Promise<Grant[]> {
  return (await controlPlaneCatalogListGrants(roleId)).data as Grant[];
}

export async function addGrant(roleId: string, input: GrantInput): Promise<void> {
  await controlPlaneCatalogAddGrant(roleId, input);
}

export async function revokeGrant(grantId: string): Promise<void> {
  await controlPlaneCatalogRevokeGrant(grantId);
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

export async function listClients(warehouseId: string): Promise<CatalogClient[]> {
  return (await controlPlaneCatalogListClients(warehouseId)).data as CatalogClient[];
}

export async function createClient(
  warehouseId: string,
  input: CreateClientInput,
): Promise<CatalogClientCreated> {
  return (await controlPlaneCatalogCreateClient(warehouseId, input)).data as CatalogClientCreated;
}

export async function revokeClient(id: string): Promise<void> {
  await controlPlaneCatalogRevokeClient(id);
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

export async function listNamespaces(warehouseId: string): Promise<Namespace[]> {
  return (await controlPlaneCatalogBrowseNamespaces(warehouseId)).data as Namespace[];
}
export async function listTabulars(warehouseId: string, namespace: string): Promise<Tabular[]> {
  return (await controlPlaneCatalogBrowseTables(warehouseId, namespace)).data as Tabular[];
}
export async function loadTableDetail(
  warehouseId: string,
  namespace: string,
  table: string,
): Promise<TableDetail> {
  return (await controlPlaneCatalogBrowseSchema(warehouseId, namespace, table)).data as TableDetail;
}
export async function loadViewColumns(
  warehouseId: string,
  namespace: string,
  view: string,
): Promise<Column[]> {
  return (await controlPlaneCatalogBrowseView(warehouseId, namespace, view)).data as Column[];
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
export async function searchTabulars(warehouseId: string, q: string): Promise<SearchResult[]> {
  return (await controlPlaneCatalogSearchTabulars(warehouseId, { q })).data as SearchResult[];
}
export async function listWarehouseTasks(warehouseId: string): Promise<WarehouseTask[]> {
  return (await controlPlaneCatalogListTasks(warehouseId)).data as WarehouseTask[];
}
export interface WarehouseConfig {
  active: boolean;
  deleteProfileSeconds: number | null;
  numTables: number;
  numViews: number;
  statsUpdatedMs: number;
}
export async function getWarehouseConfig(warehouseId: string): Promise<WarehouseConfig> {
  return (await controlPlaneCatalogWarehouseConfig(warehouseId)).data as WarehouseConfig;
}
export async function setWarehouseStatus(warehouseId: string, active: boolean): Promise<void> {
  await controlPlaneCatalogSetWarehouseStatus(warehouseId, { active });
}
export async function setDeleteProfile(warehouseId: string, seconds: number | null): Promise<void> {
  await controlPlaneCatalogSetDeleteProfileRoute(warehouseId, { seconds });
}
export async function setFormatPolicy(warehouseId: string, versions: number[]): Promise<void> {
  await controlPlaneCatalogSetFormatPolicy(warehouseId, { versions });
}
export async function rotateCredential(
  warehouseId: string,
  accessKey: string,
  secretKey: string,
): Promise<void> {
  await controlPlaneCatalogRotateCredential(warehouseId, { accessKey, secretKey });
}

export async function listDeletedTabulars(warehouseId: string): Promise<DeletedTabular[]> {
  return (await controlPlaneCatalogBrowseDeleted(warehouseId)).data as DeletedTabular[];
}
export async function undropTabular(
  warehouseId: string,
  kind: "table" | "view",
  id: string,
): Promise<void> {
  await controlPlaneCatalogUndropTabular(warehouseId, { kind, id });
}
export async function setProtection(
  warehouseId: string,
  target: "table" | "view" | "namespace",
  id: string,
  protectedFlag: boolean,
): Promise<{ protected: boolean }> {
  return (
    await controlPlaneCatalogSetProtection(warehouseId, {
      target,
      id,
      protected: protectedFlag,
    })
  ).data as { protected: boolean };
}

// ── Engines (query engines / control-plane) ──────────────────────────────────

/** Provisioning parameters surfaced back to clients (subset of the stored spec). */
export interface EngineSpec {
  coordinatorReplicas: number;
  workerReplicas: number;
  cpuMin: string;
  cpuMax: string;
  memory: string;
  imageVersion: string;
  // Fuller Trino config surface, stored under `spec.trinoConfig` (camelCase keys).
  // Only present if the engine DTO exposes it — guard for undefined on read.
  trinoConfig?: TrinoConfig;
  // StarRocks config surface, stored under `spec.starrocksConfig` (camelCase).
  // Only present on StarRocks engines — guard for undefined on read.
  starrocksConfig?: StarRocksSpecConfig;
}

/**
 * Stored StarRocks spec config (read back from GET `spec.starrocksConfig`). This
 * is the read shape the engine-detail Configuration form seeds from — the PUT
 * RESPONSE (EngineDto) only echoes Trino fields, so always seed from the GET.
 *
 * Storage coords are NOT persisted here: the FE/CN builtin storage volume is
 * re-resolved from the warehouse at provision time, so `spec.starrocksConfig`
 * never carries `storageEndpoint`/`storagePath` (declared optional purely so a
 * read-only Storage panel can render them defensively if a future spec adds them).
 */
export interface StarRocksSpecConfig {
  feImage?: string;
  cnImage?: string;
  feReplicas?: number;
  cnReplicas?: number;
  // Persisted verbatim: a `RoleResources` object or `null` (= default sizing).
  feResources?: RoleResources | null;
  cnResources?: RoleResources | null;
  // Delegation opt-in `{ mode, tlsEnabled }` (or `null` when no delegation).
  delegation?: { mode?: string; tlsEnabled?: boolean } | null;
  // Deep-config surface (Phase 1.5). fe.conf / cn.conf key-value overrides
  // merged into the generated config (protected keys rejected server-side);
  // stored as `{}` when empty.
  feConfig?: Record<string, string>;
  cnConfig?: Record<string, string>;
  // CN horizontal autoscaling, or `null` when off. `{ maxReplicas: 0 }` is the
  // stored disabled sentinel.
  cnAutoscaling?: StarRocksAutoscaling | null;
  // Pod & scheduling overrides (edited by a separate Pod tab); carried verbatim.
  pod?: Record<string, unknown> | null;
  // Never persisted today (see doc above); read defensively.
  storageEndpoint?: string;
  storagePath?: string;
}

/** CN horizontal-autoscaling policy (CPU-only). `maxReplicas: 0` disables it. */
export interface StarRocksAutoscaling {
  minReplicas?: number;
  maxReplicas: number;
  targetCpuPercent?: number;
}

/** A provisioned query engine (Phase 1: Trino on Kubernetes). */
export interface Engine {
  id: string;
  name: string;
  type: string;
  namespace: string;
  status: string;
  spec: EngineSpec;
  /**
   * `external` for an engine egret only holds a row for (SP4d-4); absent on
   * older payloads, which means `provisioned`. The fields below are external only.
   */
  origin?: string;
  host?: string | null;
  port?: number | null;
  rangerPluginVersion?: string | null;
  rangerPluginVersionModelled?: boolean | null;
}

/** Live status read back from the operator + pods. */
export interface EngineStatus {
  phase: string;
  message: string | null;
  pods: { name: string; phase: string; ready: boolean }[];
}

/** A Kubernetes liveness/readiness/startup probe (all fields optional; numbers). */
export interface Probe {
  initialDelaySeconds?: number;
  periodSeconds?: number;
  timeoutSeconds?: number;
  failureThreshold?: number;
  successThreshold?: number;
}

/** JVM heap sizing: fixed (`maxSize`/`minSize`) XOR percent (`maxPercent`/`initialPercent`). */
export interface HeapConfig {
  maxSize?: string;
  minSize?: string;
  maxPercent?: number;
  initialPercent?: number;
}

/** CPU/memory requests + limits (Kubernetes resource strings, e.g. "1" / "2Gi"). */
export interface RoleResources {
  cpuRequest?: string;
  cpuLimit?: string;
  memRequest?: string;
  memLimit?: string;
}

// ── Pod & scheduling (Phase 1.5.3) ───────────────────────────────────────────

/** A pod toleration (all fields optional). */
export interface Toleration {
  key?: string;
  operator?: string;
  value?: string;
  effect?: string;
  tolerationSeconds?: number;
}

/** A topology-spread constraint (labelSelector.matchLabels). */
export interface TopologySpread {
  maxSkew: number;
  topologyKey: string;
  whenUnsatisfiable: string;
  matchLabels: Record<string, string>;
}

/** A node-affinity match expression (nodeSelectorTerms/matchExpressions). */
export interface NodeSelectorRequirement {
  key: string;
  operator: string;
  values: string[];
}

/** Structured-common + raw-override affinity. `rawOverride` wins verbatim. */
export interface Affinity {
  nodeMatch?: NodeSelectorRequirement[];
  podAntiAffinityTopologyKey?: string;
  rawOverride?: unknown;
}

/** An extra exposed container port. */
export interface ContainerPort {
  name: string;
  containerPort: number;
}

/** Pod/container security context (flat common fields + raw override). */
export interface SecurityContext {
  runAsUser?: number;
  runAsGroup?: number;
  fsGroup?: number;
  runAsNonRoot?: boolean;
  rawOverride?: unknown;
}

/** A container volume mount. */
export interface VolumeMount {
  name: string;
  mountPath: string;
  subPath?: string;
}

/** A minimal init/sidecar container spec (structured-common + raw override). */
export interface MiniContainer {
  name: string;
  image: string;
  command?: string[];
  args?: string[];
  env?: Record<string, string>;
  volumeMounts?: VolumeMount[];
  rawOverride?: unknown;
}

/** An envFrom source referencing a ConfigMap or Secret by name. */
export interface EnvFromSource {
  configMapRef?: string;
  secretRef?: string;
}

/** A ConfigMap/Secret volume mount (defaultMode is a decimal octal-ish int). */
export interface ObjectMount {
  name: string;
  mountPath: string;
  subPath?: string;
  defaultMode?: number;
}

/** Deployment update strategy (`RollingUpdate` uses maxSurge/maxUnavailable). */
export interface DeploymentStrategy {
  kind: string;
  maxSurge?: string;
  maxUnavailable?: string;
}

/** Per-role config block shared by coordinator + worker. */
export interface RoleConfig {
  replicas?: number;
  resources?: RoleResources;
  heap?: HeapConfig;
  queryMaxMemoryPerNode?: string;
  heapHeadroomPerNode?: string;
  additionalJvmConfig?: string[];
  probes?: {
    liveness?: Probe;
    readiness?: Probe;
    startup?: Probe;
  };
  // Pod placement (per role — coordinator & worker independently).
  nodeSelector?: Record<string, string>;
  tolerations?: Toleration[];
  topologySpread?: TopologySpread[];
  affinity?: Affinity;
  terminationGraceSeconds?: number;
  additionalExposedPorts?: ContainerPort[];
  // Per-role pod shape (chart coordinator.* / worker.*).
  initContainers?: MiniContainer[];
  sidecarContainers?: MiniContainer[];
  configMounts?: ObjectMount[];
  secretMounts?: ObjectMount[];
  shareProcessNamespace?: boolean;
}

/** A Trino file resource-group node (recursive tree). */
export interface Group {
  name: string;
  softMemoryLimit: string;
  hardConcurrencyLimit: number;
  maxQueued: number;
  schedulingPolicy?: string;
  subGroups?: Group[];
}

/** A resource-group selector: match criteria → target group (dot-path). */
export interface Selector {
  user?: string;
  source?: string;
  queryType?: string;
  group: string;
}

/** A file session-property manager match rule. */
export interface SessionRule {
  matchUser?: string;
  matchSource?: string;
  matchClientTags?: string[];
  properties: Record<string, string>;
}

/** Worker role config: a `RoleConfig` plus optional graceful shutdown. */
export type WorkerRoleConfig = RoleConfig & {
  gracefulShutdown?: {
    enabled: boolean;
    gracePeriodSeconds: number;
  };
};

/**
 * Fuller per-role Trino config surface, stored/sent under `trinoConfig`.
 * Cluster-wide knobs live at the top level; sizing/JVM/probes are per role.
 */
export interface TrinoConfig {
  queryMaxMemory?: string;
  logLevel?: string;
  configProperties?: Record<string, string>;
  env?: Record<string, string>;
  additionalNodeProperties?: Record<string, string>;
  additionalLogProperties?: Record<string, string>;
  eventListener?: Record<string, string>;
  faultTolerant?: {
    retryPolicy: "TASK" | "QUERY";
    exchangeStorageConnId: string;
    exchangePrefix: string;
  };
  resourceGroups?: { rootGroups: Group[]; selectors: Selector[] };
  sessionProperties?: SessionRule[];
  // Client auth: file password users (password held in an egret secret,
  // bcrypted server-side) + optional egret-JWT acceptance.
  auth?: {
    passwordUsers: { username: string; secretRef: string }[];
    jwt?: { jwksUri?: string; requiredAudience?: string };
  };
  // Per-user delegation mode for delegated BI queries. Absent = Auto (backend
  // resolves jwt→impersonate gracefully). `impersonate` = trusted egret_delegator
  // service account; `jwt` = the caller's own JWT over TLS (OIDC users only);
  // `none` = explicit open-engine opt-out (no per-user delegation).
  delegationMode?: "impersonate" | "jwt" | "none";
  // Kubernetes Ingress exposure (TLS terminates at the ingress).
  ingress?: {
    enabled: boolean;
    className?: string;
    host: string;
    tlsSecret?: string;
    /** Optional `kubernetes.io/tls` secret id backing the ingress cert. */
    tlsCertSecretRef?: string;
    annotations?: Record<string, string>;
  };
  // Cluster-wide pod & scheduling (Phase 1.5.3): pod-shape/placement knobs
  // applied to both coordinator & worker Deployments.
  image?: string;
  imagePullSecrets?: string[];
  imagePullPolicy?: "Always" | "IfNotPresent" | "Never";
  serviceAccount?: string;
  podSecurityContext?: SecurityContext;
  containerSecurityContext?: SecurityContext;
  envFrom?: EnvFromSource[];
  deploymentStrategy?: DeploymentStrategy;
  deploymentAnnotations?: Record<string, string>;
  revisionHistoryLimit?: number;
  progressDeadlineSeconds?: number;
  // LEGACY (read-only): engines stored before per-role pod shape carried these
  // at the top level. Still read when seeding old engines (falls back onto both
  // roles); new saves write them under coordinator/worker (RoleConfig) instead.
  initContainers?: MiniContainer[];
  sidecarContainers?: MiniContainer[];
  configMounts?: ObjectMount[];
  secretMounts?: ObjectMount[];
  shareProcessNamespace?: boolean;
  // Autoscaling (Phase 1.5.4): either a native HorizontalPodAutoscaler or a
  // KEDA ScaledObject on the worker Deployment. Omitted entirely when off.
  autoscaling?: {
    mode: "hpa" | "keda";
    hpa?: {
      maxReplicas: number;
      targetCpuPercent?: number;
      targetMemoryPercent?: number;
      behaviorRaw?: unknown;
    };
    keda?: {
      minReplicaCount: number;
      maxReplicaCount: number;
      pollingInterval?: number;
      cooldownPeriod?: number;
      triggers: {
        kind: string;
        cron?: {
          start: string;
          end: string;
          desiredReplicas: number;
          timezone: string;
        };
        prometheus?: {
          serverAddress: string;
          metricName: string;
          query: string;
          threshold: string;
        };
        rawOverride?: unknown;
      }[];
      fallbackRaw?: unknown;
    };
  };
  coordinator: RoleConfig;
  worker: WorkerRoleConfig;
}

/** StarRocks-specific create config: FE/CN image + replicas (resources omitted
 * from the wizard for now — the backend accepts them but defaults suffice). */
export interface StarRocksCreateConfig {
  feImage?: string;
  cnImage?: string;
  feReplicas?: number;
  cnReplicas?: number;
  // FE (metadata) / CN (compute) role sizing. Parsed by the backend as a
  // `RoleResources` (cpuRequest/cpuLimit/memRequest/memLimit); omit for default
  // sizing. On UPDATE an omitted value preserves the stored resources.
  feResources?: RoleResources;
  cnResources?: RoleResources;
  // Per-user delegation (Access parity slice 3 + default-on delegation, Task 4).
  // Omitted = Auto (backend resolves jwt→impersonate gracefully, matching
  // Trino). `none` = explicit open-engine opt-out (no per-user delegation),
  // sent LITERALLY — never omitted. `tlsEnabled` is only meaningful alongside
  // jwt/impersonate.
  delegationMode?: "jwt" | "impersonate" | "none";
  tlsEnabled?: boolean;
  // Deep-config surface (Phase 1.5). fe.conf / cn.conf overrides merged into the
  // generated config; omitting a field on UPDATE preserves the stored value.
  // A protected fe.conf key 400s. Send only when non-empty.
  feConfig?: Record<string, string>;
  cnConfig?: Record<string, string>;
  // CN horizontal autoscaling. `{ maxReplicas: 0 }` is the disabled sentinel
  // (the only way to clear a stored policy, since omit=preserve).
  cnAutoscaling?: StarRocksAutoscaling;
  // Pod & scheduling overrides (edited by the Pod tab); carried verbatim by the
  // Config tab so a config-only save never drops them.
  pod?: Record<string, unknown>;
}

export interface CreateEngineInput {
  name: string;
  /**
   * The legacy single-warehouse shape. Optional since SP3c-1 (Task 2), and
   * **mutually exclusive with `catalogs`** — the server refuses a body that
   * sends both rather than quietly dropping one of them. Update no longer
   * sends it — SP3c-2 Task 6 dropped `engines.warehouse_id`, so the field left
   * `EngineDto` and the two config forms stopped echoing it back. The create
   * wizard sends `catalogs`.
   */
  warehouseId?: string;
  /**
   * Registry catalogs to attach as part of the create, exactly one of them
   * `primary`. The server resolves EVERY id before it provisions anything, so
   * an unknown or repeated id refuses the whole create — a typo cannot leave a
   * cluster running. The primary must be an egret warehouse catalog: the
   * engine's Ranger role and (for StarRocks) its builtin storage are derived
   * from that warehouse — there is no `engines.warehouse_id` column any more
   * (SP3c-2 Task 6) to fall back to.
   *
   * CREATE ONLY: `PUT /engines/{id}` refuses a body carrying it, which is why
   * {@link UpdateEngineInput} omits the field instead of relying on callers to
   * remember.
   */
  catalogs?: NewEngineCatalog[];
  imageVersion?: string;
  // Required by the body regardless of `kind` (a known backend wart) — the
  // StarRocks create path sends the same harmless defaults the Trino wizard
  // uses and ignores them server-side.
  coordinatorReplicas: number;
  workerReplicas: number;
  cpuMin: string;
  cpuMax: string;
  memory: string;
  trinoConfig?: TrinoConfig;
  /** `"trino"` (default, omit) or `"starrocks"`. */
  kind?: "trino" | "starrocks";
  /** Only read by the backend when `kind === "starrocks"`. */
  starrocks?: StarRocksCreateConfig;
  /** Optional `kubernetes.io/tls` named-secret id backing the coordinator TLS. */
  tlsCertSecretRef?: string;
}

export async function listEngines(): Promise<Engine[]> {
  return (await controlPlaneEnginesListEngines()).data as Engine[];
}

export async function getEngine(
  id: string,
): Promise<Omit<Engine, "status"> & { status: EngineStatus }> {
  return (await controlPlaneEnginesGetEngine(id)).data as Omit<Engine, "status"> & {
    status: EngineStatus;
  };
}

/**
 * Create body minus the one field `PUT /engines/{id}` refuses. The refusal is
 * real (`update_engine` 400s on a non-empty `catalogs`), so the type makes the
 * refused body unrepresentable rather than trusting every future caller to
 * remember it.
 */
export type UpdateEngineInput = Omit<CreateEngineInput, "catalogs">;

/**
 * `POST /engines` returns {@link CreateEngineResponse} — the engine, **plus**
 * `catalogAttachErrors` naming whichever of the body's `catalogs` the engine
 * came up without.
 *
 * THE RETURN TYPE IS NOT `Engine`, AND `.data` IS NOT CAST. It used to be
 * `(…).data as Engine`, which type-checks (a `CreateEngineResponse` is an
 * `EngineDto & {…}`, so it is assignable to `EngineDto`) and silently erases
 * the only field that can report a half-attached engine. TypeScript cannot
 * catch that, which is exactly why the field is worth a response type of its
 * own — and why this function narrows on the status instead of asserting a
 * shape. `customInstance` only ever returns the success envelope (`apiFetch`
 * throws an `ApiError` on any non-2xx), so the non-201 arm is unreachable at
 * runtime; it exists so the 201 arm narrows and `res.data` needs no `as`.
 */
export async function createEngine(input: CreateEngineInput): Promise<CreateEngineResponse> {
  // FE `CreateEngineInput.trinoConfig` is a structured `TrinoConfig`; the spec
  // types the body's `trinoConfig` as an open `{ [k]: unknown }` (serde Value),
  // so cast the REQUEST at the boundary rather than widen the FE type.
  const res = await controlPlaneEnginesCreateEngine(input as unknown as CreateEngineBody);
  if (res.status !== 201) throw new ApiError(res.status, res.data.error);
  return res.data;
}

export async function updateEngine(id: string, input: UpdateEngineInput): Promise<Engine> {
  return (await controlPlaneEnginesUpdateEngine(id, input as unknown as CreateEngineBody)).data as Engine;
}

export async function deleteEngine(id: string): Promise<void> {
  await controlPlaneEnginesDeleteEngine(id);
}

export async function restartEngine(id: string): Promise<void> {
  await controlPlaneEnginesRestartEngine(id);
}

/**
 * Mint a new Trino internal-communication secret and monitor password and
 * re-apply the engine. Every pod restarts.
 */
export async function rotateEngineSecrets(id: string): Promise<void> {
  await controlPlaneEnginesRotateEngineSecrets(id);
}

// ── Engine monitor (native Trino monitor / control-plane) ────────────────────

/**
 * A single query row from the coordinator's Trino REST API.
 * `elapsed`/`cpu`/`memory` are RAW Trino strings ("1.23s", "12MB") — display
 * verbatim; `progressPercent` is a number 0..100.
 */
export interface MonitorQuery {
  id: string;
  user: string;
  state: string;
  query: string;
  elapsed: string;
  cpu: string;
  memory: string;
  progressPercent: number;
}

/** A single query with detail — adds the failure message + raw output stage tree. */
export type MonitorQueryDetail = MonitorQuery & {
  errorMessage?: string;
  // Raw Trino `outputStage` JSON — consumed by the Task 7 stage visualizer.
  outputStage?: unknown;
};

/** Cluster/node stats derived from `/v1/query` + `/v1/node`. */
export interface ClusterInfo {
  running: number;
  queued: number;
  blocked: number;
  nodes: { uri: string; recentFailures: number }[];
}

export async function listQueries(engineId: string): Promise<MonitorQuery[]> {
  return (await controlPlaneEnginesListQueries(engineId)).data as MonitorQuery[];
}

export async function getQuery(engineId: string, qid: string): Promise<MonitorQueryDetail> {
  return (await controlPlaneEnginesGetQuery(engineId, qid)).data as MonitorQueryDetail;
}

export async function killQuery(engineId: string, qid: string): Promise<void> {
  await controlPlaneEnginesKillQuery(engineId, qid);
}

export async function getCluster(engineId: string): Promise<ClusterInfo> {
  return (await controlPlaneEnginesGetCluster(engineId)).data as ClusterInfo;
}

// ── Engine pod logs (control-plane) ──────────────────────────────────────────
//
// Only the bounded `pods/{pod}/logs` GET (JSON tail) is adopted here. The
// streaming `logs/follow` + `exec` endpoints upgrade to a WebSocket and are
// opened directly with `new WebSocket(...)` in `PodLogsDialog.tsx` /
// `PodTerminalDialog.tsx` — the generated client cannot do WebSockets.

export interface PodLogs {
  logs: string;
}

export async function getPodLogs(
  id: string,
  pod: string,
  opts: { tail?: number; previous?: boolean; container?: string } = {},
): Promise<PodLogs> {
  return (await controlPlaneEnginesPodLogs(id, pod, opts)).data as PodLogs;
}
