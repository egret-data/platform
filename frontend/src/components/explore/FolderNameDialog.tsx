import { useEffect, useState } from "react";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

/** Small styled dialog for entering/editing a folder name (replaces native prompt). */
export function FolderNameDialog({ open, title, submitLabel = "Create", initialValue = "", onOpenChange, onSubmit }: {
  open: boolean;
  title: string;
  submitLabel?: string;
  initialValue?: string;
  onOpenChange: (open: boolean) => void;
  onSubmit: (name: string) => void;
}) {
  const [name, setName] = useState(initialValue);
  useEffect(() => { if (open) setName(initialValue); }, [open, initialValue]);
  const submit = () => { const n = name.trim(); if (n) { onSubmit(n); onOpenChange(false); } };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>{title}</DialogTitle></DialogHeader>
        <Input autoFocus placeholder="Folder name" value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") submit(); }} />
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={submit} disabled={!name.trim()}>{submitLabel}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
