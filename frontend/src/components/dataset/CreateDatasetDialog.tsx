import { useState, useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
  const cats = useCatalogs(ws, isTrino ? (dbId || null) : null);
  const schemas = useSchemas(ws, dbId || null, isTrino ? catalog : "");
  const tables = useTables(ws, dbId || null, catalog, schema);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
      <DialogContent>
        <DialogHeader><DialogTitle>New dataset</DialogTitle></DialogHeader>
        <Tabs value={tab} onValueChange={(v) => setTab(v as "table" | "sql")}>
          <TabsList><TabsTrigger value="table">From table</TabsTrigger><TabsTrigger value="sql">From SQL</TabsTrigger></TabsList>

          <div className="space-y-2 py-2">
            <Select value={dbId} onValueChange={(v) => { setDbId(v); setCatalog(""); setSchema(""); setTable(""); }}>
              <SelectTrigger className="w-full"><SelectValue placeholder="Database" /></SelectTrigger>
              <SelectContent>{(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}</SelectContent>
            </Select>
            <Input placeholder="Dataset name" value={name} onChange={(e) => setName(e.target.value)} />
            <Input placeholder="Description (optional)" value={description} onChange={(e) => setDescription(e.target.value)} />
          </div>

          <TabsContent value="table" className="space-y-2">
            {isTrino && (
              <Select value={catalog} onValueChange={(v) => { setCatalog(v); setSchema(""); setTable(""); }}>
                <SelectTrigger className="w-full"><SelectValue placeholder="Catalog" /></SelectTrigger>
                <SelectContent>{(cats.data ?? []).map((c) => <SelectItem key={c} value={c}>{c}</SelectItem>)}</SelectContent>
              </Select>
            )}
            <Select value={schema} onValueChange={(v) => { setSchema(v); setTable(""); }} disabled={isTrino && !catalog}>
              <SelectTrigger className="w-full"><SelectValue placeholder="Schema" /></SelectTrigger>
              <SelectContent>{(schemas.data ?? []).map((s) => <SelectItem key={s} value={s}>{s}</SelectItem>)}</SelectContent>
            </Select>
            <Select value={table} onValueChange={setTable} disabled={!schema}>
              <SelectTrigger className="w-full"><SelectValue placeholder="Table" /></SelectTrigger>
              <SelectContent>{(tables.data ?? []).map((t) => <SelectItem key={t} value={t}>{t}</SelectItem>)}</SelectContent>
            </Select>
            <Button disabled={!table || create.isPending}
              onClick={() => submit({ kind: "physical", catalog: isTrino ? catalog : null, schema, table })}>Create</Button>
          </TabsContent>

          <TabsContent value="sql" className="space-y-2">
            <Textarea className="h-40 font-mono text-sm" placeholder="SELECT …"
              value={sql} onChange={(e) => setSql(e.target.value)} />
            <Button disabled={!sql.trim() || create.isPending}
              onClick={() => submit({ kind: "virtual", sql })}>Create</Button>
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}
