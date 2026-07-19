import { useState } from "react";
import { ChevronDown, ChevronRight, Columns3, Database, Loader2, Table } from "lucide-react";

import {
  type DatabaseListItem,
  useCatalogs,
  useColumns,
  useDatabases,
  useSchemas,
  useTables,
} from "@/api/sqllab";

export interface SchemaBrowserPanelProps {
  ws: string;
  /** Insert text at the active cell's cursor (column click). */
  onInsertText: (text: string) => void;
  /** Create a new SQL cell seeded with this query (table click). */
  onNewSqlCell: (sql: string) => void;
}

/** A collapsible tree row shared by every level of the browser. Each level
 *  fetches its own children lazily: the child-fetching subcomponent below a
 *  row is only mounted once that row is expanded, so react-query never
 *  issues a request for a collapsed node. */
function TreeRow({
  label,
  icon,
  depth,
  expanded,
  onToggle,
  onClick,
  title,
  trailing,
}: {
  label: string;
  icon: React.ReactNode;
  depth: number;
  expanded?: boolean;
  onToggle?: () => void;
  onClick?: () => void;
  title?: string;
  trailing?: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick ?? onToggle}
      className="flex w-full items-center gap-1 rounded px-1.5 py-1 text-left text-xs hover:bg-accent"
      style={{ paddingLeft: `${0.375 + depth * 0.9}rem` }}
      title={title ?? label}
    >
      {onToggle ? (
        <span
          role="button"
          tabIndex={-1}
          aria-label={expanded ? `Collapse ${label}` : `Expand ${label}`}
          onClick={(e) => {
            e.stopPropagation();
            onToggle();
          }}
          className="shrink-0 text-muted-foreground"
        >
          {expanded ? (
            <ChevronDown className="h-3 w-3" />
          ) : (
            <ChevronRight className="h-3 w-3" />
          )}
        </span>
      ) : (
        <span className="w-3 shrink-0" />
      )}
      {icon}
      <span className="truncate">{label}</span>
      {trailing}
    </button>
  );
}

function LoadingRow({ depth }: { depth: number }) {
  return (
    <div
      className="flex items-center gap-1.5 px-1.5 py-1 text-xs text-muted-foreground"
      style={{ paddingLeft: `${0.375 + depth * 0.9}rem` }}
    >
      <Loader2 className="h-3 w-3 animate-spin" />
      Loading…
    </div>
  );
}

function EmptyRow({ depth, label }: { depth: number; label: string }) {
  return (
    <div
      className="px-1.5 py-1 text-xs text-muted-foreground"
      style={{ paddingLeft: `${0.375 + depth * 0.9}rem` }}
    >
      {label}
    </div>
  );
}

// ── Columns (leaf level) ──────────────────────────────────────────────────
function ColumnList({
  ws,
  db,
  catalog,
  schema,
  table,
  depth,
  onInsertText,
}: {
  ws: string;
  db: string;
  catalog: string;
  schema: string;
  table: string;
  depth: number;
  onInsertText: (text: string) => void;
}) {
  const columns = useColumns(ws, db, catalog, schema, table, true);
  if (columns.isLoading) return <LoadingRow depth={depth} />;
  if (!columns.data || columns.data.length === 0)
    return <EmptyRow depth={depth} label="No columns." />;
  return (
    <>
      {columns.data.map((c) => (
        <button
          key={c.name}
          type="button"
          onClick={() => onInsertText(c.name)}
          title={`Insert ${c.name}`}
          className="flex w-full items-center justify-between gap-2 rounded px-1.5 py-1 text-left text-xs hover:bg-accent"
          style={{ paddingLeft: `${0.375 + depth * 0.9}rem` }}
        >
          <span className="flex min-w-0 items-center gap-1.5">
            <Columns3 className="h-3 w-3 shrink-0 text-muted-foreground" />
            <span className="truncate">{c.name}</span>
          </span>
          <span className="shrink-0 text-[10px] text-muted-foreground">{c.type}</span>
        </button>
      ))}
    </>
  );
}

