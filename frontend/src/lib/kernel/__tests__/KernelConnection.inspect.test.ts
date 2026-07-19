import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
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

afterEach(() => {
  vi.useRealTimers();
});

const VARS = [
  { name: "df", type: "DataFrame", columns: ["a", "b"] },
  { name: "x", type: "int", columns: null },
];

describe("KernelConnection.inspectVariables()", () => {
  it("sends a silent execute_request with a user_expressions.v string", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();

    void kc.inspectVariables();
    const ws = MockWS.last!;
    expect(ws.sent).toHaveLength(1);
    const sent = JSON.parse(ws.sent[0]);
    expect(sent.header.msg_type).toBe("execute_request");
    expect(sent.content.silent).toBe(true);
    expect(sent.content.store_history).toBe(false);
    expect(typeof sent.content.user_expressions.v).toBe("string");
    expect(sent.content.user_expressions.v.length).toBeGreaterThan(0);
  });

  it("resolves the parsed variable list from the correlated execute_reply", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();

    const promise = kc.inspectVariables();
    const ws = MockWS.last!;
    const sent = JSON.parse(ws.sent[0]);
    const parent = sent.header.msg_id;

    const b64 = btoa(JSON.stringify(VARS));
    const pythonRepr = "'" + b64 + "'"; // Python repr of a str is single-quoted

    ws.onmessage!({
      data: JSON.stringify({
        header: { msg_type: "execute_reply" },
        parent_header: { msg_id: parent },
        content: { user_expressions: { v: { status: "ok", data: { "text/plain": pythonRepr } } } },
      }),
    });

    await expect(promise).resolves.toEqual(VARS);
  });

  it("resolves empty when called before connect()", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await expect(kc.inspectVariables()).resolves.toEqual([]);
  });

  it("resolves empty when called after disconnect()", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    kc.disconnect();
    await expect(kc.inspectVariables()).resolves.toEqual([]);
  });

  it("drains an in-flight inspectVariables() as empty when the socket closes unexpectedly", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    const promise = kc.inspectVariables();
    const ws = MockWS.last!;
    ws.close(); // unexpected close, not disconnect()
    await expect(promise).resolves.toEqual([]);
  });

  it("times out and resolves empty if no execute_reply arrives", async () => {
    vi.useFakeTimers();
    const kc = new KernelConnection("ws1", "nb1");
    const connectPromise = kc.connect();
    await vi.advanceTimersByTimeAsync(0);
    await connectPromise;

    const promise = kc.inspectVariables();
    await vi.advanceTimersByTimeAsync(3000);
    await expect(promise).resolves.toEqual([]);
  });

  it("ignores an execute_reply whose parent msg_id does not match any pending inspect", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    const ws = MockWS.last!;
    // Start a real pending inspect so we can confirm it's unaffected.
    const promise = kc.inspectVariables();
    const parent = JSON.parse(ws.sent[0]).header.msg_id;

    expect(() =>
      ws.onmessage!({
        data: JSON.stringify({
          header: { msg_type: "execute_reply" },
          parent_header: { msg_id: "unknown" },
          content: { user_expressions: { v: { status: "ok", data: { "text/plain": "'bogus'" } } } },
        }),
      })
    ).not.toThrow();

    // The real pending inspect is still there — deliver its reply and confirm it still resolves.
    const b64 = btoa(JSON.stringify(VARS));
    ws.onmessage!({
      data: JSON.stringify({
        header: { msg_type: "execute_reply" },
        parent_header: { msg_id: parent },
        content: { user_expressions: { v: { status: "ok", data: { "text/plain": "'" + b64 + "'" } } } },
      }),
    });
    await expect(promise).resolves.toEqual(VARS);
  });
});
