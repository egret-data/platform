import { useId, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";
import { ApiError } from "@/api/client";
import { customInstance } from "@/api/mutator";
import {
  getWorkflowResumeExecutionUrl,
  useWorkflowResumeExecution,
  useWorkflowValidateResumeInputs,
  workflowValidateResumeInputs,
  type workflowResumeExecutionResponse,
} from "@/api/generated/workflow/workflow";
import type { ExecutionDto } from "@/api/generated/model";
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
import { Skeleton } from "@/components/ui/skeleton";
import { resumeFeedback, resumeRefusal } from "@/components/workflows/executions/actions";
import { useRefreshExecution } from "@/components/workflows/executions/useExecutionAction";
import { InputField } from "@/components/workflows/run/inputControls";
import {
  formFromInputs,
  requestFromForm,
  runInputFrom,
  serverErrorsByInput,
  unsupportedInputs,
  validateForm,
  validationRequestFromForm,
  validationView,
  type FieldValue,
  type FormState,
  type RunInput,
} from "@/components/workflows/run/inputModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

export const RESUME_BY_HAND = "The execution continues: the tasks that were waiting start now.";
export const RESUME_NO_INPUTS =
  "Every paused task ends now as a success, whatever its duration or timeout, and the execution continues with the tasks after it.";
export const RESUME_WITH_INPUTS = "Fill in what the paused task asks for. It then ends as a success and the execution continues.";

/**
 * Resume (package 7c, D11): asks `resume/validate` with an empty body when it opens, which answers
 * the waiting Pause's `onResume` inputs with their defaults. With none it is today's confirm; with
 * some it shows the Run-flow inputs form, checks the values on the server first, then resumes with
 * JSON (multipart when a file is chosen). A refusal stays in the dialog (FE-ACTION-3).
 */
export function ResumeDialog(props: { ws: string; execution: ExecutionDto; open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <AlertDialog open={props.open} onOpenChange={props.onOpenChange}>
      <AlertDialogContent className="max-h-[90dvh] overflow-y-auto">{props.open ? <ResumeForm {...props} /> : null}</AlertDialogContent>
    </AlertDialog>
  );
}

function ResumeForm({ ws, execution, onOpenChange }: { ws: string; execution: ExecutionDto; onOpenChange: (open: boolean) => void }) {
  const idPrefix = useId();
  const refresh = useRefreshExecution(ws, execution.id);
  const resume = useWorkflowResumeExecution();
  const validate = useWorkflowValidateResumeInputs();
  const asked = useQuery({
    queryKey: workflowsKeys.resumeInputs(execution.id),
    // FE-DATA-2 exception: a validation POST read once as a query when the dialog opens, so the generated fetcher, not its mutation hook.
    queryFn: () => workflowValidateResumeInputs(ws, execution.id, {}),
    staleTime: 0,
    gcTime: 0,
    retry: false,
    refetchOnWindowFocus: false,
  });
  const inputs: RunInput[] = useMemo(
    () =>
      asked.data?.status === 200
        ? asked.data.data.inputs.map((o) => runInputFrom(o.input)).filter((i): i is RunInput => i !== null)
        : [],
    [asked.data],
  );
  const [values, setValues] = useState<Record<string, FieldValue>>({});
  const form: FormState = useMemo(() => {
    const base = formFromInputs(inputs);
    return { ...base, values: { ...base.values, ...values } };
  }, [inputs, values]);
  const [clientErrors, setClientErrors] = useState<Record<string, string>>({});
  const [serverErrors, setServerErrors] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const loading = asked.isPending;
  // Only when nothing was read yet: a later failed refetch keeps the form the user is filling in.
  const loadError =
    asked.data !== undefined
      ? null
      : asked.error instanceof ApiError
      ? (resumeRefusal(asked.error.code) ?? asked.error.message)
      : asked.isError
        ? "The inputs could not be loaded. Close the dialog and try again."
        : null;
  const unsupported = unsupportedInputs(inputs);
  const description = execution.paused_by_hand ? RESUME_BY_HAND : inputs.length > 0 ? RESUME_WITH_INPUTS : RESUME_NO_INPUTS;

  const setValue = (id: string, v: FieldValue) => {
    setValues((s) => ({ ...s, [id]: v }));
    setClientErrors((e) => without(e, id));
    setServerErrors((e) => without(e, id));
  };

  const confirm = async () => {
    setPending(true);
    setError(null);
    setServerErrors({});
    try {
      let res: workflowResumeExecutionResponse;
      if (inputs.length === 0) {
        res = await resume.mutateAsync({ workspace: ws, id: execution.id, data: {} });
      } else {
        const errors = validateForm(form);
        setClientErrors(errors);
        if (Object.keys(errors).length > 0) return;
        // The server's own check first (D11): its per-input errors sit beside their fields.
        const checked = await validate.mutateAsync({ workspace: ws, id: execution.id, data: validationRequestFromForm(form) });
        if (checked.status === 200) {
          const view = validationView(checked.data, inputs);
          if (Object.keys(view.errors).length > 0) {
            setServerErrors(view.errors);
            return;
          }
        }
        const req = requestFromForm(form);
        res =
          "json" in req
            ? await resume.mutateAsync({ workspace: ws, id: execution.id, data: req.json })
            : // The generated function JSON.stringifies its body: a multipart resume goes through the mutator with the FormData (as RunFlow does).
              await customInstance<workflowResumeExecutionResponse>(getWorkflowResumeExecutionUrl(ws, execution.id), {
                method: "POST",
                body: req.multipart,
              });
      }
      if (res.status !== 200) throw new Error("The execution could not be resumed. Try again.");
      refresh();
      // The page stays, so a toast confirms it (FE-ACTION-2), in terms of the answered state.
      const feedback = resumeFeedback(res.data.state);
      if (feedback.tone === "success") toast.success(feedback.message);
      else toast.warning(feedback.message);
      onOpenChange(false);
    } catch (e) {
      const sentence = e instanceof ApiError ? resumeRefusal(e.code) : null;
      if (sentence) {
        setError(sentence);
        refresh();
      } else {
        const message = e instanceof Error && e.message ? e.message : "The execution could not be resumed. Try again.";
        const { fields, rest } = serverErrorsByInput(message, inputs);
        if (Object.keys(fields).length > 0) {
          setServerErrors(fields);
          if (rest.length > 0) setError(rest.join(" "));
        } else setError(message);
      }
    } finally {
      setPending(false);
    }
  };

  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle>Resume this execution?</AlertDialogTitle>
        <AlertDialogDescription>{description}</AlertDialogDescription>
      </AlertDialogHeader>
      {loading ? (
        <div aria-busy="true" className="grid gap-2">
          <Skeleton className="h-4 w-48" />
          <span className="sr-only">Reading the inputs</span>
        </div>
      ) : loadError ? (
        <Alert variant="destructive">
          <AlertDescription>{loadError}</AlertDescription>
        </Alert>
      ) : inputs.length > 0 ? (
        <div className="grid gap-3">
          {unsupported.length > 0 ? (
            <p id={`${idPrefix}-unsupported`} className="text-sm text-muted-foreground">
              {`${unsupported.map((u) => `${u.id} (${u.type})`).join(", ")} cannot be entered in this form`}
            </p>
          ) : null}
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
        </div>
      ) : null}
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <AlertDialogFooter>
        <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
        <Button
          type="button"
          disabled={pending || loading || loadError !== null || unsupported.length > 0}
          aria-describedby={unsupported.length > 0 ? `${idPrefix}-unsupported` : undefined}
          onClick={() => void confirm()}
        >
          {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
          Resume
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