// ── Tables ────────────────────────────────────────────────────────────────
function TableNode({
  ws,
  db,
  catalog,
  schema,
  table,
  depth,
  onInsertText,
  onNewSqlCell,
}: {
  ws: string;
  db: string;
  catalog: string;
  schema: string;
  table: string;
  depth: number;
  onInsertText: (text: string) => void;
  onNewSqlCell: (sql: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const qualified = catalog ? `${catalog}.${schema}.${table}` : `${schema}.${table}`;
  return (
    <div>
      <TreeRow
        label={table}
        icon={<Table className="h-3 w-3 shrink-0 text-muted-foreground" />}
        depth={depth}
        expanded={expanded}
        onToggle={() => setExpanded((v) => !v)}
        onClick={() => onNewSqlCell(`SELECT * FROM ${qualified} LIMIT 100`)}
        title={`${qualified} — click to query, chevron to browse columns`}
      />
      {expanded && (
        <ColumnList
          ws={ws}
          db={db}
          catalog={catalog}
          schema={schema}
          table={table}
          depth={depth + 1}
          onInsertText={onInsertText}
        />
      )}
    </div>
  );
}

function TableList({
  ws,
  db,
  catalog,
  schema,
  depth,
  onInsertText,
  onNewSqlCell,
}: {
  ws: string;
  db: string;
  catalog: string;
  schema: string;
  depth: number;
  onInsertText: (text: string) => void;
  onNewSqlCell: (sql: string) => void;
}) {
  const tables = useTables(ws, db, catalog, schema);
  if (tables.isLoading) return <LoadingRow depth={depth} />;
  if (!tables.data || tables.data.length === 0)
    return <EmptyRow depth={depth} label="No tables." />;
  return (
    <>
      {tables.data.map((t) => (
        <TableNode
          key={t}
          ws={ws}
          db={db}
          catalog={catalog}
          schema={schema}
          table={t}
          depth={depth}
          onInsertText={onInsertText}
          onNewSqlCell={onNewSqlCell}
        />
      ))}
    </>
  );
}

// ── Schemas ───────────────────────────────────────────────────────────────
function SchemaNode({
  ws,
  db,
  catalog,
  schema,
  depth,
  onInsertText,
  onNewSqlCell,
}: {
  ws: string;
  db: string;
  catalog: string;
  schema: string;
  depth: number;
  onInsertText: (text: string) => void;
  onNewSqlCell: (sql: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div>
      <TreeRow
        label={schema}
        icon={<span className="w-3 shrink-0" />}
        depth={depth}
        expanded={expanded}
        onToggle={() => setExpanded((v) => !v)}
      />
      {expanded && (
        <TableList
          ws={ws}
          db={db}
          catalog={catalog}
          schema={schema}
          depth={depth + 1}
          onInsertText={onInsertText}
          onNewSqlCell={onNewSqlCell}
        />
      )}
    </div>
  );
}

function SchemaList({
  ws,
  db,
  catalog,
  depth,
  onInsertText,
  onNewSqlCell,
}: {
  ws: string;
  db: string;
  catalog: string;
  depth: number;
  onInsertText: (text: string) => void;
  onNewSqlCell: (sql: string) => void;
}) {
  const schemas = useSchemas(ws, db, catalog);
  if (schemas.isLoading) return <LoadingRow depth={depth} />;
  if (!schemas.data || schemas.data.length === 0)
    return <EmptyRow depth={depth} label="No schemas." />;
  return (
    <>
      {schemas.data.map((s) => (
        <SchemaNode
          key={s}
          ws={ws}
          db={db}
          catalog={catalog}
          schema={s}
          depth={depth}
          onInsertText={onInsertText}
          onNewSqlCell={onNewSqlCell}
        />
      ))}
    </>
  );
}

// ── Catalogs (Trino only — other engines skip straight to schemas) ────────
function CatalogNode({
  ws,
  db,
  catalog,
  depth,
  onInsertText,
  onNewSqlCell,
}: {
  ws: string;
  db: string;
  catalog: string;
  depth: number;
  onInsertText: (text: string) => void;
  onNewSqlCell: (sql: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div>
      <TreeRow
        label={catalog}
        icon={<span className="w-3 shrink-0" />}
        depth={depth}
        expanded={expanded}
        onToggle={() => setExpanded((v) => !v)}
      />
      {expanded && (
        <SchemaList
          ws={ws}
          db={db}
          catalog={catalog}
          depth={depth + 1}
          onInsertText={onInsertText}
          onNewSqlCell={onNewSqlCell}
        />
      )}
    </div>
  );
}

function CatalogList({
  ws,
  db,
  depth,
  onInsertText,
  onNewSqlCell,
}: {
  ws: string;
  db: string;
  depth: number;
  onInsertText: (text: string) => void;
  onNewSqlCell: (sql: string) => void;
}) {
  const catalogs = useCatalogs(ws, db);
  if (catalogs.isLoading) return <LoadingRow depth={depth} />;
  if (!catalogs.data || catalogs.data.length === 0)
    return <EmptyRow depth={depth} label="No catalogs." />;
  return (
    <>
      {catalogs.data.map((c) => (
        <CatalogNode
          key={c}
          ws={ws}
          db={db}
          catalog={c}
          depth={depth}
          onInsertText={onInsertText}
          onNewSqlCell={onNewSqlCell}
        />
      ))}
    </>
  );
}

// ── Databases (root level) ────────────────────────────────────────────────
function DatabaseNode({
  ws,
  db,
  depth,
  onInsertText,
  onNewSqlCell,
}: {
  ws: string;
  db: DatabaseListItem;
  depth: number;
  onInsertText: (text: string) => void;
  onNewSqlCell: (sql: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const isTrino = db.engine === "trino";
  return (
    <div>
      <TreeRow
        label={db.name}
        icon={<Database className="h-3 w-3 shrink-0 text-muted-foreground" />}
        depth={depth}
        expanded={expanded}
        onToggle={() => setExpanded((v) => !v)}
        title={`${db.name} (${db.engine})`}
        trailing={
          <span className="ml-auto shrink-0 text-[10px] uppercase text-muted-foreground">
            {db.engine}
          </span>
        }
      />
      {expanded &&
        (isTrino ? (
          <CatalogList
            ws={ws}
            db={db.id}
            depth={depth + 1}
            onInsertText={onInsertText}
            onNewSqlCell={onNewSqlCell}
          />
        ) : (
          <SchemaList
            ws={ws}
            db={db.id}
            catalog=""
            depth={depth + 1}
            onInsertText={onInsertText}
            onNewSqlCell={onNewSqlCell}
          />
        ))}
    </div>
  );
}

export function SchemaBrowserPanel({ ws, onInsertText, onNewSqlCell }: SchemaBrowserPanelProps) {
  const databases = useDatabases(ws);

  if (databases.isLoading) {
    return (
      <div className="flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
        Loading databases…
      </div>
    );
  }

  if (!databases.data || databases.data.length === 0) {
    return (
      <p className="px-3 py-2 text-xs text-muted-foreground">No databases granted.</p>
    );
  }

  return (
    <div className="flex flex-col gap-0.5 py-1">
      {databases.data.map((db) => (
        <DatabaseNode
          key={db.id}
          ws={ws}
          db={db}
          depth={0}
          onInsertText={onInsertText}
          onNewSqlCell={onNewSqlCell}
        />
      ))}
    </div>
  );
}
