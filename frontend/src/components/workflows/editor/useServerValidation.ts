import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ApiError } from "@/api/client";
import { workflowValidateFlow, workflowValidateUnsavedFlow } from "@/api/generated/workflow/workflow";
import type { ValidationReportDto } from "@/api/generated/model";
import { PARSE_ERROR, type ServerFinding } from "@/components/workflows/editor/problems";

/** Spec §5.2: the server validates 600 ms after the last edit. */
export const VALIDATE_DEBOUNCE_MS = 600;

export type ServerValidation = {
  /**
   * `idle` when validation is off (a read-only viewer: the route needs
   * `workflow:manage`); `validating` from an edit until its answer lands;
   * `ok` with the findings; `error` when the request failed, which is never
   * reported as an empty list (FE-STATE-1).
   */
  status: "idle" | "validating" | "ok" | "error";
  /** The last answer's findings; kept while the next one is pending, so markers do not flicker. */
  findings: ServerFinding[];
  error: string | null;
  retry: () => void;
};

/**
 * A YAML parse error is a 400 whose message ends `(line L, column C)`
 * (`validate_flow_source` in `crates/egret-workflow/src/routes.rs`), both
 * 1-based (serde_norway's `Location`). It becomes one finding there.
 */
export function parseErrorFinding(message: string): ServerFinding {
  const m = /\(line (\d+), column (\d+)\)\s*$/.exec(message);
  return {
    severity: "error",
    code: PARSE_ERROR,
    message,
    path: "",
    ...(m ? { at: { line: Number(m[1]), col: Number(m[2]) } } : {}),
  };
}

function fromReport(report: ValidationReportDto): ServerFinding[] {
  return [
    ...report.errors.map((f) => ({ severity: "error" as const, ...f })),
    ...report.warnings.map((f) => ({ severity: "warning" as const, ...f })),
  ];
}

type Answer = { key: string; status: "ok" | "error"; findings: ServerFinding[]; error: string | null };

/**
 * Validates `source` on the server as it is edited (spec §5.2): 600 ms after
 * the last change, through `POST …/flows/{flowId}/validate` for a saved flow
 * or `POST …/flows/validate` without one. A newer request aborts the older,
 * and an answer that arrives for an older source is dropped.
 *
 * The generated request functions are called directly rather than through
 * their mutation hooks: cancelling needs a signal per request, which a
 * mutation hook cannot be given.
 */
export function useServerValidation(
  workspace: string,
  source: string,
  flowId?: string,
  { enabled = true }: { enabled?: boolean } = {},
): ServerValidation {
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [attempt, setAttempt] = useState(0);
  const immediate = useRef(false);
  const key = JSON.stringify([workspace, flowId ?? null, attempt, source]);

  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    const { signal } = controller;
    const delay = immediate.current ? 0 : VALIDATE_DEBOUNCE_MS;
    immediate.current = false;
    const timer = setTimeout(() => {
      const body = { source };
      const request = flowId
        ? workflowValidateFlow(workspace, flowId, body, { signal })
        : workflowValidateUnsavedFlow(workspace, body, { signal });
      request.then(
        (res) => {
          if (signal.aborted) return;
          if (res.status === 200) setAnswer({ key, status: "ok", findings: fromReport(res.data), error: null });
          else setAnswer({ key, status: "error", findings: [], error: res.data.error });
        },
        (err: unknown) => {
          if (signal.aborted) return;
          if (err instanceof ApiError && err.status === 400) {
            setAnswer({ key, status: "ok", findings: [parseErrorFinding(err.message)], error: null });
          } else {
            const message = err instanceof Error && err.message ? err.message : "The request did not complete.";
            setAnswer({ key, status: "error", findings: [], error: message });
          }
        },
      );
    }, delay);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
    // `key` changes with each of the others; they are listed for the linter.
  }, [key, enabled, workspace, flowId, source]);

  const retry = useCallback(() => {
    immediate.current = true;
    setAttempt((n) => n + 1);
  }, []);

  const status: ServerValidation["status"] = !enabled
    ? "idle"
    : !answer || answer.key !== key ? "validating" : answer.status;
  const findings = enabled && answer ? answer.findings : NO_FINDINGS;
  const error = status === "error" ? answer?.error ?? null : null;
  return useMemo(() => ({ status, findings, error, retry }), [status, findings, error, retry]);
}

const NO_FINDINGS: ServerFinding[] = [];
