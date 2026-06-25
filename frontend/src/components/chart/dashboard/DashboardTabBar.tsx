import { useState } from "react";
import { ChevronDown, Plus } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
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
    <div className="mb-3 flex items-center gap-1 border-b-2 border-border" role="tablist">
      {ordered.map((t, pos) => {
        const active = pos === activeTabPos;
        const cls = `-mb-[2px] flex items-center gap-1 px-3 py-1.5 text-sm border-b-2 ${active ? "border-primary text-primary font-medium" : "border-transparent text-muted-foreground"}`;
        if (renaming === t.key && editing) {
          return (
            <input key={t.key} autoFocus aria-label={`rename tab ${t.name}`} defaultValue={t.name}
              className="m-0.5 rounded border px-1 text-sm"
              onBlur={(e) => { onRename(t.key, e.target.value.trim() || t.name); setRenaming(null); }}
              onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); if (e.key === "Escape") setRenaming(null); }} />
          );
        }
        return (
          <div key={t.key} className="group flex items-center"
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
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button type="button" aria-label={`tab menu ${t.name}`} title="Tab options" className="rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-foreground group-hover:opacity-100"><ChevronDown className="h-3.5 w-3.5" /></button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start">
                  <DropdownMenuItem onClick={() => setRenaming(t.key)}>Rename</DropdownMenuItem>
                  <DropdownMenuItem onClick={() => onDuplicate(t.key)}>Duplicate</DropdownMenuItem>
                  {ordered.length >= 2 && (
                    <DropdownMenuItem className="text-destructive" onClick={() => onDelete(t.key)}>Delete</DropdownMenuItem>
                  )}
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </div>
        );
      })}
      {editing && (
        <button type="button" aria-label="add tab" title="Add tab" className="inline-flex items-center rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={onAdd}><Plus className="h-4 w-4" /></button>
      )}
    </div>
  );
}
