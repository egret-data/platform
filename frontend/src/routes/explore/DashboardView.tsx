import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import RGL, { WidthProvider, type Layout } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
import { useDashboard, useDashboardMutations, type CardInput } from "@/api/dashboards";
import { useCharts, type Chart } from "@/api/charts";
import { useChartData } from "@/api/useChartData";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { Button } from "@/components/ui/button";
import { GRID_COLS, CARD_MIN_SIZE } from "@/components/chart/dashboard-grid";
import { toast } from "sonner";

const Grid = WidthProvider(RGL);
const ROW_PX = 90;

function CardTile({ ws, chart }: { ws: string; chart: Chart | undefined }) {
  const data = useChartData(ws, chart ?? null);
  if (!chart) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Chart not found</div>;
  return (
    <div className="flex h-full flex-col">
      <div className="border-b px-2 py-1 text-sm font-medium">{chart.name}</div>
      <div className="min-h-0 flex-1 overflow-auto p-1">
        {data.error ? <div className="text-sm text-destructive">{data.error}</div>
          : data.isLoading ? <div className="text-sm text-muted-foreground">Loading…</div>
          : <ChartRenderer vizType={chart.viz_type} params={chart.params} columns={data.columns} rows={data.rows} />}
      </div>
    </div>
  );
}

export default function DashboardView() {
  const ws = useParams().workspace!;
  const id = useParams().id!;
  const { data: dash, isLoading } = useDashboard(ws, id);
  const { data: charts } = useCharts(ws);
  const { update } = useDashboardMutations(ws);
  const [editing, setEditing] = useState(false);
  const [layout, setLayout] = useState<Layout[] | null>(null);
  const chartById = useMemo(() => new Map((charts ?? []).map((c) => [c.id, c])), [charts]);

  if (isLoading || !dash) return <div className="p-4 text-muted-foreground">Loading…</div>;

  const dashboard = dash;
  const cards = dashboard.cards;
  const baseLayout: Layout[] = cards.map((c) => ({
    i: c.id, x: c.grid_col, y: c.grid_row, w: c.size_x, h: c.size_y,
    minW: CARD_MIN_SIZE.w, minH: CARD_MIN_SIZE.h,
  }));
  const activeLayout = editing && layout ? layout : baseLayout;

  async function onSave() {
    const lay = layout ?? baseLayout;
    const byId = new Map(lay.map((l) => [l.i, l]));
    const payload: CardInput[] = cards.map((c) => {
      const l = byId.get(c.id)!;
      return { chart_id: c.chart_id, card_type: c.card_type, grid_col: l.x, grid_row: l.y, size_x: l.w, size_y: l.h, options: c.options };
    });
    try {
      await update.mutateAsync({ id: dashboard.id, name: dashboard.name, description: dashboard.description, cards: payload });
      setEditing(false); setLayout(null);
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
              <Button size="sm" variant="ghost" onClick={() => { setEditing(false); setLayout(null); }}>Cancel</Button>
              <Button size="sm" onClick={onSave} disabled={update.isPending}>Save</Button>
            </>
          ) : (
            <Button size="sm" variant="outline" onClick={() => setEditing(true)}>Edit</Button>
          )}
        </div>
      </div>
      {cards.length === 0 ? (
        <p className="text-muted-foreground">No cards yet. Click Edit to add charts.</p>
      ) : (
        <Grid
          className="layout"
          cols={GRID_COLS}
          rowHeight={ROW_PX}
          isDraggable={editing}
          isResizable={editing}
          layout={activeLayout}
          onLayoutChange={(l) => { if (editing) setLayout(l); }}
        >
          {cards.map((c) => (
            <div key={c.id} className="overflow-hidden rounded-md border bg-background">
              <CardTile ws={ws} chart={chartById.get(c.chart_id ?? "")} />
            </div>
          ))}
        </Grid>
      )}
    </div>
  );
}
