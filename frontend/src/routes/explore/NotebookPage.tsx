import {
  Fragment,
  type KeyboardEvent,
  type MutableRefObject,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Link, useParams } from "react-router-dom";
import { ArrowLeft, Plus } from "lucide-react";
import { toast } from "sonner";

import {
  type Cell,
  type CellType,
  type EngineOption,
  defaultSqlInto,
  useActiveExecutions,
  useAddCell,
  useDeleteCell,
  useNotebook,
  useNotebookRuntime,
  useReorderCells,
  useUpdateCell,
  useUpdateNotebook,
} from "@/api/notebooks";
import { CellEditor } from "@/components/notebook/CellEditor";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import type { KernelConnection } from "@/lib/kernel/KernelConnection";
import type { CellOutput } from "@/lib/kernel/protocol";
import { useKernel } from "@/lib/kernel/useKernel";
import { useKernelConnectionStatus } from "@/lib/kernel/useKernelConnectionStatus";
import { NotebookHeader, type NotebookPanel } from "@/components/notebook/NotebookHeader";
import { TableOfContentsPanel } from "@/components/notebook/panels/TableOfContentsPanel";
import { DataSourcesPanel } from "@/components/notebook/panels/DataSourcesPanel";
import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { ApiError } from "@/api/client";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";

// ── Cell with autosave ────────────────────────────────────────────────────────

