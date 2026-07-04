import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useState,
} from "react";

/**
 * Tracks which raw-JSON fields currently hold invalid JSON, keyed by a stable
 * field id. Any `RawJsonField` under a `ValidityProvider` registers/clears its
 * id here; `anyInvalid` is true while at least one field is invalid, which the
 * Save gate reads to block a bad payload. `useValidity()` is null-safe, so a
 * `RawJsonField` still works when rendered without a provider.
 */
interface Registry {
  setInvalid: (id: string, invalid: boolean) => void;
  anyInvalid: boolean;
}

const Ctx = createContext<Registry | null>(null);

export function ValidityProvider({
  children,
  onAnyInvalidChange,
}: {
  children: React.ReactNode;
  onAnyInvalidChange?: (v: boolean) => void;
}) {
  const [ids, setIds] = useState<Set<string>>(() => new Set());
  const setInvalid = useCallback(
    (id: string, invalid: boolean) =>
      setIds((prev) => {
        const next = new Set(prev);
        if (invalid) next.add(id);
        else next.delete(id);
        return next;
      }),
    [],
  );
  const anyInvalid = ids.size > 0;
  useEffect(
    () => onAnyInvalidChange?.(anyInvalid),
    [anyInvalid, onAnyInvalidChange],
  );
  return createElement(Ctx.Provider, { value: { setInvalid, anyInvalid } }, children);
}

export function useValidity() {
  return useContext(Ctx);
}
