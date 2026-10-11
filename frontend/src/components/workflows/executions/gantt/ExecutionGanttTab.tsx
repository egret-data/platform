import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Info, ListTree, X } from "lucide-react";
import type { ExecutionDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { Button } from "@/components/ui/button";
import { isTerminalState } from "@/components/workflows/executionState";
import { buildGantt } from "@/components/workflows/executions/gantt/ganttModel";
import {
  GanttRunPanel,
  GanttView,
  type GanttRunSlots,
  type GanttViewHandle,
} from "@/components/workflows/executions/gantt/GanttView";

/**
 * How often a live gantt redraws at most: Kestra's `Gantt.vue` repaints every
 * 40 ms below 50 task runs and every 500 ms above. Here each redraw also waits
 * for an animation frame, so a hidden tab, which gets none, draws nothing.
 * Under `prefers-reduced-motion` the bars step once a second instead of
 * growing smoothly (FE-A11Y-5).
 */
export const LIVE_REDRAW_MS = { few: 40, many: 500, threshold: 50, reducedMotion: 1000 } as const;

const REDUCED_MOTION = "(prefers-reduced-motion: reduce)";

/** Whether the viewer asked for reduced motion, kept current. */
function usePrefersReducedMotion(): boolean {
  const query = () => (typeof window.matchMedia === "function" ? window.matchMedia(REDUCED_MOTION) : null);
  const [reduced, setReduced] = useState(() => query()?.matches ?? false);
  useEffect(() => {
    const mq = query();
    if (!mq || typeof mq.addEventListener !== "function") return;
    const onChange = () => setReduced(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return reduced;
}

/**
 * The clock live bars are drawn to. While `live`, it advances on animation
 * frames, at most once per `minMs`, and only while the document is visible:
 * `visibilitychange` to hidden cancels the frame loop, back to visible
 * restarts it. Not live, it stands still.
 */
export function useGanttClock(live: boolean, minMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    let frame = 0;
    let last = 0;
    const draw = () => {
      const t = Date.now();
      if (t - last >= minMs) {
        last = t;
        setNow(t);
      }
      frame = requestAnimationFrame(draw);
    };
    const start = () => {
      if (frame === 0 && document.visibilityState !== "hidden") frame = requestAnimationFrame(draw);
    };
    const stop = () => {
      cancelAnimationFrame(frame);
      frame = 0;
    };
    const onVisibility = () => (document.visibilityState === "hidden" ? stop() : start());
    document.addEventListener("visibilitychange", onVisibility);
    start();
    return () => {
      stop();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [live, minMs]);
  return now;
}

const parseRun = (raw: string | null): number | null => (raw !== null && /^\d+$/.test(raw) ? Number(raw) : null);

/**
 * The execution's Gantt tab (W7b spec §6): one row per task run, nested by
 * `parent_ix`, each attempt a segment. The selected run is `?run=<run_ix>`
 * (FE-PAGE-2), and its panel opens below the chart with slots for the run's
 * logs and outputs (Tasks 5 and 6 of W7b).
 */
export default function ExecutionGanttTab({ execution, slots }: { execution: ExecutionDto; slots?: GanttRunSlots }) {
  const [params, setParams] = useSearchParams();
  const view = useRef<GanttViewHandle>(null);
  const reducedMotion = usePrefersReducedMotion();
  const live = !isTerminalState(execution.state);
  const minMs = reducedMotion
    ? LIVE_REDRAW_MS.reducedMotion
    : execution.runs.length < LIVE_REDRAW_MS.threshold
      ? LIVE_REDRAW_MS.few
      : LIVE_REDRAW_MS.many;
  const now = useGanttClock(live, minMs);
  const model = useMemo(() => buildGantt(execution, now), [execution, now]);
  const raw = params.get("run");
  const selected = parseRun(raw);
  const selectedRow = selected === null ? undefined : model.rows.find((r) => r.runIx === selected);
  const selectedRun = selectedRow?.run;

  // A pushed entry, so Back walks the selection (FE-PAGE-2); none when nothing changes,
  // or Back would seem to do nothing.
  const select = useCallback(
    (runIx: number | null) => {
      if ((runIx === null ? null : String(runIx)) === raw) return;
      setParams((prev) => {
        const next = new URLSearchParams(prev);
        if (runIx === null) next.delete("run");
        else next.set("run", String(runIx));
        return next;
      });
    },
    [raw, setParams],
  );
  // Closing from the panel puts focus back on the run's row rather than on `body`.
  const close = useCallback(() => {
    select(null);
    view.current?.focusRow(selected);
  }, [select, selected]);

  // The slots render once per run, not on every clock tick: the DTO keeps its
  // identity until the next poll, and an unchanged element is not re-rendered.
  const logsSlot = slots?.logs;
  const outputsSlot = slots?.outputs;
  const logs = useMemo(() => (selectedRun && logsSlot ? logsSlot(selectedRun) : undefined), [selectedRun, logsSlot]);
  const outputs = useMemo(() => (selectedRun && outputsSlot ? outputsSlot(selectedRun) : undefined), [selectedRun, outputsSlot]);

  if (execution.runs.length === 0) {
    return (
      <EmptyState
        icon={ListTree}
        title="No task runs yet"
        description={isTerminalState(execution.state) ? "The execution ended before any task ran." : "Bars appear here as tasks start."}
      />
    );
  }

  return (
    <div>
      {model.estimated ? (
        <p data-testid="gantt-estimated-note" className="mb-3 flex items-start gap-2 text-sm text-muted-foreground">
          <Info aria-hidden className="mt-0.5 size-4 shrink-0" />
          <span>
            Some times here are estimated because they were not recorded: a start is taken from the run's first attempt, or
            its creation, and an end from its last update. Estimated bars are hatched.
          </span>
        </p>
      ) : null}
      <GanttView ref={view} model={model} selectedRunIx={selected} onSelect={select} />
      {selected === null ? null : selectedRow ? (
        // Keyed by run: a slot's own state (a log cursor) starts afresh for each run.
        <GanttRunPanel key={selectedRow.run.id} row={selectedRow} onClose={close} logs={logs} outputs={outputs} />
      ) : (
        // Its sentence is also this module's marker in `scripts/assert-workflows-lazy.mjs`.
        <div className="mt-4 flex flex-wrap items-center gap-3 rounded-md border p-4 text-sm">
          <span>Task run #{selected} is not in this execution. The link may be from another execution.</span>
          <Button variant="outline" size="sm" onClick={close}>
            <X aria-hidden />
            Close
          </Button>
        </div>
      )}
    </div>
  );
}
