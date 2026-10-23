import { lazy, useId, useMemo, useState, type ReactNode } from "react";
import { Link, Navigate, useNavigate, useParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { CircleStop, History, Loader2, Play, RotateCw } from "lucide-react";
import { ApiError } from "@/api/client";
import {
  useWorkflowGetExecution,
  useWorkflowKillExecution,
  useWorkflowReplayExecution,
  useWorkflowResumeExecution,
} from "@/api/generated/workflow/workflow";
import type { ExecutionDto, TaskRunDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { LazyRouteArea } from "@/components/layout/LazyRouteArea";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
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
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import { StateChip } from "@/components/workflows/StateChip";
import { formatDuration, isTerminalState } from "@/components/workflows/executionState";
import { WorkerOffNotice } from "@/components/workflows/WorkerOffNotice";
import ExecutionGanttTab from "@/components/workflows/executions/gantt/ExecutionGanttTab";
import type { GanttRunSlots } from "@/components/workflows/executions/gantt/GanttView";
import ExecutionLogsTab from "@/components/workflows/executions/logs/ExecutionLogsTab";
import { LogView } from "@/components/workflows/executions/logs/LogView";
import ExecutionOutputsTab from "@/components/workflows/executions/outputs/ExecutionOutputsTab";
import { OutputsTree } from "@/components/workflows/executions/outputs/OutputsTree";
import {
  canKill,
  canReplay,
  canResume,
  killFeedback,
  resumeFeedback,
  showsKill,
  showsResume,
} from "@/components/workflows/executions/actions";
import { nextExecutionPollMs, staleNote, useNow } from "@/components/workflows/executions/polling";
import {
  ESTIMATED_DURATION_TITLE,
  executionDuration,
  executionStart,
  startTitle,
} from "@/components/workflows/executions/timing";
import { TriggerLabel } from "@/components/workflows/executions/trigger";
import { fullTimestamp } from "@/lib/relativeTime";
import { Forbidden } from "@/routes/Forbidden";
import ExecutionOverview from "@/routes/workflows/ExecutionOverview";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { DetailHeader } from "@/components/layout/DetailHeader";

/**
 * The execution's Topology tab (W7c spec §6.3). Through `lazy()` only: it brings `@xyflow/react`
 * and `@dagrejs/dagre`, which the rest of the page never needs (`scripts/assert-workflows-lazy.mjs`).
 */
const ExecutionTopologyTab = lazy(() => import("@/components/workflows/executions/topology/ExecutionTopologyTab"));

/**
 * The execution detail's tabs (W7b spec §4), Kestra's minus Metrics and Dependencies (§2), in
 * Kestra's order; W7c added Topology (its spec §6.3).
 */
export const EXECUTION_TABS = ["overview", "topology", "gantt", "logs", "outputs"] as const;
type ExecutionTab = (typeof EXECUTION_TABS)[number];
const isExecutionTab = (t: string | undefined): t is ExecutionTab => (EXECUTION_TABS as readonly string[]).includes(t ?? "");

const errorMessage = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);

/**
 * `executions/:id/:tab` (W7b spec §4, §5): the execution's header, its
 * actions, and its tabs, in the URL (FE-PAGE-2). An unknown tab goes to
 * `overview`. Keyed by the execution, so a replay's new execution starts from
 * its own state. Kestra's `ExecutionRoot.vue`: a top bar with the actions, a
 * banner, then the tabs.
 */
export default function ExecutionDetail() {
  const { workspace = "", id = "", tab } = useParams();
  if (!isExecutionTab(tab)) {
    return <Navigate to={`/explore/${workspace}/workflows/executions/${encodeURIComponent(id)}/overview`} replace />;
  }
  return <ExecutionLoader key={id} ws={workspace} id={id} tab={tab} />;
}

function ExecutionLoader({ ws, id, tab }: { ws: string; id: string; tab: ExecutionTab }) {
  const query = useWorkflowGetExecution(ws, id, {
    query: {
      queryKey: workflowsKeys.execution(id),
      // Every 2 s until terminal, backing off to 30 s while reads fail (spec §5.1).
      refetchInterval: (q) => nextExecutionPollMs(q.state, Date.now()),
    },
  });
  const execution = query.data?.status === 200 ? query.data.data : undefined;
  const listHref = `/explore/${ws}/workflows/executions`;

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
                <Link to={listHref}>Back to executions</Link>
              </Button>
            }
          />
        ) : (
          <Alert variant="destructive">
            <AlertDescription className="flex flex-wrap items-center gap-3">
              <span>The execution could not be loaded: {errorMessage(query.error, "the request failed")}. Try again.</span>
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
  return (
    <div className="flex h-full min-h-0 flex-col">
      {query.isError ? (
        <div className="px-4 pt-4">
          <Alert variant="destructive">
            <AlertDescription className="flex flex-wrap items-center gap-3">
              <span>
                The execution could not be refreshed: {errorMessage(query.error, "the request failed")}.{" "}
                {staleNote(query.dataUpdatedAt)}
              </span>
              <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
                <RotateCw aria-hidden />
                Retry
              </Button>
            </AlertDescription>
          </Alert>
        </div>
      ) : null}
      {/* KI-013: an execution that has not ended stays where it is while the worker is off. */}
      {!execution.worker_enabled && !isTerminalState(execution.state) ? (
        <div className="px-4 pt-4">
          <WorkerOffNotice />
        </div>
      ) : null}
      <ExecutionFrame ws={ws} tab={tab} execution={execution} />
    </div>
  );
}

