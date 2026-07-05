import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import {
  type Affinity,
  type CreateEngineInput,
  type DeploymentStrategy,
  type Engine,
  type EnvFromSource,
  type HeapConfig,
  type MiniContainer,
  type ObjectMount,
  type Probe,
  type RoleConfig,
  type RoleResources,
  type SecurityContext,
  type Toleration,
  type TopologySpread,
  type TrinoConfig,
  updateEngine,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import {
  type Kv,
  type SessionRuleForm,
  formToSessionRules,
  kvToRecord,
  recordToKv,
  sessionRulesToForm,
} from "./EngineResourceGroups";
import { knownProperties } from "./config/knownProperties";

// Re-exported so config sub-tab components (Tasks 2-5) can import from the hook.
export type { SessionRuleForm };

// ── per-role config form model ───────────────────────────────────────────────

/** Parse a text field to a non-negative integer, or undefined when blank/invalid. */
function numOrUndef(s: string): number | undefined {
  const t = s.trim();
  if (t === "") return undefined;
  const n = Number(t);
  return Number.isFinite(n) ? n : undefined;
}

export type ProbeForm = {
  initialDelaySeconds: string;
  periodSeconds: string;
  timeoutSeconds: string;
  failureThreshold: string;
  successThreshold: string;
};

export const PROBE_FIELDS: { key: keyof ProbeForm; label: string }[] = [
  { key: "initialDelaySeconds", label: "Initial delay (s)" },
  { key: "periodSeconds", label: "Period (s)" },
  { key: "timeoutSeconds", label: "Timeout (s)" },
  { key: "failureThreshold", label: "Failure threshold" },
  { key: "successThreshold", label: "Success threshold" },
];

function probeToForm(p: Probe | undefined): ProbeForm {
  return {
    initialDelaySeconds: p?.initialDelaySeconds?.toString() ?? "",
    periodSeconds: p?.periodSeconds?.toString() ?? "",
    timeoutSeconds: p?.timeoutSeconds?.toString() ?? "",
    failureThreshold: p?.failureThreshold?.toString() ?? "",
    successThreshold: p?.successThreshold?.toString() ?? "",
  };
}

function formToProbe(f: ProbeForm): Probe | undefined {
  const p: Probe = {};
  for (const { key } of PROBE_FIELDS) {
    const n = numOrUndef(f[key]);
    if (n !== undefined) p[key] = n;
  }
  return Object.keys(p).length ? p : undefined;
}

// ── per-role pod-placement form models (Task 8 edits these) ──────────────────

/** A toleration row (numeric `tolerationSeconds` kept as text). */
export type TolerationForm = {
  key: string;
  operator: string;
  value: string;
  effect: string;
  tolerationSeconds: string;
};

/** A topology-spread-constraint row (numeric `maxSkew` kept as text). */
export type TopologySpreadForm = {
  maxSkew: string;
  topologyKey: string;
  whenUnsatisfiable: string;
  matchLabels: Kv[];
};

/** A node-affinity match-expression row (`values` as a CSV string). */
export type NodeMatchForm = {
  key: string;
  operator: string;
  values: string;
};

/** Affinity form: structured node-match + anti-affinity key + raw-JSON text. */
export type AffinityForm = {
  nodeMatch: NodeMatchForm[];
  podAntiAffinityTopologyKey: string;
  rawOverride: string;
};

/** An extra-exposed-port row (numeric `containerPort` kept as text). */
export type ContainerPortForm = {
  name: string;
  containerPort: string;
};

export type RoleForm = {
  replicas: string;
  cpuRequest: string;
  cpuLimit: string;
  memRequest: string;
  memLimit: string;
  heapMode: "fixed" | "percent";
  maxSize: string;
  minSize: string;
  maxPercent: string;
  initialPercent: string;
  queryMaxMemoryPerNode: string;
  heapHeadroomPerNode: string;
  additionalJvmConfig: string[];
  liveness: ProbeForm;
  readiness: ProbeForm;
  startup: ProbeForm;
  // Pod placement (per role).
  nodeSelector: Kv[];
  tolerations: TolerationForm[];
  topologySpread: TopologySpreadForm[];
  affinity: AffinityForm;
  terminationGraceSeconds: string;
  additionalExposedPorts: ContainerPortForm[];
};

/** Stringify a parsed raw-override object back to editable JSON text (or ""). */
function rawToText(v: unknown): string {
  return v === undefined || v === null ? "" : JSON.stringify(v, null, 2);
}

/** Parse raw-override text; returns undefined when blank or invalid. */
function textToRaw(s: string): unknown {
  const t = s.trim();
  if (t === "") return undefined;
  try {
    return JSON.parse(t);
  } catch {
    return undefined;
  }
}

function roleToForm(rc: RoleConfig | undefined): RoleForm {
  const heap = rc?.heap;
  // Prefer percent mode only when a percent is set and no fixed size is present.
  const heapMode: RoleForm["heapMode"] =
    heap?.maxPercent !== undefined && heap?.maxSize === undefined ? "percent" : "fixed";
  return {
    replicas: rc?.replicas?.toString() ?? "",
    cpuRequest: rc?.resources?.cpuRequest ?? "",
    cpuLimit: rc?.resources?.cpuLimit ?? "",
    memRequest: rc?.resources?.memRequest ?? "",
    memLimit: rc?.resources?.memLimit ?? "",
    heapMode,
    maxSize: heap?.maxSize ?? "",
    minSize: heap?.minSize ?? "",
    maxPercent: heap?.maxPercent?.toString() ?? "",
    initialPercent: heap?.initialPercent?.toString() ?? "",
    queryMaxMemoryPerNode: rc?.queryMaxMemoryPerNode ?? "",
    heapHeadroomPerNode: rc?.heapHeadroomPerNode ?? "",
    additionalJvmConfig: rc?.additionalJvmConfig ? [...rc.additionalJvmConfig] : [],
    liveness: probeToForm(rc?.probes?.liveness),
    readiness: probeToForm(rc?.probes?.readiness),
    startup: probeToForm(rc?.probes?.startup),
    nodeSelector: recordToKv(rc?.nodeSelector),
    tolerations: (rc?.tolerations ?? []).map((t) => ({
      key: t.key ?? "",
      operator: t.operator ?? "",
      value: t.value ?? "",
      effect: t.effect ?? "",
      tolerationSeconds: t.tolerationSeconds?.toString() ?? "",
    })),
    topologySpread: (rc?.topologySpread ?? []).map((t) => ({
      maxSkew: t.maxSkew?.toString() ?? "",
      topologyKey: t.topologyKey ?? "",
      whenUnsatisfiable: t.whenUnsatisfiable ?? "",
      matchLabels: recordToKv(t.matchLabels),
    })),
    affinity: {
      nodeMatch: (rc?.affinity?.nodeMatch ?? []).map((n) => ({
        key: n.key,
        operator: n.operator,
        values: (n.values ?? []).join(", "),
      })),
      podAntiAffinityTopologyKey: rc?.affinity?.podAntiAffinityTopologyKey ?? "",
      rawOverride: rawToText(rc?.affinity?.rawOverride),
    },
    terminationGraceSeconds: rc?.terminationGraceSeconds?.toString() ?? "",
    additionalExposedPorts: (rc?.additionalExposedPorts ?? []).map((p) => ({
      name: p.name,
      containerPort: p.containerPort?.toString() ?? "",
    })),
  };
}

/** Serialize a role form to a `RoleConfig`, omitting blank/unset values. */
function formToRole(f: RoleForm): RoleConfig {
  const rc: RoleConfig = {};
  const replicas = numOrUndef(f.replicas);
  if (replicas !== undefined) rc.replicas = replicas;

  const resources: RoleResources = {};
  if (f.cpuRequest.trim()) resources.cpuRequest = f.cpuRequest.trim();
  if (f.cpuLimit.trim()) resources.cpuLimit = f.cpuLimit.trim();
  if (f.memRequest.trim()) resources.memRequest = f.memRequest.trim();
  if (f.memLimit.trim()) resources.memLimit = f.memLimit.trim();
  if (Object.keys(resources).length) rc.resources = resources;

  const heap: HeapConfig = {};
  if (f.heapMode === "fixed") {
    if (f.maxSize.trim()) heap.maxSize = f.maxSize.trim();
    if (f.minSize.trim()) heap.minSize = f.minSize.trim();
  } else {
    const mp = numOrUndef(f.maxPercent);
    const ip = numOrUndef(f.initialPercent);
    if (mp !== undefined) heap.maxPercent = mp;
    if (ip !== undefined) heap.initialPercent = ip;
  }
  if (Object.keys(heap).length) rc.heap = heap;

  if (f.queryMaxMemoryPerNode.trim())
    rc.queryMaxMemoryPerNode = f.queryMaxMemoryPerNode.trim();

  if (f.heapHeadroomPerNode.trim())
    rc.heapHeadroomPerNode = f.heapHeadroomPerNode.trim();

  const jvm = f.additionalJvmConfig.map((s) => s.trim()).filter(Boolean);
  if (jvm.length) rc.additionalJvmConfig = jvm;

  const probes: NonNullable<RoleConfig["probes"]> = {};
  const liveness = formToProbe(f.liveness);
  const readiness = formToProbe(f.readiness);
  const startup = formToProbe(f.startup);
  if (liveness) probes.liveness = liveness;
  if (readiness) probes.readiness = readiness;
  if (startup) probes.startup = startup;
  if (Object.keys(probes).length) rc.probes = probes;

  // Pod placement — omit each field when empty.
  const nodeSelector = kvToRecord(f.nodeSelector);
  if (Object.keys(nodeSelector).length) rc.nodeSelector = nodeSelector;

  const tolerations = f.tolerations
    .map((t) => {
      const out: Toleration = {};
      if (t.key.trim()) out.key = t.key.trim();
      if (t.operator.trim()) out.operator = t.operator.trim();
      if (t.value.trim()) out.value = t.value.trim();
      if (t.effect.trim()) out.effect = t.effect.trim();
      const ts = numOrUndef(t.tolerationSeconds);
      if (ts !== undefined) out.tolerationSeconds = ts;
      return out;
    })
    .filter((t) => Object.keys(t).length);
  if (tolerations.length) rc.tolerations = tolerations;

  const topologySpread = f.topologySpread
    .map((t) => {
      const matchLabels = kvToRecord(t.matchLabels);
      const ts: TopologySpread = {
        maxSkew: numOrUndef(t.maxSkew) ?? 0,
        topologyKey: t.topologyKey.trim(),
        whenUnsatisfiable: t.whenUnsatisfiable.trim(),
        matchLabels,
      };
      return ts;
    })
    .filter(
      (t) =>
        t.topologyKey || t.whenUnsatisfiable || Object.keys(t.matchLabels).length,
    );
  if (topologySpread.length) rc.topologySpread = topologySpread;

  const nodeMatch = f.affinity.nodeMatch
    .map((n) => ({
      key: n.key.trim(),
      operator: n.operator.trim(),
      values: n.values
        .split(",")
        .map((v) => v.trim())
        .filter(Boolean),
    }))
    .filter((n) => n.key);
  const antiTk = f.affinity.podAntiAffinityTopologyKey.trim();
  const affinityRaw = textToRaw(f.affinity.rawOverride);
  if (nodeMatch.length || antiTk || affinityRaw !== undefined) {
    const aff: Affinity = {};
    if (nodeMatch.length) aff.nodeMatch = nodeMatch;
    if (antiTk) aff.podAntiAffinityTopologyKey = antiTk;
    if (affinityRaw !== undefined) aff.rawOverride = affinityRaw;
    rc.affinity = aff;
  }

  const grace = numOrUndef(f.terminationGraceSeconds);
  if (grace !== undefined) rc.terminationGraceSeconds = grace;

  const ports = f.additionalExposedPorts
    .map((p) => ({
      name: p.name.trim(),
      containerPort: numOrUndef(p.containerPort) ?? 0,
    }))
    .filter((p) => p.name);
  if (ports.length) rc.additionalExposedPorts = ports;

  return rc;
}

// ── cluster pod & scheduling helpers ─────────────────────────────────────────

/** An empty (all-undefined) security context — the initial hook state. */
function emptySecurityContext(): SecurityContext {
  return {};
}

/** Drop a security context to `undefined` when it carries no set field. */
function cleanSecurityContext(sc: SecurityContext): SecurityContext | undefined {
  const out: SecurityContext = {};
  if (sc.runAsUser !== undefined) out.runAsUser = sc.runAsUser;
  if (sc.runAsGroup !== undefined) out.runAsGroup = sc.runAsGroup;
  if (sc.fsGroup !== undefined) out.fsGroup = sc.fsGroup;
  if (sc.runAsNonRoot !== undefined) out.runAsNonRoot = sc.runAsNonRoot;
  if (sc.rawOverride !== undefined && sc.rawOverride !== null)
    out.rawOverride = sc.rawOverride;
  return Object.keys(out).length ? out : undefined;
}

/** An empty deployment-strategy — the initial hook state (kind unset). */
function emptyDeploymentStrategy(): DeploymentStrategy {
  return { kind: "" };
}

/** A password-user row: a username + the egret secret holding its password. */
export type PasswordUserForm = { username: string; secretRef: string };

// ── autoscaling form models ──────────────────────────────────────────────────

/** Autoscaling mode: "" is off (autoscaling dropped from the payload). */
export type AutoscaleMode = "" | "hpa" | "keda";

/** HPA form: replica cap + target utilisation (numbers kept as text) + raw behavior JSON text. */
export type HpaForm = {
  maxReplicas: string;
  targetCpuPercent: string;
  targetMemoryPercent: string;
  behaviorRaw: string;
};

/** A single KEDA trigger row. `kind` selects which structured slice (or raw text) applies. */
export type AutoscaleTriggerForm = {
  kind: string;
  cron: { start: string; end: string; desiredReplicas: string; timezone: string };
  prometheus: {
    serverAddress: string;
    metricName: string;
    query: string;
    threshold: string;
  };
  rawOverride: string;
};

/** KEDA form: replica bounds + poll/cooldown (numbers as text) + triggers + raw fallback JSON text. */
export type KedaForm = {
  minReplicaCount: string;
  maxReplicaCount: string;
  pollingInterval: string;
  cooldownPeriod: string;
  triggers: AutoscaleTriggerForm[];
  fallbackRaw: string;
};

type Autoscaling = NonNullable<TrinoConfig["autoscaling"]>;

/** A blank trigger row (defaults to a cron trigger). */
export function emptyTriggerForm(): AutoscaleTriggerForm {
  return {
    kind: "cron",
    cron: { start: "", end: "", desiredReplicas: "", timezone: "" },
    prometheus: { serverAddress: "", metricName: "", query: "", threshold: "" },
    rawOverride: "",
  };
}

function hpaToForm(hpa: Autoscaling["hpa"]): HpaForm {
  return {
    maxReplicas: hpa?.maxReplicas?.toString() ?? "",
    targetCpuPercent: hpa?.targetCpuPercent?.toString() ?? "",
    targetMemoryPercent: hpa?.targetMemoryPercent?.toString() ?? "",
    behaviorRaw: rawToText(hpa?.behaviorRaw),
  };
}

function kedaToForm(keda: Autoscaling["keda"]): KedaForm {
  return {
    minReplicaCount: keda?.minReplicaCount?.toString() ?? "",
    maxReplicaCount: keda?.maxReplicaCount?.toString() ?? "",
    pollingInterval: keda?.pollingInterval?.toString() ?? "",
    cooldownPeriod: keda?.cooldownPeriod?.toString() ?? "",
    triggers: (keda?.triggers ?? []).map((t) => ({
      kind: t.kind || "cron",
      cron: {
        start: t.cron?.start ?? "",
        end: t.cron?.end ?? "",
        desiredReplicas: t.cron?.desiredReplicas?.toString() ?? "",
        timezone: t.cron?.timezone ?? "",
      },
      prometheus: {
        serverAddress: t.prometheus?.serverAddress ?? "",
        metricName: t.prometheus?.metricName ?? "",
        query: t.prometheus?.query ?? "",
        threshold: t.prometheus?.threshold ?? "",
      },
      rawOverride: rawToText(t.rawOverride),
    })),
    fallbackRaw: rawToText(keda?.fallbackRaw),
  };
}

/** Serialize the mode + HPA/KEDA forms to an `autoscaling` slice, or undefined when off. */
function formToAutoscaling(
  mode: AutoscaleMode,
  hpa: HpaForm,
  keda: KedaForm,
): Autoscaling | undefined {
  if (mode === "hpa") {
    const out: NonNullable<Autoscaling["hpa"]> = {
      maxReplicas: numOrUndef(hpa.maxReplicas) ?? 0,
    };
    const cpu = numOrUndef(hpa.targetCpuPercent);
    if (cpu !== undefined) out.targetCpuPercent = cpu;
    const mem = numOrUndef(hpa.targetMemoryPercent);
    if (mem !== undefined) out.targetMemoryPercent = mem;
    const behavior = textToRaw(hpa.behaviorRaw);
    if (behavior !== undefined) out.behaviorRaw = behavior;
    return { mode: "hpa", hpa: out };
  }
  if (mode === "keda") {
    const triggers = keda.triggers.map((t) => {
      const trig: NonNullable<Autoscaling["keda"]>["triggers"][number] = {
        kind: t.kind,
      };
      if (t.kind === "cron") {
        trig.cron = {
          start: t.cron.start.trim(),
          end: t.cron.end.trim(),
          desiredReplicas: numOrUndef(t.cron.desiredReplicas) ?? 0,
          timezone: t.cron.timezone.trim(),
        };
      } else if (t.kind === "prometheus") {
        trig.prometheus = {
          serverAddress: t.prometheus.serverAddress.trim(),
          metricName: t.prometheus.metricName.trim(),
          query: t.prometheus.query.trim(),
          threshold: t.prometheus.threshold.trim(),
        };
      } else {
        const raw = textToRaw(t.rawOverride);
        if (raw !== undefined) trig.rawOverride = raw;
      }
      return trig;
    });
    const out: NonNullable<Autoscaling["keda"]> = {
      minReplicaCount: numOrUndef(keda.minReplicaCount) ?? 0,
      maxReplicaCount: numOrUndef(keda.maxReplicaCount) ?? 0,
      triggers,
    };
    const poll = numOrUndef(keda.pollingInterval);
    if (poll !== undefined) out.pollingInterval = poll;
    const cooldown = numOrUndef(keda.cooldownPeriod);
    if (cooldown !== undefined) out.cooldownPeriod = cooldown;
    const fallback = textToRaw(keda.fallbackRaw);
    if (fallback !== undefined) out.fallbackRaw = fallback;
    return { mode: "keda", keda: out };
  }
  return undefined;
}

// ── non-default section counts ───────────────────────────────────────────────

/**
 * Count how many field-groups are configured per Configuration section, from
 * the already-built (omit-empty) `trinoConfig`. Each group counts 1 when it is
 * present/non-empty, so a nav badge reads "how many field-groups configured".
 *
 * The badges map to the seven nav sections (mirroring the Trino Helm chart):
 * `image` (the container image string + imagePullPolicy + imagePullSecrets),
 * `server` (set keys across the config/node/log property maps + queryMaxMemory +
 * non-default logLevel + eventListener + fault-tolerant execution/exchange +
 * reliability known-properties), `coordinator` (coordinator sizing/JVM/probes +
 * placement), `worker` (same core + placement + graceful shutdown), `workload`
 * (resource groups + session properties), `pod` (cluster-wide pod/deployment
 * fields + env + init/sidecar containers + config/secret mounts), and
 * `autoscaling` (autoscaling).
 *
 * Fault-tolerant execution intentionally counts under `server` (not `workload`),
 * per the plan. Per-role placement fields count under that role's own section
 * (`coordinator`/`worker`) alongside its sizing — no field-group is
 * double-counted.
 */
export function sectionCounts(cfg: TrinoConfig): Record<string, number> {
  const present = (v: unknown): boolean =>
    v !== undefined && v !== null;

  // Coordinator/Worker core (sizing/JVM/probes) — excludes scheduling fields.
  const roleCoreCount = (rc: RoleConfig | undefined): number =>
    rc
      ? [
          rc.replicas,
          rc.resources,
          rc.heap,
          rc.probes,
          rc.queryMaxMemoryPerNode,
          rc.heapHeadroomPerNode,
          rc.additionalJvmConfig,
        ].filter(present).length
      : 0;

  // Per-role scheduling fields — counted under Placement only.
  const rolePlacementCount = (rc: RoleConfig | undefined): number =>
    rc
      ? [
          rc.nodeSelector,
          rc.tolerations,
          rc.affinity,
          rc.topologySpread,
          rc.terminationGraceSeconds,
          rc.additionalExposedPorts,
        ].filter(present).length
      : 0;

  const coordinator: RoleConfig | undefined = cfg.coordinator;
  const worker = cfg.worker;

  // Guided known-property sections: count how many of a section's known keys
  // are set (non-empty) in the raw config.properties map.
  const props = cfg.configProperties ?? {};
  const knownSectionCount = (section: string): number =>
    knownProperties.filter(
      (p) => p.section === section && (props[p.key] ?? "").trim() !== "",
    ).length;

  const keyCount = (r: Record<string, string> | undefined): number =>
    r ? Object.keys(r).length : 0;

  // Properties: every set key across the three raw property maps, plus the
  // three structured "Common" fields (queryMaxMemory / non-default logLevel /
  // event-listener) that live alongside the list.
  const propertiesCount =
    keyCount(cfg.configProperties) +
    keyCount(cfg.additionalNodeProperties) +
    keyCount(cfg.additionalLogProperties) +
    (present(cfg.queryMaxMemory) ? 1 : 0) +
    (cfg.logLevel !== undefined && cfg.logLevel !== "INFO" ? 1 : 0) +
    (keyCount(cfg.eventListener) > 0 ? 1 : 0);

  // Pod-level (cluster-wide) scheduling field-groups.
  const podCount = [
    cfg.serviceAccount,
    cfg.podSecurityContext,
    cfg.containerSecurityContext,
    cfg.deploymentStrategy,
    cfg.deploymentAnnotations,
    cfg.revisionHistoryLimit,
    cfg.progressDeadlineSeconds,
    cfg.shareProcessNamespace,
  ].filter(present).length;

  const containersMountsCount = [
    cfg.initContainers,
    cfg.sidecarContainers,
    cfg.envFrom,
    cfg.configMounts,
    cfg.secretMounts,
  ].filter(present).length;

  return {
    image:
      (present(cfg.image) ? 1 : 0) +
      (present(cfg.imagePullPolicy) ? 1 : 0) +
      ((cfg.imagePullSecrets?.length ?? 0) > 0 ? 1 : 0),
    server: propertiesCount + (present(cfg.faultTolerant) ? 1 : 0) + knownSectionCount("reliability"),
    coordinator: roleCoreCount(coordinator) + rolePlacementCount(coordinator),
    worker:
      roleCoreCount(worker) +
      rolePlacementCount(worker) +
      (present(worker?.gracefulShutdown) ? 1 : 0),
    workload: [cfg.resourceGroups, cfg.sessionProperties].filter(present).length,
    pod: podCount + (present(cfg.env) ? 1 : 0) + containersMountsCount,
    autoscaling: [cfg.autoscaling].filter(present).length,
  };
}

// ── hook ─────────────────────────────────────────────────────────────────────

export interface EngineConfigForm {
  // cluster-wide
  queryMaxMemory: string;
  setQueryMaxMemory: (v: string) => void;
  logLevel: string;
  setLogLevel: (v: string) => void;
  configProps: Kv[];
  setConfigProps: Dispatch<SetStateAction<Kv[]>>;
  // Guided read/write over a raw property map, routed by config file.
  getProp: (file: "config" | "node" | "log", key: string) => string;
  setProp: (file: "config" | "node" | "log", key: string, value: string) => void;
  env: Kv[];
  setEnv: Dispatch<SetStateAction<Kv[]>>;
  // advanced raw
  eventListener: Kv[];
  setEventListener: Dispatch<SetStateAction<Kv[]>>;
  nodeProps: Kv[];
  setNodeProps: Dispatch<SetStateAction<Kv[]>>;
  logProps: Kv[];
  setLogProps: Dispatch<SetStateAction<Kv[]>>;
  // reliability
  ftEnabled: boolean;
  setFtEnabled: (v: boolean) => void;
  retryPolicy: "TASK" | "QUERY";
  setRetryPolicy: (v: "TASK" | "QUERY") => void;
  exchangeStorageConnId: string;
  setExchangeStorageConnId: (v: string) => void;
  exchangePrefix: string;
  setExchangePrefix: (v: string) => void;
  // per-role
  coordinator: RoleForm;
  setCoordinator: Dispatch<SetStateAction<RoleForm>>;
  worker: RoleForm;
  setWorker: Dispatch<SetStateAction<RoleForm>>;
  gsEnabled: boolean;
  setGsEnabled: (v: boolean) => void;
  gracePeriod: string;
  setGracePeriod: (v: string) => void;
  // workload
  resourceGroups: TrinoConfig["resourceGroups"];
  setResourceGroups: Dispatch<SetStateAction<TrinoConfig["resourceGroups"]>>;
  sessionRules: SessionRuleForm[];
  setSessionRules: Dispatch<SetStateAction<SessionRuleForm[]>>;
  // security & exposure
  passwordUsers: PasswordUserForm[];
  setPasswordUsers: Dispatch<SetStateAction<PasswordUserForm[]>>;
  jwtEnabled: boolean;
  setJwtEnabled: (v: boolean) => void;
  jwksUri: string;
  setJwksUri: (v: string) => void;
  requiredAudience: string;
  setRequiredAudience: (v: string) => void;
  ingressEnabled: boolean;
  setIngressEnabled: (v: boolean) => void;
  ingressHost: string;
  setIngressHost: (v: string) => void;
  ingressClassName: string;
  setIngressClassName: (v: string) => void;
  ingressTlsSecret: string;
  setIngressTlsSecret: (v: string) => void;
  ingressAnnotations: Kv[];
  setIngressAnnotations: Dispatch<SetStateAction<Kv[]>>;
  // pod & scheduling (cluster-wide)
  serviceAccount: string;
  setServiceAccount: (v: string) => void;
  image: string;
  setImage: (v: string) => void;
  imagePullSecrets: string[];
  setImagePullSecrets: Dispatch<SetStateAction<string[]>>;
  imagePullPolicy: string;
  setImagePullPolicy: (v: string) => void;
  podSecurityContext: SecurityContext;
  setPodSecurityContext: Dispatch<SetStateAction<SecurityContext>>;
  containerSecurityContext: SecurityContext;
  setContainerSecurityContext: Dispatch<SetStateAction<SecurityContext>>;
  initContainers: MiniContainer[];
  setInitContainers: Dispatch<SetStateAction<MiniContainer[]>>;
  sidecarContainers: MiniContainer[];
  setSidecarContainers: Dispatch<SetStateAction<MiniContainer[]>>;
  envFrom: EnvFromSource[];
  setEnvFrom: Dispatch<SetStateAction<EnvFromSource[]>>;
  configMounts: ObjectMount[];
  setConfigMounts: Dispatch<SetStateAction<ObjectMount[]>>;
  secretMounts: ObjectMount[];
  setSecretMounts: Dispatch<SetStateAction<ObjectMount[]>>;
  deploymentStrategy: DeploymentStrategy;
  setDeploymentStrategy: Dispatch<SetStateAction<DeploymentStrategy>>;
  deploymentAnnotations: Kv[];
  setDeploymentAnnotations: Dispatch<SetStateAction<Kv[]>>;
  revisionHistoryLimit: string;
  setRevisionHistoryLimit: (v: string) => void;
  progressDeadlineSeconds: string;
  setProgressDeadlineSeconds: (v: string) => void;
  shareProcessNamespace: boolean;
  setShareProcessNamespace: (v: boolean) => void;
  // autoscaling
  autoscaleMode: AutoscaleMode;
  setAutoscaleMode: (v: AutoscaleMode) => void;
  autoscaleHpa: HpaForm;
  setAutoscaleHpa: Dispatch<SetStateAction<HpaForm>>;
  autoscaleKeda: KedaForm;
  setAutoscaleKeda: Dispatch<SetStateAction<KedaForm>>;
  // meta
  error: string | null;
  dirty: boolean;
  saving: boolean;
  saved: boolean; // last save succeeded (drives the transient "Saved" banner)
  save: () => void; // builds the payload + saveMutation.mutate; sets error on throw
  // Per-section non-default counts for nav badges, from the current built payload.
  sectionCounts: Record<string, number>;
  // The freshly-built PUT payload — `after` for the review-changes diff.
  builtTrinoConfig: TrinoConfig;
  // The stored engine spec's trinoConfig — `before` for the review-changes diff.
  storedTrinoConfig: TrinoConfig | undefined;
}

/**
 * Single source of truth for the engine Trino-config form: prefill from the
 * (polled) engine spec, `dirty` tracking, the `updateEngine` save mutation, and
 * the payload builder. The PUT payload is byte-identical to the pre-extraction
 * `handleSubmit` — the builder below was moved verbatim.
 */
export function useEngineConfigForm(
  // The engine-detail endpoint returns a richer `status` than the list `Engine`
  // type; the hook never reads `status`, so widen to accept either shape.
  engine: Omit<Engine, "status">,
): EngineConfigForm {
  const queryClient = useQueryClient();
  const tc = engine.spec.trinoConfig;

  // Cluster-wide config.
  const [queryMaxMemory, setQueryMaxMemory] = useState(tc?.queryMaxMemory ?? "");
  const [logLevel, setLogLevel] = useState(tc?.logLevel ?? "INFO");
  const [configProps, setConfigProps] = useState<Kv[]>(
    recordToKv(tc?.configProperties),
  );
  const [env, setEnv] = useState<Kv[]>(recordToKv(tc?.env));

  // Raw property maps (config-file surface).
  const [eventListener, setEventListener] = useState<Kv[]>(
    recordToKv(tc?.eventListener),
  );
  const [nodeProps, setNodeProps] = useState<Kv[]>(
    recordToKv(tc?.additionalNodeProperties),
  );
  const [logProps, setLogProps] = useState<Kv[]>(
    recordToKv(tc?.additionalLogProperties),
  );

  // Fault-tolerant execution.
  const [ftEnabled, setFtEnabled] = useState(tc?.faultTolerant != null);
  const [retryPolicy, setRetryPolicy] = useState<"TASK" | "QUERY">(
    tc?.faultTolerant?.retryPolicy ?? "TASK",
  );
  const [exchangeStorageConnId, setExchangeStorageConnId] = useState(
    tc?.faultTolerant?.exchangeStorageConnId ?? "",
  );
  const [exchangePrefix, setExchangePrefix] = useState(
    tc?.faultTolerant?.exchangePrefix ?? "exchange",
  );

  // Per-role config.
  const [coordinator, setCoordinator] = useState<RoleForm>(roleToForm(tc?.coordinator));
  const [worker, setWorker] = useState<RoleForm>(roleToForm(tc?.worker));
  const [gsEnabled, setGsEnabled] = useState(
    tc?.worker?.gracefulShutdown?.enabled ?? false,
  );
  const [gracePeriod, setGracePeriod] = useState(
    tc?.worker?.gracefulShutdown?.gracePeriodSeconds?.toString() ?? "",
  );

  // Structured configs (resource groups + session properties builders).
  const [resourceGroups, setResourceGroups] = useState(tc?.resourceGroups);
  const [sessionRules, setSessionRules] = useState<SessionRuleForm[]>(
    sessionRulesToForm(tc?.sessionProperties),
  );

  // Exposure & auth: password users, egret-JWT acceptance, and ingress.
  const [passwordUsers, setPasswordUsers] = useState<PasswordUserForm[]>(
    tc?.auth?.passwordUsers?.map((u) => ({ ...u })) ?? [],
  );
  const [jwtEnabled, setJwtEnabled] = useState(tc?.auth?.jwt != null);
  const [jwksUri, setJwksUri] = useState(tc?.auth?.jwt?.jwksUri ?? "");
  const [requiredAudience, setRequiredAudience] = useState(
    tc?.auth?.jwt?.requiredAudience ?? "",
  );
  const [ingressEnabled, setIngressEnabled] = useState(
    tc?.ingress?.enabled ?? false,
  );
  const [ingressHost, setIngressHost] = useState(tc?.ingress?.host ?? "");
  const [ingressClassName, setIngressClassName] = useState(
    tc?.ingress?.className ?? "nginx",
  );
  const [ingressTlsSecret, setIngressTlsSecret] = useState(
    tc?.ingress?.tlsSecret ?? "",
  );
  const [ingressAnnotations, setIngressAnnotations] = useState<Kv[]>(
    recordToKv(tc?.ingress?.annotations),
  );

  // Pod & scheduling (cluster-wide).
  const [serviceAccount, setServiceAccount] = useState(tc?.serviceAccount ?? "");
  const [image, setImage] = useState(tc?.image ?? "");
  const [imagePullSecrets, setImagePullSecrets] = useState<string[]>(
    tc?.imagePullSecrets ? [...tc.imagePullSecrets] : [],
  );
  const [imagePullPolicy, setImagePullPolicy] = useState(tc?.imagePullPolicy ?? "");
  const [podSecurityContext, setPodSecurityContext] = useState<SecurityContext>(
    tc?.podSecurityContext ? { ...tc.podSecurityContext } : emptySecurityContext(),
  );
  const [containerSecurityContext, setContainerSecurityContext] =
    useState<SecurityContext>(
      tc?.containerSecurityContext
        ? { ...tc.containerSecurityContext }
        : emptySecurityContext(),
    );
  const [initContainers, setInitContainers] = useState<MiniContainer[]>(
    tc?.initContainers ? tc.initContainers.map((c) => ({ ...c })) : [],
  );
  const [sidecarContainers, setSidecarContainers] = useState<MiniContainer[]>(
    tc?.sidecarContainers ? tc.sidecarContainers.map((c) => ({ ...c })) : [],
  );
  const [envFrom, setEnvFrom] = useState<EnvFromSource[]>(
    tc?.envFrom ? tc.envFrom.map((e) => ({ ...e })) : [],
  );
  const [configMounts, setConfigMounts] = useState<ObjectMount[]>(
    tc?.configMounts ? tc.configMounts.map((m) => ({ ...m })) : [],
  );
  const [secretMounts, setSecretMounts] = useState<ObjectMount[]>(
    tc?.secretMounts ? tc.secretMounts.map((m) => ({ ...m })) : [],
  );
  const [deploymentStrategy, setDeploymentStrategy] = useState<DeploymentStrategy>(
    tc?.deploymentStrategy
      ? { ...tc.deploymentStrategy }
      : emptyDeploymentStrategy(),
  );
  const [deploymentAnnotations, setDeploymentAnnotations] = useState<Kv[]>(
    recordToKv(tc?.deploymentAnnotations),
  );
  const [revisionHistoryLimit, setRevisionHistoryLimit] = useState(
    tc?.revisionHistoryLimit?.toString() ?? "",
  );
  const [progressDeadlineSeconds, setProgressDeadlineSeconds] = useState(
    tc?.progressDeadlineSeconds?.toString() ?? "",
  );
  const [shareProcessNamespace, setShareProcessNamespace] = useState(
    tc?.shareProcessNamespace ?? false,
  );

  // Autoscaling (HPA / KEDA on the worker Deployment).
  const [autoscaleMode, setAutoscaleMode] = useState<AutoscaleMode>(
    tc?.autoscaling?.mode ?? "",
  );
  const [autoscaleHpa, setAutoscaleHpa] = useState<HpaForm>(
    hpaToForm(tc?.autoscaling?.hpa),
  );
  const [autoscaleKeda, setAutoscaleKeda] = useState<KedaForm>(
    kedaToForm(tc?.autoscaling?.keda),
  );

  const [error, setError] = useState<string | null>(null);

  const [dirty, setDirty] = useState(false);
  const skipDirty = useRef(true); // skip the initial mount + every prefill
  // Latest `dirty`, readable inside the prefill effect without a stale closure.
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;

  // Keep inputs in sync when the polled engine changes (e.g. an external update).
  useEffect(() => {
    // Guard: never clobber in-progress edits. A 5s status poll re-renders with a
    // new (identical) `engine.spec`; without this the reset would discard unsaved
    // edits. First-load prefill + external updates while clean (dirty=false) still
    // apply.
    if (dirtyRef.current) return;
    const next = engine.spec.trinoConfig;
    setQueryMaxMemory(next?.queryMaxMemory ?? "");
    setLogLevel(next?.logLevel ?? "INFO");
    setConfigProps(recordToKv(next?.configProperties));
    setEnv(recordToKv(next?.env));
    setEventListener(recordToKv(next?.eventListener));
    setNodeProps(recordToKv(next?.additionalNodeProperties));
    setLogProps(recordToKv(next?.additionalLogProperties));
    setFtEnabled(next?.faultTolerant != null);
    setRetryPolicy(next?.faultTolerant?.retryPolicy ?? "TASK");
    setExchangeStorageConnId(next?.faultTolerant?.exchangeStorageConnId ?? "");
    setExchangePrefix(next?.faultTolerant?.exchangePrefix ?? "exchange");
    setCoordinator(roleToForm(next?.coordinator));
    setWorker(roleToForm(next?.worker));
    setGsEnabled(next?.worker?.gracefulShutdown?.enabled ?? false);
    setGracePeriod(
      next?.worker?.gracefulShutdown?.gracePeriodSeconds?.toString() ?? "",
    );
    setResourceGroups(next?.resourceGroups);
    setSessionRules(sessionRulesToForm(next?.sessionProperties));
    setPasswordUsers(next?.auth?.passwordUsers?.map((u) => ({ ...u })) ?? []);
    setJwtEnabled(next?.auth?.jwt != null);
    setJwksUri(next?.auth?.jwt?.jwksUri ?? "");
    setRequiredAudience(next?.auth?.jwt?.requiredAudience ?? "");
    setIngressEnabled(next?.ingress?.enabled ?? false);
    setIngressHost(next?.ingress?.host ?? "");
    setIngressClassName(next?.ingress?.className ?? "nginx");
    setIngressTlsSecret(next?.ingress?.tlsSecret ?? "");
    setIngressAnnotations(recordToKv(next?.ingress?.annotations));
    setServiceAccount(next?.serviceAccount ?? "");
    setImage(next?.image ?? "");
    setImagePullSecrets(next?.imagePullSecrets ? [...next.imagePullSecrets] : []);
    setImagePullPolicy(next?.imagePullPolicy ?? "");
    setPodSecurityContext(
      next?.podSecurityContext
        ? { ...next.podSecurityContext }
        : emptySecurityContext(),
    );
    setContainerSecurityContext(
      next?.containerSecurityContext
        ? { ...next.containerSecurityContext }
        : emptySecurityContext(),
    );
    setInitContainers(
      next?.initContainers ? next.initContainers.map((c) => ({ ...c })) : [],
    );
    setSidecarContainers(
      next?.sidecarContainers ? next.sidecarContainers.map((c) => ({ ...c })) : [],
    );
    setEnvFrom(next?.envFrom ? next.envFrom.map((e) => ({ ...e })) : []);
    setConfigMounts(
      next?.configMounts ? next.configMounts.map((m) => ({ ...m })) : [],
    );
    setSecretMounts(
      next?.secretMounts ? next.secretMounts.map((m) => ({ ...m })) : [],
    );
    setDeploymentStrategy(
      next?.deploymentStrategy
        ? { ...next.deploymentStrategy }
        : emptyDeploymentStrategy(),
    );
    setDeploymentAnnotations(recordToKv(next?.deploymentAnnotations));
    setRevisionHistoryLimit(next?.revisionHistoryLimit?.toString() ?? "");
    setProgressDeadlineSeconds(next?.progressDeadlineSeconds?.toString() ?? "");
    setShareProcessNamespace(next?.shareProcessNamespace ?? false);
    setAutoscaleMode(next?.autoscaling?.mode ?? "");
    setAutoscaleHpa(hpaToForm(next?.autoscaling?.hpa));
    setAutoscaleKeda(kedaToForm(next?.autoscaling?.keda));
    // A re-sync from the server must not mark the form dirty.
    skipDirty.current = true;
    setDirty(false);
  }, [engine.spec]);

  // Mark dirty on any user edit (the initial mount + every prefill are skipped).
  useEffect(() => {
    if (skipDirty.current) {
      skipDirty.current = false;
      return;
    }
    setDirty(true);
  }, [
    queryMaxMemory,
    logLevel,
    configProps,
    env,
    eventListener,
    nodeProps,
    logProps,
    ftEnabled,
    retryPolicy,
    exchangeStorageConnId,
    exchangePrefix,
    coordinator,
    worker,
    gsEnabled,
    gracePeriod,
    resourceGroups,
    sessionRules,
    passwordUsers,
    jwtEnabled,
    jwksUri,
    requiredAudience,
    ingressEnabled,
    ingressHost,
    ingressClassName,
    ingressTlsSecret,
    ingressAnnotations,
    serviceAccount,
    image,
    imagePullSecrets,
    imagePullPolicy,
    podSecurityContext,
    containerSecurityContext,
    initContainers,
    sidecarContainers,
    envFrom,
    configMounts,
    secretMounts,
    deploymentStrategy,
    deploymentAnnotations,
    revisionHistoryLimit,
    progressDeadlineSeconds,
    shareProcessNamespace,
    autoscaleMode,
    autoscaleHpa,
    autoscaleKeda,
  ]);

  const saveMutation = useMutation<Engine, ApiError, CreateEngineInput>({
    mutationFn: (input) => updateEngine(engine.id, input),
    onSuccess: () => {
      setDirty(false);
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engine.id] });
    },
  });

  // Build the omit-empty `trinoConfig` payload from the current form state. This
  // block was moved verbatim from `save()`; its output is byte-identical. It is
  // memoised so `sectionCounts` (nav badges) can read it without rebuilding.
  const builtTrinoConfig = useMemo<TrinoConfig>(() => {
    const trinoConfig: TrinoConfig = {
      coordinator: formToRole(coordinator),
      worker: formToRole(worker),
    };
    if (queryMaxMemory.trim()) trinoConfig.queryMaxMemory = queryMaxMemory.trim();
    if (logLevel) trinoConfig.logLevel = logLevel;
    const cp = kvToRecord(configProps);
    if (Object.keys(cp).length) trinoConfig.configProperties = cp;
    const ev = kvToRecord(env);
    if (Object.keys(ev).length) trinoConfig.env = ev;

    const el = kvToRecord(eventListener);
    if (Object.keys(el).length) trinoConfig.eventListener = el;
    const np = kvToRecord(nodeProps);
    if (Object.keys(np).length) trinoConfig.additionalNodeProperties = np;
    const lp = kvToRecord(logProps);
    if (Object.keys(lp).length) trinoConfig.additionalLogProperties = lp;

    if (ftEnabled && exchangeStorageConnId) {
      trinoConfig.faultTolerant = {
        retryPolicy,
        exchangeStorageConnId,
        exchangePrefix: exchangePrefix.trim() || "exchange",
      };
    }

    // Structured builders — omit entirely when empty.
    if (resourceGroups && resourceGroups.rootGroups.length)
      trinoConfig.resourceGroups = resourceGroups;
    const sr = formToSessionRules(sessionRules);
    if (sr.length) trinoConfig.sessionProperties = sr;

    if (gsEnabled) {
      trinoConfig.worker.gracefulShutdown = {
        enabled: true,
        gracePeriodSeconds: numOrUndef(gracePeriod) ?? 0,
      };
    }

    // Exposure & auth. Password users hold a secret ref (never plaintext);
    // JWT is undefined when off; omit `auth` entirely when neither is set.
    const cleanUsers = passwordUsers
      .map((u) => ({ username: u.username.trim(), secretRef: u.secretRef }))
      .filter((u) => u.username && u.secretRef);
    const jwt = jwtEnabled
      ? {
          ...(jwksUri.trim() ? { jwksUri: jwksUri.trim() } : {}),
          ...(requiredAudience.trim()
            ? { requiredAudience: requiredAudience.trim() }
            : {}),
        }
      : undefined;
    if (cleanUsers.length || jwt) {
      trinoConfig.auth = {
        passwordUsers: cleanUsers,
        ...(jwt ? { jwt } : {}),
      };
    }

    if (ingressEnabled && ingressHost.trim()) {
      const anns = kvToRecord(ingressAnnotations);
      trinoConfig.ingress = {
        enabled: true,
        host: ingressHost.trim(),
        ...(ingressClassName.trim()
          ? { className: ingressClassName.trim() }
          : {}),
        ...(ingressTlsSecret.trim()
          ? { tlsSecret: ingressTlsSecret.trim() }
          : {}),
        ...(Object.keys(anns).length ? { annotations: anns } : {}),
      };
    }

    // Pod & scheduling (cluster-wide) — same omit-empty style: skip empty
    // arrays/objects/blanks so an untouched engine's payload is unchanged.
    if (serviceAccount.trim()) trinoConfig.serviceAccount = serviceAccount.trim();
    if (image.trim()) trinoConfig.image = image.trim();
    const pullSecrets = imagePullSecrets.map((s) => s.trim()).filter(Boolean);
    if (pullSecrets.length) trinoConfig.imagePullSecrets = pullSecrets;
    if (imagePullPolicy)
      trinoConfig.imagePullPolicy =
        imagePullPolicy as TrinoConfig["imagePullPolicy"];

    const podSc = cleanSecurityContext(podSecurityContext);
    if (podSc) trinoConfig.podSecurityContext = podSc;
    const ctrSc = cleanSecurityContext(containerSecurityContext);
    if (ctrSc) trinoConfig.containerSecurityContext = ctrSc;

    const cleanContainers = (list: MiniContainer[]): MiniContainer[] =>
      list.filter((c) => c.name.trim() || c.rawOverride !== undefined);
    const inits = cleanContainers(initContainers);
    if (inits.length) trinoConfig.initContainers = inits;
    const sidecars = cleanContainers(sidecarContainers);
    if (sidecars.length) trinoConfig.sidecarContainers = sidecars;

    const cleanEnvFrom = envFrom
      .map((e) => {
        const out: EnvFromSource = {};
        if (e.configMapRef?.trim()) out.configMapRef = e.configMapRef.trim();
        if (e.secretRef?.trim()) out.secretRef = e.secretRef.trim();
        return out;
      })
      .filter((e) => Object.keys(e).length);
    if (cleanEnvFrom.length) trinoConfig.envFrom = cleanEnvFrom;

    const cleanMounts = (list: ObjectMount[]): ObjectMount[] =>
      list
        .map((m) => {
          const out: ObjectMount = { name: m.name.trim(), mountPath: m.mountPath.trim() };
          if (m.subPath?.trim()) out.subPath = m.subPath.trim();
          if (m.defaultMode !== undefined) out.defaultMode = m.defaultMode;
          return out;
        })
        .filter((m) => m.name && m.mountPath);
    const cfgMounts = cleanMounts(configMounts);
    if (cfgMounts.length) trinoConfig.configMounts = cfgMounts;
    const secMounts = cleanMounts(secretMounts);
    if (secMounts.length) trinoConfig.secretMounts = secMounts;

    if (deploymentStrategy.kind) {
      const ds: DeploymentStrategy = { kind: deploymentStrategy.kind };
      if (deploymentStrategy.maxSurge?.trim())
        ds.maxSurge = deploymentStrategy.maxSurge.trim();
      if (deploymentStrategy.maxUnavailable?.trim())
        ds.maxUnavailable = deploymentStrategy.maxUnavailable.trim();
      trinoConfig.deploymentStrategy = ds;
    }

    const depAnns = kvToRecord(deploymentAnnotations);
    if (Object.keys(depAnns).length) trinoConfig.deploymentAnnotations = depAnns;

    const revLimit = numOrUndef(revisionHistoryLimit);
    if (revLimit !== undefined) trinoConfig.revisionHistoryLimit = revLimit;
    const progDeadline = numOrUndef(progressDeadlineSeconds);
    if (progDeadline !== undefined)
      trinoConfig.progressDeadlineSeconds = progDeadline;
    if (shareProcessNamespace) trinoConfig.shareProcessNamespace = true;

    // Autoscaling — dropped entirely when the mode is off.
    const autoscaling = formToAutoscaling(autoscaleMode, autoscaleHpa, autoscaleKeda);
    if (autoscaling) trinoConfig.autoscaling = autoscaling;

    return trinoConfig;
  }, [
    coordinator,
    worker,
    queryMaxMemory,
    logLevel,
    configProps,
    env,
    eventListener,
    nodeProps,
    logProps,
    ftEnabled,
    retryPolicy,
    exchangeStorageConnId,
    exchangePrefix,
    resourceGroups,
    sessionRules,
    gsEnabled,
    gracePeriod,
    passwordUsers,
    jwtEnabled,
    jwksUri,
    requiredAudience,
    ingressEnabled,
    ingressHost,
    ingressClassName,
    ingressTlsSecret,
    ingressAnnotations,
    serviceAccount,
    image,
    imagePullSecrets,
    imagePullPolicy,
    podSecurityContext,
    containerSecurityContext,
    initContainers,
    sidecarContainers,
    envFrom,
    configMounts,
    secretMounts,
    deploymentStrategy,
    deploymentAnnotations,
    revisionHistoryLimit,
    progressDeadlineSeconds,
    shareProcessNamespace,
    autoscaleMode,
    autoscaleHpa,
    autoscaleKeda,
  ]);

  const sectionCountsValue = useMemo(
    () => sectionCounts(builtTrinoConfig),
    [builtTrinoConfig],
  );

  // Guided helpers over a raw property map, routed by config file: read a key's
  // value (or "") and upsert it (blank value removes the entry). The payload
  // shape is unchanged — values still live in the corresponding Kv[] state.
  const mapFor = (file: "config" | "node" | "log") =>
    file === "node"
      ? ([nodeProps, setNodeProps] as const)
      : file === "log"
        ? ([logProps, setLogProps] as const)
        : ([configProps, setConfigProps] as const);
  const getProp = (file: "config" | "node" | "log", key: string) =>
    mapFor(file)[0].find((kv) => kv.key === key)?.value ?? "";
  const setProp = (file: "config" | "node" | "log", key: string, value: string) => {
    const set = mapFor(file)[1];
    set((prev) => {
      const rest = prev.filter((kv) => kv.key !== key);
      return value.trim() === "" ? rest : [...rest, { key, value }];
    });
  };

  async function save() {
    setError(null);
    const trinoConfig = builtTrinoConfig;

    // The wizard-era required fields still ride along; replicas now come from
    // the per-role editor, resources fall back to the stored flat values.
    const input: CreateEngineInput = {
      name: engine.name,
      warehouseId: engine.warehouseId,
      coordinatorReplicas:
        numOrUndef(coordinator.replicas) ?? engine.spec.coordinatorReplicas,
      workerReplicas: numOrUndef(worker.replicas) ?? engine.spec.workerReplicas,
      cpuMin: engine.spec.cpuMin,
      cpuMax: engine.spec.cpuMax,
      memory: engine.spec.memory,
      trinoConfig,
    };
    try {
      await saveMutation.mutateAsync(input);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to update engine.");
    }
  }

  return {
    queryMaxMemory,
    setQueryMaxMemory,
    logLevel,
    setLogLevel,
    configProps,
    setConfigProps,
    getProp,
    setProp,
    env,
    setEnv,
    eventListener,
    setEventListener,
    nodeProps,
    setNodeProps,
    logProps,
    setLogProps,
    ftEnabled,
    setFtEnabled,
    retryPolicy,
    setRetryPolicy,
    exchangeStorageConnId,
    setExchangeStorageConnId,
    exchangePrefix,
    setExchangePrefix,
    coordinator,
    setCoordinator,
    worker,
    setWorker,
    gsEnabled,
    setGsEnabled,
    gracePeriod,
    setGracePeriod,
    resourceGroups,
    setResourceGroups,
    sessionRules,
    setSessionRules,
    passwordUsers,
    setPasswordUsers,
    jwtEnabled,
    setJwtEnabled,
    jwksUri,
    setJwksUri,
    requiredAudience,
    setRequiredAudience,
    ingressEnabled,
    setIngressEnabled,
    ingressHost,
    setIngressHost,
    ingressClassName,
    setIngressClassName,
    ingressTlsSecret,
    setIngressTlsSecret,
    ingressAnnotations,
    setIngressAnnotations,
    serviceAccount,
    setServiceAccount,
    image,
    setImage,
    imagePullSecrets,
    setImagePullSecrets,
    imagePullPolicy,
    setImagePullPolicy,
    podSecurityContext,
    setPodSecurityContext,
    containerSecurityContext,
    setContainerSecurityContext,
    initContainers,
    setInitContainers,
    sidecarContainers,
    setSidecarContainers,
    envFrom,
    setEnvFrom,
    configMounts,
    setConfigMounts,
    secretMounts,
    setSecretMounts,
    deploymentStrategy,
    setDeploymentStrategy,
    deploymentAnnotations,
    setDeploymentAnnotations,
    revisionHistoryLimit,
    setRevisionHistoryLimit,
    progressDeadlineSeconds,
    setProgressDeadlineSeconds,
    shareProcessNamespace,
    setShareProcessNamespace,
    autoscaleMode,
    setAutoscaleMode,
    autoscaleHpa,
    setAutoscaleHpa,
    autoscaleKeda,
    setAutoscaleKeda,
    error,
    dirty,
    saving: saveMutation.isPending,
    saved: saveMutation.isSuccess,
    save,
    sectionCounts: sectionCountsValue,
    builtTrinoConfig,
    storedTrinoConfig: tc,
  };
}
