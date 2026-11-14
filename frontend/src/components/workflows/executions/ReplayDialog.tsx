import { useId, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { ApiError } from "@/api/client";
import { customInstance } from "@/api/mutator";
import {
  getWorkflowReplayExecutionUrl,
  useWorkflowGetFlow,
  useWorkflowGetFlowVersion,
  useWorkflowListFlowVersions,
  useWorkflowReplayExecution,
  useWorkflowRestartExecution,
  type workflowReplayExecutionResponse,
} from "@/api/generated/workflow/workflow";
import type { ExecutionDto, FlowVersionDto, TaskRunDto } from "@/api/generated/model";
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
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Skeleton } from "@/components/ui/skeleton";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { replayRefusal, restartRefusal, sourceParam, type SourceChoice } from "@/components/workflows/executions/actions";
import { useRefreshExecution } from "@/components/workflows/executions/useExecutionAction";
import { InputField } from "@/components/workflows/run/inputControls";
import {
  formFromInputs,
  inputsFromSource,
  requestFromForm,
  serverErrorsByInput,
  unsupportedInputs,
  validateForm,
  type FieldValue,
  type FormState,
  type RunInput,
} from "@/components/workflows/run/inputModel";
import { fullTimestamp } from "@/lib/relativeTime";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

type Kind = SourceChoice["kind"];

const RESTART_IN_PLACE = "The failed tasks run again in this same execution, and the tasks that succeeded keep their results. The execution keeps its id.";
const RESTART_NEW =
  "Restarting on another version creates a new execution: the failed tasks run again on that version and the tasks that succeeded keep their results.";
const NO_VERSIONS = "This flow has no published versions";
/** The inputs' source text has not arrived yet: Change inputs waits, with a skeleton (FE-STATE-2). */
const LOADING = Symbol("loading");
const FLOW_DELETED = "The flow was deleted permanently, so only this run's version is available";

/**
 * Replay and Restart, with the choice of which flow source runs (package 7b, D14): this run's own
 * (the default), the flow's current draft, or a published version. Replay can start from one task
 * run and can change the inputs. The form lives in the dialog's content, so it starts afresh each
 * time the dialog opens, and a failure stays in the dialog with its sentence (FE-ACTION-3).
 */
export function ReplayDialog(props: {
  ws: string;
  execution: ExecutionDto;
  mode: "replay" | "restart";
  /** Replay from this task run, when set. */
  taskRun?: TaskRunDto | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <AlertDialog open={props.open} onOpenChange={props.onOpenChange}>
      <AlertDialogContent className="max-h-[90dvh] overflow-y-auto">
        <ReplayForm {...props} />
      </AlertDialogContent>
    </AlertDialog>
  );
}

function describe(mode: "replay" | "restart", taskRun: TaskRunDto | null | undefined, kind: Kind): string {
  if (mode === "restart") return kind === "frozen" ? RESTART_IN_PLACE : RESTART_NEW;
  if (taskRun) {
    return `Replay starts a new execution that keeps the results of the tasks before ${taskRun.task_id} and runs ${taskRun.task_id} and every task after it again.`;
  }
  return kind === "frozen"
    ? "Replay starts a new execution from this execution's frozen YAML and inputs. Edits made to the flow since it ran are not used."
    : "Replay starts a new execution from the version you pick, with this execution's inputs unless you change them.";
}