function ExecutionFrame({ ws, tab, execution }: { ws: string; tab: ExecutionTab; execution: ExecutionDto }) {
  const navigate = useNavigate();
  const base = `/explore/${ws}/workflows`;
  const href = `${base}/executions/${encodeURIComponent(execution.id)}`;
  const now = useNow();
  // Stable across polls: the Gantt renders a slot once per selected run (Task 4).
  const ganttSlots = useMemo<GanttRunSlots>(
    () => ({
      logs: (run: TaskRunDto) => <RunLogs ws={ws} executionId={execution.id} run={run} />,
      // The panel is keyed by run, so the tree's open items start afresh for each run.
      outputs: (run: TaskRunDto) => <OutputsTree ws={ws} executionId={execution.id} run={run} loopValue={execution.loop_value} />,
    }),
    [ws, execution.id, execution.loop_value],
  );

  return (
    <>
      <ExecutionHeader ws={ws} execution={execution} now={now} />
      <Tabs value={tab} onValueChange={(t) => navigate(`${href}/${t}`)} className="mt-3 flex min-h-0 flex-1 flex-col">
        <TabsList className="mx-4 self-start">
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="topology">Topology</TabsTrigger>
          <TabsTrigger value="gantt">Gantt</TabsTrigger>
          <TabsTrigger value="logs">Logs</TabsTrigger>
          <TabsTrigger value="outputs">Outputs</TabsTrigger>
        </TabsList>
        <TabsContent value="overview" className="min-h-0 flex-1 overflow-auto p-4">
          <ExecutionOverview ws={ws} execution={execution} now={now} />
        </TabsContent>
        <TabsContent value="topology" className="min-h-0 flex-1 overflow-auto p-4">
          <LazyRouteArea area="Topology" fallback={<Skeleton className="h-[32rem] w-full" />}>
            <ExecutionTopologyTab ws={ws} execution={execution} slots={ganttSlots} now={now} />
          </LazyRouteArea>
        </TabsContent>
        <TabsContent value="gantt" className="min-h-0 flex-1 overflow-auto p-4">
          <ExecutionGanttTab execution={execution} slots={ganttSlots} />
        </TabsContent>
        <TabsContent value="logs" className="flex min-h-0 flex-1 flex-col overflow-auto p-4">
          <ExecutionLogsTab ws={ws} execution={execution} />
        </TabsContent>
        <TabsContent value="outputs" className="min-h-0 flex-1 overflow-auto p-4">
          <ExecutionOutputsTab ws={ws} execution={execution} />
        </TabsContent>
      </Tabs>
    </>
  );
}

