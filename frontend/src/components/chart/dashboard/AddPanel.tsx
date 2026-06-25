import type { Chart } from "@/api/charts";
import { ChartPickerList } from "./ChartPickerList";

interface Props { charts: Chart[]; onAddChart: (c: Chart) => void }

export function AddPanel({ charts, onAddChart }: Props) {
  return (
    <div className="flex h-full flex-col gap-2 p-3">
      <div className="text-sm font-medium">Add a chart</div>
      <ChartPickerList charts={charts} onPick={onAddChart} />
    </div>
  );
}
