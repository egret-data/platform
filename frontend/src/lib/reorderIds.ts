import { arrayMove } from "@dnd-kit/sortable";

/**
 * Pure reorder helper for a dnd-kit `onDragEnd`: move `activeId` to the
 * position `overId` currently occupies in `ids`. No-op (returns the same
 * array reference) when the ids are equal or either is missing — callers
 * (e.g. EditFilterChips' drag-reorder) can skip the mutate call on a no-op.
 */
export function reorderIds(ids: string[], activeId: string, overId: string): string[] {
  if (activeId === overId) return ids;
  const from = ids.indexOf(activeId);
  const to = ids.indexOf(overId);
  if (from < 0 || to < 0) return ids;
  return arrayMove(ids, from, to);
}
