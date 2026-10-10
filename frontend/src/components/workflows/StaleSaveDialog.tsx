import { useState } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";

/**
 * A save refused because the draft changed since the editor read it (spec
 * §5.3, the 409 from `if_updated_at`). Two ways on:
 *
 * - **Reload** discards the local edits and loads the saved draft. That loses
 *   work, so it asks again first (FE-ACTION-1).
 * - **Overwrite** saves the local text anyway, without the precondition.
 *
 * Cancel keeps the edits in the editor and saves nothing.
 */
export function StaleSaveDialog({
  open,
  onOpenChange,
  onReload,
  onOverwrite,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onReload: () => Promise<void>;
  onOverwrite: () => Promise<void>;
}) {
  const [confirmingReload, setConfirmingReload] = useState(false);
  const [pending, setPending] = useState<"reload" | "overwrite" | null>(null);

  const close = () => {
    setConfirmingReload(false);
    onOpenChange(false);
  };
  const run = async (which: "reload" | "overwrite", action: () => Promise<void>) => {
    setPending(which);
    try {
      await action();
      close();
    } catch {
      // The caller has said what went wrong (a toast); the dialog stays so the user can choose again.
    } finally {
      setPending(null);
    }
  };

  return (
    <AlertDialog
      open={open}
      onOpenChange={(o) => {
        if (pending) return;
        if (!o) close();
      }}
    >
      <AlertDialogContent>
        {confirmingReload ? (
          <>
            <AlertDialogHeader>
              <AlertDialogTitle>Discard your edits?</AlertDialogTitle>
              <AlertDialogDescription>
                Reloading replaces the text in the editor with the saved draft. Your unsaved edits are lost.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <Button variant="outline" disabled={pending !== null} onClick={() => setConfirmingReload(false)}>
                Keep my edits
              </Button>
              <Button variant="destructive" disabled={pending !== null} onClick={() => void run("reload", onReload)}>
                {pending === "reload" && <Loader2 aria-hidden className="animate-spin" />}
                Discard and reload
              </Button>
            </AlertDialogFooter>
          </>
        ) : (
          <>
            <AlertDialogHeader>
              <AlertDialogTitle>The draft changed since you opened it</AlertDialogTitle>
              <AlertDialogDescription>
                Someone saved or published this flow after you opened it, so your save was refused and nothing was
                written. Reload the saved draft, or overwrite it with your text.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel disabled={pending !== null}>Cancel</AlertDialogCancel>
              <Button variant="outline" disabled={pending !== null} onClick={() => setConfirmingReload(true)}>
                Reload
              </Button>
              <Button variant="destructive" disabled={pending !== null} onClick={() => void run("overwrite", onOverwrite)}>
                {pending === "overwrite" && <Loader2 aria-hidden className="animate-spin" />}
                Overwrite
              </Button>
            </AlertDialogFooter>
          </>
        )}
      </AlertDialogContent>
    </AlertDialog>
  );
}
