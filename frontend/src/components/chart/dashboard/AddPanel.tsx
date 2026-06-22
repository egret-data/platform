import { useState } from "react";
import { BarChart3 } from "lucide-react";
import type { Chart } from "@/api/charts";
import { CHART_TYPES } from "@/components/chart/chartTypes";

const ICON_BY_VIZ = new Map(CHART_TYPES.map((t) => [t.key, t.icon] as const));

interface Props { charts: Chart[]; onAddChart: (c: Chart) => void; onAddVirtual: (t: "text" | "heading" | "link") => void }

export function AddPanel({ charts, onAddChart, onAddVirtual }: Props) {
  const [q, setQ] = useState("");
  const shown = charts.filter((c) => c.name.toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="flex h-full flex-col gap-2 p-3">
      <div className="text-sm font-medium">Add a chart</div>
      <input aria-label="search charts" placeholder="Search charts…" className="h-8 w-full rounded border bg-background px-2 text-sm" value={q} onChange={(e) => setQ(e.target.value)} />
      <div className="-mx-1 flex max-h-[60vh] flex-col overflow-auto px-1">
        {shown.length === 0 ? <p className="px-1 py-2 text-xs text-muted-foreground">No charts</p>
          : shown.map((c) => {
            const Icon = ICON_BY_VIZ.get(c.viz_type) ?? BarChart3;
            return (
            <button key={c.id} type="button" aria-label={c.name} className="group flex items-center gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent/60" onClick={() => onAddChart(c)}>
              <Icon className="h-4 w-4 shrink-0 text-primary" aria-hidden />
              <span className="min-w-0 flex-1 truncate">{c.name}</span>
              <span className="shrink-0 text-xs text-primary opacity-0 transition-opacity group-hover:opacity-100">+ Add</span>
            </button>
            );
          })}
      </div>
      <div className="flex gap-1 border-t pt-2">
        <button type="button" aria-label="Add Text" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("text")}>+ Text</button>
        <button type="button" aria-label="Add Heading" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("heading")}>+ Heading</button>
        <button type="button" aria-label="Add Link" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("link")}>+ Link</button>
      </div>
    </div>
  );
}
