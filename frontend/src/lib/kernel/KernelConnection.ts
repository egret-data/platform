import {
  kernelConnect,
  kernelKernelInterrupt,
  kernelKernelRestart,
} from "@/api/generated/kernel/kernel";
import { newMsg, iopubToOutput, randomId, type CellOutput } from "./protocol";

export type KernelStatus =
  | "idle"
  | "busy"
  | "spawning"
  | "connected"
  | "disconnected"
  | "connecting"
  | "interrupting"
  | "restarting"
  | "error"
  | "dead";

export interface ExecuteHandlers {
  onOutput: (o: CellOutput) => void;
  onStatus: (s: "busy" | "idle") => void;
  onExecutionCount: (n: number) => void;
}

export interface CompleteResult {
  matches: string[];
  cursorStart: number;
  cursorEnd: number;
}

/** Hand-rolled Jupyter client over the D1c channels WS. One per open notebook. */
export class KernelConnection {
  private ws: WebSocket | null = null;
  private session = randomId();
  private pending = new Map<string, ExecuteHandlers>(); // execute_request msg_id → handlers
  private pendingCompletes = new Map<string, (r: CompleteResult) => void>(); // complete_request msg_id → resolver
  private ready: Promise<void> | null = null;
  private _status: KernelStatus = "disconnected";
  private statusListeners = new Set<(s: KernelStatus) => void>();

  constructor(private ws_slug: string, private nb: string) {}

  get status(): KernelStatus {
    return this._status;
  }

  /** Subscribe to connection-status changes. Fires immediately with the
   *  current value; returns an unsubscribe function. */
  onStatusChange(cb: (s: KernelStatus) => void): () => void {
    this.statusListeners.add(cb);
    // Isolate the immediate fire too, so a throwing subscriber can't break
    // the caller that is registering it.
    try { cb(this._status); } catch (e) { console.error("kernel status listener threw", e); }
    return () => {
      this.statusListeners.delete(cb);
    };
  }

  private setStatus(s: KernelStatus): void {
    if (this._status === s) return;
    this._status = s;
    // Snapshot so a listener that (un)subscribes during dispatch doesn't
    // double-fire or mutate the set mid-iteration; isolate throws so one bad
    // subscriber can't break connect()/message handling.
    for (const cb of Array.from(this.statusListeners)) {
      try { cb(s); } catch (e) { console.error("kernel status listener threw", e); }
    }
  }

  private path(suffix: string) { return `/ws/${this.ws_slug}/notebooks/${this.nb}/kernel${suffix}`; }

