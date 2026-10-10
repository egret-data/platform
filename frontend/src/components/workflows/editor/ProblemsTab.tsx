import { useMemo } from "react";
import { CircleCheck, CircleX, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/explore/EmptyState";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { collectProblems, type Problem } from "@/components/workflows/editor/problems";
import { cn } from "@/lib/utils";

const LABEL = { error: "Error", warning: "Warning" } as const;

function where(p: Problem): string {
  if (!p.mapped) return `${p.path} (not found in the source, shown on line 1)`;
  const line = p.path === null && p.source === "server"
    ? `Line ${p.range.startLine}, column ${p.range.startCol}`
    : `Line ${p.range.startLine}`;
  return p.path ? `${p.path} ${line}` : line;
}

function useProblems(ctx: EditorContext) {
  const { source, validation, schemaMarkers } = ctx;
  return useMemo(
    () => collectProblems(source, validation.findings, schemaMarkers),
    [source, validation.findings, schemaMarkers],
  );
}

/**
 * The Problems tab (spec §5.2): the schema's markers and the server's
 * findings, errors first. Selecting one moves the editor there.
 *
 * Its states (FE-STATE-1/2): a skeleton row while a validation is in flight,
 * the request's error with a retry when it failed (never "No problems"),
 * `EmptyState` when the answer was empty.
 */
export function ProblemsTab({ ctx }: { ctx: EditorContext }) {
  const problems = useProblems(ctx);
  const { status, error, retry } = ctx.validation;

  return (
    <div className="flex flex-col gap-1 p-2">
      {status === "error" && (
        <div role="alert" className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/50 px-3 py-2 text-sm">
          <CircleX aria-hidden className="size-4 shrink-0 text-destructive" />
          <span className="font-medium text-foreground">Validation failed.</span>
          <span className="text-muted-foreground">
            {error ?? "The request did not complete."} The list below may be incomplete.
          </span>
          <Button type="button" variant="outline" size="sm" onClick={retry}>
            Retry validation
          </Button>
        </div>
      )}
      {status === "validating" && (
        <div role="status" className="flex items-center gap-2 px-2 py-1.5">
          <Skeleton className="size-4 shrink-0" />
          <Skeleton className="h-4 flex-1" />
          <span className="text-xs text-muted-foreground">Validating…</span>
        </div>
      )}
      {problems.length > 0 ? (
        <ul aria-label="Problems" className="flex flex-col">
          {problems.map((p) => {
            const Icon = p.severity === "error" ? CircleX : TriangleAlert;
            return (
              <li key={p.key}>
                <button
                  type="button"
                  onClick={() => ctx.focusRange(p.range)}
                  className="flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  <Icon
                    aria-hidden
                    className={cn("mt-0.5 size-4 shrink-0", p.severity === "error" ? "text-destructive" : "text-muted-foreground")}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="sr-only">{LABEL[p.severity]}</span>{" "}
                    <span className="text-foreground">{p.message}</span>{" "}
                    <span className="block font-mono text-xs break-all text-muted-foreground tabular-nums">{where(p)}</span>
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      ) : status === "ok" ? (
        <EmptyState icon={CircleCheck} title="No problems" description="The schema and the server found nothing to fix." />
      ) : status === "idle" ? (
        <EmptyState
          icon={CircleCheck}
          title="No problems"
          description="Only the schema is checked here: server validation needs the workflow:manage permission."
        />
      ) : null}
    </div>
  );
}

/** The error count drawn in the Problems tab (spec §5.2). */
export function ProblemsBadge({ ctx }: { ctx: EditorContext }) {
  const errors = useProblems(ctx).filter((p) => p.severity === "error").length;
  if (errors === 0) return null;
  return (
    <span className="rounded-full bg-destructive px-1.5 text-xs text-destructive-foreground tabular-nums">
      <span aria-hidden>{errors}</span>
      <span className="sr-only">{errors === 1 ? ", 1 error" : `, ${errors} errors`}</span>
    </span>
  );
}
