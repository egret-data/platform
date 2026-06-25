import Editor, { type Monaco } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";
import { useEffect, useRef } from "react";
import { pickRunSql } from "./pickRunSql";

const KEYWORDS = [
  "SELECT","FROM","WHERE","GROUP BY","ORDER BY","HAVING","LIMIT","JOIN","LEFT JOIN","RIGHT JOIN",
  "INNER JOIN","ON","AS","AND","OR","NOT","IN","IS NULL","IS NOT NULL","DISTINCT","COUNT","SUM",
  "AVG","MIN","MAX","CASE","WHEN","THEN","ELSE","END","UNION","UNION ALL","WITH","INSERT","UPDATE","DELETE","CREATE","TABLE",
];

export function SqlEditor({ value, onChange, schemaWords, onRun, onSelectionChange }: {
  value: string;
  onChange: (v: string) => void;
  schemaWords: string[];
  onRun?: (sql: string) => void;
  onSelectionChange?: (selectedText: string) => void;
}) {
  const wordsRef = useRef(schemaWords);
  wordsRef.current = schemaWords;
  const onRunRef = useRef(onRun);
  onRunRef.current = onRun;
  const onSelRef = useRef(onSelectionChange);
  onSelRef.current = onSelectionChange;
  const disposables = useRef<{ dispose: () => void }[]>([]);
  useEffect(() => () => { disposables.current.forEach((d) => d.dispose()); disposables.current = []; }, []);

  function selectedText(editor: MonacoNS.editor.IStandaloneCodeEditor): string {
    const sel = editor.getSelection();
    return sel && !sel.isEmpty() ? (editor.getModel()?.getValueInRange(sel) ?? "") : "";
  }

  function handleMount(editor: MonacoNS.editor.IStandaloneCodeEditor, monaco: Monaco) {
    disposables.current.push(
      monaco.languages.registerCompletionItemProvider("sql", {
        provideCompletionItems: (model: MonacoNS.editor.ITextModel, position: MonacoNS.Position) => {
          const word = model.getWordUntilPosition(position);
          const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
          const kw = KEYWORDS.map((k) => ({ label: k, kind: monaco.languages.CompletionItemKind.Keyword, insertText: k, range }));
          const objs = Array.from(new Set(wordsRef.current)).map((w) => ({ label: w, kind: monaco.languages.CompletionItemKind.Field, insertText: w, range }));
          return { suggestions: [...objs, ...kw] };
        },
      }),
    );
    // Cmd/Ctrl+Enter → run the selection (if any) else the whole editor.
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
      onRunRef.current?.(pickRunSql(editor.getValue(), selectedText(editor)));
    });
    // Report the current selection text so the toolbar can show "Run selection".
    disposables.current.push(
      editor.onDidChangeCursorSelection(() => onSelRef.current?.(selectedText(editor))),
    );
  }

  return <Editor height="100%" defaultLanguage="sql" value={value} onChange={(v) => onChange(v ?? "")} onMount={handleMount}
    options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }} />;
}
