import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useParams } from "react-router-dom";
import RGL, { WidthProvider, type Layout } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
import { useDashboard, useDashboardMutations, type DashboardCard as Card, type DashboardParameter } from "@/api/dashboards";
import { useCharts, type Chart, type ChartFilter } from "@/api/charts";
import { useDataset } from "@/api/datasets";
import { resolveClick, type ClickBehavior } from "@/components/chart/dashboard/click-behavior";
import { DrillMenu } from "@/components/chart/DrillMenu";
import { DrillRecordsDialog } from "@/components/chart/DrillRecordsDialog";
import { type DrillContext } from "@/components/chart/drill";
import { useDashboardViewControls } from "@/api/useDashboardViewControls";
import { useDashboardParams } from "@/api/useDashboardParams";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { GRID_COLS, CARD_MIN_SIZE, CARD_DEFAULT_SIZE } from "@/components/chart/dashboard-grid";
import { DashboardCardView } from "@/components/chart/dashboard/DashboardCardView";
import { DashboardFilterBar } from "@/components/chart/dashboard/DashboardFilterBar";
import { EditSidebar, type EditPanel } from "@/components/chart/dashboard/EditSidebar";
import { EditFilterChips } from "@/components/chart/dashboard/EditFilterChips";
import { CardEditToolbar } from "@/components/chart/dashboard/CardEditToolbar";
import { slugify, applyParamsToChart, isBlank } from "@/components/chart/dashboard/params";
import { useColumnValues } from "@/api/useColumnValues";
import { useDashboardTabs } from "@/api/useDashboardTabs";
import { DashboardTabBar } from "@/components/chart/dashboard/DashboardTabBar";
import { startTabsDraft, serializeLayout, addTab, renameTab, reorderTab, deleteTab, duplicateTab, moveCardToTab, type DraftTab, type DraftCard } from "@/components/chart/dashboard/tabs";
import { toast } from "sonner";

const Grid = WidthProvider(RGL);
const ROW_PX = 90;

/** Loads distinct column values for one category/id parameter (hook is called
 * unconditionally inside this child so the Rules of Hooks are respected). */
function ParamOptions({ ws, datasetId, column, slug, onLoaded }: { ws: string; datasetId: string | null; column: string | null; slug: string; onLoaded: (slug: string, values: string[]) => void }) {
  const { values } = useColumnValues(ws, datasetId, column, !!datasetId && !!column);
  useEffect(() => { if (values.length) onLoaded(slug, values); }, [slug, values, onLoaded]);
  return null;
}

