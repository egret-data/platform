import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Info, ListTree, X } from "lucide-react";
import type { ExecutionDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { Button } from "@/components/ui/button";
import { isTerminalState } from "@/components/workflows/executionState";
import { buildGantt } from "@/components/workflows/executions/gantt/ganttModel";
import { GanttRunPanel, GanttView, type GanttRunSlots } from "@/components/workflows/executions/gantt/GanttView";

/**
 * How often a live gantt redraws at most: Kestra's `Gantt.vue` repaints every
 * 40 ms below 50 task runs and every 500 ms above. Here each redraw also waits
 * for an animation frame, so a hidden tab, which gets none, draws nothing.
 */
export const LIVE_REDRAW_MS = { few: 40, many: 500, threshold: 50 } as const;

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
  const live = !isTerminalState(execution.state);
  const minMs = execution.runs.length < LIVE_REDRAW_MS.threshold ? LIVE_REDRAW_MS.few : LIVE_REDRAW_MS.many;
  const now = useGanttClock(live, minMs);
  const model = useMemo(() => buildGantt(execution, now), [execution, now]);
  const selected = parseRun(params.get("run"));
  const selectedRow = selected === null ? undefined : model.rows.find((r) => r.runIx === selected);

  // A pushed entry, so Back walks the selection (FE-PAGE-2).
  const select = (runIx: number | null) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev);
      if (runIx === null) next.delete("run");
      else next.set("run", String(runIx));
      return next;
    });

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
            Some times here are estimated: they were not recorded, so the run's creation and last update stand in. Estimated bars
            are hatched.
          </span>
        </p>
      ) : null}
      <GanttView model={model} selectedRunIx={selected} onSelect={select} />
      {selected === null ? null : selectedRow ? (
        <GanttRunPanel row={selectedRow} onClose={() => select(null)} slots={slots} />
      ) : (
        // Its sentence is also this module's marker in `scripts/assert-workflows-lazy.mjs`.
        <div className="mt-4 flex flex-wrap items-center gap-3 rounded-md border p-4 text-sm">
          <span>Task run #{selected} is not in this execution. The link may be from another execution.</span>
          <Button variant="outline" size="sm" onClick={() => select(null)}>
            <X aria-hidden />
            Close
          </Button>
        </div>
      )}
    </div>
  );
}
