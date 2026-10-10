import { useState } from "react";
import { Loader2 } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

/**
 * "Create version" (spec §5.3): snapshot the saved draft as a numbered version,
 * with an optional note, to compare and restore later. It changes nothing that
 * runs: every execution runs the saved draft (the user's ruling, 2026-10-10).
 * The identifiers keep "publish", the backend route's name (`POST …/publish`).
 *
 * `onPublish` rejects with the server's message on failure; the dialog then
 * shows it and keeps the note (FE-ACTION-3). It closes only on success.
 */
export function PublishDialog({
  open,
  onOpenChange,
  flowName,
  nextVersion,
  onPublish,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** `namespace.flow_id`, as the header shows it. */
  flowName: string;
  /** The number the new version will get, when the versions are known. */
  nextVersion: number | null;
  onPublish: (note: string) => Promise<void>;
}) {
  const [note, setNote] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const submit = async () => {
    setPending(true);
    setError(null);
    try {
      await onPublish(note.trim());
      setNote("");
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error && e.message ? e.message : "The version could not be created. Try again.");
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (pending) return;
        if (!o) setError(null);
        onOpenChange(o);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create a version of {flowName}</DialogTitle>
          <DialogDescription>
            {nextVersion !== null
              ? `Version ${nextVersion} will be a snapshot of the saved draft, to compare and restore later. `
              : "The new version will be a snapshot of the saved draft, to compare and restore later. "}
            It does not change what runs: runs always use the saved draft.
          </DialogDescription>
        </DialogHeader>
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <div className="space-y-1.5">
            <Label htmlFor="publish-note">Note (optional)</Label>
            <Textarea
              id="publish-note"
              // The dialog opened because the user asked to create a version (FE-A11Y-6).
              autoFocus
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder="What changed in this version"
              rows={3}
            />
          </div>
          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" disabled={pending} onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={pending}>
              {pending && <Loader2 aria-hidden className="animate-spin" />}
              Create version
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
