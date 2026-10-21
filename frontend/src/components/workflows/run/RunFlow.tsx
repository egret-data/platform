import { useId, useMemo, useState, type FormEvent } from "react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2, Play } from "lucide-react";
import {
  getWorkflowStartFlowExecutionUrl,
  useWorkflowStartFlowExecution,
  type workflowStartFlowExecutionResponse,
} from "@/api/generated/workflow/workflow";
import { customInstance } from "@/api/mutator";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Enablement } from "@/components/workflows/draftState";
import { InputField } from "@/components/workflows/run/inputControls";
import {
  containerFor,
  formFromInputs,
  inputsFromSource,
  requestFromForm,
  serverErrorInput,
  unsupportedInputs,
  validateForm,
  type FieldValue,
  type RunInput,
} from "@/components/workflows/run/inputModel";
import { DisabledReasons } from "@/components/workflows/ReasonButton";
import { WorkerOffNotice } from "@/components/workflows/WorkerOffNotice";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

export interface RunFlowProps {
  ws: string;
  /** The flow's UUID. */
  flowId: string;
  /** `namespace.flow_id`, for the title. */
  flowName: string;
  /**
   * The saved draft's YAML (`FlowDto.source`): what the server starts, so the
   * inputs come from here. The header offers Run only with no unsaved edits.
   */
  source: string;
  /** `FlowDto.worker_enabled`: false, the run is created and nothing runs it (KI-013). */
  workerEnabled: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Run (W7a spec §6): the saved draft's inputs as a form, in a confirm, a
 * dialog or a right-hand drawer by how many inputs there are and what kind.
 * Kestra's `FlowRun.vue`/`InputsForm.vue` give the structure: one labelled
 * control per input, its description under it, Run at the foot.
 *
 * Every execution runs the saved draft (the user's ruling, 2026-10-10). The
 * parent keys this by that text, so a new save starts a new form; while it
 * stays, closing and reopening keeps what was typed.
 */
export function RunFlow(props: RunFlowProps) {
  const parsed = useMemo(() => inputsFromSource(props.source), [props.source]);
  const inputs = "inputs" in parsed ? parsed.inputs : [];
  return <RunForm {...props} inputs={inputs} sourceError={"error" in parsed ? parsed.error : null} />;
}

function RunForm({
  ws,
  flowId,
  flowName,
  workerEnabled,
  open,
  onOpenChange,
  inputs,
  sourceError,
}: RunFlowProps & { inputs: RunInput[]; sourceError: string | null }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  // Its JSON body goes out as `application/json` by `customInstance` (`api/mutator.ts`):
  // the route takes two body types, so orval writes no `Content-Type` (W7a Task 8).
  const start = useWorkflowStartFlowExecution();
  const idPrefix = useId();
  const [form, setForm] = useState(() => formFromInputs(inputs));
  const [clientErrors, setClientErrors] = useState<Record<string, string>>({});
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [formError, setFormError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const unsupported = unsupportedInputs(inputs);
  const container = sourceError ? "dialog" : containerFor(inputs);
  const runState: Enablement = sourceError
    ? { ok: false, reason: "The flow's inputs could not be read" }
    : unsupported.length > 0
      ? { ok: false, reason: `${unsupported.map((u) => `${u.id} (${u.type})`).join(", ")} cannot be entered in this form` }
      : submitting
        ? { ok: false, reason: "Starting the run…" }
        : { ok: true };

  const setValue = (id: string, value: FieldValue) => {
    setForm((f) => ({ ...f, values: { ...f.values, [id]: value } }));
    // An edit answers that field's error; the others stay until the next Run.
    setClientErrors((errors) => without(errors, id));
    setServerErrors((errors) => without(errors, id));
  };

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!runState.ok) return;
    const errors = validateForm(form);
    setClientErrors(errors);
    setServerErrors({});
    setFormError(null);
    if (Object.keys(errors).length > 0) return;
    setSubmitting(true);
    try {
      const req = requestFromForm(form);
      // The generated function always `JSON.stringify`s its body, so a multipart
      // start goes through the same mutator with the `FormData` as the body, and
      // the browser writes the `multipart/form-data; boundary=…` header.
      const res =
        "json" in req
          ? await start.mutateAsync({ workspace: ws, id: flowId, data: req.json })
          : await customInstance<workflowStartFlowExecutionResponse>(getWorkflowStartFlowExecutionUrl(ws, flowId), {
              method: "POST",
              body: req.multipart,
            });
      if (res.status !== 201) throw new Error(res.data.error);
      // The execution page opens on the answer instead of reading it again; it polls from there.
      queryClient.setQueryData(workflowsKeys.execution(res.data.id), { status: 200, data: res.data, headers: res.headers });
      // The result is the page it opens, so no toast (FE-ACTION-2).
      navigate(`/explore/${ws}/workflows/executions/${encodeURIComponent(res.data.id)}`);
    } catch (err) {
      // Nothing typed is cleared (FE-ACTION-3); an input the server names gets the message beside it (FE-FORM-4).
      const message = err instanceof Error && err.message ? err.message : "the request failed";
      const field = serverErrorInput(message, inputs);
      if (field) setServerErrors({ [field]: message });
      else setFormError(`The run could not be started: ${message}. Fix it, then run again.`);
    } finally {
      setSubmitting(false);
    }
  };

  const reasonId = `${idPrefix}-run-reason`;
  const runButton = (
    <Button
      type={container === "confirm" ? "button" : "submit"}
      size="sm"
      disabled={!runState.ok}
      aria-describedby={runState.ok ? undefined : reasonId}
      onClick={container === "confirm" ? () => void submit() : undefined}
    >
      {submitting ? <Loader2 className="animate-spin" aria-hidden /> : <Play aria-hidden />}
      Run
    </Button>
  );
  const reasons = <DisabledReasons actions={[{ id: reasonId, label: "Run", enablement: runState }]} />;

  const notices = (
    <>
      {workerEnabled ? null : <WorkerOffNotice />}
      {sourceError ? (
        <Alert variant="destructive">
          <AlertDescription>
            The flow's inputs could not be read: {sourceError}. Fix the YAML, save, then run it.
          </AlertDescription>
        </Alert>
      ) : null}
      {unsupported.length > 0 ? (
        <Alert>
          <AlertDescription>
            <p>This form cannot enter these inputs, so the flow cannot be run from here:</p>
            <ul className="mt-1 list-disc pl-5">
              {unsupported.map((u) => (
                <li key={u.id}>
                  <code>{u.id}</code> ({u.type})
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      ) : null}
      {formError ? (
        <Alert variant="destructive">
          <AlertDescription>{formError}</AlertDescription>
        </Alert>
      ) : null}
    </>
  );

  if (container === "confirm") {
    return (
      <AlertDialog open={open} onOpenChange={onOpenChange}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Run {flowName}?</AlertDialogTitle>
            <AlertDialogDescription>
              This starts an execution of the saved draft. The flow has no inputs.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {notices}
          <AlertDialogFooter className="items-center">
            {reasons}
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            {runButton}
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-container={container}
        // The footer below is sticky; a field scrolled into view by focus must clear it
        // (WCAG 2.4.11), so the body's scroll padding is taller than the footer: 45px on
        // a wide screen, 85px where it stacks below `sm` (measured, W7a Task 8 fix round 1).
        bodyClassName="scroll-pb-28 sm:scroll-pb-16"
        className={
          container === "drawer"
            ? // The right-hand drawer (spec §6.1). There is no sheet primitive in `components/ui`,
              // so it is the dialog primitive pinned to the right edge at full height.
              "top-0 right-0 left-auto h-dvh max-h-dvh w-full max-w-lg translate-x-0 translate-y-0 rounded-none border-y-0 border-r-0"
            : undefined
        }
      >
        <form className="grid gap-4" onSubmit={(e) => void submit(e)} noValidate>
          <DialogHeader>
            <DialogTitle>Run {flowName}</DialogTitle>
            <DialogDescription>Starts an execution of the saved draft.</DialogDescription>
          </DialogHeader>
          {notices}
          {/* An unsupported input has no control; the notice above names it with its type. */}
          {inputs
            .filter((input) => !unsupported.some((u) => u.id === input.id))
            .map((input) => (
              <InputField
                key={input.id}
                input={input}
                value={form.values[input.id] ?? null}
                onChange={(v) => setValue(input.id, v)}
                error={serverErrors[input.id] ?? clientErrors[input.id]}
                idPrefix={idPrefix}
              />
            ))}
          {/* Sticky at the foot of the scrolling body, as Kestra's Execute is: a long form
              (the drawer) otherwise hides Run below every field (W7a Task 8). */}
          <DialogFooter className="sticky bottom-0 z-10 items-center border-t border-border bg-background pt-3">
            {reasons}
            <Button type="button" size="sm" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            {runButton}
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function without(errors: Record<string, string>, id: string): Record<string, string> {
  if (!(id in errors)) return errors;
  const next = { ...errors };
  delete next[id];
  return next;
}
