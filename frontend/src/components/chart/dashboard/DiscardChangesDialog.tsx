import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { buttonVariants } from "@/components/ui/button";

/** Metabase's leave confirmation, ported from LeaveConfirmModal.tsx:12-22 (dispatched from
 *  DashboardHeader.tsx:69-87). It fires only when the draft is dirty, so a clean Cancel still
 *  exits in one click — the confirmation exists to stop silent data loss, not to add a step.
 *
 *  `AlertDialog`, not `Dialog`: this is a confirmation, it has no close button to keep pinned, and
 *  Radix's `alertdialog` role is what makes a screen reader announce it as one rather than as an
 *  ordinary modal. It also matches the other confirmations in this codebase
 *  (`ui/confirm-delete-dialog.tsx`). */
export function DiscardChangesDialog({ open, onOpenChange, onDiscard }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDiscard: () => void;
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Discard your changes?</AlertDialogTitle>
          <AlertDialogDescription>
            Your changes haven't been saved, so you'll lose them if you navigate away.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          {/* AlertDialogAction already applies `buttonVariants()`; the destructive variant is
              appended so twMerge resolves the bg/text conflict in its favour. */}
          <AlertDialogAction className={buttonVariants({ variant: "destructive" })} onClick={onDiscard}>
            Discard changes
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
