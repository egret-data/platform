import Editor, { type Monaco } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";
import { useEffect, useRef } from "react";

const KEYWORDS = [
  "SELECT","FROM","WHERE","GROUP BY","ORDER BY","HAVING","LIMIT","JOIN","LEFT JOIN","RIGHT JOIN",
  "INNER JOIN","ON","AS","AND","OR","NOT","IN","IS NULL","IS NOT NULL","DISTINCT","COUNT","SUM",
  "AVG","MIN","MAX","CASE","WHEN","THEN","ELSE","END","UNION","UNION ALL","WITH","INSERT","UPDATE","DELETE","CREATE","TABLE",
];

export function SqlEditor({ value, onChange, schemaWords }: { value: string; onChange: (v: string) => void; schemaWords: string[] }) {
  const wordsRef = useRef(schemaWords);
  wordsRef.current = schemaWords;
  const disposableRef = useRef<{ dispose: () => void } | null>(null);
  useEffect(() => () => disposableRef.current?.dispose(), []);

  function handleMount(_editor: unknown, monaco: Monaco) {
    disposableRef.current = monaco.languages.registerCompletionItemProvider("sql", {
      provideCompletionItems: (model: MonacoNS.editor.ITextModel, position: MonacoNS.Position) => {
        const word = model.getWordUntilPosition(position);
        const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
        const kw = KEYWORDS.map((k) => ({ label: k, kind: monaco.languages.CompletionItemKind.Keyword, insertText: k, range }));
        const objs = Array.from(new Set(wordsRef.current)).map((w) => ({ label: w, kind: monaco.languages.CompletionItemKind.Field, insertText: w, range }));
        return { suggestions: [...objs, ...kw] };
      },
    });
  }

  return <Editor height="100%" defaultLanguage="sql" value={value} onChange={(v) => onChange(v ?? "")} onMount={handleMount}
    options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }} />;
}
