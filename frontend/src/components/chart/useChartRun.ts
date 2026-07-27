import { useRef, useState } from "react";
import { toast } from "sonner";
import type { QuerySpec } from "@/api/charts";
import { useAutoRun } from "./useAutoRun";

interface Args {
  datasetId: string;
  autoRun: boolean;
  /** The spec the current config would run. Called during render — keep it pure. */
  buildSpec: () => QuerySpec;
  /**
   * Labels of the fields still missing; a non-empty list blocks running. A thunk rather
   * than an array so the caller can hoist this hook above the declarations it reads.
   */
  missing: () => string[];
  /** Runs right after a successful run (manual or auto) — for snapshotting what ran. */
  afterRun?: () => void;
}

/**
 * Owns "what has actually run": the spec in flight, the nonce that forces a refetch, and
 * the staleness comparison the preview uses to keep the previous result on screen (#345).
 * The shell passes config in and gets run state back; it holds none of this itself.
 */
export function useChartRun({ datasetId, autoRun, buildSpec, missing, afterRun }: Args) {
  const [ranSpec, setRanSpec] = useState<QuerySpec | null>(null);
  const [runNonce, setRunNonce] = useState(0);
  const lastRunSig = useRef("");

  // Empty whenever the chart isn't runnable, so auto-run never fires an invalid query.
  const signature = datasetId && missing().length === 0 ? JSON.stringify({ datasetId, spec: buildSpec() }) : "";

  function run() {
    setRanSpec(null);
    if (!datasetId) { toast.error("Pick a dataset first"); return; }
    const gaps = missing();
    if (gaps.length) { toast.error(`This chart needs ${gaps.join(" and ")}`); return; }
    setRanSpec(buildSpec());
    setRunNonce((n) => n + 1);
    // Stamp the signature so a pending auto-run timer for this same query
    // (e.g. the user clicked Run before the debounce elapsed) won't re-fire it.
    lastRunSig.current = signature;
    afterRun?.();
  }

  useAutoRun({ enabled: autoRun, signature, onRun: run, lastRunRef: lastRunSig });

  return {
    ranSpec,
    runNonce,
    run,
    isStale: signature !== lastRunSig.current,
    reset: () => setRanSpec(null),
  };
}
