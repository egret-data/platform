import { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import {
  type CreateEngineInput,
  type Engine,
  type RoleResources,
  type StarRocksSpecConfig,
  updateEngine,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import {
  DEFAULT_SR_CN_IMAGE,
  DEFAULT_SR_FE_IMAGE,
  TRINO_FIELD_DEFAULTS,
} from "./ControlPlaneEngines";

/**
 * Per-user delegation mode for a StarRocks engine. `"none"` is the slice-2
 * open-engine default (no delegation, omitted from the PUT body); `"jwt"` and
 * `"impersonate"` mirror the create wizard + Trino's two modes. Held here (seeded
 * from the stored `delegation` block) so the Security tab — a LATER task — can
 * edit it off the SAME hook instance while a config-only save preserves it.
 */
export type StarRocksDelegationOption = "none" | "jwt" | "impersonate";

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
 * "preserve the stored value" server-side), and delegation rides along only for
 * a real mode.
 */
interface SrBody {
  feImage: string;
  cnImage: string;
  feReplicas?: number;
  cnReplicas?: number;
  feResources?: RoleResources;
  cnResources?: RoleResources;
  delegationMode?: "jwt" | "impersonate";
  tlsEnabled?: boolean;
}

/** An empty body (every field omitted) — resolves purely to the stored spec. */
const EMPTY_BODY: SrBody = { feImage: "", cnImage: "" };

/**
 * The config the backend would END UP with after PUTting `body` over `stored`,
 * canonicalised to a stable string. Mirrors `build_starrocks_spec`'s per-field
 * merge EXACTLY: a non-blank body value wins, else the stored value, else the
 * create-time default (image defaults / replicas 1 / resources null / delegation
 * null). Because `dirty` compares the resolved result of the ACTUAL body against
 * the resolved result of an empty body, blanking a field that was set resolves to
 * "no change" (the PUT omits it → the server preserves it) instead of a phantom
 * diff.
 */
function resolveConfig(body: SrBody, stored: StarRocksSpecConfig | undefined): string {
  const feImage =
    body.feImage.trim() || stored?.feImage?.trim() || DEFAULT_SR_FE_IMAGE;
  const cnImage =
    body.cnImage.trim() || stored?.cnImage?.trim() || DEFAULT_SR_CN_IMAGE;
  const feReplicas = body.feReplicas ?? stored?.feReplicas ?? 1;
  const cnReplicas = body.cnReplicas ?? stored?.cnReplicas ?? 1;
  const feResources = normResources(body.feResources ?? stored?.feResources);
  const cnResources = normResources(body.cnResources ?? stored?.cnResources);
  const delegation = body.delegationMode
    ? normDelegation({ mode: body.delegationMode, tlsEnabled: body.tlsEnabled })
    : normDelegation(stored?.delegation);
  return JSON.stringify({
    feImage,
    cnImage,
    feReplicas,
    cnReplicas,
    feResources,
    cnResources,
    delegation,
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
  // The stored spec config — `Storage` panel reads it (storage isn't editable).
  storedConfig: StarRocksSpecConfig | undefined;
  // meta (same field names as `useEngineConfigForm` so `SaveBar` works unchanged)
  error: string | null;
  dirty: boolean;
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
    (sc?.delegation?.mode as StarRocksDelegationOption) || "none",
  );
  const [tlsEnabled, setTlsEnabled] = useState(sc?.delegation?.tlsEnabled ?? true);
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
    setDelegationMode((next?.delegation?.mode as StarRocksDelegationOption) || "none");
    setTlsEnabled(next?.delegation?.tlsEnabled ?? true);
  }, [seed, engine.spec.starrocksConfig]);

  const builtFeResources = useMemo(
    () => formToResources(feResources),
    [feResources],
  );
  const builtCnResources = useMemo(
    () => formToResources(cnResources),
    [cnResources],
  );

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
    ...(delegationMode !== "none" ? { delegationMode, tlsEnabled } : {}),
  };

  // dirty ⇔ the resolved result of the real body differs from the resolved
  // result of an empty body (= the stored config). Blanking a field that was set
  // therefore reads as "no change" (the PUT omits it → the server preserves it),
  // so Save always settles instead of stranding a phantom diff.
  const dirty = resolveConfig(body, sc) !== seed;

  const saveMutation = useMutation<Engine, ApiError, CreateEngineInput>({
    mutationFn: (input) => updateEngine(engine.id, input),
    onSuccess: () => {
      // `dirty` recomputes to false once the refetched spec matches the payload.
      queryClient.invalidateQueries({ queryKey: ["cp", "engine", engine.id] });
    },
  });

  async function save() {
    setError(null);
    const input: CreateEngineInput = {
      name: engine.name,
      warehouseId: engine.warehouseId ?? "",
      kind: "starrocks",
      // The SAME body `dirty` is computed from — blank image/replicas/resources
      // are omitted (preserve stored) and delegation rides along only for a real
      // mode (so a config-only save never drops the stored delegation block).
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
    storedConfig: sc,
    error,
    dirty,
    saving: saveMutation.isPending,
    saved: saveMutation.isSuccess && !dirty,
    save,
  };
}

export { DEFAULT_SR_FE_IMAGE, DEFAULT_SR_CN_IMAGE };
