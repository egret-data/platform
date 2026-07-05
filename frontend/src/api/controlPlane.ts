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
}

/** A provisioned query engine (Phase 1: Trino on Kubernetes). */
export interface Engine {
  id: string;
  name: string;
  type: string;
  namespace: string;
  warehouseId: string;
  status: string;
  spec: EngineSpec;
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
  // Kubernetes Ingress exposure (TLS terminates at the ingress).
  ingress?: {
    enabled: boolean;
    className?: string;
    host: string;
    tlsSecret?: string;
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
  initContainers?: MiniContainer[];
  sidecarContainers?: MiniContainer[];
  envFrom?: EnvFromSource[];
  configMounts?: ObjectMount[];
  secretMounts?: ObjectMount[];
  deploymentStrategy?: DeploymentStrategy;
  deploymentAnnotations?: Record<string, string>;
  revisionHistoryLimit?: number;
  progressDeadlineSeconds?: number;
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

export interface CreateEngineInput {
  name: string;
  warehouseId: string;
  imageVersion?: string;
  coordinatorReplicas: number;
  workerReplicas: number;
  cpuMin: string;
  cpuMax: string;
  memory: string;
  trinoConfig?: TrinoConfig;
}

export function listEngines(): Promise<Engine[]> {
  return apiJson<Engine[]>("/control-plane/engines");
}

export function getEngine(
  id: string,
): Promise<Omit<Engine, "status"> & { status: EngineStatus }> {
  return apiJson<Omit<Engine, "status"> & { status: EngineStatus }>(
    `/control-plane/engines/${id}`,
  );
}

export function createEngine(input: CreateEngineInput): Promise<Engine> {
  return apiJson<Engine>("/control-plane/engines", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export function updateEngine(id: string, input: CreateEngineInput): Promise<Engine> {
  return apiJson<Engine>(`/control-plane/engines/${id}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function deleteEngine(id: string): Promise<void> {
  await apiFetch(`/control-plane/engines/${id}`, { method: "DELETE" });
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

export function listQueries(engineId: string): Promise<MonitorQuery[]> {
  return apiJson<MonitorQuery[]>(`/control-plane/engines/${engineId}/queries`);
}

export function getQuery(engineId: string, qid: string): Promise<MonitorQueryDetail> {
  return apiJson<MonitorQueryDetail>(
    `/control-plane/engines/${engineId}/queries/${encodeURIComponent(qid)}`,
  );
}

export async function killQuery(engineId: string, qid: string): Promise<void> {
  await apiFetch(
    `/control-plane/engines/${engineId}/queries/${encodeURIComponent(qid)}`,
    { method: "DELETE" },
  );
}

export function getCluster(engineId: string): Promise<ClusterInfo> {
  return apiJson<ClusterInfo>(`/control-plane/engines/${engineId}/cluster`);
}

// ── Engine catalogs (multi-catalog / control-plane) ──────────────────────────

/** A catalog attached to an engine. */
export interface EngineCatalog {
  id: string;
  name: string;
  kind: string;
  isPrimary: boolean;
}

export interface AddCatalogInput {
  name: string;
  kind: string;
  config: Record<string, unknown>;
  secretRef?: string | null;
}

export function listEngineCatalogs(id: string): Promise<EngineCatalog[]> {
  return apiJson<EngineCatalog[]>(`/control-plane/engines/${id}/catalogs`);
}

export function addEngineCatalog(id: string, input: AddCatalogInput): Promise<EngineCatalog> {
  return apiJson<EngineCatalog>(`/control-plane/engines/${id}/catalogs`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export async function deleteEngineCatalog(id: string, cid: string): Promise<void> {
  await apiFetch(`/control-plane/engines/${id}/catalogs/${cid}`, { method: "DELETE" });
}
