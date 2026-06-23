# Chart Export (CSV + PNG) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users download a chart's data as CSV and its rendered image as PNG, from the chart builder and dashboard cards.

**Architecture:** Frontend-only. A pure `export-chart.ts` (CSV from the already-client-side `columns`/`rows`; PNG by rasterising the rendered `<svg>` to a canvas). A `ChartExportMenu` dropdown (CSV always; PNG only when an `<svg>` is present) wired into `ChartExplore` (next to Run/Save) and `DashboardCardView` (view-mode hover affordance).

**Tech Stack:** React 18 + TS + Vite, Vitest + @testing-library/react, shadcn `dropdown-menu`, ECharts (SVG renderer).

**Reused contracts (verified):**
- `ResultColumn` (`api/charts.ts`): `{ name: string; type: string }`.
- `ChartRenderer({ vizType, params, columns, rows, ... })`; chart viz → `EChartsRenderer` (renders an `<svg>`, `renderer:"svg"`, no `foreignObject`); table → `ChartTable`; pivot → `PivotTable` (no `<svg>`).
- `ChartExplore.tsx`: `resultColumns` (`results.data?.columns` filtered, ~line 202), `results.data?.rows`, `finished`; Run + Save controls; a full-width preview area (~line 713).
- `DashboardCardView.tsx` → `ChartCardTile`: `const data = useChartData(...)` → `{ columns, rows, isLoading, error }`; `headerText = titleOverride || chart?.name`; outer `<div className="flex h-full flex-col">` wraps a `<div className="min-h-0 flex-1 overflow-auto px-2 pb-2">` that contains the `ChartRenderer`.
- shadcn `src/components/ui/dropdown-menu.tsx` and `popover.tsx` exist. `dash-nodrag` class prevents react-grid-layout from hijacking clicks (Grid uses `draggableCancel=".dash-nodrag"`).

**Conventions for every task:** TDD (failing test first). Full sweep before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage explicit paths. Configured git identity. End every commit message with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Branch `feat/chart-export` (spec committed). **Canvas/Image/Blob-download are NOT supported in jsdom** — unit-test `rowsToCsv`/`exportFilename` + `ChartExportMenu` item logic (with mocked download fns); `svgToPngBlob`/`downloadBlob` are verified in the live test (Task 5), not Vitest.

---

### Task 1: `export-chart.ts` utils

**Files:**
- Create: `frontend/src/components/chart/export-chart.ts`
- Test: `frontend/src/components/chart/__tests__/export-chart.test.ts`

- [ ] **Step 1: Write the failing test** `export-chart.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { rowsToCsv, exportFilename } from "../export-chart";

const cols = [{ name: "region", type: "VARCHAR" }, { name: "rev", type: "DOUBLE" }];

describe("rowsToCsv", () => {
  it("emits a header row then data rows, CRLF-separated", () => {
    expect(rowsToCsv(cols, [["EU", 10], ["NA", 20]])).toBe('region,rev\r\nEU,10\r\nNA,20');
  });
  it("quotes cells containing comma, quote, or newline (doubling internal quotes)", () => {
    expect(rowsToCsv([{ name: "a", type: "x" }], [["x,y"], ['he"llo'], ["line\nbreak"]]))
      .toBe('a\r\n"x,y"\r\n"he""llo"\r\n"line\nbreak"');
  });
  it("renders null/undefined as empty and stringifies others", () => {
    expect(rowsToCsv(cols, [[null, undefined as unknown]])).toBe('region,rev\r\n,');
    expect(rowsToCsv([{ name: "n", type: "x" }], [[0], [false]])).toBe('n\r\n0\r\nfalse');
  });
});

describe("exportFilename", () => {
  it("sanitises unsafe chars and appends the extension", () => {
    expect(exportFilename("Revenue / Region: 2026", "csv")).toBe("Revenue _ Region_ 2026.csv");
  });
  it("falls back to 'chart' for blank/undefined", () => {
    expect(exportFilename(undefined, "png")).toBe("chart.png");
    expect(exportFilename("   ", "csv")).toBe("chart.csv");
  });
});
```

- [ ] **Step 2: Run** `npx vitest run src/components/chart/__tests__/export-chart.test.ts` — FAIL (module missing).

