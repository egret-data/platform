import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import {
  type AccessItem, type AccessPolicyInput, type ServiceDef,
  createPolicy, updatePolicy, getServiceDef,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PrincipalPicker } from "./PrincipalPicker";

interface Props {
  open: boolean;
  service: string;
  initial?: { id: number; input: AccessPolicyInput };
  onOpenChange: (o: boolean) => void;
  onSaved: () => void;
}

const emptyLevel = () => ({ values: [] as string[], isExcludes: false, isRecursive: false });
const emptyItem = (): AccessItem => ({ users: [], groups: [], accesses: [], effect: "allow" });

function blankInput(service: string): AccessPolicyInput {
  return {
    name: "", service, isEnabled: true,
    resources: { catalog: emptyLevel(), schema: emptyLevel(), table: emptyLevel(), column: emptyLevel() },
    items: [emptyItem()],
    allowExceptions: [], denyExceptions: [], isDenyAllElse: false,
    description: "", labels: [],
  };
}

export function AccessPolicyDialog({ open, service, initial, onOpenChange, onSaved }: Props) {
  const [form, setForm] = useState<AccessPolicyInput>(() => initial?.input ?? blankInput(service));
  useEffect(() => {
    setForm(initial?.input ?? blankInput(service));
  }, [initial, service, open]);

  const def = useQuery<ServiceDef>({
    queryKey: ["service-def", service],
    queryFn: () => getServiceDef(service),
    enabled: open,
    retry: false,
  });
  const accessTypes = def.data?.accessTypes?.map((a) => a.name) ?? ["select", "insert", "delete", "update", "create", "drop"];

  const qc = useQueryClient();
  const save = useMutation<{ id: number }, ApiError>({
    mutationFn: () =>
      (initial ? updatePolicy(initial.id, { ...form, kind: "access" }) : createPolicy({ ...form, kind: "access" })),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["control-plane", "policies"] });
      onSaved();
      onOpenChange(false);
    },
  });

  const setLevel = (key: keyof AccessPolicyInput["resources"], value: string) =>
    setForm((f) => ({
      ...f,
      resources: { ...f.resources, [key]: { ...f.resources[key], values: value ? [value] : [] } },
    }));

  const setItem = (i: number, next: Partial<AccessItem>) =>
    setForm((f) => ({ ...f, items: f.items.map((it, j) => (j === i ? { ...it, ...next } : it)) }));

  const levelValue = (key: keyof AccessPolicyInput["resources"]) => form.resources[key].values[0] ?? "";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{initial ? "Edit" : "New"} access policy</DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1">
            <Label htmlFor="pname">Policy name</Label>
            <Input id="pname" aria-label="Policy name" value={form.name}
              onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} />
          </div>

          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={form.isEnabled}
              onChange={(e) => setForm((f) => ({ ...f, isEnabled: e.target.checked }))}
            />
            Enabled
          </label>

          <div className="grid grid-cols-2 gap-3">
            {(["catalog", "schema", "table", "column"] as const).map((k) => (
              <div key={k} className="space-y-1">
                <Label htmlFor={`res-${k}`} className="capitalize">{k}</Label>
                <Input id={`res-${k}`} aria-label={k[0].toUpperCase() + k.slice(1)}
                  placeholder="* or a name" value={levelValue(k)}
                  onChange={(e) => setLevel(k, e.target.value)} />
              </div>
            ))}
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
                <div className="flex items-center justify-between">
                  <select className="rounded border bg-background px-2 py-1 text-sm" value={it.effect}
                    aria-label={`effect ${i}`}
                    onChange={(e) => setItem(i, { effect: e.target.value as "allow" | "deny" })}>
                    <option value="allow">Allow</option>
                    <option value="deny">Deny</option>
                  </select>
                  {form.items.length > 1 && (
                    <button type="button" aria-label={`remove item ${i}`}
                      onClick={() => setForm((f) => ({ ...f, items: f.items.filter((_, j) => j !== i) }))}>
                      <Trash2 className="h-4 w-4 text-muted-foreground" />
                    </button>
                  )}
                </div>
                <PrincipalPicker users={it.users} groups={it.groups}
                  onChange={(p) => setItem(i, p)} />
                <div className="flex flex-wrap gap-2">
                  {accessTypes.map((a) => (
                    <label key={a} className="flex items-center gap-1 text-sm">
                      <input type="checkbox" checked={it.accesses.includes(a)}
                        onChange={(e) => setItem(i, {
                          accesses: e.target.checked ? [...it.accesses, a] : it.accesses.filter((x) => x !== a),
                        })} />
                      {a}
                    </label>
                  ))}
                </div>
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
