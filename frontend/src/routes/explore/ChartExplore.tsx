import { useEffect, useMemo, useRef, useState } from "react";
import { useParams, Link, useNavigate, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { useDatasets, useDataset } from "@/api/datasets";
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams, type TableState, type ChartFilter,
  type AnyChartParams, type VizSettings, isParamsV2,
} from "@/api/charts";
import { useSpecData } from "@/api/useSpecData";
import { FilterChips } from "@/components/chart/FilterChips";
import { DrillMenu } from "@/components/chart/DrillMenu";
import { DrillRecordsDialog } from "@/components/chart/DrillRecordsDialog";
import type { DrillContext } from "@/components/chart/drill";
import { ChartTypeGallery } from "@/components/chart/ChartTypeGallery";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { SharedDisplaySections, styleColumnsFor, ruleColumnsFor } from "@/components/chart/settings/SharedDisplaySections";
import { ChartExportMenu } from "@/components/chart/ChartExportMenu";
import { StudioToolbar } from "@/components/chart/StudioToolbar";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import { parseFormat } from "@/components/dataset/FormatPicker";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter,
} from "@/components/ui/dialog";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { TemporalUnitSelect } from "@/components/chart/TemporalUnitSelect";
import { useChartRun } from "@/components/chart/useChartRun";
import { getDescriptor } from "@/components/chart/viz/registry";
import { pivotDescriptor, type PivotConfig } from "@/components/chart/viz/pivot";
import { tableDescriptor } from "@/components/chart/viz/table";
import type { VizDescriptor } from "@/components/chart/viz/types";
import { shellSettings, mergeVizSettings } from "@/components/chart/viz/renderConfig";
import { StudioPanel } from "@/components/chart/StudioPanel";
import { PreviewPane } from "@/components/chart/PreviewPane";
import { VizSwitcher } from "@/components/chart/VizSwitcher";
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

  const [temporalUnits, setTemporalUnits] = useState<Record<string, string>>({});
  const [rowLimit, setRowLimit] = useState(1000);
  const [columnSettings, setColumnSettings] = useState<Record<string, ColumnFormat>>({});
  const [conditionalFormatting, setConditionalFormatting] = useState<Rule[]>([]);
  const [tableState, setTableState] = useState<TableState>({});
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
    const desc = getDescriptor(c.viz_type);
    if (desc) {
      setDescState(desc.fromParams(p));
      // Carry every stored key forward, including ones this build doesn't read.
      if (isParamsV2(p)) setSettings(p.settings);
    }
    setRowLimit(p.spec?.row_limit ?? 1000);
    setTemporalUnits(p.spec?.temporal_units ?? {});
    const shell = shellSettings(p);
    setColumnSettings(shell.columnSettings);
    setConditionalFormatting(shell.conditionalFormatting);
    setTableState(shell.tableState);
    setFilters(p.spec?.filters ?? []);
    setSaveName(c.name);
    setSaveDesc(c.description);
  }, [existing.data]);

  // keep viz in sync with ?type for new charts (browser back/forward, deep links)
  useEffect(() => {
    if (!id && isValidViz(typeParam)) setViz(typeParam);
  }, [id, typeParam]);

  // Only these two draw conditional colours, so only they get the tab.
  const hasConditionalRules = viz === "table" || viz === "pivot_table";

  const columns = detail.data?.columns ?? [];
  const metrics = detail.data?.metrics ?? [];

  const setUnit = (col: string, token: string | undefined) =>
    setTemporalUnits((prev) => {
      const next = { ...prev };
      if (token == null) delete next[col];
      else next[col] = token;
      return next;
    });
  const unitRow = (col: string | undefined) => {
    const c = col ? columns.find((x) => x.column_name === col) : undefined;
    // The label names the field. It used to read a bare "Unit", which is unambiguous with one date
    // dimension and useless with two — a pivot with a date on both axes showed two identical rows.
    const shown = c?.verbose_name?.trim() || col;
    return c?.is_temporal ? (
      <div className="flex items-center gap-2 pl-2">
        <span className="truncate text-[11px] uppercase tracking-wide text-muted-foreground" title={col}>
          Unit · {shown}
        </span>
        <TemporalUnitSelect value={temporalUnits[col!]} onChange={(u) => setUnit(col!, u)} />
      </div>
    ) : null;
  };

  function specForRun(): QuerySpec {
    if (descriptor) return descriptor.buildSpec(descConfig, { filters, rowLimit, temporalUnits });
    // Unreachable: every viz type is in the registry.
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

  /** Fields the active viz still needs, straight from its descriptor. */
  function missingNow(): string[] {
    return descriptor ? descriptor.requiredFields(descConfig) : [];
  }

  // Params the preview renders from. The field names inside them (x-axis, metrics,
  // breakout…) address columns of the result on screen, so while the query is stale
  // — edited but not re-run, the normal state with Auto off — we keep rendering the
  // params that produced that result. Pairing freshly edited field names with the old
  // columns finds nothing and renders an empty chart, which reads as "you broke it"
  // rather than "not run yet". Display-only edits don't change the signature, so a fresh
  // query previews them live; while it's stale the whole snapshot holds (a colour changed
  // mid-edit lands on the next Run).
  const previewParams = (): AnyChartParams => {
    const base = isStale && ranParams.current ? ranParams.current : paramsForSave();
    // Renderers resolve their own config from these params; only the effective column
    // formats (dataset defaults under the chart's own) are layered on here.
    return { ...base, settings: { ...(isParamsV2(base) ? base.settings : {}), column_settings: effCS } };
  };

  // Same rule for the previews that take a config instead of a ChartParams: frozen at what
  // last ran while the query is stale, because the field names inside a config address the
  // columns of the result on screen.
  function previewConfig<Cfg>(desc: VizDescriptor<Cfg>): Cfg {
    const base = isStale && ranParams.current ? ranParams.current : paramsForSave();
    return desc.fromParams(base);
  }

  /** Replace whatever the drilled columns already had, then add every new filter. Takes a LIST
   *  because a bucketed dimension drills to a half-open range — two filters on the SAME column,
   *  which the old one-at-a-time version would have deduped down to the second half. */
  function addFilter(fs: ChartFilter[]) {
    const cols = new Set(fs.map((f) => f.col));
    setFilters((prev) => [...prev.filter((x) => !cols.has(x.col)), ...fs]);
  }

  const mutations = useChartMutations(ws);
  const [saveOpen, setSaveOpen] = useState(false);
  const [saveName, setSaveName] = useState("");
  const [saveDesc, setSaveDesc] = useState("");

  function paramsForSave(): AnyChartParams {
    if (descriptor) {
      return {
        version: 2,
        spec: specForRun(),
        settings: {
          // mergeVizSettings, not a bare spread: a setting the user has put back to "Auto"
          // must LEAVE the stored settings, and a spread can only add or overwrite.
          ...mergeVizSettings(settings, descriptor, descConfig),
          column_settings: columnSettings,
          conditional_formatting: conditionalFormatting,
        },
      };
    }
    throw new Error(`no params builder for viz ${viz}`);
  }

  // Serialize the full builder config exactly as it would be persisted — the same
  // payload builder Save uses (paramsForSave) plus the dataset + viz identity that
  // live outside ChartParams. Used only for dirty-tracking (never gates Save).
  const serializeConfig = () => JSON.stringify({ datasetId, viz, params: paramsForSave() });


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
  // Switching to a viz without rules while that tab is open would leave the panel blank: the
  // trigger disappears but the selected value does not, and no TabsContent matches it.
  useEffect(() => {
    if (!hasConditionalRules && builderTab === "rules") setBuilderTab("display");
  }, [hasConditionalRules, builderTab]);
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
      {/* `scrollbar-gutter: stable` keeps a visible track instead of an overlay scrollbar that
          only appears mid-scroll. On macOS the overlay bar is hidden at rest, so a settings panel
          taller than the window looked simply cut off — the pivot's is the longest (totals,
          per-dimension subtotals, formatting, conditional formatting) and it was reported as
          "Display doesn't show everything". It always scrolled; nothing said so. */}
      <div className="w-72 shrink-0 overflow-y-auto [scrollbar-gutter:stable] border-r p-4 space-y-4">
        <Link className="text-xs text-muted-foreground hover:underline" to={`/explore/${ws}/charts`}>← Charts</Link>

        <VizSwitcher viz={viz} onChange={changeViz} />

        <Tabs value={builderTab} onValueChange={setBuilderTab}>
          <TabsList className={UNDERLINE_LIST}>
            <TabsTrigger value="data" className={UNDERLINE_TRIGGER}>Data</TabsTrigger>
            <TabsTrigger value="display" className={UNDERLINE_TRIGGER}>Display</TabsTrigger>
            {/* Its own tab, the way Metabase splits Columns | Conditional Formatting. Stacked in
                with the rest, it made the pivot's panel the longest in the app and the reason it
                read as cut off. Only table and pivot have rules, so the tab only exists there. */}
            {hasConditionalRules && (
              <TabsTrigger value="rules" className={UNDERLINE_TRIGGER}>Colors</TabsTrigger>
            )}
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










            <div className="space-y-1">
              <Label htmlFor="row-limit" className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Row limit</Label>
              <Input id="row-limit" type="number" min={1} max={10000} value={rowLimit}
                onChange={(e) => { const n = Number(e.target.value); setRowLimit(Number.isNaN(n) ? 0 : n); }}
                className="h-8 text-sm" />
            </div>
          </TabsContent>

          {/* ── DISPLAY TAB ── */}
          <TabsContent value="display" className="space-y-4">

            {descriptor && (
              <>
                <StudioPanel descriptor={descriptor} config={descConfig} onConfigChange={patchDescConfig}
                  columns={columns} metrics={metrics} resultColumns={resultColumns} rows={data.rows}
                  unitRow={unitRow} tab="display" hasMoreBelow />
                {/* Formatting, table columns and conditional formatting edit settings the
                    shell owns, not any one viz — so they sit beside the descriptor's own
                    controls rather than inside it. */}
                <SharedDisplaySections
                  vizType={viz}
                  resultColumns={resultColumns}
                  styleColumns={styleColumnsFor(viz, {
                    spec: specForRun(),
                    cartesian: descConfig as { yMetrics: string[]; xAxis?: string; series?: string },
                    scalar: descConfig as { metric?: string },
                    // …and the pie formats its dimensions and its metric. Left out, the synthetic
                    // params below carried no `pie` key at all, so the filter matched nothing and
                    // the section stayed hidden however the descriptor was configured.
                    pie: descConfig as { dimensions?: string[]; metric?: string },
                    metrics: (descConfig as { metrics?: string[] }).metrics,
                    // The pivot formats its dimensions as well as its measures — without these
                    // two, `styleColumnsFor` saw no rows/cols and offered measures only.
                    rows: (descConfig as { rows?: string[] }).rows,
                    cols: (descConfig as { cols?: string[] }).cols,
                  } as ChartParams, resultColumns)}
                  tableState={tableState}
                  columnSettings={columnSettings}
                  conditionalFormatting={conditionalFormatting}
                  onTableState={setTableState}
                  onColumnSettings={setColumnSettings}
                  onConditionalFormatting={setConditionalFormatting}
                  only="format"
                />
              </>
            )}
          </TabsContent>

          {/* ── COLORS TAB (conditional formatting) ── */}
          {hasConditionalRules && (
            <TabsContent value="rules" className="space-y-4">
              <SharedDisplaySections
                vizType={viz}
                resultColumns={resultColumns}
                styleColumns={styleColumnsFor(viz, {
                  spec: specForRun(),
                  cartesian: descConfig as { yMetrics: string[]; xAxis?: string; series?: string },
                  scalar: descConfig as { metric?: string },
                    // …and the pie formats its dimensions and its metric. Left out, the synthetic
                    // params below carried no `pie` key at all, so the filter matched nothing and
                    // the section stayed hidden however the descriptor was configured.
                    pie: descConfig as { dimensions?: string[]; metric?: string },
                  metrics: (descConfig as { metrics?: string[] }).metrics,
                  rows: (descConfig as { rows?: string[] }).rows,
                  cols: (descConfig as { cols?: string[] }).cols,
                } as ChartParams, resultColumns)}
                tableState={tableState}
                columnSettings={columnSettings}
                conditionalFormatting={conditionalFormatting}
                onTableState={setTableState}
                onColumnSettings={setColumnSettings}
                ruleColumns={ruleColumnsFor(viz, {
                  spec: specForRun(),
                  cartesian: descConfig as { yMetrics: string[]; xAxis?: string; series?: string },
                  scalar: descConfig as { metric?: string },
                    // …and the pie formats its dimensions and its metric. Left out, the synthetic
                    // params below carried no `pie` key at all, so the filter matched nothing and
                    // the section stayed hidden however the descriptor was configured.
                    pie: descConfig as { dimensions?: string[]; metric?: string },
                  metrics: (descConfig as { metrics?: string[] }).metrics,
                  rows: (descConfig as { rows?: string[] }).rows,
                  cols: (descConfig as { cols?: string[] }).cols,
                } as ChartParams, resultColumns)}
                onConditionalFormatting={setConditionalFormatting}
                only="conditional"
              />
            </TabsContent>
          )}
        </Tabs>
      </div>

      {/* Main area (full-width preview). A flex COLUMN, so the chart card below can claim
          the height left over after the toolbar instead of guessing at a viewport fraction
          — the parent row is `flex h-full min-h-0`, which makes this column's own height
          definite, which is what lets `flex-1` on the card resolve to real pixels. It keeps
          `overflow-auto` for the short-window case, where the card's own min-height wins
          and this scrolls. */}
      <div className="flex flex-1 flex-col overflow-auto p-4 space-y-3">
        <h1 className="sr-only">{saveName || "New chart"}</h1>
        <StudioToolbar
          name={saveName} onName={setSaveName}
          autoRun={autoRun} onAutoRun={setAutoRun}
          onRun={run} onSave={() => setSaveOpen(true)}
          running={data.isLoading} saveLabel={id ? "Update" : "Save"}
          exportMenu={
            <ChartExportMenu title={saveName || undefined} columns={resultColumns} rows={data.rows}
              columnSettings={effCS}
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

        {(() => {
          const pivotCfg = descConfig as unknown as PivotConfig;
          const missing = datasetId ? missingNow() : ["a dataset"];
          const frozenPivot = previewConfig(pivotDescriptor());
          const frozenTable = previewConfig(tableDescriptor());
          return (
            <PreviewPane
              viz={viz}
              params={previewParams()}
              columns={data.columns}
              rows={data.rows}
              metricTypes={metricTypes}
              columnSettings={effCS}
              conditionalFormatting={conditionalFormatting}
              isRunning={isRunning}
              error={data.error}
              finished={finished}
              onRetry={run}
              onCellDrill={(ctx, x, y) => setDrillMenu({ ctx, x, y })}
              emptyIcon={vizMeta?.icon}
              emptyTitle={`Set up your ${vizMeta?.name ?? "chart"}`}
              emptyHint={missing.length
                ? `Add: ${missing.join(", ")}${autoRun ? "" : ", then Run"}`
                : (autoRun ? "Ready — adjust the fields on the left." : "Ready — click Run to preview.")}
              containerRef={previewRef}
              pivot={viz === "pivot_table" ? {
                rowDims: frozenPivot.rows, colDims: frozenPivot.cols, metrics: frozenPivot.metrics,
                showRowTotals: pivotCfg.showRowTotals ?? true,
                showColTotals: pivotCfg.showColTotals ?? true,
                condense: pivotCfg.condense ?? true,
                collapsed: new Set(pivotCfg.collapsedRows ?? []),
                sortDims: pivotCfg.sort ?? {},
                dimTotals: pivotCfg.dimTotals,
                columnWidths: pivotCfg.columnWidths,
                temporalUnits: temporalUnits ?? {},
                // Stored with the chart like `collapsedRows` and `sort`, and deliberately NOT part
                // of `buildSpec` — a column width is presentation, and putting it in the spec would
                // re-run the query on every drag.
                onResizeColumn: (kind, index, width) => {
                  const cur = pivotCfg.columnWidths ?? {};
                  patchDescConfig({
                    columnWidths: { ...cur, [kind]: { ...(cur[kind] ?? {}), [index]: Math.round(width) } },
                  } as never);
                },
                onToggleCollapse: (path) => {
                  const cur = pivotCfg.collapsedRows ?? [];
                  patchDescConfig({ collapsedRows: cur.includes(path) ? cur.filter((x) => x !== path) : [...cur, path] } as never);
                },
                onCycleSort: (dim) => {
                  const cur = (pivotCfg.sort ?? {})[dim];
                  const next = { ...(pivotCfg.sort ?? {}) };
                  if (cur === undefined) next[dim] = "asc";
                  else if (cur === "asc") next[dim] = "desc";
                  else delete next[dim];
                  patchDescConfig({ sort: next } as never);
                },
              } : undefined}
              table={viz === "table" ? {
                state: tableState,
                onStateChange: setTableState,
                dimensionColumns: frozenTable.metrics.length > 0 ? frozenTable.columns : [],
                // From the FROZEN spec, not the live `temporalUnits` state: the rows on screen were
                // fetched with the units in force when the query ran, so a drill has to filter by
                // those. Reading live state would build a bucket for a grouping the cell never used.
                temporalUnits: previewParams().spec.temporal_units ?? {},
              } : undefined}
            >
              {filters.length > 0 && (
                <FilterChips filters={filters} onRemove={(i) => setFilters((prev) => prev.filter((_, j) => j !== i))} />
              )}
            </PreviewPane>
          );
        })()}
      </div>

      {/* Change-visualization dialog */}
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
