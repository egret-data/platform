import { useEffect, useState } from "react";
import { useParams, Link, useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { useDatasets, useDataset } from "@/api/datasets";
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams,
} from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";
import { ChartTable } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/pivot/PivotTable";
import { SettingsSidebar } from "@/components/chart/settings/SettingsSidebar";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
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

export default function ChartExplore() {
  const ws = useParams().workspace!;
  const { id } = useParams(); // present when editing a saved chart
  const navigate = useNavigate();
  const datasets = useDatasets(ws);
  const existing = useChart(ws, id ?? null);

  const [datasetId, setDatasetId] = useState<string | null>(null);
  const detail = useDataset(ws, datasetId);
  const [viz, setViz] = useState<VizType>("table");

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
  const [settingsOpen, setSettingsOpen] = useState(false);

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
        metrics: pVals,
        dimensions: dims,
        columns: [],
        filters: [],
        orderby: [],
        row_limit: rowLimit,
        rollup_groups: groups,
      };
    }
    const agg = tableMetrics.length > 0;
    return {
      metrics: tableMetrics,
      dimensions: agg ? tableCols : [],
      columns: agg ? [] : tableCols,
      filters: [],
      orderby: [],
      row_limit: rowLimit,
      rollup_groups: [],
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

  const isRunning = status.data?.status === "queued" || status.data?.status === "running";

  return (
    <div className="flex h-full min-h-0 gap-0">
      {/* Left rail */}
      <div className="w-64 shrink-0 overflow-y-auto border-r p-4 space-y-4">
        <div>
          <Link className="text-xs text-muted-foreground hover:underline" to={`/explore/${ws}/charts`}>
            ← Charts
          </Link>
        </div>

        {/* Dataset picker */}
        <div className="space-y-1">
          <Label className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Dataset</Label>
          <Select value={datasetId ?? ""} onValueChange={(v) => { setDatasetId(v || null); setTableMetrics([]); setTableCols([]); setPRows([]); setPCols([]); setPVals([]); setColumnSettings({}); setConditionalFormatting([]); setQueryId(null); }}>
            <SelectTrigger className="h-8 text-sm">
              <SelectValue placeholder="Pick a dataset…" />
            </SelectTrigger>
            <SelectContent>
              {(datasets.data ?? []).map((d) => (
                <SelectItem key={d.id} value={d.id}>{d.name}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        {/* Viz toggle */}
        <div className="space-y-1">
          <Label className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Visualization</Label>
          <div className="flex gap-1">
            <Button
              size="sm"
              variant={viz === "table" ? "default" : "outline"}
              className="flex-1 text-xs"
              onClick={() => changeViz("table")}
            >
              Table
            </Button>
            <Button
              size="sm"
              variant={viz === "pivot_table" ? "default" : "outline"}
              className="flex-1 text-xs"
              onClick={() => changeViz("pivot_table")}
            >
              Pivot
            </Button>
          </div>
        </div>

        {/* Controls: table viz */}
        {viz === "table" && (
          <>
            <div className="space-y-1">
              <Label className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Metrics</Label>
              <p className="text-xs text-muted-foreground">Pick metrics for an aggregated table, or just columns for raw rows.</p>
              <div className="space-y-1 pt-1">
                {metrics.length === 0 && <p className="text-xs text-muted-foreground italic">No metrics on this dataset.</p>}
                {metrics.map((m) => (
                  <div key={m.metric_name} className="flex items-center gap-2">
                    <Checkbox
                      id={`metric-${m.metric_name}`}
                      checked={tableMetrics.includes(m.metric_name)}
                      onCheckedChange={() => toggleTableMetric(m.metric_name)}
                    />
                    <Label htmlFor={`metric-${m.metric_name}`} className="text-sm font-normal cursor-pointer">
                      {m.metric_name}
                    </Label>
                  </div>
                ))}
              </div>
            </div>

            <div className="space-y-1">
              <Label className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Columns</Label>
              <div className="space-y-1 pt-1">
                {columns.length === 0 && <p className="text-xs text-muted-foreground italic">Select a dataset to see columns.</p>}
                {columns.map((col) => (
                  <div key={col.column_name} className="flex items-center gap-2">
                    <Checkbox
                      id={`col-${col.column_name}`}
                      checked={tableCols.includes(col.column_name)}
                      onCheckedChange={() => toggleTableCol(col.column_name)}
                    />
                    <Label htmlFor={`col-${col.column_name}`} className="text-sm font-normal cursor-pointer">
                      {col.column_name}
                    </Label>
                  </div>
                ))}
              </div>
            </div>
          </>
        )}

        {/* Controls: pivot viz */}
        {viz === "pivot_table" && (
          <>
            <OrderedPicker label="Rows" selected={pRows} onChange={setPRows}
              options={columns.map((c) => c.column_name).filter((n) => !pCols.includes(n))} />
            <OrderedPicker label="Columns" selected={pCols} onChange={setPCols}
              options={columns.map((c) => c.column_name).filter((n) => !pRows.includes(n))} />
            <OrderedPicker label="Values" selected={pVals} onChange={setPVals}
              options={metrics.map((m) => m.metric_name)} />
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={showRowTotals} onCheckedChange={(v) => setShowRowTotals(!!v)} /> Row totals
            </label>
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={showColTotals} onCheckedChange={(v) => setShowColTotals(!!v)} /> Column totals
            </label>
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={condense} onCheckedChange={(v) => setCondense(!!v)} /> Condense duplicate totals
            </label>
          </>
        )}

        {/* Row limit */}
        <div className="space-y-1">
          <Label htmlFor="row-limit" className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Row limit</Label>
          <Input
            id="row-limit"
            type="number"
            min={1}
            max={10000}
            value={rowLimit}
            onChange={(e) => {
              const n = Number(e.target.value);
              setRowLimit(Number.isNaN(n) ? 0 : n);
            }}
            className="h-8 text-sm"
          />
        </div>
      </div>

      {/* Main area */}
      <div className="flex-1 overflow-auto p-4 space-y-3">
        {/* Action bar */}
        <div className="flex items-center gap-2">
          <Button
            onClick={run}
            disabled={build.isPending || submit.isPending}
            size="sm"
          >
            {(build.isPending || submit.isPending) ? "Building…" : "Run"}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setSaveOpen(true)}
          >
            {id ? "Update" : "Save"}
          </Button>
          <Button size="sm" variant="outline" onClick={() => setSettingsOpen((o) => !o)}>Settings</Button>
          {build.data?.sql && (
            <details className="ml-auto text-xs text-muted-foreground">
              <summary className="cursor-pointer select-none hover:text-foreground">View SQL</summary>
              <pre className="mt-1 max-h-40 overflow-auto rounded bg-muted p-2 text-xs">{build.data.sql}</pre>
            </details>
          )}
        </div>

        {/* Preview + Settings sidebar */}
        <div className="flex gap-3">
          <div className="flex-1 min-w-0">
            {status.data?.status === "failed" && (
              <p className="rounded bg-destructive/10 p-3 text-sm text-destructive">{status.data.error}</p>
            )}

            {isRunning && (
              <p className="text-sm text-muted-foreground">Running…</p>
            )}

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
              <ChartTable
                columns={results.data.columns}
                rows={results.data.rows}
                columnSettings={columnSettings}
                conditionalFormatting={conditionalFormatting}
              />
            )}

            {!queryId && !build.isPending && !submit.isPending && (
              <p className="text-sm text-muted-foreground">Configure the controls on the left and click Run.</p>
            )}
          </div>

          {settingsOpen && (
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
        </div>
      </div>

      {/* Save dialog */}
      <Dialog open={saveOpen} onOpenChange={setSaveOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{id ? "Update chart" : "Save chart"}</DialogTitle>
          </DialogHeader>
          <div className="space-y-3 py-2">
            <div className="space-y-1">
              <Label htmlFor="chart-name">Name</Label>
              <Input
                id="chart-name"
                value={saveName}
                onChange={(e) => setSaveName(e.target.value)}
                placeholder="Chart name…"
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="chart-desc">Description</Label>
              <Input
                id="chart-desc"
                value={saveDesc}
                onChange={(e) => setSaveDesc(e.target.value)}
                placeholder="Optional description…"
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setSaveOpen(false)}>Cancel</Button>
            <Button onClick={save} disabled={mutations.create.isPending || mutations.update.isPending}>
              {id ? "Update" : "Save"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
