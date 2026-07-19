import { useEffect, useRef, useState } from "react";
import Editor from "@monaco-editor/react";
import type { Monaco } from "@monaco-editor/react";
import type { editor as MonacoEditor } from "monaco-editor";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ArrowDown, ArrowUp, ChevronDown, GripVertical, Loader2, Play, Trash2 } from "lucide-react";
import type { DraggableAttributes, DraggableSyntheticListeners } from "@dnd-kit/core";

import type { Cell, CellType } from "@/api/notebooks";
import { Button } from "@/components/ui/button";
import { CellOutputRenderer } from "@/components/notebook/CellOutputRenderer";
import { registerNotebookCompletions } from "@/components/notebook/monaco/notebookCompletion";
import type { CellOutput } from "@/lib/kernel/protocol";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

export interface CellEditorProps {
  cell: Cell;
  onChangeSource: (source: string) => void;
  onChangeType: (type: CellType) => void;
  onDelete: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
  onRun?: () => void;
  running?: boolean;
  outputs?: CellOutput[];
  onRunAbove?: () => void;
  onRunBelow?: () => void;
  /** Enabled engines for the SQL target dropdown (from useNotebookRuntime). */
  engines?: { id: string; name: string; engine: string; enabled: boolean }[];
  onChangeSqlTarget?: (target: string) => void;
  onChangeSqlInto?: (into: string) => void;
  /** Whether this cell is the page's active cell (shows a focus ring). */
  active?: boolean;
  /** Fired when the cell gains editing focus (Monaco text focus / markdown
   *  textarea focus) — the page sets activeCellId + mode="edit". */
  onFocusCell?: () => void;
  /** Fired when non-editor chrome (gutter / toolbar) is clicked — the page
   *  sets activeCellId while staying in command mode. */
  onSelectCell?: () => void;
  /** Registers a function that programmatically focuses this cell's editor,
   *  so the page can drive focus from keyboard navigation. */
  registerFocus?: (focus: () => void) => void;
  /** Registers a function that inserts text at the current cursor/selection
   *  in this cell's editor (Monaco cells only) — used by the schema browser's
   *  click-to-insert. */
  registerEdit?: (fn: (text: string) => void) => void;
  /** Shift+Enter: run this cell, then select the next one. */
  onRunAndNext?: () => void;
  /** Alt+Enter: run this cell, then insert a new cell below and select it. */
  onRunAndBelow?: () => void;
  /** Esc inside the editor: blur and return the page to command mode. */
  onEnterCommandMode?: () => void;
  /** dnd-kit sortable attributes+listeners (from the page's SortableCell
   *  wrapper) for the drag-handle button rendered in the toolbar. Undefined
   *  outside a DndContext (e.g. standalone tests) → no handle is rendered. */
  dragHandleProps?: { attributes: DraggableAttributes; listeners: DraggableSyntheticListeners };
}

