import { useMemo } from "react";
import { useWorkflowGetFlowSchema } from "@/api/generated/workflow/workflow";
import type { FlowSchema } from "@/components/workflows/editor/flowSchema";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

export type FlowSchemaState = {
  status: "loading" | "ready" | "error";
  schema: FlowSchema | null;
  /** The response's strong `ETag`; `monaco-yaml` is reconfigured only when it changes. */
  etag: string | null;
  error: string | null;
  retry: () => void;
};

/**
 * The workspace's flow schema (spec §7.1). The server answers with a strong
 * `ETag` and `Cache-Control: private, no-cache`, so the browser revalidates
 * with `If-None-Match` and a 304 costs no body; the query itself is kept for
 * the session, since the schema changes only when the workspace's features
 * or the deployment do.
 */
export function useFlowSchema(workspace: string): FlowSchemaState {
  const q = useWorkflowGetFlowSchema(workspace, {
    query: { queryKey: workflowsKeys.schema(workspace), staleTime: Infinity, retry: 1 },
  });
  const { data, isError, error, refetch } = q;
  return useMemo((): FlowSchemaState => {
    const retry = () => void refetch();
    // Error first (FE-STATE-1): a failed refetch must not leave a stale "ready".
    if (isError) {
      return { status: "error", schema: null, etag: null, error: error instanceof Error ? error.message : "The request failed.", retry };
    }
    if (!data) return { status: "loading", schema: null, etag: null, error: null, retry };
    return {
      status: "ready",
      schema: data.data as FlowSchema,
      etag: data.headers.get("etag"),
      error: null,
      retry,
    };
  }, [data, isError, error, refetch]);
}
