import { useEffect, useState } from "react";

interface Props {
  name: string;
  description: string;
  ownerLabel: string | null;
  updatedAt: string;
  onSaveDescription: (next: string) => void;
  onClose: () => void;
}

export function DashboardInfoSidebar({ name, description, ownerLabel, updatedAt, onSaveDescription, onClose }: Props) {
  const [draft, setDraft] = useState(description);
  // Re-sync if the description changes underneath us (e.g. a concurrent edit refetch);
  // [description] compares by value, so this only fires on a genuine external change.
  useEffect(() => { setDraft(description); }, [description]);
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
        <div className="flex justify-between"><dt className="text-muted-foreground">Owner</dt><dd>{ownerLabel ?? "—"}</dd></div>
        <div className="flex justify-between"><dt className="text-muted-foreground">Last edited</dt><dd>{new Date(updatedAt).toLocaleString()}</dd></div>
      </dl>
    </aside>
  );
}
