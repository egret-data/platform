import { toast } from "sonner";

/** Metabase's wording for the three destructive edit-mode actions it makes undoable.
 *  `Removed card` — dashboard/actions/cards-typed.ts:508.
 *  `Question replaced` — dashboard/components/DashboardGrid.tsx:699-713; Metabase branches over
 *  model/metric/question there, and an egret dashboard card is always a chart, so this has one
 *  form where Metabase has three. */
export const REMOVED_CARD = "Removed card";
export const REPLACED_CARD = "Chart replaced";

/** `Deleted "<tab name>"` — dashboard/components/DashboardTabs/use-dashboard-tabs.ts:57. */
export function deletedTab(name: string): string {
  return `Deleted "${name}"`;
}

/**
 * Announce a draft-level destructive edit and offer an Undo that restores `previous`.
 *
 * All three call sites are DRAFT edits — nothing has been PUT — so undo is a `setState` with the
 * snapshot taken immediately before the edit, and never a server round trip. That is also why
 * `previous` is a parameter handed back to `restore` rather than something `restore` closes over:
 * closing over it is how a caller accidentally captures the state AFTER the edit, which turns Undo
 * into a no-op that looks like it worked.
 *
 * The snapshot is the whole slice (every card, or every tab plus every card), not just the one
 * item removed. For the few seconds the toast is up those are the same thing in any realistic
 * sequence, and it is what keeps undo a single state write with no reconciliation.
 */
export function undoableDraftEdit<T>(message: string, previous: T, restore: (previous: T) => void): void {
  toast(message, { action: { label: "Undo", onClick: () => restore(previous) } });
}
