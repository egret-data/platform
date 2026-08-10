import {
  DndContext, KeyboardSensor, PointerSensor, useSensor, useSensors, closestCenter,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext, verticalListSortingStrategy, useSortable, arrayMove,
  sortableKeyboardCoordinates,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical } from "lucide-react";
import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";
import type { TableState } from "@/api/charts";
import type { ColumnFormat } from "@/components/chart/format-types";
import { Plus } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { CheckList } from "@/components/chart/config/CheckList";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import type { FieldLabels } from "@/components/chart/config/field-names";
import { colIcon } from "./FormatSection";
import { ColumnSettingsPopover } from "./ColumnSettingsPopover";
import { FIELD_LABEL, GROUP_LABEL } from "./FieldLabel";

interface ResultColumn { name: string; type: string }

const PAGE_SIZES = [25, 50, 100];

/** Below this many columns a search box is clutter rather than help. */
const COLUMN_FILTER_MIN = 12;

/**
 * A group of query fields the `Add or remove columns` panel offers. One per list the query
 * projects from — a raw table has "Columns"; an aggregated one also has "Metrics", which is the
 * FOURTH parallel field list this section exists to fold away.
 */
export interface QueryFieldGroup {
  label: string;
  options: string[];
  selected: string[];
  /** Machine name -> what the dataset calls the field. Optional because a caller that has no
   *  `DatasetColumn[]` to hand (a dashboard card's per-card panel) has nothing to supply; absent
   *  simply falls through to the machine name, which is `displayName`'s own contract. */
  labels?: FieldLabels;
  onChange: (next: string[]) => void;
}

/**
 * THE TABLE'S COLUMN LIST — one row per column, and the only one.
 *
 * WHAT IT REPLACED. egret described a table's columns in three parallel lists (four once the table
 * aggregated), all of them flat and all of them near-identically labelled:
 *
 *   Data ▸ "Columns"            what the QUERY projects        (viz/table.tsx — NOW REMOVED)
 *   Data ▸ "Metrics"            the aggregated half of that    (viz/table.tsx, still there)
 *   Display ▸ "Visible columns" a checkbox per result column   (this file, before)
 *   Display ▸ "Formatting"      an accordion per result column (FormatSection, for the table)
 *
 * A five-column table therefore printed the same five names three times, in three orders, across
 * two tabs, and REORDER — the one thing the panel could not do at all — existed only as a drag on
 * the rendered header, invisible from here. Metabase has one list: identity on the row, everything
 * else one click away. So does this now. The Formatting accordion is gone for the table (it still
 * serves every other viz, whose formattable columns are a SUBSET of the result and so are a
 * genuinely different list); its editor moved, unchanged, into the row's drill-in.
 *
 * WHY THIS LIST AND NOT THE DATA TAB'S. `CardVizPanel` — a dashboard card's per-card override
 * editor — composes these display sections and has no Data tab at all, so the Data tab's list
 * cannot be the one that survives. `ChartTable.tsx`'s empty-state message already records that
 * constraint.
 *
 * WHY ▲▼ AND NOT A DRAG HANDLE. `SeriesListSection` — this list's sibling, shipped for exactly
 * this problem on the series side — reorders with a pair of buttons, and buttons come with their
 * keyboard path already attached. PR #356 in this same programme shipped a column reorder that
 * was mouse-only because a `dnd-kit` list was given no `KeyboardSensor`; that is not a mistake
 * worth repeating for a visual grabber. (The rendered table's header drag stays: it is the
 * direct-manipulation path and it already has its `KeyboardSensor`.)
 *
 * ORDER IS KEYED ON THE RAW COLUMN NAME, never on what the row displays. `column_title` renames
 * the header without renaming the column (`ColumnFormatFields`), and `ChartTable` resolves
 * `columnOrder` through `c.name` — so ranking or emitting the displayed string would write an
 * order the renderer resolves nothing out of, and would do it silently. Seven bugs in this
 * codebase have now come from keying on a display string; two of them were caught only by
 * mutation, with the suite green.
 */