- [ ] **Step 3: Implement** `export-chart.ts`:

```ts
import type { ResultColumn } from "@/api/charts";

function csvCell(v: unknown): string {
  if (v === null || v === undefined) return "";
  const s = String(v);
  return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

/** RFC-4180 CSV: a header row of column names, then one row per data row (CRLF-separated). */
export function rowsToCsv(columns: ResultColumn[], rows: unknown[][]): string {
  const header = columns.map((c) => csvCell(c.name)).join(",");
  const body = rows.map((r) => r.map(csvCell).join(",")).join("\r\n");
  return body ? `${header}\r\n${body}` : header;
}

/** Sanitise a title into a safe filename stem (fallback "chart"); append `.ext`. */
export function exportFilename(title: string | undefined, ext: string): string {
  const stem = (title ?? "").replace(/[\\/:*?"<>|]/g, "_").trim() || "chart";
  return `${stem}.${ext}`;
}

/** Trigger a client-side download of a Blob. */
export function downloadBlob(filename: string, blob: Blob): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

export function downloadCsv(title: string | undefined, columns: ResultColumn[], rows: unknown[][]): void {
  downloadBlob(exportFilename(title, "csv"), new Blob([rowsToCsv(columns, rows)], { type: "text/csv;charset=utf-8" }));
}

/** Rasterise an <svg> element to a PNG Blob via Image → canvas. ECharts SVG has no foreignObject, so no taint. */
export function svgToPngBlob(svg: SVGElement, scale = 2): Promise<Blob> {
  return new Promise((resolve, reject) => {
    const rect = svg.getBoundingClientRect();
    const w = Math.max(1, Math.round((rect.width || 600) * scale));
    const h = Math.max(1, Math.round((rect.height || 400) * scale));
    const xml = new XMLSerializer().serializeToString(svg);
    const svgUrl = URL.createObjectURL(new Blob([xml], { type: "image/svg+xml;charset=utf-8" }));
    const img = new Image();
    img.onload = () => {
      URL.revokeObjectURL(svgUrl);
      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      const ctx = canvas.getContext("2d");
      if (!ctx) { reject(new Error("no 2d context")); return; }
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(0, 0, w, h);
      ctx.drawImage(img, 0, 0, w, h);
      canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("toBlob failed"))), "image/png");
    };
    img.onerror = () => { URL.revokeObjectURL(svgUrl); reject(new Error("svg image load failed")); };
    img.src = svgUrl;
  });
}

export async function downloadPng(title: string | undefined, svg: SVGElement): Promise<void> {
  downloadBlob(exportFilename(title, "png"), await svgToPngBlob(svg));
}
```

- [ ] **Step 4: Run** the test — PASS.

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/export-chart.ts src/components/chart/__tests__/export-chart.test.ts
git commit -m "feat(chart): export-chart utils (rowsToCsv, svgToPngBlob, download helpers)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `ChartExportMenu` component

**Files:**
- Create: `frontend/src/components/chart/ChartExportMenu.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartExportMenu.test.tsx`

