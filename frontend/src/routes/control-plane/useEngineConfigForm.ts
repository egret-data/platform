import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import {
  type Affinity,
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
  type UpdateEngineInput,
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
import { diffConfig } from "./config/configDiff";

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
  // Per-role pod shape (chart coordinator.* / worker.*).
  initContainers: MiniContainer[];
  sidecarContainers: MiniContainer[];
  configMounts: ObjectMount[];
  secretMounts: ObjectMount[];
  shareProcessNamespace: boolean;
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

/** Drop container rows with neither a name nor a raw override. */
function cleanContainers(list: MiniContainer[]): MiniContainer[] {
  return list.filter((c) => c.name.trim() || c.rawOverride !== undefined);
}

/** Trim + drop object-mount rows missing a name or mount path. */
function cleanMounts(list: ObjectMount[]): ObjectMount[] {
  return list
    .map((m) => {
      const out: ObjectMount = { name: m.name.trim(), mountPath: m.mountPath.trim() };
      if (m.subPath?.trim()) out.subPath = m.subPath.trim();
      if (m.defaultMode !== undefined) out.defaultMode = m.defaultMode;
      return out;
    })
    .filter((m) => m.name && m.mountPath);
}

// The dedicated `queryMaxMemory` / `logLevel` fields were folded into the
// config.properties / log.properties tables (chart `server.config.query.maxMemory`
// = a `query.max-memory` entry; `server.log.trino.level` = an `io.trino` entry).
// Migrate a legacy engine's structured values into the maps on seed; an existing
// map entry wins over the legacy value.
function seedConfigProps(tc: TrinoConfig | undefined): Kv[] {
  return recordToKv({
    ...(tc?.queryMaxMemory ? { "query.max-memory": tc.queryMaxMemory } : {}),
    ...tc?.configProperties,
  });
}
function seedLogProps(tc: TrinoConfig | undefined): Kv[] {
  return recordToKv({
    ...(tc?.logLevel && tc.logLevel !== "INFO" ? { "io.trino": tc.logLevel } : {}),
    ...tc?.additionalLogProperties,
  });
}

function roleToForm(
  rc: RoleConfig | undefined,
  // Cluster-wide legacy fallback (old engines carried the pod-shape fields at the
  // top level of trinoConfig; apply them to a role when it has none of its own).
  tc?: TrinoConfig,
): RoleForm {
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
    initContainers: (rc?.initContainers ?? tc?.initContainers ?? []).map((c) => ({
      ...c,
    })),
    sidecarContainers: (rc?.sidecarContainers ?? tc?.sidecarContainers ?? []).map(
      (c) => ({ ...c }),
    ),
    configMounts: (rc?.configMounts ?? tc?.configMounts ?? []).map((m) => ({ ...m })),
    secretMounts: (rc?.secretMounts ?? tc?.secretMounts ?? []).map((m) => ({ ...m })),
    shareProcessNamespace:
      rc?.shareProcessNamespace ?? tc?.shareProcessNamespace ?? false,
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

  // Per-role pod shape.
  const inits = cleanContainers(f.initContainers);
  if (inits.length) rc.initContainers = inits;
  const sidecars = cleanContainers(f.sidecarContainers);
  if (sidecars.length) rc.sidecarContainers = sidecars;
  const cfgMounts = cleanMounts(f.configMounts);
  if (cfgMounts.length) rc.configMounts = cfgMounts;
  const secMounts = cleanMounts(f.secretMounts);
  if (secMounts.length) rc.secretMounts = secMounts;
  if (f.shareProcessNamespace) rc.shareProcessNamespace = true;

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

/**
 * Per-user delegation mode for delegated BI queries (spec 2026-07-15).
 * `""` (Auto) is the DEFAULT: serialization OMITS `delegationMode` so the backend
 * resolves jwt→impersonate gracefully (explicit `"jwt"` on a no-JWKS platform is a
 * deliberate 400 — Auto avoids turning the intended degrade into a hard failure).
 * The explicit modes run the query AS the caller: `impersonate` authenticates to
 * Trino as a trusted `egret_delegator` service account (works for any user); `jwt`
 * uses the caller's own short-lived JWT over TLS (OIDC users only). `"none"` is the
 * explicit open-engine opt-out (no per-user delegation).
 */
export type DelegationMode = "" | "jwt" | "impersonate" | "none";

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

  const keyCount = (r: Record<string, string> | undefined): number =>
    r ? Object.keys(r).length : 0;

  // Properties: every set key across the three raw property maps, plus the
  // three structured "Common" fields (queryMaxMemory / non-default logLevel /
  // event-listener) that live alongside the list.
  const propertiesCount =
    keyCount(cfg.configProperties) +
    keyCount(cfg.additionalNodeProperties) +
    keyCount(cfg.additionalLogProperties) +
    (keyCount(cfg.eventListener) > 0 ? 1 : 0);

  // Pod-level (cluster-wide) field-groups. init/sidecar containers, mounts and
  // shareProcessNamespace are per-role now (counted under coordinator/worker).
  const podCount = [
    cfg.serviceAccount,
    cfg.podSecurityContext,
    cfg.containerSecurityContext,
    cfg.deploymentStrategy,
    cfg.deploymentAnnotations,
    cfg.revisionHistoryLimit,
    cfg.progressDeadlineSeconds,
  ].filter(present).length;

  // Per-role pod shape (chart coordinator.* / worker.*).
  const rolePodShapeCount = (rc: RoleConfig | undefined): number =>
    rc
      ? [
          rc.initContainers,
          rc.sidecarContainers,
          rc.configMounts,
          rc.secretMounts,
          rc.shareProcessNamespace,
        ].filter(present).length
      : 0;

  return {
    image:
      (present(cfg.image) ? 1 : 0) +
      (present(cfg.imagePullPolicy) ? 1 : 0) +
      ((cfg.imagePullSecrets?.length ?? 0) > 0 ? 1 : 0),
    // The reliability retry-attempt caps live in configProperties, so they are
    // already counted by keyCount(configProperties) inside propertiesCount —
    // don't add them again here.
    server: propertiesCount + (present(cfg.faultTolerant) ? 1 : 0),
    coordinator:
      roleCoreCount(coordinator) +
      rolePlacementCount(coordinator) +
      rolePodShapeCount(coordinator),
    worker:
      roleCoreCount(worker) +
      rolePlacementCount(worker) +
      rolePodShapeCount(worker) +
      (present(worker?.gracefulShutdown) ? 1 : 0),
    workload: [cfg.resourceGroups, cfg.sessionProperties].filter(present).length,
    pod: podCount + (present(cfg.env) ? 1 : 0) + (present(cfg.envFrom) ? 1 : 0),
    autoscaling: [cfg.autoscaling].filter(present).length,
    // Security & Exposure field-groups: password users, egret-JWT acceptance,
    // a non-default per-user delegation mode, and coordinator ingress. Mirrors
    // the StarRocks security nav badge.
    security:
      ((cfg.auth?.passwordUsers?.length ?? 0) > 0 ? 1 : 0) +
      (cfg.auth?.jwt != null ? 1 : 0) +
      (present(cfg.delegationMode) ? 1 : 0) +
      (present(cfg.ingress) ? 1 : 0),
  };
}

// ── hook ─────────────────────────────────────────────────────────────────────

export interface EngineConfigForm {
  // cluster-wide
  configProps: Kv[];
  setConfigProps: Dispatch<SetStateAction<Kv[]>>;
  // Guided read/write over a raw property map, routed by config file.
  getProp: (file: "config" | "node" | "log", key: string) => string;
  setProp: (file: "config" | "node" | "log", key: string, value: string) => void;
  // Guided read/write over the event-listener property map.
  getEventListenerProp: (key: string) => string;
  setEventListenerProp: (key: string, value: string) => void;
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
  delegationMode: DelegationMode;
  setDelegationMode: (v: DelegationMode) => void;
  /**
   * The delegation mode PERSISTED on the stored engine (`trinoConfig.delegationMode`),
   * unaffected by in-progress form edits. Drives the "None" ratchet on the edit
   * screen: an engine already running jwt/impersonate can't be downgraded to open
   * (no backend teardown of `auth.jwt` / rematerialize), so "None" is disabled for
   * it. Empty (`""`) for a new or currently-open engine, where all modes are offered.
   */
  storedDelegationMode: DelegationMode;
  jwksUri: string;
  setJwksUri: (v: string) => void;
  requiredAudience: string;
  setRequiredAudience: (v: string) => void;
  ingressEnabled: boolean;
  setIngressEnabled: (v: boolean) => void;
  ingressHost: string;
  setIngressHost: (v: string) => void;
  /**
   * The ingress is on with no host. An ingress needs a host, and a config built
   * without one used to drop the ingress silently (KI-200); Save and Create are
   * blocked while this is set and the host field says why.
   */
  ingressHostMissing: boolean;
  ingressClassName: string;
  setIngressClassName: (v: string) => void;
  ingressTlsSecret: string;
  setIngressTlsSecret: (v: string) => void;
  ingressTlsCertSecretRef: string;
  setIngressTlsCertSecretRef: (v: string) => void;
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
  envFrom: EnvFromSource[];
  setEnvFrom: Dispatch<SetStateAction<EnvFromSource[]>>;
  deploymentStrategy: DeploymentStrategy;
  setDeploymentStrategy: Dispatch<SetStateAction<DeploymentStrategy>>;
  deploymentAnnotations: Kv[];
  setDeploymentAnnotations: Dispatch<SetStateAction<Kv[]>>;
  revisionHistoryLimit: string;
  setRevisionHistoryLimit: (v: string) => void;
  progressDeadlineSeconds: string;
  setProgressDeadlineSeconds: (v: string) => void;
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
  // True only after a real user edit (built-vs-load-baseline). Arms the browser
  // unsaved-changes guard without the false positives `dirty` can have on load.
  editedSinceLoad: boolean;
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

  // Cluster-wide config. query.max-memory + the io.trino log level live in the
  // config.properties / log.properties tables (legacy structured values migrated
  // in via seedConfigProps / seedLogProps).
  const [configProps, setConfigProps] = useState<Kv[]>(seedConfigProps(tc));
  const [env, setEnv] = useState<Kv[]>(recordToKv(tc?.env));

  // Raw property maps (config-file surface).
  const [eventListener, setEventListener] = useState<Kv[]>(
    recordToKv(tc?.eventListener),
  );
  const [nodeProps, setNodeProps] = useState<Kv[]>(
    recordToKv(tc?.additionalNodeProperties),
  );
  const [logProps, setLogProps] = useState<Kv[]>(seedLogProps(tc));

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
  const [coordinator, setCoordinator] = useState<RoleForm>(
    roleToForm(tc?.coordinator, tc),
  );
  const [worker, setWorker] = useState<RoleForm>(roleToForm(tc?.worker, tc));
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
  // Seed from the stored (resolved) mode — the backend now persists a concrete
  // "jwt"/"impersonate"/"none". Absent ⇒ Auto ("") for new engines.
  const [delegationMode, setDelegationMode] = useState<DelegationMode>(
    (tc?.delegationMode as DelegationMode | undefined) ?? "",
  );
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
  const [ingressTlsCertSecretRef, setIngressTlsCertSecretRef] = useState(
    tc?.ingress?.tlsCertSecretRef ?? "",
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
  // init/sidecar containers, config/secret mounts and shareProcessNamespace are
  // per-role now — they live on the coordinator/worker RoleForm state above.
  const [envFrom, setEnvFrom] = useState<EnvFromSource[]>(
    tc?.envFrom ? tc.envFrom.map((e) => ({ ...e })) : [],
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

  // `dirty` is computed further down as a content diff (built payload vs stored
  // spec) rather than tracked imperatively — the old effect-based flag flipped
  // true spuriously because the 5s status poll hands back a new-but-identical
  // `engine.spec` object each tick, whose fresh array/object references tripped
  // the change effect.
  //
  // The prefill effect needs to know whether the user has UNSAVED edits (so it
  // doesn't clobber them) — distinct from "the incoming spec differs from the
  // form". We track that with two refs: the form's current built config, and the
  // spec content the form was last synced from. "Unsaved edits" = built differs
  // from the last-synced spec (not from the newly-polled one, which is briefly
  // ahead of the form during an external update).
  const builtRef = useRef<TrinoConfig | undefined>(undefined);
  const loadedSpecRef = useRef<TrinoConfig | undefined>(undefined);
  // Built config captured at load / last external sync. The browser
  // unsaved-changes guard (`editedSinceLoad`) diffs against THIS — a built-vs-built
  // comparison, so it's immune to a sparse stored spec that merely normalises
  // differently from the built payload. Reset to undefined on a genuine re-sync so
  // the next render re-snapshots the new baseline. (Distinct from `dirty`, which is
  // built-vs-stored and legitimately drives what a Save would PUT.)
  const editBaselineRef = useRef<TrinoConfig | undefined>(undefined);

  // Keep inputs in sync when the polled engine changes (e.g. an external update).
  useEffect(() => {
    // Never clobber in-progress edits: if the form no longer matches what we last
    // loaded, the user has edited it — skip the re-sync. On first load
    // (`loadedSpecRef` unset) we always prefill.
    if (
      loadedSpecRef.current !== undefined &&
      diffConfig(loadedSpecRef.current, builtRef.current ?? {}).length > 0
    ) {
      return;
    }
    const next = engine.spec.trinoConfig;
    // A genuine content change (first load or an external update) — as opposed to
    // the 5s status poll handing back identical content — rebaselines the
    // unsaved-edits guard. Guarding on real content change means a concurrent user
    // edit landing on the same tick can't be mistaken for pristine.
    const specChanged =
      loadedSpecRef.current === undefined ||
      diffConfig(loadedSpecRef.current, next ?? {}).length > 0;
    setConfigProps(seedConfigProps(next));
    setEnv(recordToKv(next?.env));
    setEventListener(recordToKv(next?.eventListener));
    setNodeProps(recordToKv(next?.additionalNodeProperties));
    setLogProps(seedLogProps(next));
    setFtEnabled(next?.faultTolerant != null);
    setRetryPolicy(next?.faultTolerant?.retryPolicy ?? "TASK");
    setExchangeStorageConnId(next?.faultTolerant?.exchangeStorageConnId ?? "");
    setExchangePrefix(next?.faultTolerant?.exchangePrefix ?? "exchange");
    setCoordinator(roleToForm(next?.coordinator, next));
    setWorker(roleToForm(next?.worker, next));
    setGsEnabled(next?.worker?.gracefulShutdown?.enabled ?? false);
    setGracePeriod(
      next?.worker?.gracefulShutdown?.gracePeriodSeconds?.toString() ?? "",
    );
    setResourceGroups(next?.resourceGroups);
    setSessionRules(sessionRulesToForm(next?.sessionProperties));
    setPasswordUsers(next?.auth?.passwordUsers?.map((u) => ({ ...u })) ?? []);
    setJwtEnabled(next?.auth?.jwt != null);
    setDelegationMode((next?.delegationMode as DelegationMode | undefined) ?? "");
    setJwksUri(next?.auth?.jwt?.jwksUri ?? "");
    setRequiredAudience(next?.auth?.jwt?.requiredAudience ?? "");
    setIngressEnabled(next?.ingress?.enabled ?? false);
    setIngressHost(next?.ingress?.host ?? "");
    setIngressClassName(next?.ingress?.className ?? "nginx");
    setIngressTlsSecret(next?.ingress?.tlsSecret ?? "");
    setIngressTlsCertSecretRef(next?.ingress?.tlsCertSecretRef ?? "");
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
    setEnvFrom(next?.envFrom ? next.envFrom.map((e) => ({ ...e })) : []);
    setDeploymentStrategy(
      next?.deploymentStrategy
        ? { ...next.deploymentStrategy }
        : emptyDeploymentStrategy(),
    );
    setDeploymentAnnotations(recordToKv(next?.deploymentAnnotations));
    setRevisionHistoryLimit(next?.revisionHistoryLimit?.toString() ?? "");
    setProgressDeadlineSeconds(next?.progressDeadlineSeconds?.toString() ?? "");
    setAutoscaleMode(next?.autoscaling?.mode ?? "");
    setAutoscaleHpa(hpaToForm(next?.autoscaling?.hpa));
    setAutoscaleKeda(kedaToForm(next?.autoscaling?.keda));
    // Remember what the form now reflects, so the next poll/update can tell a real
    // user edit apart from an identical (or externally-changed) spec.
    loadedSpecRef.current = next;
    if (specChanged) editBaselineRef.current = undefined;
  }, [engine.spec]);

  const saveMutation = useMutation<Engine, ApiError, UpdateEngineInput>({
    mutationFn: (input) => updateEngine(engine.id, input),
    onSuccess: () => {
      // `dirty` recomputes to false once the refetched spec matches the payload.
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
    // query.max-memory + the io.trino log level are serialized as entries in
    // configProperties / additionalLogProperties (below) — no structured fields.
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
    // Omit-empty like every other field: an engine with only JWT has no
    // password users, and `passwordUsers: []` listed itself in Review changes
    // as a change nobody made (KI-198).
    if (cleanUsers.length || jwt) {
      trinoConfig.auth = {
        ...(cleanUsers.length ? { passwordUsers: cleanUsers } : {}),
        ...(jwt ? { jwt } : {}),
      };
    }

    // Per-user delegation mode. `""` (Auto) OMITS the field entirely so the
    // backend resolves jwt→impersonate gracefully (never send `""`). Any explicit
    // mode ("jwt"/"impersonate"/"none") is sent literally — this also PRESERVES an
    // existing engine's resolved mode across an unrelated UI save (the key is
    // otherwise not a form field and would be dropped on round-trip).
    if (delegationMode !== "") trinoConfig.delegationMode = delegationMode;

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
        ...(ingressTlsCertSecretRef
          ? { tlsCertSecretRef: ingressTlsCertSecretRef }
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

    // envFrom stays cluster-wide (chart top-level `envFrom`). init/sidecar
    // containers, config/secret mounts and shareProcessNamespace are now
    // per-role — serialized under coordinator/worker via formToRole.
    const cleanEnvFrom = envFrom
      .map((e) => {
        const out: EnvFromSource = {};
        if (e.configMapRef?.trim()) out.configMapRef = e.configMapRef.trim();
        if (e.secretRef?.trim()) out.secretRef = e.secretRef.trim();
        return out;
      })
      .filter((e) => Object.keys(e).length);
    if (cleanEnvFrom.length) trinoConfig.envFrom = cleanEnvFrom;

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

    // Autoscaling — dropped entirely when the mode is off.
    const autoscaling = formToAutoscaling(autoscaleMode, autoscaleHpa, autoscaleKeda);
    if (autoscaling) trinoConfig.autoscaling = autoscaling;

    return trinoConfig;
  }, [
    coordinator,
    worker,
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
    delegationMode,
    jwksUri,
    requiredAudience,
    ingressEnabled,
    ingressHost,
    ingressClassName,
    ingressTlsSecret,
    ingressTlsCertSecretRef,
    ingressAnnotations,
    serviceAccount,
    image,
    imagePullSecrets,
    imagePullPolicy,
    podSecurityContext,
    containerSecurityContext,
    envFrom,
    deploymentStrategy,
    deploymentAnnotations,
    revisionHistoryLimit,
    progressDeadlineSeconds,
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
  const getEventListenerProp = (key: string) =>
    eventListener.find((kv) => kv.key === key)?.value ?? "";
  const setEventListenerProp = (key: string, value: string) =>
    setEventListener((prev) => {
      const rest = prev.filter((kv) => kv.key !== key);
      return value.trim() === "" ? rest : [...rest, { key, value }];
    });

  async function save() {
    setError(null);
    const trinoConfig = builtTrinoConfig;

    // The wizard-era required fields still ride along; replicas now come from
    // the per-role editor, resources fall back to the stored flat values.
    const input: UpdateEngineInput = {
      name: engine.name,
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

  // Content-based dirty flag: the built (omit-empty) payload differs from the
  // stored spec. Robust to the 5s status poll returning new-but-identical object
  // references, and consistent with the Review-changes dialog (which diffs the
  // same before/after). `tc` is the stored `engine.spec.trinoConfig`.
  const dirty = diffConfig(tc ?? {}, builtTrinoConfig).length > 0;
  // Expose the current built config to the prefill effect's unsaved-edits guard.
  builtRef.current = builtTrinoConfig;

  // `editedSinceLoad`: true only once the user actually changes the form from the
  // config it loaded with. Unlike `dirty` (built-vs-stored, which can read true on
  // load when the stored spec is sparse), this compares built-vs-built against the
  // baseline captured at load, so the browser unsaved-changes prompt never fires on
  // a page the user only viewed. Lazily (re)snapshot the baseline whenever a sync
  // cleared it.
  if (editBaselineRef.current === undefined) editBaselineRef.current = builtTrinoConfig;
  const editedSinceLoad =
    diffConfig(editBaselineRef.current, builtTrinoConfig).length > 0;

  return {
    configProps,
    setConfigProps,
    getProp,
    setProp,
    getEventListenerProp,
    setEventListenerProp,
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
    delegationMode,
    setDelegationMode,
    // Ratchet source: the delegation mode PERSISTED on the stored engine, read
    // straight off the stored trinoConfig (not the moving form value). Drives the
    // "None" disable on the edit screen.
    storedDelegationMode: (tc?.delegationMode as DelegationMode | undefined) ?? "",
    jwksUri,
    setJwksUri,
    requiredAudience,
    setRequiredAudience,
    ingressEnabled,
    setIngressEnabled,
    ingressHost,
    setIngressHost,
    ingressHostMissing: ingressEnabled && ingressHost.trim() === "",
    ingressClassName,
    setIngressClassName,
    ingressTlsSecret,
    setIngressTlsSecret,
    ingressTlsCertSecretRef,
    setIngressTlsCertSecretRef,
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
    envFrom,
    setEnvFrom,
    deploymentStrategy,
    setDeploymentStrategy,
    deploymentAnnotations,
    setDeploymentAnnotations,
    revisionHistoryLimit,
    setRevisionHistoryLimit,
    progressDeadlineSeconds,
    setProgressDeadlineSeconds,
    autoscaleMode,
    setAutoscaleMode,
    autoscaleHpa,
    setAutoscaleHpa,
    autoscaleKeda,
    setAutoscaleKeda,
    error,
    dirty,
    editedSinceLoad,
    saving: saveMutation.isPending,
    // Transient: the "Saved" banner clears the moment the user edits again, so it
    // never lingers next to unsaved changes and mislead them into thinking those
    // are persisted.
    saved: saveMutation.isSuccess && !dirty,
    save,
    sectionCounts: sectionCountsValue,
    builtTrinoConfig,
    storedTrinoConfig: tc,
  };
}