/**
 * The order a drop writes, as a pure function — and the reason it is one.
 *
 * Swapping the ▲▼ pair for a drag grip cost the END-TO-END reorder tests: dnd-kit's keyboard sensor
 * reads element rects, and jsdom has none, so no test in this repo drives a drag (`FieldZone`'s
 * own tests assert its grip EXISTS and stop there). What must not go untested with them is WHICH
 * STRING lands in `columnOrder`: `column_title` renames the header without renaming the column,
 * and `ChartTable` resolves the order through `c.name`, so emitting a displayed title writes an
 * order the renderer resolves nothing out of — silently. Seven bugs in this codebase have come
 * from keying on a display string.
 *
 * So the DOM drag is untested and this is not.
 */
export function reorderColumnOrder(
  names: string[],
  activeId: string | number,
  overId: string | number | undefined,
): string[] | null {
  if (overId === undefined || activeId === overId) return null;
  const from = names.indexOf(String(activeId));
  const to = names.indexOf(String(overId));
  if (from < 0 || to < 0) return null;
  return arrayMove(names, from, to);
}

/**
 * ONE COLUMN'S ROW. Its own component because `useSortable` is a hook and the list renders inside
 * a `.map()`, where a hook cannot go — the same reason `FieldZone` has its `Chip`.
 */
function ColumnRow({ c, isHidden, title, filtering, columnSettings, onColumnSettings, onToggleHidden }: {
  c: ResultColumn;
  isHidden: boolean;
  title: string;
  /** A filter is active, so reordering is suppressed — a drop would land off screen. */
  filtering: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  onColumnSettings?: (next: Record<string, ColumnFormat>) => void;
  onToggleHidden: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: c.name });
  // A dashboard card carries its columns' NAMES and not their types (`DashboardView` builds them
  // from the chart's stored spec), so `c.type` is routinely "". `colIcon` falls through to Text for
  // an unknown type, and a Text icon on `subtotal` is a confident false statement — worse than no
  // icon. Rendered as an absent type instead, with the icon's box kept so the names still line up.
  const Icon = c.type ? colIcon(c.type) : null;
  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      data-column={c.name}
      // Metabase marks a hidden row and LEAVES IT IN PLACE (`data-enabled="false"`) rather than
      // moving it to the end or into a second list. That is what makes "turn it back on"
      // findable: the row is where the reader last saw it.
      data-enabled={isHidden ? "false" : "true"}
      className={`flex items-center gap-1 rounded-md border px-1.5 py-1 ${isHidden ? "opacity-60" : ""}`}
    >
      {/* A DRAG GRIP, not the ▲▼ pair this replaced. The pair was chosen because PR #356 shipped a
          dnd-kit reorder that was mouse-only — no `KeyboardSensor` — and buttons come with their
          keyboard path attached. That reason expired: `FieldZone` fixed the sensor, so a grip here
          reorders from the keyboard too (focus, space to lift, arrows, space to drop). Metabase's
          own Columns row uses a grabber, and so does egret's field zone one tab away. */}
      <button type="button"
        className="flex size-6 shrink-0 cursor-grab items-center justify-center rounded text-muted-foreground hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset disabled:cursor-not-allowed disabled:opacity-40"
        disabled={filtering}
        title={filtering ? "Clear the search to reorder" : undefined}
        aria-label={`reorder ${c.name}`}
        {...(filtering ? {} : attributes)} {...(filtering ? {} : listeners)}>
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      {Icon
        ? <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" aria-hidden />
        : <span className="h-3.5 w-3.5 shrink-0" aria-hidden />}
      <span className="min-w-0 flex-1 truncate" title={c.name}>
        {title}
        {/* The raw name stays beside the rename — it is the name every other tab, the SQL and the
            drill menu call this column by. */}
        {title !== c.name && (
          <span className="ml-1.5 text-xs text-muted-foreground">{c.name}</span>
        )}
      </span>
      {/* THE TEXT BADGE IS GONE. The row already carries a type ICON to the left, so `NUMBER`
          beside it said the same thing twice on every row — Metabase's own Columns row has the
          icon and no badge. Removing it is most of what makes this list read as compact. */}
      {onColumnSettings && (
        <ColumnSettingsPopover column={c} value={columnSettings ?? {}} onChange={onColumnSettings} />
      )}
      {/* An eye, not the sibling list's Switch: Metabase's column row uses one, and forty switches
          down a panel this narrow is a different weight of control than ten. `aria-pressed` carries
          the state, so it is not colour-only. */}
      <button
        type="button"
        aria-label={`Show ${c.name}`}
        aria-pressed={!isHidden}
        onClick={onToggleHidden}
        className="flex size-6 shrink-0 items-center justify-center rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring text-muted-foreground hover:bg-accent hover:text-foreground"
      >
        {isHidden ? <EyeOff className="h-3.5 w-3.5" aria-hidden /> : <Eye className="h-3.5 w-3.5" aria-hidden />}
      </button>
    </div>
  );
}