/** A run's lines in the Gantt's run panel: the Logs tab's view, filtered to the run, ungrouped. */
function RunLogs({ ws, executionId, run }: { ws: string; executionId: string; run: TaskRunDto }) {
  const filters = useMemo(() => ({ taskRunId: run.id }), [run.id]);
  const runs = useMemo(() => [run], [run]);
  return (
    <LogView
      ws={ws}
      executionId={executionId}
      filters={filters}
      runs={runs}
      grouped={false}
      heightClass="h-72"
      label={`Logs of ${run.task_id} #${run.run_ix}`}
      downloadName={`${executionId}-${run.task_id}-run-${run.run_ix}.log`}
    />
  );
}

/** One `dt`/`dd` pair of the header's summary. */
function Fact({ term, children }: { term: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline gap-1.5">
      <dt className="text-muted-foreground">{term}</dt>
      <dd>{children}</dd>
    </div>
  );
}

/**
 * The header every tab shares (spec §5.1): id, flow, namespace and state;
 * start and duration; the trigger; and the "Replayed from" and "Child of"
 * links. Kestra's `overview/components/Banner.vue`, on egret's visuals.
 */
function ExecutionHeader({ ws, execution, now }: { ws: string; execution: ExecutionDto; now: number }) {
  const base = `/explore/${ws}/workflows`;
  const execHref = (id: string) => `${base}/executions/${encodeURIComponent(id)}/overview`;
  const flowName = `${execution.namespace}.${execution.flow_key}`;
  const start = executionStart(execution);
  const duration = executionDuration(execution, now);
  // A loop iteration and a retry carry `parent_execution_id`; a subflow or flow-trigger child `trigger_execution_id`.
  const parent = execution.parent_execution_id ?? execution.trigger_execution_id ?? null;

  return (
    <DetailHeader
      className="px-4 pt-4"
      back={{ to: `${base}/executions`, label: "Executions" }}
      title={execution.id}
      titleClassName="min-w-0 truncate font-mono"
      status={<StateChip state={execution.state} kind="execution" />}
      meta={
        <dl data-testid="execution-summary" className="flex flex-wrap gap-x-6 gap-y-1 text-sm">
          <Fact term="Flow">
            {execution.flow_id ? (
              <Link className="hover:underline" to={`${base}/flows/${encodeURIComponent(execution.flow_id)}/edit`}>
                {flowName}
              </Link>
            ) : (
              // A flow deleted for good is `flow_id: null`; its name is still what ran.
              flowName
            )}
          </Fact>
          <Fact term="Namespace">{execution.namespace}</Fact>
          <Fact term="Started">
            <span className="tabular-nums" title={startTitle(start)}>
              {start.kind === "created" ? (
                "Not started"
              ) : start.kind === "estimated" ? (
                <>
                  ≈ {fullTimestamp(start.iso)}
                  <span className="sr-only"> (estimated)</span>
                </>
              ) : (
                fullTimestamp(start.iso)
              )}
            </span>
          </Fact>
          <Fact term="Duration">
            <span className="tabular-nums">
              {duration === null ? (
                <span className="text-muted-foreground">—</span>
              ) : duration.estimated ? (
                <span title={ESTIMATED_DURATION_TITLE}>
                  ≈ {formatDuration(duration.ms)}
                  <span className="sr-only"> (estimated)</span>
                </span>
              ) : duration.running ? (
                `${formatDuration(duration.ms)} so far`
              ) : (
                formatDuration(duration.ms)
              )}
            </span>
          </Fact>
          <Fact term="Trigger">
            <TriggerLabel row={execution} />
          </Fact>
          {execution.replayed_from_execution_id ? (
            <Fact term="Replayed from">
              <Link className="font-mono text-xs hover:underline" to={execHref(execution.replayed_from_execution_id)}>
                {execution.replayed_from_execution_id}
              </Link>
            </Fact>
          ) : null}
          {parent ? (
            <Fact term="Child of">
              <Link className="font-mono text-xs hover:underline" to={execHref(parent)}>
                {parent}
              </Link>
            </Fact>
          ) : null}
        </dl>
      }
      actions={
        /* Every action needs `workflow:manage`; a user without it does not see them (spec §5.2). */
        execution.can_manage ? <ExecutionActions ws={ws} execution={execution} /> : null
      }
    />
  );
}

