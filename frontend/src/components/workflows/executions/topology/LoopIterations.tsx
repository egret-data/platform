import { useEffect, useId, useRef, type RefObject } from "react";
import { Link } from "react-router-dom";
import { X } from "lucide-react";
import type { NodeState } from "@/api/generated/model";
import { IconButton } from "@/components/ui/icon-button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { StateChip } from "@/components/workflows/StateChip";
import { nodeSearch } from "@/components/workflows/executions/topology/selection";
import { iterationsText } from "@/components/workflows/topology/topologyModel";

/** Escape closes a panel of the tab from inside it, as the Gantt's run panel does. */
export function useEscapeCloses(panel: RefObject<HTMLElement | null>, onClose: () => void): void {
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      if (!(e.target instanceof Node) || !panel.current?.contains(e.target)) return;
      e.preventDefault();
      onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [panel, onClose]);
}

/**
 * A `Loop` body node's iterations (W7c spec §6.3): one row per iteration the
 * node ran in, with THE NODE'S OWN state there and a link to the iteration's
 * child execution, opened on its own Topology tab at the same task.
 *
 * The rows are the server's `states[uid].iterations.runs` (graph/states.rs):
 * the node's runs in each iteration folded by Kestra's priority, by
 * `loop_index`, every iteration listed. They are not the iterations' own
 * states, which another body task can decide, and an iteration where the node
 * never ran has no row.
 */
export function LoopIterations({
  ws,
  taskId,
  state,
  childUid,
  onClose,
}: {
  ws: string;
  taskId: string;
  /** The node's folded state, with its `iterations`. */
  state: NodeState;
  /** The node's uid inside an iteration; null links to the iteration without a selection. */
  childUid: string | null;
  onClose: () => void;
}) {
  const headingId = useId();
  const panel = useRef<HTMLElement>(null);
  useEscapeCloses(panel, onClose);
  const runs = state.iterations?.runs ?? [];
  const search = childUid === null ? "" : nodeSearch(childUid);

  return (
    <section ref={panel} aria-labelledby={headingId} data-testid="loop-iterations" className="rounded-md border p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <h2 id={headingId} className="truncate font-mono text-base font-semibold">
            {taskId}
          </h2>
          <StateChip state={state.state} kind="execution" />
          {state.iterations ? <span className="text-sm tabular-nums">{iterationsText(state.iterations)}</span> : null}
        </div>
        <IconButton aria-label="Close the iterations" onClick={onClose} className="size-8">
          <X aria-hidden className="size-4" />
        </IconButton>
      </div>
      <h3 className="mt-3 text-sm font-medium">Iterations</h3>
      {runs.length === 0 ? (
        <p className="mt-1 text-sm text-muted-foreground">{`${taskId} has not run in any iteration yet.`}</p>
      ) : (
        <Table className="mt-1">
          <TableHeader>
            <TableRow>
              <TableHead>Iteration</TableHead>
              <TableHead>{`State of ${taskId}`}</TableHead>
              <TableHead>Child execution</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {runs.map((run) => (
              <TableRow key={run.child_execution_id}>
                <TableCell className="tabular-nums">{`#${run.loop_index}`}</TableCell>
                <TableCell>
                  <StateChip state={run.state} kind="execution" />
                </TableCell>
                <TableCell>
                  <Link
                    className="font-mono text-xs hover:underline"
                    to={`/explore/${ws}/workflows/executions/${encodeURIComponent(run.child_execution_id)}/topology${search}`}
                    title={run.child_execution_id}
                  >
                    {run.child_execution_id.slice(0, 8)}
                  </Link>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </section>
  );
}
