import { useQueryClient } from "@tanstack/react-query";
import { ApiError } from "@/api/client";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** Re-reads the execution and the executions list: what every action on the execution page does after the server answers. */
export function useRefreshExecution(ws: string, executionId: string): () => void {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.execution(executionId) });
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.executions(ws) });
  };
}

/**
 * The shape of the restart, change-status and change-state actions: call, and on `expected` refresh
 * and report; on a coded 409 that `refusal` has a sentence for, refresh (the page may be stale) and
 * throw that sentence for the dialog to show (FE-ACTION-3); anything else is rethrown as it is.
 */
export async function runExecutionAction<R extends { status: number }>({
  call,
  expected,
  failure,
  refresh,
  refusal,
  onSuccess,
}: {
  call: () => Promise<R>;
  expected: number;
  failure: string;
  refresh: () => void;
  refusal: (code: string | undefined) => string | null;
  onSuccess: () => void;
}): Promise<void> {
  try {
    const res = await call();
    if (res.status !== expected) throw new Error(failure);
    refresh();
    onSuccess();
  } catch (e) {
    const sentence = e instanceof ApiError ? refusal(e.code) : null;
    if (sentence) {
      refresh();
      throw new Error(sentence, { cause: e });
    }
    throw e;
  }
}
