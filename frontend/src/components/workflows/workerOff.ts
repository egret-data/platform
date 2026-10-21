/**
 * What a page says when the server's workflow worker is off (KI-013). The
 * server sends the flag (`worker_enabled`) on the flow, the executions list,
 * an execution and the Run answer: the same flag the trigger routes send, so
 * the trigger banner and `WorkerOffNotice` say this one sentence.
 */
export const WORKER_OFF_TEXT =
  "The workflow worker is off on this server, so executions are created but do not run. An administrator turns it on with workflow.workerEnabled (EGRET_WORKFLOW__WORKER_ENABLED=true).";
