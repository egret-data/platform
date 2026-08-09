import type { Ref } from "react";
import { Filter } from "lucide-react";
import type { DashboardParameter } from "@/api/dashboards";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { FilterPill } from "./FilterPill";
import { isBlank } from "./params";

interface Props {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  optionsBySlug: Record<string, string[]>;
  onChange: (slug: string, value: unknown) => void;
  /** Edit mode: a chip SELECTS its filter rather than picking a value. This is the only route from
   *  a card-hosted filter back to its settings panel, and so the only route back to the top bar. */
  onSelect?: (paramId: string) => void;
  activeId?: string | null;
  /** From `useCollapsedParameterList`. The component never measures anything itself. */
  collapsed: boolean;
  listRef: Ref<HTMLDivElement>;
}

/** Same predicate `FilterPill` uses for its own "is this set" styling (`FilterPill.tsx:36-39`);
 *  three lines, kept local rather than exported, because the two will not diverge. */
function hasValue(value: unknown): boolean {
  if (Array.isArray(value)) return value.filter((v) => !isBlank(v)).length > 0;
  return !isBlank(value);
}

/** One card's filter list. Metabase's `CollapsibleDashboardParameterList`
 *  (`:36,40-86,87-101`), including its `isSortable: false` — an inline list is deliberately NOT
 *  the reorderable one. The order is the card's `options.inlineParameters`; W6's drag-reorder
 *  stays on the top bar, where the order is a saved property of the dashboard. */
export function CardInlineFilters({
  parameters, values, optionsBySlug, onChange, onSelect, activeId, collapsed, listRef,
}: Props) {
  if (parameters.length === 0) return null;

  const list = (
    <div className="flex flex-wrap items-center gap-2">
      {parameters.map((p) =>
        onSelect ? (
          // Edit mode. A plain chip, not `EditFilterChips`: that one carries a drag handle, and
          // Metabase's inline list is explicitly `isSortable: false`.
          <button key={p.id} type="button" aria-label={p.name} data-active={p.id === activeId}
            className={`dash-nodrag inline-flex h-8 items-center rounded-sm border px-2 text-sm ${p.id === activeId ? "border-primary bg-secondary text-primary" : "bg-background hover:bg-accent/40"}`}
            onClick={(e) => { e.stopPropagation(); onSelect(p.id); }}>
            {p.name}{p.required ? " *" : ""}
          </button>
        ) : (
          // `dash-nodrag` and the stopPropagation are both load-bearing on a dashboard card: the
          // grid puts a drag handle over the card in edit mode, and the card BODY is a click
          // target that opens the card settings panel (`DashboardView.tsx`, the dashcard-body div).
          <div key={p.id} className="dash-nodrag" onClick={(e) => e.stopPropagation()}>
            <FilterPill param={p} value={values[p.slug]} options={optionsBySlug[p.slug] ?? []}
              onChange={(v) => onChange(p.slug, v)} />
          </div>
        ),
      )}
    </div>
  );

  const setCount = parameters.filter((p) => hasValue(values[p.slug])).length;

  return (
    <>
      {/* The invisible always-expanded copy the collision test measures. `fixed` takes it out of
          flow, `invisible` keeps it laid out (`display:none` would measure 0), and the inner
          `absolute` lets it shrink-wrap to its natural width. `aria-hidden` keeps it out of the
          accessibility tree so it never doubles the pills a reader can reach. Metabase renders the
          same thing for the same reason (CollapsibleDashboardParameterList.tsx:87-101). */}
      <div aria-hidden data-testid="inline-filter-measure"
        className="pointer-events-none invisible fixed left-0 top-0 w-full">
        <div ref={listRef} className="absolute">{list}</div>
      </div>
      {collapsed ? (
        <Popover>
          <PopoverTrigger asChild>
            <button type="button" aria-label="Show filters" title="Show filters"
              className="dash-nodrag inline-flex h-8 shrink-0 items-center gap-1 rounded-sm border bg-background px-2 text-sm hover:bg-accent/40"
              onClick={(e) => e.stopPropagation()}>
              <Filter className="h-3.5 w-3.5 text-muted-foreground" />
              {setCount > 0 && <span data-testid="inline-filter-count">{setCount}</span>}
            </button>
          </PopoverTrigger>
          <PopoverContent align="end" className="w-auto max-w-[20rem] p-2"
            onClick={(e) => e.stopPropagation()}>{list}</PopoverContent>
        </Popover>
      ) : (
        <div className="shrink-0">{list}</div>
      )}
    </>
  );
}
