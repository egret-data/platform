# W5 — Card-Level Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring a single dashboard card to Metabase's parity: four download formats behind
Metabase's popover (segmented format control + "Keep the data formatted" + row-limit note), the
card title as a link to its chart plus an "Edit question" affordance, a per-card description
surfaced on the header, and a per-card "Show background" toggle that lets any card drop its chrome
the way egret's heading card already does.

**Architecture:** Everything here is client-side. egret's export has always been client-side
(`export-chart.ts`) because that is where the card's *effective* column formatting lives — the
chart programme settled that and this workstream does not reopen it. The four items are
deliberately kept out of `DashboardView.tsx`: the format writers are pure modules under
`components/chart/`, the header is its own component, and the two per-card predicates
(`dashcardHref`, `showsCardBackground`) are pure functions the route calls. The route's only edit
is to replace a five-line inline class ternary with one call — it ends this workstream **shorter**
than it started.

**Tech Stack:** React 18 + TypeScript, Radix (Popover, RadioGroup, Checkbox), Tailwind v4 OKLCH
tokens, react-router-dom v6, vitest + Testing Library. One new runtime dependency:
`write-excel-file` (+ its single dependency `fflate`), behind a dynamic import.

**Design reference:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` §4 W5. Read §9 first:
the per-card edit toolbar and the "Replace chart" `options` reset are **not** defects and no work
is planned for them here.

**Branch:** `feat/dashboard-parity-05-card-level`, from `main`.

---

## Background the implementer needs

### 1. "Keep the data formatted" is not a no-op — the spec's phrasing understates it

§4 W5 says the toggle defaults on "matching today's unconditional behaviour". That is only half
true, and the half that is false is the one that changes a shipped file.

Today `rowsToCsv` (`export-chart.ts:18-34`) renames the **header** through
`columnSettings[name].column_title` and writes every **cell** with a bare `String(v)`. It applies
no `formatValue`. So today's CSV is a hybrid: display names on top of database values. Metabase has
no such hybrid — `enableFormatting` on gives display names *and* formatted cells, off gives machine
names *and* raw cells (`ExportSettingsWidget.tsx:99`, and the two description strings it swaps
between: "E.g. September 6, 2024 or $187.50" vs "E.g. 2024-09-06 or 187.50").

This plan adopts Metabase's definition, so **the default CSV changes**: a `bigint` column of
`1234` exports as `1,234` instead of `1234`, because `formatNumber` builds with
`useGrouping: true` (`format.ts:105`). That is deliberate and it is the reason the toggle exists —
a reader who wants the database's bytes turns it off. The change is pinned by an explicit test in
Task 1 so nobody "fixes" it back by accident.

Nine existing assertions in `__tests__/export-chart.test.ts` were checked by hand against
`formatValue` before this plan was written and all nine still hold (`10` → `"10"`, `false` →
`"false"`, `null` → `""`, `0` → `"0"`); that file is deliberately **not** edited, so it stands as
the regression net for this change.

### 2. A vitest pass is not evidence the bundle contains the writer

`frontend/scripts/assert-label-layout-bundled.mjs` exists because this repo shipped a build in
which six files' `labelLayout` options were dead: the code worked in vitest, worked in
`vite dev`, and was tree-shaken out of `dist/`. **Neither vitest nor `vite dev` tree-shakes or
code-splits.** The same instrument therefore cannot tell us whether `write-excel-file` stayed
*out* of the main chunk either — a lazy import and an eager one are indistinguishable to the test
runner. Task 3 asserts it against the built artefact and is not optional.

### 3. `<Link>` breaks three existing test files

Adding a react-router `<Link>` to `DashboardCardView` makes it throw outside a Router. Three files
render it bare today — `DashboardCardView.test.tsx` (11 renders),
`DashboardCardView.export.test.tsx` (5), `DashboardCardView.vizoverride.test.tsx` (1). Task 5
wraps them. One of the eleven uses `rerender`, which must be wrapped by hand.

### 4. egret's `Tooltip` is not portalled

`components/ui/tooltip.tsx` renders `TooltipPrimitive.Content` with **no** `Portal`, so its content
lands inside whatever it is nested in — and the dashcard body is `overflow-hidden`
(`DashboardView.tsx:630`). A hover tooltip carrying a description would be clipped on any card
shorter than the tooltip. Metabase hovers; egret clicks, using the already-portalled `Popover`.
This is a deliberate, recorded deviation, not an oversight: adding `Portal` to the shared
`ui/tooltip.tsx` moves every tooltip in the product and is not this workstream's call to make.

### 5. The PNG export has grabbed a UI icon before

`ChartCardTileBody` refs the render-only container, not the tile, precisely so `querySelector("svg")`
cannot find the Download button's own icon (`DashboardCardView.tsx:51-54`, and the regression test
at `DashboardCardView.export.test.tsx:51-61`). The hover cluster grows an "Edit question" pencil in
this workstream — **another `<svg>` in the tile**. It goes in the same absolutely-positioned
overlay, outside `chartRef`, and the existing regression test keeps its teeth. The live check
measures the exported PNG's pixel dimensions for the same reason.

---

## File structure

| Path | Responsibility after this workstream |
|---|---|
| `frontend/src/components/chart/export-chart.ts` | Gains `tableForExport` (the one place pivot rollup markers die and a header is resolved), `exportCell`, `rowsToJson`, `downloadJson`, and a `formatted` flag on CSV. |
| `frontend/src/components/chart/export-xlsx.ts` | **New.** `rowsToSheetData` (pure) plus `downloadXlsx`, whose `write-excel-file` import is dynamic so the writer lands in its own chunk. |
| `frontend/scripts/assert-xlsx-lazy.mjs` | **New.** Reads `dist/assets/` and fails if the xlsx writer is in the entry chunk, or in no chunk at all. |
| `frontend/src/components/chart/ChartExportMenu.tsx` | Becomes Metabase's download **popover**: title, segmented format control, "Keep the data formatted", the row-limit note, and a Download button. |
| `frontend/src/components/chart/dashboard/dashcard-href.ts` | **New.** Pure: the chart route for a card, or `undefined` when suppressed. |
| `frontend/src/components/chart/dashboard/DashboardCardHeader.tsx` | **New.** The card caption: title (as a link when there is one) + the description affordance. |
| `frontend/src/components/chart/dashboard/card-chrome.ts` | **New.** Pure: `showsCardBackground` / `cardChromeClass`. Owns the heading card's chrome-less default. |
| `frontend/src/components/chart/dashboard/DashboardCardView.tsx` | Renders the new header, and the hover cluster grows an "Edit question" link beside the export trigger. |
| `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx` | Gains a Description field (chart cards) and a "Show background" toggle (every card). |
| `frontend/src/routes/explore/DashboardView.tsx` | **Wiring only, net −4 lines:** the inline chrome ternary becomes one `cardChromeClass(c)` call. |
| `frontend/package.json` | `write-excel-file` dependency + a `test:bundle:xlsx` script. |
| `.github/workflows/ci.yml` | One extra step running that script after `npm run build`. |

---

## Task 1: One export shaping path, and `.json`

**Files:**
- Modify: `frontend/src/components/chart/export-chart.ts`
- Test: `frontend/src/components/chart/__tests__/export-chart.formats.test.ts` (new)
- **Do not touch** `frontend/src/components/chart/__tests__/export-chart.test.ts` — it is the
  regression net for the formatting change described in Background §1.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/__tests__/export-chart.formats.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { rowsToCsv, rowsToJson, tableForExport } from "../export-chart";

const cols = [{ name: "region", type: "VARCHAR" }, { name: "rev", type: "BIGINT" }];
const rows = [["EU", 1234], ["NA", 20]];

describe("tableForExport", () => {
  it("resolves display headers when formatted and machine names when not", () => {
    const settings = { rev: { column_title: "Revenue" } };
    expect(tableForExport(cols, rows, settings, true).header).toEqual(["region", "Revenue"]);
    expect(tableForExport(cols, rows, settings, false).header).toEqual(["region", "rev"]);
  });

  it("strips pivot rollup marker columns and rollup rows for every format, not just CSV", () => {
    // This used to live inside rowsToCsv. A second writer that forgot it would export the grand
    // total as its first row — which is the defect the strip was added for in the first place.
    const pivotCols = [...cols, { name: "__grp_0", type: "INTEGER" }];
    const pivotRows = [[null, 30, 1], ["EU", 10, 0], ["NA", 20, 0]];
    const t = tableForExport(pivotCols, pivotRows, undefined, true);
    expect(t.header).toEqual(["region", "rev"]);
    expect(t.columns.map((c) => c.name)).toEqual(["region", "rev"]);
    expect(t.rows).toEqual([["EU", 10], ["NA", 20]]);
  });

  it("keeps the raw cells — formatting is the writer's job, not the shaper's", () => {
    expect(tableForExport(cols, rows, undefined, true).rows).toEqual([["EU", 1234], ["NA", 20]]);
  });
});

describe("rowsToCsv formatted flag", () => {
  it("formats cells when on — grouping separators and all", () => {
    // formatNumber builds with useGrouping: true (format.ts:105), so a formatted BIGINT gains a
    // comma and therefore gains CSV quoting. This is Metabase's behaviour and the whole reason
    // the toggle exists; it is pinned here so nobody quietly reverts it.
    expect(rowsToCsv(cols, rows)).toBe('region,rev\r\nEU,"1,234"\r\nNA,20');
  });

  it("emits the database's bytes when off, machine header included", () => {
    expect(rowsToCsv(cols, rows, { rev: { column_title: "Revenue" } }, false))
      .toBe("region,rev\r\nEU,1234\r\nNA,20");
  });
});

describe("rowsToJson", () => {
  it("emits one object per row keyed by the resolved header", () => {
    expect(JSON.parse(rowsToJson(cols, rows, { rev: { column_title: "Revenue" } }, false)))
      .toEqual([{ region: "EU", rev: 1234 }, { region: "NA", rev: 20 }]);
  });

  it("keys on display names and carries formatted strings when formatted", () => {
    expect(JSON.parse(rowsToJson(cols, rows, { rev: { column_title: "Revenue" } }, true)))
      .toEqual([{ region: "EU", Revenue: "1,234" }, { region: "NA", Revenue: "20" }]);
  });

  it("is pretty-printed, so a downloaded file is readable", () => {
    expect(rowsToJson(cols, [["EU", 1]], undefined, false)).toContain("\n  {\n");
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd frontend && npx vitest run src/components/chart/__tests__/export-chart.formats.test.ts
```

