import { useState } from "react";
import type { DraftTab } from "./tabs";

interface Props {
  tabs: DraftTab[];
  activeTabPos: number;
  editing: boolean;
  onSelect: (pos: number) => void;
  onAdd: () => void;
  onRename: (key: string, name: string) => void;
  onReorder: (fromPos: number, toPos: number) => void;
  onDelete: (key: string) => void;
  onDuplicate: (key: string) => void;
}

/** The dashboard tab bar. In view mode the parent only renders this when `tabs.length >= 2`;
 *  in edit mode it is always rendered (so the first tab can be added). */
export function DashboardTabBar({ tabs, activeTabPos, editing, onSelect, onAdd, onRename, onReorder, onDelete, onDuplicate }: Props) {
  const ordered = [...tabs].sort((a, b) => a.position - b.position);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [dragFrom, setDragFrom] = useState<number | null>(null);

  return (
    <div className="mb-3 flex items-center gap-1 border-b" role="tablist">
      {ordered.map((t, pos) => {
        const active = pos === activeTabPos;
        const cls = `flex items-center gap-1 px-3 py-1.5 text-sm border-b-2 ${active ? "border-primary text-primary font-medium" : "border-transparent text-muted-foreground"}`;
        if (renaming === t.key && editing) {
          return (
            <input key={t.key} autoFocus aria-label={`rename tab ${t.name}`} defaultValue={t.name}
              className="m-0.5 rounded border px-1 text-sm"
              onBlur={(e) => { onRename(t.key, e.target.value.trim() || t.name); setRenaming(null); }}
              onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); if (e.key === "Escape") setRenaming(null); }} />
          );
        }
        return (
          <div key={t.key} className="flex items-center"
            draggable={editing}
            onDragStart={() => setDragFrom(pos)}
            onDragOver={(e) => editing && e.preventDefault()}
            onDrop={() => { if (editing && dragFrom !== null && dragFrom !== pos) onReorder(dragFrom, pos); setDragFrom(null); }}>
            <button type="button" role="tab" aria-selected={active} aria-label={t.name} className={cls}
              onClick={() => onSelect(pos)}
              onDoubleClick={() => { if (editing) setRenaming(t.key); }}>
              {t.name}
            </button>
            {editing && (
              <>
                <button type="button" aria-label={`duplicate tab ${t.name}`} className="px-1 text-xs text-muted-foreground" onClick={() => onDuplicate(t.key)}>⧉</button>
                <button type="button" aria-label={`delete tab ${t.name}`} className="px-1 text-xs text-destructive disabled:opacity-40"
                  disabled={ordered.length < 2} onClick={() => onDelete(t.key)}>✕</button>
              </>
            )}
          </div>
        );
      })}
      {editing && (
        <button type="button" aria-label="add tab" className="px-2 py-1 text-sm text-muted-foreground hover:text-foreground" onClick={onAdd}>+</button>
      )}
    </div>
  );
}
