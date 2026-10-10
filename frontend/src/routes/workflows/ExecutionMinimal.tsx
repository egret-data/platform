import { Link, useParams } from "react-router-dom";
import { ListTree, RotateCw } from "lucide-react";
import { ApiError } from "@/api/client";
import { useWorkflowGetExecution } from "@/api/generated/workflow/workflow";
import type { ExecutionDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { StateChip } from "@/components/workflows/StateChip";
import { formatDuration, isTerminalState } from "@/components/workflows/executionState";
import { Forbidden } from "@/routes/Forbidden";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** How often a running execution is read again (spec §6.3); egret's APIs are poll-only. */
export const EXECUTION_POLL_MS = 2000;

/**
 * `executions/:id`, the minimal execution page (W7a spec §6.3): what ran,
 * where it stands, and its task runs. W7b replaces it with the full detail.
 * It reads the execution every 2 s until the state is terminal.
 */
export default function ExecutionMinimal() {
  const { workspace = "", id = "" } = useParams();
  const query = useWorkflowGetExecution(workspace, id, {
    query: {
      queryKey: workflowsKeys.execution(id),
      refetchInterval: (q) => {
        const data = q.state.data;
        if (q.state.status === "error") return false;
        return data?.status === 200 && isTerminalState(data.data.state) ? false : EXECUTION_POLL_MS;
      },
    },
  });
  const execution = query.data?.status === 200 ? query.data.data : undefined;
  const flowsHref = `/explore/${workspace}/workflows/flows`;

  // Error first (FE-STATE-1), while nothing has loaded; a failed poll keeps the last answer on screen.
  if (query.isError && !execution) {
    if (query.error instanceof ApiError && query.error.status === 403) return <Forbidden />;
    const notFound = query.error instanceof ApiError && query.error.status === 404;
    return (
      <div className="p-4">
        <h1 className="sr-only">Execution</h1>
        {notFound ? (
          <EmptyState
            variant="dashed"
            title="Execution not found"
            description="It may belong to another workspace, or the link is wrong."
            action={
              <Button asChild variant="outline" size="sm">
                <Link to={flowsHref}>Back to flows</Link>
              </Button>
            }
          />
        ) : (
          <Alert variant="destructive">
            <AlertDescription className="flex flex-wrap items-center gap-3">
              <span>
                The execution could not be loaded: {query.error instanceof Error ? query.error.message : "the request failed"}. Try
                again.
              </span>
              <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
                <RotateCw aria-hidden />
                Retry
              </Button>
            </AlertDescription>
          </Alert>
        )}
      </div>
    );
  }
  if (!execution) {
    return (
      <div className="p-4" data-testid="execution-loading">
        <h1 className="sr-only">Execution</h1>
        <ListSkeleton cols={2} />
      </div>
    );
  }
  return <ExecutionView ws={workspace} execution={execution} readAt={query.dataUpdatedAt} />;
}

/** `readAt`: when this answer was read (`dataUpdatedAt`), the "now" of a running execution's duration. */
function ExecutionView({ ws, execution, readAt }: { ws: string; execution: ExecutionDto; readAt: number }) {
  const flowName = `${execution.namespace}.${execution.flow_key}`;
  const terminal = isTerminalState(execution.state);
  const started = new Date(execution.created_at);
  // Terminal: until the last write. Running: until this answer was read, so it moves with each poll.
  const end = terminal ? new Date(execution.updated_at).getTime() : readAt;
  const duration = formatDuration(end - started.getTime());
  const flowHref = execution.flow_id
    ? `/explore/${ws}/workflows/flows/${encodeURIComponent(execution.flow_id)}/edit`
    : null;

  return (
    <div className="grid gap-6 p-4">
      <h1 className="text-xl font-semibold">Execution</h1>

      <dl
        data-testid="execution-summary"
        className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 text-sm"
      >
        <dt className="text-muted-foreground">ID</dt>
        <dd className="font-mono text-xs leading-5">{execution.id}</dd>
        <dt className="text-muted-foreground">Flow</dt>
        <dd>
          {flowHref ? (
            <Link className="hover:underline" to={flowHref}>
              {flowName}
            </Link>
          ) : (
            // A flow deleted since is `flow_id: null`; its name is still what ran.
            flowName
          )}
        </dd>
        <dt className="text-muted-foreground">Namespace</dt>
        <dd>{execution.namespace}</dd>
        <dt className="text-muted-foreground">State</dt>
        <dd>
          <StateChip state={execution.state} kind="execution" />
        </dd>
        <dt className="text-muted-foreground">Started</dt>
        <dd className="tabular-nums">{started.toLocaleString()}</dd>
        <dt className="text-muted-foreground">Duration</dt>
        <dd className="tabular-nums">{terminal ? duration : `${duration} so far`}</dd>
      </dl>

      <section className="grid gap-2">
        <h2 className="text-base font-semibold">Task runs</h2>
        {execution.runs.length === 0 ? (
          <EmptyState
            icon={ListTree}
            title="No task runs yet"
            description={terminal ? "The execution ended before any task ran." : "Tasks appear here as they start."}
          />
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Task</TableHead>
                <TableHead>State</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {execution.runs.map((run) => (
                <TableRow key={run.id}>
                  <TableCell className="font-mono text-xs">{run.task_id}</TableCell>
                  <TableCell>
                    <StateChip state={run.state} kind="execution" />
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </section>
    </div>
  );
}
