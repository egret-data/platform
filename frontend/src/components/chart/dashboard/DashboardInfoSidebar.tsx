import { useEffect, useState } from "react";
import { attributionLine } from "./attribution";

interface Props {
  name: string;
  description: string;
  /** Display name for the creator, already resolved by the API (`created_by_name`); falls back
   *  to the raw id, exactly as ShareDialog.tsx does for a grant's principal. */
  ownerLabel: string | null;
  createdAt: string | null;
  editorLabel: string | null;
  updatedAt: string;
  onSaveDescription: (next: string) => void;
  onClose: () => void;
}

export function DashboardInfoSidebar({
  name, description, ownerLabel, createdAt, editorLabel, updatedAt, onSaveDescription, onClose,
}: Props) {
  const [draft, setDraft] = useState(description);
  // Re-sync if the description changes underneath us (e.g. a concurrent edit refetch);
  // [description] compares by value, so this only fires on a genuine external change.
  useEffect(() => { setDraft(description); }, [description]);
  const added = attributionLine(createdAt, ownerLabel);
  // `editorLabel ?? ownerLabel` is deliberate: updated_by is NULL for every dashboard that has
  // not been saved since migration 0106, and in that state the last person known to have touched
  // it is its creator. Metabase's last-edit-info falls back to the creator the same way.
  const edited = attributionLine(updatedAt, editorLabel ?? ownerLabel);
  return (
    <aside className="sticky top-4 max-h-[calc(100vh-7rem)] w-[300px] shrink-0 self-start overflow-auto rounded-lg border bg-card p-3 text-sm">
      <div className="mb-2 flex items-center justify-between">
        <div className="font-medium">{name}</div>
        <button type="button" aria-label="close info" className="text-muted-foreground hover:text-foreground" onClick={onClose}>✕</button>
      </div>
      <label className="mb-3 flex flex-col gap-1 text-xs text-muted-foreground">Description
        <textarea aria-label="description" className="h-24 w-full rounded border bg-background px-2 py-1 text-sm"
          value={draft} onChange={(e) => setDraft(e.target.value)}
          onBlur={() => { if (draft !== description) onSaveDescription(draft); }} />
      </label>
      <dl className="flex flex-col gap-1 text-xs">
        {added && (
          <div className="flex justify-between gap-2"><dt className="shrink-0 text-muted-foreground">Added</dt><dd className="text-right">{added}</dd></div>
        )}
        {edited && (
          <div className="flex justify-between gap-2"><dt className="shrink-0 text-muted-foreground">Last edited</dt><dd className="text-right">{edited}</dd></div>
        )}
      </dl>
    </aside>
  );
}
