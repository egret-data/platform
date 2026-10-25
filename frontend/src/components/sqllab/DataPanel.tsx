import { useEffect, useRef, useState } from "react";
import {
  AlertTriangle, ChevronDown, ChevronRight, Columns3, Database, Eye, Loader2, PanelLeftClose, Search, Table as TableIcon,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { ApiError } from "@/api/client";
import { fqnOf, useCatalogs, useColumns, useDatabases, useSchemas, useSearchObjects, useTables } from "@/api/sqllab";
import { emptySchemaIndex, type SchemaIndex, type SchemaSink } from "./schemaIndex";

export interface DataPanelProps {
  ws: string;
  dbId: string | null;
  onDb: (id: string) => void;
  onInsert: (name: string) => void;
  /** Report all loaded object names (catalogs/schemas/tables/columns) for autocomplete. */
  onWords: (words: string[]) => void;
  /** Structured schema for context-aware autocomplete (accumulated as the tree loads). */
  onSchema?: (index: SchemaIndex) => void;
  /** Preview a table's data (fully-qualified name) — opens a new query tab. */
  onPreview?: (fqn: string) => void;
  /** Called when the user clicks the collapse button. */
  onCollapse: () => void;
}

const matches = (label: string, filter: string) =>
  !filter || label.toLowerCase().includes(filter.toLowerCase());

// ── shared tree row ────────────────────────────────────────────────────────
function TreeRow({ label, icon, depth, expanded, onToggle, onClick, title, trailing, action }: {
  label: string; icon?: React.ReactNode; depth: number; expanded?: boolean;
  onToggle?: () => void; onClick?: () => void; title?: string; trailing?: React.ReactNode; action?: React.ReactNode;
}) {
  return (
    <div
      className="group/row flex items-center rounded-md text-xs hover:bg-accent"
      style={{ paddingLeft: `${0.25 + depth * 0.75}rem` }}
    >
      {onToggle ? (
        <button
          type="button"
          aria-label={expanded ? `Collapse ${label}` : `Expand ${label}`}
          onClick={onToggle}
          className="flex h-6 w-5 shrink-0 items-center justify-center text-muted-foreground hover:text-foreground"
        >
          {expanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
        </button>
      ) : (
        <span className="w-5 shrink-0" />
      )}
      <button
        type="button"
        onClick={onClick ?? onToggle}
        title={title ?? label}
        className="flex min-w-0 flex-1 items-center gap-1.5 py-1 pr-1 text-left"
      >
        {icon}
        <span className="truncate">{label}</span>
        {trailing}
      </button>
      {action}
    </div>
  );
}

function InfoRow({ depth, children, spinning, error }: { depth: number; children: React.ReactNode; spinning?: boolean; error?: boolean }) {
  return (
    <div
      className={`flex items-center gap-1.5 py-1 text-xs ${error ? "text-destructive" : "text-muted-foreground"}`}
      style={{ paddingLeft: `${0.75 + depth * 0.75}rem` }}
    >
      {spinning && <Loader2 className="h-3 w-3 animate-spin" />}
      {error && <AlertTriangle className="h-3 w-3 shrink-0" />}
      {children}
    </div>
  );
}

/** Loading / error / empty state shared by every tree level. Distinguishing an
 *  error (e.g. the catalog-browse endpoint returning 403/502) from a genuinely
 *  empty level avoids the misleading "No catalogs." on an access failure. */
function levelState(q: { isLoading: boolean; isError: boolean }, depth: number, emptyLabel: string, hasRows: boolean) {
  if (q.isLoading) return <InfoRow depth={depth} spinning>Loading…</InfoRow>;
  if (q.isError) return <InfoRow depth={depth} error>Couldn’t load — no access or engine error.</InfoRow>;
  if (!hasRows) return <InfoRow depth={depth}>{emptyLabel}</InfoRow>;
  return null;
}

// ── columns (leaf) ─────────────────────────────────────────────────────────
function ColumnList({ ws, db, catalog, schema, table, depth, filter, onInsert, sink }: {
  ws: string; db: string; catalog: string; schema: string; table: string; depth: number;
  filter: string; onInsert: (t: string) => void; sink: SchemaSink;
}) {
  const columns = useColumns(ws, db, catalog, schema, table, true);
  useEffect(() => {
    if (columns.data?.length) {
      sink.names(columns.data.map((c) => c.name));
      const fqn = catalog ? `${catalog}.${schema}.${table}` : `${schema}.${table}`;
      sink.columns([table, `${schema}.${table}`, fqn], columns.data.map((c) => ({ name: c.name, type: c.type })));
    }
  }, [columns.data, catalog, schema, table, sink]);
  const state = levelState(columns, depth, "No columns.", !!columns.data?.length);
  if (state) return state;
  const rows = columns.data!.filter((c) => matches(c.name, filter));
  if (!rows.length) return <InfoRow depth={depth}>No match.</InfoRow>;
  return (
    <>
      {rows.map((c) => (
        <TreeRow
          key={c.name}
          depth={depth}
          label={c.name}
          icon={<Columns3 className="h-3 w-3 shrink-0 text-muted-foreground" />}
          onClick={() => onInsert(c.name)}
          title={`Insert ${c.name} (${c.type})`}
          trailing={<span className="ml-auto shrink-0 pl-2 font-mono text-[10px] text-muted-foreground">{c.type}</span>}
        />
      ))}
    </>
  );
}

// ── tables ─────────────────────────────────────────────────────────────────
function TableNode({ ws, db, catalog, schema, table, depth, filter, onInsert, sink }: {
  ws: string; db: string; catalog: string; schema: string; table: string; depth: number;
  filter: string; onInsert: (t: string) => void; sink: SchemaSink;
}) {
  const [expanded, setExpanded] = useState(false);
  const qualified = catalog ? `${catalog}.${schema}.${table}` : `${schema}.${table}`;
  return (
    <div>
      <TreeRow
        depth={depth}
        label={table}
        icon={<TableIcon className="h-3 w-3 shrink-0 text-muted-foreground" />}
        expanded={expanded}
        onToggle={() => setExpanded((v) => !v)}
        onClick={() => onInsert(qualified)}
        title={`${qualified} — click to insert, chevron to browse columns`}
        action={
          <button type="button" aria-label={`Preview ${table}`} title="Preview data"
            onClick={(e) => { e.stopPropagation(); sink.preview(qualified); }}
            className="mr-1 flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 hover:bg-background hover:text-foreground focus-visible:opacity-100 group-hover/row:opacity-100">
            <Eye className="h-3.5 w-3.5" />
          </button>
        }
      />
      {expanded && (
        <ColumnList ws={ws} db={db} catalog={catalog} schema={schema} table={table} depth={depth + 1}
          filter={filter} onInsert={onInsert} sink={sink} />
      )}
    </div>
  );
}

function TableList({ ws, db, catalog, schema, depth, filter, onInsert, sink }: {
  ws: string; db: string; catalog: string; schema: string; depth: number;
  filter: string; onInsert: (t: string) => void; sink: SchemaSink;
}) {
  const tables = useTables(ws, db, catalog, schema);
  useEffect(() => {
    if (tables.data?.length) {
      const fqns = tables.data.map((t) => (catalog ? `${catalog}.${schema}.${t}` : `${schema}.${t}`));
      sink.names([...tables.data, ...fqns]);
      sink.tables([...tables.data, ...fqns]);
    }
  }, [tables.data, catalog, schema, sink]);
  const state = levelState(tables, depth, "No tables.", !!tables.data?.length);
  if (state) return state;
  const rows = tables.data!.filter((t) => matches(t, filter));
  if (!rows.length) return <InfoRow depth={depth}>No match.</InfoRow>;
  return (
    <>
      {rows.map((t) => (
        <TableNode key={t} ws={ws} db={db} catalog={catalog} schema={schema} table={t} depth={depth}
          filter={filter} onInsert={onInsert} sink={sink} />
      ))}
    </>
  );
}

// ── schemas ────────────────────────────────────────────────────────────────
function SchemaNode({ ws, db, catalog, schema, depth, filter, onInsert, sink }: {
  ws: string; db: string; catalog: string; schema: string; depth: number;
  filter: string; onInsert: (t: string) => void; sink: SchemaSink;
}) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div>
      <TreeRow depth={depth} label={schema} expanded={expanded} onToggle={() => setExpanded((v) => !v)} />
      {expanded && (
        <TableList ws={ws} db={db} catalog={catalog} schema={schema} depth={depth + 1}
          filter={filter} onInsert={onInsert} sink={sink} />
      )}
    </div>
  );
}