type Pending = "resume" | "replay" | "kill" | null;

/** The kill route's 409 code: the execution ended before the kill reached it (workflow kill spec D3). */
const EXECUTION_TERMINATED = "execution_terminated";

/**
 * Resume, Replay (spec §5.2) and Kill (workflow kill spec §7), each confirmed
 * first (FE-ACTION-1), each saying why when disabled (FE-ACTION-4).
 */
function ExecutionActions({ ws, execution }: { ws: string; execution: ExecutionDto }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const resume = useWorkflowResumeExecution();
  const replay = useWorkflowReplayExecution();
  const kill = useWorkflowKillExecution();
  const [open, setOpen] = useState<Pending>(null);
  // "Also kill subflow executions": on by default, and on again each time the dialog opens.
  const [cascade, setCascade] = useState(true);
  // Set by a 409 execution_terminated: the dialog stays open to say so, with nothing left to confirm.
  const [killEnded, setKillEnded] = useState(false);
  const reasonsId = useId();

  const resumeShown = showsResume(execution);
  const resumeState = canResume(execution);
  const replayState = canReplay(execution);
  const killShown = showsKill(execution);
  const killState = canKill(execution);

  const runResume = async () => {
    const res = await resume.mutateAsync({ workspace: ws, id: execution.id });
    if (res.status !== 200) throw new Error("The execution could not be resumed. Try again.");
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.execution(execution.id) });
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.executions(ws) });
    // The page stays, so a toast confirms it (FE-ACTION-2), in terms of the
    // state the route answered, not of what was asked.
    const feedback = resumeFeedback(res.data.state);
    if (feedback.tone === "success") toast.success(feedback.message);
    else toast.warning(feedback.message);
  };

  const runReplay = async () => {
    const res = await replay.mutateAsync({ workspace: ws, id: execution.id });
    if (res.status !== 201) throw new Error("The execution could not be replayed. Try again.");
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.executions(ws) });
    // The new execution is the result: go there, no toast (FE-ACTION-2).
    navigate(`/explore/${ws}/workflows/executions/${encodeURIComponent(res.data.id)}/overview`);
  };

  const runKill = async () => {
    const refresh = () => {
      void queryClient.invalidateQueries({ queryKey: workflowsKeys.execution(execution.id) });
      void queryClient.invalidateQueries({ queryKey: workflowsKeys.executions(ws) });
    };
    try {
      const res = await kill.mutateAsync({ workspace: ws, id: execution.id, params: { cascade } });
      if (res.status !== 202) throw new Error("The execution could not be killed. Try again.");
      refresh();
      // The page stays, so a toast confirms it (FE-ACTION-2), in terms of the state the route
      // answered. `KILLING` is not terminal: the page keeps polling until it is `KILLED`.
      toast.success(killFeedback(res.data.state));
    } catch (e) {
      if (e instanceof ApiError && e.code === EXECUTION_TERMINATED) {
        // It ended first: read it again so the page shows how, and say so in the dialog.
        setKillEnded(true);
        refresh();
        throw new Error("The execution has already ended, so there is nothing to kill. The page now shows its final state.", {
          cause: e,
        });
      }
      throw e;
    }
  };

  const inputNames = Object.keys(execution.inputs ?? {});

  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex flex-wrap items-center justify-end gap-2">
        {resumeShown ? (
          <ReasonButton enablement={resumeState} reasonId={`${reasonsId}-resume`} onClick={() => setOpen("resume")}>
            <Play aria-hidden />
            Resume
          </ReasonButton>
        ) : null}
        <ReasonButton
          enablement={replayState}
          reasonId={`${reasonsId}-replay`}
          variant="outline"
          onClick={() => setOpen("replay")}
        >
          <History aria-hidden />
          Replay
        </ReasonButton>
        {killShown ? (
          <ReasonButton
            enablement={killState}
            reasonId={`${reasonsId}-kill`}
            variant="outline"
            onClick={() => {
              setCascade(true);
              setKillEnded(false);
              setOpen("kill");
            }}
          >
            {killState.ok ? <CircleStop aria-hidden /> : <Loader2 aria-hidden className="animate-spin" />}
            {killState.ok ? "Kill" : "Killing…"}
          </ReasonButton>
        ) : null}
      </div>
      <DisabledReasons
        actions={[
          ...(resumeShown ? [{ id: `${reasonsId}-resume`, label: "Resume", enablement: resumeState }] : []),
          { id: `${reasonsId}-replay`, label: "Replay", enablement: replayState },
          ...(killShown ? [{ id: `${reasonsId}-kill`, label: "Kill", enablement: killState }] : []),
        ]}
      />
      <ConfirmAction
        open={open === "resume"}
        onOpenChange={(o) => setOpen(o ? "resume" : null)}
        title="Resume this execution?"
        description="Every paused task ends now as a success, whatever its duration or timeout, and the execution continues with the tasks after it."
        confirmLabel="Resume"
        onConfirm={runResume}
        fallbackError="The execution could not be resumed. Try again."
      />
      <ConfirmAction
        open={open === "replay"}
        onOpenChange={(o) => setOpen(o ? "replay" : null)}
        title="Replay this execution?"
        description="Replay starts a new execution from this execution's frozen YAML and inputs. Edits made to the flow since it ran are not used."
        confirmLabel="Replay"
        onConfirm={runReplay}
        fallbackError="The execution could not be replayed. Try again."
      >
        <p className="text-sm text-muted-foreground">
          {inputNames.length === 0 ? "It has no inputs." : `Inputs: ${inputNames.join(", ")}.`}
        </p>
      </ConfirmAction>
      <ConfirmAction
        open={open === "kill"}
        onOpenChange={(o) => setOpen(o ? "kill" : null)}
        title="Kill this execution?"
        description="Every running task is stopped, and every task that has not finished ends Killed. A killed execution cannot be resumed; replay it to run it again."
        confirmLabel="Kill"
        confirmVariant="destructive"
        confirmDisabled={killEnded || !killShown}
        onConfirm={runKill}
        fallbackError="The execution could not be killed. Try again."
      >
        <div className="flex items-start gap-2">
          <Checkbox
            id={`${reasonsId}-cascade`}
            checked={cascade}
            onCheckedChange={(v) => setCascade(v === true)}
            aria-describedby={`${reasonsId}-cascade-help`}
            className="mt-0.5"
          />
          <div className="grid gap-0.5">
            <Label htmlFor={`${reasonsId}-cascade`}>Also kill subflow executions</Label>
            <p id={`${reasonsId}-cascade-help`} className="text-sm text-muted-foreground">
              Its loop iterations and subflow executions, and theirs. Unchecked, they keep running.
            </p>
          </div>
        </div>
      </ConfirmAction>
    </div>
  );
}

/**
 * A confirmation that stays open on failure with the server's message
 * (FE-ACTION-3) and closes on success. Not `AlertDialogAction`, which closes
 * on click whatever the outcome. `confirmDisabled` turns the confirm off when
 * the failure left nothing to confirm (Kill's 409 execution_terminated); Cancel
 * still closes it.
 */
function ConfirmAction({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  confirmVariant,
  confirmDisabled = false,
  onConfirm,
  fallbackError,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  confirmLabel: string;
  confirmVariant?: "default" | "destructive";
  confirmDisabled?: boolean;
  onConfirm: () => Promise<void>;
  fallbackError: string;
  children?: ReactNode;
}) {
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const confirm = async () => {
    setPending(true);
    setError(null);
    try {
      await onConfirm();
      onOpenChange(false);
    } catch (e) {
      setError(errorMessage(e, fallbackError));
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
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        {children}
        {error ? (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
          <Button type="button" variant={confirmVariant} disabled={pending || confirmDisabled} onClick={() => void confirm()}>
            {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
            {confirmLabel}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