  async connect(): Promise<void> {
    if (this.ready) return this.ready;
    this.ready = (async () => {
      this.setStatus("connecting");
      try {
        // ensure the kernel (retries while spawning)
        let ready = false;
        for (let i = 0; i < 30; i++) {
          const body = ((await kernelConnect(this.ws_slug, this.nb)).data ?? {}) as {
            phase?: string;
          };
          if (body.phase === "ready" || body.phase === undefined) { ready = true; break; }
          await new Promise((res) => setTimeout(res, 1000));
        }
        if (!ready) throw new Error("kernel did not become ready after 30 s");
        await this.openSocket();
      } catch (e) {
        this.ready = null; // allow a later retry to re-attempt
        this.setStatus("error");
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
      ws.onopen = () => { this.setStatus("connected"); resolve(); };
      ws.onclose = () => {
        this.setStatus("disconnected");
        this.ws = null;
        // Unexpected close: flush pending handlers so no cell stays stuck-running.
        this.pending.forEach((h) => {
          h.onOutput({ type: "error", ename: "KernelError", evalue: "kernel connection closed", traceback: [] });
          h.onStatus("idle"); // resets the cell's running state
        });
        this.pending.clear();
        // Same for in-flight completion requests: no promise should hang forever.
        this.pendingCompletes.forEach((resolve) => resolve({ matches: [], cursorStart: 0, cursorEnd: 0 }));
        this.pendingCompletes.clear();
      };
      ws.onerror = () => { this.setStatus("error"); reject(new Error("kernel WS error")); };
      ws.onmessage = (e) => this.onMessage(String((e as MessageEvent).data));
    });
  }

  private onMessage(raw: string) {
    let m: any;
    try { m = JSON.parse(raw); } catch { return; }
    const parent = m.parent_header?.msg_id;
    if (!parent) return;
    if (m.header?.msg_type === "complete_reply") {
      const cb = this.pendingCompletes.get(parent);
      if (cb) {
        this.pendingCompletes.delete(parent);
        const content = m.content ?? {};
        cb({ matches: content.matches ?? [], cursorStart: content.cursor_start ?? 0, cursorEnd: content.cursor_end ?? 0 });
      }
      return;
    }
    const h = this.pending.get(parent);
    if (!h) return;
    const t = m.header?.msg_type;
    if (t === "execute_input") { const n = m.content?.execution_count; if (typeof n === "number") h.onExecutionCount(n); return; }
    if (t === "status") {
      const s = m.content?.execution_state;
      if (s === "busy" || s === "idle") { h.onStatus(s); this.setStatus(s); }
      if (s === "idle") this.pending.delete(parent);
      return;
    }
    const out = iopubToOutput(t, m.content ?? {});
    if (out) h.onOutput(out);
  }

  execute(cellId: string, code: string, handlers: ExecuteHandlers): void {
    if (!this.ws) throw new Error("kernel not connected");
    const msg = newMsg("execute_request", this.session, { code, silent: false, store_history: true, stop_on_error: true }, { egret_cell_id: cellId });
    this.pending.set(msg.header.msg_id, handlers);
    this.ws.send(JSON.stringify(msg));
  }

  /** Ask the kernel to complete `code` at byte offset `cursorPos`. Resolves with
   *  matches; resolves empty on timeout / no connection (never throws to the UI).
   *  NOTE: `cursor_pos` is a Jupyter *codepoint* offset, while Monaco's
   *  `model.getOffsetAt` is UTF-16; these agree for ASCII code (the common case)
   *  but can diverge for code containing astral-plane characters — acceptable
   *  for v1. */
  async complete(code: string, cursorPos: number): Promise<CompleteResult> {
    if (!this.ws) return { matches: [], cursorStart: cursorPos, cursorEnd: cursorPos };
    const msg = newMsg("complete_request", this.session, { code, cursor_pos: cursorPos });
    return new Promise((resolve) => {
      const id = msg.header.msg_id;
      const timer = setTimeout(() => { this.pendingCompletes.delete(id); resolve({ matches: [], cursorStart: cursorPos, cursorEnd: cursorPos }); }, 3000);
      this.pendingCompletes.set(id, (r) => { clearTimeout(timer); resolve(r); });
      this.ws!.send(JSON.stringify(msg));
    });
  }

  /** Interrupt the running kernel (best-effort; requires a connected kernel). */
  async interrupt(): Promise<void> {
    this.setStatus("interrupting");
    try {
      await kernelKernelInterrupt(this.ws_slug, this.nb);
    } finally {
      if (this._status === "interrupting") this.setStatus(this.ws ? "idle" : "disconnected");
    }
  }

  /** Restart the kernel. The channels WS drops → onclose flushes pending (clears running). */
  async restart(): Promise<void> {
    this.setStatus("restarting");
    try {
      await kernelKernelRestart(this.ws_slug, this.nb);
    } finally {
      if (this._status === "restarting") this.setStatus(this.ws ? "connected" : "disconnected");
    }
  }

  disconnect(): void {
    this.ws?.close();
    this.ws = null;
    this.ready = null;
    this.pending.clear();
    this.pendingCompletes.forEach((resolve) => resolve({ matches: [], cursorStart: 0, cursorEnd: 0 }));
    this.pendingCompletes.clear();
    this.setStatus("disconnected");
  }
}
