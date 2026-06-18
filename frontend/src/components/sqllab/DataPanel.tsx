import { useEffect, useState } from "react";
import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useCatalogs, useColumns, useDatabases, useSchemas, useTables } from "@/api/sqllab";

export interface DataPanelProps {
  ws: string;
  dbId: string | null;
  onDb: (id: string) => void;
  onInsert: (name: string) => void;
  /** Report all loaded object names (catalogs/schemas/tables/columns) for autocomplete. */
  onWords: (words: string[]) => void;
}

export function DataPanel({ ws, dbId, onDb, onInsert, onWords }: DataPanelProps) {
  const { data: databases } = useDatabases(ws);
  const engine = databases?.find((d) => d.id === dbId)?.engine ?? null;
  const isTrino = engine === "trino";
  const [collapsed, setCollapsed] = useState(false);
  const [catalog, setCatalog] = useState("");
  const [schema, setSchema] = useState("");
  const [table, setTable] = useState("");

  const cats = useCatalogs(ws, isTrino ? dbId : null);
  const schemas = useSchemas(ws, dbId, isTrino ? catalog : "");
  const tables = useTables(ws, dbId, catalog, schema);
  const columns = useColumns(ws, dbId, catalog, schema, table, !!table);

  // Auto-select the only available database (improves UX + makes tests deterministic).
  useEffect(() => {
    if (!dbId && databases && databases.length === 1) onDb(databases[0].id);
  }, [databases, dbId, onDb]);

  const words = [
    ...(cats.data ?? []),
    ...(schemas.data ?? []),
    ...(tables.data ?? []),
    ...((columns.data ?? []).map((c) => c.name)),
  ];
  const wkey = words.join("|");
  useEffect(() => { onWords(words); }, [wkey]); // wkey is a stable string key of `words`

  if (collapsed) {
    return (
      <div className="border-r p-1">
        <Button variant="ghost" size="sm" onClick={() => setCollapsed(false)} title="Show data panel">
          <PanelLeftOpen className="h-4 w-4" />
        </Button>
      </div>
    );
  }

  const qualified = (t: string) => (catalog ? `${catalog}.${schema}.${t}` : `${schema}.${t}`);

  return (
    <div className="flex h-full w-full flex-col border-r">
      <div className="flex items-center justify-between border-b p-2">
        <span className="text-xs font-medium">Data</span>
        <Button variant="ghost" size="sm" onClick={() => setCollapsed(true)} title="Collapse">
          <PanelLeftClose className="h-4 w-4" />
        </Button>
      </div>
      <div className="space-y-2 p-2 text-sm">
        <Select value={dbId ?? ""} onValueChange={(v) => { onDb(v); setCatalog(""); setSchema(""); setTable(""); }}>
          <SelectTrigger><SelectValue placeholder="Database" /></SelectTrigger>
          <SelectContent>{(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}</SelectContent>
        </Select>
        {isTrino && (
          <Select value={catalog} onValueChange={(v) => { setCatalog(v); setSchema(""); setTable(""); }}>
            <SelectTrigger><SelectValue placeholder="Catalog" /></SelectTrigger>
            <SelectContent>{(cats.data ?? []).map((c) => <SelectItem key={c} value={c}>{c}</SelectItem>)}</SelectContent>
          </Select>
        )}
        <Select value={schema} onValueChange={(v) => { setSchema(v); setTable(""); }} disabled={isTrino && !catalog}>
          <SelectTrigger><SelectValue placeholder="Schema" /></SelectTrigger>
          <SelectContent>{(schemas.data ?? []).map((s) => <SelectItem key={s} value={s}>{s}</SelectItem>)}</SelectContent>
        </Select>
        <Select value={table} onValueChange={setTable} disabled={!schema}>
          <SelectTrigger><SelectValue placeholder="Table" /></SelectTrigger>
          <SelectContent>{(tables.data ?? []).map((t) => <SelectItem key={t} value={t}>{t}</SelectItem>)}</SelectContent>
        </Select>
        {table && (
          <button className="text-xs text-muted-foreground underline" onClick={() => onInsert(qualified(table))}>insert {table}</button>
        )}
      </div>
      <div className="flex-1 overflow-auto border-t p-2 text-sm">
        {table
          ? (columns.data ?? []).map((c) => (
              <button key={c.name} className="flex w-full justify-between rounded px-2 py-0.5 text-left hover:bg-accent" onClick={() => onInsert(c.name)}>
                <span className="truncate">{c.name}</span><span className="ml-2 text-muted-foreground">{c.type}</span>
              </button>
            ))
          : <span className="text-muted-foreground">Pick a table to see columns.</span>}
      </div>
    </div>
  );
}
