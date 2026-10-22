import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useDatasetUsage } from "@/api/datasets";
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
