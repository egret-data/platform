import { useId, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Loader2, Plus, X } from "lucide-react";
import {
  useWorkflowChangeStatusExecutionsByIds,
  useWorkflowChangeStatusExecutionsByQuery,
  useWorkflowDeleteExecutionsByIds,
  useWorkflowDeleteExecutionsByQuery,
  useWorkflowKillExecutionsByIds,
  useWorkflowKillExecutionsByQuery,
  useWorkflowLabelsExecutionsByIds,
  useWorkflowLabelsExecutionsByQuery,
  useWorkflowPauseExecutionsByIds,
  useWorkflowPauseExecutionsByQuery,
  useWorkflowReplayExecutionsByIds,
  useWorkflowReplayExecutionsByQuery,
  useWorkflowRestartExecutionsByIds,
  useWorkflowRestartExecutionsByQuery,
  useWorkflowResumeExecutionsByIds,
  useWorkflowResumeExecutionsByQuery,
} from "@/api/generated/workflow/workflow";
import type { BulkResponse, ExecutionListItemDto, Invalid, LabelDto } from "@/api/generated/model";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import { changeTargets, type ChangeTarget } from "@/components/workflows/executions/actions";
import { BULK_ACTIONS, bulkEligibility, bulkErrorOf, queryFilterParams, type BulkAction } from "@/components/workflows/executions/bulkModel";
import { labelsFromRows, rowErrors, type LabelRow } from "@/components/workflows/executions/labelsModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** The filter, as the by-query routes take it (no paging). */
type QueryFilter = ReturnType<typeof queryFilterParams>;

/** "1 execution", "3 executions" (FE-COPY-4: the page writes its own plural). */
const executions = (n: number) => `${n} ${n === 1 ? "execution" : "executions"}`;

const DONE: Record<BulkAction, string> = {
  restart: "Restarted",
  kill: "Killed",
  delete: "Deleted",
  status: "Changed the status of",
  replay: "Replayed",
  resume: "Resumed",
  pause: "Paused",
  labels: "Edited the labels of",
};

/** The actions that ask first: a destructive one (FE-ACTION-1), or one that needs a value. */
const NEEDS_DIALOG: ReadonlySet<BulkAction> = new Set(["kill", "delete", "status", "labels", "resume"]);

interface Failure {
  message: string;
  invalids: Invalid[];
}

/** The 400 `BulkErrorResponse` or any other failure, as the toolbar words it. The backend is all or none. */
function failureOf(e: unknown): Failure {
  const bulk = bulkErrorOf(e);
  if (bulk) return { message: bulk.message, invalids: bulk.invalids };
  return { message: e instanceof Error && e.message ? e.message : "The request failed. Try again.", invalids: [] };
}

/** What the dialog or toolbar collected for the action. */
interface Extras {
  deleteStorage?: boolean;
  state?: ChangeTarget;
  labels?: LabelDto[];
  inputs?: Record<string, unknown>;
}

/**
 * The bulk-action toolbar over the executions list (package 7f). `mode` says what is acted on:
 * the chosen `ids` (the `*_by_ids` routes), or every execution `filter` matches (the `*_by_query`
 * routes). It is all or none: on a 400 nothing was applied, and the refused ids come back.
 */
