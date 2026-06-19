# Phase 3c-3.3 — Formatting + Conditional Formatting + Settings Sidebar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring egret's Table + Pivot to Metabase parity for presentation — per-column value formatting (number/currency/percent/date) + conditional formatting (single-color rules + color-range heatmaps), configured via a Metabase-style settings sidebar.

**Architecture:** Two pure engines — `format.ts` (`formatValue` via `Intl.NumberFormat` + dayjs) and `conditional.ts` (`makeCellBackgroundGetter` with operators + dep-free hex-lerp gradient + alpha). A settings sidebar (Format + Conditional Formatting sections) writes `columnSettings` + `conditionalFormatting` into chart `params`. Table + Pivot renderers apply both. No backend change.

**Tech Stack:** React 18 + Vite + TypeScript + Vitest + shadcn/ui; new dep **dayjs**.

**Spec:** `docs/en/specs/2026-06-19-phase-3c-3-3-formatting-conditional-design.md`.

**Branch:** `feat/phase-3c-3-3-formatting` (spec already committed).

**Conventions:** plain `git commit` (never `--author`); `vitest.setup.ts` registers jest-dom; shadcn primitives in `@/components/ui/*` (`select`, `input`, `label`, `checkbox`, `radio-group`, `switch`, `popover`, `button`).

---

## File Structure
- Create `frontend/src/components/chart/format-types.ts` — `ColumnFormat`, `Rule` (+ `SingleColorRule`/`ColorRangeRule`), `FormattingOperator`, type classifiers.
- Create `frontend/src/components/chart/format.ts` (+ tests) — `formatValue`.
- Create `frontend/src/components/chart/conditional.ts` (+ tests) — `buildConditional` + color helpers.
- Modify `frontend/src/api/charts.ts` — `ChartParams` gains `columnSettings` + `conditionalFormatting`.
- Modify `frontend/src/components/chart/ChartTable.tsx` (+ test) — apply formatting + conditional.
- Modify `frontend/src/components/chart/pivot/engine.ts` (+ tests) — `BodyRow.values` → raw `CellValue[]`.
- Modify `frontend/src/components/chart/pivot/PivotTable.tsx` (+ test) — format + conditional value cells.
- Create `frontend/src/components/chart/settings/FormatSection.tsx`, `ConditionalFormattingSection.tsx`, `RuleEditor.tsx`, `SettingsSidebar.tsx` (+ tests).
- Modify `frontend/src/routes/explore/ChartExplore.tsx` — sidebar host + state + persist + hydrate.
- Modify `frontend/package.json` — add `dayjs`.

---

## Task 1: Shared types + dayjs dependency

**Files:**
- Create: `frontend/src/components/chart/format-types.ts`
- Modify: `frontend/package.json`

- [ ] **Step 1: Add dayjs**

Run: `cd frontend && npm install dayjs`
Expected: `dayjs` added to `package.json` dependencies.

- [ ] **Step 2: Create the shared types**

Create `frontend/src/components/chart/format-types.ts`:

```ts
export type NumberStyle = "decimal" | "percent" | "currency" | "scientific";

export interface ColumnFormat {
  number_style?: NumberStyle;
  currency?: string;
  currency_style?: "symbol" | "code" | "name" | "narrowSymbol";
  decimals?: number;
  scale?: number;
  prefix?: string;
  suffix?: string;
  number_separators?: string; // "[decimal][thousands]", default ".,"
  compact?: boolean;
  negative_in_parentheses?: boolean;
  date_style?: string;
  date_separator?: "/" | "-" | ".";
  date_abbreviate?: boolean;
  time_enabled?: "minutes" | "seconds" | "milliseconds" | null;
  time_style?: "h:mm A" | "HH:mm";
  column_title?: string;
}

export type FormattingOperator =
  | "=" | "!=" | "<" | ">" | "<=" | ">="
  | "contains" | "does-not-contain" | "starts-with" | "ends-with"
  | "is-true" | "is-false" | "is-null" | "not-null";

export interface SingleColorRule {
  columns: string[];
  type: "single";
  operator: FormattingOperator;
  value?: string | number;
  color: string;
  highlight_row?: boolean;
}
export interface ColorRangeRule {
  columns: string[];
  type: "range";
  colors: string[];
  min_type: "custom" | "all" | null;
  max_type: "custom" | "all" | null;
  min_value?: number;
  max_value?: number;
}
export type Rule = SingleColorRule | ColorRangeRule;

const NUMERIC_RE = /int|double|decimal|real|numeric|float|bigint/i;
const TEMPORAL_RE = /date|time|timestamp/i;

export function isNumericType(type: string): boolean {
  return NUMERIC_RE.test(type);
}
export function isTemporalType(type: string): boolean {
  // exclude pure numeric types that contain "time" only via false positives — TEMPORAL_RE is safe
  return TEMPORAL_RE.test(type) && !NUMERIC_RE.test(type);
}
```

- [ ] **Step 3: Typecheck + commit**

Run: `cd frontend && npx tsc --noEmit` (the new file is standalone — should not add errors).
```bash
git add frontend/package.json frontend/package-lock.json frontend/src/components/chart/format-types.ts
git commit -m "feat(web): chart formatting/conditional shared types + dayjs dep"
```

---

## Task 2: `formatValue` engine

**Files:**
- Create: `frontend/src/components/chart/format.ts`
- Test: `frontend/src/components/chart/__tests__/format.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/__tests__/format.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { formatValue } from "../format";

describe("formatValue numbers", () => {
  it("null/empty → empty string", () => {
    expect(formatValue(null, "double")).toBe("");
    expect(formatValue(undefined, "double")).toBe("");
  });
  it("default numeric uses locale grouping", () => {
    expect(formatValue(1234567, "bigint")).toBe("1,234,567");
  });
  it("coerces string-encoded numerics", () => {
    expect(formatValue("1234.5", "double")).toBe("1,234.5");
  });
  it("decimals forces fraction digits", () => {
    expect(formatValue(3, "double", { decimals: 2 })).toBe("3.00");
  });
  it("percent style", () => {
    expect(formatValue(0.125, "double", { number_style: "percent", decimals: 1 })).toBe("12.5%");
  });
  it("currency style with code", () => {
    expect(formatValue(1234.5, "double", { number_style: "currency", currency: "USD", currency_style: "code", decimals: 2 })).toBe("USD 1,234.50");
  });
  it("scale multiplies before formatting", () => {
    expect(formatValue(2, "double", { scale: 1000 })).toBe("2,000");
  });
  it("prefix + suffix", () => {
    expect(formatValue(5, "double", { prefix: "~", suffix: " pts" })).toBe("~5 pts");
  });
  it("compact notation", () => {
    expect(formatValue(1500000, "double", { compact: true })).toBe("1.5M");
  });
  it("negative in parentheses", () => {
    expect(formatValue(-42, "double", { negative_in_parentheses: true })).toBe("(42)");
  });
  it("custom separators (space thousands, comma decimal)", () => {
    expect(formatValue(1234.5, "double", { number_separators: ", ", decimals: 1 })).toBe("1 234,5");
  });
});

describe("formatValue dates", () => {
  it("default date style", () => {
    expect(formatValue("2024-01-31", "date")).toBe("January 31, 2024");
  });
  it("abbreviated + custom separator", () => {
    expect(formatValue("2024-01-31", "date", { date_style: "M/D/YYYY", date_separator: "-" })).toBe("1-31-2024");
  });
  it("invalid date → string passthrough", () => {
    expect(formatValue("not-a-date", "date")).toBe("not-a-date");
  });
});

describe("formatValue other", () => {
  it("text passthrough", () => {
    expect(formatValue("hello", "varchar")).toBe("hello");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- chart/__tests__/format`
