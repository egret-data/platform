import { Button } from "@/components/ui/button";
import { describeFilterChanges } from "./filterDrafts";

interface Props {
  autoApply: boolean;
  changedCount: number;
  onApply: () => void;
  onCancel: () => void;
}

/** The draft-filters bar, bottom-centre. Metabase's `FilterApplyToast` renders on exactly these
 *  conditions (`FilterApplyToast.tsx:44-46`: auto-apply off, something unapplied, not editing) —
 *  the editing case is handled by the caller, which only renders the view-mode filter bar. */
export function FilterApplyBar({ autoApply, changedCount, onApply, onCancel }: Props) {
  if (autoApply || changedCount === 0) return null;
  return (
    <div role="status"
      className="fixed bottom-5 left-1/2 z-40 flex -translate-x-1/2 items-center gap-3 rounded-md border bg-card px-4 py-2 shadow-lg">
      <span className="text-sm">{describeFilterChanges(changedCount)}</span>
      <Button size="sm" variant="ghost" onClick={onCancel}>Cancel</Button>
      <Button size="sm" onClick={onApply}>Apply</Button>
    </div>
  );
}