Expected: FAIL — `No "tableForExport" export is defined on the "../export-chart" mock` / the
import is undefined, so every case errors.

- [ ] **Step 3: Write the implementation**

In `frontend/src/components/chart/export-chart.ts`, add the import beside the two existing ones:

```ts
import { formatValue } from "@/components/chart/format";
```

Insert `tableForExport` / `exportCell` immediately after `csvCell`, and rewrite `rowsToCsv` to
use them. The doc comment on the old `rowsToCsv` moves onto `tableForExport`, because that is
where the rules it describes now live:

```ts
/**
 * A RESULT, SHAPED FOR A FILE — the one place pivot rollup markers die and the one place a header
 * is resolved. Three writers share it (`csv`, `json`, `xlsx`).
 *
 * A rollup result carries one `__grp_N` marker per dimension and, interleaved with the data, a row
 * for every subtotal and the grand total. Those are how the pivot builds its bands — they are not
 * columns or rows anyone asked for. Exported raw, the file opened with `__grp_0,__grp_1,__grp_2` in
 * the header and a first row of `,,,116269,1,1,1`: a total, indistinguishable from data unless the
 * reader knows to decode the markers. When this lived inside `rowsToCsv` the strip was a property
 * of the CSV writer; the moment a second writer existed it had to become a property of the export.
 *
 * `formatted` is Metabase's "Keep the data formatted" (`ExportSettingsWidget.tsx:99`). On, the file
 * reads like the screen — the rename the table shows, and `formatValue`'d cells. Off, it reads like
 * the database — machine column names and raw values. egret's CSV used to be neither: renamed
 * headers over raw cells. That hybrid is gone.
 *
 * `||`, not `??`, on the rename: settings are never key-whitelisted server-side, so a
 * legacy/hand-edited `column_title: ""` is reachable, and `??` would emit a blank header cell
 * instead of falling back to the real name.
 */
export interface ExportTable {
  /** The header as it should be written, one entry per kept column. */
  header: string[];
  /** The kept columns themselves — a writer needs `type` to format or to type a cell. */
  columns: ResultColumn[];
  /** Data rows, markers and rollup rows removed, cells still raw. */
  rows: unknown[][];
}

export function tableForExport(
  columns: ResultColumn[],
  rows: unknown[][],
  columnSettings?: Record<string, ColumnFormat>,
  formatted = true,
): ExportTable {
  const markerIdx = columns.map((c, i) => (/^__grp_\d+$/.test(c.name) ? i : -1)).filter((i) => i >= 0);
  const keep = columns.map((_, i) => i).filter((i) => !markerIdx.includes(i));
  const isRollupRow = (r: unknown[]) => markerIdx.some((i) => Number(r[i]) === 1);
  const kept = keep.map((i) => columns[i]);
  return {
    header: formatted ? kept.map((c) => columnSettings?.[c.name]?.column_title || c.name) : kept.map((c) => c.name),
    columns: kept,
    rows: rows.filter((r) => !isRollupRow(r)).map((r) => keep.map((i) => r[i])),
  };
}

/** One cell as the file should carry it: the string the screen shows when formatted, the raw value
 *  otherwise. Only `.xlsx` keeps the raw type — `.csv` stringifies and `.json` re-serialises. */
export function exportCell(
  value: unknown,
  column: ResultColumn,
  columnSettings: Record<string, ColumnFormat> | undefined,
  formatted: boolean,
): unknown {
  return formatted ? formatValue(value, column.type, columnSettings?.[column.name] ?? {}) : value;
}
```

Replace the whole of the existing `rowsToCsv` (its old doc comment included — it now lives above)
with:

```ts
/** RFC-4180 CSV: a header row, then one row per data row, CRLF-separated. */
export function rowsToCsv(
  columns: ResultColumn[],
  rows: unknown[][],
  columnSettings?: Record<string, ColumnFormat>,
  formatted = true,
): string {
  const t = tableForExport(columns, rows, columnSettings, formatted);
  const header = t.header.map(csvCell).join(",");
  const body = t.rows
    .map((r) => r.map((v, i) => csvCell(exportCell(v, t.columns[i], columnSettings, formatted))).join(","))
    .join("\r\n");
  return body ? `${header}\r\n${body}` : header;
}
```

Change `downloadCsv` to carry the flag through:

```ts
export function downloadCsv(title: string | undefined, columns: ResultColumn[], rows: unknown[][], columnSettings?: Record<string, ColumnFormat>, formatted = true): void {
  downloadBlob(exportFilename(title, "csv"), new Blob([rowsToCsv(columns, rows, columnSettings, formatted)], { type: "text/csv;charset=utf-8" }));
}
```

And add the JSON writer at the end of the file, after `downloadPng`:

```ts
/**
 * One JSON object per row, keyed by the resolved header — the shape Metabase's `.json` export has.
 *
 * TWO COLUMNS THAT RESOLVE TO THE SAME HEADER COLLIDE, and the last one wins. Reachable only by
 * renaming two columns to the same title, which is also true of Metabase's export, and the
 * alternative (suffixing duplicates) invents a key the reader never typed.
 */
export function rowsToJson(columns: ResultColumn[], rows: unknown[][], columnSettings?: Record<string, ColumnFormat>, formatted = true): string {
  const t = tableForExport(columns, rows, columnSettings, formatted);
  const objects = t.rows.map((r) =>
    Object.fromEntries(r.map((v, i) => [t.header[i], exportCell(v, t.columns[i], columnSettings, formatted)])),
  );
  return JSON.stringify(objects, null, 2);
}

export function downloadJson(title: string | undefined, columns: ResultColumn[], rows: unknown[][], columnSettings?: Record<string, ColumnFormat>, formatted = true): void {
  downloadBlob(exportFilename(title, "json"), new Blob([rowsToJson(columns, rows, columnSettings, formatted)], { type: "application/json;charset=utf-8" }));
}
```

- [ ] **Step 4: Run both export test files to verify green**

```bash
cd frontend && npx vitest run src/components/chart/__tests__/export-chart.formats.test.ts
```

Expected: PASS, 8 tests.

```bash
cd frontend && npx vitest run src/components/chart/__tests__/export-chart.test.ts
```

Expected: PASS, 11 tests — **unchanged**. If any of these eleven went red, the formatting change
reached a case Background §1 said it would not; stop and re-read it rather than editing the file.

- [ ] **Step 5: Prove the tests survive mutation**

Three mutations, each run as its own command, scored by **exit code** (`echo $?` → non-zero), never
by matching stdout text:

1. In `tableForExport`, make `formatted` ignored: `header: kept.map((c) => c.name)`. Re-run
   `export-chart.formats.test.ts`. Expected: FAIL.
2. In `exportCell`, `return value;` unconditionally. Re-run. Expected: FAIL (the `"1,234"` cases).
3. In `tableForExport`, drop the rollup filter: `rows: rows.map(...)`. Re-run **both** files.
   Expected: FAIL in both.

Restore all three. This step is not optional — six assertions in the chart programme passed against
a stubbed implementation, and doing exactly this is the only thing that caught them.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/export-chart.ts \
        frontend/src/components/chart/__tests__/export-chart.formats.test.ts
git commit -m "feat(chart): shared export shaping, a .json writer, and Metabase's formatted/raw split"
```

---

## Task 2: The lazy `.xlsx` writer

**Files:**
- Modify: `frontend/package.json`
- Create: `frontend/src/components/chart/export-xlsx.ts`
- Test: `frontend/src/components/chart/__tests__/export-xlsx.test.ts`

- [ ] **Step 1: Add the dependency**

```bash
cd frontend && npm install write-excel-file@^4.1.1
```

Expected: `added 2 packages` — `write-excel-file` and its single dependency `fflate`. Nothing else.
If npm reports more than two, stop: the point of this choice over `xlsx`/`exceljs` is that it is
one small write-only library with one small dependency.

Verify what landed:

```bash
cd frontend && npm ls write-excel-file fflate
```

Expected:

```text
egret-frontend@ /…/egret/frontend
├── fflate@0.8.x
└── write-excel-file@4.1.1
```

- [ ] **Step 2: Write the failing test**

Create `frontend/src/components/chart/__tests__/export-xlsx.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";

