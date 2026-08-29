import { cn } from "@/lib/utils";

export interface SummaryStripItem {
  /**
   * The word beside the number in every case EXCEPT a count of exactly one.
   *
   * Also the item's React key, which is why it is the count-independent half
   * of the pair — see the component doc below.
   */
  label: string;
  /**
   * The word to use instead when `value` is the number `1`.
   *
   * Optional, and that is the design: most labels on this strip are not nouns.
   * "live", "draft", "disabled", "active", "paused", "failing", "ok",
   * "granted", "queued" are adjectives, and "phase"/"auth" are not counts at
   * all. A strip that DERIVED a singular from `label` — stripping a trailing
   * `s`, or appending one — would have to tell those apart from "charts" and
   * "folders" with no information to do it, and English has no rule to lend it
   * ("bus", "was"). Naming both words at the call site costs one field, needs
   * no dictionary and no i18n dependency, and handles an irregular
   * ("queries"/"query") for free. Omitting it is the explicit statement that
   * this label does not inflect.
   */
  singular?: string;
  value: string | number;
  /** When set, the item becomes a clickable filter toggle. */
  onClick?: () => void;
  /** Highlights the item as the active filter (only meaningful with onClick). */
  active?: boolean;
}

/** A compact one-line "value label · value label" summary (replaces KPI cards).
 * Items with an `onClick` render as toggle chips so a summary can double as a
 * quick filter (e.g. click "active" to filter the list to active rows).
 *
 * **The singular/plural choice lives here, not on the pages.** Seven Explore
 * list pages render counts through this component and most of them read
 * "1 charts" / "1 folders" / "1 dashboards"; two had been fixed by hand with a
 * ternary at the call site, which is how a rule ends up applied on two pages
 * and forgotten on five. The rule — *the number one, and only the number one,
 * takes `singular`* — is written once, below.
 *
 * `value === 1` is deliberately strict identity on a NUMBER. `value` also
 * carries formatted strings (`EngineDetail.tsx` passes `"3/4"` and a phase
 * name), and a string that happens to read `"1"` is not a count of one.
 *
 * The key is `it.label`, unchanged, and that is only safe because `singular`
 * is a separate field: the rendered word changes at 1<->2 but `label` does
 * not, so the item is not remounted mid-interaction. Had this been a
 * count-aware `label` formatter instead, the key would have been
 * count-dependent and a focused toggle chip would have lost focus the moment
 * its own count crossed one.
 */
export function SummaryStrip({ items }: { items: SummaryStripItem[] }) {
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground">
      {items.map((it, i) => {
        const word = it.singular != null && it.value === 1 ? it.singular : it.label;
        const body = (
          <>
            <span className="font-medium tabular-nums text-foreground">{it.value}</span>
            {word}
          </>
        );
        return (
          <span key={it.label} className="inline-flex items-center gap-1">
            {i > 0 && <span className="mr-1 opacity-40">·</span>}
            {it.onClick ? (
              <button
                type="button"
                onClick={it.onClick}
                aria-pressed={it.active ?? false}
                className={cn(
                  "inline-flex items-center gap-1 rounded-md px-2 py-0.5 transition-colors hover:bg-accent hover:text-foreground",
                  it.active && "bg-accent text-foreground ring-1 ring-inset ring-border",
                )}
              >
                {body}
              </button>
            ) : (
              body
            )}
          </span>
        );
      })}
    </div>
  );
}
