import { useCallback, useMemo, useState } from "react";

/** Multi-select state for a list of `{id}` rows. `rows` should be the currently visible set. */
export function useBulkSelect<T extends { id: string }>(rows: T[]) {
  const [bulkMode, setBulkModeRaw] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const toggle = useCallback((id: string) => {
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(id)) {
        n.delete(id);
      } else {
        n.add(id);
      }
      return n;
    });
  }, []);

  const clear = useCallback(() => setSelected(new Set()), []);

  const allSelected = rows.length > 0 && rows.every((r) => selected.has(r.id));
  const someSelected = rows.some((r) => selected.has(r.id));

  const toggleAll = useCallback(() => {
    setSelected((s) => {
      const everyOn = rows.length > 0 && rows.every((r) => s.has(r.id));
      return everyOn ? new Set() : new Set(rows.map((r) => r.id));
    });
  }, [rows]);

  const setBulkMode = useCallback((v: boolean) => {
    setBulkModeRaw(v);
    if (!v) setSelected(new Set());
  }, []);

  // ids that still exist in the visible rows (selection can outlive filtering)
  const selectedIds = useMemo(
    () => rows.filter((r) => selected.has(r.id)).map((r) => r.id),
    [rows, selected],
  );

  return {
    bulkMode,
    setBulkMode,
    selected,
    selectedIds,
    toggle,
    toggleAll,
    clear,
    allSelected,
    someSelected,
  };
}
