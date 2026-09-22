import React, { useState } from 'react';
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
  onConfirm: () => void;
  children?: React.ReactNode;
}

export const ConfirmDeleteDialog: React.FC<ConfirmDeleteDialogProps> = ({
  open, onOpenChange, title, description, confirmText = 'delete', confirmLabel = 'Delete',
  confirmDisabled = false, onConfirm, children,
}) => {
  const [inputValue, setInputValue] = useState('');

  const handleConfirm = () => {
    if (confirmDisabled) return;
    if (inputValue.toLowerCase() === confirmText.toLowerCase()) {
      onConfirm();
      setInputValue('');
      onOpenChange(false);
    }
  };

  return (
    <AlertDialog open={open} onOpenChange={(o) => { if (!o) setInputValue(''); onOpenChange(o); }}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        {children && <div className="text-sm text-muted-foreground">{children}</div>}
        <div className="space-y-2">
          <p className="text-sm">Type <span className="font-mono font-bold">{confirmText}</span> to confirm:</p>
          <Input
            value={inputValue}
            onChange={e => setInputValue(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter') handleConfirm(); }}
            placeholder={confirmText}
            autoFocus
          />
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={() => setInputValue('')}>Cancel</AlertDialogCancel>
          <Button
            variant="destructive"
            onClick={handleConfirm}
            disabled={confirmDisabled || inputValue.toLowerCase() !== confirmText.toLowerCase()}
          >
            {confirmLabel}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
};
