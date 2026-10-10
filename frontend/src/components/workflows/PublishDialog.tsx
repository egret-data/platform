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
 * Publish the saved draft as a new version (spec §5.3), with an optional note.
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
      setError(e instanceof Error && e.message ? e.message : "The flow could not be published. Try again.");
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
          <DialogTitle>Publish {flowName}</DialogTitle>
          <DialogDescription>
            {nextVersion !== null ? `The saved draft becomes version ${nextVersion}. ` : "The saved draft becomes a new version. "}
            Executions run the published version, never the draft.
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
              // The dialog opened because the user asked to publish (FE-A11Y-6).
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
              Publish
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
