import { useMemo, useState } from "react";
import { toast } from "sonner";
import { Calculator, Clock, Hash, HelpCircle, Search, Type as TypeIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FormatPicker } from "@/components/dataset/FormatPicker";
import { useDatasetMutations, type ColumnUpdate, type DatasetColumn } from "@/api/datasets";

interface Props {
  ws: string;
  datasetId: string;
  columns: DatasetColumn[];
}

/** Icon hinting a column's generic class. */
/** The Role picker's "no role" item. A Radix item cannot carry `""`, which is what the column
 *  stores for "no role", so the picker shows this and saves `""` in its place. */
const NO_ROLE = "__no_role__";

function classIcon(g: string) {
  if (g === "temporal") return <Clock className="h-3.5 w-3.5 text-muted-foreground" />;
  if (g === "numeric") return <Hash className="h-3.5 w-3.5 text-muted-foreground" />;
  return <TypeIcon className="h-3.5 w-3.5 text-muted-foreground" />;
}

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
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);

  const save = (name: string, body: ColumnUpdate) =>
    updateColumn.mutateAsync({ name, body }).catch((e) => toast.error(String(e)));

  const calcCount = columns.filter((c) => c.is_calculated).length;
  const sourceCols = columns.filter((c) => !c.is_calculated).map((c) => c.column_name);
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
    <TooltipProvider delayDuration={200}>
      <div className="mb-2 flex flex-wrap items-center gap-2">
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
      <p className="mb-3 text-xs text-muted-foreground">
        Choose how each column can be used in charts, and give it a friendly name and number format. Changes save automatically.
      </p>

      <div className="overflow-x-auto rounded-md border">
        <table className="w-full text-sm">
          <thead className="border-b bg-secondary text-left text-xs text-muted-foreground">
            <tr>
              <th className="px-3 py-2 font-medium">Column</th>
              <th className="px-2 py-2 font-medium">Type</th>
              <th className="px-2 py-2 font-medium"><HeaderHint label="Role" hint="Semantic role — Latitude/Longitude fill a pin map's coordinate pickers, Email/URL/Image render specially, Currency formats as money." /></th>
              <th className="px-2 py-2 text-center font-medium"><HeaderHint center label="Dimension" hint="Group charts by this column — a category like Country or Product." /></th>
              <th className="px-2 py-2 text-center font-medium"><HeaderHint center label="Filter" hint="Let viewers filter charts by this column." /></th>
              <th className="px-2 py-2 text-center font-medium"><HeaderHint center label="Time" hint="This column is a date/time — turn on to enable time-based charts." /></th>
              <th className="px-2 py-2 font-medium"><HeaderHint label="Format" hint="How values are shown. Examples: $0.00 for money, 0.0% for percent, #,##0 for whole numbers." /></th>
              <th className="px-2 py-2 font-medium"><HeaderHint label="Display name" hint="A friendly label shown in charts instead of the raw column name." /></th>
              <th className="px-2 py-2 font-medium"><HeaderHint label="Description" hint="A short note explaining what this column contains." /></th>
              <th className="w-8" />
            </tr>
          </thead>
          <tbody>
            {shown.length === 0 && (
              <tr><td colSpan={10} className="px-3 py-4 text-center text-muted-foreground">
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
                <td className="px-2 py-1.5">
                  {/* w-28, not the w-24 this was: "Longitude" is the longest option in the list and
                      did not fit, and a select that clips its own selected value is how a reader
                      cannot tell Latitude from Longitude at a glance. */}
                  <Select value={c.semantic_type || NO_ROLE}
                    onValueChange={(v) => save(c.column_name, { semantic_type: v === NO_ROLE ? "" : v })}>
                    <SelectTrigger size="sm" className="w-28 px-2 text-xs" aria-label={`Role ${c.column_name}`}><SelectValue /></SelectTrigger>
                    <SelectContent>
                    <SelectItem value={NO_ROLE}>—</SelectItem>
                    <SelectItem value="category">Category</SelectItem>
                    <SelectItem value="country">Country</SelectItem>
                    <SelectItem value="currency">Currency</SelectItem>
                    <SelectItem value="email">Email</SelectItem>
                    <SelectItem value="url">URL</SelectItem>
                    <SelectItem value="image">Image</SelectItem>
                    {/* Metabase's `type/Latitude` and `type/Longitude`, spelled as bare lowercase
                        words because that is what egret's readers compare against. The pin map's two
                        coordinate pickers default from these, which is the ONLY reason they exist —
                        #434 refused to infer a coordinate column from its name ("a wrong guess plots
                        confidently in the wrong hemisphere"), and this is the difference between a
                        guess and a statement someone made here on purpose.

                        THIS LIST IS NO LONGER THE ONLY COPY OF THE VOCABULARY, and the comment that
                        used to say so was true when it was written and is now the opposite of true.
                        It read "nothing about the value is validated server-side … so these two
                        options ARE the vocabulary" — which was exactly the defect: `PUT
                        semantic_type: "banana"` persisted, and a typo produced a picker that
                        silently never filled. The vocabulary now lives in `SEMANTIC_TYPES`
                        (egret-analytics-api), is enforced on write with a 400, is a CHECK constraint
                        in migration 0104, and is published as an OpenAPI `enum`. Adding an option
                        here without adding it there now gets a 400 the moment a user picks it, so
                        the two are pinned from BOTH sides: a test in `DatasetTabs.test.tsx` asserts
                        this select's option values, and a Rust test asserts the constant. Neither
                        can see the other's file, which is why it takes two. */}
                    <SelectItem value="latitude">Latitude</SelectItem>
                    <SelectItem value="longitude">Longitude</SelectItem>
                    </SelectContent>
                  </Select>
                </td>
                <td className="px-2 py-1.5 text-center"><Switch checked={c.groupby} onCheckedChange={(v) => save(c.column_name, { groupby: v })} aria-label={`Dimension ${c.column_name}`} /></td>
                <td className="px-2 py-1.5 text-center"><Switch checked={c.filterable} onCheckedChange={(v) => save(c.column_name, { filterable: v })} aria-label={`Filterable ${c.column_name}`} /></td>
                <td className="px-2 py-1.5 text-center"><Switch checked={c.is_temporal} onCheckedChange={(v) => save(c.column_name, { is_temporal: v })} aria-label={`Temporal ${c.column_name}`} /></td>
                <td className="px-2 py-1.5">
                  <FormatPicker type={c.data_type} value={c.format} onChange={(v) => { if (v !== c.format) save(c.column_name, { format: v }); }} />
                </td>
                <td className="px-2 py-1.5">
                  <Input key={`vn-${c.verbose_name}`} defaultValue={c.verbose_name} placeholder="e.g. Country" className="h-7 w-36 text-xs"
                    onBlur={(e) => { if (e.target.value !== c.verbose_name) save(c.column_name, { verbose_name: e.target.value }); }} />
                </td>
                <td className="px-2 py-1.5">
                  <Input key={`ds-${c.description}`} defaultValue={c.description} placeholder="Add a note…" className="h-7 w-48 text-xs"
                    onBlur={(e) => { if (e.target.value !== c.description) save(c.column_name, { description: e.target.value }); }} />
                </td>
                <td className="px-2 py-1.5 text-right">
                  {c.is_calculated && (
                    <div className="flex justify-end gap-1">
                      <Button size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={() => { setExprFor(c); setExprVal(c.expression ?? ""); }}>Edit</Button>
                      <Button size="sm" variant="ghost" className="h-7 px-2 text-xs text-destructive hover:text-destructive"
                        onClick={() => setPendingDelete(c.column_name)}>Delete</Button>
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
        <DialogContent className="sm:max-w-lg">
          <DialogHeader><DialogTitle>Add calculated column</DialogTitle></DialogHeader>
          <p className="text-sm text-muted-foreground">
            Create a new column from a formula over the existing columns — like a spreadsheet cell.
          </p>
          <div className="space-y-4 pt-1">
            <div className="space-y-1.5">
              <Label htmlFor="cc-name">Column name</Label>
              <Input id="cc-name" placeholder="e.g. revenue" value={newName} onChange={(e) => setNewName(e.target.value)} />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="cc-expr">Formula</Label>
              <Input id="cc-expr" className="font-mono" placeholder="e.g. amount * 0.9" value={newExpr} onChange={(e) => setNewExpr(e.target.value)} />
              {sourceCols.length > 0 && (
                <div className="flex flex-wrap items-center gap-1 pt-0.5">
                  <span className="text-xs text-muted-foreground">Insert:</span>
                  {sourceCols.map((name) => (
                    <button key={name} type="button"
                      className="rounded border bg-secondary px-1.5 py-0.5 font-mono text-xs hover:bg-accent"
                      onClick={() => setNewExpr((p) => (p ? `${p} ${name}` : name))}>
                      {name}
                    </button>
                  ))}
                </div>
              )}
              <p className="text-xs text-muted-foreground">Write a SQL expression. Click a column above to add it, then combine with <code>+ - * /</code> or functions like <code>ROUND(…)</code>.</p>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1.5">
                <Label htmlFor="cc-vn">Display name <span className="font-normal text-muted-foreground">(optional)</span></Label>
                <Input id="cc-vn" placeholder="e.g. Revenue" value={newVerbose} onChange={(e) => setNewVerbose(e.target.value)} />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="cc-desc">Description <span className="font-normal text-muted-foreground">(optional)</span></Label>
                <Input id="cc-desc" placeholder="What it means" value={newDesc} onChange={(e) => setNewDesc(e.target.value)} />
              </div>
            </div>
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={() => setAddOpen(false)}>Cancel</Button>
              <Button onClick={handleAdd} disabled={addColumn.isPending}>{addColumn.isPending ? "Adding…" : "Add column"}</Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title={`Delete calculated column “${pendingDelete ?? ""}”?`}
        description="Charts that use this column will stop running until it is removed from them. This cannot be undone."
        confirmLabel="Delete column"
        onConfirm={() => {
          const name = pendingDelete;
          if (name === null) return;
          deleteColumn.mutateAsync(name).then(() => toast.success(`Deleted column ${name}`)).catch((e) => toast.error(String(e)));
        }}
      />

      {/* Edit a calculated column's expression */}
      <Dialog open={exprFor != null} onOpenChange={(o) => { if (!o) setExprFor(null); }}>
        <DialogContent>
          <DialogHeader><DialogTitle className="font-mono text-sm">{exprFor?.column_name}</DialogTitle></DialogHeader>
          <Label>Formula</Label>
          <Textarea className="h-28 font-mono text-sm" value={exprVal} onChange={(e) => setExprVal(e.target.value)} placeholder="e.g. amount * 0.9" />
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setExprFor(null)}>Cancel</Button>
            <Button onClick={async () => { if (exprFor) { await save(exprFor.column_name, { expression: exprVal }); setExprFor(null); } }}>Save</Button>
          </div>
        </DialogContent>
      </Dialog>
    </TooltipProvider>
  );
}
