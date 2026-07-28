import { useEffect, useMemo, useRef, useState } from "react";
import { useParams, Link, useNavigate, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { useDatasets, useDataset } from "@/api/datasets";
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams, type TableState, type ChartFilter,
  type PieConfig, type ScalarConfig, type ProgressConfig, type GaugeConfig, type FunnelConfig, type WaterfallConfig, type SmartScalarConfig, type BoxplotConfig, type SankeyConfig,
  type AnyChartParams, type VizSettings, isParamsV2,
} from "@/api/charts";
import { useSpecData } from "@/api/useSpecData";
import { ChartTable } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/pivot/PivotTable";
import { FilterChips } from "@/components/chart/FilterChips";
import { DrillMenu } from "@/components/chart/DrillMenu";
import { DrillRecordsDialog } from "@/components/chart/DrillRecordsDialog";
import type { DrillContext } from "@/components/chart/drill";
import { ChartTypeGallery } from "@/components/chart/ChartTypeGallery";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { ChartSettingsSections } from "@/components/chart/settings/ChartSettingsSections";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { ChartErrorBoundary } from "@/components/chart/ChartErrorBoundary";
import { ChartExportMenu } from "@/components/chart/ChartExportMenu";
import { StudioToolbar } from "@/components/chart/StudioToolbar";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import { parseFormat } from "@/components/dataset/FormatPicker";
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
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { pruneUnits } from "@/components/chart/temporal-units";
import { TemporalUnitSelect } from "@/components/chart/TemporalUnitSelect";
import { missingFields } from "@/components/chart/requiredFields";
import { useChartRun } from "@/components/chart/useChartRun";
import { getDescriptor } from "@/components/chart/viz/registry";
import { pivotDescriptor } from "@/components/chart/viz/pivot";
import { tableDescriptor } from "@/components/chart/viz/table";
import type { VizDescriptor } from "@/components/chart/viz/types";
import { toRenderParams } from "@/components/chart/viz/renderParams";
import { StudioPanel } from "@/components/chart/StudioPanel";
import { ChartEmptyState, ChartLoadingState, ChartErrorState } from "@/components/chart/ChartPreviewStates";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";

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

  // New chart may arrive with a preselected dataset (?dataset=<id>) from the
  // dataset detail page's "Create chart" action; saved charts hydrate below.
  const [datasetId, setDatasetId] = useState<string | null>(() => (!id ? searchParams.get("dataset") : null));
  const detail = useDataset(ws, datasetId);
  const [viz, setViz] = useState<VizType>(!id && isValidViz(typeParam) ? typeParam : "table");
  const [vizPickerOpen, setVizPickerOpen] = useState(false);

  const [temporalUnits, setTemporalUnits] = useState<Record<string, string>>({});
  const [rowLimit, setRowLimit] = useState(1000);
  const [columnSettings, setColumnSettings] = useState<Record<string, ColumnFormat>>({});
  const [conditionalFormatting, setConditionalFormatting] = useState<Rule[]>([]);
  const [tableState, setTableState] = useState<TableState>({});
  const [pie, setPie] = useState<PieConfig>({});
  const [scalar, setScalar] = useState<ScalarConfig>({});
  const [progress, setProgress] = useState<ProgressConfig>({});
  const [gauge, setGauge] = useState<GaugeConfig>({});
  const [funnel, setFunnel] = useState<FunnelConfig>({});
  const [waterfall, setWaterfall] = useState<WaterfallConfig>({});
  const [smart, setSmart] = useState<SmartScalarConfig>({});
  const [box, setBox] = useState<BoxplotConfig>({});
  const [sankey, setSankey] = useState<SankeyConfig>({});
  const patchPie = (p: Partial<PieConfig>) => setPie((c) => ({ ...c, ...p }));
  // The descriptor for the active viz, or undefined while it is still on the bridge —
  // i.e. still driven by the inline branches below. PR3 empties that bridge out.
  const descriptor = getDescriptor(viz);
  // One opaque config for whichever viz is active. Only its descriptor reads inside it;
  // null means "not seeded yet", so the descriptor's own default applies.
  const [descState, setDescState] = useState<unknown>(null);
  const descConfig = (descState ?? descriptor?.defaultConfig) as never;
  const patchDescConfig = ((p: object) =>
    setDescState((c: unknown) => ({ ...((c ?? descriptor?.defaultConfig) as object), ...p }))) as (p: never) => void;
  // v2 settings carried over from a saved chart, so keys owned by other descriptors (or
  // by a future PR's namespace) survive a round-trip through this builder untouched.
  const [settings, setSettings] = useState<VizSettings>({});
  const [filters, setFilters] = useState<ChartFilter[]>([]);
  const [drillMenu, setDrillMenu] = useState<{ ctx: DrillContext; x: number; y: number } | null>(null);
  const [recordsFilters, setRecordsFilters] = useState<ChartFilter[] | null>(null);

  // hydrate from a saved chart once it loads (dataset/viz/controls + save dialog fields).
  // Guarded so a mid-edit refetch/remount cannot re-seed and clobber in-progress edits;
  // seeds exactly once per chart id.
  const hydratedRef = useRef<string | null>(null);
  // Serialized "clean" builder config, captured once the builder is ready (see below).
  // `dirty` compares the live config to this baseline to drive the beforeunload guard.
  const snapshotRef = useRef<string | null>(null);
  useEffect(() => {
    const c = existing.data;
    if (!c || hydratedRef.current === id) return;
    hydratedRef.current = id ?? "new";
    setDatasetId(c.dataset_id);
    setViz(c.viz_type);
    const p = c.params;
    // The bridge branches below still read the pre-A2 keys, so they hydrate from the
    // adapted view; a migrated viz reads its own config through its descriptor instead.
    const v1 = toRenderParams(c.viz_type, p);
    const desc = getDescriptor(c.viz_type);
    if (desc) {
      setDescState(desc.fromParams(p));
      // Carry every stored key forward, including namespaces this build doesn't own yet.
      if (isParamsV2(p)) setSettings(p.settings);
    } else if (c.viz_type === "pie") {
      setPie(v1.pie ?? {});
    } else if (c.viz_type === "scalar") {
      setScalar(v1.scalar ?? { metric: v1.spec?.metrics?.[0] });
    } else if (c.viz_type === "progress") {
      setProgress(v1.progress ?? { metric: v1.spec?.metrics?.[0] });
    } else if (c.viz_type === "gauge") {
      setGauge(v1.gauge ?? { metric: v1.spec?.metrics?.[0] });
    } else if (c.viz_type === "funnel") {
      setFunnel(v1.funnel ?? { dimension: v1.spec?.dimensions?.[0], metric: v1.spec?.metrics?.[0] });
    } else if (c.viz_type === "waterfall") {
      setWaterfall(v1.waterfall ?? { xAxis: v1.spec?.dimensions?.[0], metric: v1.spec?.metrics?.[0] });
    } else if (c.viz_type === "smartscalar") {
      setSmart(v1.smartscalar ?? { dimension: v1.spec?.dimensions?.[0], metric: v1.spec?.metrics?.[0] });
    } else if (c.viz_type === "boxplot") {
      setBox(v1.boxplot ?? { category: v1.spec?.columns?.[0], value: v1.spec?.columns?.[1] });
    } else if (c.viz_type === "sankey") {
      setSankey(v1.sankey ?? { source: v1.spec?.dimensions?.[0], target: v1.spec?.dimensions?.[1], metric: v1.spec?.metrics?.[0] });
    }
    setRowLimit(v1.spec?.row_limit ?? 1000);
    setTemporalUnits(v1.spec?.temporal_units ?? {});
    setColumnSettings(v1.columnSettings ?? {});
    setConditionalFormatting(v1.conditionalFormatting ?? []);
    setTableState(v1.tableState ?? {});
    setFilters(v1.spec?.filters ?? []);
    setSaveName(c.name);
    setSaveDesc(c.description);
  }, [existing.data]);

  // keep viz in sync with ?type for new charts (browser back/forward, deep links)
  useEffect(() => {
    if (!id && isValidViz(typeParam)) setViz(typeParam);
  }, [id, typeParam]);

  const columns = detail.data?.columns ?? [];
  const metrics = detail.data?.metrics ?? [];

  const setUnit = (col: string, token: string | undefined) =>
    setTemporalUnits((prev) => {
      const next = { ...prev };
      if (token == null) delete next[col];
      else next[col] = token;
      return next;
    });
  const unitRow = (col: string | undefined) =>
    col && columns.find((c) => c.column_name === col)?.is_temporal ? (
      <div className="flex items-center gap-2 pl-2">
        <span className="text-[11px] uppercase tracking-wide text-muted-foreground">Unit</span>
        <TemporalUnitSelect value={temporalUnits[col]} onChange={(u) => setUnit(col, u)} />
      </div>
    ) : null;

  function specForRun(): QuerySpec {
    if (descriptor) return descriptor.buildSpec(descConfig, { filters, rowLimit, temporalUnits });
    if (viz === "pie") {
      const dims = pie.dimension ? [pie.dimension] : [];
      return { metrics: pie.metric ? [pie.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
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
      const dims = funnel.dimension ? [funnel.dimension] : [];
      return { metrics: funnel.metric ? [funnel.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
    }
    if (viz === "waterfall") {
      const dims = waterfall.xAxis ? [waterfall.xAxis] : [];
      return { metrics: waterfall.metric ? [waterfall.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
    }
    if (viz === "smartscalar") {
      const dims = smart.dimension ? [smart.dimension] : [];
      return { metrics: smart.metric ? [smart.metric] : [], dimensions: dims, columns: [], filters, orderby: smart.dimension ? [{ field: smart.dimension, desc: false }] : [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
    }
    if (viz === "boxplot") {
      // Raw rows (no aggregation) — quartiles are computed client-side.
      return { metrics: [], dimensions: [], columns: [box.category, box.value].filter((c): c is string => !!c), filters, orderby: [], row_limit: rowLimit, rollup_groups: [] };
    }
    if (viz === "sankey") {
      const dims = [sankey.source, sankey.target].filter((d): d is string => !!d);
      return { metrics: sankey.metric ? [sankey.metric] : [], dimensions: dims, columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [], temporal_units: pruneUnits(temporalUnits, dims) };
    }
    // Unreachable: every viz without a branch above has a descriptor, which answered first.
    throw new Error(`no spec builder for viz ${viz}`);
  }

  // A temporal dimension defaults to monthly grouping (Metabase-style) until the user changes it.
  const activeDims = specForRun().dimensions;
  useEffect(() => {
    setTemporalUnits((prev) => {
      let changed = false;
      const next = { ...prev };
      for (const name of activeDims) {
        const temporal = columns.find((c) => c.column_name === name)?.is_temporal;
        if (temporal && !(name in next)) {
          next[name] = "month";
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [activeDims.join("|"), columns]);

  const [autoRun, setAutoRun] = useState(true);
  // Params of the last run, rendered while the query is stale (see previewParams).
  const ranParams = useRef<AnyChartParams | null>(null);
  // Run state lives in the hook: what ran, the refetch nonce, and whether the current
  // config has drifted from it. `missingNow`/`specForRun`/`paramsForSave` are function
  // declarations, so this can sit above them.
  const { ranSpec, runNonce, run, isStale, reset: resetRun } = useChartRun({
    datasetId: datasetId ?? "",
    autoRun,
    buildSpec: specForRun,
    missing: missingNow,
    afterRun: () => {
      // Snapshot what this run renders with (see previewParams / previewConfig).
      ranParams.current = paramsForSave();
    },
  });
  const data = useSpecData(ws, datasetId, ranSpec, !!ranSpec, runNonce);
  const finished = !data.isLoading && !data.error && !!ranSpec;
  const resultColumns = data.columns.filter((c) => !c.name.startsWith("__grp_"));
  const metricTypes = Object.fromEntries((detail.data?.metrics ?? []).map((m) => [m.metric_name, m.data_type ?? "double"]));

  // Dataset-level formats (column & metric) become chart DEFAULTS: they render
  // unless the chart has its own per-column override. Applied at render time
  // only (never baked into saved params) so editing the dataset stays live.
  const datasetDefaults = useMemo(() => {
    const m: Record<string, ColumnFormat> = {};
    for (const c of detail.data?.columns ?? []) {
      const cf: ColumnFormat = { ...parseFormat(c.format) };
      if (!cf.view_as) {
        if (c.semantic_type === "email") cf.view_as = "email";
        else if (c.semantic_type === "url") cf.view_as = "link";
        else if (c.semantic_type === "image") cf.view_as = "image";
      }
      if (c.semantic_type === "currency" && !cf.number_style) { cf.number_style = "currency"; cf.currency = cf.currency ?? "USD"; }
      if (Object.keys(cf).length) m[c.column_name] = cf;
    }
    for (const mt of detail.data?.metrics ?? []) {
      const cf = parseFormat(mt.format);
      if (Object.keys(cf).length) m[mt.metric_name] = cf;
    }
    return m;
  }, [detail.data]);
  const effCS = useMemo(() => ({ ...datasetDefaults, ...columnSettings }), [datasetDefaults, columnSettings]);

  // "View SQL" disclosure: build-only, fetched lazily (only while the
  // disclosure is open) via the `charts`-gated ch_data endpoint — independent
  // of the sql_lab entitlement that gates ch_run_spec above.
  const [sqlOpen, setSqlOpen] = useState(false);
  const chartSql = useBuildChartData(ws, datasetId, ranSpec, sqlOpen);

  // A function declaration, not a const: `missingNow` reaches this while useChartRun runs,
  // which sits above it in the body, so an arrow const would be in its temporal dead zone
  // and every bridge viz would crash on open.
  function validationInput() {
    return { pie, scalar, progress, gauge, funnel, waterfall, smart, box, sankey };
  }

  /** Fields the active viz still needs — from its descriptor, or the bridge's table. */
  function missingNow(): string[] {
    return descriptor ? descriptor.requiredFields(descConfig) : missingFields(viz, validationInput());
  }

  // Params the preview renders from. The field names inside them (x-axis, metrics,
  // breakout…) address columns of the result on screen, so while the query is stale
  // — edited but not re-run, the normal state with Auto off — we keep rendering the
  // params that produced that result. Pairing freshly edited field names with the old
  // columns finds nothing and renders an empty chart, which reads as "you broke it"
  // rather than "not run yet". Display-only edits don't change the signature, so a fresh
  // query previews them live; while it's stale the whole snapshot holds (a colour changed
  // mid-edit lands on the next Run).
  const previewParams = (): ChartParams => {
    const base = isStale && ranParams.current ? ranParams.current : paramsForSave();
    // Renderers still speak v1, so a migrated viz's v2 params go through the adapter here.
    return { ...toRenderParams(viz, base), columnSettings: effCS };
  };

  // Same rule for the previews that take a config instead of a ChartParams: frozen at what
  // last ran while the query is stale, because the field names inside a config address the
  // columns of the result on screen.
  function previewConfig<Cfg>(desc: VizDescriptor<Cfg>): Cfg {
    const base = isStale && ranParams.current ? ranParams.current : paramsForSave();
    return desc.fromParams(base);
  }

  function addFilter(f: ChartFilter) {
    setFilters((prev) => [...prev.filter((x) => x.col !== f.col), f]);
  }

  const mutations = useChartMutations(ws);
  const [saveOpen, setSaveOpen] = useState(false);
  const [saveName, setSaveName] = useState("");
  const [saveDesc, setSaveDesc] = useState("");

  function paramsForSave(): AnyChartParams {
    // Migrated viz types persist v2: the query, plus this descriptor's namespaced keys
    // merged over whatever settings the saved chart already carried.
    if (descriptor) {
      return {
        version: 2,
        spec: specForRun(),
        settings: {
          ...settings,
          ...descriptor.toSettings(descConfig),
          column_settings: columnSettings,
          conditional_formatting: conditionalFormatting,
        },
      };
    }
    if (viz === "pie") return { spec: specForRun(), pie };
    if (viz === "scalar") return { spec: specForRun(), scalar, columnSettings };
    if (viz === "progress") return { spec: specForRun(), progress, columnSettings };
    if (viz === "gauge") return { spec: specForRun(), gauge, columnSettings };
    if (viz === "funnel") return { spec: specForRun(), funnel, columnSettings };
    if (viz === "waterfall") return { spec: specForRun(), waterfall, columnSettings };
    if (viz === "smartscalar") return { spec: specForRun(), smartscalar: smart, columnSettings };
    if (viz === "boxplot") return { spec: specForRun(), boxplot: box, columnSettings };
    if (viz === "sankey") return { spec: specForRun(), sankey, columnSettings };
    // Unreachable: the remaining viz types all have a descriptor, handled above.
    throw new Error(`no params builder for viz ${viz}`);
  }

  // Serialize the full builder config exactly as it would be persisted — the same
  // payload builder Save uses (paramsForSave) plus the dataset + viz identity that
  // live outside ChartParams. Used only for dirty-tracking (never gates Save).
  const serializeConfig = () => JSON.stringify({ datasetId, viz, params: paramsForSave() });

  // Distribute a ChartParams emitted by ChartSettingsSections back into the local
  // display state. The settings component only edits these five display fields,
  // so we only fan those out (everything else in ChartExplore's state model is
  // left untouched).
  function applyParamsBack(next: ChartParams) {
    if (next.pie) setPie(next.pie);
    if (next.tableState) setTableState(next.tableState);
    if (next.columnSettings) setColumnSettings(next.columnSettings);
    if (next.conditionalFormatting) setConditionalFormatting(next.conditionalFormatting);
  }

  async function save() {
    if (!datasetId) { toast.error("Pick a dataset"); return; }
    if (!saveName.trim()) { toast.error("Name required"); return; }
    const gaps = missingNow();
    if (gaps.length) { toast.error(`This chart needs ${gaps.join(" and ")}`); return; }
    try {
      if (id) {
        await mutations.update.mutateAsync({ id, dataset_id: datasetId, name: saveName.trim(), description: saveDesc, viz_type: viz, params: paramsForSave() });
        // Re-baseline so the just-saved state is clean (equivalent to `!justSaved`).
        snapshotRef.current = serializeConfig();
        setSaveOpen(false);
        toast.success("Saved");
      } else {
        const created = await mutations.create.mutateAsync({ dataset_id: datasetId, name: saveName.trim(), description: saveDesc, viz_type: viz, params: paramsForSave() });
        snapshotRef.current = serializeConfig();
        setSaveOpen(false);
        toast.success("Saved");
        navigate(`/explore/${ws}/charts/${created.id}`);
      }
    } catch (e) { toast.error(String(e)); }
  }

  function changeViz(v: VizType) {
    setViz(v);
    resetRun();
    // A different viz means a different config shape; its descriptor's default applies.
    setDescState(null);
    if (!id) setSearchParams({ type: v });
  }


  function resetControls() {
    // `null` drops back to the active descriptor's defaultConfig; the bridge types still
    // need their own hooks cleared by hand until PR3 retires them.
    setDescState(null); setSettings({});
    setColumnSettings({}); setConditionalFormatting([]); setTableState({}); setFilters([]); resetRun();
    setPie({}); setScalar({}); setProgress({}); setGauge({}); setFunnel({}); setWaterfall({}); setSmart({}); setBox({}); setSankey({});
    setTemporalUnits({});
  }

  // Capture the "clean" baseline once the builder is ready: for a new chart that's the
  // empty default builder (first render); for a saved chart we wait until hydration has
  // seeded the state (hydratedRef === id) so the baseline reflects the loaded chart.
  useEffect(() => {
    if (snapshotRef.current !== null) return;
    if (id && hydratedRef.current !== id) return;
    snapshotRef.current = serializeConfig();
  });
  // beforeunload guard only — never gates Save/Run. Errs toward an extra prompt (safe),
  // never toward silently dropping edits.
  const dirty = snapshotRef.current !== null && serializeConfig() !== snapshotRef.current;
  useUnsavedChangesWarning(dirty);

  // Data/Display builder tab in the URL (?tab=), default "data" (preserves prior behavior).
  const builderTab = searchParams.get("tab") ?? "data";
  const setBuilderTab = (v: string) =>
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      if (v === "data") next.delete("tab");
      else next.set("tab", v);
      return next;
    }, { replace: true });

  const isRunning = data.isLoading;
  const vizMeta = CHART_TYPES.find((t) => t.key === viz);
  const previewRef = useRef<HTMLDivElement>(null);

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

  // ── Edit mode: gate on the saved-chart fetch so we never render the blank
  //    default builder while the chart is still loading or after it errors. ──
  if (id && existing.isLoading) {
    return (
      <div className="mx-auto max-w-3xl p-6">
        <Link className="text-xs text-muted-foreground hover:underline" to={`/explore/${ws}/charts`}>← Charts</Link>
        <div className="mt-4">
          <ListSkeleton rows={4} cols={1} />
        </div>
      </div>
    );
  }
  if (id && existing.isError) {
    return (
      <div className="mx-auto max-w-3xl p-6">
        <Link className="text-xs text-muted-foreground hover:underline" to={`/explore/${ws}/charts`}>← Charts</Link>
        <Alert variant="destructive" className="mt-4">
          <AlertDescription>Failed to load this chart. Please try again.</AlertDescription>
        </Alert>
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

        <Tabs value={builderTab} onValueChange={setBuilderTab}>
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





            {descriptor && (
              <StudioPanel descriptor={descriptor} config={descConfig} onConfigChange={patchDescConfig}
                columns={columns} metrics={metrics} resultColumns={resultColumns} rows={data.rows}
                unitRow={unitRow} tab="data" />
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
                {unitRow(pie.dimension)}
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
                {unitRow(funnel.dimension)}
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

            {viz === "waterfall" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">X-axis (steps)</Label>
                  <Select value={waterfall.xAxis ?? ""} onValueChange={(v) => setWaterfall((w) => ({ ...w, xAxis: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                {unitRow(waterfall.xAxis)}
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric (delta)</Label>
                  <Select value={waterfall.metric ?? ""} onValueChange={(v) => setWaterfall((w) => ({ ...w, metric: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
                    <SelectContent>
                      {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div className="flex items-center gap-2">
                  <Checkbox id="wf-total" checked={!!waterfall.showTotal} onCheckedChange={() => setWaterfall((w) => ({ ...w, showTotal: !w.showTotal }))} />
                  <Label htmlFor="wf-total" className="cursor-pointer text-sm font-normal">Show total bar</Label>
                </div>
              </>
            )}

            {viz === "smartscalar" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Time / order dimension</Label>
                  <Select value={smart.dimension ?? ""} onValueChange={(v) => setSmart((s) => ({ ...s, dimension: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                {unitRow(smart.dimension)}
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric</Label>
                  <Select value={smart.metric ?? ""} onValueChange={(v) => setSmart((s) => ({ ...s, metric: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
                    <SelectContent>
                      {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
              </>
            )}

            {viz === "boxplot" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Category</Label>
                  <Select value={box.category ?? ""} onValueChange={(v) => setBox((b) => ({ ...b, category: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a column…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Value column</Label>
                  <Select value={box.value ?? ""} onValueChange={(v) => setBox((b) => ({ ...b, value: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a numeric column…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
              </>
            )}

            {viz === "sankey" && (
              <>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Source</Label>
                  <Select value={sankey.source ?? ""} onValueChange={(v) => setSankey((s) => ({ ...s, source: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
                    <SelectContent>
                      {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Target</Label>
                  <Select value={sankey.target ?? ""} onValueChange={(v) => setSankey((s) => ({ ...s, target: v || undefined }))}>
                    <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
                    <SelectContent>
                      {columns.filter((c) => c.column_name !== sankey.source).map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1">
                  <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Metric</Label>
                  <Select value={sankey.metric ?? ""} onValueChange={(v) => setSankey((s) => ({ ...s, metric: v || undefined }))}>
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

            {descriptor ? (
              <StudioPanel descriptor={descriptor} config={descConfig} onConfigChange={patchDescConfig}
                columns={columns} metrics={metrics} resultColumns={resultColumns} rows={data.rows}
                unitRow={unitRow} tab="display" />
            ) : (
              <ChartSettingsSections
                vizType={viz}
                params={toRenderParams(viz, paramsForSave())}
                columns={resultColumns}
                rows={data.rows}
                onChange={applyParamsBack}
              />
            )}
          </TabsContent>
        </Tabs>
      </div>

      {/* Main area (full-width preview) */}
      <div className="flex-1 overflow-auto p-4 space-y-3">
        <h1 className="sr-only">{saveName || "New chart"}</h1>
        <StudioToolbar
          name={saveName} onName={setSaveName}
          autoRun={autoRun} onAutoRun={setAutoRun}
          onRun={run} onSave={() => setSaveOpen(true)}
          running={data.isLoading} saveLabel={id ? "Update" : "Save"}
          exportMenu={
            <ChartExportMenu title={saveName || undefined} columns={resultColumns} rows={data.rows}
              containerRef={previewRef} disabled={!finished || resultColumns.length === 0} />
          }
          sqlDisclosure={ranSpec && (
            <details className="text-xs text-muted-foreground" open={sqlOpen} onToggle={(e) => setSqlOpen(e.currentTarget.open)}>
              <summary className="cursor-pointer select-none hover:text-foreground">View SQL</summary>
              {sqlOpen && chartSql.isLoading && <p className="mt-1 text-xs">Loading…</p>}
              {sqlOpen && chartSql.error && <p className="mt-1 text-xs text-destructive">{String(chartSql.error)}</p>}
              {chartSql.data?.sql && <pre className="mt-1 max-h-40 overflow-auto rounded bg-muted p-2 text-xs">{chartSql.data.sql}</pre>}
            </details>
          )}
        />

        {filters.length > 0 && (
          <FilterChips filters={filters} onRemove={(i) => setFilters((prev) => prev.filter((_, j) => j !== i))} />
        )}
        {data.error && <ChartErrorState message={data.error} onRetry={run} />}
        {isRunning && <ChartLoadingState />}

        <div ref={previewRef} className="contents">
        {finished && viz === "pivot_table" && (() => {
          const pivotCfg = descConfig as unknown as import("@/components/chart/viz/pivot").PivotConfig;
          return (
          <PivotTable
            input={{
              columns: data.columns.map((c) => c.name),
              rows: data.rows,
              ...(() => {
                const c = previewConfig(pivotDescriptor());
                return { rowDims: c.rows, colDims: c.cols, metrics: c.metrics };
              })(),
              showRowTotals: pivotCfg.showRowTotals ?? true,
              showColTotals: pivotCfg.showColTotals ?? true,
              condense: pivotCfg.condense ?? true,
              collapsed: new Set(pivotCfg.collapsedRows ?? []),
              sortDims: pivotCfg.sort ?? {},
            }}
            columnSettings={effCS}
            conditionalFormatting={conditionalFormatting}
            metricTypes={metricTypes}
            onToggleCollapse={(path) => {
              const cur = pivotCfg.collapsedRows ?? [];
              patchDescConfig({ collapsedRows: cur.includes(path) ? cur.filter((x) => x !== path) : [...cur, path] } as never);
            }}
            onCycleSort={(dim) => {
              const cur = (pivotCfg.sort ?? {})[dim];
              const next = { ...(pivotCfg.sort ?? {}) };
              if (cur === undefined) next[dim] = "asc";
              else if (cur === "asc") next[dim] = "desc";
              else delete next[dim];
              patchDescConfig({ sort: next } as never);
            }}
            onCellDrill={(ctx, x, y) => setDrillMenu({ ctx, x, y })}
          />
          );
        })()}
        {finished && viz === "table" && (
          <ChartTable columns={data.columns} rows={data.rows}
            columnSettings={effCS} conditionalFormatting={conditionalFormatting}
            tableState={tableState} onTableStateChange={setTableState}
            dimensionColumns={(() => {
              const c = previewConfig(tableDescriptor());
              return c.metrics.length > 0 ? c.columns : [];
            })()}
            onCellDrill={(ctx, x, y) => setDrillMenu({ ctx, x, y })} />
        )}
        {finished && viz !== "table" && viz !== "pivot_table" && (
          <ChartErrorBoundary resetKey={`${viz}:${data.columns.length}:${data.rows.length}`}>
            {/* ChartRenderer fills its parent; the preview owns the height + frame
                (dashboard cards size it to the grid cell instead). */}
            <div className="h-[60vh] rounded-lg border p-2">
              <ChartRenderer
                vizType={viz}
                params={previewParams()}
                columns={data.columns}
                rows={data.rows}
                metricTypes={metricTypes}
              />
            </div>
          </ChartErrorBoundary>
        )}
        </div>
        {!ranSpec && !data.isLoading && (() => {
          const missing = datasetId ? missingFields(viz, validationInput()) : ["a dataset"];
          return (
            <ChartEmptyState icon={vizMeta?.icon} title={`Set up your ${vizMeta?.name ?? "chart"}`}
              hint={missing.length ? `Add: ${missing.join(", ")}${autoRun ? "" : ", then Run"}` : (autoRun ? "Ready — adjust the fields on the left." : "Ready — click Run to preview.")} />
          );
        })()}
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
