import type { Chart } from "@/api/charts";
import { ChartPickerList } from "./ChartPickerList";

interface Props { charts: Chart[]; onAddChart: (c: Chart) => void; onAddVirtual: (t: "text" | "heading" | "link") => void }

export function AddPanel({ charts, onAddChart, onAddVirtual }: Props) {
  return (
    <div className="flex h-full flex-col gap-2 p-3">
      <div className="text-sm font-medium">Add a chart</div>
      <ChartPickerList charts={charts} onPick={onAddChart} />
      <div className="flex gap-1 border-t pt-2">
        <button type="button" aria-label="Add Text" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("text")}>+ Text</button>
        <button type="button" aria-label="Add Heading" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("heading")}>+ Heading</button>
        <button type="button" aria-label="Add Link" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("link")}>+ Link</button>
      </div>
    </div>
  );
}
