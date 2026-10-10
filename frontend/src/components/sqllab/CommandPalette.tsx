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

/** ⌘K command palette: a search box over a flat list of actions.
 *  Built on the shadcn Dialog primitive (no cmdk dependency) — originally
 *  written for the notebook (plan docs/en/plans/2026-07-19-notebook-ergonomics-sprint4.md
 *  Task S4-1), now shared with SQL Lab's `SqlTab`. */
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
        className="max-w-lg p-0"
        /* The gap moved off the content and onto the scrolling body when the
           height cap landed in the primitive, so the override has to follow it:
           this dialog is a search field sitting flush on its result list, and a
           16px gap between them would read as two unrelated controls. */
        bodyClassName="gap-0"
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
                /* THE HIGHLIGHTED ROW IS THE SAME 1.03:1 DEFECT the dropdown and
                   select items had — `bg-accent` against a `--popover` that is
                   nearly the same colour — and it needs the same `--ring`
                   indicator to clear the 3:1 of WCAG 1.4.11. `dropdown-menu.tsx`
                   holds the full rationale.

                   What differs is where the indicator hangs. There it is a
                   `focus-visible` variant, because Radix moves real DOM focus onto
                   the item. Here DOM focus never leaves the search input — the
                   highlighted row is a VIRTUAL focus driven by `selected`, which is
                   what lets you keep typing while arrowing — so no focus
                   pseudo-class on the row can ever match, and the ring has to ride
                   the same conditional as the background.

                   Which means it also shows on mouse hover, because `onMouseEnter`
                   sets the same index. That turns out to match the dropdown rather
                   than diverge from it: measured in Chrome, a Radix menu item hovered
                   with a real mousemove matches `:focus-visible` too, so the ring
                   paints there on hover as well. Correct in both places for the same
                   reason — `selected` is the only pointer this list has, Enter runs
                   whichever row it marks, and a mouse user who has just moved the
                   selection has done exactly the thing the ring reports.
                   `hover:bg-accent/50` below is the un-selected hover and stays
                   ringless, so the two states remain distinguishable.

                   `ring-inset` because the list is `overflow-y-auto`: a scroll
                   container clips at its padding box, so an outset ring on the
                   first or last row would be cropped. */
                className={cn(
                  "flex w-full items-center justify-between gap-3 rounded-sm px-3 py-2 text-left text-sm",
                  index === selected
                    ? "bg-accent text-accent-foreground ring-2 ring-ring ring-inset"
                    : "hover:bg-accent/50",
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
