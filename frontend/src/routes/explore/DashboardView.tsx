import { useEffect, useMemo, useRef, useState } from "react";
import { useParams } from "react-router-dom";
import RGL, { WidthProvider, type Layout } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
import { useDashboard, useDashboardMutations, type CardInput, type DashboardCard as Card } from "@/api/dashboards";
import { useCharts, type Chart } from "@/api/charts";
import { Button } from "@/components/ui/button";
import { GRID_COLS, CARD_MIN_SIZE, CARD_DEFAULT_SIZE } from "@/components/chart/dashboard-grid";
import { DashboardCardView } from "@/components/chart/dashboard/DashboardCardView";
import { toast } from "sonner";

const Grid = WidthProvider(RGL);
const ROW_PX = 90;

export default function DashboardView() {
  const { workspace: ws, id } = useParams() as { workspace: string; id: string };
  const { data: dash, isLoading } = useDashboard(ws, id);
  const { data: charts } = useCharts(ws);
  const { update } = useDashboardMutations(ws);
  const [editing, setEditing] = useState(false);
  const [draftCards, setDraftCards] = useState<Card[] | null>(null);
  const cardSeqRef = useRef(0);
  const chartById = useMemo(() => new Map((charts ?? []).map((c) => [c.id, c])), [charts]);

  useEffect(() => {
    setEditing(false);
    setDraftCards(null);
  }, [id]);

  if (isLoading || !dash) return <div className="p-4 text-muted-foreground">Loading…</div>;

  const dashboard = dash;
  const cards = editing && draftCards ? draftCards : dashboard.cards;

  function startEdit() { setDraftCards(dashboard.cards.map((c) => ({ ...c }))); setEditing(true); }
  function cancelEdit() { setEditing(false); setDraftCards(null); }

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

  // The cards are the single source of truth for positions: the RGL layout is derived
  // from them, and drag/resize writes positions straight back into the draft. This keeps
  // every grid item sized from its card (a newly added card is never auto-placed at 1x1).
  const layout: Layout[] = cards.map((c) => ({
    i: c.id, x: c.grid_col, y: c.grid_row, w: c.size_x, h: c.size_y,
    minW: CARD_MIN_SIZE.w, minH: CARD_MIN_SIZE.h,
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
      await update.mutateAsync({ id: dashboard.id, name: dashboard.name, description: dashboard.description, cards: payload });
      setEditing(false); setDraftCards(null);
      toast.success("Saved");
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <div className="p-4">
      <div className="mb-3 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{dash.name}</h1>
        <div className="flex gap-2">
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
        </aside>
      )}
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
          {cards.map((c) => (
            <div key={c.id} className={`relative overflow-hidden ${c.card_type === "heading" ? "" : "rounded-md border bg-background"}`}>
              {editing && (
                <button
                  type="button"
                  aria-label={`remove card ${chartById.get(c.chart_id ?? "")?.name ?? ""}`.trim() || "remove card"}
                  className="absolute right-1 top-1 z-10 rounded bg-background/80 px-1 text-xs text-destructive"
                  onClick={() => removeCard(c.id)}
                >✕</button>
              )}
              <DashboardCardView ws={ws} card={c} chart={chartById.get(c.chart_id ?? "")} />
            </div>
          ))}
        </Grid>
      )}
    </div>
  );
}
