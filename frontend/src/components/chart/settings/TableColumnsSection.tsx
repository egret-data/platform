import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";
import type { TableState } from "@/api/charts";
import type { ColumnFormat } from "@/components/chart/format-types";
import { Checkbox } from "@/components/ui/checkbox";
import { IconButton } from "@/components/ui/icon-button";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import { displayName, fieldTitle, type FieldLabels } from "@/components/chart/config/field-names";
import { colIcon, colKind } from "./FormatSection";
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
 *   Data ▸ "Columns"            what the QUERY projects        (viz/table.tsx, still there)
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
export function TableColumnsSection({
  columns, value, onChange, columnSettings, onColumnSettings, queryFields,
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
}) {
  const [filter, setFilter] = useState("");
  // Metabase's own shape: the button REPLACES the list with the field panel rather than opening a
  // second thing beside it, and turns into "Done picking columns". Replacing is the point — the
  // whole defect being fixed is two column lists on screen at once.
  const [picking, setPicking] = useState(false);
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

  function toggleHidden(name: string) {
    const next = new Set(hidden);
    if (next.has(name)) next.delete(name); else next.add(name);
    onChange({ ...value, hiddenColumns: [...next] });
  }

  /**
   * Swap two rows and write the WHOLE display order back.
   *
   * Not `SeriesListSection`'s rule, which patches inside the stored array so that a series absent
   * from this run keeps its remembered place. A table has no equivalent: `reconcileTableState`
   * prunes `columnOrder` of every name that has left the result the moment it leaves, so there is
   * nothing absent to preserve — and `ChartTable`'s own header drag has always written the full
   * list this same way. Two writers of one key that disagreed about its shape would be worse than
   * either shape.
   */
  const move = (i: number, delta: number) => {
    const j = i + delta;
    if (j < 0 || j >= ordered.length) return;
    const names = ordered.map((c) => c.name);
    [names[i], names[j]] = [names[j], names[i]];
    onChange({ ...value, columnOrder: names });
  };

  // Deliberately `columns`, not the filtered list: a filter is a way to FIND a column, not a
  // selection. "Hide all" while a filter is active must not quietly hide only what is on screen.
  const setAllHidden = (hide: boolean) =>
    onChange({ ...value, hiddenColumns: hide ? columns.map((c) => c.name) : [] });

  return (
    <div className="space-y-3 text-sm">
      <div className="space-y-1">
        {/* No inner "Visible columns" heading any more. There is one list now, and its caller
            already labels it "Columns" — the second heading was only ever there to tell it apart
            from the OTHER column list, which is the thing this section removed. */}
        <div className="flex items-center justify-end">
          {!picking && (
            <div className="flex gap-2">
              <button type="button" className="text-xs text-muted-foreground hover:text-foreground hover:underline"
                onClick={() => setAllHidden(false)}>
                Show all
              </button>
              <button type="button" className="text-xs text-muted-foreground hover:text-foreground hover:underline"
                onClick={() => setAllHidden(true)}>
                Hide all
              </button>
            </div>
          )}
        </div>

        {picking && queryFields ? (
          <div className="space-y-3 rounded-md border p-2">
            {queryFields.map((g, gi) => (
              <div key={g.label} className="space-y-1">
                <div className={`truncate ${GROUP_LABEL}`} title={g.label}>{g.label}</div>
                {g.options.length === 0 && (
                  <p className="text-xs italic text-muted-foreground">Nothing to pick from.</p>
                )}
                {/* THE ROW IS THE TARGET, AND THE NAME IS THE DATASET'S.
                    This is the Data tab's field list reached from the Display tab — the same
                    fields, built by `ChartExplore` out of the same `columns.map(c => c.column_name)`
                    — and it got neither of the two things that list got. #441 measured the Data
                    tab's version at a 16×16 box beside an 11×20 label in a row of which 235px did
                    nothing, and made its rows full-width 32px `<label>`s with hover; #444 taught it
                    to read `verbose_name`. Measured here on the kind deployment before this change:
                    a `<div>` 237px wide and 20px tall, of which `region`'s box-plus-label spanned
                    62px, saying `region` while the Data tab one click away said "Khu vực".
                    Both fixes are now literally the same markup and the same helper as `table.tsx`'s
                    `CheckList`, which is the point — the defect was two lists of one thing
                    disagreeing, so a third spelling of the row would have been the same bug again. */}
                {g.options.map((name) => (
                  <label
                    key={name}
                    htmlFor={`qf-${gi}-${name}`}
                    className="flex min-h-8 cursor-pointer items-center gap-2 rounded-md px-1 hover:bg-muted/50"
                  >
                    <Checkbox
                      id={`qf-${gi}-${name}`}
                      checked={g.selected.includes(name)}
                      onCheckedChange={() =>
                        g.onChange(
                          g.selected.includes(name)
                            ? g.selected.filter((x) => x !== name)
                            : [...g.selected, name],
                        )
                      }
                    />
                    {/* The friendly name with the machine name on hover — `CheckList`'s anatomy,
                        for the reason it gives: a second LINE doubles the height of renamed rows
                        only, and a list with three renames in it goes ragged. */}
                    <span className="min-w-0 flex-1 truncate text-sm" title={fieldTitle(name, g.labels)}>
                      {displayName(name, g.labels)}
                    </span>
                  </label>
                ))}
              </div>
            ))}
          </div>
        ) : (
          <>
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
            <div className="flex flex-col gap-1">
              {visibleColumns.map((c) => {
                const i = ordered.indexOf(c);
                const isHidden = hidden.has(c.name);
                const title = titleFor(c);
                // A dashboard card carries its columns' NAMES and not their types
                // (`DashboardView` builds them from the chart's stored spec), so `c.type` is
                // routinely "". `colIcon`/`colKind` fall through to Text for an unknown type, and
                // a TEXT badge on `subtotal` is a confident false statement — worse than no badge.
                // Rendered as an absent type instead, with the icon's box kept so the names below
                // it still line up.
                const Icon = c.type ? colIcon(c.type) : null;
                return (
                  <div
                    key={c.name}
                    data-column={c.name}
                    // Metabase marks a hidden row and LEAVES IT IN PLACE (`data-enabled="false"`)
                    // rather than moving it to the end or into a second list. That is what makes
                    // "turn it back on" findable: the row is where the reader last saw it.
                    data-enabled={isHidden ? "false" : "true"}
                    className={`flex items-center gap-1 rounded-md border px-1.5 py-1 ${isHidden ? "bg-muted/30 opacity-60" : ""}`}
                  >
                    <div className="flex shrink-0 flex-col leading-none">
                      {/* Disabled under an active filter, both arrows: the row above a match is
                          off screen, so "up" would swap with something the reader cannot see. */}
                      {/* 22 of these on a table panel, two per column, and they measured 8×10 px.
                          `IconButton` also carries the focus ring this markup spelled out by hand. */}
                      <IconButton aria-label={`Move ${c.name} up`} disabled={!!q || i === 0}
                        title={q ? "Clear the search to reorder" : undefined}
                        className="text-[10px]" onClick={() => move(i, -1)}>▲</IconButton>
                      <IconButton aria-label={`Move ${c.name} down`} disabled={!!q || i === ordered.length - 1}
                        title={q ? "Clear the search to reorder" : undefined}
                        className="text-[10px]" onClick={() => move(i, 1)}>▼</IconButton>
                    </div>
                    {Icon
                      ? <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" aria-hidden />
                      : <span className="h-3.5 w-3.5 shrink-0" aria-hidden />}
                    <span className="min-w-0 flex-1 truncate" title={c.name}>
                      {title}
                      {/* The raw name stays beside the rename — the anatomy the Formatting list
                          already had. It is the name every other tab, the SQL and the drill
                          menu call this column, so a renamed column that showed only its title
                          would be unrecognisable from anywhere else in the app. */}
                      {title !== c.name && (
                        <span className="ml-1.5 text-xs text-muted-foreground">{c.name}</span>
                      )}
                    </span>
                    {c.type && (
                      <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase text-muted-foreground">
                        {colKind(c.type)}
                      </span>
                    )}
                    {onColumnSettings && (
                      <ColumnSettingsPopover
                        column={c}
                        value={columnSettings ?? {}}
                        onChange={onColumnSettings}
                      />
                    )}
                    {/* An eye, not the sibling list's Switch: Metabase's column row uses one, and
                        forty switches down a panel this narrow is a different weight of control
                        than ten. `aria-pressed` carries the state, so it is not colour-only. */}
                    <button
                      type="button"
                      aria-label={`Show ${c.name}`}
                      aria-pressed={!isHidden}
                      onClick={() => toggleHidden(c.name)}
                      className="flex w-7 shrink-0 items-center justify-center rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring text-muted-foreground hover:bg-accent hover:text-foreground"
                    >
                      {isHidden ? <EyeOff className="h-3.5 w-3.5" aria-hidden /> : <Eye className="h-3.5 w-3.5" aria-hidden />}
                    </button>
                  </div>
                );
              })}
            </div>
          </>
        )}

        {queryFields && (
          <button
            type="button"
            className="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
            onClick={() => { setPicking((v) => !v); setFilter(""); }}
          >
            {picking ? "Done picking columns" : "Add or remove columns"}
          </button>
        )}
      </div>

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
    </div>
  );
}