export function CellEditor({
  cell,
  onChangeSource,
  onChangeType,
  onDelete,
  onMoveUp,
  onMoveDown,
  onRun,
  running,
  outputs,
  onRunAbove,
  onRunBelow,
  engines = [],
  onChangeSqlTarget,
  onChangeSqlInto,
  active,
  onFocusCell,
  onSelectCell,
  registerFocus,
  registerEdit,
  onRunAndNext,
  onRunAndBelow,
  onEnterCommandMode,
  dragHandleProps,
}: CellEditorProps) {
  const [mdFocused, setMdFocused] = useState(false);
  // Local buffer for the SQL result-variable input: cell.sql_into is react-query
  // server state that only settles after a mutation refetch, so binding the input
  // directly to it reverts every keystroke. Edit locally, commit on blur.
  const [sqlIntoLocal, setSqlIntoLocal] = useState(cell.sql_into ?? "");
  // Re-seed only when the cell identity changes (not on every server refetch,
  // which would clobber an in-progress edit).
  useEffect(() => {
    setSqlIntoLocal(cell.sql_into ?? "");
  }, [cell.id]);
  // Latest callbacks for Monaco commands (captured once on mount, so they must
  // be read through refs kept fresh every render rather than closed over).
  const runRef = useRef<(() => void) | undefined>(onRun);
  runRef.current = onRun;
  const focusCellRef = useRef<(() => void) | undefined>(onFocusCell);
  focusCellRef.current = onFocusCell;
  const runAndNextRef = useRef<(() => void) | undefined>(onRunAndNext);
  runAndNextRef.current = onRunAndNext;
  const runAndBelowRef = useRef<(() => void) | undefined>(onRunAndBelow);
  runAndBelowRef.current = onRunAndBelow;
  const commandModeRef = useRef<(() => void) | undefined>(onEnterCommandMode);
  commandModeRef.current = onEnterCommandMode;

  const errored = outputs?.some((o) => o.type === "error") ?? false;
  // Thin left accent, colored only for active states (running / error); at rest
  // it stays the neutral border so cells don't read as a wall of green.
  const accent = running
    ? "var(--primary)"
    : errored
      ? "var(--destructive)"
      : "var(--border)";

  // Monaco grows with its content (no fixed empty box); capped, then scrolls.
  const [editorHeight, setEditorHeight] = useState(52);

  const execLabel =
    cell.cell_type === "markdown"
      ? ""
      : cell.execution_count == null
        ? "In [ ]:"
        : `In [${cell.execution_count}]:`;

  const monacoLanguage = cell.cell_type === "sql" ? "sql" : "python";

  const handleMount = (editor: MonacoEditor.IStandaloneCodeEditor, monaco: Monaco) => {
    registerNotebookCompletions(monaco);
    // Let the page drive focus into this cell (keyboard nav / j-k / Enter).
    registerFocus?.(() => editor.focus());
    registerEdit?.((text: string) => {
      const sel = editor.getSelection();
      if (sel) editor.executeEdits("schema-insert", [{ range: sel, text, forceMoveMarkers: true }]);
      editor.focus();
    });
    editor.onDidFocusEditorText(() => focusCellRef.current?.());
    // Blurring the editor by any means (clicking the tools panel / blank space,
    // not just Escape) must return the page to command mode, else `mode` sticks
    // at "edit" and keyboard nav silently stops working.
    editor.onDidBlurEditorText(() => commandModeRef.current?.());
    // Run shortcuts: Shift+Enter = run + select next (Jupyter convention),
    // Ctrl/Cmd+Enter = run in place, Alt+Enter = run + insert-and-select below.
    editor.addCommand(monaco.KeyMod.Shift | monaco.KeyCode.Enter, () => runAndNextRef.current?.());
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => runRef.current?.());
    editor.addCommand(monaco.KeyMod.Alt | monaco.KeyCode.Enter, () => runAndBelowRef.current?.());
    editor.addCommand(monaco.KeyCode.Escape, () => {
      editor.getDomNode()?.blur();
      commandModeRef.current?.();
    });
    const sync = () =>
      setEditorHeight(Math.min(Math.max(editor.getContentHeight(), 40), 480));
    editor.onDidContentSizeChange(sync);
    sync();
  };

  return (
    <div
      className={`group flex overflow-hidden rounded-lg border bg-card shadow-sm transition-shadow hover:shadow-md ${active ? "ring-2 ring-primary/40" : ""}`}
      style={{ borderLeftWidth: "2px", borderLeftColor: accent }}
      data-active={active ? "true" : "false"}
    >
      {/* Gutter — non-editor chrome: selects the cell without entering edit mode. */}
      <div
        className="w-16 shrink-0 select-none px-2 py-2 text-right font-mono text-[11px] text-muted-foreground"
        onClick={() => onSelectCell?.()}
      >
        {running ? <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin" /> : execLabel}
      </div>

      <div className="min-w-0 flex-1">
        {/* Toolbar — revealed on hover/focus; also non-editor chrome. */}
        <div
          className="flex items-center gap-1 border-b px-2 py-1 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100"
          onClick={() => onSelectCell?.()}
        >
          {dragHandleProps && (
            <button
              type="button"
              aria-label="Drag to reorder cell"
              className="cursor-grab touch-none text-muted-foreground opacity-0 group-hover:opacity-100 hover:text-foreground"
              onClick={(e) => e.stopPropagation()}
              {...dragHandleProps.attributes}
              {...dragHandleProps.listeners}
            >
              <GripVertical className="h-3.5 w-3.5" />
            </button>
          )}

          <Select value={cell.cell_type} onValueChange={(v) => onChangeType(v as CellType)}>
            <SelectTrigger className="h-7 w-28 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="code">Code</SelectItem>
              <SelectItem value="markdown">Markdown</SelectItem>
              <SelectItem value="sql">SQL</SelectItem>
            </SelectContent>
          </Select>

          <div className="ml-auto flex items-center gap-1">
            <Button variant="ghost" size="sm" className="h-7 w-7 p-0" onClick={onMoveUp} aria-label="Move cell up">
              <ArrowUp className="h-3.5 w-3.5" />
            </Button>
            <Button variant="ghost" size="sm" className="h-7 w-7 p-0" onClick={onMoveDown} aria-label="Move cell down">
              <ArrowDown className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 w-7 p-0 text-muted-foreground hover:text-destructive"
              onClick={onDelete}
              aria-label="Delete cell"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1 text-xs"
              disabled={!onRun || running}
              title={running ? "Running…" : "Run (Shift+Enter)"}
              aria-label="Run"
              onClick={onRun}
            >
              {running ? <Loader2 className="h-3 w-3 animate-spin" /> : <Play className="h-3 w-3" />}
              Run
            </Button>
            {cell.cell_type !== "markdown" && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button size="icon" variant="ghost" className="h-7 w-7" aria-label="Run options">
                    <ChevronDown className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem disabled={!onRunAbove} onClick={onRunAbove}>
                    Run all above
                  </DropdownMenuItem>
                  <DropdownMenuItem disabled={!onRunBelow} onClick={onRunBelow}>
                    Run all below
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </div>
        </div>

        {cell.cell_type === "sql" && (
          <div className="flex items-center gap-2 border-b px-2 py-1 text-xs">
            <span className="text-muted-foreground">Target</span>
            <Select value={cell.sql_target ?? ""} onValueChange={(v) => onChangeSqlTarget?.(v)}>
              <SelectTrigger className="h-7 w-40 text-xs" aria-label="SQL target">
                <SelectValue placeholder="Select engine" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="daft">Daft (local)</SelectItem>
                {engines
                  .filter((e) => e.enabled)
                  .map((e) => (
                    <SelectItem key={e.id} value={e.id}>
                      {e.name}
                    </SelectItem>
                  ))}
              </SelectContent>
            </Select>
            <span className="ml-2 text-muted-foreground">→</span>
            <input
              aria-label="Result variable"
              value={sqlIntoLocal}
              onChange={(e) => setSqlIntoLocal(e.target.value)}
              onBlur={() => {
                if (sqlIntoLocal !== (cell.sql_into ?? "")) onChangeSqlInto?.(sqlIntoLocal);
              }}
              className="h-7 w-32 rounded border bg-transparent px-2 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-ring"
              placeholder="df"
            />
          </div>
        )}

        {/* Body */}
        {cell.cell_type === "markdown" ? (
          mdFocused || !cell.source.trim() ? (
            <textarea
              autoFocus={mdFocused}
              className="min-h-[80px] w-full resize-y bg-transparent p-3 font-mono text-sm focus:outline-none"
              value={cell.source}
              onChange={(e) => onChangeSource(e.target.value)}
              onFocus={() => {
                setMdFocused(true);
                onFocusCell?.();
              }}
              onBlur={() => {
                setMdFocused(false);
                onEnterCommandMode?.();
              }}
              aria-label="Markdown source"
              placeholder="Write markdown…"
            />
          ) : (
            <div
              role="button"
              tabIndex={0}
              onClick={() => {
                setMdFocused(true);
                onFocusCell?.();
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setMdFocused(true);
                  onFocusCell?.();
                }
              }}
              className="cursor-text px-3 py-2 text-sm [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_h1]:mb-1 [&_h1]:text-lg [&_h1]:font-semibold [&_h2]:mb-1 [&_h2]:text-base [&_h2]:font-semibold [&_ol]:list-decimal [&_ol]:pl-5 [&_ul]:list-disc [&_ul]:pl-5"
            >
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{cell.source}</ReactMarkdown>
            </div>
          )
        ) : (
          <div className="py-1">
            <Editor
              height={editorHeight}
              defaultLanguage={monacoLanguage}
              language={monacoLanguage}
              value={cell.source}
              onChange={(v) => onChangeSource(v ?? "")}
              onMount={handleMount}
              options={{
                minimap: { enabled: false },
                fontSize: 13,
                scrollBeyondLastLine: false,
                lineNumbersMinChars: 3,
                folding: false,
                overviewRulerLanes: 0,
                renderLineHighlight: "none",
                padding: { top: 6, bottom: 6 },
              }}
            />
          </div>
        )}

        {outputs && outputs.length > 0 && <CellOutputRenderer outputs={outputs} />}
      </div>
    </div>
  );
}
