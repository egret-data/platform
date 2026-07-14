import {
  type MutableRefObject,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Link, useParams } from "react-router-dom";
import { ArrowLeft, Eraser, Play, Plus, RotateCcw, Square } from "lucide-react";
import { toast } from "sonner";

import {
  type Cell,
  type CellType,
  useActiveExecutions,
  useAddCell,
  useDeleteCell,
  useNotebook,
  useReorderCells,
  useUpdateCell,
  useUpdateNotebook,
} from "@/api/notebooks";
import { CellEditor } from "@/components/notebook/CellEditor";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import type { KernelConnection } from "@/lib/kernel/KernelConnection";
import type { CellOutput } from "@/lib/kernel/protocol";
import { useKernel } from "@/lib/kernel/useKernel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { ApiError } from "@/api/client";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";

// ── Cell with autosave ────────────────────────────────────────────────────────

interface ManagedCellProps {
  cell: Cell;
  kernel: KernelConnection;
  onChangeType: (cid: string, type: CellType) => void;
  onDelete: (cid: string) => void;
  onMoveUp: (cid: string) => void;
  onMoveDown: (cid: string) => void;
  updateCellMutate: (args: {
    cid: string;
    source?: string;
    cell_type?: CellType;
  }) => void;
  /** Per-cell runner registry keyed by cell id; owned by the page toolbar. */
  runners: MutableRefObject<
    Record<string, { run: () => Promise<void>; clear: () => void }>
  >;
  /** Best-effort seed from the active-executions registry at mount. */
  initialRunning?: boolean;
  onRunAbove?: () => void;
  onRunBelow?: () => void;
  /** Report whether this cell has an unflushed debounced save pending (for the page's
   *  beforeunload guard). Called on edit (true) and on flush/persist (false). */
  onPendingChange?: (cid: string, pending: boolean) => void;
}

function ManagedCell({
  cell,
  kernel,
  onChangeType,
  onDelete,
  onMoveUp,
  onMoveDown,
  updateCellMutate,
  runners,
  initialRunning,
  onRunAbove,
  onRunBelow,
  onPendingChange,
}: ManagedCellProps) {
  const [source, setSource] = useState(cell.source);
  // Restore-on-load: seed outputs from the persisted last_output.
  const [outputs, setOutputs] = useState<CellOutput[]>(() =>
    Array.isArray(cell.last_output) ? (cell.last_output as CellOutput[]) : [],
  );
  const [running, setRunning] = useState(initialRunning ?? false);
  const timerRef = useRef<number | undefined>(undefined);
  // True while a debounced source save is scheduled but not yet persisted.
  const pendingRef = useRef(false);

  // Sync source if the server data changes (e.g. after refetch)
  const prevCellIdRef = useRef(cell.id);
  useEffect(() => {
    if (prevCellIdRef.current !== cell.id) {
      setSource(cell.source);
      prevCellIdRef.current = cell.id;
    }
  }, [cell.id, cell.source]);

  const handleChangeSource = useCallback(
    (newSource: string) => {
      setSource(newSource);
      window.clearTimeout(timerRef.current);
      pendingRef.current = true;
      onPendingChange?.(cell.id, true);
      timerRef.current = window.setTimeout(() => {
        pendingRef.current = false;
        onPendingChange?.(cell.id, false);
        updateCellMutate({ cid: cell.id, source: newSource });
      }, 600);
    },
    [cell.id, updateCellMutate, onPendingChange],
  );

  // Live flush closure, reassigned each render so it captures the latest `source`
  // and mutate fn. On unmount we RUN the pending save (not just clear the timer),
  // so a quick navigation away doesn't drop the last debounced edit.
  const flushRef = useRef<() => void>(() => {});
  flushRef.current = () => {
    window.clearTimeout(timerRef.current);
    if (pendingRef.current) {
      pendingRef.current = false;
      onPendingChange?.(cell.id, false);
      updateCellMutate({ cid: cell.id, source });
    }
  };
  useEffect(() => {
    return () => flushRef.current();
  }, []);

  async function handleRun() {
    try {
      await kernel.connect();
      setOutputs([]);
      setRunning(true);
      kernel.execute(cell.id, source, {
        onOutput: (o) => setOutputs((prev) => [...prev, o]),
        onStatus: (s) => {
          if (s === "idle") setRunning(false);
        },
        onExecutionCount: () => {},
      });
    } catch (e) {
      setOutputs([
        { type: "error", ename: "KernelError", evalue: String(e), traceback: [] },
      ]);
      setRunning(false);
    }
  }

  // Register this cell's runner so the page toolbar can drive it. Markdown
  // cells register a no-op run (they don't execute) but a real clear.
  useEffect(() => {
    runners.current[cell.id] = {
      run: cell.cell_type === "markdown" ? async () => {} : handleRun,
      clear: () => setOutputs([]),
    };
    return () => {
      delete runners.current[cell.id];
    };
    // Re-register when the run closure's inputs change.
  }, [cell.id, source, cell.cell_type, runners]);

  return (
    <CellEditor
      cell={{ ...cell, source }}
      onChangeSource={handleChangeSource}
      onChangeType={(type) => onChangeType(cell.id, type)}
      onDelete={() => onDelete(cell.id)}
      onMoveUp={() => onMoveUp(cell.id)}
      onMoveDown={() => onMoveDown(cell.id)}
      onRun={cell.cell_type === "markdown" ? undefined : handleRun}
      running={running}
      outputs={outputs}
      onRunAbove={onRunAbove}
      onRunBelow={onRunBelow}
    />
  );
}

