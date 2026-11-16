import { useId, useRef, useState, type MutableRefObject } from "react";
import { toast } from "sonner";
import { Loader2, Plus, Trash2 } from "lucide-react";
import { useWorkflowSetExecutionLabels } from "@/api/generated/workflow/workflow";
import type { ExecutionDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { labelsRefusal } from "@/components/workflows/executions/actions";
import {
  labelsFromRows,
  rowErrors,
  rowsChanged,
  rowsFromLabels,
  type LabelRow,
} from "@/components/workflows/executions/labelsModel";
import { runExecutionAction, useRefreshExecution } from "@/components/workflows/executions/useExecutionAction";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";

type Props = { ws: string; execution: ExecutionDto; open: boolean; onOpenChange: (o: boolean) => void };

const FALLBACK = "The labels could not be saved. Try again.";

/**
 * Edit an ended execution's labels (package 7d D6, D10; Kestra's `setLabels`): the whole list,
 * the flow's labels included, replaces the execution's. `system.*` labels are egret's own and
 * are kept as they are. The form is mounted only while open, so it starts from the execution's
 * labels each time.
 */
export function EditLabelsDialog(props: Props) {
  // Escape and the overlay close through the Dialog: ignored while a save is in flight (FE-ACTION-3).
  const saving = useRef(false);
  return (
    <Dialog open={props.open} onOpenChange={(o) => (o || !saving.current) && props.onOpenChange(o)}>
      <DialogContent>{props.open ? <LabelsForm {...props} saving={saving} /> : null}</DialogContent>
    </Dialog>
  );
}

function LabelsForm({ ws, execution, open, onOpenChange, saving }: Props & { saving: MutableRefObject<boolean> }) {
  const id = useId();
  const refresh = useRefreshExecution(ws, execution.id);
  const save = useWorkflowSetExecutionLabels();
  const [rows, setRows] = useState<LabelRow[]>(() => rowsFromLabels(execution.effective_labels));
  // Shown once Save was pressed, then kept up to date as the user fixes them.
  const [checked, setChecked] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  // The row whose key input takes focus when it mounts: one Add label just appended.
  const focusRow = useRef<number | null>(null);

  useUnsavedChangesWarning(open && rowsChanged(execution.effective_labels, rows));

  const errors = checked ? rowErrors(rows) : new Map<number, { key?: string; value?: string }>();

  const update = (i: number, patch: Partial<LabelRow>) => setRows((prev) => prev.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const removeRow = (i: number) => setRows((prev) => prev.filter((_, j) => j !== i));
  const addRow = () => {
    // The new row's key takes focus: it appears because of the user's own action (FE-A11Y-6).
    focusRow.current = rows.length;
    setRows((prev) => [...prev, { key: "", value: "", system: false }]);
  };

  const submit = async () => {
    setChecked(true);
    setError(null);
    if (rowErrors(rows).size > 0) return;
    setPending(true);
    saving.current = true;
    try {
      await runExecutionAction({
        call: () => save.mutateAsync({ workspace: ws, id: execution.id, data: labelsFromRows(rows) }),
        expected: 200,
        failure: FALLBACK,
        refresh,
        refusal: labelsRefusal,
        onSuccess: () => toast.success("Saved the labels"),
      });
      onOpenChange(false);
    } catch (e) {
      // The rows stay as typed (FE-ACTION-3); an `invalid_labels` message names the label.
      setError(e instanceof Error && e.message ? e.message : FALLBACK);
    } finally {
      saving.current = false;
      setPending(false);
    }
  };

  return (
    <form
      className="grid gap-4"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <DialogHeader>
        <DialogTitle>Edit labels</DialogTitle>
        <DialogDescription>
          These labels replace the execution's labels, the flow's included. Labels set by egret (<code>system.*</code>) are kept.
        </DialogDescription>
      </DialogHeader>
      {rows.length === 0 ? <p className="text-sm text-muted-foreground">No labels. Add one, or save to clear them.</p> : null}
      <ul role="list" className="grid gap-2">
        {rows.map((row, i) => {
          const n = i + 1;
          const e = errors.get(i);
          const keyId = `${id}-key-${i}`;
          const valueId = `${id}-value-${i}`;
          return (
            <li key={i} className="grid gap-1">
              <div className="flex items-start gap-2">
                <div className="grid min-w-0 flex-1 gap-1">
                  <Label htmlFor={keyId} className="sr-only">
                    Key of label {n}
                  </Label>
                  <Input
                    id={keyId}
                    ref={(el) => {
                      if (el && focusRow.current === i) {
                        focusRow.current = null;
                        el.focus();
                      }
                    }}
                    value={row.key}
                    placeholder="Key"
                    readOnly={row.system}
                    aria-invalid={e?.key ? true : undefined}
                    aria-describedby={e?.key ? `${keyId}-error` : undefined}
                    className="font-mono text-sm"
                    onChange={(ev) => update(i, { key: ev.target.value })}
                  />
                  {e?.key ? (
                    <p id={`${keyId}-error`} className="text-xs text-destructive">
                      {e.key}
                    </p>
                  ) : null}
                </div>
                <div className="grid min-w-0 flex-1 gap-1">
                  <Label htmlFor={valueId} className="sr-only">
                    Value of label {n}
                  </Label>
                  <Input
                    id={valueId}
                    value={row.value}
                    placeholder="Value"
                    readOnly={row.system}
                    aria-invalid={e?.value ? true : undefined}
                    aria-describedby={e?.value ? `${valueId}-error` : undefined}
                    className="font-mono text-sm"
                    onChange={(ev) => update(i, { value: ev.target.value })}
                  />
                  {e?.value ? (
                    <p id={`${valueId}-error`} className="text-xs text-destructive">
                      {e.value}
                    </p>
                  ) : null}
                </div>
                {row.system ? (
                  // Keeps the columns aligned with the rows that have a Remove button.
                  <span aria-hidden className="size-9 shrink-0" />
                ) : (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="shrink-0"
                    aria-label={`Remove label ${row.key}`}
                    onClick={() => removeRow(i)}
                  >
                    <Trash2 aria-hidden />
                  </Button>
                )}
              </div>
              {row.system ? <p className="text-xs text-muted-foreground">Set by egret; kept as it is</p> : null}
            </li>
          );
        })}
      </ul>
      <div>
        <Button type="button" variant="outline" size="sm" onClick={addRow}>
          <Plus aria-hidden />
          Add label
        </Button>
      </div>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <DialogFooter>
        <Button type="button" variant="outline" disabled={pending} onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button type="submit" disabled={pending}>
          {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
          Save
        </Button>
      </DialogFooter>
    </form>
  );
}