export function BulkActionBar({
  ws,
  rows,
  count,
  mode,
  filter,
  onDone,
}: {
  ws: string;
  /** The chosen rows (their states decide which actions are on). */
  rows: ExecutionListItemDto[];
  /** What the action reaches: the chosen rows, or the whole match. */
  count: number;
  mode: "ids" | "query";
  /** The list's filter, resolved now; read only in `query` mode. */
  filter: () => QueryFilter;
  /** The action succeeded: clear the selection. */
  onDone: () => void;
}) {
  const queryClient = useQueryClient();
  const reasonsId = useId();
  const [dialog, setDialog] = useState<BulkAction | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [pending, setPending] = useState(false);

  const restartIds = useWorkflowRestartExecutionsByIds();
  const restartQuery = useWorkflowRestartExecutionsByQuery();
  const killIds = useWorkflowKillExecutionsByIds();
  const killQuery = useWorkflowKillExecutionsByQuery();
  const deleteIds = useWorkflowDeleteExecutionsByIds();
  const deleteQuery = useWorkflowDeleteExecutionsByQuery();
  const statusIds = useWorkflowChangeStatusExecutionsByIds();
  const statusQuery = useWorkflowChangeStatusExecutionsByQuery();
  const replayIds = useWorkflowReplayExecutionsByIds();
  const replayQuery = useWorkflowReplayExecutionsByQuery();
  const resumeIds = useWorkflowResumeExecutionsByIds();
  const resumeQuery = useWorkflowResumeExecutionsByQuery();
  const pauseIds = useWorkflowPauseExecutionsByIds();
  const pauseQuery = useWorkflowPauseExecutionsByQuery();
  const labelsIds = useWorkflowLabelsExecutionsByIds();
  const labelsQuery = useWorkflowLabelsExecutionsByQuery();

  // In `query` mode the rows beyond the page are unknown: every action is offered, and the server's
  // all-or-none check names the ids it refuses.
  const eligibility = bulkEligibility(mode === "ids" ? rows : []);

  /** Sends the action: the id list, or the filter. Returns how many executions it reached. */
  async function submit(action: BulkAction, x: Extras): Promise<number> {
    const workspace = ws;
    const ids = rows.map((r) => r.id);
    const params = mode === "query" ? filter() : {};
    const q = mode === "query";
    let res: { status: number; data: unknown };
    switch (action) {
      case "restart":
        res = q ? await restartQuery.mutateAsync({ workspace, params }) : await restartIds.mutateAsync({ workspace, data: { ids } });
        break;
      case "replay":
        res = q ? await replayQuery.mutateAsync({ workspace, params }) : await replayIds.mutateAsync({ workspace, data: { ids } });
        break;
      case "pause":
        res = q ? await pauseQuery.mutateAsync({ workspace, params }) : await pauseIds.mutateAsync({ workspace, data: { ids } });
        break;
      case "kill":
        res = q ? await killQuery.mutateAsync({ workspace, params }) : await killIds.mutateAsync({ workspace, data: { ids } });
        break;
      case "delete": {
        const flags = { deleteStorage: x.deleteStorage ?? true };
        res = q
          ? await deleteQuery.mutateAsync({ workspace, params: { ...params, ...flags } })
          : await deleteIds.mutateAsync({ workspace, data: { ids }, params: flags });
        break;
      }
      case "status": {
        const state = x.state!;
        res = q
          ? await statusQuery.mutateAsync({ workspace, data: { state }, params })
          : await statusIds.mutateAsync({ workspace, data: { ids, state } });
        break;
      }
      case "labels": {
        const labels = x.labels ?? [];
        res = q
          ? await labelsQuery.mutateAsync({ workspace, data: labels, params })
          : await labelsIds.mutateAsync({ workspace, data: { ids, labels } });
        break;
      }
      case "resume": {
        const inputs = x.inputs;
        res = q
          ? await resumeQuery.mutateAsync({ workspace, data: inputs ? { inputs } : {}, params })
          : await resumeIds.mutateAsync({ workspace, data: inputs ? { ids, inputs } : { ids } });
        break;
      }
    }
    if (res.status !== 200) throw new Error("The server did not confirm the change. Refresh and check the executions.");
    return (res.data as BulkResponse).count;
  }

  async function run(action: BulkAction, x: Extras = {}) {
    setPending(true);
    setFailure(null);
    try {
      const n = await submit(action, x);
      void queryClient.invalidateQueries({ queryKey: workflowsKeys.executions(ws) });
      toast.success(`${DONE[action]} ${executions(n)}`);
      setDialog(null);
      onDone();
    } catch (e) {
      // All or none: a refusal means nothing was applied, so the selection stays for the user to adjust.
      setFailure(failureOf(e));
    } finally {
      setPending(false);
    }
  }

  const choose = (action: BulkAction) => {
    setFailure(null);
    if (NEEDS_DIALOG.has(action)) setDialog(action);
    else void run(action);
  };

  const closeDialog = () => {
    if (pending) return;
    setDialog(null);
    setFailure(null);
  };

  return (
    <div className="mb-3 grid gap-2">
      <div role="toolbar" aria-label="Bulk actions" className="flex flex-wrap items-center gap-2 rounded-md border bg-muted/40 p-2">
        <span className="text-sm font-medium tabular-nums">{`${count} selected`}</span>
        {BULK_ACTIONS.map(({ action, label }) => (
          <ReasonButton
            key={action}
            enablement={eligibility[action]}
            reasonId={`${reasonsId}-${action}`}
            variant={action === "delete" || action === "kill" ? "destructive" : "outline"}
            busy={pending}
            onClick={() => choose(action)}
          >
            {label}
          </ReasonButton>
        ))}
      </div>
      <DisabledReasons actions={BULK_ACTIONS.map(({ action, label }) => ({ id: `${reasonsId}-${action}`, label, enablement: eligibility[action] }))} />
      {failure && dialog === null ? <FailureAlert failure={failure} /> : null}
      {dialog ? (
        <BulkDialog action={dialog} count={count} pending={pending} failure={failure} onClose={closeDialog} onConfirm={(x) => void run(dialog, x)} />
      ) : null}
    </div>
  );
}

