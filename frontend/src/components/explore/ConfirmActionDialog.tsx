import type { ReactNode } from "react";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { buttonVariants } from "@/components/ui/button";

/**
 * A yes/no confirmation for an action that can be undone but should not happen by accident:
 * moving to the Trash, deleting a folder (its items survive), reverting a revision (KI-075).
 * `ConfirmDeleteDialog` stays the dialog for what cannot be undone, with its type-to-confirm.
 */
export function ConfirmActionDialog({ open, onOpenChange, title, description, confirmLabel, onConfirm }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: ReactNode;
  /** The verb on the button, naming what it does ("Move to Trash"), never a bare "OK". */
  confirmLabel: string;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction className={buttonVariants({ variant: "destructive" })} onClick={onConfirm}>
            {confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** How a list names what it holds, for the confirmations below. */
export interface ItemNoun { singular: string; plural: string }

/**
 * "Move to Trash?" for one row (by its name) or the bulk selection (by its count), shared by the
 * Dashboards, Charts, Apps and Alerts lists (KI-075, KI-281). Closed while `pending` is null.
 */
export function ConfirmTrashDialog({ pending, noun, onCancel, onConfirm }: {
  pending: { name: string } | { count: number } | null;
  noun: ItemNoun;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const description = pending === null ? "" : "count" in pending
    ? `${pending.count === 1 ? `1 ${noun.singular} moves` : `${pending.count} ${noun.plural} move`} to the Trash. You can restore them from there.`
    : `"${pending.name}" moves to the Trash. You can restore it from there.`;
  return (
    <ConfirmActionDialog
      open={pending !== null}
      onOpenChange={(o) => { if (!o) onCancel(); }}
      title="Move to Trash?"
      description={description}
      confirmLabel="Move to Trash"
      onConfirm={onConfirm}
    />
  );
}

/**
 * "Delete folder?", saying what happens to what is in it: it moves to Uncategorized, nothing is
 * deleted. `itemCount` is left out where the list cannot count the folder's contents (the Alerts
 * list shows one type per tab, while a folder holds both).
 */
export function ConfirmFolderDeleteDialog({ folder, itemCount, noun, onCancel, onConfirm }: {
  folder: { name: string } | null;
  itemCount?: number;
  noun: ItemNoun;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const what = itemCount === undefined ? "Everything in it moves to Uncategorized; nothing is deleted."
    : itemCount === 0 ? "It is empty."
    : `The ${itemCount === 1 ? `${noun.singular} in it moves` : `${itemCount} ${noun.plural} in it move`} to Uncategorized; none is deleted.`;
  return (
    <ConfirmActionDialog
      open={folder !== null}
      onOpenChange={(o) => { if (!o) onCancel(); }}
      title="Delete folder?"
      description={`Delete the folder "${folder?.name ?? ""}"? ${what}`}
      confirmLabel="Delete folder"
      onConfirm={onConfirm}
    />
  );
}