- [ ] **Step 1: Write the failing test** `ChartExportMenu.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { useRef } from "react";

const downloadCsv = vi.fn();
const downloadPng = vi.fn();
vi.mock("../export-chart", () => ({ downloadCsv: (...a: unknown[]) => downloadCsv(...a), downloadPng: (...a: unknown[]) => downloadPng(...a) }));

import { ChartExportMenu } from "../ChartExportMenu";

function Harness({ withSvg, disabled }: { withSvg: boolean; disabled?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <div>
      <div ref={ref} data-testid="container">{withSvg ? <svg data-testid="thesvg" /> : <span>table</span>}</div>
      <ChartExportMenu title="My Chart" columns={[{ name: "a", type: "x" }]} rows={[["1"]]} containerRef={ref} disabled={disabled} />
    </div>
  );
}

describe("ChartExportMenu", () => {
  beforeEach(() => { downloadCsv.mockClear(); downloadPng.mockClear(); });

  it("shows CSV + PNG when the container has an <svg>", async () => {
    render(<Harness withSvg />);
    fireEvent.click(screen.getByRole("button", { name: "export chart" }));
    expect(await screen.findByText("Download CSV")).toBeTruthy();
    expect(screen.getByText("Download PNG")).toBeTruthy();
  });
  it("shows only CSV when there is no <svg> (table/pivot)", async () => {
    render(<Harness withSvg={false} />);
    fireEvent.click(screen.getByRole("button", { name: "export chart" }));
    expect(await screen.findByText("Download CSV")).toBeTruthy();
    expect(screen.queryByText("Download PNG")).toBeNull();
  });
  it("Download CSV calls downloadCsv with the title/columns/rows", async () => {
    render(<Harness withSvg />);
    fireEvent.click(screen.getByRole("button", { name: "export chart" }));
    fireEvent.click(await screen.findByText("Download CSV"));
    expect(downloadCsv).toHaveBeenCalledWith("My Chart", [{ name: "a", type: "x" }], [["1"]]);
  });
  it("disabled prop disables the trigger", () => {
    render(<Harness withSvg disabled />);
    expect((screen.getByRole("button", { name: "export chart" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
```

- [ ] **Step 2: Run** `npx vitest run src/components/chart/__tests__/ChartExportMenu.test.tsx` — FAIL (module missing).

- [ ] **Step 3: Read** `src/components/ui/dropdown-menu.tsx` to confirm the exported parts (`DropdownMenu`, `DropdownMenuTrigger`, `DropdownMenuContent`, `DropdownMenuItem`). Then create `ChartExportMenu.tsx`:

```tsx
import { useState } from "react";
import { Download } from "lucide-react";
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem } from "@/components/ui/dropdown-menu";
import { Button } from "@/components/ui/button";
import { toast } from "sonner";
import type { ResultColumn } from "@/api/charts";
import { downloadCsv, downloadPng } from "./export-chart";

interface Props {
  title: string | undefined;
  columns: ResultColumn[];
  rows: unknown[][];
  containerRef: React.RefObject<HTMLElement>;
  disabled?: boolean;
}

export function ChartExportMenu({ title, columns, rows, containerRef, disabled }: Props) {
  // PNG availability is read from the DOM when the menu opens (reflects the currently-rendered viz).
  const [hasSvg, setHasSvg] = useState(false);
  return (
    <DropdownMenu onOpenChange={(open) => { if (open) setHasSvg(!!containerRef.current?.querySelector("svg")); }}>
      <DropdownMenuTrigger asChild>
        <Button size="sm" variant="ghost" aria-label="export chart" disabled={disabled}><Download className="h-4 w-4" /></Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onSelect={() => downloadCsv(title, columns, rows)}>Download CSV</DropdownMenuItem>
        {hasSvg && (
          <DropdownMenuItem onSelect={() => {
            const svg = containerRef.current?.querySelector("svg");
            if (svg) downloadPng(title, svg as SVGElement).catch((e) => toast.error(String(e)));
          }}>Download PNG</DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
```
NOTE: if the shadcn `DropdownMenuItem` uses Radix `onSelect` (it does), the test's `fireEvent.click` on the item text triggers it. If the test can't see the items because Radix portals them, they still render in the document — `findByText` works. If Radix's `onSelect` doesn't fire on `fireEvent.click` in jsdom, switch the items to `onClick` handlers (keep the same calls). Make the 4 tests pass; adjust the trigger/item wiring minimally if the shadcn API differs (read the file first).

- [ ] **Step 4: Run** the test — PASS.

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/ChartExportMenu.tsx src/components/chart/__tests__/ChartExportMenu.test.tsx
git commit -m "feat(chart): ChartExportMenu (CSV always, PNG when an svg is rendered)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Wire into the chart builder (`ChartExplore`)

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`
- Test: none new (covered live in Task 5; the unit surface is Task 2).

- [ ] **Step 1: Read** `ChartExplore.tsx` and locate: `resultColumns` (~line 202), `results` (`results.data?.rows`), `finished`, the Run + Save buttons, the chart's display name (the loaded chart name — pass it as the export `title`; if none is readily available, pass `undefined` and `exportFilename` falls back to `"chart"`), and the preview/chart container element (the div that wraps the rendered chart/table — around line 713+, "Main area / full-width preview").

- [ ] **Step 2: Add the import** at the top:
```tsx
import { ChartExportMenu } from "@/components/chart/ChartExportMenu";
import { useRef } from "react"; // add useRef to the existing react import if not present
```

- [ ] **Step 3: Add a ref to the preview container.** On the div that wraps the rendered chart preview (the one whose subtree contains `<ChartRenderer>`/`<ChartTable>`/`<PivotTable>`), add `ref={previewRef}`. Declare it near the other refs/state (above the early returns, with the other hooks):
```tsx
  const previewRef = useRef<HTMLDivElement>(null);
