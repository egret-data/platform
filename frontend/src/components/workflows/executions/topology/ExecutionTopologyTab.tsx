import { useCallback, useEffect, useMemo, useRef } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { keepPreviousData } from "@tanstack/react-query";
import { RotateCw } from "lucide-react";
import { useWorkflowGetExecutionGraph, useWorkflowListExecutions } from "@/api/generated/workflow/workflow";
import type { ExecutionDto, FlowGraph, NodeState } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
import { isTerminalState } from "@/components/workflows/executionState";
import { buildGantt } from "@/components/workflows/executions/gantt/ganttModel";
import { GanttRunPanel, type GanttRunSlots } from "@/components/workflows/executions/gantt/GanttView";
import { nextPollMs } from "@/components/workflows/executions/polling";
import { ITERATIONS_LIMIT, LoopIterations } from "@/components/workflows/executions/topology/LoopIterations";
import { nodeSearch, selectionOf } from "@/components/workflows/executions/topology/selection";
import { TopologyView } from "@/components/workflows/topology/TopologyView";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

const NO_STATES: Readonly<Record<string, NodeState>> = {};

const errorMessage = (e: unknown) => (e instanceof Error && e.message ? e.message : "The request did not complete.");

/** A node's task or trigger id, for sentences; the uid when the graph does not have it. */
function nameOf(graph: FlowGraph | undefined, uid: string): string {
  const n = graph?.nodes.find((x) => x.uid === uid);
  if (n?.node === "task") return n.task_id;
  if (n?.node === "trigger") return n.trigger_id;
  return uid;
}

/**
 * THE EXECUTION'S TOPOLOGY TAB (W7c spec §6.3): the graph of the execution's
 * frozen source, coloured by its runs' states, drawn by {@link TopologyView}.
 *
 * - **Updates.** `GET executions/{id}/graph` on W7b's polling backoff
 *   (`nextPollMs`): every 2 s while the execution runs, longer while reads
 *   fail. It stops once the execution is terminal, after one last read for the
 *   final states (the execution's own poll, in `ExecutionDetail`, is what sees
 *   it end).
 * - **Selection** is `?node=<uid>` (FE-PAGE-2), a pushed entry as the Gantt's
 *   `?run=` is. A node with a run opens the Gantt's own `GanttRunPanel` with
 *   the same logs and outputs slots; a `Loop` body node opens its iterations
 *   (`LoopIterations`); a node of an expanded `Subflow` links to the child
 *   execution that ran it.
 * - **Expansion** is `?expand=<uid>`, repeated: a shared link draws the same
 *   Subflows open.
 *
 * This module is the Topology chunk's entry from the execution detail, which
 * reaches it through `lazy()` only (`scripts/assert-workflows-lazy.mjs`).
 */
