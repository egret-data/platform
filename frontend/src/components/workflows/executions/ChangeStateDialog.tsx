import { useId, useState } from "react";
import { Loader2 } from "lucide-react";
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
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { changeTargets, type ChangeTarget } from "@/components/workflows/executions/actions";

/** `FAILED` → `Failed`, the state chip's own word. */
const stateWord = (s: string) => s.charAt(0).toUpperCase() + s.slice(1).toLowerCase();

/**
 * A dialog that sets a state: a select of the four targets minus `current`, and a confirm that
 * stays open on failure with the message (FE-ACTION-3). The form lives in the dialog's content,
 * so it starts empty each time the dialog opens.
 */
export function ChangeStateDialog({
  open,
  onOpenChange,
  title,
  description,
  current,
  onSubmit,
  fallbackError,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  current: string;
  onSubmit: (state: ChangeTarget) => Promise<void>;
  fallbackError: string;
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <ChangeStateForm
          title={title}
          description={description}
          current={current}
          onSubmit={onSubmit}
          fallbackError={fallbackError}
          close={() => onOpenChange(false)}
        />
      </AlertDialogContent>
    </AlertDialog>
  );
}

function ChangeStateForm({
  title,
  description,
  current,
  onSubmit,
  fallbackError,
  close,
}: {
  title: string;
  description: string;
  current: string;
  onSubmit: (state: ChangeTarget) => Promise<void>;
  fallbackError: string;
  close: () => void;
}) {
  const selectId = useId();
  const [target, setTarget] = useState<ChangeTarget | "">("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const confirm = async () => {
    if (target === "") return;
    setPending(true);
    setError(null);
    try {
      await onSubmit(target);
      close();
    } catch (e) {
      setError(e instanceof Error && e.message ? e.message : fallbackError);
    } finally {
      setPending(false);
    }
  };

  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle>{title}</AlertDialogTitle>
        <AlertDialogDescription>{description}</AlertDialogDescription>
      </AlertDialogHeader>
      <div className="grid gap-1.5">
        <Label htmlFor={selectId}>New state</Label>
        <Select value={target} onValueChange={(v) => setTarget(v as ChangeTarget)} disabled={pending}>
          <SelectTrigger id={selectId} className="w-48">
            <SelectValue placeholder="Pick a state" />
          </SelectTrigger>
          <SelectContent>
            {changeTargets(current).map((s) => (
              <SelectItem key={s} value={s}>
                {stateWord(s)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <AlertDialogFooter>
        <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
        <Button type="button" disabled={pending || target === ""} onClick={() => void confirm()}>
          {pending ? <Loader2 aria-hidden className="animate-spin" /> : null}
          Change state
        </Button>
      </AlertDialogFooter>
    </>
  );
}