```

- [ ] **Step 4: Render the export menu** next to Run/Save (in the same control row as those buttons):
```tsx
        <ChartExportMenu
          title={/* the chart's display name var, or undefined */ undefined}
          columns={resultColumns}
          rows={results.data?.rows ?? []}
          containerRef={previewRef}
          disabled={!finished || resultColumns.length === 0}
        />
```
Replace the `title` value with the chart's actual name variable if one is in scope (e.g. the loaded chart's `name`); otherwise leave `undefined`. `disabled` should be true until a result exists (`finished` + non-empty columns).

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/routes/explore/ChartExplore.tsx
git commit -m "feat(chart): export menu in the chart builder

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Wire into dashboard cards (`DashboardCardView` / `ChartCardTile`)

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (pass an `editing` flag so export hides in edit mode)
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardCardView.export.test.tsx` (create)

- [ ] **Step 1: Write the failing test** `DashboardCardView.export.test.tsx`. Mock `ChartRenderer` and `useChartData`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("@/components/chart/ChartRenderer", () => ({ ChartRenderer: () => <div data-testid="cr" /> }));
const useChartData = vi.fn();
vi.mock("@/api/useChartData", () => ({ useChartData: (...a: unknown[]) => useChartData(...a) }));

import { DashboardCardView } from "../DashboardCardView";

const chart = { id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "", viz_type: "bar", params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } } as never;
const card = { id: "k", card_type: "chart", chart_id: "c", options: {} } as never;

describe("DashboardCardView export affordance", () => {
  it("shows the export button for a loaded chart card in view mode", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.getByRole("button", { name: "export chart" })).toBeTruthy();
  });
  it("hides the export button while loading", () => {
    useChartData.mockReturnValue({ columns: [], rows: [], isLoading: true, error: null });
    render(<DashboardCardView ws="eng" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.queryByRole("button", { name: "export chart" })).toBeNull();
  });
  it("hides the export button in edit mode", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" card={card} chart={chart} parameters={[]} paramValues={{}} editing />);
    expect(screen.queryByRole("button", { name: "export chart" })).toBeNull();
  });
});
```
(Confirm the data-hook module path is `@/api/useChartData` — read `DashboardCardView.tsx`'s import and match the mock to it.)

- [ ] **Step 2: Run** — FAIL (no export button).

- [ ] **Step 3: Implement.** In `DashboardCardView.tsx`:
1. Imports: `import { useRef } from "react";`, `import { ChartExportMenu } from "@/components/chart/ChartExportMenu";`.
2. Add an optional `editing?: boolean` prop to BOTH `DashboardCardView` and `ChartCardTile` (thread it through the call at the bottom: `<ChartCardTile ... editing={editing} />`).
3. In `ChartCardTile`, add a `tileRef` on the outer container and an export affordance shown only when there's data and not editing/loading/blocked:
```tsx
  const tileRef = useRef<HTMLDivElement>(null);
  const canExport = !editing && !blocked && !data.isLoading && !data.error && (data.columns?.length ?? 0) > 0;
```
Wrap the existing outer `<div className="flex h-full flex-col">` so it has `ref={tileRef}` and `className="group relative flex h-full flex-col"`, and add the affordance (hover-revealed, `dash-nodrag` so RGL doesn't hijack it):
```tsx
      {canExport && (
        <div className="dash-nodrag absolute right-1 top-1 z-10 opacity-0 transition-opacity group-hover:opacity-100">
          <ChartExportMenu title={headerText || chart?.name} columns={data.columns} rows={data.rows} containerRef={tileRef} />
        </div>
      )}
