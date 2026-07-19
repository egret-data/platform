import type { Monaco } from "@monaco-editor/react";

let registered = false;

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

/** Register python + sql static completions once per page load. Safe to call
 *  from every editor's onMount — subsequent calls are no-ops. */
export function registerStaticCompletions(monaco: Monaco): void {
  if (registered) return;
  registered = true;

  monaco.languages.registerCompletionItemProvider("python", {
    provideCompletionItems() {
      return {
        suggestions: PYTHON_ITEMS.map((it) => ({
          label: it.label,
          kind: monaco.languages.CompletionItemKind.Function,
          insertText: it.insert,
          insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
          detail: it.detail,
          range: undefined as never,
        })),
      };
    },
  });

  monaco.languages.registerCompletionItemProvider("sql", {
    provideCompletionItems() {
      return {
        suggestions: SQL_KEYWORDS.map((kw) => ({
          label: kw,
          kind: monaco.languages.CompletionItemKind.Keyword,
          insertText: kw,
          range: undefined as never,
        })),
      };
    },
  });
}
