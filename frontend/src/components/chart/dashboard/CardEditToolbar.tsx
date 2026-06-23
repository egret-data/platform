import { Settings, BarChart3, Copy, RefreshCw, Trash2 } from "lucide-react";

interface Props {
  cardType: string;
  onSettings: () => void;
  onViz: () => void;
  onDuplicate: () => void;
  onReplace: () => void;
  onRemove: () => void;
}

export function CardEditToolbar({ cardType, onSettings, onViz, onDuplicate, onReplace, onRemove }: Props) {
  const btn = "inline-flex size-7 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground";
  return (
    <div className="dash-nodrag flex items-center gap-0.5 rounded-md border bg-card p-0.5 shadow-sm">
      <button type="button" aria-label="card settings" title="Settings" className={btn} onClick={onSettings}><Settings className="h-4 w-4" /></button>
      {cardType === "chart" && <button type="button" aria-label="card visualization" title="Visualization" className={btn} onClick={onViz}><BarChart3 className="h-4 w-4" /></button>}
      <button type="button" aria-label="duplicate card" title="Duplicate" className={btn} onClick={onDuplicate}><Copy className="h-4 w-4" /></button>
      {cardType === "chart" && <button type="button" aria-label="replace card" title="Replace chart" className={btn} onClick={onReplace}><RefreshCw className="h-4 w-4" /></button>}
      <button type="button" aria-label="remove card" title="Remove" className={`${btn} text-destructive hover:text-destructive`} onClick={onRemove}><Trash2 className="h-4 w-4" /></button>
    </div>
  );
}