function ReplayForm({
  ws,
  execution,
  mode,
  taskRun,
  onOpenChange,
}: {
  ws: string;
  execution: ExecutionDto;
  mode: "replay" | "restart";
  taskRun?: TaskRunDto | null;
  onOpenChange: (open: boolean) => void;
}) {
  const flowId = execution.flow_id ?? "";
  const hasFlow = execution.flow_id != null;
  const idPrefix = useId();
  const [kind, setKind] = useState<Kind>("frozen");
  const [version, setVersion] = useState<number | null>(null);
  // The inputs the user changed. They live here, not in the body, so switching the source keeps
  // the edits of every input the new source also declares.
  const [edits, setEdits] = useState<Record<string, FieldValue>>({});
  const choice: SourceChoice | null = kind === "version" ? (version === null ? null : { kind, version }) : { kind };

  const versionsQuery = useWorkflowListFlowVersions(ws, flowId, {
    query: { queryKey: workflowsKeys.versions(flowId), enabled: hasFlow },
  });
  const versions: FlowVersionDto[] | undefined = versionsQuery.data?.status === 200 ? versionsQuery.data.data : undefined;
  const sorted = useMemo(() => [...(versions ?? [])].sort((a, b) => b.version - a.version), [versions]);
  const noVersions = versions !== undefined && versions.length === 0;
  const versionsLoading = hasFlow && versions === undefined && !versionsQuery.isError;
  const versionsReason = !hasFlow
    ? FLOW_DELETED
    : noVersions
      ? NO_VERSIONS
      : versionsQuery.isError
        ? "The published versions could not be loaded. Close the dialog and try again."
        : null;
  const radiosReasonId = `${idPrefix}-versions-reason`;

  // The inputs section needs the chosen source's text; only replay reads it.
  const wantsText = mode === "replay";
  const flowQuery = useWorkflowGetFlow(ws, flowId, {
    query: { queryKey: workflowsKeys.flow(flowId), enabled: hasFlow && wantsText && kind === "current" },
  });
  const versionQuery = useWorkflowGetFlowVersion(ws, flowId, version ?? 0, {
    query: { queryKey: workflowsKeys.version(flowId, version ?? 0), enabled: hasFlow && wantsText && kind === "version" && version !== null },
  });
  const sourceText = ((): { text: string } | { loading: true } | { error: string } | { none: true } => {
    if (kind === "frozen") return { text: execution.source };
    if (kind === "current") {
      if (flowQuery.isError) return { error: "the flow's current source could not be loaded. Close the dialog and try again." };
      return flowQuery.data?.status === 200 ? { text: flowQuery.data.data.source } : { loading: true };
    }
    if (version === null) return { none: true };
    if (versionQuery.isError) return { error: `version ${version} could not be loaded. Pick another one or try again.` };
    const snapshotSource = versionQuery.data?.status === 200 ? versionQuery.data.data.snapshot.source : undefined;
    return typeof snapshotSource === "string" ? { text: snapshotSource } : { loading: true };
  })();
  const bodyKey = `${kind}:${version ?? ""}:${"text" in sourceText ? "ready" : "wait"}`;

  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle>
          {mode === "restart" ? "Restart this execution?" : taskRun ? `Replay from ${taskRun.task_id}?` : "Replay this execution?"}
        </AlertDialogTitle>
        <AlertDialogDescription>{describe(mode, taskRun, kind)}</AlertDialogDescription>
      </AlertDialogHeader>
      <fieldset className="grid gap-2">
        <legend className="mb-1 text-sm font-medium">Run on</legend>
        <RadioGroup
          value={kind}
          onValueChange={(v) => setKind(v as Kind)}
          aria-label="Run on"
          className="gap-2"
        >
          <div className="flex items-center gap-2">
            <RadioGroupItem value="frozen" id={`${idPrefix}-frozen`} />
            <Label htmlFor={`${idPrefix}-frozen`} className="font-normal">
              This run's version
            </Label>
          </div>
          <div className="flex items-center gap-2">
            <RadioGroupItem
              value="current"
              id={`${idPrefix}-current`}
              disabled={!hasFlow}
              aria-describedby={!hasFlow ? radiosReasonId : undefined}
            />
            <Label htmlFor={`${idPrefix}-current`} className="font-normal">
              The flow's current version
            </Label>
          </div>
          <div className="flex items-center gap-2">
            <RadioGroupItem
              value="version"
              id={`${idPrefix}-version`}
              disabled={versionsLoading || versionsReason !== null}
              aria-describedby={versionsLoading ? `${idPrefix}-versions-loading` : versionsReason ? radiosReasonId : undefined}
            />
            <Label htmlFor={`${idPrefix}-version`} className="font-normal">
              A published version
            </Label>
          </div>
        </RadioGroup>
        {versionsLoading ? (
          <div className="pl-6" aria-busy="true">
            <Skeleton className="h-4 w-40" />
            <span id={`${idPrefix}-versions-loading`} className="sr-only">
              Loading the published versions
            </span>
          </div>
        ) : null}
        {!hasFlow ? (
          <p id={radiosReasonId} className="pl-6 text-sm text-muted-foreground">
            {versionsReason}
          </p>
        ) : kind === "version" || versionsReason ? (
          <div className="grid gap-1 pl-6">
            <Label htmlFor={`${idPrefix}-version-select`} className="sr-only">
              Published version
            </Label>
            <Select
              value={version === null ? "" : String(version)}
              onValueChange={(v) => setVersion(Number(v))}
              disabled={versionsReason !== null}
            >
              <SelectTrigger id={`${idPrefix}-version-select`} aria-describedby={versionsReason ? radiosReasonId : undefined}>
                <SelectValue placeholder="Pick a version" />
              </SelectTrigger>
              <SelectContent>
                {sorted.map((v) => (
                  <SelectItem key={v.version} value={String(v.version)}>
                    {`Version ${v.version}, ${fullTimestamp(v.created_at)}${v.note ? `, ${v.note}` : ""}`}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {versionsReason ? (
              <p id={radiosReasonId} className="text-sm text-muted-foreground">
                {versionsReason}
              </p>
            ) : null}
          </div>
        ) : null}
      </fieldset>
      <ReplayBody
        key={bodyKey}
        ws={ws}
        execution={execution}
        mode={mode}
        taskRun={taskRun}
        choice={choice}
        sourceText={sourceText}
        edits={edits}
        onEdit={(id, value) => setEdits((e) => ({ ...e, [id]: value }))}
        onOpenChange={onOpenChange}
      />
    </>
  );
}

type SourceText = { text: string } | { loading: true } | { error: string } | { none: true };

/** A `kestra://` file value of a past run, which the server reads again from the replay chain. */
const isStoredFile = (v: unknown): v is string => typeof v === "string" && v.startsWith("kestra://");

function ReplayBody({
  ws,
  execution,
  mode,
  taskRun,
  choice,
  sourceText,
  edits,
  onEdit,
  onOpenChange,
}: {
  ws: string;
  execution: ExecutionDto;
  mode: "replay" | "restart";
  taskRun?: TaskRunDto | null;
  choice: SourceChoice | null;
  sourceText: SourceText;
  edits: Record<string, FieldValue>;
  onEdit: (id: string, value: FieldValue) => void;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const refresh = useRefreshExecution(ws, execution.id);
  const replay = useWorkflowReplayExecution();
  const restart = useWorkflowRestartExecution();
  const idPrefix = useId();
  const old = useMemo(() => execution.inputs ?? {}, [execution.inputs]);

  const text = "text" in sourceText ? sourceText.text : null;
  const parsed = useMemo(() => (text === null ? null : inputsFromSource(text)), [text]);
  const inputs: RunInput[] = useMemo(() => {
    if (!parsed || "error" in parsed) return [];
    return parsed.inputs.map((i) => {
      if (i.type === "SECRET") {
        return { ...i, initial: undefined, description: [i.description, "Enter it again: the stored value is hidden."].filter(Boolean).join(" ") };
      }
      return i.id in old && i.type !== "FILE" ? { ...i, initial: old[i.id] } : i;
    });
  }, [parsed, old]);
  // What each input starts with, with the user's edits over it (kept for the inputs this source declares too).
  const form: FormState = useMemo(() => {
    const base = formFromInputs(inputs);
    const kept = Object.fromEntries(Object.entries(edits).filter(([id]) => inputs.some((i) => i.id === id)));
    return { ...base, values: { ...base.values, ...kept } };
  }, [inputs, edits]);
  const dirty = inputs.some((i) => i.id in edits);
  const [open, setOpen] = useState(dirty);
  const [clientErrors, setClientErrors] = useState<Record<string, string>>({});
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const unsupported = unsupportedInputs(inputs);
  const inputsReason: string | typeof LOADING | null =
    mode !== "replay"
      ? null
      : "loading" in sourceText
        ? LOADING
        : "error" in sourceText
          ? `The inputs cannot be changed: ${sourceText.error}`
          : "none" in sourceText
            ? "Pick a version to change its inputs"
            : parsed && "error" in parsed
              ? `The inputs cannot be read: ${parsed.error}`
              : unsupported.length > 0
                ? `${unsupported.map((u) => `${u.id} (${u.type})`).join(", ")} cannot be entered in this form`
                : null;

  const setValue = (id: string, value: FieldValue) => {
    onEdit(id, value);
    setClientErrors((e) => without(e, id));
    setServerErrors((e) => without(e, id));
  };

  /** The request body of a replay with changed inputs: a FILE left without a new file keeps its old `kestra://` value. */
  const changedBody = () => {
    const kept = inputs.filter((i) => i.type === "FILE" && isStoredFile(old[i.id]) && !(form.values[i.id] instanceof File));
    const lenient: FormState = {
      ...form,
      inputs: form.inputs.map((i) => (kept.some((k) => k.id === i.id) ? { ...i, required: false } : i)),
    };
    const errors = validateForm(lenient);
    setClientErrors(errors);
    if (Object.keys(errors).length > 0) {
      // The errors sit beside their fields, so the section must be open to show them (FE-FORM-4).
      setOpen(true);
      return null;
    }
    const req = requestFromForm(form);
    if ("json" in req) {
      for (const k of kept) req.json.inputs[k.id] = old[k.id];
    } else {
      for (const k of kept) req.multipart.append(k.id, String(old[k.id]));
    }
    return req;
  };

  const goTo = (id: string) => {
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.executions(ws) });
    // The new execution is the result: go there, no toast (FE-ACTION-2).
    navigate(`/explore/${ws}/workflows/executions/${encodeURIComponent(id)}/overview`);
  };

  const runReplay = async (source: string) => {
    const params = { ...(taskRun ? { taskRunId: taskRun.id } : {}), source };
    let res: workflowReplayExecutionResponse;
    const req = dirty && inputsReason === null ? changedBody() : { json: {} as { inputs?: Record<string, unknown> } };
    if (req === null) return false;
    if ("multipart" in req) {
      // The generated function always JSON.stringifies its body, so a multipart replay goes through the same mutator with the FormData (as RunFlow does).
      res = await customInstance<workflowReplayExecutionResponse>(getWorkflowReplayExecutionUrl(ws, execution.id, params), {
        method: "POST",
        body: req.multipart,
      });
    } else {
      res = await replay.mutateAsync({ workspace: ws, id: execution.id, params, data: "inputs" in req.json ? { inputs: req.json.inputs } : {} });
    }
    if (res.status !== 201) throw new Error("The execution could not be replayed. Try again.");
    goTo(res.data.id);
    return true;
  };

  const runRestart = async (source: string) => {
    const res = await restart.mutateAsync({ workspace: ws, id: execution.id, params: { source } });
    if (res.status === 201) goTo(res.data.id);
    else if (res.status === 200) {
      // Restarted in place: the page stays, so a toast confirms it (FE-ACTION-2).
      refresh();
      toast.success("Restarted the execution");
    } else throw new Error("The execution could not be restarted. Try again.");
    return true;
  };

  const confirm = async () => {
    if (choice === null) return;
    setPending(true);
    setError(null);
    setServerErrors({});
    try {
      const done = mode === "replay" ? await runReplay(sourceParam(choice)) : await runRestart(sourceParam(choice));
      if (done) onOpenChange(false);
    } catch (e) {
      if (mode === "restart" && e instanceof ApiError && restartRefusal(e.code)) refresh();
      const sentence = e instanceof ApiError ? (mode === "replay" ? replayRefusal(e.code) : restartRefusal(e.code)) : null;
      const message = e instanceof Error && e.message ? e.message : "The request failed. Try again.";
      if (sentence) {
        setError(e instanceof ApiError && e.code === "replay_task_not_in_source" && message ? `${sentence} ${message}` : sentence);
      } else {
        const { fields, rest } = serverErrorsByInput(message, inputs);
        if (mode === "replay" && dirty && Object.keys(fields).length > 0) {
          setServerErrors(fields);
          setOpen(true);
          if (rest.length > 0) setError(rest.join(" "));
        } else setError(message);
      }
    } finally {
      setPending(false);
    }
  };

  const label = mode === "restart" ? "Restart" : "Replay";
  return (
    <>
      {mode === "replay" ? (
        <div className="grid gap-2">
          {!dirty && !open ? (
            <p className="text-sm text-muted-foreground">
              {Object.keys(old).length === 0 ? "It has no inputs." : `Inputs: ${Object.keys(old).join(", ")}.`}
            </p>
          ) : null}
          <Collapsible open={open && inputsReason === null} onOpenChange={setOpen}>
            <CollapsibleTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-7 px-1.5"
                disabled={inputsReason !== null}
                aria-describedby={typeof inputsReason === "string" ? `${idPrefix}-inputs-reason` : undefined}
              >
                {open && inputsReason === null ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}
                Change inputs
              </Button>
            </CollapsibleTrigger>
            {inputsReason === LOADING ? (
              <div aria-busy="true" className="mt-1">
                <Skeleton className="h-4 w-48" />
                <span className="sr-only">Reading the inputs</span>
              </div>
            ) : inputsReason ? (
              <p id={`${idPrefix}-inputs-reason`} className="mt-1 text-sm text-muted-foreground">
                {inputsReason}
              </p>
            ) : null}
            <CollapsibleContent className="mt-2 grid gap-3">
              {inputs.length === 0 ? <p className="text-sm text-muted-foreground">This version has no inputs.</p> : null}
              {inputs.map((input) => (
                <InputField
                  key={input.id}
                  input={input}
                  value={form.values[input.id] ?? null}
                  onChange={(v) => setValue(input.id, v)}
                  error={serverErrors[input.id] ?? clientErrors[input.id]}
                  idPrefix={idPrefix}
                />
              ))}
            </CollapsibleContent>
          </Collapsible>
        </div>
      ) : null}
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <AlertDialogFooter>
        <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
        <Button type="button" disabled={pending || choice === null} onClick={() => void confirm()}>
          {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
          {label}
        </Button>
      </AlertDialogFooter>
    </>
  );
}

function without(errors: Record<string, string>, id: string): Record<string, string> {
  if (!(id in errors)) return errors;
  const next = { ...errors };
  delete next[id];
  return next;
}