Expected: FAIL (no `format.ts`).

- [ ] **Step 3: Implement `format.ts`**

Create `frontend/src/components/chart/format.ts`:

```ts
import dayjs from "dayjs";
import { isNumericType, isTemporalType, type ColumnFormat } from "./format-types";

function applySeparators(s: string, separators?: string): string {
  if (!separators || separators === ".,") return s;
  const [dec, thou] = separators;
  // Intl output uses "." decimal and "," thousands (en locale). Swap via placeholder.
  return s.replace(/,/g, " ").replace(/\./g, dec).replace(/ /g, thou ?? "");
}

function formatNumber(value: number, fmt: ColumnFormat): string {
  let n = value;
  if (typeof fmt.scale === "number") n *= fmt.scale;

  const neg = n < 0 && fmt.negative_in_parentheses;
  const abs = neg ? Math.abs(n) : n;

  let out: string;
  if (fmt.number_style === "scientific") {
    out = abs.toExponential(fmt.decimals ?? 2);
  } else if (fmt.compact) {
    out = new Intl.NumberFormat("en", {
      notation: "compact",
      maximumFractionDigits: fmt.decimals ?? 1,
    }).format(abs);
  } else {
    const style = fmt.number_style ?? "decimal";
    const opts: Intl.NumberFormatOptions = { useGrouping: true };
    if (style === "currency") {
      opts.style = "currency";
      opts.currency = fmt.currency ?? "USD";
      opts.currencyDisplay = fmt.currency_style ?? "symbol";
    } else if (style === "percent") {
      opts.style = "percent";
    }
    if (typeof fmt.decimals === "number") {
      opts.minimumFractionDigits = fmt.decimals;
      opts.maximumFractionDigits = fmt.decimals;
    } else {
      opts.maximumFractionDigits = style === "currency" ? 2 : 3;
    }
    out = new Intl.NumberFormat("en", opts).format(abs);
  }

  out = applySeparators(out, fmt.number_separators);
  if (neg) out = `(${out})`;
  return `${fmt.prefix ?? ""}${out}${fmt.suffix ?? ""}`;
}

function formatDate(value: unknown, fmt: ColumnFormat): string {
  const d = dayjs(value as string | number | Date);
  if (!d.isValid()) return String(value);
  let dateFmt = fmt.date_style ?? "MMMM D, YYYY";
  if (fmt.date_separator) dateFmt = dateFmt.replace(/\//g, fmt.date_separator);
  if (fmt.date_abbreviate) dateFmt = dateFmt.replace(/MMMM/g, "MMM").replace(/dddd/g, "ddd");
  let timeFmt = "";
  if (fmt.time_enabled) {
    timeFmt = " " + (fmt.time_style ?? "h:mm A");
    if (fmt.time_enabled === "seconds") timeFmt = timeFmt.replace(/mm/, "mm:ss");
    else if (fmt.time_enabled === "milliseconds") timeFmt = timeFmt.replace(/mm/, "mm:ss.SSS");
  }
  return `${fmt.prefix ?? ""}${d.format(dateFmt + timeFmt)}${fmt.suffix ?? ""}`;
}

/** Format a single cell value for display, honoring an optional per-column format. */
export function formatValue(value: unknown, columnType: string, fmt: ColumnFormat = {}): string {
  if (value == null) return "";
  if (isTemporalType(columnType)) return formatDate(value, fmt);
  if (isNumericType(columnType) || fmt.number_style) {
    const n = typeof value === "number" ? value : Number(value);
    if (Number.isFinite(n) && !(typeof value === "string" && value.trim() === "")) {
      return formatNumber(n, fmt);
    }
  }
  const s = String(value);
  return fmt.prefix || fmt.suffix ? `${fmt.prefix ?? ""}${s}${fmt.suffix ?? ""}` : s;
}
```

NOTE: `Intl` compact uses `1.5M` form; the test expects `"1.5M"`. Currency code style yields `"USD 1,234.50"`. If a runtime check shows a different spacing/format from the Node/jsdom ICU, adjust the EXPECTED test strings to the actual ICU output (do NOT hack the implementation) and note it. The separators test relies on en output using `.`/`,` — the placeholder swap handles it.

- [ ] **Step 4: Run + commit**

Run: `cd frontend && npm test -- chart/__tests__/format` → PASS. `npx tsc --noEmit` clean.
```bash
git add frontend/src/components/chart/format.ts frontend/src/components/chart/__tests__/format.test.ts
git commit -m "feat(web): formatValue engine (Intl numbers + dayjs dates)"
```

---

## Task 3: `conditional.ts` engine

