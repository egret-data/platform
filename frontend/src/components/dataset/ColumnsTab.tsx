import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { useDatasetMutations, type DatasetColumn } from "@/api/datasets";

interface Props {
  ws: string;
  datasetId: string;
  columns: DatasetColumn[];
}

interface EditState {
  verbose_name: string;
  description: string;
  expression: string;
}

export function ColumnsTab({ ws, datasetId, columns }: Props) {
  const { addColumn, updateColumn, deleteColumn } = useDatasetMutations(ws, datasetId);

  // Per-row inline edit: keyed by column_name
  const [editing, setEditing] = useState<Record<string, EditState>>({});

  // Add calculated column dialog
  const [addOpen, setAddOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [newExpr, setNewExpr] = useState("");
  const [newVerbose, setNewVerbose] = useState("");
  const [newDesc, setNewDesc] = useState("");

  function startEdit(c: DatasetColumn) {
    setEditing((prev) => ({
      ...prev,
      [c.column_name]: {
        verbose_name: c.verbose_name,
        description: c.description,
        expression: c.expression ?? "",
      },
    }));
  }

  function cancelEdit(name: string) {
    setEditing((prev) => {
      const next = { ...prev };
      delete next[name];
      return next;
    });
  }

  async function saveEdit(c: DatasetColumn) {
    const state = editing[c.column_name];
    if (!state) return;
    try {
      await updateColumn.mutateAsync({
        name: c.column_name,
        body: {
          verbose_name: state.verbose_name,
          description: state.description,
          ...(c.is_calculated ? { expression: state.expression } : {}),
        },
      });
      cancelEdit(c.column_name);
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function handleDelete(columnName: string) {
    try {
      await deleteColumn.mutateAsync(columnName);
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function handleAdd() {
    if (!newName.trim()) { toast.error("Column name required"); return; }
    if (!newExpr.trim()) { toast.error("Expression required"); return; }
    try {
      await addColumn.mutateAsync({
        column_name: newName.trim(),
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
          + Calculated column
        </Button>
      </div>

      <table className="w-full text-sm">
        <thead className="text-left text-muted-foreground">
          <tr>
            <th className="py-1 pr-2">Column</th>
            <th className="pr-2">Type</th>
            <th className="pr-2">Class</th>
            <th className="pr-2">Temporal</th>
            <th className="pr-2">Verbose name</th>
            <th className="pr-2">Description</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {columns.map((c) => {
            const editState = editing[c.column_name];
            const isEditing = !!editState;
            return (
              <tr key={c.column_name} className="border-t align-top">
                <td className="py-1 pr-2 font-mono">
                  {c.column_name}
                  {c.is_calculated && (
                    <Badge variant="secondary" className="ml-1 text-xs">calculated</Badge>
                  )}
                </td>
                <td className="pr-2 font-mono text-muted-foreground">{c.data_type}</td>
                <td className="pr-2">
                  <span className="rounded bg-muted px-1.5 py-0.5 text-xs">{c.type_generic}</span>
                </td>
                <td className="pr-2">{c.is_temporal ? "✓" : ""}</td>

                {isEditing ? (
                  <>
                    <td className="pr-2 pb-1">
                      <div className="space-y-1">
                        <Input
                          className="h-7 text-xs"
                          value={editState.verbose_name}
                          onChange={(e) =>
                            setEditing((prev) => ({
                              ...prev,
                              [c.column_name]: { ...prev[c.column_name], verbose_name: e.target.value },
                            }))
                          }
                          placeholder="Verbose name"
                        />
                        {c.is_calculated && (
                          <Input
                            className="h-7 font-mono text-xs"
                            value={editState.expression}
                            onChange={(e) =>
                              setEditing((prev) => ({
                                ...prev,
                                [c.column_name]: { ...prev[c.column_name], expression: e.target.value },
                              }))
                            }
                            placeholder="Expression"
                          />
                        )}
                      </div>
                    </td>
                    <td className="pr-2 pb-1">
                      <Input
                        className="h-7 text-xs"
                        value={editState.description}
                        onChange={(e) =>
                          setEditing((prev) => ({
                            ...prev,
                            [c.column_name]: { ...prev[c.column_name], description: e.target.value },
                          }))
                        }
                        placeholder="Description"
                      />
                    </td>
                    <td className="pb-1">
                      <div className="flex gap-1">
                        <Button size="sm" className="h-7 px-2 text-xs" onClick={() => saveEdit(c)}
                          disabled={updateColumn.isPending}>
                          Save
                        </Button>
                        <Button size="sm" variant="outline" className="h-7 px-2 text-xs"
                          onClick={() => cancelEdit(c.column_name)}>
                          Cancel
                        </Button>
                      </div>
                    </td>
                  </>
                ) : (
                  <>
                    <td className="pr-2 text-muted-foreground">{c.verbose_name || "—"}</td>
                    <td className="pr-2 text-muted-foreground">{c.description || "—"}</td>
                    <td>
                      <div className="flex gap-1">
                        <Button size="sm" variant="ghost" className="h-7 px-2 text-xs"
                          onClick={() => startEdit(c)}>
                          Edit
                        </Button>
                        {c.is_calculated && (
                          <Button size="sm" variant="ghost"
                            className="h-7 px-2 text-xs text-destructive hover:text-destructive"
                            onClick={() => handleDelete(c.column_name)}
                            disabled={deleteColumn.isPending}>
                            Delete
                          </Button>
                        )}
                      </div>
                    </td>
                  </>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>

      <Dialog open={addOpen} onOpenChange={setAddOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add calculated column</DialogTitle>
          </DialogHeader>
          <div className="space-y-3 pt-2">
            <Input
              placeholder="Column name"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
            />
            <Input
              className="font-mono"
              placeholder="Expression (e.g. price * quantity)"
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
              <Button onClick={handleAdd} disabled={addColumn.isPending}>
                {addColumn.isPending ? "Adding…" : "Add column"}
              </Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
