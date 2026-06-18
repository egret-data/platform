import { useState } from "react";
import { ChevronRight, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useCatalogs, useSchemas, useTables, useRefreshSchema } from "@/api/sqllab";

export function SchemaTree({ ws, db, engine, onInsert }: { ws: string; db: string | null; engine: string | null; onInsert: (name: string) => void }) {
  const refresh = useRefreshSchema(ws);
  if (!db) return <div className="p-3 text-sm text-muted-foreground">Pick a database.</div>;
  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b p-2">
        <span className="text-xs font-medium">Schema</span>
        <Button variant="ghost" size="sm" onClick={() => refresh(db)} title="Refresh schema"><RefreshCw className="h-3 w-3" /></Button>
      </div>
      <div className="flex-1 overflow-auto p-1 text-sm">
        {engine === "trino" ? <TrinoCatalogs ws={ws} db={db} onInsert={onInsert} /> : <Schemas ws={ws} db={db} catalog="" onInsert={onInsert} />}
      </div>
    </div>
  );
}

function TrinoCatalogs({ ws, db, onInsert }: { ws: string; db: string; onInsert: (n: string) => void }) {
  const { data: cats } = useCatalogs(ws, db);
  return <>{(cats ?? []).map((c) => <Node key={c} label={c}><Schemas ws={ws} db={db} catalog={c} onInsert={onInsert} /></Node>)}</>;
}
function Schemas({ ws, db, catalog, onInsert }: { ws: string; db: string; catalog: string; onInsert: (n: string) => void }) {
  const { data: schemas } = useSchemas(ws, db, catalog);
  return <>{(schemas ?? []).map((s) => <Node key={s} label={s}><Tables ws={ws} db={db} catalog={catalog} schema={s} onInsert={onInsert} /></Node>)}</>;
}
function Tables({ ws, db, catalog, schema, onInsert }: { ws: string; db: string; catalog: string; schema: string; onInsert: (n: string) => void }) {
  const { data: tables } = useTables(ws, db, catalog, schema);
  return <>{(tables ?? []).map((t) => (
    <button key={t} className="block w-full truncate rounded px-2 py-0.5 text-left hover:bg-accent" onClick={() => onInsert(catalog ? `${catalog}.${schema}.${t}` : `${schema}.${t}`)}>{t}</button>
  ))}</>;
}
function Node({ label, children }: { label: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button className="flex w-full items-center gap-1 rounded px-1 py-0.5 hover:bg-accent" onClick={() => setOpen((o) => !o)}>
        <ChevronRight className={`h-3 w-3 transition-transform ${open ? "rotate-90" : ""}`} /> {label}
      </button>
      {open && <div className="ml-4">{children}</div>}
    </div>
  );
}
