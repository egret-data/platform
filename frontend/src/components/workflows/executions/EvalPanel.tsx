import { useId, useState } from "react";
import {
  useWorkflowEvalExecutionExpression,
  useWorkflowEvalTaskRunExpression,
} from "@/api/generated/workflow/workflow";
import type { EvalResultDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

/**
 * Evaluates one expression against an execution's stored context (7e #1a), or
 * a task run's (#1b) when `taskRunId` is given. The generated mutation sends the
 * string as it is, `Content-Type: text/plain`; the server answers `result` or `error`.
 */
export function EvalPanel({ ws, executionId, taskRunId }: { ws: string; executionId: string; taskRunId?: string }) {
  const id = useId();
  const [expression, setExpression] = useState("");
  const [outcome, setOutcome] = useState<EvalResultDto | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const forExecution = useWorkflowEvalExecutionExpression();
  const forRun = useWorkflowEvalTaskRunExpression();
  const pending = forExecution.isPending || forRun.isPending;

  async function evaluate() {
    setOutcome(null);
    setFailure(null);
    try {
      const res =
        taskRunId === undefined
          ? await forExecution.mutateAsync({ workspace: ws, id: executionId, data: expression })
          : await forRun.mutateAsync({ workspace: ws, id: executionId, taskRunId, data: expression });
      if (res.status !== 200) throw new Error("The expression could not be evaluated.");
      setOutcome(res.data);
    } catch (e) {
      setFailure(e instanceof Error && e.message ? e.message : "The expression could not be evaluated.");
    }
  }

  const empty = expression.trim() === "";
  const error = failure ?? outcome?.error ?? null;
  return (
    <div className="grid gap-2" data-testid="eval-panel">
      <Label htmlFor={`${id}-expr`}>Expression</Label>
      <Textarea
        id={`${id}-expr`}
        className="font-mono text-xs"
        rows={2}
        value={expression}
        onChange={(e) => setExpression(e.target.value)}
        placeholder="{{ outputs.task.value }}"
      />
      <div className="flex flex-wrap items-center gap-3">
        <Button size="sm" variant="outline" disabled={empty || pending} onClick={() => void evaluate()}>
          Evaluate
        </Button>
        {empty ? <span className="text-sm text-muted-foreground">Type an expression to evaluate.</span> : null}
      </div>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : outcome ? (
        <pre data-testid="eval-result" className="whitespace-pre-wrap break-words rounded-md bg-muted p-2 font-mono text-xs">
          {outcome.result ?? ""}
        </pre>
      ) : null}
    </div>
  );
}
