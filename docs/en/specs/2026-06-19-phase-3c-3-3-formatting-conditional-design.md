# Phase 3c-3.3 — Value Formatting + Conditional Formatting + Settings Sidebar

> Status: approved design. Canonical (English). Vietnamese mirror:
> `docs/vi/specs/2026-06-19-phase-3c-3-3-formatting-conditional-design.md`.
> Extends the chart feature (Phase 3c-3 / 3c-3.2, merged `643ea75`). Branch
> `feat/phase-3c-3-3-formatting`.

## Goal

Bring egret's **Table** and **Pivot Table** to Metabase parity for *presentation*: per-column
**value formatting** (number / currency / percent / date) and **conditional formatting**
(single-color rules + color-range heatmaps), configured through a Metabase-style **settings
sidebar**. This is a faithful port of Metabase's model (the user's explicit directive: copy
Metabase closely — see [[metabase-fidelity-preference]]).

This is the first of four parity sub-phases (3c-3.3 formatting/conditional → 3c-3.4 table
interactions → 3c-3.5 pivot collapse/sticky/sort → 3c-3.6 drill-through).

## Scope

**In scope:**

- A pure **formatting engine** (`formatValue`) — `Intl.NumberFormat` for numbers, **dayjs**
  for dates — covering Metabase's number + date option set.
- A pure **conditional-formatting engine** (`makeCellBackgroundGetter`) — single-color
  operator rules + color-range gradients, first-match-wins, alpha-blended.
- A **settings sidebar** in the chart Explore page with two sections: per-column **Format**
  and viz-level **Conditional Formatting** (sortable rule list + rule editor).
- Wiring into **Table** and **Pivot** cell rendering; persistence in chart `params`;
  hydration on load.
- New dependency: **dayjs**.

**Out of scope (deferred):**

