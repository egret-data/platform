import type { Monaco } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";
import type { KernelConnection } from "@/lib/kernel/KernelConnection";

/** Set by NotebookPage on mount / cleared on unmount. Monaco completion
 *  providers are registered ONCE, globally, per language — this module-level
 *  context is how the (single) python/sql providers know which notebook's
 *  kernel + schema-browser word list to consult. When null (no notebook is
 *  mounted — e.g. SQL Lab), the providers fall back to statics/keywords only,
 *  so SQL Lab's own editor is never polluted by notebook state. */
export interface NotebookCompletionContext {
  kernel: KernelConnection;
  /** Loaded catalog object names (catalogs/schemas/tables/columns) for SQL. */
  sqlWords: () => string[];
}

let ctx: NotebookCompletionContext | null = null;

export function setNotebookCompletionContext(c: NotebookCompletionContext | null): void {
  ctx = c;
}

// Ported verbatim from the old registerStatic.ts (replaced by this module).
const PYTHON_ITEMS: { label: string; insert: string; detail: string }[] = [
  { label: "trino", insert: 'trino("${1:SELECT 1}")', detail: "egret · governed Trino → Daft DataFrame" },
  { label: "starrocks", insert: 'starrocks("${1:SELECT 1}")', detail: "egret · governed StarRocks → Daft DataFrame" },
  { label: "write_table", insert: 'write_table(${1:df}, "${2:namespace.table}")', detail: "egret · write a DataFrame back" },
  { label: "display", insert: "display(${1:df})", detail: "egret · render a Daft DataFrame" },
  { label: "runtime", insert: "runtime", detail: "egret · parsed notebook runtime manifest" },
  { label: "me", insert: "me", detail: "egret · your username" },
];

const SQL_KEYWORDS = [
  "SELECT", "FROM", "WHERE", "GROUP BY", "ORDER BY", "LIMIT", "JOIN",
  "LEFT JOIN", "INNER JOIN", "ON", "AS", "WITH", "DISTINCT", "COUNT",
  "SUM", "AVG", "MIN", "MAX", "HAVING", "UNION", "INSERT INTO", "VALUES",
];

let registered = false;

/** Register the notebook's python + sql completion providers once per page
 *  load. Safe to call from every editor's onMount — subsequent calls are
 *  no-ops. Python completions come from the connected kernel's Jupyter
 *  `complete_request` (real `df.<column>`/vars/imports) when a notebook
 *  context is set, falling back to the static helper snippets otherwise. SQL
 *  completions are keywords plus whatever object names the schema browser has
 *  reported (see `SchemaBrowserPanel`'s `onObjectsLoaded`), mirroring SQL
 *  Lab's own `SqlEditor` pattern — but empty (keywords only) when there's no
 *  notebook context, so SQL Lab's editor is unaffected. */
export function registerNotebookCompletions(monaco: Monaco): void {
  if (registered) return;
  registered = true;

  monaco.languages.registerCompletionItemProvider("python", {
    triggerCharacters: ["."],
    async provideCompletionItems(model: MonacoNS.editor.ITextModel, position: MonacoNS.Position) {
      const statics = PYTHON_ITEMS.map((it) => ({
        label: it.label,
        kind: monaco.languages.CompletionItemKind.Function,
        insertText: it.insert,
        insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
        detail: it.detail,
        range: undefined as never,
      }));
      if (!ctx) return { suggestions: statics };
      const offset = model.getOffsetAt(position);
      const res = await ctx.kernel.complete(model.getValue(), offset).catch(() => null);
      if (!res || res.matches.length === 0) return { suggestions: statics };
      const start = model.getPositionAt(res.cursorStart);
      const end = model.getPositionAt(res.cursorEnd);
      const range = {
        startLineNumber: start.lineNumber,
        startColumn: start.column,
        endLineNumber: end.lineNumber,
        endColumn: end.column,
      };
      const dynamic = res.matches.map((m) => ({
        label: m,
        kind: monaco.languages.CompletionItemKind.Variable,
        insertText: m,
        range,
      }));
      return { suggestions: [...dynamic, ...statics] };
    },
  });

  monaco.languages.registerCompletionItemProvider("sql", {
    provideCompletionItems(model: MonacoNS.editor.ITextModel, position: MonacoNS.Position) {
      // Fully silent outside a notebook. This provider is registered globally
      // and never disposed, so it also fires in SQL Lab (which has its own SQL
      // provider) — returning nothing there avoids duplicate keyword/word
      // suggestions. Only inside a notebook (ctx set) do we contribute.
      if (!ctx) return { suggestions: [] };
      const word = model.getWordUntilPosition(position);
      const range = {
        startLineNumber: position.lineNumber,
        endLineNumber: position.lineNumber,
        startColumn: word.startColumn,
        endColumn: word.endColumn,
      };
      const kw = SQL_KEYWORDS.map((k) => ({
        label: k,
        kind: monaco.languages.CompletionItemKind.Keyword,
        insertText: k,
        range,
      }));
      const objs = Array.from(new Set(ctx.sqlWords())).map((w) => ({
        label: w,
        kind: monaco.languages.CompletionItemKind.Field,
        insertText: w,
        range,
      }));
      return { suggestions: [...objs, ...kw] };
    },
  });
}