function SchemaList({ ws, db, catalog, depth, filter, onInsert, sink }: {
  ws: string; db: string; catalog: string; depth: number;
  filter: string; onInsert: (t: string) => void; sink: SchemaSink;
}) {
  const schemas = useSchemas(ws, db, catalog);
  useEffect(() => {
    if (schemas.data?.length) sink.names(schemas.data);
  }, [schemas.data, sink]);
  const state = levelState(schemas, depth, "No schemas.", !!schemas.data?.length);
  if (state) return state;
  const rows = schemas.data!.filter((s) => matches(s, filter));
  if (!rows.length) return <InfoRow depth={depth}>No match.</InfoRow>;
  return (
    <>
      {rows.map((s) => (
        <SchemaNode key={s} ws={ws} db={db} catalog={catalog} schema={s} depth={depth}
          filter={filter} onInsert={onInsert} sink={sink} />
      ))}
    </>
  );
}

// ── catalogs (Trino only) ──────────────────────────────────────────────────
function CatalogNode({ ws, db, catalog, depth, filter, onInsert, sink }: {
  ws: string; db: string; catalog: string; depth: number;
  filter: string; onInsert: (t: string) => void; sink: SchemaSink;
}) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div>
      <TreeRow depth={depth} label={catalog} expanded={expanded} onToggle={() => setExpanded((v) => !v)} />
      {expanded && (
        <SchemaList ws={ws} db={db} catalog={catalog} depth={depth + 1}
          filter={filter} onInsert={onInsert} sink={sink} />
      )}
    </div>
  );
}