// ── Main page ─────────────────────────────────────────────────────────────────

export default function NotebookPage() {
  const { workspace: ws, id } = useParams<{ workspace: string; id: string }>();

  const kernel = useKernel(ws!, id!);
  const notebook = useNotebook(ws!, id ?? null);
  const updateNotebook = useUpdateNotebook(ws!, id!);
  const addCell = useAddCell(ws!, id!);
  const updateCell = useUpdateCell(ws!, id!);
  const deleteCell = useDeleteCell(ws!, id!);
  const reorderCells = useReorderCells(ws!, id!);
  const active = useActiveExecutions(ws!, id!);

  // Per-cell runner registry: each ManagedCell registers { run, clear } under
  // its cell id on mount; the toolbar iterates and drives them.
  const runners = useRef<
    Record<string, { run: () => Promise<void>; clear: () => void }>
  >({});
  const [runningAll, setRunningAll] = useState(false);

  // Cells with an unflushed debounced source save. A cell reports itself in/out of
  // this set; state (not a ref) so the beforeunload guard re-evaluates on change.
  const [pendingCells, setPendingCells] = useState<Set<string>>(() => new Set());
  const setCellPending = useCallback((cid: string, pending: boolean) => {
    setPendingCells((prev) => {
      if (pending === prev.has(cid)) return prev; // no transition → same ref, no re-render
      const next = new Set(prev);
      if (pending) next.add(cid);
      else next.delete(cid);
      return next;
    });
  }, []);

  // Cells the recorder still lists as in-flight → seed a spinner on load.
  const runningSet = useMemo(
    () => new Set(active.data?.running ?? []),
    [active.data],
  );

  // Local name state for rename-on-blur
  const [localName, setLocalName] = useState("");
  useEffect(() => {
    if (notebook.data?.name) {
      setLocalName(notebook.data.name);
    }
  }, [notebook.data?.name]);

  const handleNameBlur = () => {
    if (!notebook.data) return;
    const trimmed = localName.trim();
    if (trimmed && trimmed !== notebook.data.name) {
      updateNotebook.mutate({ name: trimmed });
    }
  };

  const handlePublicToggle = (checked: boolean) => {
    updateNotebook.mutate({ is_public: checked });
  };

  // Name saves only on blur; a cell save debounces ~600ms. Warn on refresh/close while
  // either is unpersisted (beforeunload-only; SPA nav flushes cell saves on unmount).
  const nameDirty =
    !!notebook.data &&
    localName.trim() !== "" &&
    localName.trim() !== notebook.data.name;
  useUnsavedChangesWarning(pendingCells.size > 0 || nameDirty);

  const handleAddCell = (cellType: CellType = "code") => {
    const cells = notebook.data?.cells ?? [];
    const maxOrder = cells.reduce((m, c) => Math.max(m, c.cell_order), -1);
    addCell.mutate({ cell_type: cellType, cell_order: maxOrder + 1 });
  };

  const handleDeleteCell = (cid: string) => {
    deleteCell.mutate(cid);
  };

  const handleMoveUp = (cid: string) => {
    const cells = [...(notebook.data?.cells ?? [])].sort(
      (a, b) => a.cell_order - b.cell_order,
    );
    const idx = cells.findIndex((c) => c.id === cid);
    if (idx <= 0) return;
    const newOrder = cells.map((c) => c.id);
    [newOrder[idx - 1], newOrder[idx]] = [newOrder[idx], newOrder[idx - 1]];
    reorderCells.mutate(newOrder);
  };

  const handleMoveDown = (cid: string) => {
    const cells = [...(notebook.data?.cells ?? [])].sort(
      (a, b) => a.cell_order - b.cell_order,
    );
    const idx = cells.findIndex((c) => c.id === cid);
    if (idx < 0 || idx >= cells.length - 1) return;
    const newOrder = cells.map((c) => c.id);
    [newOrder[idx], newOrder[idx + 1]] = [newOrder[idx + 1], newOrder[idx]];
    reorderCells.mutate(newOrder);
  };

  const handleChangeType = (cid: string, cellType: CellType) => {
    updateCell.mutate({ cid, cell_type: cellType });
  };

  // ── Loading / error states ────────────────────────────────────────────────

  if (notebook.isLoading) {
    return (
      <section className="flex flex-col gap-4 p-4">
        <div className="h-8 w-48 animate-pulse rounded bg-muted" />
        <ListSkeleton rows={3} cols={1} />
      </section>
    );
  }

  // A 404 is a genuine "missing notebook" — fall through to the not-found UI
  // below. Any other fetch error is a real failure and gets its own alert.
  const isNotFound = (notebook.error as ApiError | null)?.status === 404;
  if (notebook.isError && !isNotFound) {
    return (
      <section className="flex flex-col items-center gap-3 p-8 text-center">
        <Alert variant="destructive" className="max-w-md text-left">
          <AlertDescription>Failed to load notebook. Please try again.</AlertDescription>
        </Alert>
        <Link
          to={`/explore/${ws}/notebooks`}
          className="inline-flex items-center gap-1 text-sm text-primary hover:underline"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          Back to notebooks
        </Link>
      </section>
    );
  }

  if (!notebook.data) {
    return (
      <section className="p-8 text-center text-muted-foreground">
        <p>Notebook not found.</p>
        <Link
          to={`/explore/${ws}/notebooks`}
          className="mt-2 inline-flex items-center gap-1 text-sm text-primary hover:underline"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          Back to notebooks
        </Link>
      </section>
    );
  }

  const nb = notebook.data;
  const sortedCells = [...nb.cells].sort((a, b) => a.cell_order - b.cell_order);

  // Run-all uses the Jupyter upfront-queue model: fire every non-markdown
  // cell's execute_request in order (each run() awaits the memoized connect,
  // then fires execute and returns). The kernel sequences them and honors
  // stop_on_error — so we do NOT await idle between cells.
  async function runAll() {
    setRunningAll(true);
    try {
      const runnable = sortedCells.filter((c) => c.cell_type !== "markdown");
      for (const c of runnable) {
        await runners.current[c.id]?.run();
      }
    } finally {
      setRunningAll(false);
    }
  }

  async function runSlice(from: number, to: number) {
    const slice = sortedCells.slice(from, to).filter((c) => c.cell_type !== "markdown");
    for (const c of slice) {
      await runners.current[c.id]?.run();
    }
  }

  function clearAll() {
    sortedCells.forEach((c) => runners.current[c.id]?.clear());
  }

  return (
    <section className="flex flex-col gap-4 p-4">
      {/* Header */}
      <div className="flex flex-col gap-3">
        <Link
          to={`/explore/${ws}/notebooks`}
          className="flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          Notebooks
        </Link>

        <div className="flex flex-wrap items-center gap-4">
          {/* Rename on blur */}
          <Input
            value={localName}
            onChange={(e) => setLocalName(e.target.value)}
            onBlur={handleNameBlur}
            className="max-w-xs text-base font-semibold"
            aria-label="Notebook name"
          />

          {/* Public toggle */}
          <div className="flex items-center gap-2">
            <Switch
              id="nb-public"
              checked={nb.is_public}
              onCheckedChange={handlePublicToggle}
              aria-label="Public"
            />
            <Label htmlFor="nb-public" className="text-sm">
              Public
            </Label>
          </div>

          {/* Notebook run controls */}
          <div className="flex items-center gap-1">
            <Button
              size="sm"
              variant="outline"
              onClick={runAll}
              disabled={runningAll}
            >
              <Play className="h-3.5 w-3.5" />
              Run all
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => kernel.interrupt().catch((e) => toast.error(String((e as Error)?.message ?? e)))}
            >
              <Square className="h-3.5 w-3.5" />
              Interrupt
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => kernel.restart().catch((e) => toast.error(String((e as Error)?.message ?? e)))}
            >
              <RotateCcw className="h-3.5 w-3.5" />
              Restart
            </Button>
            <Button size="sm" variant="ghost" onClick={clearAll}>
              <Eraser className="h-3.5 w-3.5" />
              Clear outputs
            </Button>
          </div>
        </div>
      </div>

      {/* Cells */}
      <div className="flex flex-col gap-3">
        {sortedCells.map((cell, index) => (
          <ManagedCell
            key={cell.id}
            cell={cell}
            kernel={kernel}
            onChangeType={handleChangeType}
            onDelete={handleDeleteCell}
            onMoveUp={handleMoveUp}
            onMoveDown={handleMoveDown}
            updateCellMutate={updateCell.mutate}
            runners={runners}
            onPendingChange={setCellPending}
            initialRunning={runningSet.has(cell.id)}
            onRunAbove={index > 0 ? () => runSlice(0, index) : undefined}
            onRunBelow={index < sortedCells.length - 1 ? () => runSlice(index + 1, sortedCells.length) : undefined}
          />
        ))}
      </div>

      {/* Add cell buttons */}
      <div className="flex items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          onClick={() => handleAddCell("code")}
          aria-label="Add cell"
        >
          <Plus className="h-4 w-4" />
          Add cell
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={() => handleAddCell("markdown")}
          aria-label="Add markdown cell"
        >
          <Plus className="h-4 w-4" />
          Add markdown
        </Button>
      </div>
    </section>
  );
}