**Files:**
- Create: `frontend/src/components/chart/conditional.ts`
- Test: `frontend/src/components/chart/__tests__/conditional.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/__tests__/conditional.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { buildConditional } from "../conditional";
import type { Rule } from "../format-types";

const rows = [
  ["EU", 10],
  ["US", 50],
  ["JP", 90],
];
const colIndex = { region: 0, revenue: 1 };

describe("buildConditional single-color", () => {
  it("colors cells matching a > operator", () => {
    const rules: Rule[] = [{ columns: ["revenue"], type: "single", operator: ">", value: 40, color: "#ff0000" }];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg(10, "revenue")).toBeNull();
    expect(c.bg(50, "revenue")).toMatch(/^rgba\(255, 0, 0,/);
    expect(c.bg(50, "region")).toBeNull(); // rule not on this column
  });
  it("string contains", () => {
    const rules: Rule[] = [{ columns: ["region"], type: "single", operator: "contains", value: "U", color: "#00ff00" }];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg("EU", "region")).toMatch(/^rgba\(0, 255, 0,/);
    expect(c.bg("JP", "region")).toBeNull();
  });
  it("is-null", () => {
    const rules: Rule[] = [{ columns: ["revenue"], type: "single", operator: "is-null", color: "#0000ff" }];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg(null, "revenue")).toMatch(/^rgba\(0, 0, 255,/);
    expect(c.bg(5, "revenue")).toBeNull();
  });
  it("first matching rule wins", () => {
    const rules: Rule[] = [
      { columns: ["revenue"], type: "single", operator: ">", value: 0, color: "#111111" },
      { columns: ["revenue"], type: "single", operator: ">", value: 0, color: "#222222" },
    ];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg(50, "revenue")).toMatch(/17, 17, 17/); // #111111
  });
});

describe("buildConditional color-range", () => {
  it("interpolates a 2-color gradient across the column extent", () => {
    const rules: Rule[] = [
      { columns: ["revenue"], type: "range", colors: ["#000000", "#ffffff"], min_type: null, max_type: null },
    ];
    const c = buildConditional(rows, colIndex, rules);
    // min (10) → near black, max (90) → near white, midpoint lighter than min
    const lo = c.bg(10, "revenue")!;
    const hi = c.bg(90, "revenue")!;
    expect(lo).toMatch(/^rgba\(0, 0, 0,/);
    expect(hi).toMatch(/^rgba\(255, 255, 255,/);
    expect(c.bg("x", "revenue")).toBeNull(); // non-numeric
  });
  it("custom extent", () => {
    const rules: Rule[] = [
      { columns: ["revenue"], type: "range", colors: ["#000000", "#ffffff"], min_type: "custom", max_type: "custom", min_value: 0, max_value: 100 },
    ];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg(50, "revenue")).toMatch(/^rgba\(128, 128, 128,/); // halfway
  });
});

describe("buildConditional row highlight", () => {
  it("rowBg colors a row when a highlight_row rule matches", () => {
    const rules: Rule[] = [{ columns: ["revenue"], type: "single", operator: ">", value: 80, color: "#abcdef", highlight_row: true }];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.rowBg(2)).toMatch(/^rgba\(171, 205, 239, 0.2/); // JP row (90 > 80), alpha 0.2
    expect(c.rowBg(0)).toBeNull();
  });
  it("isPivot disables row highlight", () => {
    const rules: Rule[] = [{ columns: ["revenue"], type: "single", operator: ">", value: 0, color: "#abcdef", highlight_row: true }];
    const c = buildConditional(rows, colIndex, rules, { isPivot: true });
    expect(c.rowBg(0)).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- chart/__tests__/conditional`
Expected: FAIL.

- [ ] **Step 3: Implement `conditional.ts`**

Create `frontend/src/components/chart/conditional.ts`:

```ts
import type { Rule, FormattingOperator } from "./format-types";

const CELL_ALPHA = 0.65;
const ROW_ALPHA = 0.2;
const RANGE_ALPHA = 0.75;

function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  const full = h.length === 3 ? h.split("").map((c) => c + c).join("") : h;
  return [parseInt(full.slice(0, 2), 16), parseInt(full.slice(2, 4), 16), parseInt(full.slice(4, 6), 16)];
}
function rgba(hex: string, alpha: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}
function lerp(a: number, b: number, t: number): number {
  return Math.round(a + (b - a) * t);
}
/** Interpolate a value's color across 2 or 3 hex stops within [min,max]. */
function gradient(value: number, min: number, max: number, colors: string[]): string {
  if (max <= min) return rgba(colors[0], RANGE_ALPHA);
  const t = Math.min(1, Math.max(0, (value - min) / (max - min)));
  const stops = colors.map(hexToRgb);
  // map t across (stops.length - 1) segments
  const seg = t * (stops.length - 1);
  const i = Math.min(stops.length - 2, Math.floor(seg));
  const localT = seg - i;
  const [r, g, b] = [0, 1, 2].map((k) => lerp(stops[i][k], stops[i + 1][k], localT)) as [number, number, number];
  return `rgba(${r}, ${g}, ${b}, ${RANGE_ALPHA})`;
}

function matchOperator(op: FormattingOperator, cell: unknown, ruleValue: unknown): boolean {
  if (op === "is-null") return cell == null;
  if (op === "not-null") return cell != null;
  if (op === "is-true") return cell === true || cell === "true";
  if (op === "is-false") return cell === false || cell === "false";
  if (cell == null) return false;
  if (op === "contains") return String(cell).includes(String(ruleValue));
  if (op === "does-not-contain") return !String(cell).includes(String(ruleValue));
  if (op === "starts-with") return String(cell).startsWith(String(ruleValue));
  if (op === "ends-with") return String(cell).endsWith(String(ruleValue));
  // comparisons: numeric if both parse as numbers, else string
  const cn = Number(cell), rn = Number(ruleValue);
  const numeric = Number.isFinite(cn) && Number.isFinite(rn);
  const c = numeric ? cn : String(cell);
  const r = numeric ? rn : String(ruleValue);
  switch (op) {
    case "=": return c === r;
    case "!=": return c !== r;
    case "<": return c < r;
    case ">": return c > r;
    case "<=": return c <= r;
    case ">=": return c >= r;
  }
  return false;
}

export interface Conditional {
  bg: (value: unknown, colName: string) => string | null;
  rowBg: (rowIndex: number) => string | null;
}

/** Compile conditional-formatting rules into cell + row background getters. */
export function buildConditional(
  rows: unknown[][],
  colIndexByName: Record<string, number>,
  rules: Rule[],
  opts: { isPivot?: boolean } = {},
): Conditional {
  // Per-column numeric extents for range rules.
  const extents: Record<string, [number, number]> = {};
  const extentOf = (col: string): [number, number] => {
    if (extents[col]) return extents[col];
    const idx = colIndexByName[col];
    let min = Infinity, max = -Infinity;
    for (const row of rows) {
      const v = Number(row[idx]);
      if (Number.isFinite(v)) { if (v < min) min = v; if (v > max) max = v; }
    }
    extents[col] = [min, max];
    return extents[col];
  };

  const bg = (value: unknown, colName: string): string | null => {
    for (const rule of rules) {
      if (!rule.columns.includes(colName)) continue;
      if (rule.type === "single") {
        if (rule.highlight_row && !opts.isPivot) continue; // row-level handled by rowBg
        if (matchOperator(rule.operator, value, rule.value)) {
          return rgba(rule.color, rule.highlight_row && opts.isPivot ? CELL_ALPHA : CELL_ALPHA);
        }
      } else {
        const v = Number(value);
        if (!Number.isFinite(v)) continue;
        let [min, max] = extentOf(colName);
        if (rule.min_type === "custom" && typeof rule.min_value === "number") min = rule.min_value;
        if (rule.max_type === "custom" && typeof rule.max_value === "number") max = rule.max_value;
        if (rule.min_type === "all" || rule.max_type === "all") {
          const all = rule.columns.map(extentOf);
          if (rule.min_type === "all") min = Math.min(...all.map((e) => e[0]));
          if (rule.max_type === "all") max = Math.max(...all.map((e) => e[1]));
        }
        return gradient(v, min, max, rule.colors);
      }
    }
    return null;
  };

  // Precompute row colors from highlight_row single rules (non-pivot only).
  const rowColors: (string | null)[] = rows.map((row) => {
    if (opts.isPivot) return null;
    for (const rule of rules) {
      if (rule.type !== "single" || !rule.highlight_row) continue;
      for (const col of rule.columns) {
        const idx = colIndexByName[col];
        if (idx == null) continue;
        if (matchOperator(rule.operator, row[idx], rule.value)) return rgba(rule.color, ROW_ALPHA);
      }
    }
    return null;
  });

  return { bg, rowBg: (i) => rowColors[i] ?? null };
}
```