function CatalogList({ ws, db, depth, filter, onInsert, sink }: {
  ws: string; db: string; depth: number;
  filter: string; onInsert: (t: string) => void; sink: SchemaSink;
}) {
  const catalogs = useCatalogs(ws, db);
  useEffect(() => {
    if (catalogs.data?.length) sink.names(catalogs.data);
  }, [catalogs.data, sink]);
  const state = levelState(catalogs, depth, "No catalogs.", !!catalogs.data?.length);
  if (state) return state;
  const rows = catalogs.data!.filter((c) => matches(c, filter));
  if (!rows.length) return <InfoRow depth={depth}>No match.</InfoRow>;
  return (
    <>
      {rows.map((c) => (
        <CatalogNode key={c} ws={ws} db={db} catalog={c} depth={depth}
          filter={filter} onInsert={onInsert} sink={sink} />
      ))}
    </>
  );
}

export function DataPanel({ ws, dbId, onDb, onInsert, onWords, onSchema, onPreview, onCollapse }: DataPanelProps) {
  const { data: databases, isError: dbsError, error: dbsErr } = useDatabases(ws);
  // `databases` is undefined until the list loads. Don't pick the browse branch
  // until it settles: guessing non-Trino for a Trino db fires an invalid
  // `schemas?catalog=` (empty) request that the engine 502s.
  const dbsLoading = databases == null;
  const dbMeta = dbId ? (databases?.find((d) => d.id === dbId) ?? null) : null;
  const engine = dbMeta?.engine ?? null;
  const isTrino = engine === "trino";
  const [filter, setFilter] = useState("");
  // Debounced global object search: at >=2 chars the Explorer swaps the tree for
  // a flat, cross-schema search result list (one bounded engine query).
  const [debounced, setDebounced] = useState("");
  useEffect(() => {
    const t = setTimeout(() => setDebounced(filter), 250);
    return () => clearTimeout(t);
  }, [filter]);
  const searchMode = debounced.trim().length >= 2;
  const search = useSearchObjects(ws, dbId, debounced);

  // Auto-select the only available database (improves UX + makes tests deterministic).
  useEffect(() => {
    if (!dbId && databases && databases.length === 1) onDb(databases[0].id);
  }, [databases, dbId, onDb]);

  // Accumulate the schema the tree has loaded so far, feeding both the flat
  // autocomplete word list and the structured index. Ref-backed so the `sink`
  // is stable (tree effects depend on it) and callbacks stay fresh.
  const idx = useRef<SchemaIndex>(emptySchemaIndex());
  const idSet = useRef<Set<string>>(new Set());
  const tableSet = useRef<Set<string>>(new Set());
  const cbRef = useRef({ onWords, onSchema, onPreview });
  cbRef.current = { onWords, onSchema, onPreview };
  const flush = useRef<() => void>(() => {});
  flush.current = () => {
    cbRef.current.onWords(idx.current.identifiers);
    cbRef.current.onSchema?.(idx.current);
  };
  const sink = useRef<SchemaSink>({
    names: (names) => {
      let changed = false;
      for (const n of names) if (!idSet.current.has(n)) { idSet.current.add(n); changed = true; }
      if (changed) { idx.current = { ...idx.current, identifiers: [...idSet.current] }; flush.current(); }
    },
    tables: (tables) => {
      let changed = false;
      for (const t of tables) if (!tableSet.current.has(t)) { tableSet.current.add(t); changed = true; }
      if (changed) { idx.current = { ...idx.current, tables: [...tableSet.current] }; flush.current(); }
    },
    columns: (keys, cols) => {
      const map = { ...idx.current.columnsByTable };
      for (const k of keys) if (k) map[k.toLowerCase()] = cols;
      idx.current = { ...idx.current, columnsByTable: map };
      flush.current();
    },
    preview: (fqn) => cbRef.current.onPreview?.(fqn),
  }).current;

  // Reset the accumulator when the database changes.
  useEffect(() => {
    idSet.current = new Set(); tableSet.current = new Set();
    idx.current = emptySchemaIndex();
    flush.current();
  }, [dbId]);

  return (
    <div className="flex h-full w-full flex-col border-r bg-muted/20">
      <div className="flex h-9 items-center justify-between border-b px-2">
        <span className="flex items-center gap-1.5 text-xs font-semibold text-muted-foreground">
          <Database className="h-3.5 w-3.5" /> Explorer
        </span>
        <Button variant="ghost" size="sm" className="h-6 w-6 p-0" onClick={onCollapse} aria-label="Collapse explorer" title="Collapse">
          <PanelLeftClose className="h-4 w-4" />
        </Button>
      </div>

      <div className="space-y-2 border-b p-2">
        <Select value={dbId ?? ""} onValueChange={(v) => { onDb(v); setFilter(""); }}>
          <SelectTrigger className="h-8 w-full text-xs"><SelectValue placeholder="Select a connection" /></SelectTrigger>
          <SelectContent>{(databases ?? []).map((d) => (
            <SelectItem key={d.id} value={d.id}>
              <span className="inline-flex items-center gap-1.5">
                {d.name} ({d.engine})
                {d.engine_id ? (
                  <Badge variant="secondary" className="h-4 px-1 text-[10px]" title="Provisioned by egret">egret</Badge>
                ) : null}
              </span>
            </SelectItem>
          ))}</SelectContent>
        </Select>
        <div className="relative">
          <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Search tables…"
            aria-label="Filter database objects"
            disabled={!dbId}
            className="h-8 w-full rounded-md border bg-background pl-7 pr-2 text-xs outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
          />
        </div>
      </div>

      <div className="flex-1 overflow-auto p-1">
        {dbsError ? (
          <div role="alert" className="flex h-full flex-col items-center justify-center gap-1 px-4 text-center text-xs text-destructive">
            <AlertTriangle className="h-5 w-5" />
            {(dbsErr as ApiError | null)?.status === 403
              ? "You can't list this workspace's connections. Ask a workspace admin for a role that can run queries."
              : `Couldn't load this workspace's connections: ${dbsErr?.message || "request failed"}.`}
          </div>
        ) : databases?.length === 0 ? (
          <div role="status" className="flex h-full flex-col items-center justify-center gap-1 px-4 text-center text-xs text-muted-foreground">
            <Database className="h-5 w-5 opacity-40" />
            No connection is granted to this workspace. Ask a workspace admin.
          </div>
        ) : !dbId ? (
          <div className="flex h-full flex-col items-center justify-center gap-1 px-4 text-center text-xs text-muted-foreground">
            <Database className="h-5 w-5 opacity-40" />
            Select a connection to browse its schema.
          </div>
        ) : dbsLoading ? (
          <InfoRow depth={0} spinning>Loading…</InfoRow>
        ) : !dbMeta ? (
          <InfoRow depth={0} error>This database is no longer available.</InfoRow>
        ) : searchMode ? (
          search.isLoading ? (
            <InfoRow depth={0} spinning>Searching…</InfoRow>
          ) : search.isError ? (
            <InfoRow depth={0} error>Search failed — check access or engine.</InfoRow>
          ) : !search.data?.length ? (
            <InfoRow depth={0}>No objects match “{debounced.trim()}”.</InfoRow>
          ) : (
            search.data.map((h) => {
              const fqn = fqnOf(h);
              return (
                <TreeRow
                  key={fqn}
                  depth={0}
                  label={h.table}
                  icon={<TableIcon className="h-3 w-3 shrink-0 text-muted-foreground" />}
                  onClick={() => onInsert(fqn)}
                  title={`${fqn} — click to insert`}
                  trailing={<span className="ml-auto shrink-0 truncate pl-2 text-[10px] text-muted-foreground">{h.catalog ? `${h.catalog}.${h.schema}` : h.schema}</span>}
                  action={
                    <button type="button" aria-label={`Preview ${h.table}`} title="Preview data"
                      onClick={(e) => { e.stopPropagation(); sink.preview(fqn); }}
                      className="mr-1 flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 hover:bg-background hover:text-foreground focus-visible:opacity-100 group-hover/row:opacity-100">
                      <Eye className="h-3.5 w-3.5" />
                    </button>
                  }
                />
              );
            })
          )
        ) : isTrino ? (
          <CatalogList ws={ws} db={dbId} depth={0} filter={filter} onInsert={onInsert} sink={sink} />
        ) : (
          <SchemaList ws={ws} db={dbId} catalog="" depth={0} filter={filter} onInsert={onInsert} sink={sink} />
        )}
      </div>
    </div>
  );
}
