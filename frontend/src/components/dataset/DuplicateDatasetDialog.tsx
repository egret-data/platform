import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Button } from "@/components/ui/button";
import { useDuplicateDataset } from "@/api/datasets";

/**
 * Duplicate a dataset of either kind (KI-110): the copy has the same source, every column setting
 * (display names, Dimension/Filter/Time, formats, roles, calculated columns) and every metric, and
 * opens on success. Certification, shares and tags are not copied.
 */
export function DuplicateDatasetDialog({ ws, dataset, onOpenChange }: {
  ws: string;
  /** The dataset to copy; `null` keeps the dialog closed. */
  dataset: { id: string; name: string } | null;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={dataset !== null} onOpenChange={onOpenChange}>
      <DialogContent>
        {/* Keyed by the dataset, so each opening starts from that dataset's default name. */}
        {dataset && <DuplicateForm key={dataset.id} ws={ws} dataset={dataset} onClose={() => onOpenChange(false)} />}
      </DialogContent>
    </Dialog>
  );
}

function DuplicateForm({ ws, dataset, onClose }: { ws: string; dataset: { id: string; name: string }; onClose: () => void }) {
  const nav = useNavigate();
  const dup = useDuplicateDataset(ws);
  const [name, setName] = useState(`copy of ${dataset.name}`);
  const [error, setError] = useState<string | null>(null);

  const submit = () => {
    if (!name.trim()) return;
    setError(null);
    dup.mutate(
      { id: dataset.id, name: name.trim() },
      {
        // Navigates to the copy, so no toast (FE-ACTION-2). A failure keeps the name typed and
        // shows the server's words under the field (FE-ACTION-3, FE-FORM-4).
        onSuccess: (copy) => { onClose(); nav(`/explore/${ws}/datasets/${copy.id}`); },
        onError: (e) => setError(e.message),
      },
    );
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>Duplicate dataset</DialogTitle>
        <DialogDescription>
          The copy keeps the source, the column settings and the metrics of “{dataset.name}”.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-1.5">
        <Label htmlFor="dup-name">Name</Label>
        <Input id="dup-name" value={name} aria-invalid={error ? true : undefined} aria-describedby={error ? "dup-name-error" : undefined}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") submit(); }} />
        {error && <p id="dup-name-error" className="text-xs text-destructive">{error}</p>}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={onClose}>Cancel</Button>
        <Button onClick={submit} disabled={!name.trim() || dup.isPending}>{dup.isPending ? "Duplicating…" : "Duplicate"}</Button>
      </DialogFooter>
    </>
  );
}
