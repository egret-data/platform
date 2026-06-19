import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { useDatasetMutations, type DatasetMetric } from "@/api/datasets";

interface Props {
  ws: string;
  datasetId: string;
  metrics: DatasetMetric[];
}

interface EditState {
  metric_name: string;
  expression: string;
  verbose_name: string;
  description: string;
}

export function MetricsTab({ ws, datasetId, metrics }: Props) {
  const { addMetric, updateMetric, deleteMetric } = useDatasetMutations(ws, datasetId);

  // Per-row inline edit: keyed by metric id
  const [editing, setEditing] = useState<Record<string, EditState>>({});

  // Add metric dialog
  const [addOpen, setAddOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [newExpr, setNewExpr] = useState("");
  const [newVerbose, setNewVerbose] = useState("");
  const [newDesc, setNewDesc] = useState("");

  function startEdit(m: DatasetMetric) {
    setEditing((prev) => ({
      ...prev,
      [m.id]: {
        metric_name: m.metric_name,
        expression: m.expression,
        verbose_name: m.verbose_name,
        description: m.description,
      },
    }));
  }

  function cancelEdit(id: string) {
    setEditing((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
  }

  async function saveEdit(m: DatasetMetric) {
    const state = editing[m.id];
    if (!state) return;
    try {
      await updateMetric.mutateAsync({
        metricId: m.id,
        body: {
          metric_name: state.metric_name,
          expression: state.expression,
          verbose_name: state.verbose_name,
          description: state.description,
        },
      });
      cancelEdit(m.id);
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function handleDelete(metricId: string) {
    try {
      await deleteMetric.mutateAsync(metricId);
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function handleAdd() {
    if (!newName.trim()) { toast.error("Metric name required"); return; }
    if (!newExpr.trim()) { toast.error("Expression required"); return; }
    try {
      await addMetric.mutateAsync({
        metric_name: newName.trim(),
        expression: newExpr.trim(),
        verbose_name: newVerbose.trim() || undefined,
        description: newDesc.trim() || undefined,
      });
      setAddOpen(false);
      setNewName("");
      setNewExpr("");
      setNewVerbose("");
      setNewDesc("");
    } catch (e) {
      toast.error(String(e));
    }
  }

  return (
    <div>
      <div className="mb-2 flex justify-end">
        <Button size="sm" variant="outline" onClick={() => setAddOpen(true)}>
          + Metric
        </Button>
      </div>

      {metrics.length === 0 ? (
        <p className="py-4 text-center text-sm text-muted-foreground">No metrics yet.</p>
      ) : (
        <table className="w-full text-sm">
          <thead className="text-left text-muted-foreground">
            <tr>
              <th className="py-1 pr-2">Metric name</th>
              <th className="pr-2">Expression</th>
              <th className="pr-2">Class</th>
              <th className="pr-2">Verbose name</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {metrics.map((m) => {
              const editState = editing[m.id];
              const isEditing = !!editState;
              return (
                <tr key={m.id} className="border-t align-top">
                  {isEditing ? (
                    <>
                      <td className="pr-2 pb-1">
                        <Input
                          className="h-7 text-xs"
                          value={editState.metric_name}
                          onChange={(e) =>
                            setEditing((prev) => ({
                              ...prev,
                              [m.id]: { ...prev[m.id], metric_name: e.target.value },
                            }))
                          }
                          placeholder="Metric name"
                        />
                      </td>
                      <td className="pr-2 pb-1">
                        <Input
                          className="h-7 font-mono text-xs"
                          value={editState.expression}
                          onChange={(e) =>
                            setEditing((prev) => ({
                              ...prev,
                              [m.id]: { ...prev[m.id], expression: e.target.value },
                            }))
                          }
                          placeholder="Expression"
                        />
                      </td>
                      <td className="pr-2">
                        <span className="rounded bg-muted px-1.5 py-0.5 text-xs">{m.type_generic}</span>
                      </td>
                      <td className="pr-2 pb-1">
                        <Input
                          className="h-7 text-xs"
                          value={editState.verbose_name}
                          onChange={(e) =>
                            setEditing((prev) => ({
                              ...prev,
                              [m.id]: { ...prev[m.id], verbose_name: e.target.value },
                            }))
                          }
                          placeholder="Verbose name"
                        />
                      </td>
                      <td className="pb-1">
                        <div className="flex gap-1">
                          <Button size="sm" className="h-7 px-2 text-xs" onClick={() => saveEdit(m)}
                            disabled={updateMetric.isPending}>
                            Save
                          </Button>
                          <Button size="sm" variant="outline" className="h-7 px-2 text-xs"
                            onClick={() => cancelEdit(m.id)}>
                            Cancel
                          </Button>
                        </div>
                      </td>
                    </>
                  ) : (
                    <>
                      <td className="py-1 pr-2 font-semibold">{m.metric_name}</td>
                      <td className="pr-2 font-mono text-muted-foreground">{m.expression}</td>
                      <td className="pr-2">
                        <span className="rounded bg-muted px-1.5 py-0.5 text-xs">{m.type_generic}</span>
                      </td>
                      <td className="pr-2 text-muted-foreground">{m.verbose_name || "—"}</td>
                      <td>
                        <div className="flex gap-1">
                          <Button size="sm" variant="ghost" className="h-7 px-2 text-xs"
                            onClick={() => startEdit(m)}>
                            Edit
                          </Button>
                          <Button size="sm" variant="ghost"
                            className="h-7 px-2 text-xs text-destructive hover:text-destructive"
                            onClick={() => handleDelete(m.id)}
                            disabled={deleteMetric.isPending}>
                            Delete
                          </Button>
                        </div>
                      </td>
                    </>
                  )}
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <Dialog open={addOpen} onOpenChange={setAddOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add metric</DialogTitle>
          </DialogHeader>
          <div className="space-y-3 pt-2">
            <Input
              placeholder="Metric name"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
            />
            <Input
              className="font-mono"
              placeholder="Expression (e.g. SUM(revenue))"
              value={newExpr}
              onChange={(e) => setNewExpr(e.target.value)}
            />
            <Input
              placeholder="Verbose name (optional)"
              value={newVerbose}
              onChange={(e) => setNewVerbose(e.target.value)}
            />
            <Textarea
              placeholder="Description (optional)"
              value={newDesc}
              onChange={(e) => setNewDesc(e.target.value)}
            />
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={() => setAddOpen(false)}>Cancel</Button>
              <Button onClick={handleAdd} disabled={addMetric.isPending}>
                {addMetric.isPending ? "Adding…" : "Add metric"}
              </Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
