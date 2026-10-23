import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Info, LayoutDashboard, Pencil, Star } from "lucide-react";
import { Link, useParams } from "react-router-dom";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { type LayoutItem } from "react-grid-layout";
import { useDashboard, useDashboardMutations, useDashboardRevisions, type DashboardCard as Card, type DashboardParameter } from "@/api/dashboards";
import { useCharts, type Chart, type ChartFilter } from "@/api/charts";
import { useChartData } from "@/api/useChartData";
import { useDataset, useDatasetsFilterOff } from "@/api/datasets";
import { filterableColumns } from "@/components/dataset/pickableColumns";
import { columnLabels } from "@/components/chart/config/field-names";
import { useFolders } from "@/api/folders";
import { useAuth } from "@/auth/useAuth";
import { resolveClick, type ClickBehavior } from "@/components/chart/dashboard/click-behavior";
import { resolveValuesSource } from "@/components/chart/dashboard/valuesSource";
import { hiddenEmptyCardIds } from "@/components/chart/dashboard/hide-empty";
import { CardEmptinessProbe } from "@/components/chart/dashboard/CardEmptinessProbe";
import { DrillMenu } from "@/components/chart/DrillMenu";
import { DrillRecordsDialog } from "@/components/chart/DrillRecordsDialog";
import { type DrillContext } from "@/components/chart/drill";
import { useDashboardViewControls } from "@/api/useDashboardViewControls";
import { useDashboardParams } from "@/api/useDashboardParams";
import { useFilterDrafts } from "@/api/useFilterDrafts";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { sizeForViz, getPositionForNewDashCard } from "@/components/chart/dashboard-grid";
import { DashboardGridLayout } from "@/components/chart/dashboard/DashboardGridLayout";
import { mayPersistLayout, type GridBreakpoint } from "@/components/chart/dashboard/grid-breakpoints";
import { useIsNarrowViewport } from "@/lib/useIsNarrowViewport";
import { DashboardCardView } from "@/components/chart/dashboard/DashboardCardView";
import { cardChromeClass } from "@/components/chart/dashboard/card-chrome";
import { DashboardInfoSidebar } from "@/components/chart/dashboard/DashboardInfoSidebar";
import { CardFilterMapOverlay } from "@/components/chart/dashboard/CardFilterMapOverlay";
import { cardMapState, columnsForParam } from "@/components/chart/dashboard/card-map-state";
import { findHostCardId, hostTabKey, inlineParametersFor, keepInlineParameters, mergeTopBarOrder, moveDestinations, moveParameterToCard, removeCardAndHostedParameters, topBarParameters, withoutInlineParameters } from "@/components/chart/dashboard/inlineParameters";
import { isDraftDirty, type DashboardDraft } from "@/components/chart/dashboard/edit-draft";
import { REMOVED_CARD, REPLACED_CARD, deletedTab, undoableDraftEdit } from "@/components/chart/dashboard/draft-undo";
import { DiscardChangesDialog } from "@/components/chart/dashboard/DiscardChangesDialog";
import { applyAutoConnect, findAutoConnectTargets, offerAutoConnect, revertAutoConnect } from "@/components/chart/dashboard/autoConnect";
import { DashboardFilterBar } from "@/components/chart/dashboard/DashboardFilterBar";
import { EditSidebar, type EditPanel } from "@/components/chart/dashboard/EditSidebar";
import { EditFilterChips } from "@/components/chart/dashboard/EditFilterChips";
import { CardEditToolbar } from "@/components/chart/dashboard/CardEditToolbar";
import { DashboardEditToolbar } from "@/components/chart/dashboard/DashboardEditToolbar";
import { RefreshMenu } from "@/components/chart/dashboard/RefreshMenu";
import { DashboardViewActionRows, dashboardWidthClass } from "@/components/chart/dashboard/DashboardActionsMenu.view";
import type { SectionLayout } from "@/components/chart/dashboard/dashboard-sections";
import { dropPlaceholders } from "@/components/chart/dashboard/dashboard-sections";
import { PlaceholderCard } from "@/components/chart/dashboard/PlaceholderCard";
import { slugify, applyParamsToChart, paramFiltersForCard, temporalOverridesForCard, computeLinkedFilters, isBlank } from "@/components/chart/dashboard/params";
import { useColumnValues } from "@/api/useColumnValues";
import { useDashboardTabs } from "@/api/useDashboardTabs";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { DashboardShortcuts } from "@/lib/useDashboardShortcuts";
import { DashboardTabBar } from "@/components/chart/dashboard/DashboardTabBar";
import { ChartPickerList } from "@/components/chart/dashboard/ChartPickerList";
import { startTabsDraft, serializeLayout, addTab, renameTab, reorderTab, deleteTab, duplicateTab, moveCardToTab, type DraftTab, type DraftCard } from "@/components/chart/dashboard/tabs";
import { toast } from "sonner";
import { PageTitle } from "@/components/layout/PageTitle";

/** Loads distinct column values for one category/id parameter (hook is called
 * unconditionally inside this child so the Rules of Hooks are respected). */
function ParamOptions({ ws, datasetId, column, slug, filters, onLoaded }: { ws: string; datasetId: string | null; column: string | null; slug: string; filters: ChartFilter[]; onLoaded: (slug: string, values: string[]) => void }) {
  const { values } = useColumnValues(ws, datasetId, column, !!datasetId && !!column, filters);
  useEffect(() => { if (values.length) onLoaded(slug, values); }, [slug, values, onLoaded]);
  return null;
}

/** Emits a static value list on mount (no query) for a 'static' value source. */
function StaticOptions({ slug, values, onLoaded }: { slug: string; values: string[]; onLoaded: (slug: string, v: string[]) => void }) {
  useEffect(() => { onLoaded(slug, values); }, [slug, values, onLoaded]);
  return null;
}

