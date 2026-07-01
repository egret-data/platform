import Editor from "@monaco-editor/react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ArrowDown, ArrowUp, Loader2, Play, Trash2 } from "lucide-react";

import type { Cell, CellType } from "@/api/notebooks";
import { Button } from "@/components/ui/button";
import { CellOutputRenderer } from "@/components/notebook/CellOutputRenderer";
import type { CellOutput } from "@/lib/kernel/protocol";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

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
}: CellEditorProps) {
  const monacoLanguage = cell.cell_type === "sql" ? "sql" : "python";

  return (
    <div className="rounded-lg border border-border bg-card shadow-sm">
      {/* Toolbar */}
      <div className="flex items-center gap-1 border-b border-border px-2 py-1">
        <Select
          value={cell.cell_type}
          onValueChange={(v) => onChangeType(v as CellType)}
        >
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
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0"
            onClick={onMoveUp}
            aria-label="Move cell up"
          >
            <ArrowUp className="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0"
            onClick={onMoveDown}
            aria-label="Move cell down"
          >
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
            title={running ? "Running…" : "Run"}
            aria-label="Run"
            onClick={onRun}
          >
            {running ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <Play className="h-3 w-3" />
            )}
            Run
          </Button>
        </div>
      </div>

      {/* Cell body */}
      {cell.cell_type === "markdown" ? (
        <div className="flex flex-col gap-0">
          <textarea
            className="min-h-[80px] w-full resize-y border-b border-border bg-transparent p-3 font-mono text-sm focus:outline-none"
            value={cell.source}
            onChange={(e) => onChangeSource(e.target.value)}
            aria-label="Markdown source"
            placeholder="Write markdown…"
          />
          {cell.source.trim() ? (
            <div className="px-3 py-2 text-sm [&_h1]:mb-1 [&_h1]:text-lg [&_h1]:font-semibold [&_h2]:mb-1 [&_h2]:text-base [&_h2]:font-semibold [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5 [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{cell.source}</ReactMarkdown>
            </div>
          ) : null}
        </div>
      ) : (
        <div className="min-h-[120px]">
          <Editor
            height="200px"
            defaultLanguage={monacoLanguage}
            language={monacoLanguage}
            value={cell.source}
            onChange={(v) => onChangeSource(v ?? "")}
            options={{
              minimap: { enabled: false },
              fontSize: 13,
              scrollBeyondLastLine: false,
            }}
          />
        </div>
      )}

      {/* Outputs */}
      {outputs && outputs.length > 0 && <CellOutputRenderer outputs={outputs} />}
    </div>
  );
}
