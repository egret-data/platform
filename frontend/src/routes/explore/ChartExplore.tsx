import { useEffect, useState } from "react";
import { useParams, Link, useNavigate, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { useDatasets, useDataset } from "@/api/datasets";
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams, type TableState, type ChartFilter,
} from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";
import { ChartTable } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/pivot/PivotTable";
import { FilterChips } from "@/components/chart/FilterChips";
import { DrillMenu } from "@/components/chart/DrillMenu";
import { DrillRecordsDialog } from "@/components/chart/DrillRecordsDialog";
import type { DrillContext } from "@/components/chart/drill";
import { ChartTypeGallery } from "@/components/chart/ChartTypeGallery";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FormatSection } from "@/components/chart/settings/FormatSection";
import { ConditionalFormattingSection } from "@/components/chart/settings/ConditionalFormattingSection";
import { TableColumnsSection } from "@/components/chart/settings/TableColumnsSection";
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
import { FieldZone } from "@/components/chart/config/FieldZone";

const UNDERLINE_LIST = "h-auto w-full justify-start gap-4 rounded-none border-b bg-transparent p-0";
const UNDERLINE_TRIGGER = "rounded-none border-b-2 border-transparent px-0 pb-2 text-sm data-[state=active]:border-primary data-[state=active]:bg-transparent data-[state=active]:text-foreground data-[state=active]:shadow-none";

