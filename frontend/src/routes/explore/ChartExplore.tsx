import { useEffect, useState, type ReactNode } from "react";
import { useParams, Link, useNavigate, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { useDatasets, useDataset } from "@/api/datasets";
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams, type TableState, type ChartFilter,
  type CartesianConfig, type PieConfig, type ScalarConfig, type ProgressConfig, type GaugeConfig, type FunnelConfig, isCartesianViz,
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
import { EChartsRenderer } from "@/components/chart/echarts/EChartsRenderer";
import { buildCartesianOption } from "@/components/chart/echarts/options/cartesian";
import { buildPieOption } from "@/components/chart/echarts/options/pie";
import { BigNumber } from "@/components/chart/BigNumber";
import { ProgressChart } from "@/components/chart/ProgressChart";
import { buildGaugeOption } from "@/components/chart/echarts/options/gauge";
import { buildFunnelOption } from "@/components/chart/echarts/options/funnel";
import { CartesianDisplaySection } from "@/components/chart/settings/CartesianDisplaySection";
import { PieDisplaySection } from "@/components/chart/settings/PieDisplaySection";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
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

function SectionLabel({ children }: { children: ReactNode }) {
  return <div className="border-b pb-1 text-xs font-medium uppercase tracking-wide text-muted-foreground">{children}</div>;
}

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
  const [cartesian, setCartesian] = useState<CartesianConfig>({ yMetrics: [] });
  const [pie, setPie] = useState<PieConfig>({});
  const [scalar, setScalar] = useState<ScalarConfig>({});
  const [progress, setProgress] = useState<ProgressConfig>({});
  const [gauge, setGauge] = useState<GaugeConfig>({});
  const [funnel, setFunnel] = useState<FunnelConfig>({});
  const patchCartesian = (p: Partial<CartesianConfig>) => setCartesian((c) => ({ ...c, ...p }));
  const patchPie = (p: Partial<PieConfig>) => setPie((c) => ({ ...c, ...p }));
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
    } else if (isCartesianViz(c.viz_type)) {
      setCartesian(p.cartesian ?? { yMetrics: p.spec?.metrics ?? [] });
    } else if (c.viz_type === "pie") {
      setPie(p.pie ?? {});
    } else if (c.viz_type === "scalar") {
      setScalar(p.scalar ?? { metric: p.spec?.metrics?.[0] });
    } else if (c.viz_type === "progress") {
      setProgress(p.progress ?? { metric: p.spec?.metrics?.[0] });
    } else if (c.viz_type === "gauge") {
      setGauge(p.gauge ?? { metric: p.spec?.metrics?.[0] });
    } else if (c.viz_type === "funnel") {
      setFunnel(p.funnel ?? { dimension: p.spec?.dimensions?.[0], metric: p.spec?.metrics?.[0] });
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
    if (isCartesianViz(viz)) {
      const dims = [cartesian.xAxis, cartesian.series].filter((d): d is string => !!d);
      return { metrics: cartesian.yMetrics, dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [] };
    }
    if (viz === "pie") {
      const dims = pie.dimension ? [pie.dimension] : [];
      return { metrics: pie.metric ? [pie.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [] };
    }
    if (viz === "scalar") {
      return { metrics: scalar.metric ? [scalar.metric] : [], dimensions: [], columns: [], filters, orderby: [], row_limit: 1, rollup_groups: [] };
    }
    if (viz === "progress") {
      return { metrics: progress.metric ? [progress.metric] : [], dimensions: [], columns: [], filters, orderby: [], row_limit: 1, rollup_groups: [] };
    }
    if (viz === "gauge") {
      return { metrics: gauge.metric ? [gauge.metric] : [], dimensions: [], columns: [], filters, orderby: [], row_limit: 1, rollup_groups: [] };
    }
    if (viz === "funnel") {
      return { metrics: funnel.metric ? [funnel.metric] : [], dimensions: funnel.dimension ? [funnel.dimension] : [], columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [] };
    }
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
  // In a pivot, only the value (metric) cells are formatted/colored — row/column dimension cells are
  // headers and never styled. So the Format/Conditional pickers should offer only the value columns.
  const styleColumns =
    viz === "pivot_table" ? resultColumns.filter((c) => pVals.includes(c.name))
    : isCartesianViz(viz) ? resultColumns.filter((c) => cartesian.yMetrics.includes(c.name))
    : viz === "scalar" ? resultColumns.filter((c) => c.name === scalar.metric)
    : viz === "pie" ? []   // pie value formatting handled in its tooltip; no column formatting UI
    : resultColumns;
  const metricTypes = Object.fromEntries((detail.data?.metrics ?? []).map((m) => [m.metric_name, m.data_type ?? "double"]));

  async function run() {
    setQueryId(null);
    if (!datasetId) { toast.error("Pick a dataset first"); return; }
    if (viz === "pivot_table" && (pRows.length === 0 || pVals.length === 0)) {
      toast.error("Pivot needs at least one row dimension and one value"); return;
    }
    if (isCartesianViz(viz) && (!cartesian.xAxis || cartesian.yMetrics.length === 0)) {
      toast.error("This chart needs an X-axis and at least one metric"); return;
    }
    if (viz === "pie" && (!pie.dimension || !pie.metric)) {
      toast.error("Pie needs a dimension and a metric"); return;
    }
    if (viz === "scalar" && !scalar.metric) {
      toast.error("Big number needs a metric"); return;
    }
    if (viz === "progress" && !progress.metric) {
      toast.error("Progress needs a metric"); return;
    }
    if (viz === "gauge" && !gauge.metric) {
      toast.error("Gauge needs a metric"); return;
    }
    if (viz === "funnel" && (!funnel.dimension || !funnel.metric)) {
      toast.error("Funnel needs a dimension and a metric"); return;
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
    if (viz === "pivot_table") {
      return { spec: specForRun(), rows: pRows, cols: pCols, metrics: pVals, showRowTotals, showColTotals, condense, columnSettings, conditionalFormatting, collapsedRows: [...collapsed], pivotSort };
    }
    if (isCartesianViz(viz)) return { spec: specForRun(), cartesian, columnSettings };
    if (viz === "pie") return { spec: specForRun(), pie };
    if (viz === "scalar") return { spec: specForRun(), scalar, columnSettings };
    if (viz === "progress") return { spec: specForRun(), progress, columnSettings };
    if (viz === "gauge") return { spec: specForRun(), gauge, columnSettings };
    if (viz === "funnel") return { spec: specForRun(), funnel, columnSettings };
    return { spec: specForRun(), columnSettings, conditionalFormatting, tableState };
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
    setCartesian({ yMetrics: [] }); setPie({}); setScalar({}); setProgress({}); setGauge({}); setFunnel({});
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

            {isCartesianViz(viz) && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">X-axis</Label>
                  <Select value={cartesian.xAxis ?? ""} onValueChange={(v) => patchCartesian({ xAxis: v || undefined })}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <FieldZone label="Y-axis" selected={cartesian.yMetrics} onChange={(v) => patchCartesian({ yMetrics: v })}
                  options={metrics.map((m) => m.metric_name)} />
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Series</Label>
                  <Select value={cartesian.series ?? "__none"} onValueChange={(v) => patchCartesian({ series: v === "__none" ? undefined : v })}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__none">None</SelectItem>
                      {columns.filter((c) => c.column_name !== cartesian.xAxis).map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                  {cartesian.series && cartesian.yMetrics.length > 1 && (
                    <p className="text-xs text-muted-foreground">With a series, only the first metric is plotted.</p>
                  )}
                </div>
              </>
            )}

            {viz === "pie" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Dimension</Label>
                  <Select value={pie.dimension ?? ""} onValueChange={(v) => patchPie({ dimension: v || undefined })}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric</Label>
                  <Select value={pie.metric ?? ""} onValueChange={(v) => patchPie({ metric: v || undefined })}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
                    <SelectContent>
                      {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
              </>
            )}

            {viz === "scalar" && (
              <div className="space-y-1">
                <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric</Label>
                <Select value={scalar.metric ?? ""} onValueChange={(v) => setScalar({ metric: v || undefined })}>
                  <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
                  <SelectContent>
                    {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
                  </SelectContent>
                </Select>
              </div>
            )}

            {viz === "progress" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric</Label>
                  <Select value={progress.metric ?? ""} onValueChange={(v) => setProgress((p) => ({ ...p, metric: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
                    <SelectContent>
                      {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1">
                  <Label htmlFor="progress-goal" className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Goal</Label>
                  <Input id="progress-goal" type="number" value={progress.goal ?? ""}
                    onChange={(e) => { const n = Number(e.target.value); setProgress((p) => ({ ...p, goal: e.target.value === "" || Number.isNaN(n) ? undefined : n })); }}
                    className="h-8 text-sm" placeholder="Target value" />
                </div>
              </>
            )}

            {viz === "gauge" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric</Label>
                  <Select value={gauge.metric ?? ""} onValueChange={(v) => setGauge((g) => ({ ...g, metric: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
                    <SelectContent>
                      {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid grid-cols-2 gap-2">
                  <div className="space-y-1">
                    <Label htmlFor="gauge-min" className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Min</Label>
                    <Input id="gauge-min" type="number" value={gauge.min ?? ""}
                      onChange={(e) => { const n = Number(e.target.value); setGauge((g) => ({ ...g, min: e.target.value === "" || Number.isNaN(n) ? undefined : n })); }}
                      className="h-8 text-sm" placeholder="0" />
                  </div>
                  <div className="space-y-1">
                    <Label htmlFor="gauge-max" className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Max</Label>
                    <Input id="gauge-max" type="number" value={gauge.max ?? ""}
                      onChange={(e) => { const n = Number(e.target.value); setGauge((g) => ({ ...g, max: e.target.value === "" || Number.isNaN(n) ? undefined : n })); }}
                      className="h-8 text-sm" placeholder="auto" />
                  </div>
                </div>
              </>
            )}

            {viz === "funnel" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Stage (dimension)</Label>
                  <Select value={funnel.dimension ?? ""} onValueChange={(v) => setFunnel((f) => ({ ...f, dimension: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric</Label>
                  <Select value={funnel.metric ?? ""} onValueChange={(v) => setFunnel((f) => ({ ...f, metric: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
                    <SelectContent>
                      {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
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
              <div className="space-y-1">
                <SectionLabel>Totals</SectionLabel>
                <div className="space-y-2.5 pt-1.5">
                  <div className="flex items-center justify-between">
                    <Label htmlFor="show-row-totals" className="text-sm font-normal">Row totals</Label>
                    <Switch id="show-row-totals" checked={showRowTotals} onCheckedChange={(v) => setShowRowTotals(v)} />
                  </div>
                  <div className="flex items-center justify-between">
                    <Label htmlFor="show-col-totals" className="text-sm font-normal">Column totals</Label>
                    <Switch id="show-col-totals" checked={showColTotals} onCheckedChange={(v) => setShowColTotals(v)} />
                  </div>
                  <div className="flex items-center justify-between">
                    <Label htmlFor="condense-totals" className="text-sm font-normal">Condense duplicate totals</Label>
                    <Switch id="condense-totals" checked={condense} onCheckedChange={(v) => setCondense(v)} />
                  </div>
                </div>
              </div>
            )}

            {isCartesianViz(viz) && (
              <div className="space-y-1">
                <SectionLabel>Options</SectionLabel>
                <CartesianDisplaySection kind={viz} config={cartesian} onChange={patchCartesian} />
              </div>
            )}
            {viz === "pie" && (
              <div className="space-y-1">
                <SectionLabel>Options</SectionLabel>
                <PieDisplaySection config={pie} onChange={patchPie} />
              </div>
            )}

            {resultColumns.length === 0 ? (
              <p className="text-xs text-muted-foreground">Run the chart to configure column formatting.</p>
            ) : (
              <>
                {viz === "table" && (
                  <div className="space-y-1.5">
                    <SectionLabel>Columns</SectionLabel>
                    <TableColumnsSection columns={resultColumns} value={tableState} onChange={setTableState} />
                  </div>
                )}
                {styleColumns.length > 0 && (
                  <div className="space-y-1.5">
                    <SectionLabel>Formatting</SectionLabel>
                    <FormatSection columns={styleColumns} value={columnSettings} onChange={setColumnSettings} />
                  </div>
                )}
                {(viz === "table" || viz === "pivot_table") && (
                  <div className="space-y-1.5">
                    <SectionLabel>Conditional formatting</SectionLabel>
                    <ConditionalFormattingSection columns={styleColumns} rules={conditionalFormatting} isPivot={viz === "pivot_table"} onChange={setConditionalFormatting} />
                  </div>
                )}
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
        {finished && results.data && viz === "table" && (
          <ChartTable columns={results.data.columns} rows={results.data.rows}
            columnSettings={columnSettings} conditionalFormatting={conditionalFormatting}
            tableState={tableState} onTableStateChange={setTableState}
            dimensionColumns={tableMetrics.length > 0 ? tableCols : []}
            onCellDrill={(ctx, x, y) => setDrillMenu({ ctx, x, y })} />
        )}
        {finished && results.data && isCartesianViz(viz) && (
          <div className="h-[60vh] rounded-md border p-2">
            {results.data.rows.length === 0
              ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
              : <EChartsRenderer option={buildCartesianOption(results.data.columns, results.data.rows, {
                  kind: viz, xAxis: cartesian.xAxis, yMetrics: cartesian.yMetrics, series: cartesian.series,
                  stacked: cartesian.stacked, horizontal: cartesian.horizontal, smooth: cartesian.smooth,
                  showLabels: cartesian.showLabels, showLegend: cartesian.showLegend,
                  xLabel: cartesian.xLabel, yLabel: cartesian.yLabel, seriesTypes: cartesian.seriesTypes,
                  rightAxisMetrics: cartesian.rightAxisMetrics, logScale: cartesian.logScale,
                  normalized: cartesian.normalized, goalValue: cartesian.goalValue, goalLabel: cartesian.goalLabel,
                  trendLine: cartesian.trendLine, connectNulls: cartesian.connectNulls, step: cartesian.step,
                  columnSettings, metricTypes,
                })} />}
          </div>
        )}
        {finished && results.data && viz === "pie" && (
          <div className="h-[60vh] rounded-md border p-2">
            {results.data.rows.length === 0
              ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
              : <EChartsRenderer option={buildPieOption(results.data.columns, results.data.rows, {
                  dimension: pie.dimension, metric: pie.metric, donut: pie.donut,
                  showLegend: pie.showLegend, showLabels: pie.showLabels,
                  sliceThreshold: pie.sliceThreshold, sortRows: pie.sortRows,
                  showTotal: pie.showTotal, percentVisibility: pie.percentVisibility,
                  columnSettings, metricTypes,
                })} />}
          </div>
        )}
        {finished && results.data && viz === "scalar" && (
          <div className="rounded-md border">
            <BigNumber columns={results.data.columns} rows={results.data.rows} metric={scalar.metric} format={scalar.metric ? columnSettings[scalar.metric] : undefined} />
          </div>
        )}
        {finished && results.data && viz === "progress" && (
          <div className="rounded-md border">
            <ProgressChart columns={results.data.columns} rows={results.data.rows} metric={progress.metric} goal={progress.goal}
              format={progress.metric ? columnSettings[progress.metric] : undefined} />
          </div>
        )}
        {finished && results.data && viz === "gauge" && (
          <div className="h-[50vh] rounded-md border p-2">
            <EChartsRenderer option={buildGaugeOption(results.data.columns, results.data.rows, {
              metric: gauge.metric, min: gauge.min, max: gauge.max, columnSettings, metricTypes,
            })} />
          </div>
        )}
        {finished && results.data && viz === "funnel" && (
          <div className="h-[60vh] rounded-md border p-2">
            {results.data.rows.length === 0
              ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
              : <EChartsRenderer option={buildFunnelOption(results.data.columns, results.data.rows, {
                  dimension: funnel.dimension, metric: funnel.metric, columnSettings, metricTypes,
                })} />}
          </div>
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
