import { useState } from "react";
import { toast } from "sonner";
import { BadgeCheck, Sigma } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import { Label } from "@/components/ui/label";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { HelpCircle } from "lucide-react";
import { useDatasetMutations, type DatasetMetric, type MetricInput } from "@/api/datasets";

/** A column header with an info icon that explains the field in plain language. */
function HeaderHint({ label, hint, center }: { label: string; hint: string; center?: boolean }) {
  return (
    <span className={`inline-flex items-center gap-1 ${center ? "justify-center" : ""}`}>
      {label}
      <Tooltip>
        <TooltipTrigger asChild>
          <HelpCircle className="h-3 w-3 cursor-help opacity-50 hover:opacity-100" />
        </TooltipTrigger>
        <TooltipContent side="top" className="max-w-[240px] text-left font-normal normal-case">{hint}</TooltipContent>
      </Tooltip>
    </span>
  );
}

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
    <TooltipProvider delayDuration={200}>
      <div className="mb-2 flex items-center gap-2">
        <span className="text-xs text-muted-foreground">{metrics.length} metric{metrics.length === 1 ? "" : "s"}</span>
        <Button size="sm" variant="outline" className="ml-auto gap-1" onClick={() => setEditing({ metric_name: "", expression: "" })}>
          <Sigma className="h-3.5 w-3.5" /> + Metric
        </Button>
      </div>
      <p className="mb-3 text-xs text-muted-foreground">
        Metrics are reusable aggregations (like total sales) that charts can measure. Changes save automatically.
      </p>

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
                <th className="px-2 py-2 font-medium"><HeaderHint label="Expression" hint="The SQL aggregation, e.g. SUM(amount) or COUNT(*)." /></th>
                <th className="px-2 py-2 font-medium"><HeaderHint label="Format" hint="How the value is shown. Examples: $0.00 for money, 0.0% for percent." /></th>
                <th className="px-2 py-2 text-center font-medium"><HeaderHint center label="Certified" hint="Mark as trusted/official so people know it's the approved metric." /></th>
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
                    <Input key={`fmt-${m.format}`} defaultValue={m.format} placeholder="e.g. $0.00" className="h-7 w-24 font-mono text-xs"
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
        <DialogContent className="sm:max-w-lg">
          <DialogHeader><DialogTitle>{editing?.id ? "Edit metric" : "Add metric"}</DialogTitle></DialogHeader>
          {editing && (
            <div className="space-y-4 pt-1">
              <div className="space-y-1.5">
                <Label htmlFor="m-name">Metric name</Label>
                <Input id="m-name" placeholder="e.g. total_sales" value={editing.metric_name} onChange={(e) => setEditing({ ...editing, metric_name: e.target.value })} />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="m-expr">Expression</Label>
                <Input id="m-expr" className="font-mono" placeholder="e.g. SUM(amount)" value={editing.expression} onChange={(e) => setEditing({ ...editing, expression: e.target.value })} />
                <p className="text-xs text-muted-foreground">A SQL aggregation over the dataset's columns — SUM, COUNT, AVG, MIN, MAX…</p>
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1.5">
                  <Label htmlFor="m-vn">Display name <span className="font-normal text-muted-foreground">(optional)</span></Label>
                  <Input id="m-vn" placeholder="e.g. Total Sales" value={editing.verbose_name ?? ""} onChange={(e) => setEditing({ ...editing, verbose_name: e.target.value })} />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="m-fmt">Format <span className="font-normal text-muted-foreground">(optional)</span></Label>
                  <Input id="m-fmt" className="font-mono" placeholder="e.g. $0.00" value={editing.format ?? ""} onChange={(e) => setEditing({ ...editing, format: e.target.value })} />
                </div>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="m-desc">Description <span className="font-normal text-muted-foreground">(optional)</span></Label>
                <Textarea id="m-desc" placeholder="What this metric measures" value={editing.description ?? ""} onChange={(e) => setEditing({ ...editing, description: e.target.value })} />
              </div>
              <label className="flex items-center gap-2 text-sm">
                <Switch checked={editing.certified ?? false} onCheckedChange={(v) => setEditing({ ...editing, certified: v })} />
                Certified <span className="text-muted-foreground">— mark as the trusted, official metric</span>
              </label>
              <div className="flex justify-end gap-2">
                <Button variant="outline" onClick={() => setEditing(null)}>Cancel</Button>
                <Button onClick={save} disabled={addMetric.isPending || updateMetric.isPending}>Save</Button>
              </div>
            </div>
          )}
        </DialogContent>
      </Dialog>
    </TooltipProvider>
  );
}
