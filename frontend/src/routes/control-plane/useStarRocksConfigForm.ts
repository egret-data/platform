import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import {
  type Engine,
  type RoleResources,
  type StarRocksAutoscaling,
  type StarRocksSpecConfig,
  type UpdateEngineInput,
  updateEngine,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import {
  DEFAULT_SR_CN_IMAGE,
  DEFAULT_SR_FE_IMAGE,
  TRINO_FIELD_DEFAULTS,
} from "./ControlPlaneEngines";

/**
 * Per-user delegation mode for a StarRocks engine. `""` (Auto) is the
 * DEFAULT — omitted from the PUT body so the backend preserves whatever mode
 * is already stored. `"jwt"`/`"impersonate"` mirror the create wizard +
 * Trino's two modes; `"none"` is an EXPLICIT open-engine opt-out, sent
 * literally (never omitted — it actually disables delegation). Held here
 * (seeded from the stored `delegation` block) so the Security tab can edit it
 * off the SAME hook instance while a config-only save preserves it.
 */
export type StarRocksDelegationOption = "" | "jwt" | "impersonate" | "none";

/** A role's resource-request form (Kubernetes resource strings kept as text). */
export interface RoleResourcesForm {
  cpuRequest: string;
  cpuLimit: string;
  memRequest: string;
  memLimit: string;
}

function resourcesToForm(r: RoleResources | null | undefined): RoleResourcesForm {
  return {
    cpuRequest: r?.cpuRequest ?? "",
    cpuLimit: r?.cpuLimit ?? "",
    memRequest: r?.memRequest ?? "",
    memLimit: r?.memLimit ?? "",
  };
}

/** Serialize a resource form to a `RoleResources`, or undefined when all blank. */
function formToResources(f: RoleResourcesForm): RoleResources | undefined {
  const out: RoleResources = {};
  if (f.cpuRequest.trim()) out.cpuRequest = f.cpuRequest.trim();
  if (f.cpuLimit.trim()) out.cpuLimit = f.cpuLimit.trim();
  if (f.memRequest.trim()) out.memRequest = f.memRequest.trim();
  if (f.memLimit.trim()) out.memLimit = f.memLimit.trim();
  return Object.keys(out).length ? out : undefined;
}

/** Parse a text field to a positive integer, or undefined when blank/invalid
 * (rejects non-integers, zero, and negatives — replica counts must be >= 1). */
function numOrUndef(s: string): number | undefined {
  const t = s.trim();
  if (t === "") return undefined;
  const n = Number(t);
  return Number.isInteger(n) && n > 0 ? n : undefined;
}

/** Canonical (comparison-stable) `RoleResources`, or `null` when it carries no
 * set field. Both the form-built and server-stored shapes normalise through this
 * so a content diff is order- and blank-insensitive. */
function normResources(r: RoleResources | null | undefined): RoleResources | null {
  if (!r) return null;
  const pick = (v: string | undefined) =>
    v && v.trim() ? v.trim() : undefined;
  const out: RoleResources = {};
  const cpuRequest = pick(r.cpuRequest);
  const cpuLimit = pick(r.cpuLimit);
  const memRequest = pick(r.memRequest);
  const memLimit = pick(r.memLimit);
  if (cpuRequest) out.cpuRequest = cpuRequest;
  if (cpuLimit) out.cpuLimit = cpuLimit;
  if (memRequest) out.memRequest = memRequest;
  if (memLimit) out.memLimit = memLimit;
  return Object.keys(out).length ? out : null;
}

/** Canonical (order-stable) string map, or `null` when empty. Both the form map
 * and the server-stored map normalise through this so a content diff is
 * key-order-insensitive. */
function normMap(
  m: Record<string, string> | null | undefined,
): Record<string, string> | null {
  if (!m) return null;
  const out: Record<string, string> = {};
  for (const k of Object.keys(m).sort()) out[k] = m[k];
  return Object.keys(out).length ? out : null;
}

/** The CN-autoscaling form fields (text, so blank/invalid is representable). */
export interface AutoscalingForm {
  enabled: boolean;
  minReplicas: string;
  maxReplicas: string;
  targetCpuPercent: string;
}

/** The `cnAutoscaling` body a save would send, or `undefined` to omit it. */
type AutoscalingBody = StarRocksAutoscaling | undefined;

/** Canonical (fixed field order) autoscaling block, or `null` when absent. Every
 * field is optional here (unlike the wire `StarRocksAutoscaling`) because this is
 * only a comparison snapshot — a stored block missing `maxReplicas` still norms. */
function normAutoscaling(
  a: Partial<StarRocksAutoscaling> | null | undefined,
): Partial<StarRocksAutoscaling> | null {
  if (!a) return null;
  const out: Partial<StarRocksAutoscaling> = {};
  if (typeof a.minReplicas === "number") out.minReplicas = a.minReplicas;
  if (typeof a.maxReplicas === "number") out.maxReplicas = a.maxReplicas;
  if (typeof a.targetCpuPercent === "number") out.targetCpuPercent = a.targetCpuPercent;
  return Object.keys(out).length ? out : null;
}

/** Seed the autoscaling form from a stored `cnAutoscaling` block. `enabled` is
 * on only when a policy exists with a real (`> 0`) `maxReplicas` — a stored
 * `{ maxReplicas: 0 }` is the disabled sentinel and seeds OFF. */
function seedAutoscaling(
  a: Partial<StarRocksAutoscaling> | null | undefined,
): AutoscalingForm {
  const str = (n: number | undefined) => (typeof n === "number" ? String(n) : "");
  return {
    enabled: !!a && typeof a.maxReplicas === "number" && a.maxReplicas > 0,
    minReplicas: str(a?.minReplicas),
    maxReplicas: str(a?.maxReplicas),
    targetCpuPercent: str(a?.targetCpuPercent),
  };
}

/**
 * The engine's cluster-wide pod & container overrides — the block the backend
 * renders onto BOTH the FE and CN StarRocks roles. Every field is optional;
 * maps/arrays are the raw k8s shapes (raw-JSON for the array/object fields so
 * the section can stay engine-agnostic). Note: `securityContext` is NOT here —
 * the StarRocks operator doesn't support it, so it's never rendered or edited.
 */
export interface StarRocksPod {
  // Open-ended: the block is an arbitrary k8s pod override, so it must remain a
  // `Record<string, unknown>` subtype for the `starrocks` PUT body. The named
  // fields below are just the ones this section edits.
  [key: string]: unknown;
  serviceAccount?: string;
  nodeSelector?: Record<string, string>;
  annotations?: Record<string, string>;
  podLabels?: Record<string, string>;
  imagePullSecrets?: string[];
  env?: unknown;
  tolerations?: unknown;
  affinity?: unknown;
  initContainers?: unknown;
  sidecars?: unknown;
}

/** True when a value is a non-empty array. */
function nonEmptyArray(v: unknown): boolean {
  return Array.isArray(v) && v.length > 0;
}

/** True when a value is a plain object with at least one key. */
function nonEmptyObject(v: unknown): boolean {
  return (
    !!v &&
    typeof v === "object" &&
    !Array.isArray(v) &&
    Object.keys(v as object).length > 0
  );
}

/**
 * Canonical (fixed field order, empty sub-fields dropped) pod block, or `null`
 * when it carries no set field. Both the form-built pod and the server-stored
 * pod normalise through this so `dirty` is order/blank-insensitive and an
 * all-empty pod → `null` (the PUT omits `pod` → the server preserves it), exactly
 * like `normMap`/`normResources`. String maps sort their keys; the raw-JSON
 * array/object fields are carried verbatim (their order is semantically
 * meaningful) but dropped when empty.
 */
function normPod(
  p: StarRocksPod | Record<string, unknown> | null | undefined,
): StarRocksPod | null {
  if (!p) return null;
  const src = p as StarRocksPod;
  const out: StarRocksPod = {};
  const sa = src.serviceAccount?.trim();
  if (sa) out.serviceAccount = sa;
  const nodeSelector = normMap(src.nodeSelector);
  if (nodeSelector) out.nodeSelector = nodeSelector;
  const annotations = normMap(src.annotations);
  if (annotations) out.annotations = annotations;
  const podLabels = normMap(src.podLabels);
  if (podLabels) out.podLabels = podLabels;
  const secrets = Array.isArray(src.imagePullSecrets)
    ? src.imagePullSecrets.map((s) => s.trim()).filter((s) => s !== "")
    : [];
  if (secrets.length) out.imagePullSecrets = secrets;
  if (nonEmptyArray(src.env)) out.env = src.env;
  if (nonEmptyArray(src.tolerations)) out.tolerations = src.tolerations;
  if (nonEmptyObject(src.affinity)) out.affinity = src.affinity;
  if (nonEmptyArray(src.initContainers)) out.initContainers = src.initContainers;
  if (nonEmptyArray(src.sidecars)) out.sidecars = src.sidecars;
  return Object.keys(out).length ? out : null;
}

/** Canonical delegation block, or `null` when no mode. `tlsEnabled` defaults to
 * true (matching the backend), so an absent flag compares equal to an explicit
 * `true`. */
function normDelegation(
  d: { mode?: string; tlsEnabled?: boolean } | null | undefined,
): { mode: string; tlsEnabled: boolean } | null {
  if (!d || !d.mode) return null;
  return { mode: d.mode, tlsEnabled: d.tlsEnabled ?? true };
}

/**
 * The `starrocks` body a save would PUT, in the omit-when-blank shape `save()`
 * sends: blank image/replicas/resources are OMITTED (an absent field means
 * "preserve the stored value" server-side), and delegation rides along for any
 * explicit mode — including `"none"`, which now disables delegation literally
 * instead of being dropped.
 */
interface SrBody {
  feImage: string;
  cnImage: string;
  feReplicas?: number;
  cnReplicas?: number;
  feResources?: RoleResources;
  cnResources?: RoleResources;
  delegationMode?: "jwt" | "impersonate" | "none";
  tlsEnabled?: boolean;
  feConfig?: Record<string, string>;
  cnConfig?: Record<string, string>;
  cnAutoscaling?: StarRocksAutoscaling;
  pod?: StarRocksPod;
}

/** An empty body (every field omitted) — resolves purely to the stored spec. */
const EMPTY_BODY: SrBody = { feImage: "", cnImage: "" };

/**
 * The config the backend would END UP with after PUTting `body` over `stored`,
 * canonicalised to a stable string. Mirrors `build_starrocks_spec`'s per-field
 * merge EXACTLY: a non-blank body value wins, else the stored value, else the
 * default (no image / replicas 1 / resources null / delegation null). Because `dirty` compares the resolved result of the ACTUAL body against
 * the resolved result of an empty body, blanking a field that was set resolves to
 * "no change" (the PUT omits it → the server preserves it) instead of a phantom
 * diff.
 */
function resolveConfig(body: SrBody, stored: StarRocksSpecConfig | undefined): string {
  // No image default here: the UPDATE route never defaults an image (neither
  // body nor stored → 400), so typing the server's create-time default onto an
  // engine that stores none is a real change, not "no change".
  const feImage = body.feImage.trim() || stored?.feImage?.trim() || "";
  const cnImage = body.cnImage.trim() || stored?.cnImage?.trim() || "";
  const feReplicas = body.feReplicas ?? stored?.feReplicas ?? 1;
  const cnReplicas = body.cnReplicas ?? stored?.cnReplicas ?? 1;
  const feResources = normResources(body.feResources ?? stored?.feResources);
  const cnResources = normResources(body.cnResources ?? stored?.cnResources);
  const delegation = body.delegationMode
    ? normDelegation({ mode: body.delegationMode, tlsEnabled: body.tlsEnabled })
    : normDelegation(stored?.delegation);
  // Deep-config surface: a non-blank body value wins, else the stored value,
  // else empty/absent — mirroring `build_starrocks_spec`'s omit=preserve merge.
  const feConfig = normMap(body.feConfig ?? stored?.feConfig);
  const cnConfig = normMap(body.cnConfig ?? stored?.cnConfig);
  const cnAutoscaling = normAutoscaling(body.cnAutoscaling ?? stored?.cnAutoscaling);
  const pod = normPod(body.pod ?? stored?.pod);
  return JSON.stringify({
    feImage,
    cnImage,
    feReplicas,
    cnReplicas,
    feResources,
    cnResources,
    delegation,
    feConfig,
    cnConfig,
    cnAutoscaling,
    pod,
  });
}

export interface StarRocksConfigForm {
  feImage: string;
  setFeImage: (v: string) => void;
  cnImage: string;
  setCnImage: (v: string) => void;
  feReplicas: string;
  setFeReplicas: (v: string) => void;
  cnReplicas: string;
  setCnReplicas: (v: string) => void;
  feResources: RoleResourcesForm;
  setFeResources: (v: RoleResourcesForm) => void;
  cnResources: RoleResourcesForm;
  setCnResources: (v: RoleResourcesForm) => void;
  // Delegation — seeded + preserved here; edited by the Security tab (next task).
  delegationMode: StarRocksDelegationOption;
  setDelegationMode: (v: StarRocksDelegationOption) => void;
  tlsEnabled: boolean;
  setTlsEnabled: (v: boolean) => void;
  // Deep config — fe.conf / cn.conf key-value overrides + CN autoscaling.
  feConfig: Record<string, string>;
  setFeConfig: (v: Record<string, string>) => void;
  cnConfig: Record<string, string>;
  setCnConfig: (v: Record<string, string>) => void;
  autoscaling: AutoscalingForm;
  setAutoscaling: (v: AutoscalingForm) => void;
  // Pod & container overrides (serviceAccount, nodeSelector, annotations,
  // podLabels, imagePullSecrets, env, tolerations, affinity, initContainers,
  // sidecars) — edited by the Pod & Containers section off this same instance.
  pod: StarRocksPod | undefined;
  // A `useState` setter (accepts a functional updater) so the section's raw-JSON
  // editors — several debounced fields feeding the one `pod` object — each merge
  // against the LATEST state instead of a captured render snapshot.
  setPod: Dispatch<SetStateAction<StarRocksPod | undefined>>;
  // True when autoscaling is enabled but Max or target CPU % is blank/invalid —
  // the tab passes this to `SaveBar`'s `disabled` so Save can't emit a broken
  // (or sentinel-colliding) policy.
  autoscalingInvalid: boolean;
  // The stored spec config — `Storage` panel reads it (storage isn't editable).
  storedConfig: StarRocksSpecConfig | undefined;
  // meta (same field names as `useEngineConfigForm` so `SaveBar` works unchanged)
  error: string | null;
  dirty: boolean;
  // Mirrors `useEngineConfigForm.editedSinceLoad` so the unsaved-changes guard can
  // read one field for either engine kind. Here it equals `dirty`: StarRocks
  // `dirty` is already `resolveConfig(body, sc) !== seed`, which settles to false on
  // load, so it never false-positives the way the Trino built-vs-stored diff can.
  editedSinceLoad: boolean;
  saving: boolean;
  saved: boolean;
  save: () => void;
}

/**
 * Single source of truth for the engine StarRocks-config form (Task 5): prefill
 * from the polled `engine.spec.starrocksConfig`, content-based `dirty` tracking,
 * and the `updateEngine` PUT. Parallel to (but independent of) the Trino
 * `useEngineConfigForm`.
 *
 * SEED FROM THE GET, NOT THE PUT RESPONSE: the update response (EngineDto) only
 * echoes Trino fields, so the form always reads `engine.spec.starrocksConfig`
 * (the query result) and re-syncs when the invalidated engine query refetches.
 */
export function useStarRocksConfigForm(
  // The engine-detail endpoint carries a richer `status` than the list `Engine`;
  // the hook never reads `status`, so accept either shape.
  engine: Omit<Engine, "status">,
): StarRocksConfigForm {
  const queryClient = useQueryClient();
  const sc = engine.spec.starrocksConfig;

  const [feImage, setFeImage] = useState(sc?.feImage ?? "");
  const [cnImage, setCnImage] = useState(sc?.cnImage ?? "");
  const [feReplicas, setFeReplicas] = useState(sc?.feReplicas?.toString() ?? "");
  const [cnReplicas, setCnReplicas] = useState(sc?.cnReplicas?.toString() ?? "");
  const [feResources, setFeResources] = useState<RoleResourcesForm>(
    resourcesToForm(sc?.feResources),
  );
  const [cnResources, setCnResources] = useState<RoleResourcesForm>(
    resourcesToForm(sc?.cnResources),
  );
  const [delegationMode, setDelegationMode] = useState<StarRocksDelegationOption>(
    (sc?.delegation?.mode as StarRocksDelegationOption) || "",
  );
  const [tlsEnabled, setTlsEnabled] = useState(sc?.delegation?.tlsEnabled ?? true);
  const [feConfig, setFeConfig] = useState<Record<string, string>>(
    () => ({ ...(sc?.feConfig ?? {}) }),
  );
  const [cnConfig, setCnConfig] = useState<Record<string, string>>(
    () => ({ ...(sc?.cnConfig ?? {}) }),
  );
  const [autoscaling, setAutoscaling] = useState<AutoscalingForm>(() =>
    seedAutoscaling(sc?.cnAutoscaling),
  );
  // Pod & scheduling — edited by the Pod & Containers section (which shares this
  // hook instance). Seeded from the stored block and normalised into the SAME
  // omit=preserve save/dirty path as every other field.
  const [pod, setPod] = useState<StarRocksPod | undefined>(
    () => (sc?.pod as StarRocksPod) ?? undefined,
  );
  const [error, setError] = useState<string | null>(null);

  // `seed` is the config the stored spec resolves to (an empty body over `sc`).
  // Re-sync the form from the spec whenever this GENUINELY changes — i.e. the
  // server-derived seed differs from the last one we applied. That covers a
  // save-triggered refetch (the submitted values land in `sc`, so the seed moves
  // and the form re-syncs → dirty settles false) and external edits, while the
  // 5s status poll returning an identical spec (same seed) is a no-op that never
  // clobbers an in-progress edit. Basing the decision on the seed — not a ref
  // frozen at first load — is what lets the baseline recover after any edit.
  const seed = resolveConfig(EMPTY_BODY, sc);
  const lastSeedRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    if (lastSeedRef.current === seed) return; // spec unchanged — preserve edits
    lastSeedRef.current = seed;
    const next = engine.spec.starrocksConfig;
    setFeImage(next?.feImage ?? "");
    setCnImage(next?.cnImage ?? "");
    setFeReplicas(next?.feReplicas?.toString() ?? "");
    setCnReplicas(next?.cnReplicas?.toString() ?? "");
    setFeResources(resourcesToForm(next?.feResources));
    setCnResources(resourcesToForm(next?.cnResources));
    setDelegationMode((next?.delegation?.mode as StarRocksDelegationOption) || "");
    setTlsEnabled(next?.delegation?.tlsEnabled ?? true);
    setFeConfig({ ...(next?.feConfig ?? {}) });
    setCnConfig({ ...(next?.cnConfig ?? {}) });
    setAutoscaling(seedAutoscaling(next?.cnAutoscaling));
    setPod((next?.pod as StarRocksPod) ?? undefined);
  }, [seed, engine.spec.starrocksConfig]);

  const builtFeResources = useMemo(
    () => formToResources(feResources),
    [feResources],
  );
  const builtCnResources = useMemo(
    () => formToResources(cnResources),
    [cnResources],
  );

  // fe.conf / cn.conf overrides: send only when non-empty (else omit → the
  // server preserves the stored map, mirroring resources).
  const builtFeConfig = normMap(feConfig) ?? undefined;
  const builtCnConfig = normMap(cnConfig) ?? undefined;

  // CN autoscaling body (omit=preserve, with a disable sentinel):
  // - enabled + valid max → { minReplicas?, maxReplicas, targetCpuPercent? }
  // - enabled, blank/invalid max → undefined (OMIT — never emit `maxReplicas: 0`
  //   here, or a half-filled enable would collide with the disable sentinel and
  //   silently clear autoscaling; Save is also gated by `autoscalingInvalid`)
  // - disabled + stored  → { maxReplicas: 0 } (the only way to clear a policy)
  // - disabled, no stored→ undefined (omit → nothing to clear)
  const hasStoredAutoscaling = normAutoscaling(sc?.cnAutoscaling) !== null;
  const builtAutoscaling: AutoscalingBody = autoscaling.enabled
    ? (() => {
        const max = numOrUndef(autoscaling.maxReplicas);
        if (max === undefined) return undefined; // half-filled enable → omit
        const out: StarRocksAutoscaling = { maxReplicas: max };
        const min = numOrUndef(autoscaling.minReplicas);
        if (min !== undefined) out.minReplicas = min;
        const cpu = numOrUndef(autoscaling.targetCpuPercent);
        if (cpu !== undefined) out.targetCpuPercent = cpu;
        return out;
      })()
    : hasStoredAutoscaling
      ? { maxReplicas: 0 }
      : undefined;

  // Save-gating: an ENABLED policy needs both a valid Max and target CPU % (the
  // backend requires targetCpuPercent > 0 when enabled). The red inline hints in
  // the Autoscaling section have teeth only because this feeds `SaveBar disabled`.
  const autoscalingInvalid =
    autoscaling.enabled &&
    (numOrUndef(autoscaling.maxReplicas) === undefined ||
      numOrUndef(autoscaling.targetCpuPercent) === undefined);

  // Pod overrides body (omit=preserve): drop empty sub-fields; an all-empty pod
  // normalises to null → omitted → the server keeps the stored block.
  const builtPod = normPod(pod) ?? undefined;

  // The exact `starrocks` body a save would PUT (omit-when-blank), so `dirty`
  // reflects what the request would ACTUALLY change — and `save()` reuses it, so
  // the two can never drift apart.
  const body: SrBody = {
    feImage: feImage.trim(),
    cnImage: cnImage.trim(),
    feReplicas: numOrUndef(feReplicas),
    cnReplicas: numOrUndef(cnReplicas),
    ...(builtFeResources ? { feResources: builtFeResources } : {}),
    ...(builtCnResources ? { cnResources: builtCnResources } : {}),
    ...(delegationMode !== "" ? { delegationMode, tlsEnabled } : {}),
    ...(builtFeConfig ? { feConfig: builtFeConfig } : {}),
    ...(builtCnConfig ? { cnConfig: builtCnConfig } : {}),
    ...(builtAutoscaling ? { cnAutoscaling: builtAutoscaling } : {}),
    // Pod overrides: send only when non-empty (else omit → the server preserves
    // the stored block, mirroring feConfig/resources).
    ...(builtPod ? { pod: builtPod } : {}),
  };

  // dirty ⇔ the resolved result of the real body differs from the resolved
  // result of an empty body (= the stored config). Blanking a field that was set
  // therefore reads as "no change" (the PUT omits it → the server preserves it),
  // so Save always settles instead of stranding a phantom diff.
  const dirty = resolveConfig(body, sc) !== seed;

  const saveMutation = useMutation<Engine, ApiError, UpdateEngineInput>({
    mutationFn: (input) => updateEngine(engine.id, input),
    onSuccess: () => {
      // `dirty` recomputes to false once the refetched spec matches the payload.
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engine.id] });
    },
  });

  async function save() {
    setError(null);
    const input: UpdateEngineInput = {
      name: engine.name,
      kind: "starrocks",
      // The SAME body `dirty` is computed from — blank image/replicas/resources
      // are omitted (preserve stored) and delegation rides along for any
      // explicit mode, including "none" (so a config-only save never drops the
      // stored delegation block, and an explicit "none" actually disables it).
      starrocks: body,
      // Required by the body regardless of kind; ignored server-side for StarRocks.
      ...TRINO_FIELD_DEFAULTS,
    };
    try {
      await saveMutation.mutateAsync(input);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to update engine.");
    }
  }

  return {
    feImage,
    setFeImage,
    cnImage,
    setCnImage,
    feReplicas,
    setFeReplicas,
    cnReplicas,
    setCnReplicas,
    feResources,
    setFeResources,
    cnResources,
    setCnResources,
    delegationMode,
    setDelegationMode,
    tlsEnabled,
    setTlsEnabled,
    feConfig,
    setFeConfig,
    cnConfig,
    setCnConfig,
    autoscaling,
    setAutoscaling,
    autoscalingInvalid,
    pod,
    setPod,
    storedConfig: sc,
    error,
    dirty,
    editedSinceLoad: dirty,
    saving: saveMutation.isPending,
    saved: saveMutation.isSuccess && !dirty,
    save,
  };
}

export { DEFAULT_SR_FE_IMAGE, DEFAULT_SR_CN_IMAGE };
