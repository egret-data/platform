import { Link } from "react-router-dom";
import { BarChart3 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { CHART_TYPES } from "@/components/chart/chartTypes";

/**
 * The chart type's own icon from the viz registry (KI-166). A viz type the registry does not know
 * (a chart written by an older or newer build) falls back to the generic bar icon, and the label
 * still names the type.
 */
export function ChartTypeIcon({ vizType, className }: { vizType: string; className?: string }) {
  const meta = CHART_TYPES.find((t) => t.key === vizType);
  const Icon = meta?.icon ?? BarChart3;
  return <Icon className={className} role="img" aria-label={meta?.name ?? vizType} />;
}

/**
 * The charts a delete or rename would affect, as named by the server: each with its type icon,
 * a link, and a tag when it is in the trash. Capped so a dataset with hundreds of charts does not
 * push the dialog's buttons off screen; the count is always the full one.
 */
export function ChartList({ ws, charts, max = 8 }: {
  ws: string;
  charts: { id: string; name: string; viz_type: string; trashed?: boolean }[];
  max?: number;
}) {
  const shown = charts.slice(0, max);
  const more = charts.length - shown.length;
  return (
    <ul className="max-h-48 space-y-1 overflow-y-auto rounded-md border p-2" aria-label="Affected charts">
      {shown.map((c) => (
        <li key={c.id} className="flex items-center gap-2 text-sm text-foreground">
          <ChartTypeIcon vizType={c.viz_type} className="h-4 w-4 shrink-0 text-muted-foreground" />
          <Link to={`/explore/${ws}/charts/${c.id}`} target="_blank" rel="noreferrer" className="min-w-0 truncate hover:underline">{c.name}</Link>
          {c.trashed && <Badge variant="secondary" className="text-xs">in Trash</Badge>}
        </li>
      ))}
      {more > 0 && <li className="text-xs text-muted-foreground">and {more} more</li>}
    </ul>
  );
}
