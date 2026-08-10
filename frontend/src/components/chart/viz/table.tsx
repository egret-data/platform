import { useState } from "react";
import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FIELD_LABEL as LABEL_CLASS } from "@/components/chart/settings/FieldLabel";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { isParamsV2 } from "@/api/charts";
import type { QuerySpec } from "@/api/charts";
import {
  FIELD_FILTER_THRESHOLD, columnLabels, displayName, fieldTitle, matchesField, metricLabels,
  type FieldLabels,
} from "@/components/chart/config/field-names";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";

export interface TableConfig { columns: string[]; metrics: string[] }
export interface ObjectConfig { columns: string[] }

/**
 * A checkbox list — the shape both of these viz types configure with.
 *
 * WHY THIS IS STILL A CHECKBOX LIST AND NOT THE PIVOT'S DRAG ZONES. The two sit one screen apart on
 * the same tab and both answer "which fields does this chart use", which is a fair thing to call
 * inconsistent — but Metabase draws the same two shapes for the same two jobs, and for a reason
 * that survives being copied. Its notebook Data step is a checkbox list with a tri-state "Select
 * all" (`querying/notebook/components/FieldPicker/FieldPicker.tsx`); its pivot settings are drag
 * zones. A table's columns are a SET chosen out of many, where the common act is "not that one";
 * a pivot's rows/columns/values are three small ORDERED lists where the common act is "this one,
 * here". Ticking twenty boxes to build a set is right; dragging twenty chips to build one is not.
 *
 * So the gap that mattered was never the widget — it was that this one had none of the widget's
 * affordances. It had no count, no select-all, no filter, no bounded height (ten rows on a
 * ten-column dataset, fifty on a fifty-column one), and each row's only target was a 16×16
 * checkbox with a label beside it — measured live at 16×16 and 11×20 for the column named "id",
 * against the 24×24 minimum in WCAG 2.2 SC 2.5.8, in a row 255px wide of which 235px did nothing.
 * Metabase's own row is a full-width `<label>` at `min-height: 2.25rem` with a hover background,
 * which is what this is now.
 *
 * MAKING IT REORDERABLE IS NOT PART OF THIS, AND IS NOW SETTLED RATHER THAN DEFERRED. #441 left
 * this open on the grounds that a second order needed a rule; the rule is that THIS LIST NEVER
 * REORDERS, and the reason is stronger than "two places to store it". The Data tab's list is the
 * QUERY'S PROJECTION and the Display tab's is the order the table DRAWS in — `TableColumnsSection`,
 * backed by `tableState.columnOrder`, which #410 made the list the table actually renders from.
 * A drag added here would therefore be a control with NO VISIBLE EFFECT: whatever order it wrote,
 * the Display tab's `columnOrder` would still decide what appears on screen. A control that looks
 * like it works and does nothing is worse than an absent one.
 *
 * The two alternatives, both rejected. Making Display FOLLOW Data would discard an author's
 * explicit column order every time they ticked a box one tab over — the same silent-loss failure
 * `reconcileTableState` exists to prevent. Making Data follow Display cannot be built: this is a
 * checkbox list with no drag to follow it with. And a single shared order is what #410 ALREADY
 * SHIPPED, on the Display tab; re-siting it here would undo that PR's projection-versus-presentation
 * distinction, which is the one thing its body asks the next reader not to do.
 *
 * WHAT WAS ACTUALLY LEFT was that this list and the pivot's zones NAMED THE SAME COLUMN DIFFERENTLY
 * — see `field-names.ts`. That is the real residue of "it looks ugly next to the field zones", and
 * it is a naming fix, not a widget change.
 */
