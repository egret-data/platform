import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2, X } from "lucide-react";
import {
  type AccessItem, type AccessPolicyInput, type ServiceDef,
  createPolicy, updatePolicy, getServiceDef,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from "@/components/ui/accordion";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { PrincipalPicker } from "./PrincipalPicker";
import { ResourceLevelEditor } from "./ResourceLevelEditor";

interface Props {
  open: boolean;
  service: string;
  initial?: { id: number; input: AccessPolicyInput };
  onOpenChange: (o: boolean) => void;
  onSaved: () => void;
}

type ExceptionKey = "allowExceptions" | "denyExceptions";

const emptyLevel = () => ({ values: [] as string[], isExcludes: false, isRecursive: false });
const emptyItem = (effect: "allow" | "deny" = "allow"): AccessItem => ({ users: [], groups: [], accesses: [], effect });

function blankInput(service: string): AccessPolicyInput {
  return {
    name: "", service, isEnabled: true,
    resources: { catalog: emptyLevel(), schema: emptyLevel(), table: emptyLevel(), column: emptyLevel() },
    items: [emptyItem()],
    allowExceptions: [], denyExceptions: [], isDenyAllElse: false,
    description: "", labels: [],
  };
}

interface ItemRowProps {
  item: AccessItem;
  accessTypes: string[];
  removeLabel: string;
  onChange: (next: Partial<AccessItem>) => void;
  onRemove: () => void;
}

function ItemRow({ item, accessTypes, removeLabel, onChange, onRemove }: ItemRowProps) {
  return (
    <div className="space-y-2 rounded-md border p-3">
      <div className="flex items-start justify-between gap-2">
        <div className="flex-1">
          <PrincipalPicker users={item.users} groups={item.groups} onChange={(p) => onChange(p)} />
        </div>
        <button type="button" aria-label={removeLabel} onClick={onRemove}>
          <Trash2 className="h-4 w-4 text-muted-foreground" />
        </button>
      </div>
      <div className="flex flex-wrap gap-3">
        {accessTypes.map((a) => (
          <label key={a} className="flex items-center gap-1.5 text-sm">
            <Checkbox
              checked={item.accesses.includes(a)}
              onCheckedChange={(c) => onChange({
                accesses: c ? [...item.accesses, a] : item.accesses.filter((x) => x !== a),
              })}
            />
            {a}
          </label>
        ))}
      </div>
    </div>
  );
}

export function AccessPolicyDialog({ open, service, initial, onOpenChange, onSaved }: Props) {
  const [form, setForm] = useState<AccessPolicyInput>(() => initial?.input ?? blankInput(service));
  const [tab, setTab] = useState<"allow" | "deny">("allow");
  const [labelInput, setLabelInput] = useState("");

  useEffect(() => {
    setForm(initial?.input ?? blankInput(service));
    setTab("allow");
    setLabelInput("");
  }, [initial, service, open]);

  useEffect(() => {
    if (form.isDenyAllElse && tab === "deny") setTab("allow");
  }, [form.isDenyAllElse, tab]);

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

  const setItem = (i: number, next: Partial<AccessItem>) =>
    setForm((f) => ({ ...f, items: f.items.map((it, j) => (j === i ? { ...it, ...next } : it)) }));
  const addItem = (effect: "allow" | "deny") =>
    setForm((f) => ({ ...f, items: [...f.items, emptyItem(effect)] }));
  const removeItem = (i: number) =>
    setForm((f) => ({ ...f, items: f.items.filter((_, j) => j !== i) }));

  const setException = (key: ExceptionKey, i: number, next: Partial<AccessItem>) =>
    setForm((f) => ({ ...f, [key]: f[key].map((it, j) => (j === i ? { ...it, ...next } : it)) }));
  const addException = (key: ExceptionKey, effect: "allow" | "deny") =>
    setForm((f) => ({ ...f, [key]: [...f[key], emptyItem(effect)] }));
  const removeException = (key: ExceptionKey, i: number) =>
    setForm((f) => ({ ...f, [key]: f[key].filter((_, j) => j !== i) }));

  const addLabel = () => {
    const t = labelInput.trim();
    if (t && !form.labels.includes(t)) setForm((f) => ({ ...f, labels: [...f.labels, t] }));
    setLabelInput("");
  };
  const removeLabel = (l: string) => setForm((f) => ({ ...f, labels: f.labels.filter((x) => x !== l) }));

  const allowItems = form.items.map((it, i) => ({ it, i })).filter((x) => x.it.effect === "allow");
  const denyItems = form.items.map((it, i) => ({ it, i })).filter((x) => x.it.effect === "deny");

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{initial ? "Edit" : "New"} access policy</DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          <div className="flex items-end gap-3">
            <div className="flex-1 space-y-1">
              <Label htmlFor="pname">Policy name</Label>
              <Input id="pname" aria-label="Policy name" value={form.name}
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
                  <Label htmlFor="pdesc">Description</Label>
                  <Textarea id="pdesc" aria-label="Description" value={form.description}
                    onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))} />
                </div>
                <div className="space-y-1">
                  <Label htmlFor="plabels">Labels</Label>
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
                  <Input id="plabels" aria-label="Labels" placeholder="Add label…" value={labelInput}
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
            <div className="grid grid-cols-2 gap-3">
              {(["catalog", "schema", "table", "column"] as const).map((lvl) => (
                <ResourceLevelEditor key={lvl} label={lvl[0].toUpperCase() + lvl.slice(1)} level={lvl} service={service}
                  parents={{
                    catalog: form.resources.catalog.values[0],
                    schema: form.resources.schema.values[0],
                    table: form.resources.table.values[0],
                  }}
                  value={form.resources[lvl]}
                  onChange={(v) => setForm((f) => ({ ...f, resources: { ...f.resources, [lvl]: v } }))} />
              ))}
            </div>
          </div>

          <div className="flex items-center gap-2">
            <Switch aria-label="Deny all else" checked={form.isDenyAllElse}
              onCheckedChange={(c) => setForm((f) => ({ ...f, isDenyAllElse: c }))} />
            <span className="text-sm">Deny everything else not explicitly allowed</span>
          </div>

          <Tabs value={tab} onValueChange={(v) => setTab(v as "allow" | "deny")}>
            <TabsList>
              <TabsTrigger value="allow">Allow</TabsTrigger>
              {!form.isDenyAllElse && <TabsTrigger value="deny">Deny</TabsTrigger>}
            </TabsList>

            <TabsContent value="allow" className="space-y-3">
              <div className="flex items-center justify-between">
                <Label>Allow items</Label>
                <Button type="button" variant="outline" size="sm" onClick={() => addItem("allow")}>
                  <Plus className="mr-1 h-4 w-4" /> Add item
                </Button>
              </div>
              {allowItems.map(({ it, i }) => (
                <ItemRow key={i} item={it} accessTypes={accessTypes}
                  removeLabel={`remove allow item ${i}`}
                  onChange={(next) => setItem(i, next)}
                  onRemove={() => removeItem(i)} />
              ))}
              <Accordion type="single" collapsible>
                <AccordionItem value="exceptions">
                  <AccordionTrigger>Exceptions</AccordionTrigger>
                  <AccordionContent className="space-y-3">
                    <div className="flex items-center justify-between">
                      <span className="text-xs text-muted-foreground">Allow exceptions (deny overrides)</span>
                      <Button type="button" variant="outline" size="sm" onClick={() => addException("allowExceptions", "allow")}>
                        <Plus className="mr-1 h-4 w-4" /> Add exception
                      </Button>
                    </div>
                    {form.allowExceptions.map((it, i) => (
                      <ItemRow key={i} item={it} accessTypes={accessTypes}
                        removeLabel={`remove allow exception ${i}`}
                        onChange={(next) => setException("allowExceptions", i, next)}
                        onRemove={() => removeException("allowExceptions", i)} />
                    ))}
                  </AccordionContent>
                </AccordionItem>
              </Accordion>
            </TabsContent>

            {!form.isDenyAllElse && (
              <TabsContent value="deny" className="space-y-3">
                <div className="flex items-center justify-between">
                  <Label>Deny items</Label>
                  <Button type="button" variant="outline" size="sm" onClick={() => addItem("deny")}>
                    <Plus className="mr-1 h-4 w-4" /> Add item
                  </Button>
                </div>
                {denyItems.map(({ it, i }) => (
                  <ItemRow key={i} item={it} accessTypes={accessTypes}
                    removeLabel={`remove deny item ${i}`}
                    onChange={(next) => setItem(i, next)}
                    onRemove={() => removeItem(i)} />
                ))}
                <Accordion type="single" collapsible>
                  <AccordionItem value="exceptions">
                    <AccordionTrigger>Exceptions</AccordionTrigger>
                    <AccordionContent className="space-y-3">
                      <div className="flex items-center justify-between">
                        <span className="text-xs text-muted-foreground">Deny exceptions (allow overrides)</span>
                        <Button type="button" variant="outline" size="sm" onClick={() => addException("denyExceptions", "deny")}>
                          <Plus className="mr-1 h-4 w-4" /> Add exception
                        </Button>
                      </div>
                      {form.denyExceptions.map((it, i) => (
                        <ItemRow key={i} item={it} accessTypes={accessTypes}
                          removeLabel={`remove deny exception ${i}`}
                          onChange={(next) => setException("denyExceptions", i, next)}
                          onRemove={() => removeException("denyExceptions", i)} />
                      ))}
                    </AccordionContent>
                  </AccordionItem>
                </Accordion>
              </TabsContent>
            )}
          </Tabs>

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
