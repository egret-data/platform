import Editor, { type Monaco } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";
import { useRef } from "react";

export interface SchemaHint { tables: string[]; columnsByTable: Record<string, string[]> }

export function SqlEditor({ value, onChange, hint }: { value: string; onChange: (v: string) => void; hint: SchemaHint }) {
  const hintRef = useRef(hint);
  hintRef.current = hint;

  function handleMount(_editor: unknown, monaco: Monaco) {
    monaco.languages.registerCompletionItemProvider("sql", {
      provideCompletionItems: (model: MonacoNS.editor.ITextModel, position: MonacoNS.Position) => {
        const word = model.getWordUntilPosition(position);
        const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
        const h = hintRef.current;
        const tableItems = h.tables.map((t) => ({ label: t, kind: monaco.languages.CompletionItemKind.Struct, insertText: t, range }));
        const colItems = Object.values(h.columnsByTable).flat().map((c) => ({ label: c, kind: monaco.languages.CompletionItemKind.Field, insertText: c, range }));
        return { suggestions: [...tableItems, ...colItems] };
      },
    });
  }

  return (
    <Editor height="100%" defaultLanguage="sql" value={value} onChange={(v) => onChange(v ?? "")} onMount={handleMount}
      options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }} />
  );
}
