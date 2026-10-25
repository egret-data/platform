import { useState, useEffect } from "react";
import { useNavigate, Link } from "react-router-dom";
import { toast } from "sonner";
import { Code2 } from "lucide-react";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useDatabases, useCatalogs, useSchemas, useTables } from "@/api/sqllab";
import { useCreateDataset, type CreateSource } from "@/api/datasets";

/**
 * Two shapes, one component (matches Superset's split):
 *  - Default (no `initialSql`): create a PHYSICAL dataset by picking a table.
 *    Virtual/SQL datasets are made in SQL Lab, not here.
 *  - `initialSql` provided (SQL Lab "Save as dataset"): a
 *    "Save as dataset" form — name it; the query is fixed and shown read-only.
 */
export function CreateDatasetDialog({ ws, open, onOpenChange, initialSql, initialDbId, initialName }: {
  ws: string; open: boolean; onOpenChange: (o: boolean) => void;
  initialSql?: string; initialDbId?: string | null; initialName?: string;
}) {
  const sqlMode = initialSql != null;
  const nav = useNavigate();
  const { data: databases } = useDatabases(ws);
  const create = useCreateDataset(ws);
  const [dbId, setDbId] = useState(initialDbId ?? "");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const engine = databases?.find((d) => d.id === dbId)?.engine ?? null;
  const isTrino = engine === "trino";
  const [catalog, setCatalog] = useState("");
  const [schema, setSchema] = useState("");
  const [table, setTable] = useState("");
  // Only hit the engine for the physical picker, while open, and — on Trino —
  // not until a catalog is chosen (a `schemas?catalog=` with empty catalog 502s).
  const cats = useCatalogs(ws, open && !sqlMode && isTrino ? (dbId || null) : null);
  const schemas = useSchemas(ws, open && !sqlMode && dbId && !(isTrino && !catalog) ? dbId : null, isTrino ? catalog : "");
  const tables = useTables(ws, open && !sqlMode ? (dbId || null) : null, catalog, schema);
  const [sql, setSql] = useState(initialSql ?? "");

  useEffect(() => {
    if (open) {
      setName(initialName ?? "");
      setDescription("");
      setCatalog(""); setSchema(""); setTable("");
      setSql(initialSql ?? "");
      setDbId(initialDbId ?? "");
    }
  }, [open]);

  async function submit(source: CreateSource) {
    if (!dbId) { toast.error("Pick a database"); return; }
    if (!name.trim()) { toast.error("Name required"); return; }
    try {
      const ds = await create.mutateAsync({ name: name.trim(), database_id: dbId, description, source });
      toast.success("Dataset created");
      onOpenChange(false);
      nav(`/explore/${ws}/datasets/${ds.id}`);
    } catch (e) { toast.error(String(e)); }
  }

  const nameDesc = (
    <div className="grid gap-3 sm:grid-cols-2">
      <div className="space-y-1.5">
        <Label htmlFor="nd-name">Name</Label>
        <Input id="nd-name" placeholder="Dataset name" value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="space-y-1.5">
        <Label htmlFor="nd-desc">Description <span className="font-normal text-muted-foreground">(optional)</span></Label>
        <Input id="nd-desc" placeholder="What it's for" value={description} onChange={(e) => setDescription(e.target.value)} />
      </div>
    </div>
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        {sqlMode ? (
          <>
            <DialogHeader><DialogTitle>Save as dataset</DialogTitle></DialogHeader>
            <p className="-mt-1 text-sm text-muted-foreground">
              Save this SQL query as a reusable dataset you can build charts and dashboards on.
            </p>
            <div className="space-y-4 pt-1">
              {nameDesc}
              <div className="space-y-1.5">
                <Label>Query</Label>
                <pre className="max-h-40 overflow-auto rounded-md border bg-muted p-2 font-mono text-xs whitespace-pre-wrap">{sql.trim() || "—"}</pre>
                <p className="text-xs text-muted-foreground">To change the query, edit it in SQL Lab.</p>
              </div>
              <div className="flex justify-end gap-2">
                <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
                <Button disabled={!sql.trim() || !name.trim() || create.isPending}
                  onClick={() => submit({ kind: "virtual", sql })}>
                  {create.isPending ? "Saving…" : "Create dataset"}
                </Button>
              </div>
            </div>
          </>
        ) : (
          <>
            <DialogHeader><DialogTitle>New dataset</DialogTitle></DialogHeader>
            <p className="-mt-1 text-sm text-muted-foreground">
              Build a dataset from an existing table. Its columns are detected automatically.
            </p>
            <div className="space-y-4 pt-1">
              <div className="space-y-1.5">
                <Label>Connection</Label>
                <Select value={dbId} onValueChange={(v) => { setDbId(v); setCatalog(""); setSchema(""); setTable(""); }}>
                  <SelectTrigger className="w-full"><SelectValue placeholder="Choose a connection" /></SelectTrigger>
                  <SelectContent>{(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}</SelectContent>
                </Select>
              </div>
              {nameDesc}
              {isTrino && (
                <div className="space-y-1.5">
                  <Label>Catalog</Label>
                  <Select value={catalog} onValueChange={(v) => { setCatalog(v); setSchema(""); setTable(""); }} disabled={!dbId}>
                    <SelectTrigger className="w-full"><SelectValue placeholder={!dbId ? "Pick a database first" : "Choose a catalog"} /></SelectTrigger>
                    <SelectContent>{(cats.data ?? []).map((c) => <SelectItem key={c} value={c}>{c}</SelectItem>)}</SelectContent>
                  </Select>
                </div>
              )}
              <div className="space-y-1.5">
                <Label>Schema</Label>
                <Select value={schema} onValueChange={(v) => { setSchema(v); setTable(""); }} disabled={!dbId || (isTrino && !catalog)}>
                  <SelectTrigger className="w-full"><SelectValue placeholder={isTrino && !catalog ? "Pick a catalog first" : "Choose a schema"} /></SelectTrigger>
                  <SelectContent>{(schemas.data ?? []).map((s) => <SelectItem key={s} value={s}>{s}</SelectItem>)}</SelectContent>
                </Select>
              </div>
              <div className="space-y-1.5">
                <Label>Table</Label>
                <Select value={table} onValueChange={setTable} disabled={!schema}>
                  <SelectTrigger className="w-full"><SelectValue placeholder={!schema ? "Pick a schema first" : "Choose a table"} /></SelectTrigger>
                  <SelectContent>{(tables.data ?? []).map((t) => <SelectItem key={t} value={t}>{t}</SelectItem>)}</SelectContent>
                </Select>
              </div>
              <div className="flex items-center justify-between gap-3 pt-1">
                <Link to={`/explore/${ws}/sql-lab`} onClick={() => onOpenChange(false)}
                  className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground hover:underline">
                  <Code2 className="h-3.5 w-3.5" /> Need a custom query? Use SQL Lab
                </Link>
                <Button disabled={!table || create.isPending}
                  onClick={() => submit({ kind: "physical", catalog: isTrino ? catalog : null, schema, table })}>
                  {create.isPending ? "Creating…" : "Create"}
                </Button>
              </div>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