export default function ExecutionTopologyTab({
  ws,
  execution,
  slots,
  now,
}: {
  ws: string;
  execution: ExecutionDto;
  slots: GanttRunSlots;
  /** The detail's clock, for a running run's duration in the panel. */
  now: number;
}) {
  const [params, setParams] = useSearchParams();
  const selected = params.get("node");
  const expandedRaw = params.getAll("expand");
  const expandKey = expandedRaw.join("\n");
  const expanded = useMemo(() => (expandKey ? expandKey.split("\n") : []), [expandKey]);
  const terminal = isTerminalState(execution.state);
  const container = useRef<HTMLDivElement>(null);

  const query = useWorkflowGetExecutionGraph(
    ws,
    execution.id,
    { subflows: expanded },
    {
      query: {
        queryKey: workflowsKeys.executionGraph(execution.id, expanded),
        // An expansion keeps the drawn graph until its answer comes (FE-STATE-1).
        placeholderData: keepPreviousData,
        refetchInterval: (q) => nextPollMs(q.state, Date.now(), () => terminal),
      },
    },
  );
  const answer = query.data?.status === 200 ? query.data.data : undefined;
  const graph = answer?.graph;
  const states = answer?.states ?? NO_STATES;

  // One more read when the execution ends: the last poll may predate its final states.
  const ranLive = useRef(!terminal);
  const { refetch } = query;
  useEffect(() => {
    if (terminal && ranLive.current) {
      ranLive.current = false;
      void refetch();
    }
  }, [terminal, refetch]);

  // Pushed entries, so Back walks the selection (FE-PAGE-2); none when nothing changes.
  const select = useCallback(
    (uid: string | null) => {
      if (uid === selected) return;
      setParams((prev) => {
        const next = new URLSearchParams(prev);
        if (uid === null) next.delete("node");
        else next.set("node", uid);
        return next;
      });
    },
    [selected, setParams],
  );
  const onExpandSubflow = useCallback(
    (uid: string) => {
      setParams((prev) => {
        if (prev.getAll("expand").includes(uid)) return prev;
        const next = new URLSearchParams(prev);
        next.append("expand", uid);
        return next;
      });
    },
    [setParams],
  );
  // Closing puts focus back on the node rather than on `body`.
  const close = useCallback(() => {
    const uid = selected;
    select(null);
    if (uid !== null) {
      container.current?.querySelector<HTMLElement>(`[data-uid="${CSS.escape(uid)}"]`)?.focus();
    }
  }, [select, selected]);

  const selection = graph && selected !== null ? selectionOf(selected, graph, states) : null;
  const runId = selection?.kind === "run" ? selection.runId : null;
  // The Gantt's own row for the run, so the panel shows exactly what the Gantt's does.
  const row = runId ? buildGantt(execution, now).rows.find((r) => r.run.id === runId) : undefined;
  const selectedRun = row?.run;
  // The slots render once per run, not on every clock tick, as in the Gantt tab.
  const logsSlot = slots.logs;
  const outputsSlot = slots.outputs;
  const logs = useMemo(() => (selectedRun ? logsSlot(selectedRun) : null), [selectedRun, logsSlot]);
  const outputs = useMemo(() => (selectedRun ? outputsSlot(selectedRun) : null), [selectedRun, outputsSlot]);

  const runIxOf = (uid: string): number | null => {
    const id = states[uid]?.run_id;
    return execution.runs.find((r) => r.id === id)?.run_ix ?? null;
  };

  let panel = null;
  let note: string | null = null;
  if (selected !== null && graph && selection) {
    const name = nameOf(graph, selected);
    if (!graph.nodes.some((n) => n.uid === selected)) {
      note = `${selected} is not a node of this execution's graph. The link may be from another execution.`;
    } else if (selection.kind === "run") {
      if (row) {
        // Keyed by run: a slot's own state (a log cursor) starts afresh for each run.
        panel = <GanttRunPanel key={row.run.id} row={row} onClose={close} logs={logs} outputs={outputs} />;
      } else {
        note = `The run of ${name} is not in this execution's task runs yet.`;
      }
    } else if (selection.kind === "iterations") {
      panel = (
        <LoopIterations
          key={selected}
          ws={ws}
          execution={execution}
          taskId={name}
          state={states[selected]}
          loopRunIx={runIxOf(selection.loop)}
          childUid={selection.childUid}
          onClose={close}
        />
      );
    } else if (selection.kind === "child") {
      panel = (
        <SubflowChild
          ws={ws}
          execution={execution}
          name={name}
          subflowName={nameOf(graph, selection.subflow)}
          subflowRunIx={runIxOf(selection.subflow)}
          rest={selection.rest}
        />
      );
    } else {
      note = `${name} has not run in this execution.`;
    }
  }

  const busy = query.isFetching && (!answer || query.isPlaceholderData);
  const error = query.isError ? `The graph could not be ${answer ? "refreshed" : "loaded"}: ${errorMessage(query.error)}.` : null;

  return (
    <div ref={container} className="flex flex-col gap-2">
      {query.isError ? (
        <div className="flex justify-end">
          <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
            <RotateCw aria-hidden />
            Retry
          </Button>
        </div>
      ) : null}
      <div className="h-[32rem] min-h-0">
        <TopologyView
          graph={graph}
          states={states}
          selectedUid={selected}
          onSelect={select}
          onExpandSubflow={onExpandSubflow}
          busy={busy}
          error={error}
        />
      </div>
      {/* Present before it changes, so a screen reader announces the note (FE-A11Y). */}
      <p role="status" className="rounded-md border p-4 text-sm empty:hidden">
        {note}
      </p>
      {panel}
    </div>
  );
}

/**
 * A node of an expanded `Subflow`: its run is in the child execution, found
 * among this execution's children by the Subflow's run (`trigger_run_ix`).
 */
function SubflowChild({
  ws,
  execution,
  name,
  subflowName,
  subflowRunIx,
  rest,
}: {
  ws: string;
  execution: ExecutionDto;
  name: string;
  subflowName: string;
  subflowRunIx: number | null;
  rest: string;
}) {
  const query = useWorkflowListExecutions(
    ws,
    { parent: execution.id, limit: ITERATIONS_LIMIT },
    {
      query: {
        queryKey: workflowsKeys.executionChildrenPage(execution.id, ITERATIONS_LIMIT),
        enabled: subflowRunIx !== null,
      },
    },
  );
  const rows = query.data?.status === 200 ? query.data.data.rows : [];
  const child = rows.find((r) => r.trigger_execution_id === execution.id && r.trigger_run_ix === subflowRunIx);
  return (
    <div className="flex flex-wrap items-center gap-3 rounded-md border p-4 text-sm">
      <span>{`${name} is a task of the flow the Subflow ${subflowName} called; its runs are in that child execution.`}</span>
      {child ? (
        <Button asChild variant="outline" size="sm">
          <Link to={`/explore/${ws}/workflows/executions/${encodeURIComponent(child.id)}/topology${nodeSearch(rest)}`}>
            Open the child execution
          </Link>
        </Button>
      ) : query.isError ? (
        <span className="text-muted-foreground">{`The child execution could not be found: ${errorMessage(query.error)}.`}</span>
      ) : null}
    </div>
  );
}
