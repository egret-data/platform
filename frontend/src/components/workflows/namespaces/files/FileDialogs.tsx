import { useId, useState, type FormEvent } from "react";
import { Loader2 } from "lucide-react";
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
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

/**
 * A one-field dialog (FE-PAGE-3): a name to create or rename, or the folder
 * to move to. `onSubmit` resolves to `null` when done, or to the error to
 * show under the field: the form's own check, or the server's message
 * (FE-FORM-4). What was typed is kept on failure (FE-ACTION-3).
 */
export function OneFieldDialog({
  title,
  description,
  label,
  initial,
  submitLabel,
  help,
  suggestions,
  onSubmit,
  onClose,
}: {
  title: string;
  description: string;
  label: string;
  initial: string;
  submitLabel: string;
  help?: string;
  /** Values to pick from while typing (a native combobox, FE-FORM-3). */
  suggestions?: string[];
  onSubmit: (value: string) => Promise<string | null>;
  onClose: () => void;
}) {
  const id = useId();
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      const failure = await onSubmit(value);
      if (failure) setError(failure);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(o) => (o || busy ? undefined : onClose())}>
      <DialogContent>
        <form onSubmit={(e) => void submit(e)} className="grid gap-4">
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>{description}</DialogDescription>
          </DialogHeader>
          <div className="grid gap-1.5">
            <Label htmlFor={`${id}-field`}>{label}</Label>
            <Input
              id={`${id}-field`}
              // The user opened this dialog to type here (FE-A11Y-6).
              autoFocus
              value={value}
              list={suggestions ? `${id}-options` : undefined}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={error ? true : undefined}
              aria-describedby={[help ? `${id}-help` : null, error ? `${id}-error` : null].filter(Boolean).join(" ") || undefined}
              onChange={(e) => {
                setValue(e.target.value);
                setError(null);
              }}
            />
            {suggestions ? (
              <datalist id={`${id}-options`}>
                {suggestions.map((s) => (
                  <option key={s} value={s} />
                ))}
              </datalist>
            ) : null}
            {help ? (
              <p id={`${id}-help`} className="text-xs text-muted-foreground">
                {help}
              </p>
            ) : null}
            {error ? (
              <p id={`${id}-error`} data-testid="file-dialog-error" className="text-sm text-destructive">
                {error}
              </p>
            ) : null}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" disabled={busy} onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={busy}>
              {busy ? <Loader2 aria-hidden className="animate-spin" /> : null}
              {submitLabel}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * The question before a delete, naming how many files go (W7d spec §4.3).
 * `count` is null while it is being counted; `countError` says why it
 * could not be.
 */
export function DeleteDialog({
  name,
  isDir,
  count,
  countError,
  unsaved,
  busy,
  error,
  onConfirm,
  onClose,
}: {
  name: string;
  isDir: boolean;
  count: number | null;
  countError: string | null;
  /** Open files under the path with unsaved edits. */
  unsaved: number;
  busy: boolean;
  error: string | null;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const files = (n: number) => (n === 1 ? "1 file" : `${n} files`);
  const what = !isDir
    ? `This deletes the file ${name}.`
    : countError
      ? `This deletes the folder ${name} and every file in it. How many could not be counted: ${countError}`
      : count === null
        ? `This deletes the folder ${name} and every file in it.`
        : count === 0
          ? `This deletes the empty folder ${name}.`
          : `This deletes the folder ${name} and the ${files(count)} in it.`;
  return (
    <AlertDialog open onOpenChange={(o) => (o || busy ? undefined : onClose())}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{isDir ? `Delete the folder ${name}?` : `Delete ${name}?`}</AlertDialogTitle>
          <AlertDialogDescription>
            {what} Its revisions are kept until a flow&apos;s PurgeFiles task removes them.
            {unsaved > 0 ? ` Unsaved edits to ${unsaved === 1 ? "1 open file are" : `${unsaved} open files are`} lost.` : ""}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {isDir && count === null && !countError ? (
          <p role="status" className="text-sm text-muted-foreground">
            Counting the files…
          </p>
        ) : null}
        {error ? (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <Button type="button" variant="destructive" disabled={busy || (isDir && count === null && !countError)} onClick={onConfirm}>
            {busy ? <Loader2 aria-hidden className="animate-spin" /> : null}
            Delete
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** Imported files whose path already holds a file: overwrite them, or keep what is there. */
export function OverwriteDialog({ paths, busy, onConfirm, onClose }: { paths: string[]; busy: boolean; onConfirm: () => void; onClose: () => void }) {
  const files = paths.length === 1 ? "1 file" : `${paths.length} files`;
  return (
    <AlertDialog open onOpenChange={(o) => (o || busy ? undefined : onClose())}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Overwrite {files}?</AlertDialogTitle>
          <AlertDialogDescription>
            These already exist, so they were not imported. Overwriting writes the imported content as a new revision of each; the old
            revisions are kept.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <ul aria-label="Files that already exist" className="max-h-48 overflow-auto rounded-md border p-2 font-mono text-xs">
          {paths.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>Keep the existing files</AlertDialogCancel>
          <Button type="button" variant="destructive" disabled={busy} onClick={onConfirm}>
            {busy ? <Loader2 aria-hidden className="animate-spin" /> : null}
            Overwrite
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
