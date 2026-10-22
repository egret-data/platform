import React, { useId, useState } from 'react';
import {
  AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle,
  AlertDialogDescription, AlertDialogFooter, AlertDialogCancel,
} from './alert-dialog';
import { Button } from './button';
import { Input } from './input';

interface ConfirmDeleteDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  confirmText?: string; // text user must type, default "delete"
  // The confirm button's own label — defaults to "Delete" so every existing
  // call site (which reuses this component for genuine deletes) is
  // unaffected. A call site whose action isn't a delete (revoke, rollback…)
  // should pass its own verb here rather than let the button lie about what
  // it does.
  confirmLabel?: string;
  /**
   * Withholds the confirm button when the caller already KNOWS the action will
   * be refused — e.g. a catalog the server will not delete while an engine is
   * still attached. Defaults to `false`, so every existing call site is
   * unaffected; the same reasoning as `confirmLabel` above. A dialog that
   * explains the refusal and then offers the button anyway invites the user to
   * type the confirmation word and collect an error they were just warned
   * about.
   */
  confirmDisabled?: boolean;
  /**
   * Why the action cannot happen, when the caller knows it will be refused
   * (FE-ACTION-4). Given, the type-to-confirm field is not shown, and the
   * confirm button stays focusable but `aria-disabled`, with this reason as
   * its description; a click does nothing. The reason may hold links (the
   * things to change first). Optional; omitted, nothing changes.
   */
  confirmDisabledReason?: React.ReactNode;
  /**
   * Why the confirm button waits while the typed text does not match yet
   * (FE-ACTION-4). Given, the button is `aria-disabled` rather than native
   * `disabled` until it matches: it stays focusable, this reason is shown under
   * the field as its description, and a click does nothing. Optional; omitted,
   * the button stays natively disabled as before.
   */
  typeToConfirmReason?: string;
  /**
   * Match `confirmText` exactly, case included. Defaults to `false` (every
   * existing call site types a word whose case does not matter). A
   * classification's key and name can differ only in case (`pii` / `PII`),
   * and typing the name must not confirm the delete of the key.
   */
  caseSensitive?: boolean;
  /**
   * Passed to the dialog's content: a dialog opened from state rather than a
   * Radix trigger has no element to return focus to, so its caller can focus
   * the opener here (as `EnginePermissionsPanel` does). Optional; omitted,
   * nothing changes.
   */
  onCloseAutoFocus?: (event: Event) => void;
  onConfirm: () => void;
  children?: React.ReactNode;
}

export const ConfirmDeleteDialog: React.FC<ConfirmDeleteDialogProps> = ({
  open, onOpenChange, title, description, confirmText = 'delete', confirmLabel = 'Delete',
  confirmDisabled = false, confirmDisabledReason, typeToConfirmReason, caseSensitive = false, onCloseAutoFocus, onConfirm, children,
}) => {
  const [inputValue, setInputValue] = useState('');
  const reasonId = useId();
  const typeReasonId = useId();
  const blocked = confirmDisabledReason !== undefined && confirmDisabledReason !== null;
  const matches = caseSensitive
    ? inputValue === confirmText
    : inputValue.toLowerCase() === confirmText.toLowerCase();
  // Waiting only for the typed text, with a reason to say: aria-disabled, never native disabled.
  const awaitingText = !blocked && !confirmDisabled && !matches && typeToConfirmReason !== undefined;

  const handleConfirm = () => {
    if (confirmDisabled || blocked) return;
    if (matches) {
      onConfirm();
      setInputValue('');
      onOpenChange(false);
    }
  };

  return (
    <AlertDialog open={open} onOpenChange={(o) => { if (!o) setInputValue(''); onOpenChange(o); }}>
      <AlertDialogContent onCloseAutoFocus={onCloseAutoFocus}>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        {children && <div className="text-sm text-muted-foreground">{children}</div>}
        {blocked ? (
          <div id={reasonId} className="text-sm">{confirmDisabledReason}</div>
        ) : (
          <div className="space-y-2">
            <p className="text-sm">Type <span className="font-mono font-bold">{confirmText}</span> to confirm:</p>
            <Input
              value={inputValue}
              onChange={e => setInputValue(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter') handleConfirm(); }}
              placeholder={confirmText}
              autoFocus
            />
            {awaitingText && (
              <p id={typeReasonId} className="text-xs text-muted-foreground">{typeToConfirmReason}</p>
            )}
          </div>
        )}
        <AlertDialogFooter>
          <AlertDialogCancel onClick={() => setInputValue('')}>Cancel</AlertDialogCancel>
          <Button
            variant="destructive"
            onClick={handleConfirm}
            disabled={!blocked && !awaitingText && (confirmDisabled || !matches)}
            aria-disabled={blocked || awaitingText ? true : undefined}
            aria-describedby={blocked ? reasonId : awaitingText ? typeReasonId : undefined}
          >
            {confirmLabel}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
};
