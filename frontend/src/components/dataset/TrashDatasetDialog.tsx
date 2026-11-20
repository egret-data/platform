import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useDatasetUsage, useDatasetsUsage } from "@/api/datasets";
import { ChartList } from "@/components/dataset/ChartList";

/**
 * Move a dataset to the Trash (KI-111). Trashing hides the dataset but deletes nothing: its charts
 * keep running, and each says its dataset is in the Trash. The dialog says that BEFORE the move,
 * with the charts it means, because "Trash" otherwise reads as "gone" and the charts' owners are
 * told nothing.
 */
export function TrashDatasetDialog({ ws, dataset, onOpenChange, onConfirm }: {
  ws: string;
  /** The dataset to trash; `null` keeps the dialog closed. */
  dataset: { id: string; name: string } | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const usage = useDatasetUsage(ws, dataset?.id ?? null);
  const charts = usage.data ?? [];
  const n = charts.length;
  const description = usage.isPending
    ? "Checking which charts use this dataset…"
    : usage.isError
      ? "Could not check which charts use this dataset. Any that do will keep working until the dataset is deleted."
      : n === 0
        ? "No charts use this dataset. You can restore it from the Trash."
        : `${n} ${n === 1 ? "chart uses" : "charts use"} this dataset and will keep working until the dataset is deleted. You can restore it from the Trash.`;
  return (
    <AlertDialog open={dataset !== null} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Move “{dataset?.name}” to Trash?</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        {n > 0 && <ChartList ws={ws} charts={charts} />}
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>Move to Trash</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** Words for the charts a bulk trash leaves running (KI-252); shared by the dialog and the toast. */
export function bulkChartsNotice(chartCount: number): string {
  return chartCount === 0
    ? "No charts use these datasets."
    : `${chartCount} ${chartCount === 1 ? "chart uses" : "charts use"} these datasets and will keep working until the datasets are deleted.`;
}

/**
 * Move several datasets to the Trash (KI-252): the single dialog's notice about the charts that use
 * them, for the selection as a whole, with each affected dataset's charts listed. `onConfirm`
 * receives the number of distinct charts affected, for the toast that follows.
 */
export function TrashDatasetsDialog({ ws, datasets, onOpenChange, onConfirm }: {
  ws: string;
  /** The datasets to trash; `null` keeps the dialog closed. */
  datasets: { id: string; name: string }[] | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: (chartCount: number) => void;
}) {
  const list = datasets ?? [];
  const usage = useDatasetsUsage(ws, list.map((d) => d.id));
  const distinct = new Map<string, { id: string; name: string; viz_type: string; trashed?: boolean }>();
  for (const d of list) for (const c of usage.byId[d.id] ?? []) distinct.set(c.id, c);
  const charts = [...distinct.values()];
  const n = list.length;
  const description = usage.isPending
    ? "Checking which charts use these datasets…"
    : usage.isError
      ? "Could not check which charts use these datasets. Any that do will keep working until the datasets are deleted."
      : `${bulkChartsNotice(charts.length)} You can restore them from the Trash.`;
  return (
    <AlertDialog open={datasets !== null} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Move {n} {n === 1 ? "dataset" : "datasets"} to Trash?</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        {charts.length > 0 && <ChartList ws={ws} charts={charts} />}
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction disabled={usage.isPending} onClick={() => onConfirm(charts.length)}>
            Move to Trash
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
