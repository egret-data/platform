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

/** Parse a text field to a positive integer, or undefined when blank/invalid. */
function numOrUndef(s: string): number | undefined {
  const t = s.trim();
  if (t === "") return undefined;
  const n = Number(t);
  return Number.isFinite(n) ? n : undefined;
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

/** A canonical, comparison-friendly snapshot of the editable StarRocks fields.
 * Built identically from the form and from the stored spec so `dirty` is a pure
 * content diff (robust to the 5s status poll handing back new object refs). */
function canonical(
  feImage: string,
  cnImage: string,
  feReplicas: string,
  cnReplicas: string,
  feResources: RoleResources | undefined,
  cnResources: RoleResources | undefined,
  delegationMode: StarRocksDelegationOption,
  tlsEnabled: boolean,
): string {
  return JSON.stringify({
    feImage: feImage.trim(),
    cnImage: cnImage.trim(),
    feReplicas: numOrUndef(feReplicas) ?? null,
    cnReplicas: numOrUndef(cnReplicas) ?? null,
    feResources: feResources ?? null,
    cnResources: cnResources ?? null,
    delegationMode,
    tlsEnabled: delegationMode === "none" ? false : tlsEnabled,
  });
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

  // Re-sync from the polled spec on external change, but never clobber unsaved
  // edits: track the canonical snapshot the form was last loaded from, and the
  // form's current snapshot — a mismatch means the user has edited (skip re-sync).
  const builtRef = useRef<string | undefined>(undefined);
  const loadedRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    if (
      loadedRef.current !== undefined &&
      builtRef.current !== undefined &&
      loadedRef.current !== builtRef.current
    ) {
      return; // unsaved edits in progress — don't overwrite them
    }
    const next = engine.spec.starrocksConfig;
    setFeImage(next?.feImage ?? "");
    setCnImage(next?.cnImage ?? "");
    setFeReplicas(next?.feReplicas?.toString() ?? "");
    setCnReplicas(next?.cnReplicas?.toString() ?? "");
    setFeResources(resourcesToForm(next?.feResources));
    setCnResources(resourcesToForm(next?.cnResources));
    setDelegationMode((next?.delegation?.mode as StarRocksDelegationOption) || "none");
    setTlsEnabled(next?.delegation?.tlsEnabled ?? true);
    loadedRef.current = canonical(
      next?.feImage ?? "",
      next?.cnImage ?? "",
      next?.feReplicas?.toString() ?? "",
      next?.cnReplicas?.toString() ?? "",
      formToResources(resourcesToForm(next?.feResources)),
      formToResources(resourcesToForm(next?.cnResources)),
      (next?.delegation?.mode as StarRocksDelegationOption) || "none",
      next?.delegation?.tlsEnabled ?? true,
    );
  }, [engine.spec]);

  const builtFeResources = useMemo(
    () => formToResources(feResources),
    [feResources],
  );
  const builtCnResources = useMemo(
    () => formToResources(cnResources),
    [cnResources],
  );

  const built = canonical(
    feImage,
    cnImage,
    feReplicas,
    cnReplicas,
    builtFeResources,
    builtCnResources,
    delegationMode,
    tlsEnabled,
  );
  builtRef.current = built;

  // Content diff vs the stored spec — false right after a save+refetch matches.
  const seed = canonical(
    sc?.feImage ?? "",
    sc?.cnImage ?? "",
    sc?.feReplicas?.toString() ?? "",
    sc?.cnReplicas?.toString() ?? "",
    formToResources(resourcesToForm(sc?.feResources)),
    formToResources(resourcesToForm(sc?.cnResources)),
    (sc?.delegation?.mode as StarRocksDelegationOption) || "none",
    sc?.delegation?.tlsEnabled ?? true,
  );
  const dirty = built !== seed;

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
      starrocks: {
        feImage: feImage.trim(),
        cnImage: cnImage.trim(),
        feReplicas: numOrUndef(feReplicas),
        cnReplicas: numOrUndef(cnReplicas),
        // Omit when empty: an absent value preserves the stored resources.
        ...(builtFeResources ? { feResources: builtFeResources } : {}),
        ...(builtCnResources ? { cnResources: builtCnResources } : {}),
        // Emit delegation only for a real mode — absent preserves the existing
        // delegation block server-side (this tab never edits delegation).
        ...(delegationMode !== "none" ? { delegationMode, tlsEnabled } : {}),
      },
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