export default function DashboardView() {
  const { workspace: ws, id } = useParams() as { workspace: string; id: string };
  const { data: dash, isLoading } = useDashboard(ws, id);
  const { data: charts } = useCharts(ws);
  const { update } = useDashboardMutations(ws);
  const [editing, setEditing] = useState(false);
  const [draftCards, setDraftCards] = useState<DraftCard[] | null>(null);
  const [draftParameters, setDraftParameters] = useState<DashboardParameter[] | null>(null);
  const [draftTabs, setDraftTabs] = useState<DraftTab[] | null>(null);
  const [draftWidth, setDraftWidth] = useState<"fixed" | "full">("full");
  const serverTabCount = dash?.tabs?.length ?? 0;
  const { activeTabPos, setActiveTabPos } = useDashboardTabs(editing ? (draftTabs?.length ?? 0) : serverTabCount);
  const cardSeqRef = useRef(0);
  const chartById = useMemo(() => new Map((charts ?? []).map((c) => [c.id, c])), [charts]);
  const view = useDashboardViewControls();
  // Hook must run before the early return — use `dash?` since dash may be undefined while loading.
  const paramSource = editing && draftParameters ? draftParameters : (dash?.parameters ?? []);
  const paramsState = useDashboardParams(paramSource);
  const [editPanel, setEditPanel] = useState<EditPanel>(null);
  const [replacingId, setReplacingId] = useState<string | null>(null);
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

  useEffect(() => {
    setEditing(false);
    setDraftCards(null);
    setDraftParameters(null);
    setDraftTabs(null);
  }, [id]);

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
  const visibleCards = tabs.length === 0
    ? cards
    : cards.filter((c) =>
        editing ? (c as DraftCard).tabKey === activeTab?.key : c.tab_id === activeTab?.key,
      );
  const requiredUnset = parameters.some((p) => {
    const v = paramsState.values[p.slug];
    return p.required && (Array.isArray(v) ? v.filter((x) => !isBlank(x)).length === 0 : isBlank(v));
  });

  function onCardValueClick(card: Card, ctx: DrillContext, x: number, y: number) {
    const cb = card.options.clickBehavior as ClickBehavior | undefined;
    const r = resolveClick(cb, ctx, parameters, paramsState.values);
    const datasetId = chartById.get(card.chart_id ?? "")?.dataset_id ?? null;
    if (r.kind === "url") { if (r.href) window.open(r.href, "_blank", "noreferrer"); return; }
    if (r.kind === "crossfilter") { for (const s of r.sets) paramsState.setValue(s.slug, s.value); return; }
    setDrillMenu({ ctx, x, y, datasetId });
  }

  function startEdit() {
    const { draftTabs: dt, draftCards: dc } = startTabsDraft(dashboard.tabs ?? [], dashboard.cards);
    setDraftCards(dc);
    setDraftParameters((dashboard.parameters ?? []).map((p) => ({ ...p })));
    setDraftTabs(dt);
    setDraftWidth(dashboard.width);
    setEditing(true);
    setEditPanel({ kind: "add" });
  }
  function cancelEdit() { setEditing(false); setDraftCards(null); setDraftParameters(null); setDraftTabs(null); setEditPanel(null); }

  function addCard(chart: Chart) {
    const maxY = cards.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
    const newCard: DraftCard = {
      id: `new-${chart.id}-${cardSeqRef.current++}`, chart_id: chart.id, card_type: "chart",
      grid_col: 0, grid_row: maxY, size_x: CARD_DEFAULT_SIZE.w, size_y: CARD_DEFAULT_SIZE.h, options: {}, tab_id: null,
      tabKey: activeTab?.key ?? null,
    };
    setDraftCards([...(cards as DraftCard[]), newCard]);
  }
  function removeCard(cardId: string) {
    setDraftCards((cards as DraftCard[]).filter((c) => c.id !== cardId));
  }

  function duplicateCard(cardId: string) {
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const src = base.find((c) => c.id === cardId);
      if (!src) return base;
      const maxY = base.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
      return [...base, { ...src, id: `new-dup-${cardSeqRef.current++}`, grid_row: maxY, options: { ...src.options } }];
    });
  }
  function replaceCard(cardId: string, chart: Chart) {
    setDraftCards((prev) => (prev ?? cardsAsDraft()).map((c) => (c.id === cardId ? { ...c, chart_id: chart.id, options: {} } : c)));
    setReplacingId(null);
  }

  function addVirtualCard(card_type: "text" | "heading" | "link", options: Record<string, unknown>) {
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const maxY = base.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
      const newCard: DraftCard = {
        id: `new-${card_type}-${cardSeqRef.current++}`, chart_id: null, card_type,
        grid_col: 0, grid_row: maxY, size_x: CARD_DEFAULT_SIZE.w, size_y: card_type === "heading" ? 1 : CARD_DEFAULT_SIZE.h, options, tab_id: null,
        tabKey: activeTab?.key ?? null,
      };
      return [...base, newCard];
    });
  }
  function updateCardOptions(cardId: string, patch: Record<string, unknown>) {
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      return base.map((c) => (c.id === cardId ? { ...c, options: { ...c.options, ...patch } } : c));
    });
  }

  // The cards are the single source of truth for positions: the RGL layout is derived
  // from them, and drag/resize writes positions straight back into the draft. This keeps
  // every grid item sized from its card (a newly added card is never auto-placed at 1x1).
  const layout: Layout[] = visibleCards.map((c) => ({
    i: c.id, x: c.grid_col, y: c.grid_row, w: c.size_x, h: c.size_y,
    // clamp the minimums to the card's own size so a short card (e.g. a 1-row
    // heading) never has minH/minW larger than its height/width (RGL warns).
    minW: Math.min(CARD_MIN_SIZE.w, c.size_x), minH: Math.min(CARD_MIN_SIZE.h, c.size_y),
  }));

  function onLayoutChange(next: Layout[]) {
    if (!editing) return;
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      let changed = false;
      const merged = base.map((c) => {
        const l = next.find((x) => x.i === c.id);
        if (!l || (l.x === c.grid_col && l.y === c.grid_row && l.w === c.size_x && l.h === c.size_y)) return c;
        changed = true;
        return { ...c, grid_col: l.x, grid_row: l.y, size_x: l.w, size_y: l.h };
      });
      return changed ? merged : base;   // same ref when nothing moved → no re-render loop
    });
  }

  async function onSave() {
    const { tabs: tabPayload, cards: cardPayload } = serializeLayout(draftTabs ?? [], (cards as DraftCard[]));
    try {
      await update.mutateAsync({ id: dashboard.id, name: dashboard.name, description: dashboard.description, parameters, tabs: tabPayload, width: draftWidth, cards: cardPayload });
      setEditing(false); setDraftCards(null); setDraftTabs(null); setEditPanel(null);
      toast.success("Saved");
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <div className={view.isFullscreen ? "fixed inset-0 z-50 overflow-auto bg-muted/30 p-4" : "min-h-full bg-muted/30 p-4"}>
      <div className={effectiveWidth === "fixed" ? "mx-auto w-full max-w-[1200px]" : ""}>
      <div className="mb-3 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{dash.name}</h1>
        <div className="flex gap-2">
          {!editing && (
            <>
              <label className="flex items-center gap-1 text-xs text-muted-foreground">
                <span className="sr-only">Auto-refresh</span>
                <select aria-label="auto-refresh" className="h-8 rounded border bg-background px-1 text-xs"
                  value={view.refreshSeconds ?? 0}
                  onChange={(e) => view.setRefreshSeconds(Number(e.target.value) || null)}>
                  <option value={0}>No refresh</option>
                  <option value={60}>1 min</option>
                  <option value={300}>5 min</option>
                  <option value={600}>10 min</option>
                  <option value={900}>15 min</option>
                  <option value={1800}>30 min</option>
                  <option value={3600}>60 min</option>
                </select>
              </label>
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
            </>
          )}
          {editing ? (
            <>
              <Button size="sm" variant="outline" onClick={() => setEditPanel({ kind: "add" })}>+ Chart</Button>
              <Button size="sm" variant="outline" onClick={() => {
                const slug = slugify("filter", parameters.map((p) => p.slug));
                const id = `new-${slug}-${cardSeqRef.current++}`;
                setDraftParameters([...parameters, { id, name: "Filter", slug, type: "text", operator: "is" }]);
                setEditPanel({ kind: "filter", paramId: id });
              }}>+ Filter</Button>
              <div className="flex items-center gap-1 rounded border p-0.5">
                <button type="button" aria-label="width full" className={`rounded px-2 py-0.5 text-xs ${draftWidth === "full" ? "bg-accent" : ""}`} onClick={() => setDraftWidth("full")}>Full</button>
                <button type="button" aria-label="width fixed" className={`rounded px-2 py-0.5 text-xs ${draftWidth === "fixed" ? "bg-accent" : ""}`} onClick={() => setDraftWidth("fixed")}>Fixed</button>
              </div>
              <Button size="sm" variant="ghost" onClick={cancelEdit}>Cancel</Button>
              <Button size="sm" onClick={onSave} disabled={update.isPending}>Save</Button>
            </>
          ) : (
            <Button size="sm" variant="outline" onClick={startEdit}>Edit</Button>
          )}
        </div>
      </div>
      {/* For each category/id parameter, load distinct values from the first card that maps it. */}
      {parameters
        .filter((p) => p.type === "category" || p.type === "id")
        .map((p) => {
          const card = cards.find((c) => c.card_type === "chart" && ((c.options.paramMappings as Record<string, string>) ?? {})[p.id]);
          if (!card) return null;
          const ch = chartById.get(card.chart_id ?? "");
          const column = ((card.options.paramMappings as Record<string, string>) ?? {})[p.id] ?? null;
          if (!ch || !column) return null;
          return <ParamOptions key={p.id} ws={ws} datasetId={ch.dataset_id} column={column} slug={p.slug} onLoaded={onOptionsLoaded} />;
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
            if (!hasCards) {
              const r = deleteTab(draftTabs ?? [], (cards as DraftCard[]), key, { mode: "delete" });
              setDraftTabs(r.draftTabs); setDraftCards(r.draftCards);
              setActiveTabPos(Math.min(activeTabPos, r.draftTabs.length - 1));
            } else {
              const firstOther = (draftTabs ?? []).find((t) => t.key !== key) ?? null;
              setReassignTo(firstOther ? firstOther.key : null);
              setDeletingTabKey(key);
            }
          }}
          onDuplicate={(key) => { const r = duplicateTab(draftTabs ?? [], (cards as DraftCard[]), key); setDraftTabs(r.draftTabs); setDraftCards(r.draftCards); setActiveTabPos(r.activeTabPos); }}
        />
      )}
      {editing
        ? <EditFilterChips parameters={parameters} activeId={editPanel?.kind === "filter" ? editPanel.paramId : null} onSelect={(id) => setEditPanel({ kind: "filter", paramId: id })} />
        : <DashboardFilterBar parameters={parameters} values={paramsState.values} onChange={paramsState.setValue} optionsBySlug={optionsBySlug} />}
      <div className={editing ? "flex gap-4" : ""}>
        <div className={editing ? "min-w-0 flex-1" : ""}>
      {cards.length === 0 ? (
        <p className="text-muted-foreground">No cards yet. Click Edit to add charts.</p>
      ) : tabs.length > 0 && visibleCards.length === 0 ? (
        <p className="py-12 text-center text-muted-foreground">No cards on this tab.</p>
      ) : (
        <Grid
          className="layout"
          cols={GRID_COLS}
          rowHeight={ROW_PX}
          margin={[12, 12]}
          draggableHandle=".dash-draghandle"
          draggableCancel=".dash-nodrag"
          isDraggable={editing}
          isResizable={editing}
          layout={layout}
          onLayoutChange={onLayoutChange}
        >
          {visibleCards.map((c) => {
            const baseChart = chartById.get(c.chart_id ?? "");
            const mappings = (c.options.paramMappings as Record<string, string>) ?? {};
            const injected = baseChart ? applyParamsToChart(baseChart, parameters, paramsState.values, mappings) : undefined;
            return (
            <div key={c.id} className={`group relative overflow-hidden ${c.card_type === "heading" ? "" : "rounded-lg border bg-card shadow-sm"} ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
              {editing && (
                <div className="dash-draghandle absolute inset-x-0 top-0 z-10 flex h-7 cursor-move items-center justify-end gap-1 bg-gradient-to-b from-card/95 via-card/70 to-transparent px-1 opacity-0 transition-opacity group-hover:opacity-100">
                  <CardEditToolbar cardType={c.card_type}
                    onSettings={() => setEditPanel({ kind: "card", cardId: c.id })}
                    onDuplicate={() => duplicateCard(c.id)}
                    onReplace={() => setReplacingId(c.id)}
                    onRemove={() => removeCard(c.id)} />
                </div>
              )}
              <DashboardCardView ws={ws} card={c} chart={injected} refreshNonce={view.refreshNonce} parameters={parameters} paramValues={paramsState.values}
                blocked={!editing && requiredUnset}
                onValueClick={editing ? undefined : (ctx, x, y) => onCardValueClick(c, ctx, x, y)} />
            </div>
            );
          })}
        </Grid>
      )}
        </div>
        {editing && (
          <EditSidebar
            panel={editPanel}
            charts={charts ?? []}
            parameters={parameters}
            tabs={tabs}
            cardForPanel={editPanel?.kind === "card" ? (cards as DraftCard[]).find((c) => c.id === editPanel.cardId) : undefined}
            chartForCard={editPanel?.kind === "card" ? chartById.get((cards as DraftCard[]).find((c) => c.id === editPanel.cardId)?.chart_id ?? "") : undefined}
            paramForPanel={editPanel?.kind === "filter" ? parameters.find((p) => p.id === editPanel.paramId) : undefined}
            onAddChart={(c) => addCard(c)}
            onAddVirtual={(t) => addVirtualCard(t, t === "link" ? { url: "", label: "" } : { text: "" })}
            onUpdateCardOptions={(patch) => { if (editPanel?.kind === "card") updateCardOptions(editPanel.cardId, patch); }}
            onMoveCardToTab={(tabKey) => { if (editPanel?.kind === "card") setDraftCards(moveCardToTab((cards as DraftCard[]), editPanel.cardId, tabKey)); }}
            onChangeParam={(next) => setDraftParameters(parameters.map((x) => x.id === next.id ? { ...next, slug: x.slug } : x))}
            onDeleteParam={() => { if (editPanel?.kind === "filter") { const id = editPanel.paramId; setDraftParameters(parameters.filter((x) => x.id !== id)); setEditPanel({ kind: "add" }); } }}
            onClose={() => setEditPanel({ kind: "add" })}
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
                  const r = deleteTab(draftTabs ?? [], (cards as DraftCard[]), deletingTabKey, { mode: "reassign", toKey: reassignTo });
                  setDraftTabs(r.draftTabs); setDraftCards(r.draftCards);
                  setActiveTabPos(Math.min(activeTabPos, r.draftTabs.length - 1));
                  setDeletingTabKey(null); setReassignTo(null);
                }}>reassign and delete</Button>
              <Button size="sm" variant="ghost" className="text-destructive" aria-label="delete cards too"
                onClick={() => {
                  if (!deletingTabKey) return;
                  const r = deleteTab(draftTabs ?? [], (cards as DraftCard[]), deletingTabKey, { mode: "delete" });
                  setDraftTabs(r.draftTabs); setDraftCards(r.draftCards);
                  setActiveTabPos(Math.min(activeTabPos, r.draftTabs.length - 1));
                  setDeletingTabKey(null); setReassignTo(null);
                }}>delete cards too</Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>
      <Dialog open={replacingId !== null} onOpenChange={(o) => { if (!o) setReplacingId(null); }}>
        <DialogContent>
          <DialogHeader><DialogTitle>Replace chart</DialogTitle></DialogHeader>
          <div className="flex flex-wrap gap-1 py-2">
            {(charts ?? []).map((ch) => (
              <Button key={ch.id} size="sm" variant="outline" aria-label={`use ${ch.name}`} onClick={() => replaceCard(replacingId!, ch)}>
                {ch.name}
              </Button>
            ))}
          </div>
        </DialogContent>
      </Dialog>
      {drillMenu && (
        <DrillMenu ctx={drillMenu.ctx} x={drillMenu.x} y={drillMenu.y}
          onFilterBy={(f) => {
            if (!("value" in f)) return;
            const target = parameters.find((p) =>
              dashboard.cards.some((c) => ((c.options.paramMappings as Record<string, string>) ?? {})[p.id] === f.col));
            if (target) paramsState.setValue(target.slug, f.value);
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