- [ ] **Step 4: Run + commit**

Run: `cd frontend && npm test -- chart/__tests__/conditional` → PASS. `npx tsc --noEmit` clean.
```bash
git add frontend/src/components/chart/conditional.ts frontend/src/components/chart/__tests__/conditional.test.ts
git commit -m "feat(web): conditional-formatting engine (operators + color-range gradient + alpha)"
```

---

## Task 4: `ChartParams` fields

**Files:**
- Modify: `frontend/src/api/charts.ts`
- Modify: `frontend/src/api/__tests__/charts.test.ts`

- [ ] **Step 1: Extend `ChartParams`**

In `frontend/src/api/charts.ts`, add an import and two fields:

```ts
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
```
In `interface ChartParams`, after `condense?: boolean;`:
```ts
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
```

- [ ] **Step 2: Extend the test**

In `frontend/src/api/__tests__/charts.test.ts`, add to the existing `ChartParams` test object:
```ts
      columnSettings: { revenue: { number_style: "currency", currency: "USD" } },
      conditionalFormatting: [{ columns: ["revenue"], type: "single", operator: ">", value: 100, color: "#ff0000" }],
```
and assert:
```ts
    expect(p.columnSettings?.revenue.number_style).toBe("currency");
    expect(p.conditionalFormatting?.[0].type).toBe("single");
```

- [ ] **Step 3: Verify + commit**

Run: `cd frontend && npm test -- charts && npx tsc --noEmit` → PASS / clean.
```bash
git add frontend/src/api/charts.ts frontend/src/api/__tests__/charts.test.ts
git commit -m "feat(web): ChartParams columnSettings + conditionalFormatting"
```

---

## Task 5: Apply formatting + conditional in `ChartTable`

**Files:**
- Modify: `frontend/src/components/chart/ChartTable.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartTable.test.tsx`

- [ ] **Step 1: Write the failing test (append)**

Append to `frontend/src/components/chart/__tests__/ChartTable.test.tsx`:

```tsx
  it("applies a currency format and a single-color rule", () => {
    render(
      <ChartTable
        columns={[{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }]}
        rows={[["EU", 1200], ["US", 50]]}
        columnSettings={{ revenue: { number_style: "currency", currency: "USD", currency_style: "code", decimals: 0 } }}
        conditionalFormatting={[{ columns: ["revenue"], type: "single", operator: ">", value: 1000, color: "#ff0000" }]}
      />,
    );
    expect(screen.getByText("USD 1,200")).toBeInTheDocument();
    const cell = screen.getByText("USD 1,200").closest("td")!;
    expect(cell.style.backgroundColor).toMatch(/rgba\(255, 0, 0/);
  });
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- ChartTable` → FAIL (props not accepted; no formatting).

- [ ] **Step 3: Implement**

In `frontend/src/components/chart/ChartTable.tsx`:

1. Add imports:
```tsx
import { formatValue } from "./format";
import { buildConditional } from "./conditional";
import type { ColumnFormat, Rule } from "./format-types";
```
2. Change the component signature + props:
```tsx
export function ChartTable({
  columns, rows, columnSettings = {}, conditionalFormatting = [],
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
}) {
```
3. Build the conditional getter (after `rows`):
```tsx
  const colIndexByName = useMemo(
    () => Object.fromEntries(columns.map((c, i) => [c.name, i])),
    [columns],
  );
  const cond = useMemo(
    () => buildConditional(rows, colIndexByName, conditionalFormatting),
    [rows, colIndexByName, conditionalFormatting],
  );
```
4. Change the column `cell` formatter to use `formatValue`:
```tsx
        cell: (ctx) => formatValue(ctx.getValue(), c.type, columnSettings[c.name]),
```
   (Remove the old `formatCell` usage for the cell; keep `accessorFn` coercion as-is. The `formatCell`/`NUMERIC` local helpers can stay for the `isNum` alignment meta, OR be replaced — keep `isNum` meta via `NUMERIC.test(c.type)`.)
5. Apply backgrounds: in the body `<tr>`, compute a row color; in each `<td>`, a cell color:
```tsx
          {table.getRowModel().rows.map((r) => {
            const rowColor = cond.rowBg(r.index);
            return (
              <tr key={r.id} className="border-t" style={rowColor ? { backgroundColor: rowColor } : undefined}>
                {r.getVisibleCells().map((cell) => {
                  const isNum = (cell.column.columnDef.meta as { isNum?: boolean } | undefined)?.isNum ?? false;
                  const bg = cond.bg(cell.getValue(), cell.column.id);
                  return (
                    <td
                      key={cell.id}
                      className={`py-1 px-3 ${isNum ? "text-right tabular-nums" : ""}`}
                      style={bg ? { backgroundColor: bg } : undefined}
                    >
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </td>
                  );
                })}
              </tr>
            );
          })}
```
   Note `cell.column.id` is `col_${i}` (from the existing `id`), but `cond.bg` expects the column NAME. Fix: set the column `id` to `c.name`? The existing code uses `id: col_${i}` to avoid duplicate-name collisions. Keep `col_${i}` as id but pass the NAME to `cond.bg`: store the name in `meta` and read it: add `meta: { isNum, name: c.name }`, then `cond.bg(cell.getValue(), (cell.column.columnDef.meta as any).name)`. Update the `meta` type cast accordingly.

- [ ] **Step 4: Run + commit**

Run: `cd frontend && npm test -- ChartTable && npx tsc --noEmit && npx eslint src/components/chart/ChartTable.tsx` → PASS / clean.
```bash
git add frontend/src/components/chart/ChartTable.tsx frontend/src/components/chart/__tests__/ChartTable.test.tsx
git commit -m "feat(web): ChartTable applies formatting + conditional backgrounds"
```

---

## Task 6: Pivot engine raw values + `PivotTable` formatting/conditional

**Files:**
- Modify: `frontend/src/components/chart/pivot/engine.ts`
- Modify: `frontend/src/components/chart/pivot/__tests__/engine.test.ts`
- Modify: `frontend/src/components/chart/pivot/PivotTable.tsx`

- [ ] **Step 1: Change `BodyRow.values` to raw + drop `fmt`**

In `engine.ts`:
- Change the `BodyRow` interface field `values: string[];` → `values: CellValue[];`.
- Delete the `const fmt = (v: CellValue): string => {...}` helper.
- Change `rowValues` to return raw cells:
```ts
  function rowValues(rowPath: (string | null)[]): CellValue[] {
    return colLeaves.map((cl) => ix.cell(rowPath, cl.colPath, cl.metric));
  }
```

