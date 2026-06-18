import { useState } from "react";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

export function SaveQueryDialog({ open, onOpenChange, onSave }: {
  open: boolean; onOpenChange: (o: boolean) => void; onSave: (name: string, description: string) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit() {
    if (!name.trim()) return;
    setBusy(true);
    try { await onSave(name.trim(), description.trim()); onOpenChange(false); setName(""); setDescription(""); }
    finally { setBusy(false); }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>Save query</DialogTitle></DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1"><Label htmlFor="sq-name">Name</Label><Input id="sq-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Monthly revenue" /></div>
          <div className="space-y-1"><Label htmlFor="sq-desc">Description</Label><Textarea id="sq-desc" value={description} onChange={(e) => setDescription(e.target.value)} rows={3} /></div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={submit} disabled={busy || !name.trim()}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
