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

export interface VarInfo { name: string; type: string; columns: string[] | null }

/** atob → UTF-8 decode. The kernel introspection payload is base64-encoded so
 *  the `text/plain` repr stays quote-clean regardless of the JSON content. */
function decodeUtf8Base64(b64: string): string {
  const bin = atob(b64);
  const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

/** Hand-rolled Jupyter client over the D1c channels WS. One per open notebook. */
export class KernelConnection {
  private ws: WebSocket | null = null;
  private session = randomId();
  private pending = new Map<string, ExecuteHandlers>(); // execute_request msg_id → handlers
  private pendingCompletes = new Map<string, (r: CompleteResult) => void>(); // complete_request msg_id → resolver
  private pendingInspects = new Map<string, (u: Record<string, any> | undefined) => void>(); // inspect execute_request msg_id → resolver
  private ready: Promise<void> | null = null;
  private _status: KernelStatus = "disconnected";
  private statusListeners = new Set<(s: KernelStatus) => void>();

  // The introspection is a SINGLE Python expression (evaluated via user_expressions,
  // so no def is needed). base64 so the text/plain repr is quote-clean.
  private static readonly VARS_EXPR =
    "__import__('base64').b64encode(__import__('json').dumps([" +
    "{'name': k, 'type': type(v).__name__, 'columns': (list(v.column_names) " +
    "if hasattr(v, 'column_names') and getattr(type(v), '__module__', '').split('.')[0] == 'daft' else None)} " +
    "for k, v in list(get_ipython().user_ns.items()) " +
    "if not k.startswith('_') " +
    "and k not in ('In','Out','exit','quit','get_ipython','open','runtime','me','trino','starrocks','display','write_table') " +
    "and type(v).__name__ not in ('module','function','builtin_function_or_method','method','type','ZMQExitAutocall','MagicsManager')" +
    "], default=str).encode()).decode()";

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
        // Same for in-flight inspect requests: an undefined user_expressions
        // payload short-circuits the resolver's own parsing to `[]`.
        this.pendingInspects.forEach((cb) => cb(undefined));
        this.pendingInspects.clear();
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
    if (m.header?.msg_type === "execute_reply") {
      const cb = this.pendingInspects.get(parent);
      if (cb) { this.pendingInspects.delete(parent); cb(m.content?.user_expressions); return; }
      // else: a normal cell run's execute_reply — currently unused; fall through.
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

  /** Introspect the kernel namespace via a silent `execute_request` +
   *  `user_expressions` (no execution_count pollution, no cell output).
   *  Resolves empty on timeout / no connection / any parse failure — never
   *  throws to the UI. */
  async inspectVariables(): Promise<VarInfo[]> {
    if (!this.ws) return [];
    const msg = newMsg("execute_request", this.session, {
      code: "", silent: true, store_history: false,
      user_expressions: { v: KernelConnection.VARS_EXPR },
    });
    return new Promise((resolve) => {
      const id = msg.header.msg_id;
      const timer = setTimeout(() => { this.pendingInspects.delete(id); resolve([]); }, 3000);
      this.pendingInspects.set(id, (u) => {
        clearTimeout(timer);
        try {
          const raw = u?.v?.data?.["text/plain"] as string | undefined;
          if (!raw) return resolve([]);
          const b64 = raw.replace(/^['"]|['"]$/g, "");
          const json = decodeUtf8Base64(b64);
          resolve(JSON.parse(json) as VarInfo[]);
        } catch { resolve([]); }
      });
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
    this.pendingInspects.forEach((cb) => cb(undefined));
    this.pendingInspects.clear();
    this.setStatus("disconnected");
  }
}
