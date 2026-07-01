import { useEffect, useRef } from "react";
import { KernelConnection } from "./KernelConnection";

/**
 * Returns a memoized {@link KernelConnection} for a single open notebook.
 * The connection is created lazily on first render and disconnected on unmount.
 *
 * The page remounts per notebook (route param change → new component tree), so
 * ws/nb do not change within a mounted page; the ref is intentionally not keyed
 * on them.
 */
export function useKernel(ws: string, nb: string): KernelConnection {
  const ref = useRef<KernelConnection | null>(null);
  if (!ref.current) ref.current = new KernelConnection(ws, nb);
  useEffect(
    () => () => {
      ref.current?.disconnect();
      ref.current = null;
    },
    [],
  );
  return ref.current;
}
