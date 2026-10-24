import { useId } from "react";
import type { AccessUser } from "@/api/access";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";

/**
 * The delete and disable confirmations of a user, shared by the Users list and
 * the user detail page.
 *
 * A delete retires the user's engine username for good (username-takeover spec
 * 2026-10-23 §4.4): nobody can be given it again, so the dialog names it and
 * offers Disable, which keeps the username, grants and history.
 */
export function DeleteUserDialog({
  user,
  open,
  onOpenChange,
  onConfirm,
  onDisableInstead,
  canDisable,
  disableBlockedReason,
}: {
  user: AccessUser | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
  /** Closes this dialog and opens the disable confirmation. */
  onDisableInstead: () => void;
  /** Whether the viewer may disable users at all; without it Disable is not offered. */
  canDisable: boolean;
  /** Why the server would refuse disabling this user (FE-ACTION-4); `null` when it would not. */
  disableBlockedReason: string | null;
}) {
  const reasonId = useId();
  const offerDisable = user?.status === "active" && canDisable;
  return (
    <ConfirmDeleteDialog
      open={open}
      onOpenChange={onOpenChange}
      title={user ? `Delete "${user.email}"?` : "Delete user?"}
      description={
        <>
          Deletes this user, their sessions, group memberships and role grants.{" "}
          {user?.username ? (
            <>
              Their engine username <strong>{user.username}</strong> is retired for good: nobody
              can be given it again, including this person if they come back, so grants and
              policies naming it will never apply to anyone.
            </>
          ) : (
            "They have no engine username yet."
          )}{" "}
          {offerDisable
            ? "To stop them signing in and keep their username, grants and history, disable them instead."
            : null}
        </>
      }
      confirmText="delete"
      onConfirm={onConfirm}
    >
      {offerDisable && (
        <span className="flex flex-col items-start gap-1">
          <Button
            type="button"
            variant="outline"
            size="sm"
            aria-disabled={disableBlockedReason ? true : undefined}
            aria-describedby={disableBlockedReason ? reasonId : undefined}
            onClick={disableBlockedReason ? undefined : onDisableInstead}
          >
            Disable instead
          </Button>
          {disableBlockedReason ? (
            <span id={reasonId} className="text-xs">
              {disableBlockedReason}
            </span>
          ) : null}
        </span>
      )}
    </ConfirmDeleteDialog>
  );
}

/** KI-209: disabling signs the user out at once, so it asks first. */
export function DisableUserDialog({
  user,
  open,
  onOpenChange,
  onConfirm,
}: {
  user: AccessUser | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{user ? `Disable ${user.email}?` : "Disable user?"}</AlertDialogTitle>
          <AlertDialogDescription>
            They are signed out at once and cannot sign in until the account is enabled again.
            Their memberships and role grants are kept. Their engine username stays theirs.
            {user?.roles.includes("platform_admin") &&
              " While disabled they lose all-access on engines."}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={onConfirm}
            className="bg-destructive text-white hover:bg-destructive/90"
          >
            Disable user
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
