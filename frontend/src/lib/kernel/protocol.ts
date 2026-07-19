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

/** UUIDv4 that also works in non-secure contexts (plain http, non-localhost)
 *  where `crypto.randomUUID` is undefined — the in-cluster SPA is served over
 *  http, so we must not depend on the secure-context-only API. */
export function randomId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c && typeof c.randomUUID === "function") return c.randomUUID();
  const b = new Uint8Array(16);
  if (c && typeof c.getRandomValues === "function") c.getRandomValues(b);
  else for (let i = 0; i < 16; i++) b[i] = Math.floor(Math.random() * 256);
  b[6] = (b[6] & 0x0f) | 0x40; // version 4
  b[8] = (b[8] & 0x3f) | 0x80; // variant
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0"));
  return `${h[0]}${h[1]}${h[2]}${h[3]}-${h[4]}${h[5]}-${h[6]}${h[7]}-${h[8]}${h[9]}-${h[10]}${h[11]}${h[12]}${h[13]}${h[14]}${h[15]}`;
}

export function newMsg(msgType: string, session: string, content: Record<string, unknown>, metadata: Record<string, unknown> = {}): JupyterMsg {
  return {
    header: { msg_id: randomId(), msg_type: msgType, session, username: "egret", version: "5.3" },
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
