import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2, X } from "lucide-react";
import {
  type RowFilterItem, type RowFilterPolicyInput,
  createPolicy, updatePolicy,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { PrincipalPicker } from "./PrincipalPicker";
import { ResourceLevelEditor } from "./ResourceLevelEditor";

interface Props {
  open: boolean;
  service: string;
  initial?: { id: number; input: RowFilterPolicyInput };
  onOpenChange: (o: boolean) => void;
  onSaved: () => void;
}

const emptyLevel = () => ({ values: [] as string[], isExcludes: false, isRecursive: false });
const emptyItem = (): RowFilterItem => ({ users: [], groups: [], filterExpr: "" });

function blankInput(service: string): RowFilterPolicyInput {
  return {
    kind: "rowfilter",
    name: "", service, isEnabled: true,
    resources: { catalog: emptyLevel(), schema: emptyLevel(), table: emptyLevel() },
    items: [emptyItem()],
    description: "", labels: [],
  };
}

export function RowFilterPolicyDialog({ open, service, initial, onOpenChange, onSaved }: Props) {
  const [form, setForm] = useState<RowFilterPolicyInput>(() => initial?.input ?? blankInput(service));
  const [labelInput, setLabelInput] = useState("");
  useEffect(() => {
    setForm(initial?.input ?? blankInput(service));
    setLabelInput("");
  }, [initial, service, open]);

  const qc = useQueryClient();
  const save = useMutation<{ id: number }, ApiError>({
    mutationFn: () => (initial ? updatePolicy(initial.id, form) : createPolicy(form)),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["control-plane", "policies"] });
      onSaved();
      onOpenChange(false);
    },
  });

  const setItem = (i: number, next: Partial<RowFilterItem>) =>
    setForm((f) => ({ ...f, items: f.items.map((it, j) => (j === i ? { ...it, ...next } : it)) }));

  const addLabel = () => {
    const t = labelInput.trim();
    if (t && !form.labels.includes(t)) setForm((f) => ({ ...f, labels: [...f.labels, t] }));
    setLabelInput("");
  };
  const removeLabel = (l: string) => setForm((f) => ({ ...f, labels: f.labels.filter((x) => x !== l) }));

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{initial ? "Edit" : "New"} row-filter policy</DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          <div className="flex items-end gap-3">
            <div className="flex-1 space-y-1">
              <Label htmlFor="rfpname">Policy name</Label>
              <Input id="rfpname" aria-label="Policy name" value={form.name}
                onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} />
            </div>
            <div className="flex items-center gap-2 pb-2">
              <Switch aria-label="Enabled" checked={form.isEnabled}
                onCheckedChange={(c) => setForm((f) => ({ ...f, isEnabled: c }))} />
              <span className="text-sm">Enabled</span>
            </div>
            <Popover>
              <PopoverTrigger asChild>
                <Button type="button" variant="outline" size="sm">Details</Button>
              </PopoverTrigger>
              <PopoverContent align="end" className="w-80 space-y-3">
                <div className="space-y-1">
                  <Label htmlFor="rfpdesc">Description</Label>
                  <Textarea id="rfpdesc" aria-label="Description" value={form.description}
                    onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))} />
                </div>
                <div className="space-y-1">
                  <Label htmlFor="rfplabels">Labels</Label>
                  {form.labels.length > 0 && (
                    <div className="flex flex-wrap gap-1">
                      {form.labels.map((l) => (
                        <Badge key={l} variant="secondary" className="gap-1">
                          {l}
                          <button type="button" aria-label={`remove label ${l}`} onClick={() => removeLabel(l)}>
                            <X className="h-3 w-3" />
                          </button>
                        </Badge>
                      ))}
                    </div>
                  )}
                  <Input id="rfplabels" aria-label="Labels" placeholder="Add label…" value={labelInput}
                    onChange={(e) => setLabelInput(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === ",") { e.preventDefault(); addLabel(); }
                    }} />
                </div>
              </PopoverContent>
            </Popover>
          </div>

          <div className="space-y-3 rounded-md border p-3">
            <Label>Resources</Label>
            <div className="grid grid-cols-3 gap-3">
              {(["catalog", "schema", "table"] as const).map((lvl) => (
                <ResourceLevelEditor key={lvl} label={lvl[0].toUpperCase() + lvl.slice(1)} level={lvl} service={service}
                  parents={{
                    catalog: form.resources.catalog.values[0],
                    schema: form.resources.schema.values[0],
                  }}
                  value={form.resources[lvl]}
                  onChange={(v) => setForm((f) => ({ ...f, resources: { ...f.resources, [lvl]: v } }))} />
              ))}
            </div>
          </div>

          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <Label>Policy items</Label>
              <Button type="button" variant="outline" size="sm"
                onClick={() => setForm((f) => ({ ...f, items: [...f.items, emptyItem()] }))}>
                <Plus className="mr-1 h-4 w-4" /> Add item
              </Button>
            </div>
            {form.items.map((it, i) => (
              <div key={i} className="space-y-2 rounded-md border p-3">
                <div className="flex items-center justify-between gap-2">
                  <Input aria-label={`filter ${i}`} placeholder="e.g. country = 'US'"
                    value={it.filterExpr} onChange={(e) => setItem(i, { filterExpr: e.target.value })} />
                  {form.items.length > 1 && (
                    <button type="button" aria-label={`remove item ${i}`}
                      onClick={() => setForm((f) => ({ ...f, items: f.items.filter((_, j) => j !== i) }))}>
                      <Trash2 className="h-4 w-4 text-muted-foreground" />
                    </button>
                  )}
                </div>
                <PrincipalPicker users={it.users} groups={it.groups}
                  onChange={(p) => setItem(i, p)} />
              </div>
            ))}
          </div>

          {save.isError && <Alert variant="destructive"><AlertDescription>{save.error.message}</AlertDescription></Alert>}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={() => save.mutate()} disabled={save.isPending || !form.name.trim()}>
            {save.isPending ? "Saving…" : "Save"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
