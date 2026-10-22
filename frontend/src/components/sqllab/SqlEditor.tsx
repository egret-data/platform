import type { Monaco } from "@monaco-editor/react";
import Editor from "@/components/monaco/MonacoEditor";
import type * as MonacoNS from "monaco-editor";
import { useEffect, useRef } from "react";
import { useIsDark } from "@/hooks/useIsDark";
import { emptySchemaIndex, type SchemaIndex } from "./schemaIndex";
import { pickRunSql } from "./pickRunSql";

const KEYWORDS = [
  "SELECT","FROM","WHERE","GROUP BY","ORDER BY","HAVING","LIMIT","JOIN","LEFT JOIN","RIGHT JOIN",
  "INNER JOIN","ON","AS","AND","OR","NOT","IN","IS NULL","IS NOT NULL","DISTINCT","COUNT","SUM",
  "AVG","MIN","MAX","CASE","WHEN","THEN","ELSE","END","UNION","UNION ALL","WITH","INSERT","UPDATE","DELETE","CREATE","TABLE",
];

// Editor backgrounds tuned to egret's surface tokens so the editor blends with
// the surrounding panels rather than reading as a foreign white/black block.
const LIGHT_THEME = "egret-sql-light";
const DARK_THEME = "egret-sql-dark";

function defineThemes(monaco: Monaco) {
  monaco.editor.defineTheme(LIGHT_THEME, {
    base: "vs", inherit: true, rules: [],
    colors: { "editor.background": "#ffffff" },
  });
  monaco.editor.defineTheme(DARK_THEME, {
    base: "vs-dark", inherit: true, rules: [],
    colors: { "editor.background": "#1c1c22" },
  });
}

/** Tables referenced by the whole query, so column suggestions can be scoped to
 *  what's actually in scope (FROM/JOIN targets). */
