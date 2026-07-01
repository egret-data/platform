import { describe, it, expect, vi, beforeEach } from "vitest";
import { KernelConnection } from "../KernelConnection";

class MockWS {
  static last: MockWS | null = null;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  sent: string[] = [];
  readyState = 0;
  constructor(public url: string) { MockWS.last = this; setTimeout(() => { this.readyState = 1; this.onopen?.(); }, 0); }
  send(d: string) { this.sent.push(d); }
  close() { this.readyState = 3; this.onclose?.(); }
}

beforeEach(() => {
  vi.stubGlobal("WebSocket", MockWS as unknown as typeof WebSocket);
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ phase: "ready" }), { status: 200 })));
  vi.stubGlobal("crypto", { randomUUID: () => "uuid-" + Math.random().toString(36).slice(2) });
});

describe("KernelConnection", () => {
  it("connects, sends execute_request with egret_cell_id, routes iopub by parent msg_id", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    const outputs: any[] = [];
    let finalStatus = "";
    kc.execute("cellA", "print(1)", {
      onOutput: (o) => outputs.push(o),
      onStatus: (s) => { finalStatus = s; },
      onExecutionCount: () => {},
    });
    const ws = MockWS.last!;
    const sent = JSON.parse(ws.sent[0]);
    expect(sent.header.msg_type).toBe("execute_request");
    expect(sent.metadata.egret_cell_id).toBe("cellA");
    const parent = sent.header.msg_id;
    const iopub = (t: string, c: any) => ws.onmessage!({ data: JSON.stringify({ header: { msg_type: t }, parent_header: { msg_id: parent }, content: c }) });
    iopub("stream", { name: "stdout", text: "1\n" });
    iopub("status", { execution_state: "idle" });
    expect(outputs).toEqual([{ type: "stream", name: "stdout", text: "1\n" }]);
    expect(finalStatus).toBe("idle");
  });

  it("ignores iopub whose parent msg_id is unknown (no cross-cell leak)", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    const outputs: any[] = [];
    kc.execute("cellA", "x", { onOutput: (o) => outputs.push(o), onStatus: () => {}, onExecutionCount: () => {} });
    const ws = MockWS.last!;
    ws.onmessage!({ data: JSON.stringify({ header: { msg_type: "stream" }, parent_header: { msg_id: "other" }, content: { text: "leak" } }) });
    expect(outputs).toEqual([]);
  });

  it("throws if execute() is called before connect()", () => {
    const kc = new KernelConnection("ws1", "nb1");
    expect(() => kc.execute("cellA", "x", { onOutput: () => {}, onStatus: () => {}, onExecutionCount: () => {} })).toThrow(/not connected/);
  });

  it("ignores non-JSON iopub frames without throwing", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    const outputs: any[] = [];
    kc.execute("cellA", "x", { onOutput: (o) => outputs.push(o), onStatus: () => {}, onExecutionCount: () => {} });
    const ws = MockWS.last!;
    expect(() => ws.onmessage!({ data: "not json {{{" })).not.toThrow();
    expect(outputs).toEqual([]);
  });
});