/** A by-query refusal can name thousands of ids; the list shows this many and scrolls. */
const MAX_LISTED_INVALIDS = 50;

/** Which ids the server refused and why, and that nothing was applied (the routes are all or none). */
function FailureAlert({ failure }: { failure: Failure }) {
  return (
    <Alert variant="destructive">
      <AlertDescription>
        <p>{failure.message}</p>
        {failure.invalids.length > 0 ? (
          <>
            <p className="mt-1">Nothing was changed. Remove these executions from the selection, or fix what blocks them, and try again.</p>
            <ul role="list" className="mt-1 grid max-h-48 gap-0.5 overflow-auto">
              {failure.invalids.slice(0, MAX_LISTED_INVALIDS).map((i) => (
                <li key={i.id} className="flex flex-wrap gap-x-2">
                  <span className="font-mono text-xs">{i.id}</span>
                  <span>{i.reason}</span>
                </li>
              ))}
              {failure.invalids.length > MAX_LISTED_INVALIDS ? (
                <li className="tabular-nums">{`and ${failure.invalids.length - MAX_LISTED_INVALIDS} more`}</li>
              ) : null}
            </ul>
          </>
        ) : null}
      </AlertDescription>
    </Alert>
  );
}

const TITLES: Record<string, (n: number) => string> = {
  delete: (n) => `Delete ${executions(n)}?`,
  kill: (n) => `Kill ${executions(n)}?`,
  status: (n) => `Change the status of ${executions(n)}`,
  labels: (n) => `Edit the labels of ${executions(n)}`,
  resume: (n) => `Resume ${executions(n)}`,
};

const BODIES: Record<string, string> = {
  delete: "Deletes the executions with their task runs, logs, loop iterations and retries. This cannot be undone.",
  kill: "Stops every running task of the executions. This cannot be undone.",
  status: "Sets every chosen execution to the status below.",
  labels: "Replaces the labels of every chosen execution with the list below.",
  resume: "Releases every paused execution. Add inputs only if every flow takes the same ones.",
};

const stateWord = (s: string) => s.charAt(0).toUpperCase() + s.slice(1).toLowerCase();

