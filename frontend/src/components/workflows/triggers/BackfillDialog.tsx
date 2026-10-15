import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { useWorkflowCreateTriggerBackfill, useWorkflowGetFlow } from "@/api/generated/workflow/workflow";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { Enablement } from "@/components/workflows/draftState";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import { InputField } from "@/components/workflows/run/inputControls";
import {
  formFromInputs,
  inputsFromSource,
  unsupportedInputs,
  validateForm,
  type FieldValue,
  type FormState,
  type RunInput,
} from "@/components/workflows/run/inputModel";
import { LabelsEditor } from "@/components/workflows/triggers/LabelsEditor";
import {
  bodyFromForm,
  hasEdits,
  localValue,
  instantFromLocal,
  offeredInputs,
  placeError,
  requiredFile,
  scheduleInputs,
  submitReasons,
  type BackfillForm,
  type ErrorPlace,
  type LabelRow,
} from "@/components/workflows/triggers/backfillForm";
import { adoptEntries, switchError } from "@/components/workflows/triggers/useTriggerSwitch";
import { type TriggerRow } from "@/components/workflows/triggers/triggersModel";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** A schedule's value from the flow's YAML, as the form's control takes it; null when it does not fit the input's type. */
function fromSchedule(input: RunInput, v: unknown): FieldValue | null {
  if (input.type === "BOOLEAN" && typeof v === "boolean") return v;
  if (Array.isArray(v)) return v.map(String);
  if (typeof v === "string" || typeof v === "number") return String(v);
  return null;
}

/** The form's start: the flow's inputs, the schedule's plain values over them. */
function initialForm(inputs: RunInput[], source: string, triggerId: string): FormState {
  const base = formFromInputs(inputs);
  const plain = scheduleInputs(source, triggerId);
  for (const input of inputs) {
    if (!(input.id in plain)) continue;
    const v = fromSchedule(input, plain[input.id]);
    if (v !== null) base.values[input.id] = v;
  }
  return base;
}

/** An instant in the trigger's zone, as `Intl` writes it; the raw text when the browser does not know the zone. */
function inZone(d: Date, timeZone: string): string {
  try {
    return new Intl.DateTimeFormat(undefined, { timeZone, dateStyle: "medium", timeStyle: "medium" }).format(d);
  } catch {
    return d.toISOString();
  }
}

/**
 * The create form of a schedule's backfill (W7f spec §4.3): a window of two
 * local date-times (the end may be empty: now), the schedule's inputs, and
 * labels. The submit is a `ReasonButton`: it says what is missing. A server
 * refusal sits beside the field it names; a conflict closes the form and says
 * so, and nothing typed is cleared on a refusal (FE-ACTION-3).
 */
