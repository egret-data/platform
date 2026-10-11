import { useMemo } from "react";
import { Link, useLocation, useSearchParams } from "react-router-dom";
import { FileOutput } from "lucide-react";
import type { ExecutionDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { isTerminalState } from "@/components/workflows/executionState";
import { OutputsTree } from "@/components/workflows/executions/outputs/OutputsTree";

/**
 * The execution's Outputs tab (W7b spec §8): on the left, the task runs that
 * have outputs, in `run_ix` order; on the right, the selected run's outputs
 * tree. The selection is `?run=<run_ix>`, as on the Gantt and Logs tabs
 * (FE-PAGE-2), and each run is a link, so it is pushed and Back walks it.
 * Without `?run=` the first run is shown. Kestra's
 * `ExecutionVariableExplorer.vue`: its "Tasks outputs" list beside the tree;
 * its variables, trigger and flow-outputs sections are left out (the Overview
 * shows the inputs and trigger, and egret has no execution-level outputs,
 * spec §10).
 */
export default function ExecutionOutputsTab({ ws, execution }: { ws: string; execution: ExecutionDto }) {
  const [params] = useSearchParams();
  const { pathname } = useLocation();
  const runs = useMemo(
    () => execution.runs.filter((r) => Object.keys(r.outputs ?? {}).length > 0).sort((a, b) => a.run_ix - b.run_ix),
    [execution.runs],
  );
  const raw = params.get("run");
  const selected = raw === null ? runs[0] : /^\d+$/.test(raw) ? runs.find((r) => r.run_ix === Number(raw)) : undefined;

  if (runs.length === 0) {
    return (
      <EmptyState
        icon={FileOutput}
        title="No outputs"
        description={
          isTerminalState(execution.state)
            ? "No task run of this execution has outputs."
            : "No task run has outputs yet. They appear here as tasks finish."
        }
      />
    );
  }

  const hrefFor = (runIx: number) => {
    const next = new URLSearchParams(params);
    next.set("run", String(runIx));
    return `${pathname}?${next.toString()}`;
  };

  return (
    <div className="grid gap-4 md:grid-cols-[minmax(12rem,16rem)_minmax(0,1fr)]">
      <nav aria-label="Task runs with outputs" className="min-w-0">
        <ul className="grid gap-0.5">
          {runs.map((r) => {
            const current = r.id === selected?.id;
            return (
              <li key={r.id}>
                <Link
                  to={hrefFor(r.run_ix)}
                  aria-current={current ? "true" : undefined}
                  className={`flex min-w-0 items-baseline gap-2 rounded-md px-2 py-1 text-sm hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none ${
                    current ? "bg-muted font-medium" : ""
                  }`}
                >
                  <span className="truncate font-mono text-xs">{r.task_id}</span>
                  {r.value ? <span className="truncate font-mono text-xs text-muted-foreground">{r.value}</span> : null}
                  <span className="ml-auto tabular-nums text-xs text-muted-foreground">#{r.run_ix}</span>
                </Link>
              </li>
            );
          })}
        </ul>
      </nav>
      <div className="min-w-0">
        {selected ? (
          <>
            <h2 className="mb-2 truncate font-mono text-base font-semibold">
              {selected.task_id}
              {selected.value ? (
                <>
                  {" "}
                  <span className="font-normal text-muted-foreground">{selected.value}</span>
                </>
              ) : null}{" "}
              <span className="font-normal tabular-nums text-muted-foreground">#{selected.run_ix}</span>
            </h2>
            <OutputsTree key={selected.id} ws={ws} executionId={execution.id} run={selected} />
          </>
        ) : (
          // Also this module's marker in `scripts/assert-workflows-lazy.mjs`.
          <p className="text-sm text-muted-foreground">
            Task run #{raw} has no outputs in this execution. Pick a run from the list; the link may be from another execution.
          </p>
        )}
      </div>
    </div>
  );
}
