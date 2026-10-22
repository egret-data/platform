import { Link } from "react-router-dom";
import { BarChart3, ExternalLink } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { useDatasetUsage } from "@/api/datasets";
import { ChartTypeIcon } from "@/components/dataset/ChartList";

interface Props {
  ws: string;
  datasetId: string;
}

// Lists the charts built on this dataset — the "where is this used?" view
// (Superset's Charts tab / Metabase's usage panel). Read-only.
export function UsageTab({ ws, datasetId }: Props) {
  const { data, isLoading, isError, error } = useDatasetUsage(ws, datasetId);

  if (isLoading) {
    return <div className="space-y-2">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-10 w-full" />)}</div>;
  }
  if (isError) {
    return (
      <Alert variant="destructive">
        <AlertDescription>{(error as Error)?.message ?? "Failed to load usage."}</AlertDescription>
      </Alert>
    );
  }
  if (!data || data.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-3 rounded-md border py-10 text-center">
        <BarChart3 className="h-8 w-8 text-muted-foreground opacity-40" />
        <p className="text-sm text-muted-foreground">No charts use this dataset yet.</p>
        <Button asChild size="sm" variant="outline">
          <Link to={`/explore/${ws}/charts/new?dataset=${datasetId}`}>Create a chart</Link>
        </Button>
      </div>
    );
  }

  return (
    <div>
      <p className="mb-3 text-xs text-muted-foreground">
        {data.length} chart{data.length === 1 ? "" : "s"} built on this dataset
      </p>
      <ul className="divide-y rounded-md border">
        {data.map((c) => (
          <li key={c.id}>
            <Link
              to={`/explore/${ws}/charts/${c.id}`}
              className="flex items-center gap-2 px-3 py-2.5 text-sm hover:bg-accent/40"
            >
              <ChartTypeIcon vizType={c.viz_type} className="h-4 w-4 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate">{c.name}</span>
              <ExternalLink className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
