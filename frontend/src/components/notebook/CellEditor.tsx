import { useRef, useState } from "react";
import Editor from "@monaco-editor/react";
import type { Monaco } from "@monaco-editor/react";
import type { editor as MonacoEditor } from "monaco-editor";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ArrowDown, ArrowUp, ChevronDown, Loader2, Play, Trash2 } from "lucide-react";

import type { Cell, CellType } from "@/api/notebooks";
import { Button } from "@/components/ui/button";
import { CellOutputRenderer } from "@/components/notebook/CellOutputRenderer";
import { registerStaticCompletions } from "@/components/notebook/monaco/registerStatic";
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
}: CellEditorProps) {
  const [mdFocused, setMdFocused] = useState(false);
  // Latest onRun for the Monaco Shift+Enter command (captured once on mount).
  const runRef = useRef<(() => void) | undefined>(onRun);
  runRef.current = onRun;

  const errored = outputs?.some((o) => o.type === "error") ?? false;
  // Left accent bar keyed to state.
  const accent = running
    ? "var(--primary)"
    : errored
      ? "var(--destructive)"
      : outputs && outputs.length > 0
        ? "var(--success)"
        : "var(--border)";

  const execLabel =
    cell.cell_type === "markdown"
      ? ""
      : cell.execution_count == null
        ? "In [ ]:"
        : `In [${cell.execution_count}]:`;

  const monacoLanguage = cell.cell_type === "sql" ? "sql" : "python";

  const handleMount = (editor: MonacoEditor.IStandaloneCodeEditor, monaco: Monaco) => {
    registerStaticCompletions(monaco);
    editor.addCommand(monaco.KeyMod.Shift | monaco.KeyCode.Enter, () => runRef.current?.());
  };

  return (
    <div
      className="group flex rounded-lg border bg-card shadow-sm"
      style={{ borderLeft: `3px solid ${accent}` }}
    >
      {/* Gutter */}
      <div className="w-16 shrink-0 select-none px-2 py-2 text-right font-mono text-[11px] text-muted-foreground">
        {running ? <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin" /> : execLabel}
      </div>

      <div className="min-w-0 flex-1">
        {/* Toolbar — revealed on hover/focus */}
        <div className="flex items-center gap-1 border-b px-2 py-1 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
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
              value={cell.sql_into ?? ""}
              onChange={(e) => onChangeSqlInto?.(e.target.value)}
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
              onFocus={() => setMdFocused(true)}
              onBlur={() => setMdFocused(false)}
              aria-label="Markdown source"
              placeholder="Write markdown…"
            />
          ) : (
            <div
              role="button"
              tabIndex={0}
              onClick={() => setMdFocused(true)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setMdFocused(true);
                }
              }}
              className="cursor-text px-3 py-2 text-sm [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_h1]:mb-1 [&_h1]:text-lg [&_h1]:font-semibold [&_h2]:mb-1 [&_h2]:text-base [&_h2]:font-semibold [&_ol]:list-decimal [&_ol]:pl-5 [&_ul]:list-disc [&_ul]:pl-5"
            >
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{cell.source}</ReactMarkdown>
            </div>
          )
        ) : (
          <div className="min-h-[120px]">
            <Editor
              height="200px"
              defaultLanguage={monacoLanguage}
              language={monacoLanguage}
              value={cell.source}
              onChange={(v) => onChangeSource(v ?? "")}
              onMount={handleMount}
              options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }}
            />
          </div>
        )}

        {outputs && outputs.length > 0 && <CellOutputRenderer outputs={outputs} />}
      </div>
    </div>
  );
}