export default function DashboardView() {
  const { workspace: ws, id } = useParams() as { workspace: string; id: string };
  const { data: dash, isLoading, isError } = useDashboard(ws, id);
  const { data: charts } = useCharts(ws);
  const { data: dashboardFolders = [] } = useFolders(ws, "dashboards");
  const { update, restore: restoreMut, addBookmark, removeBookmark, revert, setCertification } = useDashboardMutations(ws);
  // PLATFORM-scoped: holding dashboard:manage on this dashboard grants nothing here. Mirrors
  // RequirePermission (guards.tsx:59) — platform_admin always passes.
  const { me } = useAuth();
  const canCertify = !!me && (me.platform_roles.includes("platform_admin") || me.permissions.includes("dashboard:certify"));
  const [editing, setEditing] = useState(false);
  const [showInfo, setShowInfo] = useState(false);
  // The history query stays disabled until the History tab is opened, so an info sidebar opened
  // on Overview costs nothing.
  const [historyOpen, setHistoryOpen] = useState(false);
  const revisionsQ = useDashboardRevisions(ws, id, showInfo && historyOpen);
  const [draftCards, setDraftCards] = useState<DraftCard[] | null>(null);
  const [draftParameters, setDraftParameters] = useState<DashboardParameter[] | null>(null);
  const [draftTabs, setDraftTabs] = useState<DraftTab[] | null>(null);
  const [draftName, setDraftName] = useState("");
  const [draftWidth, setDraftWidth] = useState<"fixed" | "full">("full");
  // The header row's rule keys off the VIEWPORT (same hook as the app shell), not off the grid's
  // measured container — the two answer different questions and must not be collapsed.
  const narrowViewport = useIsNarrowViewport();
  // A ref, not state, and it starts at "mobile" — both are load-bearing.
  //
  // A ref because the two writers it guards are called from ResizeObserver callbacks, which all
  // fire in one batch before React re-renders: state read from a render closure would still say
  // "desktop" while the card being measured is already phone-width. `DashboardGridLayout`
  // updates this from inside its own observer, in that same batch.
  //
  // "mobile" because the grid measures its container in an effect, so its FIRST render is always
  // width 0 → the one-column projection, and RGL emits that collapsed layout before any effect
  // can report the real breakpoint. Defaulting to "desktop" would let a fresh mount inside edit
  // mode (empty dashboard → add a card) flatten everything to x:0 w:1.
  const gridBreakpointRef = useRef<GridBreakpoint>("mobile");
  const onGridBreakpointChange = useCallback((bp: GridBreakpoint) => {
    gridBreakpointRef.current = bp;
  }, []);
  const serverTabCount = dash?.tabs?.length ?? 0;
  const { activeTabPos, setActiveTabPos } = useDashboardTabs(editing ? (draftTabs?.length ?? 0) : serverTabCount);
  // beforeunload guard: warn on refresh/close while the dashboard editor is open (unsaved draft).
  useUnsavedChangesWarning(editing);
  const cardSeqRef = useRef(0);
  const interactingRef = useRef(false);
  // What the draft looked like when Edit was pressed. Cancel compares against this, and a null
  // baseline means "not editing" — never "clean", so a missing snapshot cannot silently disarm
  // the confirmation.
  const editBaselineRef = useRef<DashboardDraft | null>(null);
  const [confirmingDiscard, setConfirmingDiscard] = useState(false);
  // "Hide this card if there are no results": id -> did that card's query return no rows.
  // Filled by CardEmptinessProbe, which runs the query OUTSIDE the grid so that hiding a
  // card does not unmount the only hook that could ever un-hide it. Absent = not yet
  // known, which hide-empty.ts reads as hidden.
  const [cardEmptiness, setCardEmptiness] = useState<Record<string, boolean>>({});
  const reportCardEmpty = useCallback((cardId: string, empty: boolean) => {
    // Bail on an unchanged value so a re-render of the probe cannot loop the parent.
    setCardEmptiness((prev) => (prev[cardId] === empty ? prev : { ...prev, [cardId]: empty }));
  }, []);
  const chartById = useMemo(() => new Map((charts ?? []).map((c) => [c.id, c])), [charts]);
  const view = useDashboardViewControls();
  // Hook must run before the early return — use `dash?` since dash may be undefined while loading.
  const paramSource = editing && draftParameters ? draftParameters : (dash?.parameters ?? []);
  const paramsState = useDashboardParams(paramSource);
  // Draft/Apply layer over the URL params. `appliedValues` is what the CARDS run on; `editorValues`
  // is what the filter bar shows. Named `filterDrafts`, not `filters`, because `filters` is already
  // a local inside the ParamOptions map below.
  const filterDrafts = useFilterDrafts(id, paramsState);
  const [editPanel, setEditPanel] = useState<EditPanel>(null);
  const [replacingId, setReplacingId] = useState<string | null>(null);
  const [fillingPlaceholderId, setFillingPlaceholderId] = useState<string | null>(null);
  const [newCardId, setNewCardId] = useState<string | null>(null);
  const openPanel = useCallback((p: EditPanel) => { setEditPanel(p); }, []);
  const [deletingTabKey, setDeletingTabKey] = useState<string | null>(null);
  const [reassignTo, setReassignTo] = useState<string | null>(null);
  // category/id dropdown options, keyed by param slug; populated by <ParamOptions> children.
  const [optionsBySlug, setOptionsBySlug] = useState<Record<string, string[]>>({});
  const onOptionsLoaded = useCallback((slug: string, vals: string[]) => {
    setOptionsBySlug((m) => (m[slug]?.join("") === vals.join("") ? m : { ...m, [slug]: vals }));
  }, []);
  // Drill state — must be declared above the early return (Rules of Hooks).
  const [drillMenu, setDrillMenu] = useState<{ ctx: DrillContext; x: number; y: number; datasetId: string | null } | null>(null);
  const [records, setRecords] = useState<{ filters: ChartFilter[]; datasetId: string | null } | null>(null);
  // Load the drilled dataset's columns so the records dialog can SELECT them (no-op while records is null).
  const recordsDataset = useDataset(ws, records?.datasetId ?? null);

  // ── WHAT A COLUMN IS CALLED IN THE EDIT PANELS ──────────────────────────────────────────────
  //
  // A chart's spec carries column NAMES only (`string[]`), so a picker built from it shows
  // `region` where the rest of the product shows "Khu vực". The friendly name lives on
  // `DatasetColumn.verbose_name`, which means loading the dataset behind the card.
  //
  // Both panels below show one card's columns at a time, so this is ONE dataset each, and each
  // query is inert until its panel is open. The filter→card MAPPING overlay is deliberately not
  // fixed here: it draws on every card at once, so it would need one dataset per distinct card —
  // see the note at its call site.
  const panelChartDatasetId = (() => {
    if (editPanel?.kind !== "card") return null;
    const c = dash?.cards.find((x) => x.id === editPanel.cardId);
    return chartById.get(c?.chart_id ?? "")?.dataset_id ?? null;
  })();
  const panelDataset = useDataset(ws, panelChartDatasetId);
  const panelLabels = useMemo(
    () => columnLabels(panelDataset.data?.columns ?? []),
    [panelDataset.data],
  );

  // A dashboard filter maps only onto card columns whose dataset has Filter ON (KI-096). Loaded
  // only while a filter's mapping is being edited, since that is the only reader.
  const filterOffByDataset = useDatasetsFilterOff(
    ws,
    (dash?.cards ?? []).map((c) => chartById.get(c.chart_id ?? "")?.dataset_id).filter((x): x is string => !!x),
    editing && editPanel?.kind === "filter",
  );
  /** `columnsForParam` for one chart, without the columns its dataset marks not filterable. */
  const mappableColumns = (chart: Chart, paramType: DashboardParameter["type"], current?: string | null) =>
    filterableColumns(columnsForParam(chart.params.spec, paramType), filterOffByDataset.get(chart.dataset_id), current);

  // The value-source picker names a card first, then one of ITS columns — so the labels follow
  // the card the user picked, not the card whose settings are open.
  const valuesSourceDatasetId = (() => {
    if (editPanel?.kind !== "filter") return null;
    const p = (dash?.parameters ?? []).find((x) => x.id === editPanel.paramId);
    const srcCardId = p?.valuesSource?.type === "card" ? p.valuesSource.cardId : null;
    const c = dash?.cards.find((x) => x.id === srcCardId);
    return chartById.get(c?.chart_id ?? "")?.dataset_id ?? null;
  })();
  const valuesSourceDataset = useDataset(ws, valuesSourceDatasetId);
  const valuesSourceLabels = useMemo(
    () => columnLabels(valuesSourceDataset.data?.columns ?? []),
    [valuesSourceDataset.data],
  );

  // ── THE CARD SETTINGS PANEL'S VIEW OF ITS OWN CARD'S RESULT ─────────────────────────────────
  //
  // Three of the panel's controls read the RESULT, not the saved spec, and every one of them
  // degrades SILENTLY to a wrong-but-plausible answer when it does not get one. This route used to
  // hand the panel a column list reconstructed from `params.spec` — names only, `type: ""` for
  // every dimension — and no rows at all, with a comment saying result columns "aren't readily
  // available in DashboardView". They are; the card beside the panel has already run this exact
  // query to draw itself. What that omission cost, measured against the builder route on the same
  // charts:
  //
  //   - THE GAUGE'S RANGES, and this is the one that destroys a card. `GaugeDisplayControl` sizes
  //     its default zones from `rows[0][metricIndex]`; with no rows that is `0`, and
  //     `defaultGaugeSegments(0)` returns the 0-25 / 25-50 / 50-100 placeholder. So the panel
  //     offered 0/25/50/100 next to an arc drawn on 0 - 3,021,243.38 — and because
  //     `SegmentRangesSection` emits the WHOLE array on any edit, nudging one bound wrote all three
  //     placeholder rows into the card's override and re-scaled the card to 0-100 with a 1.5M
  //     metric. Destructive, from a control the panel gave the user every reason to trust.
  //   - EVERY FORMAT POPOVER. `ColumnFormatFields` branches on `isNumericType(c.type)` /
  //     `isTemporalType(c.type)`, and `""` is neither — so a date dimension and a numeric
  //     non-metric column both rendered as Text with nothing but "Column title", and a date format
  //     authored in the builder could not be edited from the dashboard at all.
  //   - THE PIE'S SLICES. `PieDisplayControl` derives its per-slice rows with
  //     `pieSliceRows(resultColumns, rows, …)`; no rows means no slices, so the reorder / colour /
  //     rename / hide block simply was not there.
  //
  // THE INJECTED CHART, NOT THE SAVED ONE. `applyParamsToChart` is what the edit-mode tile renders
  // (see `injected` in the card map below), so keying on it means two things at once: the panel
  // describes the numbers actually on screen under a dashboard filter, and `useSpecData`'s query key
  // (`JSON.stringify(spec)`) is byte-identical to the tile's, making this a react-query CACHE READ
  // rather than a second round trip. Keying on the base chart would have looked identical on a
  // dashboard with no filters and been wrong on both counts on one with them.
  //
  // These hooks sit above the `isLoading` early return because they have to run unconditionally;
  // everything they read (`dash`, `draftCards`, `chartById`, `paramSource`, `editPanel`) is declared
  // above it for the same reason. `enabled` is false unless the viz panel is actually open, so a
  // dashboard nobody is editing runs nothing extra.
  const vizPanelChart = useMemo(() => {
    if (editPanel?.kind !== "viz") return null;
    const list = (editing && draftCards ? draftCards : dash?.cards) ?? [];
    const card = list.find((c) => c.id === editPanel.cardId);
    const base = card ? chartById.get(card.chart_id ?? "") : undefined;
    if (!card || !base) return null;
    return applyParamsToChart(base, paramSource, paramsState.values, (card.options.paramMappings as Record<string, string>) ?? {});
  }, [editPanel, editing, draftCards, dash?.cards, chartById, paramSource, paramsState.values]);
  const vizPanelData = useChartData(ws, vizPanelChart, view.refreshNonce, !!vizPanelChart);

  useEffect(() => {
    setEditing(false);
    setDraftCards(null);
    setDraftParameters(null);
    setDraftTabs(null);
    setDraftName("");
    setNewCardId(null);
    editBaselineRef.current = null;
    setConfirmingDiscard(false);
  }, [id]);

  if (isError) {
    return (
      <div className="p-4">
        <Alert variant="destructive">
          <AlertDescription>
            Failed to load this dashboard. Please try again.{" "}
            <Link to={`/explore/${ws}/dashboards`} className="font-medium underline">
              Back to dashboards
            </Link>
          </AlertDescription>
        </Alert>
      </div>
    );
  }
  if (isLoading || !dash) return <div className="p-4 text-muted-foreground">Loading…</div>;

  const dashboard = dash;
  const effectiveWidth = editing ? draftWidth : dashboard.width;
  const cards = editing && draftCards ? draftCards : dashboard.cards;
  // Fallback DraftCard[] for the draft setters (each runs only while editing, so draftCards is
  // non-null and this is effectively dead — but it keeps the setters typed as DraftCard[]).
  const cardsAsDraft = (): DraftCard[] => dashboard.cards.map((c) => ({ ...c, tabKey: c.tab_id }));
  const parameters = editing && draftParameters ? draftParameters : (dashboard.parameters ?? []);
  const tabs: DraftTab[] = editing
    ? (draftTabs ?? [])
    : (dashboard.tabs ?? []).map((t, i) => ({ key: t.id, name: t.name, position: i }));
  const activeTab = tabs[activeTabPos] ?? null;
  const tabCards = tabs.length === 0
    ? cards
    : cards.filter((c) =>
        editing ? (c as DraftCard).tabKey === activeTab?.key : c.tab_id === activeTab?.key,
      );
  const requiredUnset = parameters.some((p) => {
    const v = filterDrafts.appliedValues[p.slug];
    return p.required && (Array.isArray(v) ? v.filter((x) => !isBlank(x)).length === 0 : isBlank(v));
  });
  // Drop the cards that asked to hide when empty and came back with nothing. This has to
  // bite on the SAME list that feeds both the RGL `layout` array and the grid's children,
  // or the two disagree and RGL renders a ghost slot.
  const hiddenIds = hiddenEmptyCardIds(tabCards, cardEmptiness, editing, requiredUnset);
  const visibleCards = hiddenIds.size === 0 ? tabCards : tabCards.filter((c) => !hiddenIds.has(c.id));
  // Cards to keep querying regardless of whether their tile is on screen. The predicate
  // MUST agree with hide-empty.ts's, or a card can be hidden with nothing left running to
  // un-hide it. Only the opted-in ones, so an ordinary board issues not one extra hook.
  const probeCards = editing ? [] : tabCards.filter((c) => c.card_type === "chart" && !!c.chart_id && !!c.options.hideEmpty);
  // A board whose every card hid itself is not the same as an empty board, and saying
  // "No cards yet. Click Edit to add charts." there would be a lie.
  const allHiddenByEmpty = !editing && tabCards.length > 0 && visibleCards.length === 0;

  /**
   * The arguments a card's data query is keyed on. The tile and its emptiness probe MUST
   * pass identical values or react-query treats them as two different queries and the
   * board issues the request twice — so both callers read them from here rather than
   * computing their own.
   */
  function cardQueryArgs(c: Card) {
    const baseChart = chartById.get(c.chart_id ?? "");
    const mappings = (c.options.paramMappings as Record<string, string>) ?? {};
    return {
      baseChart,
      mappings,
      // Resolved dashboard-param filters for this card — sent to the view-mode
      // card-data endpoint so filtering is applied server-side (validated).
      paramFilters: baseChart ? paramFiltersForCard(parameters, filterDrafts.appliedValues, mappings) : [],
      // Resolved temporal-unit overrides for this card — sent to the view-mode
      // card-data endpoint so a "Time grouping" param re-grids server-side.
      paramTemporal: baseChart ? temporalOverridesForCard(parameters, filterDrafts.appliedValues, mappings, baseChart.params.spec) : {},
    };
  }

  function onCardValueClick(card: Card, ctx: DrillContext, x: number, y: number) {
    const cb = card.options.clickBehavior as ClickBehavior | undefined;
    const r = resolveClick(cb, ctx, parameters, filterDrafts.appliedValues);
    const datasetId = chartById.get(card.chart_id ?? "")?.dataset_id ?? null;
    if (r.kind === "url") { if (r.href) window.open(r.href, "_blank", "noreferrer"); return; }
    // ONE write, not a loop: a crossfilter click can set several parameters, and `setValue` per
    // slug loses all but the last while auto-apply is on — react-router hands every functional
    // updater the same current location, so they overwrite instead of accumulating.
    if (r.kind === "crossfilter") { filterDrafts.setValues(Object.fromEntries(r.sets.map((s) => [s.slug, s.value]))); return; }
    setDrillMenu({ ctx, x, y, datasetId });
  }

  function startEdit() {
    const { draftTabs: dt, draftCards: dc } = startTabsDraft(dashboard.tabs ?? [], dashboard.cards);
    const dp = (dashboard.parameters ?? []).map((p) => ({ ...p }));
    setDraftCards(dc);
    setDraftParameters(dp);
    setDraftTabs(dt);
    setDraftName(dashboard.name);
    setDraftWidth(dashboard.width);
    setEditing(true);
    setEditPanel(null);
    // The snapshot Cancel compares against. Taken from the DRAFT, not the server payload:
    // startTabsDraft mints fresh client tab keys, so the two are not the same shape.
    editBaselineRef.current = { name: dashboard.name, width: dashboard.width, cards: dc, parameters: dp, tabs: dt };
  }

  /** Leave edit mode and drop the draft. This is the destructive half — reachable only from a
   *  clean draft, or through the confirmation. */
  function discardEdit() {
    setEditing(false); setDraftCards(null); setDraftParameters(null); setDraftTabs(null);
    setEditPanel(null); setDraftName(""); setNewCardId(null);
    editBaselineRef.current = null;
    setConfirmingDiscard(false);
  }

  /** What the Cancel button calls. Metabase routes Cancel through its leave confirmation whenever
   *  the dashboard is dirty (DashboardHeader.tsx:69-87) and exits straight away when it is not. */
  function requestCancelEdit() {
    const baseline = editBaselineRef.current;
    const current: DashboardDraft = {
      name: draftName, width: draftWidth, cards: (cards as DraftCard[]), parameters, tabs: draftTabs ?? [],
    };
    if (baseline && isDraftDirty(baseline, current)) { setConfirmingDiscard(true); return; }
    discardEdit();
  }

  function addCard(chart: Chart) {
    const { w, h } = sizeForViz(chart.viz_type).default;
    const pos = getPositionForNewDashCard(cards, w, h);
    const newCard: DraftCard = {
      id: `new-${chart.id}-${cardSeqRef.current++}`, chart_id: chart.id, card_type: "chart",
      grid_col: pos.grid_col, grid_row: pos.grid_row, size_x: w, size_y: h, options: {}, tab_id: null,
      tabKey: activeTab?.key ?? null,
    };
    setDraftCards([...(cards as DraftCard[]), newCard]);
  }
  function removeCard(cardId: string) {
    // A card's inline filters are deleted WITH the card, matching Metabase. The undo snapshot is
    // therefore BOTH slices: W2's toast restoring only the cards would bring back a card holding
    // parameter ids that no longer name anything — a half-undo that looks like it worked.
    const previous = { cards: cards as DraftCard[], parameters };
    const next = removeCardAndHostedParameters(previous.cards, previous.parameters, cardId);
    setDraftCards(next.cards);
    if (next.parameters !== previous.parameters) setDraftParameters(next.parameters);
    undoableDraftEdit(REMOVED_CARD, previous, (p) => { setDraftCards(p.cards); setDraftParameters(p.parameters); });
  }

  function duplicateCard(cardId: string) {
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const src = base.find((c) => c.id === cardId);
      if (!src) return base;
      const maxY = base.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
      // The clone gets NO inline filters. egret's clone is a shallow spread, so without the strip
      // two cards would claim one parameter id, `findHostCardId` would pick the first, and the
      // clone would draw nothing. Deliberate divergence from Metabase, which duplicates the
      // parameters themselves (cards-typed.ts:416-441) — see inlineParameters.ts.
      return [...base, { ...src, id: `new-dup-${cardSeqRef.current++}`, grid_row: maxY, options: withoutInlineParameters({ ...src.options }) }];
    });
  }
  function replaceCard(cardId: string, chart: Chart) {
    // Resetting `options` is correct and is NOT the divergence — Metabase discards more than this
    // (cards-typed.ts:245-284). What was missing is the way back, which is the toast.
    //
    // `inlineParameters` is the ONE key that survives the reset: the filter list is a PLACEMENT,
    // not a property of the chart, and Metabase's own replaceCard leaves `inline_parameters` alone
    // (cards-typed.ts:262-270). Without this, "Replace chart" silently teleports the card's filter
    // back to the top bar.
    const previous = cards as DraftCard[];
    setDraftCards(previous.map((c) => (c.id === cardId ? { ...c, chart_id: chart.id, options: keepInlineParameters(c.options) } : c)));
    setReplacingId(null);
    undoableDraftEdit(REPLACED_CARD, previous, setDraftCards);
  }

  /** Delete a tab from the draft and offer an Undo, as Metabase does (use-dashboard-tabs.ts:43-61).
   *  All three delete paths — the no-cards fast path and the dialog's two buttons — funnel through
   *  here so the snapshot is taken in exactly one place; three inline copies were three chances to
   *  snapshot the state after the edit instead of before it. */
  function deleteTabWithUndo(key: string, opts: { mode: "reassign"; toKey: string } | { mode: "delete" }) {
    const previous = { tabs: draftTabs ?? [], cards: cards as DraftCard[], activeTabPos };
    const name = previous.tabs.find((t) => t.key === key)?.name ?? "";
    const r = deleteTab(previous.tabs, previous.cards, key, opts);
    setDraftTabs(r.draftTabs);
    setDraftCards(r.draftCards);
    setActiveTabPos(Math.min(activeTabPos, r.draftTabs.length - 1));
    undoableDraftEdit(deletedTab(name), previous, (p) => {
      setDraftTabs(p.tabs); setDraftCards(p.cards); setActiveTabPos(p.activeTabPos);
    });
  }

  function fillPlaceholder(cardId: string, chart: Chart) {
    setDraftCards((prev) =>
      (prev ?? cardsAsDraft()).map((c) =>
        c.id === cardId ? { ...c, card_type: "chart", chart_id: chart.id, options: {} } : c,
      ),
    );
    setFillingPlaceholderId(null);
  }

  function addVirtualCard(card_type: "text" | "heading" | "link" | "iframe", options: Record<string, unknown>) {
    const id = `new-${card_type}-${cardSeqRef.current++}`;
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const { w, h } = sizeForViz(card_type).default;
      const pos = getPositionForNewDashCard(base, w, h);
      const newCard: DraftCard = {
        id, chart_id: null, card_type,
        grid_col: pos.grid_col, grid_row: pos.grid_row, size_x: w, size_y: h, options, tab_id: null,
        tabKey: activeTab?.key ?? null,
      };
      return [...base, newCard];
    });
    if (card_type === "text" || card_type === "heading") setNewCardId(id);
    // An iframe card has no inline editor, so the URL is entered in the settings panel. Open it
    // on add, the way addFilter does for a new filter.
    if (card_type === "iframe") openPanel({ kind: "card", cardId: id });
  }
  function addFilter() {
    const slug = slugify("filter", parameters.map((p) => p.slug));
    const id = `new-${slug}-${cardSeqRef.current++}`;
    setDraftParameters([...parameters, { id, name: "Filter", slug, type: "text", operator: "is" }]);
    openPanel({ kind: "filter", paramId: id });
  }

  function addSection(layout: SectionLayout) {
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const onTab = base.filter((c) => c.tabKey === (activeTab?.key ?? null));
      const row = onTab.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
      const built: DraftCard[] = layout.build({ col: 0, row }).map((s) => ({
        id: `new-section-${cardSeqRef.current++}`,
        chart_id: null,
        card_type: s.card_type,
        grid_col: s.grid_col,
        grid_row: s.grid_row,
        size_x: s.size_x,
        size_y: s.size_y,
        options: s.card_type === "heading" ? { text: "" } : {},
        tab_id: null,
        tabKey: activeTab?.key ?? null,
      }));
      return [...base, ...built];
    });
  }

  function updateCardOptions(cardId: string, patch: Record<string, unknown>) {
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      return base.map((c) => (c.id === cardId ? { ...c, options: { ...c.options, ...patch } } : c));
    });
  }

  /** Auto-connect: mapping ONE card offers to map every other card on this tab that has a column
   *  of the same name. Resolution, the patch, its inverse and both toasts live in autoConnect.ts;
   *  this is the wiring that gives them the draft. */
  function offerAutoConnectFor(sourceCardId: string, paramId: string, column: string) {
    const paramType = parameters.find((p) => p.id === paramId)?.type ?? "text";
    const columnsOf = (cardId: string) => {
      const chart = chartById.get((cards as DraftCard[]).find((x) => x.id === cardId)?.chart_id ?? "");
      return chart ? mappableColumns(chart, paramType) : [];
    };
    const targets = findAutoConnectTargets({
      cards: cards as DraftCard[], columnsOf, paramId, column, sourceCardId,
      tabKey: activeTab?.key ?? null,
    });
    offerAutoConnect({
      targets, columnName: column, hasMultipleTabs: tabs.length > 1,
      connect: () => setDraftCards((prev) => applyAutoConnect(prev ?? cardsAsDraft(), paramId, targets)),
      revert: () => setDraftCards((prev) => revertAutoConnect(prev ?? cardsAsDraft(), paramId, targets)),
    });
  }

  // The cards are the single source of truth for positions: the RGL layout is derived
  // from them, and drag/resize writes positions straight back into the draft. This keeps
  // every grid item sized from its card (a newly added card is never auto-placed at 1x1).
  const vizKeyOf = (c: DraftCard) => (c.card_type === "chart" ? (chartById.get(c.chart_id ?? "")?.viz_type ?? "") : c.card_type);
  const layout: LayoutItem[] = visibleCards.map((c) => {
    const autoH = !!c.options.autoHeight;
    const min = sizeForViz(vizKeyOf(c as DraftCard)).min;
    return {
      i: c.id, x: c.grid_col, y: c.grid_row, w: c.size_x, h: c.size_y,
      // clamp mins to the card's own size so RGL never warns (minW/minH > current).
      minW: Math.min(min.w, c.size_x),
      minH: autoH ? c.size_y : Math.min(min.h, c.size_y),
      ...(autoH ? { maxH: c.size_y } : {}),
    };
  });

  function applyAutoHeight(cardId: string, rows: number) {
    if (interactingRef.current) return;            // never fight an active drag/resize
    // A mobile card is a different width, so its measured content height belongs to a layout
    // we are not editing. Writing it back would corrupt the desktop rows.
    if (!mayPersistLayout(editing, gridBreakpointRef.current)) return;
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      let changed = false;
      const merged = base.map((c) => {
        if (c.id !== cardId || c.size_y === rows) return c;
        changed = true;
        return { ...c, size_y: rows };
      });
      return changed ? merged : base;
    });
  }

  function onLayoutChange(next: readonly LayoutItem[]) {
    if (!mayPersistLayout(editing, gridBreakpointRef.current)) return;
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      let changed = false;
      const merged = base.map((c) => {
        const l = next.find((x) => x.i === c.id);
        if (!l) return c;
        const row = Math.max(0, l.y);
        if (l.x === c.grid_col && row === c.grid_row && l.w === c.size_x && l.h === c.size_y) return c;
        changed = true;
        return { ...c, grid_col: l.x, grid_row: row, size_x: l.w, size_y: l.h };
      });
      return changed ? merged : base;   // same ref when nothing moved → no re-render loop
    });
  }

  async function onSave() {
    if (!draftName.trim()) { toast.error("Dashboard name can't be empty"); return; }
    const persistCards = dropPlaceholders(cards as DraftCard[]);
    const { tabs: tabPayload, cards: cardPayload } = serializeLayout(draftTabs ?? [], persistCards);
    try {
      await update.mutateAsync({ id: dashboard.id, name: draftName.trim() || dashboard.name, description: dashboard.description, parameters, tabs: tabPayload, width: draftWidth, cards: cardPayload });
      setEditing(false); setDraftCards(null); setDraftTabs(null); setEditPanel(null); setDraftName(""); setNewCardId(null);
      editBaselineRef.current = null;
      toast.success("Saved");
    } catch (e) { toast.error(String(e)); }
  }

  // Description-only save from the info sidebar: rebuild the SAME whole-layout PUT body from the
  // server dashboard (single source of truth in view mode) via the same startTabsDraft +
  // serializeLayout machinery the Save button uses, so the layout is preserved unchanged and only
  // `description` differs.
  function saveDescription(next: string) {
    const { draftTabs: dt, draftCards: dc } = startTabsDraft(dashboard.tabs ?? [], dashboard.cards);
    const { tabs: tabPayload, cards: cardPayload } = serializeLayout(dt, dc);
    update
      .mutateAsync({ id: dashboard.id, name: dashboard.name, description: next, parameters: dashboard.parameters ?? [], tabs: tabPayload, width: dashboard.width, cards: cardPayload })
      .then(() => toast.success("Saved"))
      .catch((e) => toast.error(String(e)));
  }

  const activeParamId = editing && editPanel?.kind === "filter" ? editPanel.paramId : null;
  // Only the TWO widget lists get the split. `cardQueryArgs`, `applyParamsToChart`,
  // `interpolateParams`, `resolveValuesSource`, `computeLinkedFilters` and the required-filter
  // check keep reading the full `parameters` array, because moving a filter onto a card changes
  // where its widget is DRAWN and nothing about what it filters (Metabase subtracts inline
  // parameters from the header list alone — selectors.ts:447-454). Feeding this anywhere else
  // silently un-applies the filter, and the symptom is a card that quietly stops being filtered
  // rather than an error.
  const topBar = topBarParameters(parameters, cards);
  const activeParam = activeParamId ? parameters.find((p) => p.id === activeParamId) : undefined;
  // Per-card column lists for the filter value-source "card" picker (label + selectable columns).
  const cardColumns = cards
    .filter((c) => c.card_type === "chart")
    .map((c) => {
      const ch = chartById.get(c.chart_id ?? "");
      return { cardId: c.id, label: ch?.name ?? "Card", columns: ch ? [...ch.params.spec.dimensions, ...ch.params.spec.columns] : [] };
    });

  // Per-card viz override panel data, built on the card's OWN RESULT — see the long note beside
  // `vizPanelChart` above for the three controls that read it and what each of them showed instead.
  const vizForPanel = (() => {
    if (editPanel?.kind !== "viz") return undefined;
    const card = (cards as DraftCard[]).find((c) => c.id === editPanel.cardId);
    const chart = card ? chartById.get(card.chart_id ?? "") : undefined;
    if (!card || !chart) return undefined;
    // `__grp_` columns are the grouping-set bookkeeping the pivot's compiler adds to the projection;
    // the builder route strips them from `resultColumns` too (`ChartExplore`), so both editors offer
    // the same list and neither shows a column the user never asked for.
    const resultColumns = vizPanelData.columns.filter((c) => !c.name.startsWith("__grp_"));
    return {
      // The injected chart when we have it, so `CardVizPanel`'s `spec.temporal_units` — which is
      // what tells the pie's slice list whether the ring keys are "Mon" or `1` — is the bucketing
      // the ROWS came back under rather than the saved one a "Time grouping" param has re-gridded.
      chart: vizPanelChart ?? chart,
      override: card.options.vizOverride as Record<string, unknown> | undefined,
      // The spec-derived list survives ONLY as the degenerate fallback: a card whose query has not
      // landed yet, or errored, has nothing to show and this at least keeps the Formatting
      // accordion populated with the right NAMES, exactly as before. It cannot desynchronise the
      // rows from the columns, because `columns` and `rows` come from one response — an empty
      // `columns` implies an empty `rows`, so no control can index a real row against a fake
      // column list. Metrics are typed `double` here for the same reason they always were:
      // without them a cartesian card's Formatting section lists nothing at all.
      columns: resultColumns.length > 0 ? resultColumns : [
        ...chart.params.spec.dimensions.map((name) => ({ name, type: "" })),
        ...chart.params.spec.columns.map((name) => ({ name, type: "" })),
        ...chart.params.spec.metrics.map((name) => ({ name, type: "double" })),
      ],
      rows: vizPanelData.rows,
    };
  })();

  return (
    <div className={view.isFullscreen ? "fixed inset-0 z-50 overflow-auto bg-dashboard p-4" : "min-h-full bg-dashboard p-4"}>
      {/* `requestCancelEdit`, NOT `discardEdit`: the `e` shortcut must take the same path as the
          Cancel button — through the discard confirmation — or a keystroke silently destroys a
          dirty draft, which is the exact loss that confirmation exists to prevent. */}
      <DashboardShortcuts
        editing={editing}
        tabCount={tabs.length}
        onEdit={startEdit}
        onCancelEdit={requestCancelEdit}
        onSave={onSave}
        onToggleBookmark={() => { (dash.bookmarked ? removeBookmark : addBookmark).mutateAsync(dash.id).catch((e) => toast.error(String(e))); }}
        onToggleInfo={() => setShowInfo((v) => !v)}
        onAddFilter={addFilter}
        onAddCard={() => openPanel({ kind: "add" })}
        onSelectTab={setActiveTabPos}
      />
      {editing && (
        <div className="-mx-4 -mt-4 mb-4 flex items-center justify-between gap-2 bg-primary/90 px-4 py-2 text-sm font-medium text-primary-foreground">
          <span className="flex items-center gap-2"><Pencil className="h-4 w-4" /> You're editing this dashboard</span>
          <span className="flex items-center gap-2">
            <Button size="sm" variant="ghost" className="text-primary-foreground hover:bg-primary-foreground/15 hover:text-primary-foreground" onClick={requestCancelEdit}>Cancel</Button>
            <Button size="sm" variant="secondary" onClick={onSave} disabled={update.isPending}>Save</Button>
          </span>
        </div>
      )}
<div className={dashboardWidthClass(effectiveWidth)}>
      {dash.deleted_at && (
        <div className="mb-3 flex items-center justify-between rounded-md border border-highlight/40 bg-highlight/10 px-3 py-2 text-sm">
          <span>This dashboard is in the trash.</span>
          <Button size="sm" variant="outline" onClick={async () => { try { await restoreMut.mutateAsync(dash.id); } catch (e) { toast.error(String(e)); } }}>Restore</Button>
        </div>
      )}
      {/* Narrow viewports wrap this row rather than clipping it. Clipping is what made the
          Fullscreen control unreachable at 390px: it sat past the right edge inside an ancestor
          that hid the overflow, so it could be neither seen nor scrolled to. */}
      <div className={`mb-3 flex items-center justify-between border-b pb-3 ${narrowViewport ? "flex-wrap gap-y-2" : ""}`}>
        {editing ? (
          <input aria-label="dashboard name" value={draftName} onChange={(e) => setDraftName(e.target.value)}
            className="min-w-0 flex-1 rounded border border-transparent bg-transparent text-xl font-semibold focus:bg-background focus:px-2 focus-visible:border-border focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" />
        ) : (
          <PageTitle className={narrowViewport ? "min-w-0 basis-full" : undefined}>{dash.name}</PageTitle>
        )}
        <div className={`flex items-center gap-2 ${narrowViewport ? "min-w-0 flex-wrap gap-y-2" : ""}`}>
          {!editing && (
            <>
              <Button size="sm" variant="ghost" aria-label={dash.bookmarked ? "remove bookmark" : "add bookmark"}
                onClick={async () => { try { await (dash.bookmarked ? removeBookmark : addBookmark).mutateAsync(dash.id); } catch (e) { toast.error(String(e)); } }}>
                <Star className={`h-4 w-4 ${dash.bookmarked ? "fill-highlight text-highlight" : ""}`} />
              </Button>
              <RefreshMenu
                refreshSeconds={view.refreshSeconds}
                onChangeRefreshSeconds={view.setRefreshSeconds}
                refreshedAt={view.refreshedAt}
                onRefreshNow={view.refreshNow}
              />
              <Button size="sm" variant="outline" aria-label="fullscreen"
                onClick={() => {
                  const next = !view.isFullscreen;
                  view.setFullscreen(next);
                  // best-effort native fullscreen; these return a Promise that rejects when
                  // the browser denies it (no gesture / headless), so swallow it (.catch).
                  try { if (next) document.documentElement.requestFullscreen?.()?.catch(() => {}); else if (document.fullscreenElement) document.exitFullscreen?.()?.catch(() => {}); } catch { /* ignore */ }
                }}>
                {view.isFullscreen ? "Exit fullscreen" : "Fullscreen"}
              </Button>
              <Button size="sm" variant="ghost" aria-label="dashboard info" onClick={() => setShowInfo((v) => !v)}><Info className="h-4 w-4" /></Button>
              <DashboardViewActionRows
                ws={ws}
                dashboardId={dashboard.id}
                canResetFilters={paramsState.canReset}
                onResetFilters={() => { filterDrafts.cancel(); paramsState.reset(); }}
              />
            </>
          )}
          {editing ? (
            <DashboardEditToolbar
              onAddChart={() => openPanel({ kind: "add" })}
              onAddText={() => addVirtualCard("text", { text: "" })}
              onAddHeading={() => addVirtualCard("heading", { text: "" })}
              onAddLink={() => addVirtualCard("link", { url: "", label: "" })}
              onAddIframe={() => addVirtualCard("iframe", { iframe: "" })}
              onAddSection={addSection}
              onAddFilter={addFilter}
              width={draftWidth}
              onChangeWidth={setDraftWidth}
            />
          ) : (
            // Withheld, not disabled, and for the same reason the certify control is: a greyed
            // button advertises a capability the reader does not have. A viewer previously got
            // the full editor and only learned it was refused when Save returned 403 — after
            // making the changes.
            //
            // `can_manage` comes from the server because it CANNOT be derived here: the session's
            // permission list is built from platform grants only, so `dashboard:manage` — which
            // is workspace-scoped — never appears in it.
            dash.can_manage !== false && (
              <Button size="sm" variant="outline" onClick={startEdit}>Edit</Button>
            )
          )}
        </div>
      </div>
      {/* For each category/id parameter, load distinct values from the first card that maps it. */}
      {parameters
        .filter((p) => p.type === "category" || p.type === "id" || p.type === "location")
        .map((p) => {
          const resolved = resolveValuesSource(p, cards, chartById);
          if (!resolved) return null;
          if (resolved.kind === "static") {
            return <StaticOptions key={p.id} slug={p.slug} values={resolved.values} onLoaded={onOptionsLoaded} />;
          }
          // query: compute linked filters from the param's mapped card (distinct semantics).
          // For a 'card' source, linked filters don't meaningfully apply → pass [].
          const mappedCard = cards.find((c) => c.card_type === "chart" && (((c.options.paramMappings as Record<string, string>) ?? {})[p.id]));
          const cardMappings = (mappedCard?.options.paramMappings as Record<string, string>) ?? {};
          const filters = p.valuesSource?.type === "card" ? [] : computeLinkedFilters(p, cardMappings, parameters, filterDrafts.appliedValues);
          return <ParamOptions key={p.id} ws={ws} datasetId={resolved.datasetId} column={resolved.column} slug={p.slug} filters={filters} onLoaded={onOptionsLoaded} />;
        })}
      {(editing || tabs.length >= 2) && (
        <DashboardTabBar
          tabs={tabs} activeTabPos={activeTabPos} editing={editing}
          onSelect={setActiveTabPos}
          onAdd={() => { const r = addTab(draftTabs ?? [], (cards as DraftCard[])); setDraftTabs(r.draftTabs); setDraftCards(r.draftCards); setActiveTabPos(r.activeTabPos); }}
          onRename={(key, name) => setDraftTabs(renameTab(draftTabs ?? [], key, name))}
          onReorder={(from, to) => setDraftTabs(reorderTab(draftTabs ?? [], from, to))}
          onDelete={(key) => {
            const hasCards = (cards as DraftCard[]).some((c) => c.tabKey === key);
            if (!hasCards) { deleteTabWithUndo(key, { mode: "delete" }); return; }
            const firstOther = (draftTabs ?? []).find((t) => t.key !== key) ?? null;
            setReassignTo(firstOther ? firstOther.key : null);
            setDeletingTabKey(key);
          }}
          onDuplicate={(key) => { const r = duplicateTab(draftTabs ?? [], (cards as DraftCard[]), key); setDraftTabs(r.draftTabs); setDraftCards(r.draftCards); setActiveTabPos(r.activeTabPos); }}
        />
      )}
      {/* THE TWO — and only two — call sites the split list reaches. */}
      {editing
        ? <EditFilterChips parameters={topBar} activeId={editPanel?.kind === "filter" ? editPanel.paramId : null}
            onSelect={(id) => openPanel({ kind: "filter", paramId: id })}
            /* `mergeTopBarOrder`, NOT `setDraftParameters`. W6's reorder writes back the whole
               list it was handed; now that the list is a SUBSET, writing it back unmerged would
               delete every filter an author had moved onto a card. */
            onReorder={(next) => setDraftParameters(mergeTopBarOrder(parameters, next))} />
        : <DashboardFilterBar parameters={topBar} values={filterDrafts.editorValues}
            onChange={filterDrafts.setValue} optionsBySlug={optionsBySlug}
            autoApply={filterDrafts.autoApply} onAutoApplyChange={filterDrafts.setAutoApply}
            changedCount={filterDrafts.changedCount} onApply={filterDrafts.apply} onCancel={filterDrafts.cancel} />}
      {/* Outside the grid on purpose: these must keep running for a card whose tile is
          hidden, which is the whole reason a hidden card can ever come back. */}
      {probeCards.map((c) => {
        const { baseChart, paramFilters, paramTemporal } = cardQueryArgs(c);
        return (
          <CardEmptinessProbe key={c.id} ws={ws} dashboardId={dashboard.id} cardId={c.id}
            refreshNonce={view.refreshNonce} enabled={!requiredUnset && !!baseChart}
            paramFilters={paramFilters} paramTemporal={paramTemporal} onEmpty={reportCardEmpty} />
        );
      })}
      <div className={editing || showInfo ? "flex gap-4" : ""}>
        <div className={editing ? "min-w-0 flex-1 pb-24" : showInfo ? "min-w-0 flex-1" : ""}>
      {allHiddenByEmpty ? (
        <div className="flex flex-col items-center gap-2 py-20 text-center text-muted-foreground">
          <LayoutDashboard className="h-8 w-8 text-muted-foreground/40" aria-hidden />
          <p className="text-sm">No results, so every card on this dashboard is hidden.</p>
        </div>
      ) : (cards.length === 0 || (tabs.length > 0 && visibleCards.length === 0)) && editing ? (
        <div className="flex flex-col items-center gap-3 rounded-lg border border-dashed py-20 text-center">
          <Pencil className="h-8 w-8 text-muted-foreground/40" />
          <p className="text-sm text-muted-foreground">{cards.length === 0 ? "This dashboard is empty." : "No cards on this tab."}</p>
          <button type="button" className="rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:opacity-90" onClick={() => openPanel({ kind: "add" })}>Add a chart</button>
        </div>
      ) : cards.length === 0 ? (
        <div className="flex flex-col items-center gap-2 py-20 text-center text-muted-foreground">
          <LayoutDashboard className="h-8 w-8 text-muted-foreground/40" aria-hidden />
          <p className="text-sm">
            No cards yet. Click <span className="font-medium text-foreground">Edit</span> to add charts.
          </p>
        </div>
      ) : tabs.length > 0 && visibleCards.length === 0 ? (
        <div className="flex flex-col items-center gap-2 py-20 text-center text-muted-foreground">
          <LayoutDashboard className="h-8 w-8 text-muted-foreground/40" aria-hidden />
          <p className="text-sm">No cards on this tab.</p>
        </div>
      ) : (
        <DashboardGridLayout
          editing={editing}
          layout={layout}
          kindOf={(id) => {
            const c = (cards as DraftCard[]).find((x) => x.id === id);
            return c ? vizKeyOf(c) : "";
          }}
          onBreakpointChange={onGridBreakpointChange}
          onLayoutChange={onLayoutChange}
          onResizeStart={() => { interactingRef.current = true; }}
          onResizeStop={() => { interactingRef.current = false; }}
          onDragStart={() => { interactingRef.current = true; }}
          onDragStop={() => { interactingRef.current = false; }}
        >
          {(rowHeight) => visibleCards.map((c) => {
            const { baseChart, mappings, paramFilters, paramTemporal } = cardQueryArgs(c);
            const injected = baseChart ? applyParamsToChart(baseChart, parameters, filterDrafts.appliedValues, mappings) : undefined;
            return (
            <div key={c.id} className="group relative h-full">
              {editing && (
                <div className="dash-nodrag absolute -top-3.5 left-1/2 z-30 -translate-x-1/2 opacity-0 transition-opacity group-hover:opacity-100">
                  <CardEditToolbar cardType={c.card_type}
                    onViz={() => openPanel({ kind: "viz", cardId: c.id })}
                    onDuplicate={() => duplicateCard(c.id)}
                    onReplace={() => setReplacingId(c.id)}
                    onRemove={() => removeCard(c.id)} />
                </div>
              )}
              <div
                data-testid="dashcard-body"
                role={editing && c.card_type !== "placeholder" ? "button" : undefined}
                tabIndex={editing && c.card_type !== "placeholder" ? 0 : undefined}
                onClick={editing && c.card_type !== "placeholder" ? () => openPanel({ kind: "card", cardId: c.id }) : undefined}
                /* Only a key pressed ON the card itself activates it (KI-059). The heading input, the text
                   textarea and any inline filter inside the card bubble their keydown up here, and
                   the preventDefault swallowed every space and newline typed into them. */
                onKeyDown={editing && c.card_type !== "placeholder" ? (e) => { if (e.target !== e.currentTarget) return; if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openPanel({ kind: "card", cardId: c.id }); } } : undefined}
                className={`relative h-full overflow-hidden ${editing && c.card_type !== "placeholder" ? "cursor-pointer" : ""} ${cardChromeClass(c)} ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
                {editing && <div className="dash-draghandle absolute inset-x-0 top-0 z-10 h-6 cursor-move" title="Drag to move" />}
                {c.card_type === "placeholder" ? (
                  <PlaceholderCard onPick={() => setFillingPlaceholderId(c.id)} />
                ) : (
                  <DashboardCardView ws={ws} dashboardId={dashboard.id} card={c} chart={injected} refreshNonce={view.refreshNonce} parameters={parameters} paramValues={filterDrafts.appliedValues} paramFilters={paramFilters} paramTemporal={paramTemporal}
                    blocked={!editing && requiredUnset} editing={editing}
                    autoHeight={!!c.options.autoHeight}
                    rowHeight={rowHeight}
                    onAutoHeight={editing ? (rows) => applyAutoHeight(c.id, rows) : undefined}
                    onValueClick={editing ? undefined : (ctx, x, y) => onCardValueClick(c, ctx, x, y)}
                    onEditText={editing && (c.card_type === "text" || c.card_type === "heading") ? (t) => { updateCardOptions(c.id, { text: t }); if (newCardId === c.id) setNewCardId(null); } : undefined}
                    autoFocusText={c.id === newCardId}
                    /* The filters THIS card hosts. `CardInlineFilters` returns null for an empty
                       list, so a card that hosts nothing renders exactly as it does today. The
                       values are W6's EDITOR values, so a card-hosted filter behaves identically
                       to a top-bar one under the draft/Apply mode. */
                    inline={{
                      parameters: inlineParametersFor(c, parameters),
                      values: filterDrafts.editorValues, optionsBySlug, onChange: filterDrafts.setValue,
                      onSelect: editing ? (paramId: string) => openPanel({ kind: "filter", paramId }) : undefined,
                      activeId: activeParamId,
                    }} />
                )}
                {activeParamId && activeParam && (
                  <CardFilterMapOverlay
                    state={cardMapState({
                      cardType: c.card_type,
                      paramType: activeParam.type,
                      // One column rule for the whole dashboard: the overlay and auto-connect both
                      // call this, so a card can never offer a column that auto-connect would not
                      // also match.
                      columns: baseChart ? mappableColumns(baseChart, activeParam.type, mappings[activeParamId]) : [],
                      rows: c.size_y,
                      chartMissing: c.card_type === "chart" && !baseChart,
                      // W2 BUILT THIS BRANCH AND LEFT IT INERT; THIS IS THE LINE THAT TURNS IT ON.
                      // "The selected filter is on another tab." had no reachable precondition
                      // while every filter was dashboard-level (card-map-state.ts:54-63). An
                      // inline parameter finally gives a filter an owning card, and therefore a
                      // tab. `hostTabKey` returns `undefined` for a top-bar filter and `null` for
                      // a host on a tabless dashboard, and `cardMapState` guards with `!= null`,
                      // so both keep the hint exactly as inert as it is today.
                      filterTabKey: hostTabKey(activeParamId, cards),
                      cardTabKey: (c as DraftCard).tabKey,
                    })}
                    value={mappings[activeParamId] ?? ""}
                    onChange={(col) => {
                      updateCardOptions(c.id, { paramMappings: { ...mappings, [activeParamId]: col } });
                      if (col) offerAutoConnectFor(c.id, activeParamId, col);
                    }}
                  />
                )}
              </div>
            </div>
            );
          })}
        </DashboardGridLayout>
      )}
        </div>
        {editing && editPanel && (
          <EditSidebar
            panel={editPanel}
            charts={charts ?? []}
            parameters={parameters}
            cardColumns={cardColumns}
            panelLabels={panelLabels}
            valuesSourceLabels={valuesSourceLabels}
            vizForPanel={vizForPanel}
            onChangeVizOverride={(next) => { if (editPanel?.kind === "viz") updateCardOptions(editPanel.cardId, { vizOverride: next }); }}
            tabs={tabs}
            cardForPanel={editPanel?.kind === "card" ? (cards as DraftCard[]).find((c) => c.id === editPanel.cardId) : undefined}
            chartForCard={editPanel?.kind === "card" ? chartById.get((cards as DraftCard[]).find((c) => c.id === editPanel.cardId)?.chart_id ?? "") : undefined}
            paramForPanel={editPanel?.kind === "filter" ? parameters.find((p) => p.id === editPanel.paramId) : undefined}
            defaultOptions={editPanel?.kind === "filter"
              ? (optionsBySlug[parameters.find((p) => p.id === editPanel.paramId)?.slug ?? ""] ?? [])
              : []}
            moveDestinations={moveDestinations({
              cards: cards as DraftCard[], tabs,
              chartNameOf: (cardId) => chartById.get((cards as DraftCard[]).find((x) => x.id === cardId)?.chart_id ?? "")?.name,
            })}
            hostCardId={editPanel?.kind === "filter" ? findHostCardId(editPanel.paramId, cards) : null}
            onMoveParam={(cardId) => { if (editPanel?.kind === "filter") setDraftCards(moveParameterToCard(cards as DraftCard[], editPanel.paramId, cardId)); }}
            onAddChart={(c) => addCard(c)}
            onUpdateCardOptions={(patch) => { if (editPanel?.kind === "card") updateCardOptions(editPanel.cardId, patch); }}
            onMoveCardToTab={(tabKey) => { if (editPanel?.kind === "card") setDraftCards(moveCardToTab((cards as DraftCard[]), editPanel.cardId, tabKey)); }}
            onChangeParam={(next) => setDraftParameters(parameters.map((x) => x.id === next.id ? { ...next, slug: x.slug } : x))}
            onDeleteParam={() => { if (editPanel?.kind === "filter") { const id = editPanel.paramId; setDraftParameters(parameters.filter((x) => x.id !== id)); setEditPanel({ kind: "add" }); } }}
            onClose={() => setEditPanel(null)}
            onBack={() => setEditPanel({ kind: "add" })}
          />
        )}
        {!editing && showInfo && (
          <DashboardInfoSidebar
            name={dash.name}
            description={dash.description}
            folderName={dashboardFolders.find((f) => f.id === dash.folder_id)?.name ?? null}
            folderHref={dash.folder_id ? `/explore/${ws}/dashboards?folder=${dash.folder_id}` : null}
            ownerLabel={dash.created_by_name ?? dash.created_by}
            createdAt={dash.created_at ?? null}
            editorLabel={dash.updated_by_name ?? dash.updated_by}
            updatedAt={dash.updated_at}
            revisions={revisionsQ.data ?? []}
            revisionsLoading={revisionsQ.isLoading}
            reverting={revert.isPending}
            onOpenHistory={() => setHistoryOpen(true)}
            onRevert={async (revisionId) => {
              try { await revert.mutateAsync({ id: dash.id, revisionId }); toast.success("Reverted"); }
              catch (e) { toast.error(String(e)); }
            }}
            onClose={() => setShowInfo(false)}
            onSaveDescription={saveDescription}
            certified={!!dash.certified}
            certifiedBy={dash.certified_by ?? ""}
            certificationDetails={dash.certification_details ?? ""}
            canCertify={canCertify}
            certifying={setCertification.isPending}
            onSetCertification={async (next, details) => {
              try {
                await setCertification.mutateAsync({ id: dash.id, certified: next, details });
                toast.success(next ? "Certified" : "Certification withdrawn");
              } catch (e) { toast.error(String(e)); }
            }}
          />
        )}
      </div>
      <Dialog open={deletingTabKey !== null} onOpenChange={(o) => { if (!o) { setDeletingTabKey(null); setReassignTo(null); } }}>
        <DialogContent>
          <DialogHeader><DialogTitle>Delete tab</DialogTitle></DialogHeader>
          <p className="text-sm text-muted-foreground">This tab has cards. Move them to another tab, or delete them with the tab.</p>
          <div className="flex flex-col gap-2 py-2">
            <label className="flex items-center gap-2 text-sm">
              <span>Move cards to</span>
              <select aria-label="reassign target" className="h-8 rounded border bg-background px-1 text-sm"
                value={reassignTo ?? ""} onChange={(e) => setReassignTo(e.target.value)}>
                {(draftTabs ?? []).filter((t) => t.key !== deletingTabKey).map((t) => (
                  <option key={t.key} value={t.key}>{t.name}</option>
                ))}
              </select>
            </label>
            <div className="flex justify-end gap-2">
              <Button size="sm" variant="outline" aria-label="reassign and delete"
                onClick={() => {
                  if (!deletingTabKey || !reassignTo) return;
                  deleteTabWithUndo(deletingTabKey, { mode: "reassign", toKey: reassignTo });
                  setDeletingTabKey(null); setReassignTo(null);
                }}>reassign and delete</Button>
              <Button size="sm" variant="ghost" className="text-destructive" aria-label="delete cards too"
                onClick={() => {
                  if (!deletingTabKey) return;
                  deleteTabWithUndo(deletingTabKey, { mode: "delete" });
                  setDeletingTabKey(null); setReassignTo(null);
                }}>delete cards too</Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>
      <Dialog open={replacingId !== null} onOpenChange={(o) => { if (!o) setReplacingId(null); }}>
        <DialogContent>
          <DialogHeader><DialogTitle>Replace chart</DialogTitle></DialogHeader>
          <ChartPickerList charts={charts ?? []} onPick={(ch) => replaceCard(replacingId!, ch)} autoFocus />
        </DialogContent>
      </Dialog>
      <DiscardChangesDialog
        open={confirmingDiscard}
        onOpenChange={(o) => { if (!o) setConfirmingDiscard(false); }}
        onDiscard={discardEdit}
      />
      <Dialog open={fillingPlaceholderId !== null} onOpenChange={(o) => { if (!o) setFillingPlaceholderId(null); }}>
        <DialogContent>
          <DialogHeader><DialogTitle>Add a chart</DialogTitle></DialogHeader>
          <ChartPickerList charts={charts ?? []} onPick={(ch) => fillPlaceholder(fillingPlaceholderId!, ch)} autoFocus />
        </DialogContent>
      </Dialog>
      {drillMenu && (
        <DrillMenu ctx={drillMenu.ctx} x={drillMenu.x} y={drillMenu.y}
          onFilterBy={(fs) => {
            // A dashboard parameter holds one scalar, so a bucketed drill contributes its lower
            // bound — the start of the period the cell stood for.
            const f = fs[0];
            if (!f || !("value" in f)) return;
            const target = parameters.find((p) =>
              dashboard.cards.some((c) => ((c.options.paramMappings as Record<string, string>) ?? {})[p.id] === f.col));
            if (target) filterDrafts.setValue(target.slug, f.value);
          }}
          onSeeRecords={(fs) => setRecords({ filters: fs, datasetId: drillMenu.datasetId })}
          onClose={() => setDrillMenu(null)} />
      )}
      <DrillRecordsDialog
        open={records !== null}
        onOpenChange={(o) => { if (!o) setRecords(null); }}
        ws={ws} datasetId={records?.datasetId ?? null}
        columns={(recordsDataset.data?.columns ?? []).map((c) => ({ name: c.column_name, type: c.data_type ?? "varchar" }))}
        filters={records?.filters ?? []}
      />
      </div>
    </div>
  );
}