- Drag-to-reorder rules (use up/down buttons; drag → 3c-3.4+).
- Metabase's type-aware field-ref column key (we key by column **name**, unique per chart).
- Table interactions (resize/reorder/visibility/sticky/totals/pagination) → 3c-3.4.
- Pivot collapse/sticky/sort → 3c-3.5; drill-through → 3c-3.6.
- Markdown/link/image/email cell rendering, mini-bars, coordinate formatting.
- Global/admin default formatting (Metabase's `custom-formatting`) — per-chart only.

**No backend change** — `params` is opaque JSONB; all settings live there.

## Background — the Metabase models we mirror

**Formatting** (`metabase/.../lib/settings/column.ts`, `utils/formatting/numbers.tsx`,
`lib/formatting/date.tsx`): per-column settings under `column_settings[key]`, applied by
`formatValue(value, options)` which routes to `formatNumber` (`Intl.NumberFormat`) or
`formatDateTimeWithUnit` (dayjs). Number keys: `number_style` (decimal/percent/scientific/
currency), `currency`, `currency_style`, `decimals`, `scale`, `prefix`, `suffix`,
`number_separators`, compact, negative-in-parentheses. Date keys: `date_style`,
`date_separator`, `date_abbreviate`, `time_enabled`, `time_style`.

**Conditional formatting** (`lib/table_format.ts`, `components/settings/
ChartSettingsTableFormatting/`): a viz-level array `table.column_formatting` of rules.
Single-color: `{ columns, type:"single", operator, value, color, highlight_row }`.
Color-range: `{ columns, type:"range", colors, min_type, max_type, min_value, max_value }`.
`makeCellBackgroundGetter` compiles rules → `(value, rowIndex, colName) => color|null`
(first match wins; cell alpha 0.65, row alpha 0.2; gradient via d3-scale). Pivot disables
`highlight_row` and restricts to numeric value cells.

## 1. Persistence — chart `params` additions

`ChartParams` (in `frontend/src/api/charts.ts`) gains two optional fields, stored verbatim
in the `params` JSONB:

```ts
export interface ChartParams {
  spec: QuerySpec;
  rows?: string[]; cols?: string[]; metrics?: string[];
  showRowTotals?: boolean; showColTotals?: boolean; condense?: boolean;
  // NEW:
  columnSettings?: Record<string, ColumnFormat>; // keyed by result column name
  conditionalFormatting?: Rule[];                 // viz-level, ordered (first match wins)
}
```

## 2. Data model (`frontend/src/components/chart/format-types.ts`)

```ts
export type NumberStyle = "decimal" | "percent" | "currency" | "scientific";

export interface ColumnFormat {
  // number
  number_style?: NumberStyle;
  currency?: string;            // ISO 4217, default "USD"
  currency_style?: "symbol" | "code" | "name" | "narrowSymbol";
  decimals?: number;            // exact fraction digits (min=max)
  scale?: number;               // multiply before formatting
  prefix?: string;
  suffix?: string;
  number_separators?: string;   // 2 chars: "[decimal][thousands]", default ".,"
  compact?: boolean;            // 1.2K / 3.4M
  negative_in_parentheses?: boolean;
  // date
  date_style?: string;          // dayjs format, e.g. "MMMM D, YYYY"
  date_separator?: "/" | "-" | ".";
  date_abbreviate?: boolean;
  time_enabled?: "minutes" | "seconds" | "milliseconds" | null;
  time_style?: "h:mm A" | "HH:mm";
  // common
  column_title?: string;        // override the displayed header
}

export type FormattingOperator =
  | "=" | "!=" | "<" | ">" | "<=" | ">="
  | "contains" | "does-not-contain" | "starts-with" | "ends-with"
  | "is-true" | "is-false" | "is-null" | "not-null";

export interface SingleColorRule {
  columns: string[];
  type: "single";
  operator: FormattingOperator;
  value?: string | number;      // omitted for is-null/not-null/is-true/is-false
  color: string;                // hex
  highlight_row?: boolean;      // ignored in pivot
}
export interface ColorRangeRule {
  columns: string[];
  type: "range";
  colors: string[];             // 2 or 3 hex colors
  min_type: "custom" | "all" | null; // null = per-column min
  max_type: "custom" | "all" | null;
  min_value?: number;           // when min_type === "custom"
  max_value?: number;
}
export type Rule = SingleColorRule | ColorRangeRule;
```

## 3. Formatting engine (`frontend/src/components/chart/format.ts`, pure)

`formatValue(value, columnType, fmt?) -> string`:

- **Null/empty** → `""`.
- **Numeric** column (or `number_style` set): apply `scale`; `scientific` →
  `toExponential(decimals ?? 2)`; `compact` → `Intl.NumberFormat("en",{notation:"compact",
  maximumFractionDigits: decimals ?? 2})`; else `Intl.NumberFormat("en", { style, currency,
  currencyDisplay: currency_style, minimumFractionDigits: decimals ?? 0,
  maximumFractionDigits: decimals ?? (style==="currency"?2:3), useGrouping:true })`. Then
  apply `number_separators` (post-replace), `negative_in_parentheses`, `prefix`/`suffix`.
  Numerics arriving as strings are coerced (the StarRocks text-protocol case).
- **Temporal** column: `dayjs(value)`; if invalid → `String(value)`. Resolve the dayjs format
  from `date_style` (default `"MMMM D, YYYY"`) with `date_separator` replacement and
  `date_abbreviate` (`MMMM`→`MMM`, `dddd`→`ddd`); append a time format from `time_style` +
  `time_enabled` granularity when the value carries a time.
- **Otherwise** → `String(value)` (with `prefix`/`suffix` if set).

`columnType` is the result column's engine type string (the `type` from query results) →
classified as numeric / temporal / other (reuse the `NUMERIC` regex pattern already in
`ChartTable`, plus a temporal regex `date|time|timestamp`). Defaults: when no `ColumnFormat`,
numbers use locale grouping (current behavior), dates use `MMMM D, YYYY`.

## 4. Conditional engine (`frontend/src/components/chart/conditional.ts`, pure)

`makeCellBackgroundGetter(rows: unknown[][], colIndexByName: Record<string,number>, rules:
Rule[], opts?: { isPivot?: boolean }) -> (value, colName) => string | null`:

- Precompute per-column numeric **extents** (min/max ignoring non-numbers) once, used by
  range rules with `min_type/max_type === null` or `"all"`.
- For each cell, iterate rules **in order**; the first rule whose `columns` includes
  `colName` and that yields a color wins.
- **Single-color:** evaluate `operator` against the (coerced) value; on match return
  `alpha(color, isRowFormatter ? 0.2 : 0.65)`. Operators: numeric comparisons coerce to
  number; string ops use `String(value)`; `is-null`/`not-null`/`is-true`/`is-false` take no
  `value`. `highlight_row` rules produce a row-level color **only when `!isPivot`** (handled
  by a separate row-getter the renderers may call; pivots ignore it).
- **Color-range:** resolve `[min,max]` from extents/custom; build a gradient over 2–3 colors
  by **linear RGB interpolation** of the hex endpoints (a small local helper — no d3
  dependency), clamp to `[min,max]`, alpha 0.75; non-numeric value → `null`.
- `alpha(hex, a)` = a small hex→`rgba()` helper.

## 5. Settings sidebar (`frontend/src/components/chart/settings/`)

A right-hand panel in the Explore page, toggled by a **Settings** (gear) button. Two
sections (a simple section switcher, mirroring Metabase's sidebar):

### 5.1 `FormatSection.tsx` — per-column formatting
- A column picker (the result columns / the dataset's columns for the chart). Selecting a
  column reveals type-aware controls writing `columnSettings[colName]`:
  - **Numeric:** Style (Normal/Percent/Currency/Scientific) · Currency + Currency-style (when
    currency) · Decimals (number) · Multiply-by (scale) · Separators · Compact toggle ·
    Negative-in-parentheses toggle · Prefix · Suffix.
  - **Temporal:** Date style (preset list rendered with an example date) · Separator (/ - .) ·
    Abbreviate toggle · Time (Off / HH:MM / HH:MM:SS) · 12/24-hour.
  - **All:** Column title override.
- Controls hide based on dependencies (currency options only when `number_style==="currency"`,
  etc.) — mirror `getHidden` from Metabase.

### 5.2 `ConditionalFormattingSection.tsx` — viz-level rules
- A **rule list**: each rule a card (columns + a one-line description + a color/gradient swatch)
  with **up/down** reorder + delete; an **Add rule** button.
- A **`RuleEditor`** (inline, replaces the list while editing):
  1. **Columns** — a multi-select of formattable columns (numeric/string/boolean). Selecting
     a column type disables incompatible columns (operators must agree).
  2. **Type** — Single color / Color range radio (range only offered for all-numeric column
     sets).
  3. **Single color:** Operator select (from the selected columns' type), Value input (hidden
     for null/true/false ops), Color picker, "Highlight whole row" toggle (hidden in pivot).
  4. **Color range:** a 2–3 color range picker; Min/Max extent radios (per-column / all-columns
     (multi) / custom) with a numeric input when custom.
  5. Add/Update/Delete/Cancel.

A small color palette + a few preset color-ranges are provided as constants.

## 6. Renderer integration

- **`ChartTable`** gains optional props `columnSettings?` + `conditionalFormatting?`. Cell text
  → `formatValue(value, col.type, columnSettings[col.name])`; cell `style.backgroundColor` →
  the getter `(value, col.name)`. A `highlight_row` getter colors the whole `<tr>` when not
  pivot.
- **`PivotTable`** value cells → `formatValue(value, metricType, columnSettings[metricName])`;
  background via the getter keyed by the metric/leaf column; `isPivot:true` (numeric-only, no
  row highlight). The engine pre-formats values to strings today. **Decision:** change
  `buildPivot` so `BodyRow.values` holds **raw `CellValue[]`** (drop the engine's internal
  `fmt`); the **renderer** applies `formatValue` (display) and the background getter (which
  needs the raw number). The engine's existing string-`values` test assertions are updated to
  raw values.

## 7. Explore integration

`ChartExplore` holds `columnSettings` + `conditionalFormatting` state, renders the **Settings**
toggle + sidebar, passes both into the active renderer, includes them in `paramsForSave`, and
restores them in the hydration effect. The result columns for the settings UI come from the
last successful `results.data.columns` (numeric/temporal classification from each column's
`type`).

## 8. Error handling

- Unknown column in a rule / settings (e.g. after the query shape changes) → that rule/setting
  is simply inert (no match), never throws.
- Invalid format option (e.g. negative decimals) → clamp/ignore; formatting never throws.
- A rule referencing a removed column stays in `params` but produces no color.

## 9. Testing

- **`format.ts` unit tests:** decimal/percent/currency/scientific, decimals, scale,
  separators, compact, negative-in-parentheses, prefix/suffix; date style/separator/abbreviate/
  time; null/invalid; string-encoded numerics.
- **`conditional.ts` unit tests:** each operator (match + non-match), is-null/not-null, string
  ops, first-match-wins ordering, color-range gradient endpoints + midpoint + clamp, per-column
  vs all-columns vs custom extent, alpha output, non-numeric → null.
- **Component tests:** `RuleEditor` builds a correct rule object for single + range; the Format
  section writes the right `columnSettings`.
- **Renderer tests:** `ChartTable` applies a currency format + a single-color rule (asserts
  formatted text + a background style); `PivotTable` applies a format to value cells.
- **Manual Playwright smoke:** open the seeded pivot, add a currency format + a heatmap rule via
  the sidebar, confirm live render; save → reopen → settings hydrate.

## 10. File structure

- Create `frontend/src/components/chart/format-types.ts` (shared types).
- Create `frontend/src/components/chart/format.ts` (+ tests) — `formatValue`.
- Create `frontend/src/components/chart/conditional.ts` (+ tests) — getter + color helpers.
- Create `frontend/src/components/chart/settings/SettingsSidebar.tsx`,
  `FormatSection.tsx`, `ConditionalFormattingSection.tsx`, `RuleEditor.tsx` (+ tests).
- Modify `frontend/src/components/chart/ChartTable.tsx` and
  `frontend/src/components/chart/pivot/PivotTable.tsx` (+ `engine.ts` raw-value exposure).
- Modify `frontend/src/api/charts.ts` (`ChartParams`) and
  `frontend/src/routes/explore/ChartExplore.tsx` (sidebar host + persist + hydrate).
- Modify `frontend/package.json` (add `dayjs`).