interface ManagedCellProps {
  cell: Cell;
  /** Position among the sorted cells — needed for "insert below" (Alt+Enter). */
  index: number;
  kernel: KernelConnection;
  onChangeType: (cid: string, type: CellType) => void;
  onDelete: (cid: string) => void;
  onMoveUp: (cid: string) => void;
  onMoveDown: (cid: string) => void;
  updateCellMutate: (args: {
    cid: string;
    source?: string;
    cell_type?: CellType;
    sql_target?: string;
    sql_into?: string;
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
  /** Enabled engines for the SQL target dropdown (from useNotebookRuntime). */
  engines?: EngineOption[];
  /** Whether this cell is the page's active cell (focus ring). */
  active?: boolean;
  /** Editor gained edit-focus (Monaco text focus / markdown edit) → page sets mode="edit". */
  onFocusCell?: (cid: string) => void;
  /** Non-editor chrome clicked → page sets active while staying in command mode. */
  onSelectCell?: (cid: string) => void;
  /** Registers this cell's programmatic-focus function under its id. */
  registerFocus?: (cid: string, focus: () => void) => void;
  /** Shift+Enter: select the next cell (the run itself is driven by handleRun below). */
  onSelectNext?: (cid: string) => void;
  /** Alt+Enter: insert + select a new code cell right below this index. */
  onInsertBelow?: (index: number) => void;
  /** Esc inside the editor: return the page to command mode. */
  onEnterCommandMode?: () => void;
}

function ManagedCell({
  cell,
  index,
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
  engines,
  active,
  onFocusCell,
  onSelectCell,
  registerFocus,
  onSelectNext,
  onInsertBelow,
  onEnterCommandMode,
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
      // SQL cells edit clean SQL in the Monaco buffer; the kernel needs the
      // %%egret_sql magic prefix so it binds the query result to sql_into.
      const runSource =
        cell.cell_type === "sql"
          ? `%%egret_sql --target ${cell.sql_target ?? ""} --into ${cell.sql_into || "df"}\n${source}`
          : source;
      kernel.execute(cell.id, runSource, {
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
    // Re-register when the run closure's inputs change. sql_target/sql_into
    // are included so run-all/run-above/run-below (which drive the cell via
    // `runners.current`, not the CellEditor's onRun prop) pick up target changes.
  }, [cell.id, source, cell.cell_type, cell.sql_target, cell.sql_into, runners]);

  return (
    <>
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
        engines={engines}
        onChangeSqlTarget={(t) => updateCellMutate({ cid: cell.id, sql_target: t })}
        onChangeSqlInto={(v) => updateCellMutate({ cid: cell.id, sql_into: v })}
        active={active}
        onFocusCell={() => onFocusCell?.(cell.id)}
        onSelectCell={() => onSelectCell?.(cell.id)}
        registerFocus={(fn) => registerFocus?.(cell.id, fn)}
        onRunAndNext={() => {
          if (cell.cell_type !== "markdown") void handleRun();
          onSelectNext?.(cell.id);
        }}
        onRunAndBelow={() => {
          if (cell.cell_type !== "markdown") void handleRun();
          onInsertBelow?.(index);
        }}
        onEnterCommandMode={onEnterCommandMode}
      />
      {cell.cell_type === "sql" && cell.sql_into && (outputs?.length ?? 0) > 0 && (
        <div className="px-3 pb-2 text-[11px] text-muted-foreground">
          Bound to <code className="font-mono">{cell.sql_into}</code> · Daft DataFrame
        </div>
      )}
    </>
  );
}

/** A slim, hover-revealed "+ insert" row rendered between cells (and after the
 *  last one) so a cell can be inserted at any position, not just appended. */
function InsertBar({ onInsert }: { onInsert: () => void }) {
  return (
    <div className="group/insert relative h-2">
      <div className="pointer-events-none absolute inset-x-0 top-1/2 flex -translate-y-1/2 items-center justify-center opacity-0 transition-opacity group-hover/insert:opacity-100">
        <div className="h-px flex-1 bg-border" />
        <button
          type="button"
          aria-label="Insert cell here"
          onClick={onInsert}
          className="pointer-events-auto mx-2 rounded-full border bg-background p-0.5 text-muted-foreground shadow-sm hover:text-foreground"
        >
          <Plus className="h-3.5 w-3.5" />
        </button>
        <div className="h-px flex-1 bg-border" />
      </div>
    </div>
  );
}

// ── Main page ─────────────────────────────────────────────────────────────────

export default function NotebookPage() {
  const { workspace: ws, id } = useParams<{ workspace: string; id: string }>();

  const kernel = useKernel(ws!, id!);
  const notebook = useNotebook(ws!, id ?? null);
  const runtime = useNotebookRuntime(ws!);
  const updateNotebook = useUpdateNotebook(ws!, id!);
  const addCell = useAddCell(ws!, id!);
  const updateCell = useUpdateCell(ws!, id!);
  const deleteCell = useDeleteCell(ws!, id!);
  const reorderCells = useReorderCells(ws!, id!);
  const active = useActiveExecutions(ws!, id!);
  const engines = runtime.data?.engines ?? [];

  // Connect the kernel eagerly so the first Run is instant (ad-hoc friction
  // fix): don't make the user click Run just to see "Connecting…". Best-effort
  // — a real failure surfaces on the first actual Run, not as a passive-open toast.
  useEffect(() => {
    kernel.connect().catch(() => {});
    // kernel is stable for the page's lifetime (keyed on ws/id remount).
  }, [kernel]);

  const kernelStatus = useKernelConnectionStatus(kernel);
  const [nameDirty, setNameDirty] = useState(false);
  // Tools (outline / data sources) live in an on-demand RIGHT panel, closed by
  // default — the notebook content stays centered and full-width (Deepnote /
  // Databricks pattern), avoiding a second left sidebar next to the app nav.
  const [activePanel, setActivePanel] = useState<NotebookPanel>(null);
  const togglePanel = (p: Exclude<NotebookPanel, null>) =>
    setActivePanel((cur) => (cur === p ? null : p));

  const jumpToCell = (cellId: string) => {
    document
      .getElementById(`cell-${cellId}`)
      ?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

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

  // A cell save debounces ~600ms; a name edit commits only on blur. Warn on
  // refresh/close while either is unpersisted (beforeunload-only; SPA nav
  // flushes cell saves on unmount).
  useUnsavedChangesWarning(pendingCells.size > 0 || nameDirty);

  // ── Active-cell + mode model (S1-3 / S1-4) ────────────────────────────────
  // activeCellId is the selected cell; mode="edit" while its editor has text
  // focus, mode="command" while the keyboard drives the notebook (Jupyter-
  // style j/k/a/b/dd). cellFocus is a per-cell registry of "focus this editor"
  // closures (parallel to the `runners` registry above) so the page can drive
  // focus programmatically from keyboard nav / Shift+Enter.
  const [activeCellId, setActiveCellId] = useState<string | null>(null);
  const [mode, setMode] = useState<"edit" | "command">("command");
  const cellFocus = useRef<Record<string, () => void>>({});
  // Timestamp of the last unpaired "d" press, for the "dd" delete chord.
  const ddRef = useRef<number>(0);
  const sectionRef = useRef<HTMLElement | null>(null);

  const focusCell = (cid: string) => {
    setActiveCellId(cid);
    cellFocus.current[cid]?.();
  };

  // Move the active selection to the adjacent cell. In edit mode (Shift+Enter
  // from within an editor) this focuses the next editor so typing can continue;
  // in command mode (j/k/arrow nav) it just moves the highlight and keeps
  // keyboard focus on the section so further chords keep routing here.
  const selectAdjacent = (cid: string, dir: 1 | -1) => {
    const cells = [...(notebook.data?.cells ?? [])].sort(
      (a, b) => a.cell_order - b.cell_order,
    );
    const ids = cells.map((c) => c.id);
    const next = ids[ids.indexOf(cid) + dir];
    if (!next) return;
    if (mode === "edit") {
      focusCell(next);
    } else {
      setActiveCellId(next);
      sectionRef.current?.focus();
    }
  };

  // Insert a cell at an arbitrary position: add it (server assigns the next
  // cell_order), then reorder so it lands at `atIndex` among the existing cells.
  const insertCell = (cellType: CellType, atIndex: number) => {
    const cells = [...(notebook.data?.cells ?? [])].sort(
      (a, b) => a.cell_order - b.cell_order,
    );
    const maxOrder = cells.reduce((m, c) => Math.max(m, c.cell_order), -1);
    addCell.mutate(
      {
        cell_type: cellType,
        cell_order: maxOrder + 1,
        // Seed a collision-free variable and default to the first enabled engine so
        // Run doesn't emit `%%egret_sql --target  …` (an IPython parse-time error).
        ...(cellType === "sql"
          ? {
              sql_into: defaultSqlInto(cells),
              sql_target: engines.find((e) => e.enabled)?.id,
            }
          : {}),
      },
      {
        onSuccess: (detail) => {
          // The new cell is the highest-order one; move it to `atIndex`.
          const created = [...detail.cells].sort((a, b) => a.cell_order - b.cell_order).at(-1);
          if (!created) return;
          const order = cells.map((c) => c.id).filter((cid) => cid !== created.id);
          order.splice(Math.max(0, Math.min(atIndex, order.length)), 0, created.id);
          reorderCells.mutate(order);
        },
      },
    );
  };

  const handleAddCell = (cellType: CellType = "code") => {
    insertCell(cellType, (notebook.data?.cells ?? []).length);
  };

  // Command-mode keyboard nav must never hijack typing in a real input/select/
  // Monaco (the notebook-name field, the SQL result-variable input, etc.).
  const isEditableTarget = (el: EventTarget | null): boolean => {
    if (!(el instanceof HTMLElement)) return false;
    return (
      el.tagName === "INPUT" ||
      el.tagName === "TEXTAREA" ||
      el.tagName === "SELECT" ||
      el.isContentEditable
    );
  };

  // Jupyter-style command-mode chords, bound to the scrollable section. Only
  // acts when mode==="command" (mode flips to "edit" the instant a cell's
  // editor gains focus) and never over an editable target.
  const onSectionKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (mode !== "command" || !activeCellId || isEditableTarget(e.target)) return;
    const cells = [...(notebook.data?.cells ?? [])].sort(
      (a, b) => a.cell_order - b.cell_order,
    );
    const cid = activeCellId;
    const idx = cells.findIndex((c) => c.id === cid);
    if (idx === -1) return;
    switch (e.key) {
      case "Enter":
        e.preventDefault();
        setMode("edit");
        focusCell(cid);
        break;
      case "ArrowDown":
      case "j":
        e.preventDefault();
        selectAdjacent(cid, 1);
        break;
      case "ArrowUp":
      case "k":
        e.preventDefault();
        selectAdjacent(cid, -1);
        break;
      case "a":
        e.preventDefault();
        insertCell("code", idx); // insert above
        break;
      case "b":
        e.preventDefault();
        insertCell("code", idx + 1); // insert below
        break;
      case "d":
        // "dd" — two d presses within 400ms deletes the active cell.
        if (Date.now() - ddRef.current < 400) {
          e.preventDefault();
          handleDeleteCell(cid);
          ddRef.current = 0;
        } else {
          ddRef.current = Date.now();
        }
        break;
      default:
        break;
    }
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
    if (cellType === "sql") {
      const cells = notebook.data?.cells ?? [];
      const existing = cells.find((c) => c.id === cid);
      updateCell.mutate({
        cid,
        cell_type: cellType,
        ...(existing?.sql_into ? {} : { sql_into: defaultSqlInto(cells) }),
        ...(existing?.sql_target ? {} : { sql_target: (runtime.data?.engines ?? []).find((e) => e.enabled)?.id }),
      });
    } else {
      updateCell.mutate({ cid, cell_type: cellType });
    }
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

  const panelTitle = activePanel === "toc" ? "Outline" : "Data sources";

  return (
    <div className="flex h-full min-h-0">
      <section
        ref={sectionRef}
        tabIndex={0}
        onKeyDown={onSectionKeyDown}
        className="flex min-w-0 flex-1 flex-col overflow-y-auto focus:outline-none"
      >
        <h1 className="sr-only">{nb.name}</h1>
        <NotebookHeader
          ws={ws!}
          name={nb.name}
          isPublic={nb.is_public}
          status={kernelStatus}
          runningAll={runningAll}
          activePanel={activePanel}
          onTogglePanel={togglePanel}
          onRename={(name) => updateNotebook.mutate({ name })}
          onTogglePublic={(v) => updateNotebook.mutate({ is_public: v })}
          onRunAll={runAll}
          onInterrupt={() =>
            kernel.interrupt().catch((e) => toast.error(String((e as Error)?.message ?? e)))
          }
          onRestart={() =>
            kernel.restart().catch((e) => toast.error(String((e as Error)?.message ?? e)))
          }
          onClear={clearAll}
          onDirtyChange={setNameDirty}
        />

        {/* Centered, wide cell column — code-first, so generous width with a
            cap that keeps lines readable on ultra-wide monitors. */}
        <div className="mx-auto flex w-full max-w-7xl flex-col gap-3 px-6 py-6">
          {sortedCells.map((cell, index) => (
            <Fragment key={cell.id}>
              <InsertBar onInsert={() => insertCell("code", index)} />
              <div id={`cell-${cell.id}`} style={{ scrollMarginTop: "6rem" }}>
                <ManagedCell
                  cell={cell}
                  index={index}
                  kernel={kernel}
                  onChangeType={handleChangeType}
                  onDelete={handleDeleteCell}
                  onMoveUp={handleMoveUp}
                  onMoveDown={handleMoveDown}
                  updateCellMutate={updateCell.mutate}
                  runners={runners}
                  engines={engines}
                  onPendingChange={setCellPending}
                  initialRunning={runningSet.has(cell.id)}
                  onRunAbove={index > 0 ? () => runSlice(0, index) : undefined}
                  onRunBelow={
                    index < sortedCells.length - 1
                      ? () => runSlice(index + 1, sortedCells.length)
                      : undefined
                  }
                  active={activeCellId === cell.id}
                  onFocusCell={(cid) => {
                    setActiveCellId(cid);
                    setMode("edit");
                  }}
                  onSelectCell={(cid) => {
                    setActiveCellId(cid);
                    setMode("command");
                    sectionRef.current?.focus();
                  }}
                  registerFocus={(cid, fn) => {
                    cellFocus.current[cid] = fn;
                  }}
                  onSelectNext={(cid) => selectAdjacent(cid, 1)}
                  onInsertBelow={(idx) => insertCell("code", idx + 1)}
                  onEnterCommandMode={() => {
                    setMode("command");
                    sectionRef.current?.focus();
                  }}
                />
              </div>
            </Fragment>
          ))}
          <InsertBar onInsert={() => insertCell("code", sortedCells.length)} />

          <div className="flex items-center gap-2">
            <Button variant="outline" size="sm" onClick={() => handleAddCell("code")} aria-label="Add cell">
              <Plus className="h-4 w-4" />
              Code
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => handleAddCell("markdown")}
              aria-label="Add markdown cell"
            >
              <Plus className="h-4 w-4" />
              Markdown
            </Button>
            <Button variant="outline" size="sm" onClick={() => handleAddCell("sql")} aria-label="Add SQL cell">
              <Plus className="h-4 w-4" />
              SQL
            </Button>
          </div>
        </div>
      </section>

      {/* On-demand tools panel (right). */}
      {activePanel && (
        <aside className="flex w-72 shrink-0 flex-col overflow-y-auto border-l bg-muted/10">
          <div className="sticky top-0 flex items-center justify-between border-b bg-background/95 px-3 py-2 backdrop-blur">
            <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              {panelTitle}
            </span>
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6"
              aria-label="Close panel"
              onClick={() => setActivePanel(null)}
            >
              <X className="h-4 w-4" />
            </Button>
          </div>
          {activePanel === "toc" ? (
            <TableOfContentsPanel cells={sortedCells} onJump={jumpToCell} />
          ) : (
            <DataSourcesPanel ws={ws!} />
          )}
        </aside>
      )}
    </div>
  );
}
