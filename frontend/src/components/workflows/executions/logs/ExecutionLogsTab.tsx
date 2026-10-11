import { useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import type { ExecutionDto, LogLevel } from "@/api/generated/model";
import { LogLevel as LOG_LEVELS } from "@/api/generated/model";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { LogView } from "@/components/workflows/executions/logs/LogView";
import type { LogFilters } from "@/components/workflows/executions/logs/useLogFollow";

/**
 * The execution's Logs tab (W7b spec §7): the filters, then the log view.
 * The filters live in the URL (FE-PAGE-2) as `?level=`, `?task=` and `?run=`
 * (a run's `run_ix`, as on the Gantt tab), and each change pushes an entry, so
 * Back walks them. Kestra's `executions/Logs.vue`: a filter bar with the
 * minimum level, then the lines grouped by task run.
 *
 * A value the execution does not have (a task or run from another execution's
 * link) is ignored rather than sent, and the run filter is then said to be
 * unknown.
 */
const LEVEL_OPTIONS = (Object.values(LOG_LEVELS) as LogLevel[]).map((l) => ({
  value: l,
  label: l === "ERROR" ? "ERROR only" : `${l} and above`,
}));

const isLevel = (v: string | null): v is LogLevel => v !== null && (Object.values(LOG_LEVELS) as string[]).includes(v);

export default function ExecutionLogsTab({ ws, execution }: { ws: string; execution: ExecutionDto }) {
  const [params, setParams] = useSearchParams();
  const rawLevel = params.get("level");
  const rawTask = params.get("task");
  const rawRun = params.get("run");

  const runs = useMemo(() => [...execution.runs].sort((a, b) => a.run_ix - b.run_ix), [execution.runs]);
  const taskIds = useMemo(() => [...new Set(runs.map((r) => r.task_id))], [runs]);

  const level = isLevel(rawLevel) ? rawLevel : undefined;
  const task = rawTask !== null && taskIds.includes(rawTask) ? rawTask : undefined;
  const run = rawRun !== null && /^\d+$/.test(rawRun) ? runs.find((r) => r.run_ix === Number(rawRun)) : undefined;
  const unknownRun = rawRun !== null && run === undefined;

  const filters: LogFilters = useMemo(
    () => ({ minLevel: level, taskId: task, taskRunId: run?.id }),
    [level, task, run?.id],
  );
  const active = Boolean(level || task || run);

  const set = (key: "level" | "task" | "run", value: string | undefined) =>
    setParams((prev) => {
      const next = new URLSearchParams(prev);
      if (value === undefined) next.delete(key);
      else next.set(key, value);
      // A run of another task would select nothing: picking a task clears it.
      if (key === "task") next.delete("run");
      return next;
    });

  const clear = () =>
    setParams((prev) => {
      const next = new URLSearchParams(prev);
      next.delete("level");
      next.delete("task");
      next.delete("run");
      return next;
    });

  const runOptions = runs
    .filter((r) => !task || r.task_id === task)
    .map((r) => ({ value: String(r.run_ix), label: `${r.task_id} #${r.run_ix}${r.value ? ` (${r.value})` : ""}` }));

  return (
    <div className="flex min-h-0 flex-col">
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <FacetSelect
          label="Level"
          value={level ?? FACET_ALL}
          onChange={(v) => set("level", v === FACET_ALL ? undefined : v)}
          options={LEVEL_OPTIONS}
        />
        <FacetSelect
          label="Task"
          value={task ?? FACET_ALL}
          onChange={(v) => set("task", v === FACET_ALL ? undefined : v)}
          options={taskIds.map((t) => ({ value: t, label: t }))}
        />
        <FacetSelect
          label="Task run"
          value={run ? String(run.run_ix) : FACET_ALL}
          onChange={(v) => set("run", v === FACET_ALL ? undefined : v)}
          options={runOptions}
        />
      </div>
      {unknownRun ? (
        <p className="mb-3 text-sm text-muted-foreground">
          Task run #{rawRun} is not in this execution, so every run is shown. The link may point to another execution.
        </p>
      ) : null}
      <LogView
        ws={ws}
        executionId={execution.id}
        filters={filters}
        runs={execution.runs}
        filtersActive={active}
        onClearFilters={clear}
        heightClass="h-[calc(100vh-22rem)] min-h-80"
        label="Execution logs"
      />
    </div>
  );
}
