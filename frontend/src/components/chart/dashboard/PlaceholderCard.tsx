import { Plus } from "lucide-react";

/** Edit-mode-only empty drop target produced by "Add section". Click to fill it with a chart.
 *  `dash-nodrag` so a click fills (the wrapper's top strip remains the drag handle). */
export function PlaceholderCard({ onPick }: { onPick: () => void }) {
  return (
    <button
      type="button"
      aria-label="add a chart"
      onClick={onPick}
      className="dash-nodrag flex h-full w-full flex-col items-center justify-center gap-1 rounded-sm border-2 border-dashed border-border bg-card/40 text-muted-foreground hover:border-primary hover:text-foreground"
    >
      <Plus className="h-5 w-5" />
      <span className="text-xs">Add a chart</span>
    </button>
  );
}