function CheckList({ idPrefix, empty, options, selected, onToggle, onSetAll, labels }: {
  idPrefix: string;
  empty: string;
  options: string[];
  selected: string[];
  onToggle: (name: string) => void;
  /** Replaces the whole selection — the same `string[]` the toggles produce, never a new shape. */
  onSetAll: (next: string[]) => void;
  /** Machine name -> display name. Identity stays raw; this only changes what a row SAYS. */
  labels?: FieldLabels;
}) {
  const [query, setQuery] = useState("");
  const shown = options.filter((o) => matchesField(o, query, labels));
  const chosen = options.filter((o) => selected.includes(o));
  const allOn = options.length > 0 && chosen.length === options.length;

  if (options.length === 0) return <p className="text-xs italic text-muted-foreground">{empty}</p>;

  return (
    <div className="space-y-1 pt-1">
      {options.length > FIELD_FILTER_THRESHOLD && (
        <Input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={`Filter ${options.length} fields…`}
          aria-label="Filter fields"
          className="w-full text-sm"
        />
      )}
      {/* Select-all is INDETERMINATE at a partial selection rather than showing an empty box that
          would tick everything — that is Metabase's `indeterminate={!isAll && !isNone}` and it is
          the only way the control can state the list's state as well as offer an action. It also
          carries the count, which is the one fact a scrolled list hides. */}
      <label className="flex min-h-8 cursor-pointer items-center gap-2 rounded-md border-b px-1 hover:bg-muted/50">
        <Checkbox
          checked={allOn ? true : chosen.length === 0 ? false : "indeterminate"}
          aria-label="Select all fields"
          onCheckedChange={() => onSetAll(allOn ? [] : options)}
        />
        <span className="text-sm text-muted-foreground">
          {chosen.length} of {options.length} selected
        </span>
      </label>
      {/* Bounded, and the bound is why: without it the panel's own scrollbar was the only one, so
          on a wide table the Row limit below this list was several screens down and the author had
          to scroll past every column to reach it. `max-h-64` is eight rows — past the point where
          the filter above is the faster way in. */}
      <div className="max-h-64 space-y-0.5 overflow-y-auto [scrollbar-gutter:stable]">
        {shown.length === 0 && (
          <p className="px-1 py-2 text-xs text-muted-foreground">No field matches “{query.trim()}”.</p>
        )}
        {shown.map((name) => (
          // The whole row is the target: a `<label>` wrapping the checkbox, so the 235px that used
          // to do nothing now toggles it, and the row is at least 32px tall in both axes.
          <label
            key={name}
            htmlFor={`${idPrefix}-${name}`}
            className="flex min-h-8 cursor-pointer items-center gap-2 rounded-md px-1 hover:bg-muted/50"
          >
            <Checkbox id={`${idPrefix}-${name}`} checked={selected.includes(name)} onCheckedChange={() => onToggle(name)} />
            {/* The friendly name, with the machine name on hover — the chip in `FieldZone` has said
                it exactly this way since #438, and matching it is the point of the change. A second
                LINE showing the raw name (which the zone's own popover uses) was tried and dropped:
                it doubles the height of renamed rows only, so a list with three renames in it goes
                ragged, and this list is bounded and scrolled where that popover is neither. */}
            <span className="min-w-0 flex-1 truncate text-sm" title={fieldTitle(name, labels)}>
              {displayName(name, labels)}
            </span>
          </label>
        ))}
      </div>
    </div>
  );
}

const toggle = (list: string[], name: string) =>
  list.includes(name) ? list.filter((x) => x !== name) : [...list, name];

function TableMetricsControl({ config, onChange, metrics }: VizControlProps<TableConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Metrics</Label>
      {/* ONE LINE, NOT THREE. This block used to open with a two-line instruction and then, on the
          common case of a dataset that defines no metrics, an italic "No metrics on this dataset."
          — four lines of prose before the reader reached anything they could act on, at the very
          top of the tab. The sentence still has to be said (a table with a metric aggregates and
          one without does not, and nothing else on screen says so), so it is said once and short;
          `CheckList` owns the empty case, which is where it belongs. */}
      <p className="text-xs text-muted-foreground">With none, the table lists raw rows.</p>
      <CheckList idPrefix="metric" empty="No metrics on this dataset."
        options={metrics.map((m) => m.metric_name)} selected={config.metrics}
        labels={metricLabels(metrics)}
        onToggle={(n) => onChange({ metrics: toggle(config.metrics, n) })}
        onSetAll={(next) => onChange({ metrics: next })} />
    </div>
  );
}

function TableColumnsControl({ config, onChange, columns, unitRow }: VizControlProps<TableConfig>) {
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Columns</Label>
        <CheckList idPrefix="col" empty="Select a dataset to see columns."
          options={columns.map((c) => c.column_name)} selected={config.columns}
          labels={columnLabels(columns)}
          onToggle={(n) => onChange({ columns: toggle(config.columns, n) })}
          onSetAll={(next) => onChange({ columns: next })} />
      </div>
      {config.metrics.length > 0 && config.columns.map((c) => <div key={c}>{unitRow(c)}</div>)}
    </>
  );
}