// The real writer zips through fflate, which reaches for a Worker. jsdom has none, and this file
// is not testing fflate — it is testing what WE hand the writer and what we do with its Blob.
const toBlob = vi.fn(() => Promise.resolve(new Blob(["xlsx"], { type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" })));
const writeXlsxFile = vi.fn(() => ({ toBlob }));
vi.mock("write-excel-file/browser", () => ({ default: (...a: unknown[]) => writeXlsxFile(...a) }));

const downloadBlob = vi.fn();
vi.mock("../export-chart", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../export-chart")>()),
  downloadBlob: (...a: unknown[]) => downloadBlob(...a),
}));

import { downloadXlsx, rowsToSheetData } from "../export-xlsx";

const cols = [{ name: "region", type: "VARCHAR" }, { name: "rev", type: "BIGINT" }];
const rows = [["EU", 1234], [null, 20]];

beforeEach(() => { writeXlsxFile.mockClear(); toBlob.mockClear(); downloadBlob.mockClear(); });

describe("rowsToSheetData", () => {
  it("puts the header first, then one row per data row", () => {
    expect(rowsToSheetData(cols, rows, undefined, false)).toEqual([
      ["region", "rev"],
      ["EU", 1234],
      [null, 20],
    ]);
  });

  it("keeps numbers typed when unformatted, so Excel can sum the column", () => {
    expect(typeof rowsToSheetData(cols, rows, undefined, false)[1][1]).toBe("number");
  });

  it("writes strings when formatted, which is what the toggle trades away", () => {
    expect(rowsToSheetData(cols, rows, undefined, true)[1]).toEqual(["EU", "1,234"]);
  });

  it("renames the header from column_title, like every other format", () => {
    expect(rowsToSheetData(cols, rows, { rev: { column_title: "Revenue" } }, true)[0])
      .toEqual(["region", "Revenue"]);
  });

  it("writes an empty cell as null and a non-finite number as null, never as a broken cell", () => {
    const odd = rowsToSheetData([{ name: "n", type: "BIGINT" }], [[null], [Number.NaN]], undefined, false);
    expect(odd[1]).toEqual([null]);
    expect(odd[2]).toEqual([null]);
  });

  it("drops pivot rollup markers and rows, via the shared shaper", () => {
    const pivotCols = [...cols, { name: "__grp_0", type: "INTEGER" }];
    const pivotRows = [[null, 30, 1], ["EU", 10, 0]];
    expect(rowsToSheetData(pivotCols, pivotRows, undefined, false)).toEqual([
      ["region", "rev"],
      ["EU", 10],
    ]);
  });
});

describe("downloadXlsx", () => {
  it("hands the sheet data to the writer and downloads its Blob under an .xlsx name", async () => {
    await downloadXlsx("My Chart", cols, rows, undefined, false);
    expect(writeXlsxFile).toHaveBeenCalledWith([["region", "rev"], ["EU", 1234], [null, 20]]);
    expect(toBlob).toHaveBeenCalledTimes(1);
    expect(downloadBlob).toHaveBeenCalledWith("My Chart.xlsx", expect.any(Blob));
  });
});
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cd frontend && npx vitest run src/components/chart/__tests__/export-xlsx.test.ts
```

Expected: FAIL — `Failed to resolve import "../export-xlsx"`.

- [ ] **Step 4: Write the implementation**

Create `frontend/src/components/chart/export-xlsx.ts`:

```ts
import type { ResultColumn } from "@/components/chart/ChartTable";
import type { ColumnFormat } from "@/components/chart/format-types";
import { downloadBlob, exportCell, exportFilename, tableForExport } from "./export-chart";

/**
 * One `.xlsx` cell as `write-excel-file` wants it: a bare value, or `null` for an empty cell.
 *
 * Spelled out here rather than imported from the library. A `import type` is erased and would be
 * safe, but this file's whole contract is that the library's name appears in exactly ONE place —
 * the dynamic import in `downloadXlsx` — and a second mention is a second thing for a future reader
 * to "tidy up" into a static import.
 */
export type XlsxCell = string | number | boolean | Date | null;
export type XlsxSheetData = XlsxCell[][];

/** `write-excel-file` derives a cell's type from its value. Anything it cannot type — an object, a
 *  bigint, a NaN — is written as an empty cell rather than as `[object Object]` or a `#NUM!`. */
function toXlsxCell(v: unknown): XlsxCell {
  if (v === null || v === undefined) return null;
  if (typeof v === "number") return Number.isFinite(v) ? v : null;
  if (typeof v === "string" || typeof v === "boolean") return v;
  if (v instanceof Date) return v;
  return String(v);
}

/**
 * Project a result onto sheet rows: a header row, then one row per data row.
 *
 * WITH `formatted` OFF numbers stay numbers, so Excel sums the column. WITH IT ON every cell is the
 * string the screen shows — which is the point of the toggle and also the reason the column stops
 * being summable. Metabase makes exactly this trade (`enableFormatting` picks a string writer over
 * a typed one); the toggle is how the reader chooses which file they wanted.
 */
export function rowsToSheetData(
  columns: ResultColumn[],
  rows: unknown[][],
  columnSettings?: Record<string, ColumnFormat>,
  formatted = true,
): XlsxSheetData {
  const t = tableForExport(columns, rows, columnSettings, formatted);
  return [
    t.header.map((h): XlsxCell => h),
    ...t.rows.map((r) => r.map((v, i) => toXlsxCell(exportCell(v, t.columns[i], columnSettings, formatted)))),
  ];
}

/**
 * Download the result as `.xlsx`.
 *
 * THE DYNAMIC IMPORT IS THE POINT, and it is load-bearing rather than tidy. `write-excel-file` plus
 * `fflate` is the only dependency this workstream adds, and no dashboard needs a byte of it until
 * someone picks `.xlsx` in the download popover — so it must land in its own chunk.
 *
 * `frontend/scripts/assert-xlsx-lazy.mjs` asserts that against `dist/`, because no unit test can:
 * NEITHER VITEST NOR `vite dev` TREE-SHAKES OR CODE-SPLITS, so a green test says nothing about
 * which chunk this ended up in. This repo has already shipped a library that passed every test and
 * was absent from the bundle — see `scripts/assert-label-layout-bundled.mjs`.
 */
export async function downloadXlsx(
  title: string | undefined,
  columns: ResultColumn[],
  rows: unknown[][],
  columnSettings?: Record<string, ColumnFormat>,
  formatted = true,
): Promise<void> {
  const { default: writeXlsxFile } = await import("write-excel-file/browser");
  const blob = await writeXlsxFile(rowsToSheetData(columns, rows, columnSettings, formatted)).toBlob();
  downloadBlob(exportFilename(title, "xlsx"), blob);
}
```

- [ ] **Step 5: Run the test to verify it passes**

```bash
cd frontend && npx vitest run src/components/chart/__tests__/export-xlsx.test.ts
```

Expected: PASS, 7 tests.

- [ ] **Step 6: Prove the tests survive mutation**

1. In `rowsToSheetData`, drop the header row (return only the mapped data rows). Re-run.
   Expected: FAIL.
2. In `toXlsxCell`, `return v as XlsxCell;` for every input. Re-run. Expected: FAIL on the
   NaN/null case.
3. In `downloadXlsx`, change `exportFilename(title, "xlsx")` to `exportFilename(title, "csv")`.
   Re-run. Expected: FAIL.

Restore all three.

- [ ] **Step 7: Commit**

```bash
git add frontend/package.json frontend/package-lock.json \
        frontend/src/components/chart/export-xlsx.ts \
        frontend/src/components/chart/__tests__/export-xlsx.test.ts
git commit -m "feat(chart): lazy .xlsx writer behind a dynamic import"
```

---

## Task 3: Prove the writer stayed out of the main bundle

**Files:**
- Create: `frontend/scripts/assert-xlsx-lazy.mjs`
- Modify: `frontend/package.json` (scripts), `.github/workflows/ci.yml`

This task produces numbers for the PR body. Do not skip it because Task 2 was green — see
Background §2.

- [ ] **Step 1: Write the check**

Create `frontend/scripts/assert-xlsx-lazy.mjs`:

```js
#!/usr/bin/env node
/**
 * IS THE `.xlsx` WRITER IN ITS OWN CHUNK, AND IN A CHUNK AT ALL?
 *
 * `export-xlsx.ts` reaches `write-excel-file` through `await import(...)`, so Rollup should emit it
 * as a separate chunk that no dashboard fetches until someone picks `.xlsx`. One static import
 * anywhere — an editor auto-import, a "tidy-up" of the dynamic one, a type import written without
 * `import type` — collapses that back into the entry bundle, and NOTHING ELSE IN THIS REPO WOULD
 * NOTICE:
 *
 *   NEITHER VITEST NOR `vite dev` TREE-SHAKES OR CODE-SPLITS. Every unit test passes with the
 *   library eager, lazy, or duplicated. The property this asserts exists only in a production
 *   build, so only a production build can be asked about it. This is the same reason
 *   `assert-label-layout-bundled.mjs` exists, and that one was written after the repo shipped a
 *   build where six files' options were dead code.
 *
 * WHY THIS MARKER. `writeXlsxFile`, `generateXlsxFileContents` and `zipToArrayBuffer` are local
 * bindings and are mangled to nothing by the minifier. `xl/worksheets/sheet` is a STRING LITERAL
 * the writer concatenates a sheet id onto (`xlsx/generateXlsxFileContents.js:118`), so it survives
 * minification verbatim and cannot be produced by anything else in this app.
 *
 * TWO ASSERTIONS, AND THE SECOND MATTERS AS MUCH AS THE FIRST. "Absent from the entry chunk" is
 * also true of a build where the import was deleted outright and `.xlsx` downloads throw — so the
 * marker must be absent from the entry AND present in exactly one other chunk.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ASSETS = join(dirname(fileURLToPath(import.meta.url)), "..", "dist", "assets");
const MARKER = "xl/worksheets/sheet";

function die(message) {
  console.error(`\n  ✗ ${message}\n`);
  process.exit(1);
}

let files;
try {
  files = readdirSync(ASSETS).filter((f) => f.endsWith(".js"));
} catch {
  die(`no ${ASSETS} — run \`npm run build\` first. This check reads the production bundle; there is nothing to assert without one.`);
}

const entries = files.filter((f) => /^index-.*\.js$/.test(f));
if (entries.length === 0) die("no dist/assets/index-*.js — run `npm run build` first.");
if (entries.length > 1) {
  // A stale bundle beside a fresh one has produced a wrong conclusion twice in this programme.
  die(`${entries.length} candidate entry bundles (${entries.join(", ")}) — \`dist/\` is stale. Remove it and rebuild.`);
}
const entry = entries[0];

const carriers = files.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(MARKER));
const size = (f) => statSync(join(ASSETS, f)).size;

if (carriers.includes(entry)) {
  die(
    `${entry} (${size(entry)} bytes) contains "${MARKER}" — the .xlsx writer is in the ENTRY bundle.\n`
    + `    Every dashboard visitor now downloads write-excel-file + fflate to render a chart.\n`
    + `    Check that src/components/chart/export-xlsx.ts still reaches it only through \`await import(...)\`,\n`
    + `    and that nothing imports write-excel-file statically.`,
  );
}
if (carriers.length === 0) {
  die(
    `no chunk in dist/assets contains "${MARKER}" — the .xlsx writer is in NO bundle at all.\n`
    + `    A .xlsx download will throw at runtime. Check that downloadXlsx() is still reachable\n`
    + `    from ChartExportMenu, and that the import specifier is "write-excel-file/browser".`,
  );
}
if (carriers.length > 1) {
  die(`"${MARKER}" appears in ${carriers.length} chunks (${carriers.join(", ")}) — the writer is duplicated.`);
}

console.log(
  `  ✓ ${entry} (${size(entry)} bytes) carries no .xlsx writer; `
  + `write-excel-file is lazy in ${carriers[0]} (${size(carriers[0])} bytes)`,
);
```

- [ ] **Step 2: Wire it into the scripts and CI**

In `frontend/package.json`, add one line beside the existing `test:bundle`:

```json
    "test:bundle": "node scripts/assert-label-layout-bundled.mjs",
    "test:bundle:xlsx": "node scripts/assert-xlsx-lazy.mjs",
```

A **separate** script, not `&&`-chained onto `test:bundle`: two independent facts about the bundle
deserve two exit codes, and a chained failure hides which one broke.

In `.github/workflows/ci.yml`, immediately after the existing `- run: npm run test:bundle` step
(currently line 85):

```yaml
      # Asserts the .xlsx writer is still in its own chunk and still in some chunk. Same reason as
      # the step above: vitest does not code-split, so this can only fail for the right reason
      # against the artefact `npm run build` just produced.
      - run: npm run test:bundle:xlsx
```

- [ ] **Step 3: Build and run the check — record the numbers**

Remove any stale `dist/` first; the script refuses to guess between two entry bundles, but a stale
*lazy* chunk would still be counted.

```bash
cd frontend && rm -rf dist
```

```bash
cd frontend && npm run build
```

Expected: `✓ built in …s`, no Rollup warnings naming `write-excel-file`.

```bash
cd frontend && npm run test:bundle:xlsx
```

Expected, with the real numbers in place of the placeholders:

```text
  ✓ index-XXXXXXXX.js (NNNNNNN bytes) carries no .xlsx writer; write-excel-file is lazy in write-excel-file-XXXXXXXX.js (NNNNN bytes)
```

Then list the chunk table for the PR body:

```bash
cd frontend && ls -l dist/assets
```

Record: the entry chunk's byte size, the lazy chunk's byte size, and the entry chunk's size on
`main` for comparison. **The entry chunk must not have grown by more than a few hundred bytes** —
the dynamic-import shim and nothing else. A jump of tens of kilobytes means the writer is eager
and the marker check has a hole in it.

Get the `main` baseline with:

```bash
cd frontend && git stash --include-untracked
```

```bash
cd frontend && rm -rf dist && npm run build && ls -l dist/assets
```

```bash
cd frontend && git stash pop
```

- [ ] **Step 4: Prove the check can fail**

Temporarily add `import "write-excel-file/browser";` at the top of
`frontend/src/components/chart/export-xlsx.ts`, then:

```bash
cd frontend && rm -rf dist && npm run build
```

```bash
cd frontend && npm run test:bundle:xlsx
```

Expected: FAIL, exit code 1, with the "in the ENTRY bundle" message. Remove the import, rebuild,
re-run, confirm green. A bundle guard that has never been seen to fail is not a guard.

- [ ] **Step 5: Commit**

```bash
git add frontend/scripts/assert-xlsx-lazy.mjs frontend/package.json .github/workflows/ci.yml
git commit -m "test(chart): assert the .xlsx writer stays in its own chunk"
```

---

## Task 4: Metabase's download popover

**Files:**
- Modify: `frontend/src/components/chart/ChartExportMenu.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartExportMenu.test.tsx` (rewritten — the flat
  menu it asserts no longer exists)

- [ ] **Step 1: Rewrite the test**

Replace the whole of `frontend/src/components/chart/__tests__/ChartExportMenu.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef } from "react";

const downloadCsv = vi.fn();
const downloadJson = vi.fn();
const downloadPng = vi.fn((..._a: unknown[]) => Promise.resolve());
const downloadXlsx = vi.fn((..._a: unknown[]) => Promise.resolve());
vi.mock("../export-chart", () => ({
  downloadCsv: (...a: unknown[]) => downloadCsv(...a),
  downloadJson: (...a: unknown[]) => downloadJson(...a),
  downloadPng: (...a: unknown[]) => downloadPng(...a),
}));
vi.mock("../export-xlsx", () => ({ downloadXlsx: (...a: unknown[]) => downloadXlsx(...a) }));

import { ChartExportMenu } from "../ChartExportMenu";

const columns = [{ name: "a", type: "x" }];
const rows = [["1"]];

function Harness({ withSvg, disabled, columnSettings }: { withSvg: boolean; disabled?: boolean; columnSettings?: Record<string, { column_title?: string }> }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <div>
      <div ref={ref} data-testid="container">
        {withSvg ? <svg data-testid="thesvg" /> : <span>table</span>}
      </div>
      <ChartExportMenu title="My Chart" columns={columns} rows={rows} containerRef={ref} disabled={disabled} columnSettings={columnSettings} />
    </div>
  );
}

async function open(withSvg: boolean, columnSettings?: Record<string, { column_title?: string }>) {
  const user = userEvent.setup();
  render(<Harness withSvg={withSvg} columnSettings={columnSettings} />);
  await user.click(screen.getByRole("button", { name: /export chart/i }));
  await screen.findByText("Download data");
  return user;
}

describe("ChartExportMenu", () => {
  beforeEach(() => {
    downloadCsv.mockClear(); downloadJson.mockClear(); downloadPng.mockClear(); downloadXlsx.mockClear();
  });

  it("offers Metabase's three table formats, in its order", async () => {
    await open(false);
    // Asserted by ROLE, not by text: these are radios in a radiogroup, which is what makes the
    // control a segmented control rather than three buttons that happen to look like one.
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.textContent)).toEqual([".csv", ".xlsx", ".json"]);
  });

  it("adds .png only when the container rendered an <svg>", async () => {
    await open(true);
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual([".csv", ".xlsx", ".json", ".png"]);
  });

  it("carries Metabase's formatting checkbox, default on, and its row-limit note", async () => {
    await open(false);
    expect(screen.getByLabelText("keep the data formatted")).toBeChecked();
    expect(screen.getByText("The maximum download size is 1 million rows.")).toBeInTheDocument();
  });

  it("hides the formatting checkbox and the row-limit note for .png", async () => {
    // Metabase's canConfigureFormatting: an image has no cells to format.
    const user = await open(true);
    await user.click(screen.getByRole("radio", { name: ".png" }));
    expect(screen.queryByLabelText("keep the data formatted")).toBeNull();
    expect(screen.queryByText("The maximum download size is 1 million rows.")).toBeNull();
  });

  it("downloads CSV with the formatting flag on by default", async () => {
    const user = await open(false, { a: { column_title: "Column A" } });
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadCsv).toHaveBeenCalledWith("My Chart", columns, rows, { a: { column_title: "Column A" } }, true);
  });

  it("passes the formatting flag off once the checkbox is cleared", async () => {
    const user = await open(false);
    await user.click(screen.getByLabelText("keep the data formatted"));
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadCsv).toHaveBeenCalledWith("My Chart", columns, rows, undefined, false);
  });

  it("downloads .xlsx through the lazy writer when that format is picked", async () => {
    const user = await open(false);
    await user.click(screen.getByRole("radio", { name: ".xlsx" }));
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadXlsx).toHaveBeenCalledWith("My Chart", columns, rows, undefined, true);
    expect(downloadCsv).not.toHaveBeenCalled();
  });

  it("downloads .json", async () => {
    const user = await open(false);
    await user.click(screen.getByRole("radio", { name: ".json" }));
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadJson).toHaveBeenCalledWith("My Chart", columns, rows, undefined, true);
  });

  it("downloads PNG from the container's svg, not from the whole tile", async () => {
    const user = await open(true);
    await user.click(screen.getByRole("radio", { name: ".png" }));
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadPng).toHaveBeenCalledWith("My Chart", screen.getByTestId("thesvg"));
  });

  it("falls back to .csv when a card that had an svg no longer does", async () => {
    // hasSvg is read on open. A card that re-renders as a table between two opens would otherwise
    // leave .png selected and the Download button would silently do nothing.
    const user = userEvent.setup();
    const { rerender } = render(<Harness withSvg />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    await user.click(await screen.findByRole("radio", { name: ".png" }));
    await user.keyboard("{Escape}");
    rerender(<Harness withSvg={false} />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    await screen.findByText("Download data");
    expect(screen.getByRole("radio", { name: ".csv" })).toBeChecked();
  });

  it("disabled prop disables the trigger", () => {
    render(<Harness withSvg disabled />);
    expect((screen.getByRole("button", { name: /export chart/i }) as HTMLButtonElement).disabled).toBe(true);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd frontend && npx vitest run src/components/chart/__tests__/ChartExportMenu.test.tsx
```

Expected: FAIL — `Unable to find an element with the text: Download data`; the component still
renders a dropdown menu with two items.

- [ ] **Step 3: Write the implementation**

Replace the whole of `frontend/src/components/chart/ChartExportMenu.tsx`:

```tsx
import { useId, useState } from "react";
import { Download } from "lucide-react";
import { toast } from "sonner";
import * as RadioGroupPrimitive from "@radix-ui/react-radio-group";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import type { ResultColumn } from "@/components/chart/ChartTable";
import type { ColumnFormat } from "@/components/chart/format-types";
import { downloadCsv, downloadJson, downloadPng } from "./export-chart";
import { downloadXlsx } from "./export-xlsx";

/** Metabase's three table formats in its own order (`common/types/export.ts:4-9`); `.png` is
 *  appended only when the card actually rendered an <svg>, matching its `canSavePng`. */
const TABLE_FORMATS = ["csv", "xlsx", "json"] as const;
type ExportFormat = (typeof TABLE_FORMATS)[number] | "png";

interface Props {
  title: string | undefined;
  columns: ResultColumn[];
  rows: unknown[][];
  containerRef: React.RefObject<HTMLElement>;
  disabled?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
}

/**
 * Metabase's "Download data" popover, not a flat menu: a segmented format control, the
 * "Keep the data formatted" checkbox, the row-limit note, and one Download button
 * (`QuestionDownloadWidget.tsx:157-232` + `ExportSettingsWidget.tsx:80-121`). The shape matters as
 * much as the formats — a flat menu forces the format and the formatting choice into one click and
 * has nowhere to put the note.
 */
export function ChartExportMenu({ title, columns, rows, containerRef, disabled, columnSettings }: Props) {
  // PNG availability is read from the DOM when the popover opens, so it reflects the
  // currently-rendered viz rather than whatever it was the last time this mounted.
  const [hasSvg, setHasSvg] = useState(false);
  const [format, setFormat] = useState<ExportFormat>("csv");
  const [formatted, setFormatted] = useState(true);
  const [busy, setBusy] = useState(false);
  const formattedId = useId();

  const formats: readonly ExportFormat[] = hasSvg ? [...TABLE_FORMATS, "png"] : TABLE_FORMATS;
  // DERIVED, not synced with an effect. A card can stop rendering an <svg> between two opens (a
  // viz override switched it to a table), and a `format` of "png" that is no longer on offer would
  // leave the Download button doing nothing at all.
  const effFormat: ExportFormat = formats.includes(format) ? format : "csv";
  // Metabase's canConfigureFormatting: an image has no cells to format, and no row limit to note.
  const isTable = effFormat !== "png";

  async function run() {
    setBusy(true);
    try {
      if (effFormat === "png") {
        const svg = containerRef.current?.querySelector("svg");
        if (svg) await downloadPng(title, svg);
      } else if (effFormat === "xlsx") {
        await downloadXlsx(title, columns, rows, columnSettings, formatted);
      } else if (effFormat === "json") {
        downloadJson(title, columns, rows, columnSettings, formatted);
      } else {
        downloadCsv(title, columns, rows, columnSettings, formatted);
      }
    } catch (e) {
      console.error(e);
      toast.error(`Failed to export ${effFormat.toUpperCase()}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Popover onOpenChange={(open) => { if (open) setHasSvg(!!containerRef.current?.querySelector("svg")); }}>
      <PopoverTrigger asChild>
        <Button size="sm" variant="ghost" aria-label="Export chart" disabled={disabled}>
          <Download className="h-4 w-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="flex w-72 flex-col gap-4 p-3">
        <div className="text-sm font-medium">Download data</div>
        {/* The Radix primitive rather than `ui/radio-group.tsx`: that wrapper hard-codes a
            `rounded-full` dot, which is the right shape for a list of choices and the wrong one for
            a segmented control. Using the primitive keeps the real radiogroup semantics — roving
            focus, arrow keys, one checked radio — that three styled buttons would not have. */}
        <RadioGroupPrimitive.Root
          aria-label="Download format"
          className="grid auto-cols-fr grid-flow-col gap-1 rounded-md bg-muted p-1"
          value={effFormat}
          onValueChange={(v) => setFormat(v as ExportFormat)}
        >
          {formats.map((f) => (
            <RadioGroupPrimitive.Item
              key={f}
              value={f}
              className="rounded px-2 py-1 text-xs text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring data-[state=checked]:bg-background data-[state=checked]:text-foreground data-[state=checked]:shadow-sm"
            >
              .{f}
            </RadioGroupPrimitive.Item>
          ))}
        </RadioGroupPrimitive.Root>
        {isTable && (
          <label htmlFor={formattedId} className="flex items-start justify-between gap-2 text-sm">
            <span className="flex flex-col gap-0.5">
              <span>Keep the data formatted</span>
              {/* Metabase swaps the example with the switch, so the reader sees what they are
                  choosing rather than a label they have to imagine the effect of. */}
              <span className="text-xs text-muted-foreground">
                {formatted ? "E.g. September 6, 2026 or $187.50, like on screen" : "E.g. 2026-09-06 or 187.50, like in the database"}
              </span>
            </span>
            <Checkbox id={formattedId} aria-label="keep the data formatted" checked={formatted} onCheckedChange={(v) => setFormatted(v === true)} />
          </label>
        )}
        {isTable && <p className="text-xs text-muted-foreground">The maximum download size is 1 million rows.</p>}
        <Button size="sm" className="ml-auto" disabled={busy} onClick={run}>Download</Button>
      </PopoverContent>
    </Popover>
  );
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd frontend && npx vitest run src/components/chart/__tests__/ChartExportMenu.test.tsx
```

Expected: PASS, 11 tests.

- [ ] **Step 5: Prove the tests survive mutation**

1. Replace `const effFormat = formats.includes(format) ? format : "csv";` with
   `const effFormat = format;`. Re-run. Expected: FAIL on the "falls back to .csv" case.
2. In `run`, pass `true` instead of `formatted` to `downloadCsv`. Re-run. Expected: FAIL on
   "passes the formatting flag off".
3. Change `const isTable = effFormat !== "png"` to `const isTable = true`. Re-run. Expected: FAIL
   on the `.png` case.
4. Drop `"xlsx"` from `TABLE_FORMATS`. Re-run. Expected: FAIL on two cases.

Restore all four.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/ChartExportMenu.tsx \
        frontend/src/components/chart/__tests__/ChartExportMenu.test.tsx
git commit -m "feat(chart): Metabase's download popover — four formats, formatting toggle, row-limit note"
```

---

## Task 5: The card title links to its chart, and carries its description

**Files:**
- Create: `frontend/src/components/chart/dashboard/dashcard-href.ts`
- Create: `frontend/src/components/chart/dashboard/DashboardCardHeader.tsx`
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Modify: `frontend/src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx`,
  `…/DashboardCardView.export.test.tsx`, `…/DashboardCardView.vizoverride.test.tsx` (Router wrap —
  see Background §3)
- Test: `frontend/src/components/chart/dashboard/__tests__/dashcard-href.test.ts`,
  `frontend/src/components/chart/dashboard/__tests__/DashboardCardHeader.test.tsx`

- [ ] **Step 1: Write the failing test for the href rule**

Create `frontend/src/components/chart/dashboard/__tests__/dashcard-href.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import type { DashboardCard } from "@/api/dashboards";
import { dashcardHref } from "../dashcard-href";

const card = (over: Partial<DashboardCard> = {}): DashboardCard => ({
  id: "k", chart_id: "ch1", card_type: "chart",
  grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {}, tab_id: null, ...over,
});

describe("dashcardHref", () => {
  it("points at the chart route the router actually declares", () => {
    // App.tsx:94 — `/explore/:workspace/charts/:id`. Read from the router, not guessed: a guessed
    // URL renders NotFound, which reads to a user as a permissions problem.
    expect(dashcardHref("demo", card(), false)).toBe("/explore/demo/charts/ch1");
  });

  it("is suppressed while editing", () => {
    expect(dashcardHref("demo", card(), true)).toBeUndefined();
  });

  it("is suppressed when the title override hides the header", () => {
    // "" is how a reader hides the header, and the link lives on the header.
    expect(dashcardHref("demo", card({ options: { title: "" } }), false)).toBeUndefined();
    expect(dashcardHref("demo", card({ options: { title: "Revenue" } }), false)).toBe("/explore/demo/charts/ch1");
    expect(dashcardHref("demo", card({ options: {} }), false)).toBe("/explore/demo/charts/ch1");
  });

  it("is suppressed for every virtual card kind and for a card with no chart", () => {
    for (const card_type of ["text", "heading", "link", "placeholder"]) {
      expect(dashcardHref("demo", card({ card_type }), false)).toBeUndefined();
    }
    expect(dashcardHref("demo", card({ chart_id: null }), false)).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/dashcard-href.test.ts
```

Expected: FAIL — `Failed to resolve import "../dashcard-href"`.

- [ ] **Step 3: Write it**

Create `frontend/src/components/chart/dashboard/dashcard-href.ts`:

```ts
import type { DashboardCard } from "@/api/dashboards";

/**
 * The chart a dashboard card was built from, as a route — or `undefined` when the card must not
 * offer one. This is egret's `getDashcardHref` (`dashboard/selectors.ts:228-248`), which returns
 * `undefined` for anything that is not a question card.
 *
 * THE ROUTE SHAPE IS READ FROM `App.tsx:94`, not assumed. `/explore/:workspace/charts/:id`.
 *
 * THREE SUPPRESSIONS:
 *  - while EDITING, because the card body is already a click target that opens the card settings
 *    panel, and a link nested inside it would navigate out of an unsaved draft;
 *  - when the title override is exactly `""`, which is how a reader hides the header — the link and
 *    the "Edit question" affordance both hang off the header, so hiding it hides them. `undefined`
 *    (no override at all) and any non-empty override keep them;
 *  - for the virtual card kinds and for a chart card whose `chart_id` is null, which have no chart
 *    to open.
 */
export function dashcardHref(
  ws: string,
  card: Pick<DashboardCard, "card_type" | "chart_id" | "options">,
  editing: boolean | undefined,
): string | undefined {
  if (editing) return undefined;
  if (card.card_type !== "chart" || !card.chart_id) return undefined;
  if (card.options.title === "") return undefined;
  return `/explore/${ws}/charts/${card.chart_id}`;
}
```

Re-run Step 2's command. Expected: PASS, 4 tests.

- [ ] **Step 4: Write the failing test for the header**

Create `frontend/src/components/chart/dashboard/__tests__/DashboardCardHeader.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { DashboardCardHeader } from "../DashboardCardHeader";

function renderHeader(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

describe("DashboardCardHeader", () => {
  it("renders the title as a link when there is an href", () => {
    renderHeader(<DashboardCardHeader text="Revenue" href="/explore/demo/charts/ch1" />);
    expect(screen.getByRole("link", { name: "Revenue" })).toHaveAttribute("href", "/explore/demo/charts/ch1");
  });

  it("renders plain text when there is none, and no link at all", () => {
    renderHeader(<DashboardCardHeader text="Revenue" />);
    expect(screen.getByText("Revenue")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("offers no description affordance when there is no description", () => {
    renderHeader(<DashboardCardHeader text="Revenue" />);
    expect(screen.queryByRole("button", { name: "Card description" })).toBeNull();
  });

  it("shows the description behind an info affordance", async () => {
    const user = userEvent.setup();
    renderHeader(<DashboardCardHeader text="Revenue" description="Net of refunds." />);
    expect(screen.queryByText("Net of refunds.")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Card description" }));
    expect(await screen.findByText("Net of refunds.")).toBeInTheDocument();
  });

  it("carries dash-nodrag on both affordances, so a click is not swallowed by the grid", () => {
    // react-grid-layout's draggableCancel is `.dash-nodrag` (DashboardGridLayout). Without it the
    // link and the info button are drag handles first and controls second.
    renderHeader(<DashboardCardHeader text="Revenue" href="/x" description="d" />);
    expect(screen.getByRole("link", { name: "Revenue" }).className).toContain("dash-nodrag");
    expect(screen.getByRole("button", { name: "Card description" }).className).toContain("dash-nodrag");
  });
});
```

- [ ] **Step 5: Run it to verify it fails, then write the header**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardCardHeader.test.tsx
```

Expected: FAIL — `Failed to resolve import "../DashboardCardHeader"`.

Create `frontend/src/components/chart/dashboard/DashboardCardHeader.tsx`:

```tsx
import { Info } from "lucide-react";
import { Link } from "react-router-dom";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";

/**
 * A chart card's caption: the title, and the description beside it.
 *
 * Metabase's `ChartCaption` reads exactly these two settings — `card.title` and `card.description`
 * (`visualizations/components/ChartCaption.tsx:40-46`) — and makes the title a link when the card
 * resolves to a single question. `dashcard-href.ts` owns the "when".
 *
 * A POPOVER, WHERE METABASE HOVERS A TOOLTIP, and it is a measured choice rather than a shortcut.
 * `components/ui/tooltip.tsx` renders Radix's `Content` with NO `Portal`, so a tooltip lands inside
 * the dashcard body — which is `overflow-hidden` (`DashboardView.tsx:630`) and is frequently
 * shorter than the tooltip. The description would be clipped on exactly the cards most likely to
 * need one. `Popover` is portalled and already used across the product. If `ui/tooltip.tsx` ever
 * grows a `Portal`, this becomes a hover tooltip and the assertion above still holds.
 */
export function DashboardCardHeader({ text, href, description }: { text: string; href?: string; description?: string }) {
  return (
    <div className="flex items-center gap-1.5 px-4 pt-3 pb-2 text-sm font-medium text-foreground">
      {href ? (
        <Link to={href} className="dash-nodrag truncate rounded-sm hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
          {text}
        </Link>
      ) : (
        <span className="truncate">{text}</span>
      )}
      {description && (
        <Popover>
          <PopoverTrigger asChild>
            <button type="button" aria-label="Card description" className="dash-nodrag shrink-0 rounded-sm text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
              <Info className="h-3.5 w-3.5" aria-hidden />
            </button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-64 p-3 text-sm font-normal whitespace-pre-wrap">
            {description}
          </PopoverContent>
        </Popover>
      )}
    </div>
  );
}
```

Re-run. Expected: PASS, 5 tests.

- [ ] **Step 6: Wire the header and the "Edit question" affordance into the card**

In `frontend/src/components/chart/dashboard/DashboardCardView.tsx`, add to the imports (the
`lucide-react` and `react-router-dom` lines are new; the last three join the existing local block):

```tsx
import { Pencil } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { DashboardCardHeader } from "./DashboardCardHeader";
import { dashcardHref } from "./dashcard-href";
```

In `ChartCardTileBody`, add `ws` to the destructured props:

```tsx
function ChartCardTileBody({ ws, card, chart, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, data }: ChartCardTileProps & { data: ChartData }) {
```

Below the existing `const eff = applyVizOverride(...)` line, add the two per-card values. They sit
*after* the `if (!chart)` guard on purpose — a card whose chart is missing has nothing to link to:

```tsx
  // Both hang off the header, so both die with it — `dashcardHref` encodes that rule once.
  const href = dashcardHref(ws, card, editing);
  const description = asString(card.options.description);
```

Replace the export-affordance block (currently `{canExport && (…)}`) with the two-affordance
cluster. The pencil is a second `<svg>` in the tile, which is exactly why it goes here — outside
`chartRef` — and why the "does NOT offer Download PNG for a table card" regression test still has
teeth:

```tsx
      {(canExport || href) && (
        <div className="dash-nodrag absolute right-1 top-1 z-10 flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
          {href && (
            <Button asChild size="sm" variant="ghost" aria-label="Edit question">
              <Link to={href}><Pencil className="h-4 w-4" /></Link>
            </Button>
          )}
          {canExport && (
            <ChartExportMenu title={headerText || chart?.name} columns={data.columns} rows={data.rows}
              columnSettings={shellSettings(eff.params).columnSettings} containerRef={chartRef} />
          )}
        </div>
      )}
```

Replace the header line:

```tsx
      {showHeader && <DashboardCardHeader text={headerText} href={href} description={description} />}
```

- [ ] **Step 7: Give the three existing DashboardCardView test files a Router**

Each of the three renders `DashboardCardView` bare and will now throw
`useHref() may be used only in the context of a <Router>`. Add this helper to the top of each,
after the imports:

```tsx
// DashboardCardView renders react-router <Link>s (the card title and the Edit-question
// affordance), which throw outside a Router.
function renderCard(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}
```

with `import { MemoryRouter } from "react-router-dom";` added to each file's imports, then replace
every `render(<DashboardCardView` with `renderCard(<DashboardCardView`.

**One call site cannot be swept.** `DashboardCardView.test.tsx:55-57` destructures `rerender`; a
`rerender` of a bare element would drop the Router. Rewrite that one by hand:

```tsx
    const { rerender } = renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={chart} paramFilters={[{ col: "region", op: "eq" as const, value: "EU" }]} />);
```

```tsx
    rerender(<MemoryRouter><DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={chart} paramFilters={[{ col: "region", op: "eq" as const, value: "US" }]} /></MemoryRouter>);
```

- [ ] **Step 8: Update the export-affordance test for the popover, and add the link assertions**

In `frontend/src/components/chart/dashboard/__tests__/DashboardCardView.export.test.tsx`, the two
cases that assert menu item text now assert the popover's radios. Replace their bodies:

```tsx
  it("offers .png for a chart card (chart svg present in the render container)", async () => {
    const user = userEvent.setup();
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    expect(await screen.findByRole("radio", { name: ".csv" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: ".png" })).toBeInTheDocument();
  });

  // Regression: the export popover must read the render-only container, not the whole tile. The
  // tile's hover cluster now holds TWO icons — a Download and an Edit-question pencil — and if the
  // popover pointed at the tile, a table card would find one of them and wrongly offer .png,
  // exporting a 16×16 icon.
  it("does NOT offer .png for a table card (no chart svg), despite the pencil in the hover cluster", async () => {
    const user = userEvent.setup();
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={tableChart} parameters={[]} paramValues={{}} />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    expect(await screen.findByRole("radio", { name: ".csv" })).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: ".png" })).toBeNull();
  });
```

Then append a new `describe` to the same file, covering the wiring this task added:

```tsx
describe("DashboardCardView chart link", () => {
  it("titles the card with a link to its chart", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.getByRole("link", { name: "C" })).toHaveAttribute("href", "/explore/eng/charts/c");
  });

  it("offers Edit question beside the export trigger", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.getByRole("link", { name: "Edit question" })).toHaveAttribute("href", "/explore/eng/charts/c");
  });

  it("offers neither while editing", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} editing />);
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("offers neither when the title override hides the header", () => {
    const hidden = { ...(card as object), options: { title: "" } } as never;
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={hidden} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("shows options.description behind the card's info affordance", async () => {
    const user = userEvent.setup();
    const described = { ...(card as object), options: { description: "Net of refunds." } } as never;
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={described} chart={chart} parameters={[]} paramValues={{}} />);
    await user.click(screen.getByRole("button", { name: "Card description" }));
    expect(await screen.findByText("Net of refunds.")).toBeInTheDocument();
  });
});
```

- [ ] **Step 9: Run the dashboard suite**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/
```

Expected: PASS. Every previously-green dashboard test must still be green; the only file whose
*count* changes is `DashboardCardView.export.test.tsx` (+5).

- [ ] **Step 10: Prove the new assertions survive mutation**

1. In `dashcardHref`, delete `if (editing) return undefined;`. Re-run. Expected: FAIL.
2. In `dashcardHref`, delete `if (card.options.title === "") return undefined;`. Re-run.
   Expected: FAIL.
3. In `DashboardCardHeader`, render `{text}` as a plain `<span>` unconditionally. Re-run.
   Expected: FAIL.
4. In `DashboardCardView`, pass `description={undefined}`. Re-run. Expected: FAIL.

Restore all four.

- [ ] **Step 11: Commit**

```bash
git add frontend/src/components/chart/dashboard/dashcard-href.ts \
        frontend/src/components/chart/dashboard/DashboardCardHeader.tsx \
        frontend/src/components/chart/dashboard/DashboardCardView.tsx \
        frontend/src/components/chart/dashboard/__tests__/dashcard-href.test.ts \
        frontend/src/components/chart/dashboard/__tests__/DashboardCardHeader.test.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardCardView.export.test.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardCardView.vizoverride.test.tsx
git commit -m "feat(dashboard): card title links to its chart, plus Edit question and a per-card description"
```

---

## Task 6: Any card can drop its chrome

**Files:**
- Create: `frontend/src/components/chart/dashboard/card-chrome.ts`
- Modify: `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (one expression, net −4 lines)
- Test: `frontend/src/components/chart/dashboard/__tests__/card-chrome.test.ts`, plus additions to
  `…/CardSettingsPanel.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/card-chrome.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { cardChromeClass, showsCardBackground } from "../card-chrome";

const card = (card_type: string, options: Record<string, unknown> = {}) => ({ card_type, options });

describe("showsCardBackground", () => {
  it("defaults on for a chart, text or link card", () => {
    expect(showsCardBackground(card("chart"))).toBe(true);
    expect(showsCardBackground(card("text"))).toBe(true);
    expect(showsCardBackground(card("link"))).toBe(true);
  });

  it("defaults OFF for a heading, which egret has always rendered chrome-less", () => {
    expect(showsCardBackground(card("heading"))).toBe(false);
  });

  it("defaults off for a placeholder, which draws its own dashed target", () => {
    expect(showsCardBackground(card("placeholder"))).toBe(false);
  });

  it("lets the stored option win over the default, in BOTH directions", () => {
    // Both directions, because "generalise the heading's chrome-less rendering to any card" also
    // means a heading whose author switched the background on gets chrome. A one-way override
    // would make the toggle a no-op on the one card type that motivated it.
    expect(showsCardBackground(card("chart", { background: false }))).toBe(false);
    expect(showsCardBackground(card("heading", { background: true }))).toBe(true);
  });

  it("ignores a non-boolean stored value rather than coercing it", () => {
    expect(showsCardBackground(card("chart", { background: "false" }))).toBe(true);
    expect(showsCardBackground(card("heading", { background: 1 }))).toBe(false);
  });
});

describe("cardChromeClass", () => {
  it("paints border + card background when the background is on", () => {
    expect(cardChromeClass(card("chart"))).toBe("rounded-sm border bg-card");
  });

  it("falls back to the canvas colour, not to nothing, for a chrome-less card", () => {
    expect(cardChromeClass(card("heading"))).toBe("bg-dashboard");
    expect(cardChromeClass(card("chart", { background: false }))).toBe("bg-dashboard");
  });

  it("gives a placeholder no classes at all", () => {
    expect(cardChromeClass(card("placeholder"))).toBe("");
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/card-chrome.test.ts
```

Expected: FAIL — `Failed to resolve import "../card-chrome"`.

- [ ] **Step 3: Write it**

Create `frontend/src/components/chart/dashboard/card-chrome.ts`:

```ts
import type { DashboardCard } from "@/api/dashboards";

type CardChrome = Pick<DashboardCard, "card_type" | "options">;

/**
 * Does this card paint the card chrome — border plus card background?
 *
 * Metabase's `dashcard.background` ("Show background", `getDefault: () => true`,
 * `dashboard/visualizations/Text/index.ts:67-73`), with egret's existing exceptions folded in
 * rather than bolted beside: a HEADING has always rendered chrome-less (`HeadingCard.tsx`, and the
 * `bg-dashboard` branch this replaces in `DashboardView.tsx`), and a PLACEHOLDER draws its own
 * dashed drop target.
 *
 * So the default is per card type and the stored option, when present, wins over it — in BOTH
 * directions. Generalising the heading's rendering to any card has to mean a heading can go the
 * other way too, or the toggle is a no-op on the card type that motivated it.
 *
 * `typeof stored === "boolean"` rather than a truthiness check: `options` is `Record<string,
 * unknown>` and is never key-whitelisted server-side, so a hand-edited `"false"` is reachable, and
 * coercing it would silently strip a chart card's chrome.
 */
export function showsCardBackground(card: CardChrome): boolean {
  const stored = card.options.background;
  if (typeof stored === "boolean") return stored;
  return card.card_type !== "heading" && card.card_type !== "placeholder";
}

/** The chrome classes for a card body. A chrome-less card falls back to `bg-dashboard`, not to
 *  nothing: the tile still has to paint the canvas colour beneath it, which is what the heading
 *  branch has always done. A placeholder paints neither — its own component owns its look. */
export function cardChromeClass(card: CardChrome): string {
  if (showsCardBackground(card)) return "rounded-sm border bg-card";
  return card.card_type === "placeholder" ? "" : "bg-dashboard";
}
```

Re-run Step 2's command. Expected: PASS, 8 tests.

- [ ] **Step 4: Add the two settings controls**

In `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx`, add to the imports:

```tsx
import { Textarea } from "@/components/ui/textarea";
import { showsCardBackground } from "./card-chrome";
```

Add `backgroundRow` immediately after `hideEmptyRow` (before the `text`/`heading` early return, so
all three branches can use it):

```tsx
  // Metabase's label verbatim ("Show background", `Text/index.ts:67-73`). EVERY card type gets it,
  // which is the generalisation §4 W5 asks for — Metabase offers it on text, heading, link and
  // iframe cards and egret has no reason to draw the line anywhere else. `showsCardBackground`
  // owns the default so the checkbox and the tile cannot disagree: a heading reads unchecked until
  // someone switches it on.
  const backgroundRow = (
    <label htmlFor={`cs-background-${card.id}`} className="flex items-center justify-between gap-2 text-sm"><span>Show background</span>
      <Checkbox id={`cs-background-${card.id}`} aria-label="show background" checked={showsCardBackground(card)} onCheckedChange={(v) => onUpdateOptions({ background: v === true })} />
    </label>
  );
```

Add `{backgroundRow}` to all three branches, directly after `{autoHeightRow}` in the
text/heading branch and the link branch, and after `{hideEmptyRow}` in the chart branch:

```tsx
        {autoHeightRow}
        {backgroundRow}
        {moveToTab}
```

```tsx
      {autoHeightRow}
      {hideEmptyRow}
      {backgroundRow}
      {moveToTab}
```

And in the chart branch, add the Description field directly under the existing Title field:

```tsx
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Description
        <Textarea aria-label="card description" rows={2} placeholder="Shown behind the info button on the card header"
          value={(card.options.description as string) ?? ""} onChange={(e) => onUpdateOptions({ description: e.target.value })} />
      </label>
```

- [ ] **Step 5: Extend the settings-panel test**

Append to `frontend/src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx`, inside
the existing `describe("CardSettingsPanel (chart card)")`:

```tsx
  it("edits the per-card description", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("card description"), { target: { value: "Net of refunds." } });
    expect(onUpdateOptions.mock.calls[0][0]).toStrictEqual({ description: "Net of refunds." });
  });

  it("shows the background on by default for a chart card and writes only that key", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    expect(screen.getByLabelText("show background")).toBeChecked();
    fireEvent.click(screen.getByLabelText("show background"));
    // toStrictEqual, for the same reason the hide-when-empty case gives: the point is that the
    // patch carries NOTHING but background.
    expect(onUpdateOptions).toHaveBeenCalledTimes(1);
    expect(onUpdateOptions.mock.calls[0][0]).toStrictEqual({ background: false });
  });

  it("uses Metabase's wording for the background toggle", () => {
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.getByText("Show background")).toBeInTheDocument();
  });
```

and inside `describe("CardSettingsPanel (text card)")`:

```tsx
  it("offers the background toggle, reading OFF for a heading", () => {
    const c: DraftCard = { id: "h1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 2, options: { text: "" }, tab_id: null, tabKey: null };
    render(<CardSettingsPanel card={c} chart={undefined} parameters={[param]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.getByLabelText("show background")).not.toBeChecked();
  });
```

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx
```

Expected: PASS, 13 tests.

- [ ] **Step 6: Wire the route — and let it shrink**

In `frontend/src/routes/explore/DashboardView.tsx`, add the import beside the other dashboard
component imports:

```tsx
import { cardChromeClass } from "@/components/chart/dashboard/card-chrome";
```

Replace the five-line chrome ternary at the dashcard body (currently `:630-634`) with one call.
Before:

```tsx
                className={`relative h-full overflow-hidden ${editing && c.card_type !== "placeholder" ? "cursor-pointer" : ""} ${
                c.card_type === "heading" ? "bg-dashboard"
                : c.card_type === "placeholder" ? ""
                : "rounded-sm border bg-card"
              } ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
```

After:

```tsx
                className={`relative h-full overflow-hidden ${editing && c.card_type !== "placeholder" ? "cursor-pointer" : ""} ${cardChromeClass(c)} ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
```

The route ends this workstream **four lines shorter** than it started. That is the test for whether
the extraction was real: a workstream that adds net lines to `DashboardView.tsx` has a missing
extraction (index, "File structure").

- [ ] **Step 7: Prove the new assertions survive mutation**

1. In `showsCardBackground`, `return true;`. Re-run `card-chrome.test.ts` and
   `CardSettingsPanel.test.tsx`. Expected: FAIL in both.
2. In `showsCardBackground`, change the guard to `if (stored !== undefined) return !!stored;`.
   Re-run `card-chrome.test.ts`. Expected: FAIL on the non-boolean case.
3. In `cardChromeClass`, return `""` for the chrome-less branch. Re-run. Expected: FAIL.

Restore all three.

- [ ] **Step 8: Run every gate, each as its own command**

Never chain them, and never pipe into `rtk` — a swallowed pipe breaks the `&&` chain behind it and
the later gates silently never run.

```bash
cd frontend && npm run lint
```

```bash
cd frontend && npx tsc --noEmit
```

```bash
cd frontend && npm run test
```

Expected: all green. Record the exact test count; it should exceed `main`'s by the number added
here (8 + 7 + 11 − 6 replaced + 4 + 5 + 5 + 8 + 4 = **46 net new**, minus the 6 rewritten
`ChartExportMenu` cases already counted). A count that differs from the previous PR's by more than
that means a gate did not run.

Then rebuild and re-run both bundle checks, because Task 4 changed what imports what:

```bash
cd frontend && rm -rf dist && npm run build
```

```bash
cd frontend && npm run test:bundle
```

```bash
cd frontend && npm run test:bundle:xlsx
```

- [ ] **Step 9: Commit**

```bash
git add frontend/src/components/chart/dashboard/card-chrome.ts \
        frontend/src/components/chart/dashboard/CardSettingsPanel.tsx \
        frontend/src/components/chart/dashboard/__tests__/card-chrome.test.ts \
        frontend/src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): per-card Show background toggle, generalising the heading card's chrome-less render"
```

---

## Task 7: Live check — measure, do not eyeball

**Files:** none. This task produces the numbers for the PR body.

Every defect that mattered in the chart programme was found by measuring the running app. A
screenshot that "looks right" is not evidence, and a subagent's *simulation* of a browser is not
evidence either.

- [ ] **Step 1: Build and deploy**

```bash
just deploy-kind
```

Expected: `deployment "egret" successfully rolled out`, SPA served at `http://localhost:30080`.

Open `http://localhost:30080/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`
(admin@egret.local / Admin12345!) at 1440×900. **Do not edit the harness dashboard's cards** — 22
cards per side on byte-identical data; a hand-edit invalidates the whole harness
(`docs/en/guides/chart-parity-harness.md`).

- [ ] **Step 2: Confirm the xlsx chunk is fetched only on demand**

Open DevTools → Network, filter `.js`, and reload. Then hover a chart card, open the download
popover, pick `.xlsx`, and press Download.

Record: the chunk filename that appears **only after** the Download click, and its transferred
size. Required: nothing matching `write-excel-file` or `fflate` in the initial page load, and
exactly one such chunk after the click. This is the runtime half of Task 3's static assertion — the
script proves the chunk exists separately, this proves nothing pulls it early.

Watch for a `Refused to create a worker` console error: `fflate` zips through a blob-URL worker. The
repo's `nginx-prod.conf` sets no CSP, so this should not fire; if it does, the deployment has grown
one and `worker-src blob:` is the fix.

- [ ] **Step 3: Measure the four downloaded files**

With the four formats downloaded from the same card (default settings, "Keep the data formatted"
on):

```bash
ls -l ~/Downloads
```

Record the byte size of each of the four. Required: all four **> 1000 bytes**. A file of a few
dozen bytes is a header with no rows.

```bash
file ~/Downloads/*.xlsx
```

Required: `Microsoft Excel 2007+` (or `Zip archive data`). A `.xlsx` that reports as ASCII text is
a CSV with the wrong extension.

```bash
unzip -l ~/Downloads/*.xlsx
```

Required: the listing contains `xl/worksheets/sheet1.xml` and `xl/sharedStrings.xml`. Record the
uncompressed size of `sheet1.xml`.

```bash
node -e "const j=require('fs').readFileSync(process.argv[1],'utf8');const a=JSON.parse(j);console.log('rows',a.length,'keys',Object.keys(a[0]))" ~/Downloads/*.json
```

Required: `rows` equals the row count the card shows on screen, and `keys` are the **display**
column names (the rename the card applies), because formatting was on.

- [ ] **Step 4: Measure the PNG — pixel dimensions AND bytes**

This is the step that has caught a real defect in this repo. Pick a chart card that renders an
`<svg>`, download `.png`, then:

```bash
sips -g pixelWidth -g pixelHeight ~/Downloads/*.png
```

```bash
ls -l ~/Downloads/*.png
```

Required: `pixelWidth` and `pixelHeight` are roughly **twice** the card's on-screen CSS size
(`svgToPngBlob` rasterises at `scale = 2`), so on a 4-column harness card measuring ~184×~250 CSS
px expect ~368×~500, and bytes **> 10000**.

**A 16×16 or 32×32 result means the export grabbed a UI icon, not the chart** — that has happened
in this repo, and it is now a live risk again because this workstream put a second `<svg>` (the
Edit-question pencil) into the card's hover cluster. If you see it, `chartRef` has stopped pointing
at the render-only container.

Record the exact `pixelWidth × pixelHeight` and the byte count.

- [ ] **Step 5: Measure "Keep the data formatted" actually changes the file**

Download `.csv` twice from the same card, once with the checkbox on and once off. Then:

```bash
head -3 ~/Downloads/<formatted>.csv
```

```bash
head -3 ~/Downloads/<raw>.csv
```

Required: the two files **differ**, on the header row (display name vs machine name for any renamed
column) and/or on a numeric cell (grouping separator present vs absent). Quote both header rows and
one differing data row in the PR body. Two identical files mean the flag is not reaching the writer
and every unit test in Task 1 is passing for the wrong reason.

- [ ] **Step 6: Measure the card-level affordances**

At 1440×900 on the harness dashboard, evaluate in the console:

```js
(() => {
  const cards = [...document.querySelectorAll('.react-grid-item')];
  return {
    cards: cards.length,
    titleLinks: document.querySelectorAll('.react-grid-item a[href*="/charts/"]').length,
    editQuestion: [...document.querySelectorAll('.react-grid-item a[aria-label="Edit question"]')].length,
    infoButtons: document.querySelectorAll('[aria-label="Card description"]').length,
  };
})()
```

Required: `titleLinks` equals the number of chart cards whose header is visible, `editQuestion`
equals the same number, and `infoButtons` is 0 before any description is set. Record all four.

Then click one title link. Required: it lands on `/explore/demo/charts/<id>` and the chart opens —
not a NotFound page.

- [ ] **Step 7: Measure the background toggle on a real card**

Enter edit mode, open a chart card's settings, clear **Show background**, and evaluate:

```js
(() => {
  const el = document.querySelector('[data-testid="dashcard-body"]');
  const cs = getComputedStyle(el);
  return { border: cs.borderTopWidth, bg: cs.backgroundColor, radius: cs.borderTopLeftRadius };
})()
```

Required, with the toggle **off**: `border: "0px"` and `bg` equal to the dashboard canvas colour —
the same value a heading card reports. Record both readings (on and off), and record the heading
card's reading beside them; if the chrome-less chart card and the heading card disagree, the
generalisation in `card-chrome.ts` did not land.

Then set a Description on the same card, leave edit mode, and click the info button. Required: the
popover is **fully visible** and is not clipped by the card's `overflow-hidden` — which is the
entire reason it is a Popover and not a Tooltip (Background §4). Record the description text and a
note that it rendered outside the card bounds.

- [ ] **Step 8: Open the PR and stop**

Open it with, quoted verbatim: the bundle numbers from Task 3 Step 3 (entry chunk before and after,
lazy chunk size), the four file sizes and the `unzip -l` listing, the PNG's `pixelWidth ×
pixelHeight` and bytes, the two differing CSV header rows, the four affordance counts, and the
frontend test count from Task 6 Step 8.

Then stop. The merge is the user's call.

---

## Self-review checklist for the implementer

- [ ] `DashboardView.tsx` is **shorter** than it was on `main`. If it is not, an extraction is
      missing — check the index's "File structure" note.
- [ ] `export-chart.ts`'s original test file was not edited, and all eleven of its cases are still
      green.
- [ ] `write-excel-file` is named in exactly **one** place in `src/`: the `await import(...)` in
      `export-xlsx.ts`. Confirm with a search, not from memory.
- [ ] `npm run test:bundle:xlsx` has been **seen to fail** (Task 3 Step 4), not just seen to pass.
- [ ] The PNG live check recorded pixel dimensions, not just "it downloaded".
- [ ] `dashcardHref`'s three suppressions each have a test that fails when that suppression alone is
      removed.
- [ ] `showsCardBackground` was exercised in **both** override directions — a heading forced on as
      well as a chart forced off.
- [ ] The format control is a real `radiogroup` (assertions use `getByRole("radio", …)`), not three
      buttons that look like one.
- [ ] No test asserts a display string where the code keys on a raw one. In particular, the CSV
      assertions key on the exact emitted bytes, not on "contains the number".
- [ ] Every mutation run was scored by **exit code**, never by matching stdout text — the
      `PASS (n) FAIL (m)` line is the rtk wrapper's reformatting and raw vitest never prints it.
