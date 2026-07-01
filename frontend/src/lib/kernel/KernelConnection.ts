import { apiFetch } from "@/api/client";
import { newMsg, iopubToOutput, type CellOutput } from "./protocol";

export type KernelStatus = "idle" | "busy" | "spawning" | "connected" | "disconnected";

export interface ExecuteHandlers {
  onOutput: (o: CellOutput) => void;
  onStatus: (s: "busy" | "idle") => void;
  onExecutionCount: (n: number) => void;
}

/** Hand-rolled Jupyter client over the D1c channels WS. One per open notebook. */
export class KernelConnection {
  private ws: WebSocket | null = null;
  private session = crypto.randomUUID();
  private pending = new Map<string, ExecuteHandlers>(); // execute_request msg_id → handlers
  private ready: Promise<void> | null = null;

  constructor(private ws_slug: string, private nb: string) {}

  private path(suffix: string) { return `/ws/${this.ws_slug}/notebooks/${this.nb}/kernel${suffix}`; }

  async connect(): Promise<void> {
    if (this.ready) return this.ready;
    this.ready = (async () => {
      try {
        // ensure the kernel (retries while spawning)
        let ready = false;
        for (let i = 0; i < 30; i++) {
          const r = await apiFetch(this.path("/connect"), { method: "POST" });
          const body = await r.json().catch(() => ({}));
          if (body.phase === "ready" || body.phase === undefined) { ready = true; break; }
          await new Promise((res) => setTimeout(res, 1000));
        }
        if (!ready) throw new Error("kernel did not become ready after 30 s");
        await this.openSocket();
      } catch (e) {
        this.ready = null; // allow a later retry to re-attempt
        throw e;
      }
    })();
    return this.ready;
  }

  private openSocket(): Promise<void> {
    return new Promise((resolve, reject) => {
      const proto = location.protocol === "https:" ? "wss:" : "ws:";
      const url = `${proto}//${location.host}/api${this.path("/channels")}`;
      const ws = new WebSocket(url);
      this.ws = ws;
      ws.onopen = () => resolve();
      ws.onclose = () => { this.ws = null; };
      ws.onerror = () => reject(new Error("kernel WS error"));
      ws.onmessage = (e) => this.onMessage(String((e as MessageEvent).data));
    });
  }

  private onMessage(raw: string) {
    let m: any;
    try { m = JSON.parse(raw); } catch { return; }
    const parent = m.parent_header?.msg_id;
    if (!parent) return;
    const h = this.pending.get(parent);
    if (!h) return;
    const t = m.header?.msg_type;
    if (t === "execute_input") { const n = m.content?.execution_count; if (typeof n === "number") h.onExecutionCount(n); return; }
    if (t === "status") { const s = m.content?.execution_state; if (s === "busy" || s === "idle") h.onStatus(s); if (s === "idle") this.pending.delete(parent); return; }
    const out = iopubToOutput(t, m.content ?? {});
    if (out) h.onOutput(out);
  }

  execute(cellId: string, code: string, handlers: ExecuteHandlers): void {
    if (!this.ws) throw new Error("kernel not connected");
    const msg = newMsg("execute_request", this.session, { code, silent: false, store_history: true, stop_on_error: true }, { egret_cell_id: cellId });
    this.pending.set(msg.header.msg_id, handlers);
    this.ws.send(JSON.stringify(msg));
  }

  disconnect(): void { this.ws?.close(); this.ws = null; this.ready = null; this.pending.clear(); }
}
