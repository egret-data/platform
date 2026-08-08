import { useMemo, useState } from "react";
import { Search } from "lucide-react";
import type { VizType } from "@/api/charts";
import { Input } from "@/components/ui/input";
import { CHART_TYPES } from "./chartTypes";

/**
 * The nineteen visualization types, as description cards.
 *
 * WHY CARDS AND NOT METABASE'S ICON GRID. Metabase's `ChartTypeOption` is a 50px circular icon
 * with a name under it, four to a row, and no description anywhere — which is how it fits its list
 * into a sidebar. egret shows a sentence per type on purpose, and `chartTypes.ts` records why on
 * the Map entry: the card is the only place a reader is told what a type CAN DO before picking it.
 * Copying Metabase here would fit the grid by deleting the thing the grid is for, so the fix below
 * is to make the cards reachable rather than to make them smaller.
 *
 * WHAT WAS WRONG. Seven rows of 158px cards is 1146px of grid; `DialogContent` sets no maximum
 * height and does not scroll, so at 1440×900 the dialog measured 1246px tall at `top: -173px`.
 * Measured on the kind deployment: 173px above the viewport and 173px below it were unreachable —
 * that is the dialog TITLE, the CLOSE BUTTON (`y = -156`, off-screen entirely) and the whole first
 * row (Table, Pivot table, Bar), with Map clipped at the bottom. Nothing scrolled: the content is
 * `position: fixed` with `overflow: visible` inside a `body` that Radix sets `overflow: hidden`,
 * and `scrollHeight === clientHeight` on every element in the chain. Three of the nineteen types
 * could not be chosen with a mouse at all, and keyboard focus could not scroll them into view
 * either, because there was no scroll container to scroll.
 *
 * The grid is now the scroll container (see `VizSwitcher`, which caps the dialog and lets this
 * flex-grow inside it), and the filter above it turns nineteen items into a reachable few. Both
 * matter: the scroll makes every type reachable, the filter makes finding one not a scan.
 */
export function ChartTypeGallery({
  selected, onSelect,
}: { selected?: VizType; onSelect: (key: VizType) => void }) {
  const [query, setQuery] = useState("");
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return CHART_TYPES;
    // Name AND description: "distribution" finds the box plot and "share" finds the pie, neither of
    // which says so in its name. The descriptions are already written and already on screen; not
    // searching them would make the filter narrower than the reader's own eyes.
    return CHART_TYPES.filter(
      (t) => t.name.toLowerCase().includes(q) || t.description.toLowerCase().includes(q),
    );
  }, [query]);

  return (
    <div className="flex min-h-0 flex-col gap-3">
      <div className="relative shrink-0">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search chart types…"
          aria-label="Search chart types"
          className="pl-8"
        />
      </div>
      {shown.length === 0 ? (
        <p className="py-6 text-center text-sm text-muted-foreground">
          No chart type matches “{query.trim()}”.
        </p>
      ) : (
        /* `ring-inset` rather than a ring that paints outside the button: this list clips, so an
           outside ring on the first or last row is drawn under the scroll edge and disappears.
           #419's rule is `focus-visible:ring-2 ring-ring`, plus `ring-inset` exactly where a
           container clips — which is here. No `ring-offset`: it hard-codes `--background` and
           paints a near-black band on `--card` in dark mode. */
        <div
          role="listbox"
          aria-label="Chart type"
          className="grid min-h-0 flex-1 grid-cols-2 gap-3 overflow-y-auto [scrollbar-gutter:stable] p-0.5 sm:grid-cols-3"
        >
          {shown.map((t) => {
            const Icon = t.icon;
            const active = selected === t.key;
            return (
              <button
                key={t.key}
                type="button"
                role="option"
                aria-selected={active}
                onClick={() => onSelect(t.key)}
                className={`flex h-full flex-col items-start gap-2 rounded-lg border p-4 text-left transition hover:border-primary/50 hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset ${
                  active ? "border-primary ring-2 ring-primary/30 ring-inset" : ""
                }`}
              >
                <Icon className="h-6 w-6 text-primary" />
                <div className="font-semibold">{t.name}</div>
                <div className="text-xs text-muted-foreground">{t.description}</div>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
