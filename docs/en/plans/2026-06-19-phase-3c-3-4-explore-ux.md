# Phase 3c-3.4 — Chart Explore UX (gallery + Data/Display tabs) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign the chart Explore UX to match Metabase/Superset — a chart-type gallery on new-chart, and a two-tab (Data / Display) left panel with Format + Conditional folded into Display (right sidebar removed).

**Architecture:** Pure frontend restructuring. A `chartTypes` registry + reusable `ChartTypeGallery`; `ChartExplore` renders the gallery when creating a new chart (no `?type`), else the editor with a viz header ("Change visualization") and a `Tabs` left panel (Data / Display). Reuses `FormatSection`, `ConditionalFormattingSection`, renderers, persist/hydrate verbatim. No backend/engine/data-model change.

**Tech Stack:** React 18 + Vite + TypeScript + Vitest + shadcn/ui (`Tabs`, `Dialog`) + lucide-react.

**Spec:** `docs/en/specs/2026-06-19-phase-3c-3-4-explore-ux-design.md`.

**Branch:** `feat/phase-3c-3-4-explore-ux` (spec committed). Built on merged main (3c-3.3 = `30c8bbf`).

**Conventions:** plain `git commit` (never `--author`); jest-dom in `vitest.setup.ts`; shadcn primitives in `@/components/ui/*`.

---

## File Structure
- Create `frontend/src/components/chart/chartTypes.ts` — viz-type registry.
- Create `frontend/src/components/chart/ChartTypeGallery.tsx` (+ test) — reusable card grid.
- Modify `frontend/src/routes/explore/ChartExplore.tsx` — gallery branch + viz header + 2-tab layout, remove the right sidebar.
- Delete `frontend/src/components/chart/settings/SettingsSidebar.tsx` (superseded).

---

## Task 1: Chart-type registry + gallery component

**Files:**
- Create: `frontend/src/components/chart/chartTypes.ts`
- Create: `frontend/src/components/chart/ChartTypeGallery.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartTypeGallery.test.tsx`

- [ ] **Step 1: Create the registry**

`frontend/src/components/chart/chartTypes.ts`:

```ts
import { Table2, LayoutGrid, type LucideIcon } from "lucide-react";
import type { VizType } from "@/api/charts";

export interface ChartTypeMeta {
  key: VizType;
  name: string;
  description: string;
  icon: LucideIcon;
}

export const CHART_TYPES: ChartTypeMeta[] = [
  {
    key: "table",
    name: "Table",
    description: "Raw rows or an aggregated table with sorting and formatting.",
    icon: Table2,
  },
  {
    key: "pivot_table",
    name: "Pivot Table",
    description: "Cross-tab rows × columns × measures with subtotals and grand totals.",
    icon: LayoutGrid,
  },
];
```

- [ ] **Step 2: Write the failing gallery test**

`frontend/src/components/chart/__tests__/ChartTypeGallery.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ChartTypeGallery } from "../ChartTypeGallery";

describe("ChartTypeGallery", () => {
  it("renders a card per type and calls onSelect on click", () => {
    const onSelect = vi.fn();
    render(<ChartTypeGallery onSelect={onSelect} />);
    expect(screen.getByText("Table")).toBeInTheDocument();
    expect(screen.getByText("Pivot Table")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Pivot Table"));
    expect(onSelect).toHaveBeenCalledWith("pivot_table");
  });
});
```

- [ ] **Step 3: Run → FAIL.** `cd frontend && npm test -- ChartTypeGallery`

- [ ] **Step 4: Implement the gallery**

`frontend/src/components/chart/ChartTypeGallery.tsx`:

