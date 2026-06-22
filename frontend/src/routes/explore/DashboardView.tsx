import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useParams } from "react-router-dom";
import RGL, { WidthProvider, type Layout } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
import { useDashboard, useDashboardMutations, type CardInput, type DashboardCard as Card, type DashboardParameter } from "@/api/dashboards";
import { useCharts, type Chart } from "@/api/charts";
import { useDashboardViewControls } from "@/api/useDashboardViewControls";
import { useDashboardParams } from "@/api/useDashboardParams";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { GRID_COLS, CARD_MIN_SIZE, CARD_DEFAULT_SIZE } from "@/components/chart/dashboard-grid";
import { DashboardCardView } from "@/components/chart/dashboard/DashboardCardView";
import { DashboardFilterBar } from "@/components/chart/dashboard/DashboardFilterBar";
import { ParameterEditor } from "@/components/chart/dashboard/ParameterEditor";
import { slugify, applyParamsToChart } from "@/components/chart/dashboard/params";
import { useColumnValues } from "@/api/useColumnValues";
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
  const [draftCards, setDraftCards] = useState<Card[] | null>(null);
  const [draftParameters, setDraftParameters] = useState<DashboardParameter[] | null>(null);
  const cardSeqRef = useRef(0);
  const chartById = useMemo(() => new Map((charts ?? []).map((c) => [c.id, c])), [charts]);
  const view = useDashboardViewControls();
  // Hook must run before the early return — use `dash?` since dash may be undefined while loading.
  const paramSource = editing && draftParameters ? draftParameters : (dash?.parameters ?? []);
  const paramsState = useDashboardParams(paramSource);
  const [replacingId, setReplacingId] = useState<string | null>(null);
  // category/id dropdown options, keyed by param slug; populated by <ParamOptions> children.
  const [optionsBySlug, setOptionsBySlug] = useState<Record<string, string[]>>({});
  const onOptionsLoaded = useCallback((slug: string, vals: string[]) => setOptionsBySlug((m) => ({ ...m, [slug]: vals })), []);

  useEffect(() => {
    setEditing(false);
    setDraftCards(null);
    setDraftParameters(null);
  }, [id]);

  if (isLoading || !dash) return <div className="p-4 text-muted-foreground">Loading…</div>;

  const dashboard = dash;
  const cards = editing && draftCards ? draftCards : dashboard.cards;
  const parameters = editing && draftParameters ? draftParameters : (dashboard.parameters ?? []);

  function startEdit() {
    setDraftCards(dashboard.cards.map((c) => ({ ...c })));
    setDraftParameters((dashboard.parameters ?? []).map((p) => ({ ...p })));
    setEditing(true);
  }
  function cancelEdit() { setEditing(false); setDraftCards(null); setDraftParameters(null); }

  function addCard(chart: Chart) {
    const maxY = cards.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
    const newCard: Card = {
      id: `new-${chart.id}-${cardSeqRef.current++}`, chart_id: chart.id, card_type: "chart",
      grid_col: 0, grid_row: maxY, size_x: CARD_DEFAULT_SIZE.w, size_y: CARD_DEFAULT_SIZE.h, options: {},
    };
    setDraftCards([...cards, newCard]);
  }
  function removeCard(cardId: string) {
    setDraftCards(cards.filter((c) => c.id !== cardId));
  }

  function duplicateCard(cardId: string) {
    setDraftCards((prev) => {
      const base = prev ?? dashboard.cards;
      const src = base.find((c) => c.id === cardId);
      if (!src) return base;
      const maxY = base.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
      return [...base, { ...src, id: `new-dup-${cardSeqRef.current++}`, grid_row: maxY, options: { ...src.options } }];
    });
  }
  function replaceCard(cardId: string, chart: Chart) {
    setDraftCards((prev) => (prev ?? dashboard.cards).map((c) => (c.id === cardId ? { ...c, chart_id: chart.id, options: {} } : c)));
    setReplacingId(null);
  }

  function addVirtualCard(card_type: "text" | "heading" | "link", options: Record<string, unknown>) {
    setDraftCards((prev) => {
      const base = prev ?? dashboard.cards;
      const maxY = base.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
      const newCard: Card = {
        id: `new-${card_type}-${cardSeqRef.current++}`, chart_id: null, card_type,
        grid_col: 0, grid_row: maxY, size_x: CARD_DEFAULT_SIZE.w, size_y: card_type === "heading" ? 1 : CARD_DEFAULT_SIZE.h, options,
      };
      return [...base, newCard];
    });
  }
  function updateCardOptions(cardId: string, patch: Record<string, unknown>) {
    setDraftCards((prev) => {
      const base = prev ?? dashboard.cards;
      return base.map((c) => (c.id === cardId ? { ...c, options: { ...c.options, ...patch } } : c));
    });
  }

  // The cards are the single source of truth for positions: the RGL layout is derived
  // from them, and drag/resize writes positions straight back into the draft. This keeps
  // every grid item sized from its card (a newly added card is never auto-placed at 1x1).
  const layout: Layout[] = cards.map((c) => ({
    i: c.id, x: c.grid_col, y: c.grid_row, w: c.size_x, h: c.size_y,
    // clamp the minimums to the card's own size so a short card (e.g. a 1-row
    // heading) never has minH/minW larger than its height/width (RGL warns).
    minW: Math.min(CARD_MIN_SIZE.w, c.size_x), minH: Math.min(CARD_MIN_SIZE.h, c.size_y),
  }));

  function onLayoutChange(next: Layout[]) {
    if (!editing) return;
    setDraftCards((prev) => {
      const base = prev ?? dashboard.cards;
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
    const payload: CardInput[] = cards.map((c) => ({
      chart_id: c.chart_id, card_type: c.card_type,
      grid_col: c.grid_col, grid_row: c.grid_row, size_x: c.size_x, size_y: c.size_y, options: c.options,
    }));
    try {
      await update.mutateAsync({ id: dashboard.id, name: dashboard.name, description: dashboard.description, parameters, cards: payload });
      setEditing(false); setDraftCards(null);
      toast.success("Saved");
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <div className={view.isFullscreen ? "fixed inset-0 z-50 overflow-auto bg-background p-4" : "p-4"}>
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
              <Button size="sm" variant="ghost" onClick={cancelEdit}>Cancel</Button>
              <Button size="sm" onClick={onSave} disabled={update.isPending}>Save</Button>
            </>
          ) : (
            <Button size="sm" variant="outline" onClick={startEdit}>Edit</Button>
          )}
        </div>
      </div>
      {editing && (
        <aside className="mb-3 rounded-md border p-2">
          <div className="mb-1 text-xs font-medium text-muted-foreground">Add a chart</div>
          <div className="flex flex-wrap gap-1">
            {(charts ?? []).map((c) => (
              <Button key={c.id} size="sm" variant="outline" aria-label={`add ${c.name}`} onClick={() => addCard(c)}>
                {c.name}
              </Button>
            ))}
          </div>
          <div className="mt-2 flex gap-1 border-t pt-2">
            <Button size="sm" variant="outline" aria-label="Add Text" onClick={() => addVirtualCard("text", { text: "" })}>+ Text</Button>
            <Button size="sm" variant="outline" aria-label="Add Heading" onClick={() => addVirtualCard("heading", { text: "" })}>+ Heading</Button>
            <Button size="sm" variant="outline" aria-label="Add Link" onClick={() => addVirtualCard("link", { url: "", label: "" })}>+ Link</Button>
          </div>
          <div className="mt-2 rounded-md border p-2">
            <div className="mb-1 flex items-center justify-between">
              <span className="text-xs font-medium text-muted-foreground">Filters</span>
              <Button size="sm" variant="outline" onClick={() => {
                const slug = slugify("filter", parameters.map((p) => p.slug));
                setDraftParameters([...parameters, { id: `new-${slug}-${cardSeqRef.current++}`, name: "Filter", slug, type: "text", operator: "is" }]);
              }}>+ Filter</Button>
            </div>
            <div className="flex flex-col gap-1">
              {parameters.map((p) => (
                <ParameterEditor key={p.id} param={p}
                  onChange={(next) => setDraftParameters(parameters.map((x) => x.id === p.id ? { ...next, slug: x.slug } : x))}
                  onDelete={() => setDraftParameters(parameters.filter((x) => x.id !== p.id))} />
              ))}
            </div>
          </div>
        </aside>
      )}
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
      <DashboardFilterBar parameters={parameters} values={paramsState.values} onChange={paramsState.setValue} optionsBySlug={optionsBySlug} />
      {cards.length === 0 ? (
        <p className="text-muted-foreground">No cards yet. Click Edit to add charts.</p>
      ) : (
        <Grid
          className="layout"
          cols={GRID_COLS}
          rowHeight={ROW_PX}
          isDraggable={editing}
          isResizable={editing}
          layout={layout}
          onLayoutChange={onLayoutChange}
        >
          {cards.map((c) => {
            const baseChart = chartById.get(c.chart_id ?? "");
            const mappings = (c.options.paramMappings as Record<string, string>) ?? {};
            const injected = baseChart ? applyParamsToChart(baseChart, parameters, paramsState.values, mappings) : undefined;
            return (
            <div key={c.id} className={`relative overflow-hidden ${c.card_type === "heading" ? "" : "rounded-md border bg-background"}`}>
              {editing && (
                <div className="absolute right-1 top-1 z-10 flex gap-1">
                  <button type="button" aria-label={`duplicate card ${c.id}`} className="rounded bg-background/80 px-1 text-xs"
                    onClick={() => duplicateCard(c.id)}>⧉</button>
                  {c.card_type === "chart" && (
                    <button type="button" aria-label={`replace card ${c.id}`} className="rounded bg-background/80 px-1 text-xs"
                      onClick={() => setReplacingId(c.id)}>↺</button>
                  )}
                  <button type="button" aria-label={`remove card ${chartById.get(c.chart_id ?? "")?.name ?? ""}`.trim() || "remove card"}
                    className="rounded bg-background/80 px-1 text-xs text-destructive" onClick={() => removeCard(c.id)}>✕</button>
                </div>
              )}
              {editing && c.card_type === "chart" && (
                <input aria-label="card title" placeholder="Title (blank = chart name)" className="m-1 w-[calc(100%-0.5rem)] rounded border px-1 text-xs"
                  value={(c.options.title as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { title: e.target.value })} />
              )}
              {editing && parameters.length > 0 && c.card_type === "chart" && (() => {
                const ch = chartById.get(c.chart_id ?? "");
                const cols = ch ? [...ch.params.spec.dimensions, ...ch.params.spec.columns] : [];
                const cardMappings = (c.options.paramMappings as Record<string, string>) ?? {};
                return (
                  <div className="m-1 flex flex-col gap-0.5 border-t pt-1">
                    {parameters.map((p) => (
                      <label key={p.id} className="flex items-center justify-between gap-1 text-[10px] text-muted-foreground">
                        <span>{p.name}</span>
                        <select aria-label={`map ${p.name}`} className="h-6 rounded border bg-background text-[10px]"
                          value={cardMappings[p.id] ?? ""}
                          onChange={(e) => updateCardOptions(c.id, { paramMappings: { ...cardMappings, [p.id]: e.target.value } })}>
                          <option value="">none</option>
                          {cols.map((col) => <option key={col} value={col}>{col}</option>)}
                        </select>
                      </label>
                    ))}
                  </div>
                );
              })()}
              {editing && c.card_type === "text" && (
                <textarea aria-label="text body" className="m-1 h-[calc(100%-1rem)] w-[calc(100%-0.5rem)] resize-none rounded border p-1 text-xs"
                  value={(c.options.text as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { text: e.target.value })} />
              )}
              {editing && c.card_type === "heading" && (
                <input aria-label="heading text" className="m-1 w-[calc(100%-0.5rem)] rounded border px-1 text-sm"
                  value={(c.options.text as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { text: e.target.value })} />
              )}
              {editing && c.card_type === "link" && (
                <div className="m-1 flex flex-col gap-1">
                  <input aria-label="link url" placeholder="https://…" className="rounded border px-1 text-xs"
                    value={(c.options.url as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { url: e.target.value })} />
                  <input aria-label="link label" placeholder="Label (optional)" className="rounded border px-1 text-xs"
                    value={(c.options.label as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { label: e.target.value })} />
                </div>
              )}
              {(!editing || c.card_type === "chart") && (
                <DashboardCardView ws={ws} card={c} chart={injected} refreshNonce={view.refreshNonce} parameters={parameters} paramValues={paramsState.values} />
              )}
            </div>
            );
          })}
        </Grid>
      )}
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
    </div>
  );
}
