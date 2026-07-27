import { useMemo, useState } from "react";
import { toast } from "sonner";
import { Calculator, Clock, Hash, Search, Type as TypeIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useDatasetMutations, type ColumnUpdate, type DatasetColumn } from "@/api/datasets";

interface Props {
  ws: string;
  datasetId: string;
  columns: DatasetColumn[];
}

/** Icon hinting a column's generic class. */
function classIcon(g: string) {
  if (g === "temporal") return <Clock className="h-3.5 w-3.5 text-muted-foreground" />;
  if (g === "numeric") return <Hash className="h-3.5 w-3.5 text-muted-foreground" />;
  return <TypeIcon className="h-3.5 w-3.5 text-muted-foreground" />;
}

export function ColumnsTab({ ws, datasetId, columns }: Props) {
  const { addColumn, updateColumn, deleteColumn } = useDatasetMutations(ws, datasetId);
  const [q, setQ] = useState("");
  const [addOpen, setAddOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [newExpr, setNewExpr] = useState("");
  const [newVerbose, setNewVerbose] = useState("");
  const [newDesc, setNewDesc] = useState("");
  const [exprFor, setExprFor] = useState<DatasetColumn | null>(null);
  const [exprVal, setExprVal] = useState("");

  const save = (name: string, body: ColumnUpdate) =>
    updateColumn.mutateAsync({ name, body }).catch((e) => toast.error(String(e)));

  const calcCount = columns.filter((c) => c.is_calculated).length;
  const shown = useMemo(() => {
    const n = q.trim().toLowerCase();
    return n ? columns.filter((c) => c.column_name.toLowerCase().includes(n) || c.verbose_name.toLowerCase().includes(n)) : columns;
  }, [columns, q]);

  async function handleAdd() {
    if (!newName.trim() || !newExpr.trim()) { toast.error("Name and expression required"); return; }
    try {
      await addColumn.mutateAsync({ column_name: newName.trim(), expression: newExpr.trim(), verbose_name: newVerbose.trim() || undefined, description: newDesc.trim() || undefined });
      setAddOpen(false); setNewName(""); setNewExpr(""); setNewVerbose(""); setNewDesc("");
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <div>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <div className="relative max-w-xs flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input className="h-8 pl-8" placeholder="Search columns…" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        <span className="text-xs text-muted-foreground">
          {columns.length} column{columns.length === 1 ? "" : "s"}{calcCount ? ` · ${calcCount} calculated` : ""}
        </span>
        <Button size="sm" variant="outline" className="ml-auto gap-1" onClick={() => setAddOpen(true)}>
          <Calculator className="h-3.5 w-3.5" /> + Calculated column
        </Button>
      </div>

      <div className="overflow-x-auto rounded-md border">
        <table className="w-full text-sm">
          <thead className="border-b bg-secondary text-left text-xs text-muted-foreground">
            <tr>
              <th className="px-3 py-2 font-medium">Column</th>
              <th className="px-2 py-2 font-medium">Type</th>
              <th className="px-2 py-2 text-center font-medium" title="Usable as a groupby dimension">Dimension</th>
              <th className="px-2 py-2 text-center font-medium" title="Usable as a filter">Filter</th>
              <th className="px-2 py-2 text-center font-medium" title="Treat as a time column">Time</th>
              <th className="px-2 py-2 font-medium" title="Display format (d3-format / date pattern)">Format</th>
              <th className="px-2 py-2 font-medium">Display name</th>
              <th className="px-2 py-2 font-medium">Description</th>
              <th className="w-8" />
            </tr>
          </thead>
          <tbody>
            {shown.length === 0 && (
              <tr><td colSpan={9} className="px-3 py-4 text-center text-muted-foreground">
                {columns.length === 0 ? "No columns yet — Refresh to sync from the source." : `No columns match “${q}”.`}
              </td></tr>
            )}
            {shown.map((c) => (
              <tr key={c.column_name} className="border-t align-middle hover:bg-accent/40">
                <td className="px-3 py-1.5">
                  <span className="inline-flex items-center gap-1.5">
                    {c.is_calculated ? <Calculator className="h-3.5 w-3.5 text-primary" /> : classIcon(c.type_generic)}
                    <span className="font-mono">{c.column_name}</span>
                    {c.is_calculated && <Badge variant="secondary" className="text-[10px]">calculated</Badge>}
                  </span>
                </td>
                <td className="px-2 py-1.5 font-mono text-xs text-muted-foreground">{c.data_type}</td>
                <td className="px-2 py-1.5 text-center"><Switch checked={c.groupby} onCheckedChange={(v) => save(c.column_name, { groupby: v })} aria-label={`Dimension ${c.column_name}`} /></td>
                <td className="px-2 py-1.5 text-center"><Switch checked={c.filterable} onCheckedChange={(v) => save(c.column_name, { filterable: v })} aria-label={`Filterable ${c.column_name}`} /></td>
                <td className="px-2 py-1.5 text-center"><Switch checked={c.is_temporal} onCheckedChange={(v) => save(c.column_name, { is_temporal: v })} aria-label={`Temporal ${c.column_name}`} /></td>
                <td className="px-2 py-1.5">
                  <Input key={`fmt-${c.format}`} defaultValue={c.format} placeholder="—" className="h-7 w-20 font-mono text-xs"
                    onBlur={(e) => { if (e.target.value !== c.format) save(c.column_name, { format: e.target.value }); }} />
                </td>
                <td className="px-2 py-1.5">
                  <Input key={`vn-${c.verbose_name}`} defaultValue={c.verbose_name} placeholder="—" className="h-7 w-36 text-xs"
                    onBlur={(e) => { if (e.target.value !== c.verbose_name) save(c.column_name, { verbose_name: e.target.value }); }} />
                </td>
                <td className="px-2 py-1.5">
                  <Input key={`ds-${c.description}`} defaultValue={c.description} placeholder="—" className="h-7 w-48 text-xs"
                    onBlur={(e) => { if (e.target.value !== c.description) save(c.column_name, { description: e.target.value }); }} />
                </td>
                <td className="px-2 py-1.5 text-right">
                  {c.is_calculated && (
                    <div className="flex justify-end gap-1">
                      <Button size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={() => { setExprFor(c); setExprVal(c.expression ?? ""); }}>Expr</Button>
                      <Button size="sm" variant="ghost" className="h-7 px-2 text-xs text-destructive hover:text-destructive"
                        onClick={() => deleteColumn.mutateAsync(c.column_name).catch((e) => toast.error(String(e)))}>Delete</Button>
                    </div>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Add calculated column */}
      <Dialog open={addOpen} onOpenChange={setAddOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>Add calculated column</DialogTitle></DialogHeader>
          <div className="space-y-3 pt-2">
            <Input placeholder="Column name" value={newName} onChange={(e) => setNewName(e.target.value)} />
            <Input className="font-mono" placeholder="Expression (e.g. price * quantity)" value={newExpr} onChange={(e) => setNewExpr(e.target.value)} />
            <Input placeholder="Display name (optional)" value={newVerbose} onChange={(e) => setNewVerbose(e.target.value)} />
            <Textarea placeholder="Description (optional)" value={newDesc} onChange={(e) => setNewDesc(e.target.value)} />
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={() => setAddOpen(false)}>Cancel</Button>
              <Button onClick={handleAdd} disabled={addColumn.isPending}>{addColumn.isPending ? "Adding…" : "Add column"}</Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>

      {/* Edit a calculated column's expression */}
      <Dialog open={exprFor != null} onOpenChange={(o) => { if (!o) setExprFor(null); }}>
        <DialogContent>
          <DialogHeader><DialogTitle className="font-mono text-sm">{exprFor?.column_name}</DialogTitle></DialogHeader>
          <Textarea className="h-28 font-mono text-sm" value={exprVal} onChange={(e) => setExprVal(e.target.value)} placeholder="Expression" />
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setExprFor(null)}>Cancel</Button>
            <Button onClick={async () => { if (exprFor) { await save(exprFor.column_name, { expression: exprVal }); setExprFor(null); } }}>Save</Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
