import { toast } from "sonner";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { useDatasetUsage, useDeleteDataset } from "@/api/datasets";
import { ChartList } from "@/components/dataset/ChartList";

/**
 * Delete a dataset (D8, KI-098). The charts built on it are deleted WITH it (the database
 * cascades), so the dialog lists them — the trashed ones too, they go as well — and then asks for
 * the dataset's NAME rather than the word "delete". The server enforces the same rule: without
 * the name such a delete is a 409.
 */
export function DeleteDatasetDialog({ ws, dataset, onOpenChange, onDeleted, toastOnSuccess = true }: {
  ws: string;
  /** The dataset to delete; `null` keeps the dialog closed. */
  dataset: { id: string; name: string } | null;
  onOpenChange: (open: boolean) => void;
  onDeleted?: () => void;
  /** Off for a caller that navigates away on success (FE-ACTION-2: no toast then). */
  toastOnSuccess?: boolean;
}) {
  const usage = useDatasetUsage(ws, dataset?.id ?? null, { includeTrashed: true });
  const del = useDeleteDataset(ws);
  const charts = usage.data ?? [];
  const n = charts.length;
  const name = dataset?.name ?? "";

  const description = usage.isError
    ? "Could not check which charts use this dataset, so it cannot be deleted right now."
    : n === 0
      ? "This removes the dataset and its column snapshot. The underlying table or query is not affected. This cannot be undone."
      : `${n} ${n === 1 ? "chart is" : "charts are"} built on this dataset and will be deleted with it. The underlying table or query is not affected. This cannot be undone.`;

  return (
    <ConfirmDeleteDialog
      open={dataset !== null}
      onOpenChange={onOpenChange}
      title={`Delete dataset “${name}”?`}
      description={description}
      // With charts at stake the confirmation is the dataset's own name, exactly — the same value
      // the server checks.
      confirmText={n > 0 ? name : "delete"}
      caseSensitive={n > 0}
      confirmLabel={n > 0 ? `Delete dataset and ${n} ${n === 1 ? "chart" : "charts"}` : "Delete"}
      // Until the chart list is known the dialog cannot say what the delete takes with it.
      confirmDisabled={usage.isPending || usage.isError}
      onConfirm={() => {
        if (!dataset) return;
        del.mutate(
          { id: dataset.id, confirm: n > 0 ? dataset.name : undefined },
          {
            onSuccess: () => {
              if (toastOnSuccess) toast.success(n > 0 ? `Deleted the dataset and ${n} ${n === 1 ? "chart" : "charts"}` : "Deleted");
              onDeleted?.();
            },
            onError: (e) => toast.error(e.message),
          },
        );
      }}
    >
      {n > 0 && <ChartList ws={ws} charts={charts} />}
    </ConfirmDeleteDialog>
  );
}
