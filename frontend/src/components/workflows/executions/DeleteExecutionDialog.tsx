import { useId, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";
import { ApiError } from "@/api/client";
import { useWorkflowDeleteExecution } from "@/api/generated/workflow/workflow";
import type { ExecutionDto } from "@/api/generated/model";
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
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { deleteRefusal, dependentsOf } from "@/components/workflows/executions/actions";
import { useRefreshExecution } from "@/components/workflows/executions/useExecutionAction";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

const FALLBACK = "The execution could not be deleted. Try again.";

/** What the dialog says after a refusal: a sentence, and for `has_dependents` the executions to delete first. */
interface Failure {
  message: string;
  dependents?: { executions: string[]; total: number };
}

function failureOf(e: unknown): Failure {
  const dependents = dependentsOf(e);
  const code = e instanceof ApiError ? e.code : undefined;
  const sentence = deleteRefusal(code);
  if (dependents) return { message: sentence ?? (e as Error).message, dependents };
  if (sentence) return { message: sentence };
  return { message: e instanceof Error && e.message ? e.message : FALLBACK };
}

/**
 * Delete an execution (package 7d D1-D4, D10; Kestra's `ExecutionController.delete`), confirmed
 * first (FE-ACTION-1). It stays open on a refusal with the server's words (FE-ACTION-3); a
 * `has_dependents` refusal lists the executions to delete first. On success it opens the
 * executions list and toasts (FE-ACTION-2, spec correction S14: the list is not the result).
 */
export function DeleteExecutionDialog({
  ws,
  execution,
  open,
  onOpenChange,
}: {
  ws: string;
  execution: ExecutionDto;
  open: boolean;
  onOpenChange: (o: boolean) => void;
}) {
  const id = useId();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const refresh = useRefreshExecution(ws, execution.id);
  const remove = useWorkflowDeleteExecution();
  // Checked again each time the dialog opens: a close resets it.
  const [deleteStorage, setDeleteStorage] = useState(true);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [pending, setPending] = useState(false);

  const confirm = async () => {
    setPending(true);
    setFailure(null);
    try {
      const res = await remove.mutateAsync({ workspace: ws, id: execution.id, params: { deleteStorage } });
      if (res.status !== 204) throw new Error(FALLBACK);
      queryClient.removeQueries({ queryKey: workflowsKeys.execution(execution.id) });
      void queryClient.invalidateQueries({ queryKey: workflowsKeys.executions(ws) });
      onOpenChange(false);
      navigate(`/explore/${ws}/workflows/executions`);
      toast.success(`Deleted execution ${execution.id}`);
    } catch (e) {
      // A coded refusal says the page may be stale: read the execution again.
      if (e instanceof ApiError && deleteRefusal(e.code)) refresh();
      setFailure(failureOf(e));
    } finally {
      setPending(false);
    }
  };

  const more = failure?.dependents ? failure.dependents.total - failure.dependents.executions.length : 0;

  return (
    <AlertDialog
      open={open}
      onOpenChange={(o) => {
        if (pending) return;
        if (!o) {
          setDeleteStorage(true);
          setFailure(null);
        }
        onOpenChange(o);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete this execution?</AlertDialogTitle>
          <AlertDialogDescription>
            Deletes execution {execution.id} with its task runs, logs, loop iterations and retries. This cannot be undone.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="flex items-start gap-2">
          <Checkbox
            id={`${id}-storage`}
            checked={deleteStorage}
            onCheckedChange={(v) => setDeleteStorage(v === true)}
            aria-describedby={`${id}-storage-help`}
            className="mt-0.5"
          />
          <div className="grid gap-0.5">
            <Label htmlFor={`${id}-storage`}>Delete stored files</Label>
            <p id={`${id}-storage-help`} className="text-sm text-muted-foreground">
              The files its tasks and inputs stored. Unchecked, they stay in storage.
            </p>
          </div>
        </div>
        {failure ? (
          <Alert variant="destructive">
            <AlertDescription>
              <p>{failure.message}</p>
              {failure.dependents ? (
                <ul role="list" className="mt-1 grid gap-0.5">
                  {failure.dependents.executions.map((dep) => (
                    <li key={dep}>
                      <Link
                        className="font-mono text-xs underline-offset-2 hover:underline"
                        to={`/explore/${ws}/workflows/executions/${encodeURIComponent(dep)}/overview`}
                      >
                        {dep}
                      </Link>
                    </li>
                  ))}
                  {more > 0 ? <li className="tabular-nums">and {more} more</li> : null}
                </ul>
              ) : null}
            </AlertDescription>
          </Alert>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
          <Button type="button" variant="destructive" disabled={pending} onClick={() => void confirm()}>
            {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
            Delete
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
