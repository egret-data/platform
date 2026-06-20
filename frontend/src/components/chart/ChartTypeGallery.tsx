import type { VizType } from "@/api/charts";
import { CHART_TYPES } from "./chartTypes";

export function ChartTypeGallery({
  selected, onSelect,
}: { selected?: VizType; onSelect: (key: VizType) => void }) {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
      {CHART_TYPES.map((t) => {
        const Icon = t.icon;
        const active = selected === t.key;
        return (
          <button
            key={t.key}
            type="button"
            onClick={() => onSelect(t.key)}
            className={`flex flex-col items-start gap-2 rounded-lg border p-4 text-left transition hover:border-primary/50 hover:bg-muted/40 ${
              active ? "border-primary ring-2 ring-primary/30" : ""
            }`}
          >
            <Icon className="h-6 w-6 text-primary" />
            <div className="font-semibold">{t.name}</div>
            <div className="text-xs text-muted-foreground">{t.description}</div>
          </button>
        );
      })}
    </div>
  );
}