- [ ] **Step 2: Update the engine tests for raw values**

In `engine.test.ts`, the assertions on `.values` currently expect formatted strings (`["10","20","30"]`, `["15","20","35"]`). Change them to raw numbers: `[10, 20, 30]` and `[15, 20, 35]` respectively. (Find the two `expect(...).toEqual([...])` on `values` and drop the quotes.) Run `npm test -- engine` and fix any other `values` assertions to raw.

- [ ] **Step 3: Format + color in `PivotTable`**

In `PivotTable.tsx`:
1. Imports:
```tsx
import { formatValue } from "../format";
import { buildConditional } from "../conditional";
import type { ColumnFormat, Rule } from "../format-types";
```
2. Props: add `columnSettings?: Record<string, ColumnFormat>` and `conditionalFormatting?: Rule[]` and `metricTypes?: Record<string, string>` (type per metric, for formatting). The component currently takes `{ input }`:
```tsx
export function PivotTable({
  input, columnSettings = {}, conditionalFormatting = [], metricTypes = {},
}: {
  input: PivotInput;
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
  metricTypes?: Record<string, string>;
}) {
```
3. Build a conditional getter over the pivot body. The conditional engine works on `rows: unknown[][]` keyed by column name — synthesize a flat view from the body cells keyed by each colLeaf's metric, so a rule on a metric colors its value cells:
```tsx
  const p = buildPivot(input);
  // synthesize rows for conditional: one row per body row, columns = metric names (by their values across colLeaves)
  const condRows = p.bodyRows.map((br) => br.values);
  const condColIndex = Object.fromEntries(p.colLeaves.map((cl, i) => [cl.metric, i] as const));
  const cond = buildConditional(condRows, condColIndex, conditionalFormatting, { isPivot: true });
```
   NOTE: a metric appears in multiple colLeaves (one per col path). Keying `condColIndex` by metric name maps to the FIRST colLeaf index — acceptable for MVP (rules apply per-metric; extent across that metric's first column). For correctness of the cell getter we color by the metric name regardless of which colLeaf, so pass the metric name to `cond.bg` per cell (below). The extent is computed from `condRows` column = first colLeaf of that metric; documented limitation.
4. In the value-cell render, format + color:
```tsx
                {br.values.map((v, vi) => {
                  const cl = p.colLeaves[vi];
                  const text = formatValue(v, metricTypes[cl.metric] ?? "double", columnSettings[cl.metric]);
                  const bg = cond.bg(v, cl.metric);
                  return (
                    <td
                      key={vi}
                      className={`border-l px-4 py-1.5 text-right tabular-nums last:border-r-0 ${
                        br.kind === "subtotal" || br.kind === "grand" || cl.isTotal ? "font-semibold" : ""
                      }`}
                      style={bg ? { backgroundColor: bg } : undefined}
                    >
                      {text}
                    </td>
                  );
                })}
```

- [ ] **Step 4: Update the PivotTable test**

The existing `PivotTable.test.tsx` asserts `screen.getByText("35")` (grand total). With raw values now formatted via `formatValue` default (locale grouping), `35` still renders as `"35"`. Confirm it still passes; if the default format changed a value, update the assertion to the formatted form. Add a test that a `columnSettings` currency format applies to a pivot value:
```tsx
  it("applies a column format to value cells", () => {
    render(<PivotTable input={input} metricTypes={{ revenue: "double" }} columnSettings={{ revenue: { prefix: "$" } }} />);
    expect(screen.getByText("$35")).toBeInTheDocument();
  });
```
(`input`'s metric is `revenue`; the grand total 35 → `$35`.)

- [ ] **Step 5: Run + commit**

Run: `cd frontend && npm test -- pivot && npx tsc --noEmit && npx eslint src/components/chart/pivot/` → PASS / clean.
```bash
git add frontend/src/components/chart/pivot/engine.ts frontend/src/components/chart/pivot/__tests__/engine.test.ts frontend/src/components/chart/pivot/PivotTable.tsx frontend/src/components/chart/pivot/__tests__/PivotTable.test.tsx
git commit -m "feat(web): pivot value cells apply formatting + conditional (engine returns raw)"
```

---

## Task 7: Settings sidebar — Format section

**Files:**
- Create: `frontend/src/components/chart/settings/FormatSection.tsx`
- Test: `frontend/src/components/chart/settings/__tests__/FormatSection.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/settings/__tests__/FormatSection.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FormatSection } from "../FormatSection";

describe("FormatSection", () => {
  it("writes a column format when the user picks a number style", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "revenue", type: "double" }]}
        value={{}}
        onChange={onChange}
      />,
    );
    // pick the column (it's the only one, auto-selected or click it)
    fireEvent.click(screen.getByText("revenue"));
    // open the style select and choose Currency — implemented as a native <select> for testability
    const styleSelect = screen.getByLabelText("Number style") as HTMLSelectElement;
    fireEvent.change(styleSelect, { target: { value: "currency" } });
    expect(onChange).toHaveBeenCalledWith({ revenue: { number_style: "currency" } });
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- FormatSection` → FAIL.

- [ ] **Step 3: Implement `FormatSection.tsx`**

Create `frontend/src/components/chart/settings/FormatSection.tsx`. Use native `<select>`/`<input>` controls (simple + testable; shadcn `Select` can be substituted later for polish). It lists columns, lets the user select one, and edits that column's `ColumnFormat`:

```tsx
import { useState } from "react";
import { isNumericType, isTemporalType, type ColumnFormat } from "../format-types";

interface ResultColumn { name: string; type: string }

export function FormatSection({
  columns, value, onChange,
}: {
  columns: ResultColumn[];
  value: Record<string, ColumnFormat>;
  onChange: (v: Record<string, ColumnFormat>) => void;
}) {
  const [sel, setSel] = useState<string | null>(columns[0]?.name ?? null);
  const col = columns.find((c) => c.name === sel);
  const fmt = (sel && value[sel]) || {};

  function patch(p: Partial<ColumnFormat>) {
    if (!sel) return;
    onChange({ ...value, [sel]: { ...fmt, ...p } });
  }

  return (
    <div className="space-y-3 text-sm">
      <div className="space-y-1">
        <div className="text-xs font-medium uppercase text-muted-foreground">Column</div>
        <div className="flex flex-col gap-0.5">
          {columns.map((c) => (
            <button
              key={c.name}
              type="button"
              onClick={() => setSel(c.name)}
              className={`rounded px-2 py-1 text-left ${sel === c.name ? "bg-muted font-medium" : "hover:bg-muted/50"}`}
            >
              {c.name}
            </button>
          ))}
        </div>
      </div>

      {col && isNumericType(col.type) && (
        <div className="space-y-2">
          <label className="block">
            <span className="text-xs text-muted-foreground">Number style</span>
            <select aria-label="Number style" className="mt-0.5 w-full rounded border px-2 py-1"
              value={fmt.number_style ?? "decimal"} onChange={(e) => patch({ number_style: e.target.value as ColumnFormat["number_style"] })}>
              <option value="decimal">Normal</option>
              <option value="percent">Percent</option>
              <option value="currency">Currency</option>
              <option value="scientific">Scientific</option>
            </select>
          </label>
          {fmt.number_style === "currency" && (
            <label className="block">
              <span className="text-xs text-muted-foreground">Currency</span>
              <input className="mt-0.5 w-full rounded border px-2 py-1" value={fmt.currency ?? "USD"}
                onChange={(e) => patch({ currency: e.target.value.toUpperCase() })} />
            </label>
          )}
          <label className="block">
            <span className="text-xs text-muted-foreground">Decimals</span>
            <input type="number" min={0} className="mt-0.5 w-full rounded border px-2 py-1"
              value={fmt.decimals ?? ""} onChange={(e) => patch({ decimals: e.target.value === "" ? undefined : Number(e.target.value) })} />
          </label>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={!!fmt.compact} onChange={(e) => patch({ compact: e.target.checked })} /> Compact (1.2K)
          </label>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={!!fmt.negative_in_parentheses} onChange={(e) => patch({ negative_in_parentheses: e.target.checked })} /> Negatives in ( )
          </label>
          <div className="flex gap-2">
            <label className="block flex-1"><span className="text-xs text-muted-foreground">Prefix</span>
              <input className="mt-0.5 w-full rounded border px-2 py-1" value={fmt.prefix ?? ""} onChange={(e) => patch({ prefix: e.target.value })} /></label>
            <label className="block flex-1"><span className="text-xs text-muted-foreground">Suffix</span>
              <input className="mt-0.5 w-full rounded border px-2 py-1" value={fmt.suffix ?? ""} onChange={(e) => patch({ suffix: e.target.value })} /></label>
          </div>
        </div>
      )}

      {col && isTemporalType(col.type) && (
        <div className="space-y-2">
          <label className="block">
            <span className="text-xs text-muted-foreground">Date style</span>
            <select aria-label="Date style" className="mt-0.5 w-full rounded border px-2 py-1"
              value={fmt.date_style ?? "MMMM D, YYYY"} onChange={(e) => patch({ date_style: e.target.value })}>
              <option value="MMMM D, YYYY">January 31, 2024</option>
              <option value="M/D/YYYY">1/31/2024</option>
              <option value="D/M/YYYY">31/1/2024</option>
              <option value="YYYY-MM-DD">2024-01-31</option>
            </select>
          </label>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={!!fmt.date_abbreviate} onChange={(e) => patch({ date_abbreviate: e.target.checked })} /> Abbreviate
          </label>
        </div>
      )}

      {col && !isNumericType(col.type) && !isTemporalType(col.type) && (
        <p className="text-xs text-muted-foreground">No format options for this column type.</p>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Run + commit**

Run: `cd frontend && npm test -- FormatSection && npx tsc --noEmit && npx eslint src/components/chart/settings/FormatSection.tsx` → PASS / clean.
```bash
git add frontend/src/components/chart/settings/FormatSection.tsx frontend/src/components/chart/settings/__tests__/FormatSection.test.tsx
git commit -m "feat(web): settings sidebar — per-column Format section"
```

---

## Task 8: Settings sidebar — Conditional Formatting section + RuleEditor

**Files:**
- Create: `frontend/src/components/chart/settings/RuleEditor.tsx`
- Create: `frontend/src/components/chart/settings/ConditionalFormattingSection.tsx`
- Test: `frontend/src/components/chart/settings/__tests__/RuleEditor.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/settings/__tests__/RuleEditor.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { RuleEditor } from "../RuleEditor";

describe("RuleEditor", () => {
  it("builds a single-color rule and saves it", () => {
    const onSave = vi.fn();
    render(
      <RuleEditor
        columns={[{ name: "revenue", type: "double" }, { name: "region", type: "varchar" }]}
        rule={{ columns: ["revenue"], type: "single", operator: ">", value: "100", color: "#ff0000" }}
        isPivot={false}
        onSave={onSave}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Save rule"));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ type: "single", operator: ">", value: "100", columns: ["revenue"] }));
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- RuleEditor` → FAIL.

- [ ] **Step 3: Implement `RuleEditor.tsx`**

Create `frontend/src/components/chart/settings/RuleEditor.tsx`. A controlled editor over a working-copy rule, with native controls (testable):

```tsx
import { useState } from "react";
import { isNumericType, type Rule, type SingleColorRule, type ColorRangeRule, type FormattingOperator } from "../format-types";

interface ResultColumn { name: string; type: string }

const NUMERIC_OPS: FormattingOperator[] = ["=", "!=", "<", ">", "<=", ">="];
const STRING_OPS: FormattingOperator[] = ["=", "!=", "contains", "does-not-contain", "starts-with", "ends-with"];
const NULL_OPS: FormattingOperator[] = ["is-null", "not-null"];
const NO_VALUE_OPS = new Set<FormattingOperator>(["is-null", "not-null", "is-true", "is-false"]);
const PALETTE = ["#ef4444", "#f59e0b", "#22c55e", "#3b82f6", "#a855f7"];
const RANGE = ["#ffffff", "#3b82f6"];

export function RuleEditor({
  columns, rule, isPivot, onSave, onCancel,
}: {
  columns: ResultColumn[];
  rule: Rule;
  isPivot: boolean;
  onSave: (r: Rule) => void;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState<Rule>(rule);
  const selected = columns.filter((c) => draft.columns.includes(c.name));
  const allNumeric = selected.length > 0 && selected.every((c) => isNumericType(c.type));
  const ops = allNumeric ? [...NUMERIC_OPS, ...NULL_OPS] : [...STRING_OPS, ...NULL_OPS];

  function toggleColumn(name: string) {
    setDraft((d) => ({ ...d, columns: d.columns.includes(name) ? d.columns.filter((c) => c !== name) : [...d.columns, name] }));
  }
  function setType(type: "single" | "range") {
    if (type === draft.type) return;
    setDraft((d) =>
      type === "single"
        ? ({ columns: d.columns, type: "single", operator: ">", value: "", color: PALETTE[0] } as SingleColorRule)
        : ({ columns: d.columns, type: "range", colors: RANGE, min_type: null, max_type: null } as ColorRangeRule),
    );
  }

  return (
    <div className="space-y-3 text-sm">
      <div>
        <div className="text-xs font-medium uppercase text-muted-foreground">Columns</div>
        <div className="flex flex-col gap-0.5 pt-1">
          {columns.map((c) => (
            <label key={c.name} className="flex items-center gap-2">
              <input type="checkbox" checked={draft.columns.includes(c.name)} onChange={() => toggleColumn(c.name)} /> {c.name}
            </label>
          ))}
        </div>
      </div>

      {allNumeric && (
        <label className="block">
          <span className="text-xs text-muted-foreground">Formatting</span>
          <select aria-label="Rule type" className="mt-0.5 w-full rounded border px-2 py-1" value={draft.type}
            onChange={(e) => setType(e.target.value as "single" | "range")}>
            <option value="single">Single color</option>
            <option value="range">Color range</option>
          </select>
        </label>
      )}

      {draft.type === "single" ? (
        <div className="space-y-2">
          <label className="block">
            <span className="text-xs text-muted-foreground">When a value is</span>
            <select aria-label="Operator" className="mt-0.5 w-full rounded border px-2 py-1" value={draft.operator}
              onChange={(e) => setDraft((d) => ({ ...(d as SingleColorRule), operator: e.target.value as FormattingOperator }))}>
              {ops.map((op) => <option key={op} value={op}>{op}</option>)}
            </select>
          </label>
          {!NO_VALUE_OPS.has(draft.operator) && (
            <label className="block">
              <span className="text-xs text-muted-foreground">Value</span>
              <input className="mt-0.5 w-full rounded border px-2 py-1" value={String((draft as SingleColorRule).value ?? "")}
                onChange={(e) => setDraft((d) => ({ ...(d as SingleColorRule), value: e.target.value }))} />
            </label>
          )}
          <div>
            <span className="text-xs text-muted-foreground">Color</span>
            <div className="flex gap-1 pt-1">
              {PALETTE.map((c) => (
                <button key={c} type="button" aria-label={`color ${c}`} onClick={() => setDraft((d) => ({ ...(d as SingleColorRule), color: c }))}
                  className={`h-6 w-6 rounded ${(draft as SingleColorRule).color === c ? "ring-2 ring-offset-1" : ""}`} style={{ backgroundColor: c }} />
              ))}
            </div>
          </div>
          {!isPivot && (
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={!!(draft as SingleColorRule).highlight_row}
                onChange={(e) => setDraft((d) => ({ ...(d as SingleColorRule), highlight_row: e.target.checked }))} /> Highlight whole row
            </label>
          )}
        </div>
      ) : (
        <div className="space-y-2">
          <div>
            <span className="text-xs text-muted-foreground">Colors (start → end)</span>
            <div className="flex gap-2 pt-1">
              <input type="color" value={(draft as ColorRangeRule).colors[0]} onChange={(e) => setDraft((d) => { const r = d as ColorRangeRule; const colors = [...r.colors]; colors[0] = e.target.value; return { ...r, colors }; })} />
              <input type="color" value={(draft as ColorRangeRule).colors[(draft as ColorRangeRule).colors.length - 1]} onChange={(e) => setDraft((d) => { const r = d as ColorRangeRule; const colors = [...r.colors]; colors[colors.length - 1] = e.target.value; return { ...r, colors }; })} />
            </div>
          </div>
        </div>
      )}

      <div className="flex justify-end gap-2 pt-1">
        <button type="button" className="rounded border px-3 py-1" onClick={onCancel}>Cancel</button>
        <button type="button" className="rounded bg-primary px-3 py-1 text-primary-foreground"
          disabled={draft.columns.length === 0}
          onClick={() => onSave(draft)}>Save rule</button>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run the RuleEditor test → PASS.**

- [ ] **Step 5: Implement `ConditionalFormattingSection.tsx`**

Create `frontend/src/components/chart/settings/ConditionalFormattingSection.tsx` — a rule list (up/down reorder + delete) + the editor:

```tsx
import { useState } from "react";
import type { Rule, SingleColorRule } from "../format-types";
import { RuleEditor } from "./RuleEditor";

interface ResultColumn { name: string; type: string }

function describe(rule: Rule): string {
  if (rule.type === "single") return `${rule.columns.join(", ")} ${rule.operator} ${rule.value ?? ""}`;
  return `${rule.columns.join(", ")} color range`;
}

export function ConditionalFormattingSection({
  columns, rules, isPivot, onChange,
}: {
  columns: ResultColumn[];
  rules: Rule[];
  isPivot: boolean;
  onChange: (r: Rule[]) => void;
}) {
  const [editing, setEditing] = useState<number | null>(null);

  if (editing !== null) {
    const rule = rules[editing] ?? ({ columns: [], type: "single", operator: ">", value: "", color: "#ef4444" } as SingleColorRule);
    return (
      <RuleEditor
        columns={columns}
        rule={rule}
        isPivot={isPivot}
        onCancel={() => setEditing(null)}
        onSave={(r) => {
          const next = [...rules];
          if (editing >= rules.length) next.push(r); else next[editing] = r;
          onChange(next);
          setEditing(null);
        }}
      />
    );
  }

  const move = (i: number, dir: -1 | 1) => {
    const j = i + dir;
    if (j < 0 || j >= rules.length) return;
    const next = [...rules];
    [next[i], next[j]] = [next[j], next[i]];
    onChange(next);
  };

  return (
    <div className="space-y-2 text-sm">
      <button type="button" className="rounded border px-3 py-1" onClick={() => setEditing(rules.length)}>+ Add rule</button>
      {rules.length === 0 && <p className="text-xs text-muted-foreground">No rules yet.</p>}
      {rules.map((rule, i) => (
        <div key={i} className="flex items-center justify-between rounded border px-2 py-1">
          <button type="button" className="flex items-center gap-2 text-left" onClick={() => setEditing(i)}>
            <span className="inline-block h-4 w-4 rounded" style={{ backgroundColor: rule.type === "single" ? rule.color : rule.colors[rule.colors.length - 1] }} />
            <span className="text-xs">{describe(rule)}</span>
          </button>
          <div className="flex gap-1">
            <button type="button" aria-label="up" onClick={() => move(i, -1)}>↑</button>
            <button type="button" aria-label="down" onClick={() => move(i, 1)}>↓</button>
            <button type="button" aria-label="delete" className="text-destructive" onClick={() => onChange(rules.filter((_, k) => k !== i))}>✕</button>
          </div>
        </div>
      ))}
    </div>
  );
}
```

- [ ] **Step 6: Run + commit**

Run: `cd frontend && npm test -- RuleEditor && npx tsc --noEmit && npx eslint src/components/chart/settings/` → PASS / clean.
```bash
git add frontend/src/components/chart/settings/RuleEditor.tsx frontend/src/components/chart/settings/ConditionalFormattingSection.tsx frontend/src/components/chart/settings/__tests__/RuleEditor.test.tsx
git commit -m "feat(web): settings sidebar — conditional formatting rules + editor"
```

---

## Task 9: Sidebar shell + ChartExplore integration

**Files:**
- Create: `frontend/src/components/chart/settings/SettingsSidebar.tsx`
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Create the sidebar shell**

Create `frontend/src/components/chart/settings/SettingsSidebar.tsx` — a section switcher hosting the two sections:

```tsx
import { useState } from "react";
import type { ColumnFormat, Rule } from "../format-types";
import { FormatSection } from "./FormatSection";
import { ConditionalFormattingSection } from "./ConditionalFormattingSection";

interface ResultColumn { name: string; type: string }

export function SettingsSidebar({
  columns, isPivot, columnSettings, conditionalFormatting, onColumnSettings, onConditional, onClose,
}: {
  columns: ResultColumn[];
  isPivot: boolean;
  columnSettings: Record<string, ColumnFormat>;
  conditionalFormatting: Rule[];
  onColumnSettings: (v: Record<string, ColumnFormat>) => void;
  onConditional: (r: Rule[]) => void;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<"format" | "conditional">("format");
  return (
    <div className="w-72 shrink-0 border-l pl-3">
      <div className="mb-2 flex items-center justify-between">
        <div className="flex gap-1">
          <button type="button" className={`rounded px-2 py-1 text-xs ${tab === "format" ? "bg-muted font-medium" : ""}`} onClick={() => setTab("format")}>Formatting</button>
          <button type="button" className={`rounded px-2 py-1 text-xs ${tab === "conditional" ? "bg-muted font-medium" : ""}`} onClick={() => setTab("conditional")}>Conditional</button>
        </div>
        <button type="button" aria-label="close settings" className="text-muted-foreground" onClick={onClose}>✕</button>
      </div>
      {tab === "format"
        ? <FormatSection columns={columns} value={columnSettings} onChange={onColumnSettings} />
        : <ConditionalFormattingSection columns={columns} rules={conditionalFormatting} isPivot={isPivot} onChange={onConditional} />}
    </div>
  );
}
```

- [ ] **Step 2: Wire into `ChartExplore.tsx`**

1. Imports:
```tsx
import { SettingsSidebar } from "@/components/chart/settings/SettingsSidebar";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
```
2. State:
```tsx
  const [columnSettings, setColumnSettings] = useState<Record<string, ColumnFormat>>({});
  const [conditionalFormatting, setConditionalFormatting] = useState<Rule[]>([]);
  const [settingsOpen, setSettingsOpen] = useState(false);
```
3. The settings columns come from the last result: `const resultColumns = results.data?.columns ?? [];` (each `{name,type}`). For pivot metric types: `const metricTypes = Object.fromEntries((detail.data?.metrics ?? []).map((m) => [m.metric_name, m.data_type ?? "double"]));`
4. Add a **Settings** toggle button next to Run/Save:
```tsx
        <Button size="sm" variant="outline" onClick={() => setSettingsOpen((o) => !o)}>Settings</Button>
```
5. Pass props into the renderers. Add the new props to the EXISTING `<ChartTable>` and
   `<PivotTable>` JSX — keep the existing `input={{ columns: ..., rows: ..., rowDims: pRows,
   colDims: pCols, metrics: pVals, showRowTotals, showColTotals, condense, collapsed: new Set() }}`
   object on `<PivotTable>` unchanged; only append the three new props:
```tsx
        // ChartTable (existing): add columnSettings + conditionalFormatting props
        <ChartTable columns={results.data.columns} rows={results.data.rows}
          columnSettings={columnSettings} conditionalFormatting={conditionalFormatting} />

        // PivotTable (existing): keep its current input={{...}} object; append these three props
        //   columnSettings={columnSettings}
        //   conditionalFormatting={conditionalFormatting}
        //   metricTypes={metricTypes}
```
6. Render the sidebar (right of the preview) when `settingsOpen && resultColumns.length`:
```tsx
        {settingsOpen && resultColumns.length > 0 && (
          <SettingsSidebar
            columns={resultColumns}
            isPivot={viz === "pivot_table"}
            columnSettings={columnSettings}
            conditionalFormatting={conditionalFormatting}
            onColumnSettings={setColumnSettings}
            onConditional={setConditionalFormatting}
            onClose={() => setSettingsOpen(false)}
          />
        )}
```
   (Place the preview + sidebar in a flex row so the sidebar sits to the right.)
7. Persist in `paramsForSave()`: add `columnSettings, conditionalFormatting` to BOTH the table and pivot param objects (top level of `ChartParams`).
8. Hydrate in the `existing.data` effect: `setColumnSettings(p.columnSettings ?? {}); setConditionalFormatting(p.conditionalFormatting ?? []);`
9. Reset on dataset change (where pivot/table state is reset): `setColumnSettings({}); setConditionalFormatting([]);`

- [ ] **Step 3: Typecheck, lint, build, test**

Run: `cd frontend && npm run typecheck && npm run lint && npm run build && npm test`
Expected: clean / all pass.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/chart/settings/SettingsSidebar.tsx frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(web): settings sidebar host + ChartExplore integration (persist + hydrate)"
```

---

## Task 10: Sweep + manual smoke + final review

**Files:** none (verification).

- [ ] **Step 1: Frontend sweep**

Run: `cd frontend && npm test && npm run typecheck && npm run lint && npm run build` → all green.

- [ ] **Step 2: Manual Playwright smoke**

The dev frontend (vite :5173) hot-reloads; backend unchanged (no migration). With the stack up (login `admin@egret.local` / `Admin12345!`, ws `eng`):
1. Open the seeded pivot chart → **Settings** → **Formatting** → pick `revenue` → Currency (USD) → confirm cells reformat live.
2. **Conditional** → Add rule → columns `revenue`, `>`, value (a midpoint), a color → confirm cells colour; add a color-range rule → confirm heatmap.
3. Switch to a Table chart → repeat (incl. Highlight-whole-row).
4. Save → reopen → confirm formatting + rules hydrate.

- [ ] **Step 3: Final holistic review**

Dispatch a final reviewer over the phase `git diff`. Focus: the `formatValue` correctness (Intl + dayjs, separators, null/string-numeric), the conditional engine (operator correctness, gradient math, first-match-wins, isPivot row-highlight gating), the pivot raw-value change (no regression in totals), and the persistence round-trip.

- [ ] **Step 4: Final commit (if fixes)**

```bash
git add -A && git commit -m "test: phase 3c-3.3 sweep + manual smoke fixes"
```

Then use **superpowers:finishing-a-development-branch**.

---

## Self-Review notes (plan author)
- **Spec coverage:** types (T1), formatValue (T2), conditional engine (T3), params (T4), Table apply (T5), pivot raw + apply (T6), Format section (T7), Conditional section + editor (T8), sidebar + explore wiring (T9), sweep+smoke+review (T10). The §5 settings UI uses native controls (testable) rather than shadcn `Select` — a deliberate simplification; shadcn polish is a fast follow.
- **Type consistency:** `ColumnFormat`, `Rule`/`SingleColorRule`/`ColorRangeRule`, `FormattingOperator`, `buildConditional → { bg, rowBg }`, `formatValue(value, type, fmt)` used identically across tasks.
- **Known limitation (documented in T6):** pivot conditional keys extent by a metric's first colLeaf column; acceptable for MVP. ICU output strings in T2/T5 tests may need adjusting to the runtime's exact format (adjust expectations, not impl).
