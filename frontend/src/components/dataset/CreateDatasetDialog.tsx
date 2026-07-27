import { useState, useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { Code2, Table2 } from "lucide-react";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useDatabases, useCatalogs, useSchemas, useTables } from "@/api/sqllab";
import { useCreateDataset, type CreateSource } from "@/api/datasets";

export function CreateDatasetDialog({ ws, open, onOpenChange, initialSql, initialDbId, initialTab, initialName }: {
  ws: string; open: boolean; onOpenChange: (o: boolean) => void;
  initialSql?: string; initialDbId?: string | null; initialTab?: "table" | "sql"; initialName?: string;
}) {
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
  // Only hit the engine while the dialog is actually open (it stays mounted-but-
  // closed inside SQL Lab), and — on Trino — not until a catalog is chosen: a
  // `schemas?catalog=` with an empty catalog is invalid and 502s. Passing a null
  // db id disables the query via each hook's `enabled: !!db` gate.
  const cats = useCatalogs(ws, open && isTrino ? (dbId || null) : null);
  const schemas = useSchemas(ws, open && dbId && !(isTrino && !catalog) ? dbId : null, isTrino ? catalog : "");
  const tables = useTables(ws, open ? (dbId || null) : null, catalog, schema);
  const [sql, setSql] = useState(initialSql ?? "");
  const [tab, setTab] = useState<"table" | "sql">(initialTab ?? "table");

  useEffect(() => {
    if (open) {
      setName(initialName ?? "");
      setDescription("");
      setCatalog("");
      setSchema("");
      setTable("");
      setSql(initialSql ?? "");
      setDbId(initialDbId ?? "");
      setTab(initialTab ?? "table");
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

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader><DialogTitle>New dataset</DialogTitle></DialogHeader>
        <p className="-mt-1 text-sm text-muted-foreground">
          A dataset is a reusable, named source of data that you build charts and dashboards on.
        </p>

        <div className="space-y-4 pt-1">
          <div className="space-y-1.5">
            <Label>Database</Label>
            <Select value={dbId} onValueChange={(v) => { setDbId(v); setCatalog(""); setSchema(""); setTable(""); }}>
              <SelectTrigger className="w-full"><SelectValue placeholder="Choose a connection" /></SelectTrigger>
              <SelectContent>{(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}</SelectContent>
            </Select>
          </div>
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

          <div className="space-y-1.5">
            <Label>Source</Label>
            <Tabs value={tab} onValueChange={(v) => setTab(v as "table" | "sql")}>
              <TabsList className="grid w-full grid-cols-2">
                <TabsTrigger value="table" className="gap-1.5"><Table2 className="h-4 w-4" /> From table</TabsTrigger>
                <TabsTrigger value="sql" className="gap-1.5"><Code2 className="h-4 w-4" /> From SQL</TabsTrigger>
              </TabsList>

              <TabsContent value="table" className="space-y-3 pt-3">
                <p className="text-xs text-muted-foreground">Pick an existing table — its columns are detected automatically.</p>
                {isTrino && (
                  <div className="space-y-1.5">
                    <Label>Catalog</Label>
                    <Select value={catalog} onValueChange={(v) => { setCatalog(v); setSchema(""); setTable(""); }}>
                      <SelectTrigger className="w-full"><SelectValue placeholder="Choose a catalog" /></SelectTrigger>
                      <SelectContent>{(cats.data ?? []).map((c) => <SelectItem key={c} value={c}>{c}</SelectItem>)}</SelectContent>
                    </Select>
                  </div>
                )}
                <div className="space-y-1.5">
                  <Label>Schema</Label>
                  <Select value={schema} onValueChange={(v) => { setSchema(v); setTable(""); }} disabled={isTrino && !catalog}>
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
                <Button className="w-full" disabled={!table || create.isPending}
                  onClick={() => submit({ kind: "physical", catalog: isTrino ? catalog : null, schema, table })}>
                  {create.isPending ? "Creating…" : "Create"}
                </Button>
              </TabsContent>

              <TabsContent value="sql" className="space-y-3 pt-3">
                <p className="text-xs text-muted-foreground">Write a query — its result becomes the dataset. Columns are detected automatically.</p>
                <div className="space-y-1.5">
                  <Label htmlFor="nd-sql">SQL query</Label>
                  <Textarea id="nd-sql" className="h-40 font-mono text-sm" placeholder="SELECT …"
                    value={sql} onChange={(e) => setSql(e.target.value)} />
                </div>
                <Button className="w-full" disabled={!sql.trim() || create.isPending}
                  onClick={() => submit({ kind: "virtual", sql })}>
                  {create.isPending ? "Creating…" : "Create"}
                </Button>
              </TabsContent>
            </Tabs>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
