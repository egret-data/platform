import { useEffect, useRef, type MutableRefObject } from "react";

/**
 * Debounced auto-run: when `enabled` and `signature` is non-empty and differs
 * from the last run, call `onRun` after `delay` ms. An empty signature (the
 * chart isn't ready) never fires.
 *
 * `lastRunRef` is shared with the caller's manual Run path (rather than kept
 * internally) so a manual run that stamps it while a debounce timer is
 * pending cancels that pending auto-fire instead of re-running the same
 * query a second time.
 */
export function useAutoRun({ enabled, signature, delay = 500, onRun, lastRunRef }: {
  enabled: boolean;
  signature: string;
  delay?: number;
  onRun: () => void;
  lastRunRef: MutableRefObject<string>;
}) {
  const cbRef = useRef(onRun);
  cbRef.current = onRun;
  useEffect(() => {
    if (!enabled || !signature || signature === lastRunRef.current) return;
    const t = setTimeout(() => {
      // re-check: a manual run may have stamped lastRunRef in the meantime
      if (signature !== lastRunRef.current) { lastRunRef.current = signature; cbRef.current(); }
    }, delay);
    return () => clearTimeout(t);
  }, [enabled, signature, delay, lastRunRef]);
}
