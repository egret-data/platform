import { useEffect, useMemo, useRef, useState } from "react";
import { useParams, Link, useNavigate, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { useDatasets, useDataset } from "@/api/datasets";
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams, type TableState, type ChartFilter, type ChartOrderBy,
  type AnyChartParams, type VizSettings, isParamsV2, MAX_CHART_ROWS,
} from "@/api/charts";
import { useSpecData } from "@/api/useSpecData";
import { FilterChips } from "@/components/chart/FilterChips";
import { DrillMenu } from "@/components/chart/DrillMenu";
import { DrillRecordsDialog } from "@/components/chart/DrillRecordsDialog";
import type { DrillContext } from "@/components/chart/drill";
import { ChartTypeGallery } from "@/components/chart/ChartTypeGallery";
import { columnLabels, metricLabels } from "@/components/chart/config/field-names";
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
import { withDefaultTemporalUnits } from "@/components/chart/temporal-units";
import { reconcileTableState } from "@/components/chart/table-state";
import { useChartRun } from "@/components/chart/useChartRun";
import { getDescriptor } from "@/components/chart/viz/registry";
import { authoredOrderby, honoursOrderby, honoursRowLimit, sortableFields } from "@/components/chart/viz/orderby";
import { SortRows } from "@/components/chart/SortRows";
import { pivotDescriptor, type PivotConfig } from "@/components/chart/viz/pivot";
import { tableDescriptor } from "@/components/chart/viz/table";
import type { VizDescriptor } from "@/components/chart/viz/types";
import { shellSettings, mergeVizSettings } from "@/components/chart/viz/renderConfig";
import { StudioPanel, hasAxesControls } from "@/components/chart/StudioPanel";
import { FIELD_LABEL } from "@/components/chart/settings/FieldLabel";
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
  // Only the charts that declare axis controls get an Axes tab — a pie or a scalar has none.
  const showAxesTab = hasAxesControls(descriptor);
  // Metabase's table settings are Columns | Display | Conditional Formatting — the column list has
  // a tab of its own and Display is three toggles. egret stacked the list on top of those toggles,
  // which is what made this the panel's longest scroll. Only the table has the section.
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
  // Shared query state, exactly like `filters` and `rowLimit`: the sort the author asked for, which
  // every descriptor's `buildSpec` folds into the spec it emits. Empty is "no explicit sort" — the
  // state a chart saved before this existed comes back in, and the state in which the cartesian
  // descriptor's own ascending-by-breakout default applies unchanged.
  const [orderby, setOrderby] = useState<ChartOrderBy[]>([]);
  const [drillMenu, setDrillMenu] = useState<{ ctx: DrillContext; x: number; y: number } | null>(null);
  const [recordsFilters, setRecordsFilters] = useState<ChartFilter[] | null>(null);

  // hydrate from a saved chart once it loads (dataset/viz/controls + save dialog fields).
  // Guarded so a mid-edit refetch/remount cannot re-seed and clobber in-progress edits;
  // seeds exactly once per chart id.
  const hydratedRef = useRef<string | null>(null);
  /**
   * The same fact as `hydratedRef`, in state rather than in a ref, because the baseline capture
   * below needs it to arrive with the seeded values and a ref cannot do that.
   *
   * A ref is mutated DURING this effect, which React runs before it processes the `setState` calls
   * the effect makes. The baseline effect runs later in the very same flush, so it saw
   * `hydratedRef.current === id` while every piece of builder state was still at its pre-hydration
   * default — and captured THAT as the chart's clean baseline. One render later the seeded values
   * landed, the live config no longer matched, and every saved chart was born dirty: the
   * beforeunload guard was armed from the moment the page finished loading, with the author having
   * touched nothing. (In Chrome the dialog is only *shown* once the page has been interacted with,
   * which is why it read as "any click in the canvas dirties the chart" — the click supplied the
   * user activation, not the change.)
   *
   * State is set in the same batch as the seeding writes, so the render that first observes
   * `hydratedId === id` is the render that has them.
   */
  const [hydratedId, setHydratedId] = useState<string | null>(null);
  // Serialized "clean" builder config, captured once the builder is ready (see below).
  // `dirty` compares the live config to this baseline to drive the beforeunload guard.
  const snapshotRef = useRef<string | null>(null);
  useEffect(() => {
    const c = existing.data;
    if (!c || hydratedRef.current === id) return;
    hydratedRef.current = id ?? "new";
    setHydratedId(id ?? "new");
    // Re-seeding replaces the config wholesale, so any baseline taken against the previous one is
    // meaningless — including the one `save()` takes for a NEW chart, which is captured before the
    // navigation to /charts/<new id> brings the server's copy back through here.
    snapshotRef.current = null;
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
    // NOT `p.spec?.orderby` — a stored spec's order is the author's sort plus whatever the
    // descriptor derived, and only the first belongs in editable state. See `authoredOrderby`.
    setOrderby(authoredOrderby(p, desc));
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
    // AND IT USED TO READ "UNIT · ORDER_MONTH". Three things were wrong with that and only the
    // third is cosmetic: "Unit" is the name of an internal concept (`temporal_units`) rather than
    // of anything the reader is doing, "·" is a separator standing in for a preposition nobody can
    // guess, and the uppercase turned the column's own name into a shouted constant — so a row
    // that says "group this column by month" read as a system string. It is a sentence now, and
    // the column name inside it is left in its own case (`ORDER_MONTH` was the CSS, not the data).
    // AND NOW THE SENTENCE IS GONE TOO. "Group order_month by" repeated a column name that the
    // field picker shows on the line directly above, and it did so in a control narrower than the
    // picker — two widths and two mentions of one column, for one decision.
    //
    // Metabase does not have this row at all: a bucketed column simply reads `Created At: Month`
    // on the column itself. This is the same shape, one control wide, keeping the capability
    // Metabase lacks — it cannot re-bucket a SQL result at all, which is what egret is doing here.
    return c?.is_temporal ? (
      <div className="pl-2">
        <TemporalUnitSelect
          label={`Group ${shown} by`}
          prefix={shown}
          value={temporalUnits[col!]}
          onChange={(u) => setUnit(col!, u)}
        />
      </div>
    ) : null;
  };

  function specForRun(): QuerySpec {
    if (descriptor) return descriptor.buildSpec(descConfig, { filters, rowLimit, temporalUnits, orderby });
    // Unreachable: every viz type is in the registry.
    throw new Error(`no spec builder for viz ${viz}`);
  }

  // A temporal dimension defaults to monthly grouping (Metabase-style) until the user changes it.
  // The rule itself lives beside `pruneUnits` so headless callers — the parity-harness seed
  // script — apply the same one instead of a copy that can drift; it returns `prev` untouched
  // when nothing changed, so this stays a no-op re-render.
  const activeDims = specForRun().dimensions;
  useEffect(() => {
    setTemporalUnits((prev) => withDefaultTemporalUnits(prev, activeDims, columns));
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

  /**
   * THE ONE PLACE A TABLE'S COLUMN STATE CAN GO STALE, so the one place it is reconciled.
   *
   * `hiddenColumns` / `columnOrder` / `columnWidths` all key on column NAME, and the only thing
   * that can take a name out of the result is an edit to the query — which is only reachable here,
   * from this route's Data tab. A dashboard card's `CardVizPanel` composes the very same display
   * sections but has no Data tab at all (`ChartTable.tsx` records why), and the drill dialog and
   * the dataset preview render `ChartTable` with no `tableState` to go stale. So there is a single
   * place, not a sprinkling.
   *
   * WHAT GUARDS IT, since this writes settings back rather than deriving them:
   *  - `data.hasResult` — a run has completed for this key at least once. Not `!isLoading`, which
   *    is also true while the query is merely disabled (see `useSpecData`'s own note). Without it
   *    the reconcile fires on the empty pre-run result and prunes a saved chart's state to nothing
   *    before its first query has even gone out.
   *  - a non-empty column list, belt-and-braces against a result that came back with no columns.
   *  - `viz === "table"`. Nothing else reads `TableState` (`ChartRenderer` hands it to the table
   *    branch only), so reconciling while a bar chart is on screen would prune a table's hidden
   *    columns against the bar's two result columns and silently lose them on the way back.
   *  - the keying is on the NAMES, not the array, which is rebuilt every render.
   *
   * `reconcileTableState` returns its argument by identity when there is nothing to prune, so the
   * common case is a React bail-out and not a re-render — the same shape, and the same reason, as
   * `withDefaultTemporalUnits` above.
   */
  const resultColumnNames = resultColumns.map((c) => c.name);
  // NUL as the separator, not a comma or a pipe: a quoted column name can legally contain either,
  // and two different results colliding on one key would skip a reconcile that was due.
  const resultColumnKey = resultColumnNames.join("\u0000");
  useEffect(() => {
    if (viz !== "table" || !data.hasResult || resultColumnNames.length === 0) return;
    setTableState((prev) => reconcileTableState(prev, resultColumnNames));
  }, [viz, data.hasResult, resultColumnKey]);

  /**
   * The query's field lists, handed to the Display tab's column list so it can fold them behind
   * `Add or remove columns` — Metabase's shape, where the one column list owns both "which columns
   * exist" and "which of them are drawn".
   *
   * BOTH lists, not just the columns one. A table with a metric on it kept `revenue` in a FOURTH
   * parallel field list one tab over, so the display list showed a column that no field list the
   * reader could see from there mentioned at all. That was measured disagreement #1.
   *
   * Only for the table. `object` has a field list too but renders no `TableState`, and every other
   * viz configures its fields through role-shaped controls (x-axis, metrics, breakout) that a flat
   * checkbox panel would misrepresent.
   *
   * It does NOT replace the Data tab's own controls, which stay: they carry the per-column temporal
   * unit pickers (`unitRow`), which are query grain and not display, and Metabase keeps its own
   * notebook field picker for the same reason. Two entry points to one setting is Metabase's
   * arrangement exactly; what this PR removes is two entry points that disagreed.
   */
  const tableQueryFields = useMemo(() => {
    if (viz !== "table") return undefined;
    const cfg = descConfig as { columns?: string[]; metrics?: string[] };
    return [
      {
        // The DATASET's name, the way Metabase heads this panel's field group with the source
        // table's. It also stops the group repeating the section's own "Columns" heading directly
        // above it, which is what a plain "Columns" here read as.
        label: detail.data?.name ?? "Columns",
        options: columns.map((c) => c.column_name),
        selected: cfg.columns ?? [],
        // The Data tab's list is built from this exact array and has read `verbose_name` since
        // #444. The options here are bare strings, so the naming has to travel beside them or the
        // two lists — the same fields, one click apart — go back to disagreeing.
        labels: columnLabels(columns),
        onChange: (next: string[]) => patchDescConfig({ columns: next } as never),
      },
      ...(metrics.length
        ? [{
            label: "Metrics",
            options: metrics.map((m) => m.metric_name),
            selected: cfg.metrics ?? [],
            labels: metricLabels(metrics),
            onChange: (next: string[]) => patchDescConfig({ metrics: next } as never),
          }]
        : []),
    ];
    // `descConfig` is rebuilt on every render, so key on what is actually read out of it.
  }, [viz, columns, metrics, (descConfig as { columns?: string[] }).columns, (descConfig as { metrics?: string[] }).metrics]);

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

  /**
   * The temporal units the RESULT ON SCREEN was bucketed by, for the settings panel.
   *
   * From `previewParams`, i.e. the FROZEN spec, and not from the live `temporalUnits` state — the
   * same rule (and the same reason) as the table drill's below: `data.rows` came back from a query
   * that ran with the units in force at the time, so a control that lists those rows' dimension
   * values back to the author has to read them under the same units. It is also the exact map the
   * preview's own `ChartRenderer` builds from, so the panel and the chart cannot key differently.
   */
  const shownUnits = (): Record<string, string> => previewParams().spec.temporal_units ?? {};

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
      const out: VizSettings = {
        // mergeVizSettings, not a bare spread: a setting the user has put back to "Auto"
        // must LEAVE the stored settings, and a spread can only add or overwrite.
        ...mergeVizSettings(settings, descriptor, descConfig),
        column_settings: columnSettings,
        conditional_formatting: conditionalFormatting,
      };
      /**
       * The table's column layout — hidden columns, display order, widths, the freeze count.
       *
       * It was missing from here entirely, which is not the same as being missing from the chart:
       * `settings` above is the blob this chart was LOADED with, so a stored `table.state` rode
       * through untouched and the layout appeared to persist. Only edits made in this editor were
       * lost — hide a column, save, reload, and it is back. Everything else the shell owns
       * (`column_settings`, `conditional_formatting`) was already written from its own state; this
       * key alone was read at hydration and never written.
       *
       * The same removal rule as `mergeVizSettings`, and for the same reason: the default state is
       * the ABSENCE of the key (`shellSettings` reads it as `?? {}`), so a chart with nothing to
       * store must not carry an empty object — every chart in the workspace would grow a
       * `"table.state": {}` on its next save, including the pie and the scalar that cannot read
       * one. `delete` and not merely "don't write": the blob spread above can already contain the
       * key, and the whole point is that what is stored no longer outranks what is on screen.
       * (Emptiness is measured on the KEYS. `{ hiddenColumns: [] }` — what `Show all` leaves
       * behind — is kept as-is: it hydrates back to a state equal to itself, so pruning it further
       * would need a per-key notion of "default" that `TableState` does not have and nothing
       * would render differently for it.)
       *
       * `CardVizPanel` writes the same key from the dashboard's override editor without this
       * clause, and that divergence is deliberate: an override is a sparse patch over the chart's
       * own settings, so `{}` there means "override with nothing" and the panel's Reset button —
       * not a key count — is what removes it.
       */
      if (Object.keys(tableState).length > 0) out["table.state"] = tableState;
      else delete out["table.state"];
      return { version: 2, spec: specForRun(), settings: out };
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
    // ONE NAMESPACE, ONE CONFIG. Bar, line, area, combo, row and scatter are all
    // `cartesianDescriptor`s writing `cartesian.*`, so between them the config already has the
    // new type's shape and there is nothing to translate — dropping it cleared the X-axis and the
    // Y-axis on a bar → line switch (KI-047), which is not what Metabase does either. The sort
    // stays with it for the same reason: it names fields of a spec built from the same config.
    // And so does the result: the same rows draw as the new type at once, and a type whose spec
    // differs (a row chart's default sort) goes stale and re-runs like any other edit.
    if (getDescriptor(v)?.prefix === descriptor?.prefix) {
      if (!id) setSearchParams({ type: v });
      return;
    }
    resetRun();
    // A different viz means a different config shape; its descriptor's default applies.
    setDescState(null);
    // And the sort goes with it. Unlike a filter, which names a DATASET column and stays valid
    // whatever is on screen, a sort names a field of the SPEC — a dimension, a metric or a raw
    // column the descriptor put there (`orderby_sql`, chartquery.rs). Dropping the config drops
    // those, so carrying the sort over would ask the engine to order by a field the new query does
    // not select, which is a 400 rather than a wrong picture.
    setOrderby([]);
    if (!id) setSearchParams({ type: v });
  }


  function resetControls() {
    // `null` drops back to the active descriptor's defaultConfig; the bridge types still
    // need their own hooks cleared by hand until PR3 retires them.
    setDescState(null); setSettings({});
    setColumnSettings({}); setConditionalFormatting([]); setTableState({}); setFilters([]); resetRun();
    setTemporalUnits({}); setOrderby([]);
  }

  /**
   * Is every LOAD-TIME normalization that can move the saved config already a fixed point?
   *
   * `withDefaultTemporalUnits` writes config back on LOAD rather than in response to the author:
   * a temporal dimension with no unit stored on it gets the Metabase-style monthly default, and
   * that write lands one render AFTER the state it normalizes. A baseline captured the instant
   * hydration finishes is therefore a baseline the load itself then moves away from — the chart is
   * dirty before the author has done anything, the same failure as the ref-vs-state bug above but
   * one render later. (Measured: a chart on a date axis saved without units, e.g. anything seeded
   * by the parity-harness script rather than by this editor.)
   *
   * Rather than guess at a settling delay, ask the normalizer itself: it returns its own argument
   * by identity when there is nothing to change, which is exactly the test its effect uses to
   * decide whether to write. Identity here means "that effect has nothing left to do", so the
   * baseline is taken at the first render where that holds.
   *
   * `reconcileTableState` is the OTHER load-time write-back, and it is now in this gate. It was
   * left out when this was written, on two measurements that were true at the time and are worth
   * recording because only one of them survived:
   *   - `paramsForSave` did not emit `tableState` at all, so pruning it could not change
   *     `serializeConfig()` and could not move this flag. That was a defect in its own right — a
   *     table's hidden/reordered/resized columns were not persisted by this editor — and fixing it
   *     (see `paramsForSave`) is what puts this key inside the compared payload and ends that
   *     argument.
   *   - Even patched to serialize it, a mounted table stayed clean in the test harness: the
   *     reconcile's first run is in the same effect flush as hydration, so its functional updater
   *     prunes the seeded value inside the same batch, one render before this baseline is taken.
   *     That holds only because the harness's query is instantaneous. In a browser the result
   *     arrives well after hydration, the prune lands on a baseline already taken, and the chart is
   *     dirty on load with the author having touched nothing — the exact bug this gate exists for,
   *     one query round-trip later. Measured live on the kind deployment, with nothing done to slow
   *     the query down: the chart's own GET completed 423ms after navigation and the result of its
   *     query 600ms after that, and a build without this arm armed the beforeunload guard on load.
   *     `ChartExplore.dirty.test.tsx` fakes that arrival delay, because a mocked query that answers
   *     within the hydration flush cannot reproduce the ordering at all.
   *
   * The arm is not a copy of the effect's guards. Asking `reconcileTableState` with the columns of
   * a result THAT HAS NOT ARRIVED yet — the empty list — answers a different and more useful
   * question: is there anything in this state that a result could ever prune? If not (the common
   * case, including every empty state), it is settled now and the baseline is taken immediately.
   * If there is, the value on screen may still move, so the baseline waits for the result rather
   * than guessing. `data.error` ends that wait: once the query has failed, no result is coming, and
   * a baseline that is never taken is a guard that never arms — silent data loss, which is worse
   * than the extra prompt a later successful run could cause.
   */
  const tableStateSettled =
    viz !== "table"
      // Nothing else renders a `TableState`, so the reconcile effect cannot fire (its own note
      // says why it must not) and there is nothing pending to wait for.
      ? true
      : data.hasResult && resultColumnNames.length > 0
        ? reconcileTableState(tableState, resultColumnNames) === tableState
        : reconcileTableState(tableState, []) === tableState || !!data.error;
  const loadNormalizersSettled =
    withDefaultTemporalUnits(temporalUnits, activeDims, columns) === temporalUnits && tableStateSettled;

  // Capture the "clean" baseline once the builder is ready: for a new chart that's the
  // empty default builder (first render); for a saved chart we wait until hydration has
  // seeded the state (hydratedId === id) and the load-time normalizers have settled, so the
  // baseline reflects the loaded chart as the author is actually looking at it.
  useEffect(() => {
    if (snapshotRef.current !== null) return;
    if (id && hydratedId !== id) return;
    if (!loadNormalizersSettled) return;
    snapshotRef.current = serializeConfig();
  });
  // beforeunload guard only — never gates Save/Run.
  //
  // `snapshotRef.current === null` means the baseline has not been taken yet, and that reads as
  // CLEAN: before the load has settled there is no edit to lose, and treating "not ready" as dirty
  // is what made every chart prompt on the way out. The old note here claimed erring toward an
  // extra prompt was the safe direction; in practice it trained the author to dismiss the dialog,
  // which is strictly worse than not showing it during the second the chart is loading.
  const dirty = snapshotRef.current !== null && serializeConfig() !== snapshotRef.current;
  useUnsavedChangesWarning(dirty);

  // Data/Display builder tab in the URL (?tab=), default "data" (preserves prior behavior).
  const builderTab = searchParams.get("tab") ?? "data";
  // Switching to a viz without rules while that tab is open would leave the panel blank: the
  // trigger disappears but the selected value does not, and no TabsContent matches it.
  useEffect(() => {
    if (!hasConditionalRules && builderTab === "rules") setBuilderTab("display");
  }, [hasConditionalRules, builderTab]);
  // Same for Axes: a pie has none, so switching to one while that tab is open must not strand
  // the panel on a tab whose trigger has just disappeared.
  useEffect(() => {
    if (!showAxesTab && builderTab === "axes") setBuilderTab("display");
  }, [showAxesTab, builderTab]);
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
      {/* Metabase's chart settings sidebar is 400px with 24px padding, giving 352px of content —
          the width every control in this panel takes. Written here ONCE so that 352 is a
          CONSEQUENCE of the panel rather than a number repeated at call sites; repeating it is
          what produced the eight different widths this replaces. */}
      <div className="w-[400px] shrink-0 overflow-y-auto [scrollbar-gutter:stable] border-r p-6 space-y-4">
        <Link className="text-xs text-muted-foreground hover:underline" to={`/explore/${ws}/charts`}>← Charts</Link>

        <VizSwitcher viz={viz} onChange={changeViz} />

        <Tabs value={builderTab} onValueChange={setBuilderTab}>
          <TabsList className={UNDERLINE_LIST}>
            <TabsTrigger value="data" className={UNDERLINE_TRIGGER}>Data</TabsTrigger>
            <TabsTrigger value="display" className={UNDERLINE_TRIGGER}>Display</TabsTrigger>
            {/* Metabase splits Data | Display | Axes, and the axis settings are the half of the
                Display tab a reader goes looking for by name. Only the charts that HAVE axes
                declare anything in that group, so only they get the tab. */}
            {showAxesTab && (
              <TabsTrigger value="axes" className={UNDERLINE_TRIGGER}>Axes</TabsTrigger>
            )}
            {/* Its own tab, the way Metabase splits Columns | Conditional Formatting. Stacked in
                with the rest, it made the pivot's panel the longest in the app and the reason it
                read as cut off. Only table and pivot have rules, so the tab only exists there.

                NAMED AFTER THE FEATURE, NOT AFTER COLOUR. It read "Colors" while the section
                heading inside it read "Conditional formatting" (`SharedDisplaySections`) — one
                feature under two names, and the shorter one describes the effect rather than the
                thing. Metabase's own section is `Conditional Formatting` (`Table.tsx`,
                `PivotTable/settings.ts`); the lower-case f is deliberate, because Metabase is
                itself inconsistent about it (`RuleListing.tsx` writes "Conditional formatting")
                and egret's existing heading already picked one.

                IT FITS, MEASURED — this is the widest label in the strip and the strip does not
                wrap. Read off the live panel: the tab list is 255px with a 16px gap and the label
                renders at 139px in the panel's 13px/500 Inter, against Data at 29 and Display at
                45. 29 + 16 + 45 + 16 + 139 = 245. Only table and pivot reach this branch and
                neither of them draws the Axes tab, so 245 is the worst case rather than a
                best one. */}
            {hasConditionalRules && (
              <TabsTrigger value="rules" className={UNDERLINE_TRIGGER}>Conditional formatting</TabsTrigger>
            )}
          </TabsList>

          {/* ── DATA TAB ── */}
          <TabsContent value="data" className="space-y-4">
            <div className="space-y-1">
              <Label className={FIELD_LABEL}>Dataset</Label>
              <Select value={datasetId ?? ""} onValueChange={(v) => { setDatasetId(v || null); resetControls(); }}>
                <SelectTrigger className="w-full text-sm"><SelectValue placeholder="Pick a dataset…" /></SelectTrigger>
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
                temporalUnits={shownUnits()} columnSettings={columnSettings} unitRow={unitRow} tab="data" />
            )}










            {/* THE COLUMN LIST, ON THE DATA TAB, between the fields it belongs with and the
                Sort/Row-limit that act on them. It had a tab of its own for one commit; beside
                Metrics is where it reads as one question — "which fields does this table select,
                and in what order" — instead of two.
                The dashboard card is untouched: it composes `SharedDisplaySections` itself and
                passes no `only`, so where the BUILDER puts this block is the builder's business.
                THE COST, measured: this tab is 795px tall on a ten-column table against a 783px
                viewport, so it scrolls — and a fifty-column table scrolls a long way. That is a
                list of fifty things being fifty things, not the old defect, which was the same
                names printed three times across two tabs. */}
            {viz === "table" && (
              <SharedDisplaySections
                vizType={viz}
                resultColumns={resultColumns}
                styleColumns={[]}
                tableState={tableState}
                columnSettings={columnSettings}
                conditionalFormatting={conditionalFormatting}
                onTableState={setTableState}
                onColumnSettings={setColumnSettings}
                onConditionalFormatting={setConditionalFormatting}
                queryFields={tableQueryFields}
                only="columns"
              />
            )}


            {/* Sort sits between the fields and the row limit, the order Metabase's notebook puts
                its steps in — and the order that reads correctly, since the sort is what decides
                WHICH rows survive the limit. Not offered where the descriptor would ignore it. */}
            {descriptor && honoursOrderby(descriptor, descConfig) && (
              <SortRows fields={sortableFields(specForRun())} value={orderby} onChange={setOrderby} />
            )}

            {/* THE CEILING IS NOW ON SCREEN. `max={10000}` mirrors the backend's `MAX_CHART_ROWS`
                and did all its work invisibly: measured live, typing 99999 is accepted by the
                control (`min`/`max` on a number input set `validity.rangeOverflow` and block no
                keystroke), the query runs, the backend caps the result at 10,000 and the author is
                shown ten thousand rows having asked for a hundred thousand — with nothing anywhere
                saying a limit exists, let alone that it was applied.

                NOT CLAMPED, deliberately. Rewriting 99999 to 10000 under the cursor would change a
                value the author typed, and this pass is presentation only; the number stays theirs
                and the panel now says what will happen to it. The clamp is worth its own change,
                and is written up as such.

                Not offered where the query ignores it (`honoursRowLimit`): Big number, Progress and
                Gauge always ask for one row. */}
            {(!descriptor || honoursRowLimit(descriptor, descConfig)) && (
            <div className="space-y-1">
              <Label htmlFor="row-limit" className={FIELD_LABEL}>Row limit</Label>
              <Input id="row-limit" type="number" min={1} max={MAX_CHART_ROWS} value={rowLimit}
                aria-describedby="row-limit-hint"
                aria-invalid={rowLimit > MAX_CHART_ROWS || undefined}
                onChange={(e) => { const n = Number(e.target.value); setRowLimit(Number.isNaN(n) ? 0 : n); }}
                className="w-full text-sm" />
              <p id="row-limit-hint" className="text-xs text-muted-foreground">
                {rowLimit > MAX_CHART_ROWS
                  ? `Over the maximum — only ${MAX_CHART_ROWS.toLocaleString()} rows will be returned.`
                  : `Up to ${MAX_CHART_ROWS.toLocaleString()} rows.`}
              </p>
            </div>
            )}
          </TabsContent>

          {/* ── DISPLAY TAB ── */}

          <TabsContent value="display" className="space-y-4">

            {descriptor && (
              <>
                <StudioPanel descriptor={descriptor} config={descConfig} onConfigChange={patchDescConfig}
                  columns={columns} metrics={metrics} resultColumns={resultColumns} rows={data.rows}
                  temporalUnits={shownUnits()} columnSettings={columnSettings} unitRow={unitRow} tab="display" hasMoreBelow />
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
                    // …and the map offers its METRIC only, so its key has to be here for exactly
                    // the same reason the pie's does — `styleColumnsFor`'s map arm filters on
                    // `params.map?.metric`, and filtering against a key this object never carried
                    // matched nothing and hid the accordion outright. CAUGHT IN THE BROWSER, on the
                    // very PR that added the arm: the unit tests around `styleColumnsFor` build a
                    // real `ChartParams`, so nothing below this line is visible to them. That is
                    // twice now for this one object, which is why every arm that filters is listed.
                    map: descConfig as { region?: string; metric?: string },
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
                  queryFields={tableQueryFields}
                  only="format"
                />
              </>
            )}
          </TabsContent>

          {/* ── AXES TAB ── */}
          {showAxesTab && (
            <TabsContent value="axes" className="space-y-4">
              {descriptor && (
                <StudioPanel descriptor={descriptor} config={descConfig} onConfigChange={patchDescConfig}
                  columns={columns} metrics={metrics} resultColumns={resultColumns} rows={data.rows}
                  temporalUnits={shownUnits()} columnSettings={columnSettings} unitRow={unitRow} tab="axes" />
              )}
            </TabsContent>
          )}

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
                  // See the Display tab's copy above: without this the map's arm filters on
                  // `undefined` and the section disappears.
                  map: descConfig as { region?: string; metric?: string },
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
                  // See the Display tab's copy above: without this the map's arm filters on
                  // `undefined` and the section disappears.
                  map: descConfig as { region?: string; metric?: string },
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
