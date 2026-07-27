import { useEffect, useRef } from "react";

/**
 * Debounced auto-run: when `enabled` and `signature` is non-empty and differs
 * from the last run, call `onRun` after `delay` ms. An empty signature (the
 * chart isn't ready) never fires.
 */
export function useAutoRun({ enabled, signature, delay = 500, onRun }: {
  enabled: boolean;
  signature: string;
  delay?: number;
  onRun: () => void;
}) {
  const lastRef = useRef<string | null>(null);
  const cbRef = useRef(onRun);
  cbRef.current = onRun;
  useEffect(() => {
    if (!enabled || !signature || signature === lastRef.current) return;
    const t = setTimeout(() => { lastRef.current = signature; cbRef.current(); }, delay);
    return () => clearTimeout(t);
  }, [enabled, signature, delay]);
}