function referencedTables(sql: string): string[] {
  return [...sql.matchAll(/\b(?:from|join)\s+([a-zA-Z_][\w".]*)/gi)].map((m) => m[1].replace(/"/g, ""));
}

/** Build ranked, context-aware completions for the cursor position. */
function completionsFor(
  monaco: Monaco,
  model: MonacoNS.editor.ITextModel,
  position: MonacoNS.Position,
  schema: SchemaIndex,
  extraWords: string[],
) {
  const K = monaco.languages.CompletionItemKind;
  const word = model.getWordUntilPosition(position);
  const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
  const textUntil = model.getValueInRange({ startLineNumber: 1, startColumn: 1, endLineNumber: position.lineNumber, endColumn: position.column });
  const cols = schema.columnsByTable;

  const colItem = (c: { name: string; type: string }, src: string, sort: string) => ({
    label: c.name, kind: K.Field, insertText: c.name, range, detail: `${c.type} · ${src}`, sortText: sort + c.name,
  });
  const tableItem = (t: string, sort: string) => ({
    label: t, kind: K.Struct, insertText: t, range, detail: "table", sortText: sort + t,
  });

  // 1) Dotted path `prefix.` → columns of that table, else child objects.
  const dot = /([a-zA-Z_][\w"]*)\.\s*[\w"]*$/.exec(textUntil);
  if (dot) {
    const prefix = dot[1].replace(/"/g, "").toLowerCase();
    const tableCols = cols[prefix] ?? cols[prefix.split(".").pop() ?? ""];
    if (tableCols) return { suggestions: tableCols.map((c) => colItem(c, prefix, "0")) };
    const children = new Set<string>();
    for (const t of schema.tables) {
      const tl = t.toLowerCase();
      if (tl.startsWith(prefix + ".")) children.add(t.slice(prefix.length + 1).split(".")[0]);
    }
    if (children.size) return { suggestions: [...children].map((n) => ({ label: n, kind: K.Module, insertText: n, range, sortText: "0" + n })) };
  }

  // 2) Right after FROM/JOIN → tables.
  if (/\b(from|join)\s+[\w".]*$/i.test(textUntil)) {
    return { suggestions: schema.tables.map((t) => tableItem(t, "0")) };
  }

  // 3) General: columns of referenced tables first, then tables, keywords, others.
  const suggestions: MonacoNS.languages.CompletionItem[] = [];
  const seen = new Set<string>();
  for (const ref of referencedTables(model.getValue())) {
    const key = ref.toLowerCase();
    const tc = cols[key] ?? cols[key.split(".").pop() ?? ""];
    if (tc) for (const c of tc) if (!seen.has(c.name)) { seen.add(c.name); suggestions.push(colItem(c, ref, "0") as MonacoNS.languages.CompletionItem); }
  }
  for (const t of schema.tables) suggestions.push(tableItem(t, "1") as MonacoNS.languages.CompletionItem);
  for (const k of KEYWORDS) suggestions.push({ label: k, kind: K.Keyword, insertText: k, range, sortText: "2" + k });
  for (const w of extraWords) if (!seen.has(w)) suggestions.push({ label: w, kind: K.Field, insertText: w, range, sortText: "3" + w });
  return { suggestions };
}

/** The id of the editor's ⌘↵ action (run the selection, else the whole text). */
export const SQL_RUN_ACTION = "egret.sql.run";

export function SqlEditor({ value, onChange, schemaWords, schema, onRun, onSelectionChange, readOnly, ariaLabel }: {
  value: string;
  onChange: (v: string) => void;
  schemaWords: string[];
  schema?: SchemaIndex;
  onRun?: (sql: string) => void;
  onSelectionChange?: (selectedText: string) => void;
  /** Refuse edits. A surrounding `<fieldset disabled>` cannot do this — a
   *  fieldset disables form controls, and Monaco is a contenteditable-driven
   *  widget rather than one, so a read-only surface has to say so here. */
  readOnly?: boolean;
  /** Accessible name for the editor's text surface. Monaco's default is a
   *  generic "Editor content…", which cannot distinguish two editors on a page
   *  — a caller embedding this in a form should name the field it stands for. */
  ariaLabel?: string;
}) {
  const isDark = useIsDark();
  const wordsRef = useRef(schemaWords);
  wordsRef.current = schemaWords;
  const schemaRef = useRef(schema ?? emptySchemaIndex());
  schemaRef.current = schema ?? emptySchemaIndex();
  // **A read-only editor has no run callback.** Folded into the ref the command
  // already reads rather than checked inside it, so the unsafe pair cannot exist
  // at the point of use — and kept in a ref rather than captured because
  // `handleMount` runs once and `readOnly` can turn on afterwards: an endpoint
  // can be moved to the trash while its editor is on screen.
  const onRunRef = useRef(onRun);
  onRunRef.current = readOnly ? undefined : onRun;
  const onSelRef = useRef(onSelectionChange);
  onSelRef.current = onSelectionChange;
  const disposables = useRef<{ dispose: () => void }[]>([]);
  useEffect(() => () => { disposables.current.forEach((d) => d.dispose()); disposables.current = []; }, []);

  // **The editor's text is not handed back to it on every keystroke (KI-006).**
  // `@monaco-editor/react` given a `value` writes it into the editor whenever it
  // differs from the editor's text. The caller's value makes a round trip
  // through its own state, so it could arrive one keystroke old while the editor
  // already held the next one, and the write erased what was just typed
  // (measured: characters lost typing at 40 ms a key with a 1,000-row result on
  // screen; none lost at any speed with the editor left alone). So the editor
  // gets the caller's text only at mount (`defaultValue`) and, after that, only
  // a value it did not emit itself: Format, a snippet, a history entry.
  const editorRef = useRef<MonacoNS.editor.IStandaloneCodeEditor | null>(null);
  const valueRef = useRef(value);
  /** Texts this editor emitted that the caller has not echoed back yet, oldest first. */
  const emitted = useRef<string[]>([]);
  /** Set while this component writes the caller's value, so that write is not emitted. */
  const writing = useRef(false);
  function writeValue(editor: MonacoNS.editor.IStandaloneCodeEditor, text: string) {
    const model = editor.getModel();
    if (!model || editor.getValue() === text) return;
    writing.current = true;
    try {
      if (readOnly) editor.setValue(text);
      else {
        // An edit, not `setValue`, so Undo can take it back.
        editor.executeEdits("", [{ range: model.getFullModelRange(), text, forceMoveMarkers: true }]);
        editor.pushUndoStop();
      }
    } finally {
      writing.current = false;
    }
  }
  useEffect(() => {
    valueRef.current = value;
    const echoed = emitted.current.indexOf(value);
    if (echoed >= 0) {
      // Our own text coming back, possibly behind what has been typed since.
      emitted.current.splice(0, echoed + 1);
      return;
    }
    emitted.current = [];
    if (editorRef.current) writeValue(editorRef.current, value);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- runs for a new value only
  }, [value]);
  function handleChange(v: string | undefined) {
    if (writing.current) return;
    const text = v ?? "";
    emitted.current.push(text);
    // A caller that never echoes would grow this without bound.
    if (emitted.current.length > 200) emitted.current.shift();
    onChange(text);
  }

  function selectedText(editor: MonacoNS.editor.IStandaloneCodeEditor): string {
    const sel = editor.getSelection();
    return sel && !sel.isEmpty() ? (editor.getModel()?.getValueInRange(sel) ?? "") : "";
  }

  function handleMount(editor: MonacoNS.editor.IStandaloneCodeEditor, monaco: Monaco) {
    // A second mount replaces the editor: what the first registered goes first.
    disposables.current.forEach((d) => d.dispose());
    disposables.current = [];
    // The caller's text may have changed while the editor was loading.
    editorRef.current = editor;
    writeValue(editor, valueRef.current);
    // **Answers only for this editor's model.** Monaco keeps completion
    // providers per LANGUAGE and asks every `sql` provider for every SQL model,
    // merging the answers (`suggest.js` `provideSuggestionItems`), so with two
    // SqlEditors mounted each would offer the other's schema and every keyword
    // twice. The model is read at call time, not captured, so a model swapped
    // under the editor is still answered.
    disposables.current.push(
      monaco.languages.registerCompletionItemProvider("sql", {
        triggerCharacters: ["."],
        provideCompletionItems: (model: MonacoNS.editor.ITextModel, position: MonacoNS.Position) =>
          model === editor.getModel()
            ? completionsFor(monaco, model, position, schemaRef.current, wordsRef.current)
            : { suggestions: [] },
      }),
    );
    // Cmd/Ctrl+Enter → run the selection (if any) else the whole editor.
    //
    // **Inert while `readOnly`, and that has to be the component's job rather
    // than every caller's.** `readOnly`/`domReadOnly` stop TYPING; neither
    // unregisters a keybinding, and a surrounding `<fieldset disabled>` never
    // sees a keystroke — so a read-only editor given an `onRun` would run from
    // the keyboard while its own surface refuses edits and its Run button is
    // disabled. `Query.tsx` avoids that by passing `onRun={trashed ? undefined
    // : …}`, which is correct and is pinned by its own test, but it left the
    // safety as something every future caller has to remember. The `onRunRef`
    // assignment above is what makes the unsafe pair unrepresentable at the
    // point of use. Registered-but-inert rather than not registered: the
    // action binds once at mount, and read-only can arrive after that.
    //
    // **An editor ACTION, not `addCommand`.** Standalone Monaco keeps every
    // editor's `addCommand` keybinding in one shared service: it answers in
    // whichever editor has focus, the newest registration wins, and it is never
    // removed, so ⌘↵ in one SQL Lab tab ran another tab's query, even one since
    // closed. An action's keybinding is scoped to this editor
    // (`editorId == getId()`) and its disposable is disposed with the rest.
    disposables.current.push(
      editor.addAction({
        id: SQL_RUN_ACTION,
        label: "Run SQL",
        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter],
        run: () => {
          onRunRef.current?.(pickRunSql(editor.getValue(), selectedText(editor)));
        },
      }),
    );
    // Report the current selection text so the toolbar can show "Run selection".
    disposables.current.push(
      editor.onDidChangeCursorSelection(() => onSelRef.current?.(selectedText(editor))),
    );
  }

  return <Editor height="100%" defaultLanguage="sql" defaultValue={value} onChange={handleChange}
    theme={isDark ? DARK_THEME : LIGHT_THEME}
    beforeMount={defineThemes}
    onMount={handleMount}
    options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false, padding: { top: 10, bottom: 10 }, renderLineHighlight: "line", smoothScrolling: true,
      readOnly: !!readOnly,
      // Without this the underlying textarea stays writable: `readOnly` alone
      // only makes Monaco discard the edit, so the DOM node still reports
      // itself as editable to assistive tech and to a test.
      domReadOnly: !!readOnly,
      ariaLabel }} />;
}
