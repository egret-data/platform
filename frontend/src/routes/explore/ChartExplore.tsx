import { useEffect, useState } from "react";
import { useParams, Link } from "react-router-dom";
import { toast } from "sonner";
import { useDatasets, useDataset } from "@/api/datasets";
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams,
} from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";
import { ChartTable } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/PivotTable";
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

export default function ChartExplore() {
  const ws = useParams().workspace!;
  const { id } = useParams(); // present when editing a saved chart
  const datasets = useDatasets(ws);
  const existing = useChart(ws, id ?? null);

  const [datasetId, setDatasetId] = useState<string | null>(null);
  const detail = useDataset(ws, datasetId);
  const [viz, setViz] = useState<VizType>("table");

  // table-mode controls
  const [tableMetrics, setTableMetrics] = useState<string[]>([]);
  const [tableCols, setTableCols] = useState<string[]>([]);
  // pivot-mode controls (single-dim each)
  const [pivotRow, setPivotRow] = useState<string>("");
  const [pivotCol, setPivotCol] = useState<string>("");
  const [pivotMetric, setPivotMetric] = useState<string>("");
  const [rowLimit, setRowLimit] = useState(1000);

  // hydrate from a saved chart once it loads
  useEffect(() => {
    const c = existing.data;
    if (!c) return;
    setDatasetId(c.dataset_id);
    setViz(c.viz_type);
    const p = c.params;
    if (c.viz_type === "pivot_table") {
      setPivotRow(p.rows?.[0] ?? "");
      setPivotCol(p.cols?.[0] ?? "");
      setPivotMetric(p.metric ?? "");
    } else {
      setTableMetrics(p.spec.metrics ?? []);
      setTableCols([...(p.spec.dimensions ?? []), ...(p.spec.columns ?? [])]);
    }
    setRowLimit(p.spec.row_limit ?? 1000);
  }, [existing.data]);

  const columns = detail.data?.columns ?? [];
  const metrics = detail.data?.metrics ?? [];

  function specForRun(): QuerySpec {
    if (viz === "pivot_table") {
      return {
        metrics: pivotMetric ? [pivotMetric] : [],
        dimensions: [pivotRow, pivotCol].filter(Boolean),
        columns: [],
        filters: [],
        orderby: [],
        row_limit: rowLimit,
        rollup: true,
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
      rollup: false,
    };
  }

  const build = useBuildChartData(ws);
  const submit = useSubmitQuery(ws);
  const [queryId, setQueryId] = useState<string | null>(null);
  const status = useQueryStatus(ws, queryId);
  const finished = status.data?.status === "finished";
  const results = useResults(ws, queryId, 0, finished);

  async function run() {
    if (!datasetId) { toast.error("Pick a dataset first"); return; }
    if (viz === "pivot_table" && (!pivotRow || !pivotMetric)) {
      toast.error("Pivot needs a row dimension and a metric"); return;
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
  useEffect(() => {
    if (existing.data) { setSaveName(existing.data.name); setSaveDesc(existing.data.description); }
  }, [existing.data]);

  function paramsForSave(): ChartParams {
    return viz === "pivot_table"
      ? { spec: specForRun(), rows: [pivotRow], cols: pivotCol ? [pivotCol] : [], metric: pivotMetric }
      : { spec: specForRun() };
  }

  async function save() {
    if (!datasetId) { toast.error("Pick a dataset"); return; }
    if (!saveName.trim()) { toast.error("Name required"); return; }
    try {
      if (id) {
        await mutations.update.mutateAsync({ id, name: saveName.trim(), description: saveDesc, viz_type: viz, params: paramsForSave() });
      } else {
        await mutations.create.mutateAsync({ dataset_id: datasetId, name: saveName.trim(), description: saveDesc, viz_type: viz, params: paramsForSave() });
      }
      setSaveOpen(false);
      toast.success("Saved");
    } catch (e) { toast.error(String(e)); }
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
          <Select value={datasetId ?? ""} onValueChange={(v) => { setDatasetId(v || null); setTableMetrics([]); setTableCols([]); setPivotRow(""); setPivotCol(""); setPivotMetric(""); }}>
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
              onClick={() => setViz("table")}
            >
              Table
            </Button>
            <Button
              size="sm"
              variant={viz === "pivot_table" ? "default" : "outline"}
              className="flex-1 text-xs"
              onClick={() => setViz("pivot_table")}
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
            <p className="text-xs text-muted-foreground">Pivot supports one row + one column dimension.</p>

            <div className="space-y-1">
              <Label className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Row dimension</Label>
              <Select value={pivotRow} onValueChange={setPivotRow}>
                <SelectTrigger className="h-8 text-sm">
                  <SelectValue placeholder="Pick a column…" />
                </SelectTrigger>
                <SelectContent>
                  {columns.map((col) => (
                    <SelectItem key={col.column_name} value={col.column_name}>{col.column_name}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1">
              <Label className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Column dimension</Label>
              <Select value={pivotCol} onValueChange={setPivotCol}>
                <SelectTrigger className="h-8 text-sm">
                  <SelectValue placeholder="None (optional)" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="">None</SelectItem>
                  {columns.map((col) => (
                    <SelectItem key={col.column_name} value={col.column_name}>{col.column_name}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1">
              <Label className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Metric</Label>
              <Select value={pivotMetric} onValueChange={setPivotMetric}>
                <SelectTrigger className="h-8 text-sm">
                  <SelectValue placeholder="Pick a metric…" />
                </SelectTrigger>
                <SelectContent>
                  {metrics.map((m) => (
                    <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </>
        )}

        {/* Row limit */}
        <div className="space-y-1">
          <Label htmlFor="row-limit" className="text-xs font-medium text-muted-foreground uppercase tracking-wide">Row limit</Label>
          <Input
            id="row-limit"
            type="number"
            min={1}
            max={100000}
            value={rowLimit}
            onChange={(e) => setRowLimit(Number(e.target.value) || 1000)}
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
          {build.data?.sql && (
            <details className="ml-auto text-xs text-muted-foreground">
              <summary className="cursor-pointer select-none hover:text-foreground">View SQL</summary>
              <pre className="mt-1 max-h-40 overflow-auto rounded bg-muted p-2 text-xs">{build.data.sql}</pre>
            </details>
          )}
        </div>

        {/* Preview */}
        {status.data?.status === "failed" && (
          <p className="rounded bg-destructive/10 p-3 text-sm text-destructive">{status.data.error}</p>
        )}

        {isRunning && (
          <p className="text-sm text-muted-foreground">Running…</p>
        )}

        {finished && results.data && (
          viz === "pivot_table" ? (
            <PivotTable
              columns={results.data.columns.map((c) => c.name)}
              rows={results.data.rows}
              rowDims={pivotRow ? [pivotRow] : []}
              colDims={pivotCol ? [pivotCol] : []}
              metric={pivotMetric}
            />
          ) : (
            <ChartTable columns={results.data.columns} rows={results.data.rows} />
          )
        )}

        {!queryId && !build.isPending && !submit.isPending && (
          <p className="text-sm text-muted-foreground">Configure the controls on the left and click Run.</p>
        )}
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