function ObjectFieldsControl({ config, onChange, columns }: VizControlProps<ObjectConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Fields</Label>
      {/* The object-detail viz picks from the same dataset columns through the same widget, so it
          gets the same names — it was never a separate decision, only a separate call site. */}
      <CheckList idPrefix="obj" empty="Select a dataset to see columns."
        options={columns.map((c) => c.column_name)} selected={config.columns}
        labels={columnLabels(columns)}
        onToggle={(n) => onChange({ columns: toggle(config.columns, n) })}
        onSetAll={(next) => onChange({ columns: next })} />
    </div>
  );
}

export function tableDescriptor(): VizDescriptor<TableConfig> {
  return {
    key: "table",
    meta: CHART_TYPES.find((t) => t.key === "table")!,
    prefix: "table.",
    defaultConfig: { columns: [], metrics: [] },
    fromParams: (p) => {
      // v1 stored no table config at all: the columns were re-derived from the spec. A v2 row
      // whose `settings` never got a `table.*` slice needs the same recovery — the API accepts
      // `settings: {}` (keys are deliberately open), so a chart written by anything but this
      // editor arrives that way, and reading only `settings` opened it on "add at least one
      // column or metric" with its columns sitting untouched in the spec.
      const fromSpec = {
        metrics: p.spec?.metrics ?? [],
        columns: [...(p.spec?.dimensions ?? []), ...(p.spec?.columns ?? [])],
      };
      if (isParamsV2(p)) {
        // `??` on the stored key, not on its emptiness: a metrics-only table stores
        // `table.columns: []` on purpose, and re-deriving would put the columns back.
        return {
          columns: (p.settings["table.columns"] as string[]) ?? fromSpec.columns,
          metrics: (p.settings["table.metrics"] as string[]) ?? fromSpec.metrics,
        };
      }
      return fromSpec;
    },
    toSettings: (cfg) => ({ "table.columns": cfg.columns, "table.metrics": cfg.metrics }),
    // A bare table is valid — with no metric it just lists raw rows — but an EMPTY one is not.
    // Returning [] unconditionally told the runner the chart was ready the moment a dataset was
    // picked, so it fired `columns: [], metrics: []` and the backend answered 400 twice before the
    // user had chosen anything. The preview now asks for a field instead of failing a query.
    requiredFields: (cfg) => (cfg.metrics.length > 0 || cfg.columns.length > 0 ? [] : ["at least one column or metric"]),
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => {
      const agg = cfg.metrics.length > 0;
      const dims = agg ? cfg.columns : [];
      return {
        metrics: cfg.metrics, dimensions: dims, columns: agg ? [] : cfg.columns,
        // The table's own header click sorts the rows ON SCREEN, within the page it already has.
        // `shared.orderby` is the other thing: it sorts in the QUERY, so it decides which rows come
        // back at all under `row_limit`. Both branches carry it — `orderby_sql` resolves a raw
        // column as readily as a dimension or a metric.
        filters: shared.filters, orderby: shared.orderby, row_limit: shared.rowLimit, rollup_groups: [],
        temporal_units: pruneUnits(shared.temporalUnits, dims),
      };
    },
    controls: [
      { id: "metrics", section: "data", render: TableMetricsControl },
      { id: "columns", section: "data", render: TableColumnsControl },
    ],
  };
}

export function objectDescriptor(): VizDescriptor<ObjectConfig> {
  return {
    key: "object",
    meta: CHART_TYPES.find((t) => t.key === "object")!,
    prefix: "object.",
    defaultConfig: { columns: [] },
    fromParams: (p) => {
      if (isParamsV2(p)) {
        // Falls back to `table.columns` so switching table→object keeps the selection,
        // which is what the shared `tableCols` state did before A2 — and then to the spec,
        // for the same reason the table descriptor does.
        const own = p.settings["object.columns"] as string[] | undefined;
        return { columns: own ?? (p.settings["table.columns"] as string[]) ?? p.spec?.columns ?? [] };
      }
      return { columns: p.spec?.columns ?? [] };
    },
    toSettings: (cfg) => ({ "object.columns": cfg.columns }),
    requiredFields: (cfg) => (cfg.columns.length ? [] : ["at least one column"]),
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => ({
      metrics: [], dimensions: [], columns: cfg.columns,
      filters: shared.filters, orderby: shared.orderby, row_limit: shared.rowLimit, rollup_groups: [],
    }),
    controls: [{ id: "fields", section: "data", render: ObjectFieldsControl }],
  };
}
