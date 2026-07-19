import { useEffect, useMemo, useRef, useState } from "react";

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

export interface PaletteAction {
  id: string;
  label: string;
  hint?: string;
  run: () => void;
}

interface CommandPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  actions: PaletteAction[];
}

/** ⌘K command palette: a search box over a flat list of notebook actions.
 *  Built on the shadcn Dialog primitive (no cmdk dependency) — see plan
 *  docs/en/plans/2026-07-19-notebook-ergonomics-sprint4.md Task S4-1. */
export function CommandPalette({ open, onOpenChange, actions }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Reset search + selection each time the palette opens, and focus the input.
  useEffect(() => {
    if (open) {
      setQuery("");
      setSelected(0);
      // Radix moves focus into the content on open; do it after that settles.
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return actions;
    return actions.filter((a) => a.label.toLowerCase().includes(q));
  }, [actions, query]);

  // Clamp selection whenever the filtered list shrinks/grows.
  useEffect(() => {
    setSelected((s) => Math.min(Math.max(s, 0), Math.max(filtered.length - 1, 0)));
  }, [filtered.length]);

  // Keep the highlighted row scrolled into view as selection moves.
  useEffect(() => {
    const row = listRef.current?.querySelector<HTMLElement>(`[data-index="${selected}"]`);
    row?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const runSelected = () => {
    const action = filtered[selected];
    if (!action) return;
    action.run();
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="max-w-lg gap-0 p-0"
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          inputRef.current?.focus();
        }}
      >
        <DialogHeader className="sr-only">
          <DialogTitle>Command palette</DialogTitle>
        </DialogHeader>
        <input
          ref={inputRef}
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setSelected((s) => Math.min(s + 1, filtered.length - 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setSelected((s) => Math.max(s - 1, 0));
            } else if (e.key === "Enter") {
              e.preventDefault();
              runSelected();
            }
          }}
          placeholder="Type a command…"
          className="w-full border-b bg-transparent px-4 py-3 text-sm outline-none placeholder:text-muted-foreground"
        />
        <div ref={listRef} className="max-h-80 overflow-y-auto p-1">
          {filtered.length === 0 ? (
            <p className="px-3 py-6 text-center text-sm text-muted-foreground">
              No matching commands.
            </p>
          ) : (
            filtered.map((action, index) => (
              <button
                key={action.id}
                type="button"
                data-index={index}
                onMouseEnter={() => setSelected(index)}
                onClick={() => {
                  setSelected(index);
                  action.run();
                  onOpenChange(false);
                }}
                className={cn(
                  "flex w-full items-center justify-between gap-3 rounded-sm px-3 py-2 text-left text-sm",
                  index === selected ? "bg-accent text-accent-foreground" : "hover:bg-accent/50",
                )}
              >
                <span className="truncate">{action.label}</span>
                {action.hint && (
                  <span className="shrink-0 truncate text-xs text-muted-foreground">
                    {action.hint}
                  </span>
                )}
              </button>
            ))
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
