import { useState } from "react";
import type { Chart } from "@/api/charts";

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
          : shown.map((c) => (
            <button key={c.id} type="button" aria-label={c.name} className="rounded px-2 py-1.5 text-left text-sm hover:bg-accent/40" onClick={() => onAddChart(c)}>{c.name}</button>
          ))}
      </div>
      <div className="flex gap-1 border-t pt-2">
        <button type="button" aria-label="Add Text" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("text")}>+ Text</button>
        <button type="button" aria-label="Add Heading" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("heading")}>+ Heading</button>
        <button type="button" aria-label="Add Link" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("link")}>+ Link</button>
      </div>
    </div>
  );
}