export function TableColumnsSection({
  columns, value, onChange, columnSettings, onColumnSettings, queryFields, part,
}: {
  columns: ResultColumn[];
  value: TableState;
  onChange: (next: TableState) => void;
  columnSettings?: Record<string, ColumnFormat>;
  /** Writes `column_settings` from the row's drill-in. Absent — a caller that renders this list
   *  without owning the format map — prunes the drill-in rather than opening a read-only one. */
  onColumnSettings?: (next: Record<string, ColumnFormat>) => void;
  /** Query projection, folded behind `Add or remove columns`. Supplied only where the query is
   *  EDITABLE, which is the builder's route and nowhere else: a dashboard card overrides display
   *  settings and cannot change what the card's query selects. */
  queryFields?: QueryFieldGroup[];
  /**
   * Which half to draw. The column LIST and the table's LAYOUT settings (freeze, row index,
   * pagination) live in one component because they edit one `tableState`, but Metabase puts them
   * on different tabs — Columns for the list, Display for the three toggles — and so does egret
   * now. Absent draws both, which is the dashboard card's case: it has no tabs to split them
   * across.
   *
   * This split exists because moving the whole section to the Columns tab took the toggles with
   * it and left the table's Display tab EMPTY — measured in the browser, and invisible to the
   * suite, which asserts what each tab contains but never that Display contains anything.
   */
  part?: "list" | "layout";
}) {
  const [filter, setFilter] = useState("");
  const hidden = new Set(value.hiddenColumns ?? []);
  const pinnedLeft = value.pinnedLeft ?? 0;
  const pinnedTop = value.pinnedTop ?? 0;
  const pagination = value.pagination ?? { enabled: false, pageSize: 25 };

  // What the user sees for a column. Everything besides this label and the filter match below
  // keeps dealing in the raw `c.name` — hiddenColumns, columnOrder, column_settings and
  // setAllHidden all key off the column identity the query actually returns, not its title.
  const titleFor = (c: ResultColumn) => columnSettings?.[c.name]?.column_title || c.name;

  // The order the table DRAWS in: the stored order first, then anything it does not mention in
  // result order. Exactly `ChartTable`'s `displayOrder` rule, because a list that claims to be
  // the column order has to be the order on screen — the old list never read `columnOrder` at
  // all, so a dragged column moved in the table and stayed put here.
  const rank = new Map((value.columnOrder ?? []).map((n, i) => [n, i]));
  const ordered = [...columns].sort(
    (a, b) => (rank.get(a.name) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b.name) ?? Number.MAX_SAFE_INTEGER),
  );

  const q = filter.trim().toLowerCase();
  const visibleColumns = q
    ? ordered.filter(
        (c) => titleFor(c).toLowerCase().includes(q) || c.name.toLowerCase().includes(q),
      )
    : ordered;

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    // The sensor PR #356 forgot, which is why this list reordered with buttons for so long.
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  /**
   * A drop writes the WHOLE display order back, exactly as the ▲▼ pair did.
   *
   * Reordering is suppressed while a filter is active (the grips are disabled), so `ordered` and
   * what is on screen are the same list here — dragging a filtered subset would otherwise write an
   * order derived from rows the reader cannot see.
   */
  function onDragEnd(e: DragEndEvent) {
    const next = reorderColumnOrder(ordered.map((c) => c.name), e.active.id, e.over?.id);
    if (next) onChange({ ...value, columnOrder: next });
  }

  function toggleHidden(name: string) {
    const next = new Set(hidden);
    if (next.has(name)) next.delete(name); else next.add(name);
    onChange({ ...value, hiddenColumns: [...next] });
  }

  // Deliberately `columns`, not the filtered list: a filter is a way to FIND a column, not a
  // selection. "Hide all" while a filter is active must not quietly hide only what is on screen.
  const setAllHidden = (hide: boolean) =>
    onChange({ ...value, hiddenColumns: hide ? columns.map((c) => c.name) : [] });

  return (
    <div className="space-y-3 text-sm">
      {part !== "layout" && (
      <div className="space-y-1">
        {/* No inner "Visible columns" heading any more. There is one list now, and its caller
            already labels it "Columns" — the second heading was only ever there to tell it apart
            from the OTHER column list, which is the thing this section removed. */}
        <div className="flex items-center justify-end">
          <>
            <div className="flex gap-2">
              <button type="button" className="min-h-6 text-xs text-muted-foreground hover:text-foreground hover:underline"
                onClick={() => setAllHidden(false)}>
                Show all
              </button>
              <button type="button" className="min-h-6 text-xs text-muted-foreground hover:text-foreground hover:underline"
                onClick={() => setAllHidden(true)}>
                Hide all
              </button>
            </div>
          </>
        </div>

            {columns.length >= COLUMN_FILTER_MIN && (
              <Input
                aria-label="search columns" placeholder="Search columns…" value={filter}
                onChange={(e) => setFilter(e.target.value)}
                className="w-full text-sm"
              />
            )}
            {q && visibleColumns.length === 0 && (
              <p className="text-xs text-muted-foreground">No column matches “{filter}”.</p>
            )}
            <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
              <SortableContext items={visibleColumns.map((c) => c.name)} strategy={verticalListSortingStrategy}>
            <div className="flex flex-col gap-1">
              {visibleColumns.map((c) => (
                <ColumnRow
                  key={c.name}
                  c={c}
                  isHidden={hidden.has(c.name)}
                  title={titleFor(c)}
                  filtering={!!q}
                  columnSettings={columnSettings}
                  onColumnSettings={onColumnSettings}
                  onToggleHidden={() => toggleHidden(c.name)}
                />
              ))}
            </div>
              </SortableContext>
            </DndContext>

        {queryFields && (
          /* A POPOVER, not a mode. This used to REPLACE the list with the picker and turn the
             button into "Done picking columns" — one surface pretending to be two. The pivot's
             field zone already had the better shape: a dashed add-row that floats the picker above
             the zone, so the list you are adding to stays on screen while you add to it. The old
             comment defended replacing on the grounds that two column lists must not be visible at
             once; an overlay is not a second list in the panel's flow, which is what that was
             about. */
          <Popover>
            <PopoverTrigger asChild>
              <button type="button" aria-label="add or remove columns"
                className="flex min-h-10 w-full items-center gap-1 rounded-md border border-dashed px-2 text-xs text-muted-foreground hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset">
                <Plus className="h-3.5 w-3.5" /> Add or remove columns…
              </button>
            </PopoverTrigger>
            <PopoverContent align="start" className="w-72 space-y-3 p-2">
              {queryFields.map((g, gi) => (
                <div key={g.label} className="space-y-1">
                  <div className={`truncate ${GROUP_LABEL}`} title={g.label}>{g.label}</div>
                  {/* The SAME `CheckList` the Data tab used before its duplicate was removed —
                      component, not a copy of its markup. It brings the filter, the select-all with
                      a count and the bounded height that a hand-rolled row list never had. */}
                  <CheckList
                    idPrefix={`qf-${gi}`}
                    empty="Nothing to pick from."
                    options={g.options}
                    selected={g.selected}
                    labels={g.labels}
                    onToggle={(name) =>
                      g.onChange(
                        g.selected.includes(name)
                          ? g.selected.filter((x) => x !== name)
                          : [...g.selected, name],
                      )
                    }
                    onSetAll={g.onChange}
                  />
                </div>
              ))}
            </PopoverContent>
          </Popover>
        )}
      </div>
      )}

      {part !== "list" && (<>
      <div className="space-y-1">
        <Label htmlFor="freeze-left" className={FIELD_LABEL}>Freeze first columns</Label>
        <Input
          id="freeze-left" type="number" min={0} max={columns.length} value={pinnedLeft}
          onChange={(e) => {
            const n = Number(e.target.value);
            onChange({ ...value, pinnedLeft: Number.isNaN(n) ? 0 : Math.max(0, Math.min(n, columns.length)) });
          }}
          className="w-full text-sm"
        />
      </div>

      {/*
        THE OTHER AXIS. Metabase spends four settings on the freeze — `table.freeze_columns` with
        `table.freeze_columns_count`, `table.freeze_rows` with `table.freeze_rows_count`, each count
        hidden behind its own toggle (Table.tsx). egret's column half already collapsed its pair
        into one number, on the grounds that a count of 0 says exactly what the toggle-off state
        says; the row half takes the same shape, and now that there are two of them the consistency
        is its own argument. A panel that spelled the same idea as a number on one axis and a
        switch-plus-number on the other would be teaching it twice.

        WHAT THE COLLAPSE GIVES UP: Metabase keeps the count while the toggle is off, so off-then-on
        returns to the last number used. Zero does not remember. Accepted here for the same reason
        it was accepted for `pinnedLeft` — a freeze count is a single digit in plain view, not
        configuration worth carrying through a state it cannot be seen in.

        NO `max`, unlike its sibling above, and that is a limit of where this control lives rather
        than an oversight: this section is handed `columns` and never the rows, so the bound does
        not exist at this level. Metabase's own rows input has no max either — it clamps in the
        renderer (`Math.min(count, rows.length)`, TableInteractive.tsx), which is where `ChartTable`
        clamps it too. The `Math.max(0, …)` below is still needed here: PR #398 measured that the
        `min` attribute only sets `validity.*` flags at form submission and blocks no keystroke.
      */}
      <div className="space-y-1">
        <Label htmlFor="freeze-top" className={FIELD_LABEL}>Freeze first rows</Label>
        <Input
          id="freeze-top" type="number" min={0} value={pinnedTop}
          onChange={(e) => {
            const n = Number(e.target.value);
            onChange({ ...value, pinnedTop: Number.isNaN(n) ? 0 : Math.max(0, n) });
          }}
          className="w-full text-sm"
        />
      </div>

      <div className="flex items-center justify-between">
        <Label htmlFor="row-index" className="text-sm font-normal">Show row index</Label>
        <Switch id="row-index" checked={!!value.rowIndex}
          onCheckedChange={(v) => onChange({ ...value, rowIndex: v })} />
      </div>

      <div className="space-y-2">
        <div className={GROUP_LABEL}>Pagination</div>
        <div className="flex items-center justify-between">
          <Label htmlFor="paginate" className="text-sm font-normal">Paginate</Label>
          <Switch id="paginate" checked={pagination.enabled}
            onCheckedChange={(v) => onChange({ ...value, pagination: { ...pagination, enabled: v } })} />
        </div>
        {pagination.enabled && (
          <div className="space-y-1">
            <Label className={FIELD_LABEL}>Page size</Label>
            <Select value={String(pagination.pageSize)} onValueChange={(v) => onChange({ ...value, pagination: { ...pagination, pageSize: Number(v) } })}>
              <SelectTrigger className="w-full text-sm"><SelectValue /></SelectTrigger>
              <SelectContent>
                {PAGE_SIZES.map((s) => <SelectItem key={s} value={String(s)}>{s}</SelectItem>)}
              </SelectContent>
            </Select>
          </div>
        )}
      </div>
      </>)}
    </div>
  );
}
