import { useState } from "react";
import { ChevronDown, ChevronRight, RefreshCw } from "lucide-react";
import type { VarInfo } from "@/lib/kernel/KernelConnection";
import { Button } from "@/components/ui/button";

export interface VariablesPanelProps {
  vars: VarInfo[];
  loading: boolean;
  onRefresh: () => void;
  /** Click a variable's name — inserts + runs a `display(name)` cell. */
  onDisplay: (name: string) => void;
}

/** A single variable row: name (click → display), type badge, and — for
 *  DataFrames (non-null `columns`) — an expander revealing the column names. */
function VarRow({ v, onDisplay }: { v: VarInfo; onDisplay: (name: string) => void }) {
  const [expanded, setExpanded] = useState(false);
  const hasColumns = Array.isArray(v.columns) && v.columns.length > 0;

  return (
    <div>
      <div className="flex w-full items-center gap-1 rounded px-1.5 py-1 text-xs hover:bg-accent">
        {hasColumns ? (
          <span
            role="button"
            tabIndex={-1}
            aria-label={expanded ? `Collapse ${v.name}` : `Expand ${v.name}`}
            onClick={() => setExpanded((e) => !e)}
            className="shrink-0 text-muted-foreground"
          >
            {expanded ? (
              <ChevronDown className="h-3 w-3" />
            ) : (
              <ChevronRight className="h-3 w-3" />
            )}
          </span>
        ) : (
          <span className="w-3 shrink-0" />
        )}
        <button
          type="button"
          onClick={() => onDisplay(v.name)}
          title={`display(${v.name})`}
          className="min-w-0 flex-1 truncate text-left font-mono text-foreground hover:underline"
        >
          {v.name}
        </button>
        <span className="ml-auto shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
          {v.type}
        </span>
      </div>
      {expanded && hasColumns && (
        <div className="px-1.5 py-1 pl-7 text-[11px] text-muted-foreground">
          {v.columns!.join(", ")}
        </div>
      )}
    </div>
  );
}

export function VariablesPanel({ vars, loading, onRefresh, onDisplay }: VariablesPanelProps) {
  return (
    <div className="flex flex-col gap-0.5 py-1">
      <div className="flex items-center justify-between px-3 py-1">
        <span className="text-[11px] text-muted-foreground">Kernel namespace</span>
        <Button
          variant="ghost"
          size="icon"
          className="h-6 w-6"
          aria-label="Refresh variables"
          title="Refresh"
          onClick={onRefresh}
          disabled={loading}
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
        </Button>
      </div>

      {vars.length === 0 && !loading && (
        <p className="px-3 py-2 text-xs text-muted-foreground">
          No variables yet — run a cell.
        </p>
      )}

      {vars.map((v) => (
        <VarRow key={v.name} v={v} onDisplay={onDisplay} />
      ))}
    </div>
  );
}
