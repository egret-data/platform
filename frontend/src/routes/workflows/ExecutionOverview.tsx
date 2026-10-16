import { useId, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { ChevronDown, ChevronRight, GitFork, ListTree, RotateCw, TextCursorInput } from "lucide-react";
import { useWorkflowListExecutions } from "@/api/generated/workflow/workflow";
import type { ExecutionDto, ExecutionListItemDto, TaskRunDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { StateChip } from "@/components/workflows/StateChip";
import { formatDuration, isTerminalState } from "@/components/workflows/executionState";
import { FileRow, FileRowList } from "@/components/workflows/executions/files/FileRow";
import { findFileUris, isFileUri, pathSteps } from "@/components/workflows/executions/outputs/outputsModel";
import { nextPollMs } from "@/components/workflows/executions/polling";
import {
  ESTIMATED_DURATION_TITLE,
  executionDuration,
  executionStart,
  startTitle,
} from "@/components/workflows/executions/timing";
import { TriggerLabel } from "@/components/workflows/executions/trigger";
import { timeAgo } from "@/lib/relativeTime";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** How many children the overview lists; the rest are counted, not shown. */
export const CHILDREN_LIMIT = 50;

const errorMessage = (e: unknown) => (e instanceof Error && e.message ? e.message : "the request failed");

/**
 * The Overview tab (W7b spec §5.3): the inputs, the trigger, the child
 * executions and the task runs. Kestra's `overview/Overview.vue` puts its
 * banner and topology here; egret's header carries the banner for every tab,
 * and topology arrives with W7c.
 */
export default function ExecutionOverview({ ws, execution, now }: { ws: string; execution: ExecutionDto; now: number }) {
  return (
    <div className="grid gap-6">
      <Section title="Inputs">
        <InputsTable ws={ws} executionId={execution.id} inputs={execution.inputs} />
      </Section>
      <Section title="Trigger">
        <TriggerSection execution={execution} />
      </Section>
      <Section title="Labels">
        <LabelChips labels={execution.effective_labels} />
      </Section>
      <Section title="Child executions">
        <ChildExecutions ws={ws} execution={execution} />
      </Section>
      <Section title="Task runs">
        <TaskRuns ws={ws} execution={execution} now={now} />
      </Section>
    </div>
  );
}

/** The labels an execution runs with, as `key: value` chips (W7f spec §4.4); "None" for none. */
function LabelChips({ labels }: { labels: { key: string; value: string }[] }) {
  if (labels.length === 0) return <p className="text-sm text-muted-foreground">None</p>;
  return (
    <ul className="flex flex-wrap gap-1.5" aria-label="Labels">
      {labels.map((l) => (
        <li key={l.key}>
          <Badge variant="outline" className="font-mono text-xs">
            {l.key}: {l.value}
          </Badge>
        </li>
      ))}
    </ul>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  const id = useId();
  return (
    <section className="grid gap-2" aria-labelledby={id}>
      <h2 id={id} className="text-base font-semibold">
        {title}
      </h2>
      {children}
    </section>
  );
}

/** A value as the inputs and trigger tables show it: text as itself, anything else as JSON. */
function plainValue(v: unknown): string {
  return typeof v === "string" ? v : JSON.stringify(v);
}

/**
 * An input's value (spec §5.3). A `kestra:///` string is a file row: its name,
 * size, Preview and Download. A structured value is shown as JSON, with a file
 * row under it for each file inside it (a multi-file input, an array of URIs).
 */
function InputValue({ ws, executionId, value }: { ws: string; executionId: string; value: unknown }) {
  if (isFileUri(value)) return <FileRow ws={ws} executionId={executionId} uri={value} />;
  const files =
    typeof value === "object" && value !== null
      ? findFileUris(value).map((f) => ({ key: JSON.stringify(f.path), uri: f.uri, label: pathSteps(f.path) }))
      : [];
  return (
    <div className="grid gap-1.5">
      <span className="whitespace-pre-wrap break-all font-mono text-xs">{plainValue(value)}</span>
      {files.length > 0 ? <FileRowList ws={ws} executionId={executionId} files={files} /> : null}
    </div>
  );
}

function InputsTable({ ws, executionId, inputs }: { ws: string; executionId: string; inputs: ExecutionDto["inputs"] }) {
  const entries = Object.entries(inputs ?? {});
  if (entries.length === 0) {
    return <EmptyState icon={TextCursorInput} title="No inputs" description="This execution was started without inputs." />;
  }
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Value</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {entries.map(([name, value]) => (
          <TableRow key={name}>
            <TableCell className="align-top font-mono text-xs">{name}</TableCell>
            <TableCell>
              <InputValue ws={ws} executionId={executionId} value={value} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

/** A structured trigger variable (a webhook's body, headers, parameters): collapsed JSON (spec §5.3). */
function JsonValue({ name, value }: { name: string; value: unknown }) {
  const [open, setOpen] = useState(false);
  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger asChild>
        <Button type="button" variant="ghost" size="sm" className="h-7 px-1.5">
          {open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}
          {`${open ? "Hide" : "Show"} ${name}`}
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <pre className="mt-1 max-h-80 overflow-auto rounded border bg-muted/40 p-2 font-mono text-xs">{JSON.stringify(value, null, 2)}</pre>
      </CollapsibleContent>
    </Collapsible>
  );
}

/**
 * What started the execution (spec §5.3): its kind, the trigger's id, and its
 * variables. Kestra's `ExecutionTrigger` is `{id, type, variables}`
 * (`trigger.tsx` names the types). An API start, a loop iteration, a retry and
 * a replay carry no record.
 */
function TriggerSection({ execution }: { execution: ExecutionDto }) {
  const record = execution.execution_trigger;
  const triggerId = record && typeof record.id === "string" ? record.id : null;
  const variables = record && typeof record.variables === "object" && record.variables !== null ? Object.entries(record.variables) : [];
  return (
    <div className="grid gap-2 text-sm">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
        <TriggerLabel row={execution} />
        {triggerId ? (
          <span className="inline-flex items-center gap-2">
            <span>
              <span className="text-muted-foreground">Trigger </span>
              <span className="font-mono text-xs">{triggerId}</span>
            </span>
            {execution.backfill_id ? <Badge variant="outline">Backfill</Badge> : null}
          </span>
        ) : null}
      </div>
      {variables.length === 0 ? (
        <p className="text-muted-foreground">{record ? "The trigger passed no variables." : "No trigger started this execution, so it has no trigger variables."}</p>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Variable</TableHead>
              <TableHead>Value</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {variables.map(([name, value]) => (
              <TableRow key={name}>
                <TableCell className="align-top font-mono text-xs">{name}</TableCell>
                <TableCell>
                  {typeof value === "object" && value !== null ? (
                    <JsonValue name={name} value={value} />
                  ) : (
                    <span className="whitespace-pre-wrap break-all font-mono text-xs">{plainValue(value)}</span>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </div>
  );
}

/**
 * The execution's direct children (spec §5.3): loop iterations, subflow and
 * flow-trigger children, and a retry's replacement, read with `parent=`. It
 * follows while the execution or a child is still running.
 */
function ChildExecutions({ ws, execution }: { ws: string; execution: ExecutionDto }) {
  const terminal = isTerminalState(execution.state);
  const query = useWorkflowListExecutions(
    ws,
    { parent: execution.id, limit: CHILDREN_LIMIT },
    {
      query: {
        queryKey: workflowsKeys.executionChildren(execution.id),
        refetchInterval: (q) =>
          nextPollMs(q.state, Date.now(), (d) => terminal && d.status === 200 && d.data.rows.every((r) => isTerminalState(r.state))),
      },
    },
  );
  const page = query.data?.status === 200 ? query.data.data : undefined;

  // Error first (FE-STATE-1): a failed refresh keeps the rows and says so.
  const error = query.isError ? (
    <Alert variant="destructive">
      <AlertDescription className="flex flex-wrap items-center gap-3">
        <span>
          The child executions could not be {page ? "refreshed" : "loaded"}: {errorMessage(query.error)}.
          {page ? " The list shows the last answer." : " Try again."}
        </span>
        <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
          <RotateCw aria-hidden />
          Retry
        </Button>
      </AlertDescription>
    </Alert>
  ) : null;
  if (!page) return error ?? <ListSkeleton cols={4} rows={2} />;
  if (page.rows.length === 0) {
    return (
      <>
        {error}
        <EmptyState icon={GitFork} title="No child executions" description="No loop, subflow or flow trigger started an execution from this one." />
      </>
    );
  }
  return (
    <div className="grid gap-2">
      {error}
      <ChildTable ws={ws} rows={page.rows} />
      {page.total > page.rows.length ? (
        <p className="text-sm text-muted-foreground tabular-nums">{`Showing the first ${page.rows.length} of ${page.total} child executions.`}</p>
      ) : null}
    </div>
  );
}

function ChildTable({ ws, rows }: { ws: string; rows: ExecutionListItemDto[] }) {
  const base = `/explore/${ws}/workflows`;
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Execution</TableHead>
          <TableHead>Started by</TableHead>
          <TableHead>Iteration</TableHead>
          <TableHead>State</TableHead>
          <TableHead>Start</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((row) => {
          const start = executionStart(row);
          return (
            <TableRow key={row.id}>
              <TableCell>
                <Link
                  className="font-mono text-xs hover:underline"
                  to={`${base}/executions/${encodeURIComponent(row.id)}/overview`}
                  title={row.id}
                >
                  {row.id.slice(0, 8)}
                </Link>
              </TableCell>
              <TableCell>
                <TriggerLabel row={row} />
              </TableCell>
              <TableCell className="tabular-nums">
                {row.loop_index != null ? (
                  <>
                    {`#${row.loop_index}`}
                    {row.loop_value != null ? <span className="ml-2 font-mono text-xs text-muted-foreground">{row.loop_value}</span> : null}
                  </>
                ) : (
                  <span className="text-muted-foreground">—</span>
                )}
              </TableCell>
              <TableCell>
                <StateChip state={row.state} kind="execution" />
              </TableCell>
              <TableCell className="text-muted-foreground">
                <span title={startTitle(start)}>{start.kind === "created" ? "Not started" : timeAgo(start.iso)}</span>
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

/** A run's duration, by the same per-field rule as an execution's (`timing.ts`). */
function RunDuration({ run, now }: { run: TaskRunDto; now: number }) {
  const duration = executionDuration(run, now);
  if (duration === null) return <span className="text-muted-foreground">—</span>;
  if (duration.estimated) {
    return (
      <span title={ESTIMATED_DURATION_TITLE}>
        ≈ {formatDuration(duration.ms)}
        <span className="sr-only"> (estimated)</span>
      </span>
    );
  }
  // Still running: measured to now, and said so, as the header does (review M-7).
  return <>{duration.running ? `${formatDuration(duration.ms)} so far` : formatDuration(duration.ms)}</>;
}

/**
 * Every task run, in `run_ix` order (spec §5.3). The task id opens the Gantt
 * with that run selected (`gantt?run=<run_ix>`, spec §6.4).
 */
function TaskRuns({ ws, execution, now }: { ws: string; execution: ExecutionDto; now: number }) {
  const gantt = `/explore/${ws}/workflows/executions/${encodeURIComponent(execution.id)}/gantt`;
  if (execution.runs.length === 0) {
    return (
      <EmptyState
        icon={ListTree}
        title="No task runs yet"
        description={isTerminalState(execution.state) ? "The execution ended before any task ran." : "Tasks appear here as they start."}
      />
    );
  }
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Task</TableHead>
          <TableHead>Value</TableHead>
          <TableHead>State</TableHead>
          <TableHead>Duration</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {execution.runs.map((run) => (
          <TableRow key={run.id}>
            <TableCell>
              <Link className="font-mono text-xs hover:underline" to={`${gantt}?run=${run.run_ix}`}>
                {run.task_id}
              </Link>
            </TableCell>
            <TableCell className="font-mono text-xs text-muted-foreground">{run.value ?? "—"}</TableCell>
            <TableCell>
              <StateChip state={run.state} kind="execution" />
            </TableCell>
            <TableCell className="tabular-nums">
              <RunDuration run={run} now={now} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