```
   (`headerText`/`chart?.name` are already computed in `ChartCardTile`. Keep all existing render branches.)
4. In `src/routes/explore/DashboardView.tsx`, find where `<DashboardCardView .../>` is rendered inside the card grid and pass `editing={editing}` so the affordance hides in edit mode. (`editing` is already in scope there.)

- [ ] **Step 4: Run** the test — PASS, plus the existing `DashboardCardView.test.tsx` + the dashboard `__tests__` dir (no regression).

- [ ] **Step 5: Full sweep + commit**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/DashboardCardView.tsx src/routes/explore/DashboardView.tsx src/components/chart/dashboard/__tests__/DashboardCardView.export.test.tsx
git commit -m "feat(dashboards): per-card export affordance (CSV + PNG) in view mode

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Live verification + PR

**Files:** none (verification only).

- [ ] **Step 1: Final sweep** (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run` → green.

- [ ] **Step 2: Run the stack & log in** (frontend-only change — the dev backend on :8080 is fine as-is; `npm run dev`), `admin@egret.local` / `Admin12345!`, eng workspace. **Do NOT touch "a1".** Use a throwaway dashboard and delete it after.

- [ ] **Step 3: Live-verify (Playwright MCP), 0 console errors:**
1. **Chart builder**: open a chart with an ECharts viz (e.g. a bar/pie), Run, then the export menu → **Download CSV** (a `.csv` with the right header+rows downloads) and **Download PNG** (a `.png` downloads). Open a **table** chart → the menu shows **only CSV** (no PNG).
2. **Dashboard card**: on a throwaway dashboard with a chart card, hover the card (view mode) → the export affordance → **CSV** + **PNG**. A **table** card → CSV only. Confirm the affordance does **not** appear in edit mode or while loading.
3. Verify the downloaded CSV content matches the displayed columns/rows and the PNG opens as an image.

- [ ] **Step 4: Clean up & open PR**
```bash
git push -u origin feat/chart-export
```
Open PR `feat(chart): export chart to CSV + PNG (builder + dashboard cards)`; summarise CSV (all viz) + PNG (ECharts viz, SVG→canvas), both locations; note frontend-only, MVP exports the displayed rows. End the PR body with the `🤖 Generated with [Claude Code]` footer. Squash-merge when CI is green (a flaky `egret-ranger` rust test may need a rerun — unrelated to this frontend change).

---

## Self-Review

**Spec coverage:**
- `rowsToCsv` + `downloadCsv` + `exportFilename` + `downloadBlob` + `svgToPngBlob` + `downloadPng` → Task 1. ✓
- `ChartExportMenu` (CSV always; PNG only when `<svg>`; disabled) → Task 2. ✓
- Chart builder wiring (next to Run/Save, preview ref, disabled until result) → Task 3. ✓
- Dashboard card wiring (view-mode hover affordance, `dash-nodrag`, hidden while loading/blocked/editing) → Task 4. ✓
- Error handling: header-only CSV for no rows (Task 1 `rowsToCsv`), PNG hidden without `<svg>` (Task 2), toast on PNG failure (Task 2), disabled gating (Tasks 2–4). ✓
- Testing: Vitest for CSV/filename/menu-logic (Tasks 1–2), card affordance (Task 4); live for the canvas/PNG path (Task 5). ✓

**Type/signature consistency:**
- `ResultColumn` `{name,type}` used in `rowsToCsv` (T1), `ChartExportMenu` props (T2), and both wirings (T3/T4).
- `ChartExportMenu({ title, columns, rows, containerRef, disabled? })` — identical usage in ChartExplore (T3) and ChartCardTile (T4).
- `downloadCsv(title, columns, rows)` / `downloadPng(title, svg)` — defined T1, mocked/asserted T2.
- `editing?: boolean` threaded DashboardCardView → ChartCardTile (T4) and supplied by DashboardView (T4).

**Placeholder scan:** No TBD/TODO; every code step shows complete code. The one intentionally-open value — the chart-builder export `title` — is bounded (use the chart's name var if in scope, else `undefined`, which `exportFilename` resolves to `"chart"`).
