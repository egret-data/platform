import { useState } from "react";
import { BarChart3 } from "lucide-react";
import type { Chart } from "@/api/charts";
import { ICON_BY_VIZ } from "./viz-icons";

interface Props {
  charts: Chart[];
  onPick: (c: Chart) => void;
  autoFocus?: boolean;
}

export function ChartPickerList({ charts, onPick, autoFocus }: Props) {
  const [q, setQ] = useState("");
  const shown = charts.filter((c) => c.name.toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="flex flex-col gap-2">
      <input autoFocus={autoFocus} aria-label="search charts" placeholder="Search charts…" className="h-8 w-full rounded border bg-background px-2 text-sm" value={q} onChange={(e) => setQ(e.target.value)} />
      <div className="-mx-1 flex max-h-[60vh] flex-col overflow-auto px-1">
        {shown.length === 0 ? <p className="px-1 py-2 text-xs text-muted-foreground">No charts</p>
          : shown.map((c) => {
            const Icon = ICON_BY_VIZ.get(c.viz_type) ?? BarChart3;
            return (
              <button key={c.id} type="button" aria-label={c.name} className="flex items-center gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent/60" onClick={() => onPick(c)}>
                <Icon className="h-4 w-4 shrink-0 text-primary" aria-hidden />
                <span className="min-w-0 flex-1 truncate">{c.name}</span>
              </button>
            );
          })}
      </div>
    </div>
  );
}
