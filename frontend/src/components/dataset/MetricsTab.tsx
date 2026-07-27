import { useState } from "react";
import { toast } from "sonner";
import { BadgeCheck, Sigma } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useDatasetMutations, type DatasetMetric, type MetricInput } from "@/api/datasets";

interface Props {
  ws: string;
  datasetId: string;
  metrics: DatasetMetric[];
}

type Editing = (Partial<DatasetMetric> & { id?: string; metric_name: string; expression: string }) | null;

export function MetricsTab({ ws, datasetId, metrics }: Props) {
  const { addMetric, updateMetric, deleteMetric } = useDatasetMutations(ws, datasetId);
  const [editing, setEditing] = useState<Editing>(null);

  // Inline edits (format/certified) resend the whole metric so the backend can
  // re-validate the expression.
  const patchMetric = (m: DatasetMetric, patch: Partial<MetricInput>) =>
    updateMetric.mutateAsync({
      metricId: m.id,
      body: { metric_name: m.metric_name, expression: m.expression, verbose_name: m.verbose_name, description: m.description, format: m.format, certified: m.certified, ...patch },
    }).catch((e) => toast.error(String(e)));

  async function save() {
    if (!editing) return;
    const body: MetricInput = {
      metric_name: editing.metric_name.trim(),
      expression: editing.expression.trim(),
      verbose_name: (editing.verbose_name ?? "").trim(),
      description: (editing.description ?? "").trim(),
      format: (editing.format ?? "").trim(),
      certified: editing.certified ?? false,
    };
    if (!body.metric_name || !body.expression) { toast.error("Name and expression required"); return; }
    try {
      if (editing.id) await updateMetric.mutateAsync({ metricId: editing.id, body });
      else await addMetric.mutateAsync(body);
      setEditing(null);
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <div>
      <div className="mb-3 flex items-center gap-2">
        <span className="text-xs text-muted-foreground">{metrics.length} metric{metrics.length === 1 ? "" : "s"}</span>
        <Button size="sm" variant="outline" className="ml-auto gap-1" onClick={() => setEditing({ metric_name: "", expression: "" })}>
          <Sigma className="h-3.5 w-3.5" /> + Metric
        </Button>
      </div>

      {metrics.length === 0 ? (
        <div className="flex flex-col items-center justify-center gap-2 rounded-md border py-8 text-center text-sm text-muted-foreground">
          <Sigma className="h-7 w-7 opacity-40" /> No metrics yet.
        </div>
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <table className="w-full text-sm">
            <thead className="border-b bg-secondary text-left text-xs text-muted-foreground">
              <tr>
                <th className="px-3 py-2 font-medium">Metric</th>
                <th className="px-2 py-2 font-medium">Expression</th>
                <th className="px-2 py-2 font-medium">Format</th>
                <th className="px-2 py-2 text-center font-medium" title="Trusted / certified metric">Certified</th>
                <th className="px-2 py-2 font-medium">Display name</th>
                <th className="w-8" />
              </tr>
            </thead>
            <tbody>
              {metrics.map((m) => (
                <tr key={m.id} className="border-t align-middle hover:bg-accent/40">
                  <td className="px-3 py-1.5">
                    <span className="inline-flex items-center gap-1.5 font-semibold">
                      {m.certified && <BadgeCheck className="h-4 w-4 text-primary" />}
                      {m.metric_name}
                      <Badge variant="secondary" className="text-[10px]">{m.type_generic}</Badge>
                    </span>
                  </td>
                  <td className="px-2 py-1.5 font-mono text-xs text-muted-foreground">{m.expression}</td>
                  <td className="px-2 py-1.5">
                    <Input key={`fmt-${m.format}`} defaultValue={m.format} placeholder="—" className="h-7 w-20 font-mono text-xs"
                      onBlur={(e) => { if (e.target.value !== m.format) patchMetric(m, { format: e.target.value }); }} />
                  </td>
                  <td className="px-2 py-1.5 text-center"><Switch checked={m.certified} onCheckedChange={(v) => patchMetric(m, { certified: v })} aria-label={`Certified ${m.metric_name}`} /></td>
                  <td className="px-2 py-1.5 text-muted-foreground">{m.verbose_name || "—"}</td>
                  <td className="px-2 py-1.5 text-right">
                    <div className="flex justify-end gap-1">
                      <Button size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={() => setEditing({ ...m })}>Edit</Button>
                      <Button size="sm" variant="ghost" className="h-7 px-2 text-xs text-destructive hover:text-destructive"
                        onClick={() => deleteMetric.mutateAsync(m.id).catch((e) => toast.error(String(e)))}>Delete</Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <Dialog open={editing != null} onOpenChange={(o) => { if (!o) setEditing(null); }}>
        <DialogContent>
          <DialogHeader><DialogTitle>{editing?.id ? "Edit metric" : "Add metric"}</DialogTitle></DialogHeader>
          {editing && (
            <div className="space-y-3 pt-1">
              <Input placeholder="Metric name" value={editing.metric_name} onChange={(e) => setEditing({ ...editing, metric_name: e.target.value })} />
              <Input className="font-mono" placeholder="Expression (e.g. SUM(revenue))" value={editing.expression} onChange={(e) => setEditing({ ...editing, expression: e.target.value })} />
              <div className="flex gap-2">
                <Input placeholder="Display name (optional)" value={editing.verbose_name ?? ""} onChange={(e) => setEditing({ ...editing, verbose_name: e.target.value })} />
                <Input className="w-28 font-mono" placeholder="Format" value={editing.format ?? ""} onChange={(e) => setEditing({ ...editing, format: e.target.value })} />
              </div>
              <Textarea placeholder="Description (optional)" value={editing.description ?? ""} onChange={(e) => setEditing({ ...editing, description: e.target.value })} />
              <label className="flex items-center gap-2 text-sm">
                <Switch checked={editing.certified ?? false} onCheckedChange={(v) => setEditing({ ...editing, certified: v })} /> Certified
              </label>
              <div className="flex justify-end gap-2">
                <Button variant="outline" onClick={() => setEditing(null)}>Cancel</Button>
                <Button onClick={save} disabled={addMetric.isPending || updateMetric.isPending}>Save</Button>
              </div>
            </div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
