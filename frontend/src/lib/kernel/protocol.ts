// Jupyter Messaging Protocol (subset) + the egret CellOutput contract (matches
// the backend E-P1/E-P2 `last_output` shape: serde tag "type", snake_case).
export type CellOutput =
  | { type: "stream"; name: string; text: string }
  | { type: "execute_result"; data: Record<string, unknown> }
  | { type: "display_data"; data: Record<string, unknown> }
  | { type: "error"; ename: string; evalue: string; traceback: string[] }
  | { type: "truncated"; dropped: number };

export interface JupyterMsg {
  header: { msg_id: string; msg_type: string; session: string; username: string; version: string };
  parent_header: { msg_id?: string };
  metadata: Record<string, unknown>;
  content: Record<string, unknown>;
  channel: string;
}

export function newMsg(msgType: string, session: string, content: Record<string, unknown>, metadata: Record<string, unknown> = {}): JupyterMsg {
  return {
    header: { msg_id: crypto.randomUUID(), msg_type: msgType, session, username: "egret", version: "5.3" },
    parent_header: {},
    metadata,
    content,
    channel: "shell",
  };
}

/** Map an iopub message to a CellOutput, or null if it isn't output.
 *  Note: the `"truncated"` CellOutput variant is a recorder-side synthetic
 *  (E-P2) — it is never emitted as a live iopub message, so it is not mapped here. */
export function iopubToOutput(msgType: string, content: Record<string, any>): CellOutput | null {
  switch (msgType) {
    case "stream": return { type: "stream", name: content.name ?? "stdout", text: content.text ?? "" };
    case "execute_result": return { type: "execute_result", data: content.data ?? {} };
    case "display_data": return { type: "display_data", data: content.data ?? {} };
    case "error": return { type: "error", ename: content.ename ?? "", evalue: content.evalue ?? "", traceback: content.traceback ?? [] };
    default: return null;
  }
}