```tsx
import type { VizType } from "@/api/charts";
import { CHART_TYPES } from "./chartTypes";

export function ChartTypeGallery({
  selected, onSelect,
}: { selected?: VizType; onSelect: (key: VizType) => void }) {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
      {CHART_TYPES.map((t) => {
        const Icon = t.icon;
        const active = selected === t.key;
        return (
          <button
            key={t.key}
            type="button"
            onClick={() => onSelect(t.key)}
            className={`flex flex-col items-start gap-2 rounded-lg border p-4 text-left transition hover:border-primary/50 hover:bg-muted/40 ${
              active ? "border-primary ring-2 ring-primary/30" : ""
            }`}
          >
            <Icon className="h-6 w-6 text-primary" />
            <div className="font-semibold">{t.name}</div>
            <div className="text-xs text-muted-foreground">{t.description}</div>
          </button>
        );
      })}
    </div>
  );
}
```

- [ ] **Step 5: Run + gates + commit**

`cd frontend && npm test -- ChartTypeGallery` → PASS. `npx tsc --noEmit` + `npx eslint src/components/chart/chartTypes.ts src/components/chart/ChartTypeGallery.tsx` → clean.
```bash
git add frontend/src/components/chart/chartTypes.ts frontend/src/components/chart/ChartTypeGallery.tsx frontend/src/components/chart/__tests__/ChartTypeGallery.test.tsx
git commit -m "feat(web): chart-type registry + reusable ChartTypeGallery"
```

---

## Task 2: ChartExplore restructure (gallery branch + viz header + Data/Display tabs)

**Files:**
- Modify (full rewrite of the render + imports): `frontend/src/routes/explore/ChartExplore.tsx`
- Delete: `frontend/src/components/chart/settings/SettingsSidebar.tsx`

This rewrites the component's layout while keeping ALL existing state/logic
(datasetId, viz, table/pivot controls, rowLimit, columnSettings, conditionalFormatting,
specForRun, build/submit/run, save/paramsForSave, changeViz, hydration). The diff: (a) read
`?type`; (b) render the gallery when creating a new chart with no type; (c) a viz header with
a "Change visualization" dialog instead of the Table/Pivot toggle; (d) a `Tabs` Data/Display
left panel; (e) remove the right `SettingsSidebar` and `settingsOpen` state — Format +
Conditional move into the Display tab; (f) full-width preview.

- [ ] **Step 1: Delete the superseded sidebar**

```bash
git rm frontend/src/components/chart/settings/SettingsSidebar.tsx
```
(No test existed for it. `ChartExplore` was its only importer — that import is removed below.)

- [ ] **Step 2: Replace `ChartExplore.tsx` with the restructured version**

Replace the ENTIRE file `frontend/src/routes/explore/ChartExplore.tsx` with:

```tsx
import { useEffect, useState } from "react";
import { useParams, Link, useNavigate, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { useDatasets, useDataset } from "@/api/datasets";
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams,
} from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";
import { ChartTable } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/pivot/PivotTable";
import { ChartTypeGallery } from "@/components/chart/ChartTypeGallery";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FormatSection } from "@/components/chart/settings/FormatSection";
import { ConditionalFormattingSection } from "@/components/chart/settings/ConditionalFormattingSection";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter,
} from "@/components/ui/dialog";

function OrderedPicker({
  label, selected, onChange, options,
}: { label: string; selected: string[]; onChange: (v: string[]) => void; options: string[] }) {
  const remaining = options.filter((o) => !selected.includes(o));
  return (
    <div className="space-y-1">
      <div className="text-sm">{label}</div>
      <div className="flex flex-wrap gap-1">
        {selected.map((s) => (
          <span key={s} className="inline-flex items-center gap-1 rounded bg-muted px-2 py-0.5 text-xs">
            {s}
            <button type="button" className="text-muted-foreground hover:text-destructive"
              onClick={() => onChange(selected.filter((x) => x !== s))}>×</button>
          </span>
        ))}
      </div>
      {remaining.length > 0 && (
        <Select value="" onValueChange={(v) => v && onChange([...selected, v])}>
          <SelectTrigger className="h-8"><SelectValue placeholder={`Add ${label.toLowerCase()}…`} /></SelectTrigger>
          <SelectContent>
            {remaining.map((o) => <SelectItem key={o} value={o}>{o}</SelectItem>)}
          </SelectContent>
        </Select>
      )}
    </div>
  );
}

const UNDERLINE_LIST = "h-auto w-full justify-start gap-4 rounded-none border-b bg-transparent p-0";
const UNDERLINE_TRIGGER = "rounded-none border-b-2 border-transparent px-0 pb-2 text-sm data-[state=active]:border-primary data-[state=active]:bg-transparent data-[state=active]:text-foreground data-[state=active]:shadow-none";

export default function ChartExplore() {
  const ws = useParams().workspace!;
  const { id } = useParams(); // present when editing a saved chart
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const typeParam = searchParams.get("type");
  const datasets = useDatasets(ws);
  const existing = useChart(ws, id ?? null);

  const [datasetId, setDatasetId] = useState<string | null>(null);
  const detail = useDataset(ws, datasetId);
  const [viz, setViz] = useState<VizType>((typeParam as VizType) || "table");
  const [vizPickerOpen, setVizPickerOpen] = useState(false);

  // table-mode controls
  const [tableMetrics, setTableMetrics] = useState<string[]>([]);
  const [tableCols, setTableCols] = useState<string[]>([]);
  // pivot-mode controls (multi-dim ordered)
  const [pRows, setPRows] = useState<string[]>([]);
  const [pCols, setPCols] = useState<string[]>([]);
  const [pVals, setPVals] = useState<string[]>([]);
  const [showRowTotals, setShowRowTotals] = useState(true);
  const [showColTotals, setShowColTotals] = useState(true);
  const [condense, setCondense] = useState(true);
  const [rowLimit, setRowLimit] = useState(1000);
  const [columnSettings, setColumnSettings] = useState<Record<string, ColumnFormat>>({});
  const [conditionalFormatting, setConditionalFormatting] = useState<Rule[]>([]);

  // hydrate from a saved chart once it loads (dataset/viz/controls + save dialog fields)
  useEffect(() => {
    const c = existing.data;
    if (!c) return;
    setDatasetId(c.dataset_id);
    setViz(c.viz_type);
    const p = c.params;
    if (c.viz_type === "pivot_table") {
      setPRows(p.rows ?? []);
      setPCols(p.cols ?? []);
      setPVals(p.metrics ?? []);
      setShowRowTotals(p.showRowTotals ?? true);
      setShowColTotals(p.showColTotals ?? true);
      setCondense(p.condense ?? true);
    } else {
      setTableMetrics(p.spec?.metrics ?? []);
      setTableCols([...(p.spec?.dimensions ?? []), ...(p.spec?.columns ?? [])]);
    }
    setRowLimit(p.spec?.row_limit ?? 1000);
    setColumnSettings(p.columnSettings ?? {});
    setConditionalFormatting(p.conditionalFormatting ?? []);
    setSaveName(c.name);
    setSaveDesc(c.description);
  }, [existing.data]);

  const columns = detail.data?.columns ?? [];
  const metrics = detail.data?.metrics ?? [];

  function specForRun(): QuerySpec {
    if (viz === "pivot_table") {
      const dims = [...pRows, ...pCols];
      const groups = [pRows, pCols].filter((g) => g.length > 0);
      return {
        metrics: pVals, dimensions: dims, columns: [], filters: [], orderby: [],
        row_limit: rowLimit, rollup_groups: groups,
      };
    }
    const agg = tableMetrics.length > 0;
    return {
      metrics: tableMetrics, dimensions: agg ? tableCols : [], columns: agg ? [] : tableCols,
      filters: [], orderby: [], row_limit: rowLimit, rollup_groups: [],
    };
  }

  const build = useBuildChartData(ws);
  const submit = useSubmitQuery(ws);
  const [queryId, setQueryId] = useState<string | null>(null);
  const status = useQueryStatus(ws, queryId);
  const finished = status.data?.status === "finished";
  const results = useResults(ws, queryId, 0, finished);
  const resultColumns = (results.data?.columns ?? []).filter((c) => !c.name.startsWith("__grp_"));
  const metricTypes = Object.fromEntries((detail.data?.metrics ?? []).map((m) => [m.metric_name, m.data_type ?? "double"]));

  async function run() {
    setQueryId(null);
    if (!datasetId) { toast.error("Pick a dataset first"); return; }
    if (viz === "pivot_table" && (pRows.length === 0 || pVals.length === 0)) {
      toast.error("Pivot needs at least one row dimension and one value"); return;
    }
    try {
      const { sql, database_id } = await build.mutateAsync({ dataset_id: datasetId, query_spec: specForRun() });
      const ack = await submit.mutateAsync({ database_id, sql, limit: rowLimit });
      setQueryId(ack.id);
    } catch (e) { toast.error(String(e)); }
  }

  const mutations = useChartMutations(ws);
  const [saveOpen, setSaveOpen] = useState(false);
  const [saveName, setSaveName] = useState("");
  const [saveDesc, setSaveDesc] = useState("");

  function paramsForSave(): ChartParams {
    return viz === "pivot_table"
      ? { spec: specForRun(), rows: pRows, cols: pCols, metrics: pVals, showRowTotals, showColTotals, condense, columnSettings, conditionalFormatting }
      : { spec: specForRun(), columnSettings, conditionalFormatting };
  }

  async function save() {
    if (!datasetId) { toast.error("Pick a dataset"); return; }
    if (!saveName.trim()) { toast.error("Name required"); return; }
    if (viz === "pivot_table" && (pRows.length === 0 || pVals.length === 0)) {
      toast.error("Pivot needs at least one row dimension and one value"); return;
    }
    try {
      if (id) {
        await mutations.update.mutateAsync({ id, name: saveName.trim(), description: saveDesc, viz_type: viz, params: paramsForSave() });
        setSaveOpen(false);
        toast.success("Saved");
      } else {
        const created = await mutations.create.mutateAsync({ dataset_id: datasetId, name: saveName.trim(), description: saveDesc, viz_type: viz, params: paramsForSave() });
        setSaveOpen(false);
        toast.success("Saved");
        navigate(`/explore/${ws}/charts/${created.id}`);
      }
    } catch (e) { toast.error(String(e)); }
  }

  function changeViz(v: VizType) {
    setViz(v);
    setQueryId(null);
  }

  function toggleTableMetric(name: string) {
    setTableMetrics((prev) => prev.includes(name) ? prev.filter((m) => m !== name) : [...prev, name]);
  }
  function toggleTableCol(name: string) {
    setTableCols((prev) => prev.includes(name) ? prev.filter((c) => c !== name) : [...prev, name]);
  }

  function resetControls() {
    setTableMetrics([]); setTableCols([]); setPRows([]); setPCols([]); setPVals([]);
    setColumnSettings({}); setConditionalFormatting([]); setQueryId(null);
  }

  const isRunning = status.data?.status === "queued" || status.data?.status === "running";
  const vizMeta = CHART_TYPES.find((t) => t.key === viz);

  // ── New-chart gallery: shown when creating a chart with no type chosen yet ──
  if (!id && !typeParam) {
    return (
      <div className="mx-auto max-w-3xl p-6">
        <Link className="text-xs text-muted-foreground hover:underline" to={`/explore/${ws}/charts`}>← Charts</Link>
        <h1 className="mb-1 mt-2 text-lg font-semibold">Choose a visualization</h1>
        <p className="mb-4 text-sm text-muted-foreground">Pick a chart type to start building.</p>
        <ChartTypeGallery onSelect={(k) => { setViz(k); setSearchParams({ type: k }); }} />
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 gap-0">
      {/* Left panel */}
      <div className="w-72 shrink-0 overflow-y-auto border-r p-4 space-y-4">
        <Link className="text-xs text-muted-foreground hover:underline" to={`/explore/${ws}/charts`}>← Charts</Link>

        {/* Viz header + change */}
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            {vizMeta && <vizMeta.icon className="h-4 w-4 text-primary" />}
            <span className="text-sm font-medium">{vizMeta?.name ?? viz}</span>
          </div>
          <Button size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={() => setVizPickerOpen(true)}>
            Change
          </Button>
        </div>

        <Tabs defaultValue="data">
          <TabsList className={UNDERLINE_LIST}>
            <TabsTrigger value="data" className={UNDERLINE_TRIGGER}>Data</TabsTrigger>
            <TabsTrigger value="display" className={UNDERLINE_TRIGGER}>Display</TabsTrigger>
          </TabsList>

          {/* ── DATA TAB ── */}
          <TabsContent value="data" className="space-y-4">
            <div className="space-y-1">
              <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Dataset</Label>
              <Select value={datasetId ?? ""} onValueChange={(v) => { setDatasetId(v || null); resetControls(); }}>
                <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dataset…" /></SelectTrigger>
                <SelectContent>
                  {(datasets.data ?? []).map((d) => (
                    <SelectItem key={d.id} value={d.id}>{d.name}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {viz === "table" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metrics</Label>
                  <p className="text-xs text-muted-foreground">Pick metrics for an aggregated table, or just columns for raw rows.</p>
                  <div className="space-y-1 pt-1">
                    {metrics.length === 0 && <p className="text-xs italic text-muted-foreground">No metrics on this dataset.</p>}
                    {metrics.map((m) => (
                      <div key={m.metric_name} className="flex items-center gap-2">
                        <Checkbox id={`metric-${m.metric_name}`} checked={tableMetrics.includes(m.metric_name)} onCheckedChange={() => toggleTableMetric(m.metric_name)} />
                        <Label htmlFor={`metric-${m.metric_name}`} className="cursor-pointer text-sm font-normal">{m.metric_name}</Label>
                      </div>
                    ))}
                  </div>
                </div>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Columns</Label>
                  <div className="space-y-1 pt-1">
                    {columns.length === 0 && <p className="text-xs italic text-muted-foreground">Select a dataset to see columns.</p>}
                    {columns.map((col) => (
                      <div key={col.column_name} className="flex items-center gap-2">
                        <Checkbox id={`col-${col.column_name}`} checked={tableCols.includes(col.column_name)} onCheckedChange={() => toggleTableCol(col.column_name)} />
                        <Label htmlFor={`col-${col.column_name}`} className="cursor-pointer text-sm font-normal">{col.column_name}</Label>
                      </div>
                    ))}
                  </div>
                </div>
              </>
            )}

            {viz === "pivot_table" && (
              <>
                <OrderedPicker label="Rows" selected={pRows} onChange={setPRows}
                  options={columns.map((c) => c.column_name).filter((n) => !pCols.includes(n))} />
                <OrderedPicker label="Columns" selected={pCols} onChange={setPCols}
                  options={columns.map((c) => c.column_name).filter((n) => !pRows.includes(n))} />
                <OrderedPicker label="Values" selected={pVals} onChange={setPVals}
                  options={metrics.map((m) => m.metric_name)} />
              </>
            )}

            <div className="space-y-1">
              <Label htmlFor="row-limit" className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Row limit</Label>
              <Input id="row-limit" type="number" min={1} max={10000} value={rowLimit}
                onChange={(e) => { const n = Number(e.target.value); setRowLimit(Number.isNaN(n) ? 0 : n); }}
                className="h-8 text-sm" />
            </div>
          </TabsContent>

          {/* ── DISPLAY TAB ── */}
          <TabsContent value="display" className="space-y-4">
            {viz === "pivot_table" && (
              <div className="space-y-2">
                <label className="flex items-center gap-2 text-sm">
                  <Checkbox checked={showRowTotals} onCheckedChange={(v) => setShowRowTotals(!!v)} /> Row totals
                </label>
                <label className="flex items-center gap-2 text-sm">
                  <Checkbox checked={showColTotals} onCheckedChange={(v) => setShowColTotals(!!v)} /> Column totals
                </label>
                <label className="flex items-center gap-2 text-sm">
                  <Checkbox checked={condense} onCheckedChange={(v) => setCondense(!!v)} /> Condense duplicate totals
                </label>
              </div>
            )}

            {resultColumns.length === 0 ? (
              <p className="text-xs text-muted-foreground">Run the chart to configure column formatting.</p>
            ) : (
              <>
                <div className="space-y-1">
                  <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Formatting</div>
                  <FormatSection columns={resultColumns} value={columnSettings} onChange={setColumnSettings} />
                </div>
                <div className="space-y-1">
                  <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Conditional formatting</div>
                  <ConditionalFormattingSection columns={resultColumns} rules={conditionalFormatting} isPivot={viz === "pivot_table"} onChange={setConditionalFormatting} />
                </div>
              </>
            )}
          </TabsContent>
        </Tabs>
      </div>

      {/* Main area (full-width preview) */}
      <div className="flex-1 overflow-auto p-4 space-y-3">
        <div className="flex items-center gap-2">
          <Button onClick={run} disabled={build.isPending || submit.isPending} size="sm">
            {(build.isPending || submit.isPending) ? "Building…" : "Run"}
          </Button>
          <Button variant="outline" size="sm" onClick={() => setSaveOpen(true)}>{id ? "Update" : "Save"}</Button>
          {build.data?.sql && (
            <details className="ml-auto text-xs text-muted-foreground">
              <summary className="cursor-pointer select-none hover:text-foreground">View SQL</summary>
              <pre className="mt-1 max-h-40 overflow-auto rounded bg-muted p-2 text-xs">{build.data.sql}</pre>
            </details>
          )}
        </div>

        {status.data?.status === "failed" && (
          <p className="rounded bg-destructive/10 p-3 text-sm text-destructive">{status.data.error}</p>
        )}
        {isRunning && <p className="text-sm text-muted-foreground">Running…</p>}

        {finished && results.data && viz === "pivot_table" && (
          <PivotTable
            input={{
              columns: results.data.columns.map((c) => c.name),
              rows: results.data.rows,
              rowDims: pRows, colDims: pCols, metrics: pVals,
              showRowTotals, showColTotals, condense, collapsed: new Set(),
            }}
            columnSettings={columnSettings}
            conditionalFormatting={conditionalFormatting}
            metricTypes={metricTypes}
          />
        )}
        {finished && results.data && viz !== "pivot_table" && (
          <ChartTable columns={results.data.columns} rows={results.data.rows}
            columnSettings={columnSettings} conditionalFormatting={conditionalFormatting} />
        )}
        {!queryId && !build.isPending && !submit.isPending && (
          <p className="text-sm text-muted-foreground">Configure the controls on the left and click Run.</p>
        )}
      </div>

      {/* Change-visualization dialog */}
      <Dialog open={vizPickerOpen} onOpenChange={setVizPickerOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>Change visualization</DialogTitle></DialogHeader>
          <div className="py-2">
            <ChartTypeGallery selected={viz} onSelect={(k) => { changeViz(k); setVizPickerOpen(false); }} />
          </div>
        </DialogContent>
      </Dialog>

      {/* Save dialog */}
      <Dialog open={saveOpen} onOpenChange={setSaveOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>{id ? "Update chart" : "Save chart"}</DialogTitle></DialogHeader>
          <div className="space-y-3 py-2">
            <div className="space-y-1">
              <Label htmlFor="chart-name">Name</Label>
              <Input id="chart-name" value={saveName} onChange={(e) => setSaveName(e.target.value)} placeholder="Chart name…" />
            </div>
            <div className="space-y-1">
              <Label htmlFor="chart-desc">Description</Label>
              <Input id="chart-desc" value={saveDesc} onChange={(e) => setSaveDesc(e.target.value)} placeholder="Optional description…" />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setSaveOpen(false)}>Cancel</Button>
            <Button onClick={save} disabled={mutations.create.isPending || mutations.update.isPending}>{id ? "Update" : "Save"}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
```

