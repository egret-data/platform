/** Structured schema the Explorer accumulates as its tree loads, consumed by the
 *  Monaco completion provider for context-aware autocomplete (dotted-path,
 *  FROM→tables, columns-of-referenced-tables). Keys in `columnsByTable` are
 *  lowercased and stored under several forms (bare table, schema.table, fqn) so
 *  a reference in any of those shapes resolves. */
export interface SchemaColumn {
  name: string;
  type: string;
}

export interface SchemaIndex {
  /** Every known identifier (catalogs/schemas/tables/fqns/columns) — flat fallback. */
  identifiers: string[];
  /** Table names + fully-qualified names, for FROM/JOIN completion. */
  tables: string[];
  /** Columns keyed by lowercased table reference (bare / schema.table / fqn). */
  columnsByTable: Record<string, SchemaColumn[]>;
}

export const emptySchemaIndex = (): SchemaIndex => ({ identifiers: [], tables: [], columnsByTable: {} });

/** Mutation sink threaded through the Explorer tree; each level reports what it
 *  loaded. Implementations debounce/flush to `onWords` + `onSchema`. */
export interface SchemaSink {
  names: (names: string[]) => void;
  tables: (tables: string[]) => void;
  columns: (keys: string[], cols: SchemaColumn[]) => void;
  /** Open a data preview for a table (fully-qualified name). */
  preview: (fqn: string) => void;
}
