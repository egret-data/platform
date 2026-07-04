import { useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import {
  type CreateEngineInput,
  type Engine,
  type HeapConfig,
  type Probe,
  type RoleConfig,
  type RoleResources,
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
};

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

  return rc;
}

/** A password-user row: a username + the egret secret holding its password. */
export type PasswordUserForm = { username: string; secretRef: string };

// ── hook ─────────────────────────────────────────────────────────────────────

export interface EngineConfigForm {
  // cluster-wide
  queryMaxMemory: string;
  setQueryMaxMemory: (v: string) => void;
  logLevel: string;
  setLogLevel: (v: string) => void;
  configProps: Kv[];
  setConfigProps: Dispatch<SetStateAction<Kv[]>>;
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
  // meta
  error: string | null;
  dirty: boolean;
  saving: boolean;
  saved: boolean; // last save succeeded (drives the transient "Saved" banner)
  save: () => void; // builds the payload + saveMutation.mutate; sets error on throw
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

  const [error, setError] = useState<string | null>(null);

  const [dirty, setDirty] = useState(false);
  const skipDirty = useRef(true); // skip the initial mount + every prefill

  // Keep inputs in sync when the polled engine changes (e.g. an external update).
  useEffect(() => {
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
  ]);

  const saveMutation = useMutation<Engine, ApiError, CreateEngineInput>({
    mutationFn: (input) => updateEngine(engine.id, input),
    onSuccess: () => {
      setDirty(false);
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engine.id] });
    },
  });

  async function save() {
    setError(null);

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
    error,
    dirty,
    saving: saveMutation.isPending,
    saved: saveMutation.isSuccess,
    save,
  };
}
