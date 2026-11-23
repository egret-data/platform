import { useWorkflowExecutionAverageDuration } from "@/api/generated/workflow/workflow";
import { Progress } from "@/components/ui/progress";
import { isTerminalState } from "@/components/workflows/executionState";
import { executionDuration, type TimedExecution } from "@/components/workflows/executions/timing";

/**
 * How far a running execution is, as a share of its flow's average duration (7e #6).
 * The average keys on the namespace and the flow's KEY (its YAML id), never the uuid.
 * An estimate, so it stops at 100% and says so; with no history to average it shows nothing.
 */
export function AverageDurationProgress({
  ws,
  execution,
  now,
}: {
  ws: string;
  execution: TimedExecution & { namespace: string; flow_key: string };
  now: number;
}) {
  const running = !isTerminalState(execution.state);
  const query = useWorkflowExecutionAverageDuration(ws, execution.namespace, execution.flow_key, {
    query: { enabled: running, staleTime: 60_000 },
  });
  const avg = query.data?.status === 200 ? query.data.data : undefined;
  const elapsed = executionDuration(execution, now);
  if (!running || !avg || !avg.avgDurationMs || avg.avgDurationMs <= 0 || avg.count === 0 || elapsed === null) return null;

  const share = elapsed.ms / avg.avgDurationMs;
  const percent = Math.min(100, Math.round(share * 100));
  return (
    <div className="flex items-center gap-2 text-sm" data-testid="average-duration-progress">
      <Progress value={percent} className="h-2 w-32" aria-label="Estimated progress" />
      <span className="tabular-nums text-muted-foreground">
        {share > 1 ? "longer than usual" : `about ${percent}% of the usual run`}
      </span>
    </div>
  );
}