NOTES for the implementer:
- All React hooks are declared before the gallery early-return (the early return is after every hook call), satisfying the rules of hooks. Do not move hooks below the `if (!id && !typeParam)` return.
- The gallery `onSelect` sets `viz` AND `setSearchParams({ type: k })` so the URL reflects the choice and the editor renders.
- `FormatSection`/`ConditionalFormattingSection` are the same components the deleted `SettingsSidebar` used — now imported + rendered directly in the Display tab.
- `resetControls()` consolidates the dataset-change reset (was inline before).

- [ ] **Step 3: Typecheck, lint, build, test**

`cd frontend && npm run typecheck && npm run lint && npm run build && npm test`
Expected: 0 type errors; lint clean; build ok; all existing tests pass (no test imports the deleted `SettingsSidebar`; the renderer/section/engine/gallery tests are unaffected).

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(web): chart Explore — type gallery + Data/Display tabs, drop right sidebar"
```

---

## Task 3: Sweep + manual smoke + final review

**Files:** none (verification).

- [ ] **Step 1: Frontend sweep**

`cd frontend && npm test && npm run typecheck && npm run lint && npm run build` → all green.

- [ ] **Step 2: Manual Playwright smoke** (vite HMR; backend unchanged — no migration)

Login `admin@egret.local` / `Admin12345!`, ws `eng`:
1. Explore → Charts → **+ Chart** → the **gallery** shows (Table, Pivot Table). Pick **Pivot Table** → editor opens (URL `…/charts/new?type=pivot_table`).
2. **Data tab**: pick dataset `orders`, Rows = `orderpriority`, Values = `revenue`. **Run** → pivot renders.
3. **Display tab**: toggle totals; **Formatting** → `revenue` → Currency → cells reformat; **Conditional** → add a rule → cells color.
4. **Change** (viz header) → dialog gallery → pick **Table** → re-run → table renders (config preserved).
5. **Save** → reopen the saved chart → gallery is skipped (it has an id), editor hydrates viz + tabs + formatting.

- [ ] **Step 3: Final review**

Dispatch a final reviewer over the phase `git diff`. Focus: rules-of-hooks (gallery early-return after all hooks), the `?type` routing + gallery↔editor transition, the Data/Display tab content correctness, that Format/Conditional moved intact (no behavior change), the removed sidebar leaves no dangling refs, persist/hydrate unchanged.

- [ ] **Step 4: Final commit (if fixes)**

```bash
git add -A && git commit -m "test: phase 3c-3.4 sweep + manual smoke fixes"
```

Then use **superpowers:finishing-a-development-branch**.

---

## Self-Review notes (plan author)
- **Spec coverage:** registry + gallery (T1); gallery routing branch + viz header + Data/Display tabs + sidebar removal + delete SettingsSidebar (T2); sweep+smoke+review (T3).
- **Type consistency:** `VizType`, `CHART_TYPES`/`ChartTypeMeta`, `ChartTypeGallery({selected,onSelect})`, `FormatSection`/`ConditionalFormattingSection` props all match their existing definitions. `useSearchParams` is react-router v6 (already the project's router).
- **No backend/engine/param change** — `paramsForSave`/hydration/`specForRun` are byte-identical to the merged version; only layout + the new-chart entry changed.