export default function BackfillDialog({
  ws,
  flowUuid,
  trigger,
  source,
  onClose,
}: {
  ws: string;
  flowUuid: string;
  trigger: TriggerRow;
  /** The flow's saved YAML: its inputs, and the schedule's own values. */
  source: string;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const create = useWorkflowCreateTriggerBackfill();
  const parsed = inputsFromSource(source);
  const declared = "inputs" in parsed ? parsed.inputs : [];
  const unsupported = new Set(unsupportedInputs(declared).map((u) => u.id));
  const inputs = offeredInputs(declared).filter((i) => !unsupported.has(i.id));
  const tz = trigger.schedule?.timezone ?? "UTC";
  const triggerId = trigger.trigger_id;

  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const [labels, setLabels] = useState<LabelRow[]>([]);
  const [form, setForm] = useState<FormState>(() => initialForm(inputs, source, triggerId));
  const [refusal, setRefusal] = useState<ErrorPlace | null>(null);
  const [pending, setPending] = useState(false);

  const now = new Date();
  const bf: BackfillForm = { start, end, labels, inputs: form };
  const dirty = hasEdits(bf);
  useUnsavedChangesWarning(dirty);

  const file = requiredFile(declared);
  const reasons = [
    ...("error" in parsed ? [`The flow's inputs could not be read: ${parsed.error}`] : []),
    ...(file ? ["A FILE input cannot be backfilled"] : []),
    ...submitReasons(bf, now),
  ];
  const submit: Enablement = reasons.length === 0 ? { ok: true } : { ok: false, reason: reasons[0] };
  const clientErrors = validateForm(form);
  const startInstant = instantFromLocal(start);
  const endInstant = instantFromLocal(end);

  const onSubmit = async () => {
    setRefusal(null);
    setPending(true);
    try {
      const res = await create.mutateAsync({ workspace: ws, id: flowUuid, triggerId, data: bodyFromForm(bf) });
      if (res.status !== 201) throw new Error("unexpected answer");
      adoptEntries(queryClient, ws, [{ flow_uuid: flowUuid, entry: res.data }]);
      toast.success(`Started a backfill of ${triggerId}: ${res.data.backfill?.total_ticks ?? 0} ticks.`);
      onClose();
    } catch (e) {
      if (e instanceof ApiError && e.status === 409 && e.code === "backfill_exists") {
        void queryClient.invalidateQueries({ queryKey: workflowsKeys.flowTriggers(flowUuid) });
        void queryClient.invalidateQueries({ queryKey: workflowsKeys.triggers(ws) });
        toast.error(switchError(e));
        onClose();
        return;
      }
      setRefusal(e instanceof ApiError ? placeError(e.code, switchError(e), inputs) : { field: "form", message: switchError(e) });
    } finally {
      setPending(false);
    }
  };

  const maxLocal = localValue(now);
  const endLabel = endInstant ? inZone(endInstant, tz) : "now";

  return (
    <Dialog open onOpenChange={(o) => !o && !pending && onClose()}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Backfill {triggerId}</DialogTitle>
          <DialogDescription>
            Runs this schedule for each of its ticks from start to end, both included, one about every second. Regular runs wait until it ends.
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (submit.ok && !pending) void onSubmit();
          }}
        >
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="grid gap-1">
              <Label htmlFor={`${triggerId}-backfill-start`}>Start</Label>
              <Input
                id={`${triggerId}-backfill-start`}
                type="datetime-local"
                step={1}
                max={maxLocal}
                value={start}
                aria-invalid={refusal?.field === "start" ? true : undefined}
                aria-describedby={refusal?.field === "start" ? `${triggerId}-backfill-start-error` : undefined}
                onChange={(e) => setStart(e.target.value)}
              />
              {refusal?.field === "start" ? (
                <p id={`${triggerId}-backfill-start-error`} className="text-sm text-destructive">
                  {refusal.message}
                </p>
              ) : null}
            </div>
            <div className="grid gap-1">
              <Label htmlFor={`${triggerId}-backfill-end`}>End</Label>
              <Input
                id={`${triggerId}-backfill-end`}
                type="datetime-local"
                step={1}
                max={maxLocal}
                value={end}
                placeholder="Leave empty to end now"
                aria-invalid={refusal?.field === "end" ? true : undefined}
                aria-describedby={refusal?.field === "end" ? `${triggerId}-backfill-end-error` : undefined}
                onChange={(e) => setEnd(e.target.value)}
              />
              {refusal?.field === "end" ? (
                <p id={`${triggerId}-backfill-end-error`} className="text-sm text-destructive">
                  {refusal.message}
                </p>
              ) : null}
            </div>
          </div>
          <p className="text-sm text-muted-foreground tabular-nums">
            {startInstant ? `In ${tz}: ${inZone(startInstant, tz)} → ${endLabel}` : `In ${tz}: pick a start`}
          </p>

          {inputs.length > 0 ? (
            <fieldset className="grid gap-3">
              <legend className="text-sm font-medium">Inputs</legend>
              {inputs.map((input) => (
                <InputField
                  key={input.id}
                  idPrefix={`${triggerId}-backfill`}
                  input={input}
                  value={form.values[input.id] ?? null}
                  onChange={(v) => setForm({ ...form, values: { ...form.values, [input.id]: v } })}
                  error={
                    clientErrors[input.id] ?? (refusal?.field === "input" && refusal.input === input.id ? refusal.message : undefined)
                  }
                />
              ))}
            </fieldset>
          ) : null}

          <LabelsEditor
            rows={labels}
            onChange={setLabels}
            idPrefix={`${triggerId}-backfill`}
            serverError={refusal?.field === "labels" ? refusal.message : undefined}
          />

          {refusal?.field === "form" ? (
            <Alert variant="destructive">
              <AlertDescription>{refusal.message}</AlertDescription>
            </Alert>
          ) : null}

          <DialogFooter className="flex-col gap-2 sm:flex-row sm:justify-between">
            <DisabledReasons actions={[{ id: `${triggerId}-backfill-submit`, label: "Start backfill", enablement: submit }]} />
            <div className="flex gap-2">
              <Button type="button" variant="outline" disabled={pending} onClick={onClose}>
                Cancel
              </Button>
              <ReasonButton
                enablement={submit}
                reasonId={`${triggerId}-backfill-submit`}
                busy={pending}
                onClick={() => void onSubmit()}
              >
                Start backfill
              </ReasonButton>
            </div>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * The form on the global page, which has no flow source in hand: reads the
 * flow when it opens, then shows the form. The flow's tab passes its source
 * straight to `BackfillDialog`.
 */
export function FlowBackfillDialog({
  ws,
  flowUuid,
  trigger,
  onClose,
}: {
  ws: string;
  flowUuid: string;
  trigger: TriggerRow;
  onClose: () => void;
}) {
  const flow = useWorkflowGetFlow(ws, flowUuid, { query: { queryKey: workflowsKeys.flow(flowUuid) } });
  if (flow.data?.status === 200) {
    return <BackfillDialog ws={ws} flowUuid={flowUuid} trigger={trigger} source={flow.data.data.source} onClose={onClose} />;
  }
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Backfill {trigger.trigger_id}</DialogTitle>
          <DialogDescription>
            {flow.isError ? "The flow could not be read, so its inputs are not known. Close this and try again." : "Reading the flow…"}
          </DialogDescription>
        </DialogHeader>
      </DialogContent>
    </Dialog>
  );
}