const isValidViz = (v: string | null): v is VizType => CHART_TYPES.some((t) => t.key === v);

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
  const [viz, setViz] = useState<VizType>(!id && isValidViz(typeParam) ? typeParam : "table");
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
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [pivotSort, setPivotSort] = useState<Record<string, "asc" | "desc">>({});
  const [tableState, setTableState] = useState<TableState>({});
  const [filters, setFilters] = useState<ChartFilter[]>([]);
  const [drillMenu, setDrillMenu] = useState<{ ctx: DrillContext; x: number; y: number } | null>(null);
  const [recordsFilters, setRecordsFilters] = useState<ChartFilter[] | null>(null);

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
    setCollapsed(new Set(p.collapsedRows ?? []));
    setPivotSort(p.pivotSort ?? {});
    setTableState(p.tableState ?? {});
    setFilters(p.spec?.filters ?? []);
    setSaveName(c.name);
    setSaveDesc(c.description);
  }, [existing.data]);

  // keep viz in sync with ?type for new charts (browser back/forward, deep links)
  useEffect(() => {
    if (!id && isValidViz(typeParam)) setViz(typeParam);
  }, [id, typeParam]);

  const columns = detail.data?.columns ?? [];
  const metrics = detail.data?.metrics ?? [];

  function specForRun(): QuerySpec {
    if (viz === "pivot_table") {
      const dims = [...pRows, ...pCols];
      const groups = [pRows, pCols].filter((g) => g.length > 0);
      return {
        metrics: pVals, dimensions: dims, columns: [], filters, orderby: [],
        row_limit: rowLimit, rollup_groups: groups,
      };
    }
    const agg = tableMetrics.length > 0;
    return {
      metrics: tableMetrics, dimensions: agg ? tableCols : [], columns: agg ? [] : tableCols,
      filters, orderby: [], row_limit: rowLimit, rollup_groups: [],
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

  function addFilter(f: ChartFilter) {
    setFilters((prev) => [...prev.filter((x) => x.col !== f.col), f]);
  }
  // Re-run whenever filters change, but only if a query is already in flight/done
  useEffect(() => { if (queryId) void run(); }, [filters]); // intentionally omits run/queryId to avoid infinite loop

  const mutations = useChartMutations(ws);
  const [saveOpen, setSaveOpen] = useState(false);
  const [saveName, setSaveName] = useState("");
  const [saveDesc, setSaveDesc] = useState("");

  function paramsForSave(): ChartParams {
    return viz === "pivot_table"
      ? { spec: specForRun(), rows: pRows, cols: pCols, metrics: pVals, showRowTotals, showColTotals, condense, columnSettings, conditionalFormatting, collapsedRows: [...collapsed], pivotSort }
      : { spec: specForRun(), columnSettings, conditionalFormatting, tableState };
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
    if (!id) setSearchParams({ type: v });
  }

  function toggleTableMetric(name: string) {
    setTableMetrics((prev) => prev.includes(name) ? prev.filter((m) => m !== name) : [...prev, name]);
  }
  function toggleTableCol(name: string) {
    setTableCols((prev) => prev.includes(name) ? prev.filter((c) => c !== name) : [...prev, name]);
  }

  function resetControls() {
    setTableMetrics([]); setTableCols([]); setPRows([]); setPCols([]); setPVals([]);
    setColumnSettings({}); setConditionalFormatting([]); setCollapsed(new Set()); setPivotSort({}); setTableState({}); setFilters([]); setQueryId(null);
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
                <FieldZone label="Rows" selected={pRows} onChange={setPRows}
                  options={columns.map((c) => c.column_name).filter((n) => !pCols.includes(n))} />
                <FieldZone label="Columns" selected={pCols} onChange={setPCols}
                  options={columns.map((c) => c.column_name).filter((n) => !pRows.includes(n))} />
                <FieldZone label="Values" selected={pVals} onChange={setPVals}
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
                <div className="flex items-center gap-2">
                  <Checkbox id="show-row-totals" checked={showRowTotals} onCheckedChange={(v) => setShowRowTotals(!!v)} />
                  <Label htmlFor="show-row-totals" className="cursor-pointer text-sm font-normal">Row totals</Label>
                </div>
                <div className="flex items-center gap-2">
                  <Checkbox id="show-col-totals" checked={showColTotals} onCheckedChange={(v) => setShowColTotals(!!v)} />
                  <Label htmlFor="show-col-totals" className="cursor-pointer text-sm font-normal">Column totals</Label>
                </div>
                <div className="flex items-center gap-2">
                  <Checkbox id="condense-totals" checked={condense} onCheckedChange={(v) => setCondense(!!v)} />
                  <Label htmlFor="condense-totals" className="cursor-pointer text-sm font-normal">Condense duplicate totals</Label>
                </div>
              </div>
            )}

            {resultColumns.length === 0 ? (
              <p className="text-xs text-muted-foreground">Run the chart to configure column formatting.</p>
            ) : (
              <>
                {viz === "table" && (
                  <div className="space-y-1">
                    <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Columns</div>
                    <TableColumnsSection columns={resultColumns} value={tableState} onChange={setTableState} />
                  </div>
                )}
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

        {filters.length > 0 && (
          <FilterChips filters={filters} onRemove={(i) => setFilters((prev) => prev.filter((_, j) => j !== i))} />
        )}
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
              showRowTotals, showColTotals, condense, collapsed,
              sortDims: pivotSort,
            }}
            columnSettings={columnSettings}
            conditionalFormatting={conditionalFormatting}
            metricTypes={metricTypes}
            onToggleCollapse={(path) =>
              setCollapsed((prev) => {
                const next = new Set(prev);
                if (next.has(path)) next.delete(path); else next.add(path);
                return next;
              })
            }
            onCycleSort={(dim) =>
              setPivotSort((prev) => {
                const cur = prev[dim];
                const next = { ...prev };
                if (cur === undefined) next[dim] = "asc";
                else if (cur === "asc") next[dim] = "desc";
                else delete next[dim];
                return next;
              })
            }
            onCellDrill={(ctx, x, y) => setDrillMenu({ ctx, x, y })}
          />
        )}
        {finished && results.data && viz !== "pivot_table" && (
          <ChartTable columns={results.data.columns} rows={results.data.rows}
            columnSettings={columnSettings} conditionalFormatting={conditionalFormatting}
            tableState={tableState} onTableStateChange={setTableState}
            dimensionColumns={tableMetrics.length > 0 ? tableCols : []}
            onCellDrill={(ctx, x, y) => setDrillMenu({ ctx, x, y })} />
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

      {drillMenu && (
        <DrillMenu
          ctx={drillMenu.ctx} x={drillMenu.x} y={drillMenu.y}
          onFilterBy={addFilter}
          onSeeRecords={(fs) => setRecordsFilters(fs)}
          onClose={() => setDrillMenu(null)}
        />
      )}
      <DrillRecordsDialog
        open={recordsFilters !== null}
        onOpenChange={(o) => { if (!o) setRecordsFilters(null); }}
        ws={ws} datasetId={datasetId}
        columns={(detail.data?.columns ?? []).map((c) => ({ name: c.column_name, type: c.data_type ?? "varchar" }))}
        filters={recordsFilters ?? []}
        rowLimit={rowLimit}
      />
    </div>
  );
}
