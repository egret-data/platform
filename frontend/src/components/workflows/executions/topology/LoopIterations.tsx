import { useEffect, useId, useRef } from "react";
import { Link } from "react-router-dom";
import { RotateCw, X } from "lucide-react";
import { useWorkflowListExecutions } from "@/api/generated/workflow/workflow";
import type { ExecutionDto, NodeState } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { IconButton } from "@/components/ui/icon-button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { StateChip } from "@/components/workflows/StateChip";
import { isTerminalState } from "@/components/workflows/executionState";
import { nextPollMs } from "@/components/workflows/executions/polling";
import { nodeSearch } from "@/components/workflows/executions/topology/selection";
import { iterationsText } from "@/components/workflows/topology/topologyModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** How many children the iteration list reads: the list route's maximum. */
export const ITERATIONS_LIMIT = 200;

const errorMessage = (e: unknown) => (e instanceof Error && e.message ? e.message : "the request failed");

/**
 * A `Loop` body node's iterations (W7c spec §6.3): one row per iteration of
 * the `Loop`'s run in this execution, with its state and a link to the child
 * execution that ran it, opened on its own Topology tab at the same task.
 *
 * The rows come from W7b's children list (`GET …/executions?parent=`), not
 * from the graph: `states` counts iterations by state but names none. A child
 * is an iteration of THIS loop when it carries a `loop_index` and its
 * `parent_run_ix` is the loop's run. The row's state is the iteration's (its
 * child execution's), not the node's own run in it.
 */
export function LoopIterations({
  ws,
  execution,
  taskId,
  state,
  loopRunIx,
  childUid,
  onClose,
}: {
  ws: string;
  execution: ExecutionDto;
  taskId: string;
  /** The node's folded state; absent while no iteration has run it. */
  state: NodeState | undefined;
  /** The `Loop`'s own run, or null while it has not started. */
  loopRunIx: number | null;
  /** The node's uid inside an iteration. */
  childUid: string;
  onClose: () => void;
}) {
  const headingId = useId();
  const panel = useRef<HTMLElement>(null);
  const terminal = isTerminalState(execution.state);
  const query = useWorkflowListExecutions(
    ws,
    { parent: execution.id, limit: ITERATIONS_LIMIT },
    {
      query: {
        queryKey: workflowsKeys.executionChildrenPage(execution.id, ITERATIONS_LIMIT),
        enabled: loopRunIx !== null,
        // Followed until the execution and every child have ended, as the Overview's children are.
        refetchInterval: (q) =>
          nextPollMs(q.state, Date.now(), (d) => terminal && d.status === 200 && d.data.rows.every((r) => isTerminalState(r.state))),
      },
    },
  );
  const page = query.data?.status === 200 ? query.data.data : undefined;
  const iterations = (page?.rows ?? [])
    .filter((r) => r.loop_index != null && r.parent_execution_id === execution.id && r.parent_run_ix === loopRunIx)
    .sort((a, b) => (a.loop_index ?? 0) - (b.loop_index ?? 0));

  // Escape closes the panel from inside it, as the Gantt's run panel does.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      if (!(e.target instanceof Node) || !panel.current?.contains(e.target)) return;
      e.preventDefault();
      onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  let body;
  if (loopRunIx === null) {
    body = <p className="text-sm text-muted-foreground">The Loop has not started in this execution, so it has no iterations yet.</p>;
  } else if (query.isError) {
    // Error first (FE-STATE-1); a failed refresh keeps the rows below.
    body = null;
  } else if (!page) {
    body = <ListSkeleton cols={3} rows={2} />;
  } else if (iterations.length === 0) {
    body = <p className="text-sm text-muted-foreground">No iteration has started yet.</p>;
  }

  return (
    <section ref={panel} aria-labelledby={headingId} data-testid="loop-iterations" className="rounded-md border p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <h2 id={headingId} className="truncate font-mono text-base font-semibold">
            {taskId}
          </h2>
          {state ? <StateChip state={state.state} kind="execution" /> : null}
          {state?.iterations ? <span className="text-sm tabular-nums">{iterationsText(state.iterations)}</span> : null}
        </div>
        <IconButton aria-label="Close the iterations" onClick={onClose} className="size-8">
          <X aria-hidden className="size-4" />
        </IconButton>
      </div>
      <h3 className="mt-3 text-sm font-medium">Iterations</h3>
      <div className="mt-1 grid gap-2">
        {query.isError ? (
          <Alert variant="destructive">
            <AlertDescription className="flex flex-wrap items-center gap-3">
              <span>
                The iterations could not be {page ? "refreshed" : "loaded"}: {errorMessage(query.error)}.
                {page ? " The list shows the last answer." : " Try again."}
              </span>
              <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
                <RotateCw aria-hidden />
                Retry
              </Button>
            </AlertDescription>
          </Alert>
        ) : null}
        {body}
        {iterations.length > 0 ? (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Iteration</TableHead>
                <TableHead>State</TableHead>
                <TableHead>Child execution</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {iterations.map((row) => (
                <TableRow key={row.id}>
                  <TableCell className="tabular-nums">
                    {`#${row.loop_index}`}
                    {row.loop_value != null ? <span className="ml-2 font-mono text-xs text-muted-foreground">{row.loop_value}</span> : null}
                  </TableCell>
                  <TableCell>
                    <StateChip state={row.state} kind="execution" />
                  </TableCell>
                  <TableCell>
                    <Link
                      className="font-mono text-xs hover:underline"
                      to={`/explore/${ws}/workflows/executions/${encodeURIComponent(row.id)}/topology${nodeSearch(childUid)}`}
                      title={row.id}
                    >
                      {row.id.slice(0, 8)}
                    </Link>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : null}
        {page && page.total > page.rows.length ? (
          <p className="text-sm text-muted-foreground tabular-nums">
            {`Only the first ${page.rows.length} of this execution's ${page.total} child executions were read, so later iterations are not listed.`}
          </p>
        ) : null}
      </div>
    </section>
  );
}