function BulkDialog({
  action,
  count,
  pending,
  failure,
  onClose,
  onConfirm,
}: {
  action: BulkAction;
  count: number;
  pending: boolean;
  failure: Failure | null;
  onClose: () => void;
  onConfirm: (x: Extras) => void;
}) {
  const id = useId();
  const destructive = action === "delete" || action === "kill";
  const [deleteStorage, setDeleteStorage] = useState(true);
  const [state, setState] = useState<ChangeTarget | "">("");
  const [rows, setRows] = useState<LabelRow[]>([{ key: "", value: "", system: false }]);
  const [inputsText, setInputsText] = useState("");
  const [inputsError, setInputsError] = useState<string | null>(null);

  const errors = action === "labels" ? rowErrors(rows) : new Map();
  const ready = action === "status" ? state !== "" : action === "labels" ? rows.length > 0 && errors.size === 0 : true;

  const confirm = () => {
    if (action === "delete") return onConfirm({ deleteStorage });
    if (action === "status") return onConfirm({ state: state as ChangeTarget });
    if (action === "labels") return onConfirm({ labels: labelsFromRows(rows) });
    if (action === "resume" && inputsText.trim() !== "") {
      try {
        const parsed: unknown = JSON.parse(inputsText);
        if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("not an object");
        setInputsError(null);
        return onConfirm({ inputs: parsed as Record<string, unknown> });
      } catch {
        setInputsError("Enter the inputs as a JSON object, for example {\"name\": \"value\"}");
        return;
      }
    }
    onConfirm({});
  };

  let fields: ReactNode = null;
  if (action === "delete") {
    fields = (
      <div className="flex items-start gap-2">
        <Checkbox id={`${id}-storage`} checked={deleteStorage} onCheckedChange={(v) => setDeleteStorage(v === true)} className="mt-0.5" />
        <Label htmlFor={`${id}-storage`}>Delete stored files</Label>
      </div>
    );
  } else if (action === "status") {
    fields = (
      <div className="grid gap-1.5">
        <Label htmlFor={`${id}-state`}>New status</Label>
        <Select value={state} onValueChange={(v) => setState(v as ChangeTarget)}>
          <SelectTrigger id={`${id}-state`}>
            <SelectValue placeholder="Pick a status" />
          </SelectTrigger>
          <SelectContent>
            {changeTargets("").map((s) => (
              <SelectItem key={s} value={s}>
                {stateWord(s)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    );
  } else if (action === "labels") {
    fields = (
      <div className="grid gap-2">
        {rows.map((row, i) => (
          <div key={i} className="grid grid-cols-[1fr_1fr_auto] items-start gap-2">
            <div className="grid gap-1">
              <Label htmlFor={`${id}-k${i}`} className="sr-only">{`Label ${i + 1} key`}</Label>
              <Input
                id={`${id}-k${i}`}
                placeholder="Key"
                value={row.key}
                onChange={(e) => setRows(rows.map((r, j) => (j === i ? { ...r, key: e.target.value } : r)))}
              />
              {errors.get(i)?.key && row.key !== "" ? <p className="text-xs text-destructive">{errors.get(i)?.key}</p> : null}
            </div>
            <div className="grid gap-1">
              <Label htmlFor={`${id}-v${i}`} className="sr-only">{`Label ${i + 1} value`}</Label>
              <Input
                id={`${id}-v${i}`}
                placeholder="Value"
                value={row.value}
                onChange={(e) => setRows(rows.map((r, j) => (j === i ? { ...r, value: e.target.value } : r)))}
              />
            </div>
            <Button type="button" variant="ghost" size="icon" aria-label={`Remove label ${i + 1}`} onClick={() => setRows(rows.filter((_, j) => j !== i))}>
              <X aria-hidden />
            </Button>
          </div>
        ))}
        <Button type="button" variant="outline" size="sm" className="justify-self-start" onClick={() => setRows([...rows, { key: "", value: "", system: false }])}>
          <Plus aria-hidden />
          Add a label
        </Button>
      </div>
    );
  } else if (action === "resume") {
    fields = (
      <div className="grid gap-1.5">
        <Label htmlFor={`${id}-inputs`}>Inputs (JSON, optional)</Label>
        <Textarea id={`${id}-inputs`} className="font-mono text-xs" value={inputsText} onChange={(e) => setInputsText(e.target.value)} />
        {inputsError ? <p className="text-xs text-destructive">{inputsError}</p> : null}
      </div>
    );
  }

  return (
    <AlertDialog open onOpenChange={(o) => (o ? undefined : onClose())}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{TITLES[action](count)}</AlertDialogTitle>
          <AlertDialogDescription>{BODIES[action]}</AlertDialogDescription>
        </AlertDialogHeader>
        {fields}
        {failure ? <FailureAlert failure={failure} /> : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
          <Button type="button" variant={destructive ? "destructive" : "default"} disabled={pending || !ready} onClick={confirm}>
            {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
            {BULK_ACTIONS.find((a) => a.action === action)!.label}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
