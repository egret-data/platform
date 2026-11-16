import { useState } from "react";
import { Loader2 } from "lucide-react";
import { useWorkflowDeleteExecutionLogs } from "@/api/generated/workflow/workflow";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import type { LogFilters } from "@/components/workflows/executions/logs/useLogFollow";

const FALLBACK = "The logs could not be deleted. Try again.";

/**
 * Delete an execution's log lines by filter (package 7d D8; Kestra's `LogController.delete`),
 * confirmed first (FE-ACTION-1). `scope` says in words which lines `filters` select. It stays
 * open on failure with the server's message (FE-ACTION-3); on 204 it calls `onDeleted`, which
 * toasts and reads the view again, and closes.
 */
export function DeleteLogsDialog({
  ws,
  executionId,
  filters,
  scope,
  open,
  onOpenChange,
  onDeleted,
}: {
  ws: string;
  executionId: string;
  filters: LogFilters;
  scope: string;
  open: boolean;
  onOpenChange: (o: boolean) => void;
  onDeleted: () => void;
}) {
  const remove = useWorkflowDeleteExecutionLogs();
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const confirm = async () => {
    setPending(true);
    setError(null);
    try {
      const res = await remove.mutateAsync({
        workspace: ws,
        id: executionId,
        params: { task_id: filters.taskId, task_run_id: filters.taskRunId, min_level: filters.minLevel },
      });
      if (res.status !== 204) throw new Error(FALLBACK);
      onDeleted();
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error && e.message ? e.message : FALLBACK);
    } finally {
      setPending(false);
    }
  };

  return (
    <AlertDialog
      open={open}
      onOpenChange={(o) => {
        if (pending) return;
        if (!o) setError(null);
        onOpenChange(o);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete these logs?</AlertDialogTitle>
          <AlertDialogDescription>Deletes {scope}. Lines written after this stay. This cannot be undone.</AlertDialogDescription>
        </AlertDialogHeader>
        {error ? (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
          <Button type="button" variant="destructive" disabled={pending} onClick={() => void confirm()}>
            {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
            Delete logs
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
