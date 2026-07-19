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

describe("KernelConnection.inspect()", () => {
  it("sends an inspect_request and resolves the text/plain from a found inspect_reply", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();

    const promise = kc.inspect("len(", 4, 0);
    const ws = MockWS.last!;
    expect(ws.sent).toHaveLength(1);
    const sent = JSON.parse(ws.sent[0]);
    expect(sent.header.msg_type).toBe("inspect_request");
    expect(sent.content.code).toBe("len(");
    expect(sent.content.cursor_pos).toBe(4);
    expect(sent.content.detail_level).toBe(0);

    const parent = sent.header.msg_id;
    ws.onmessage!({
      data: JSON.stringify({
        header: { msg_type: "inspect_reply" },
        parent_header: { msg_id: parent },
        content: { found: true, data: { "text/plain": "docstring…" } },
      }),
    });

    await expect(promise).resolves.toBe("docstring…");
  });

  it("resolves null when the inspect_reply reports found:false", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();

    const promise = kc.inspect("xyz", 3);
    const ws = MockWS.last!;
    const sent = JSON.parse(ws.sent[0]);
    const parent = sent.header.msg_id;

    ws.onmessage!({
      data: JSON.stringify({
        header: { msg_type: "inspect_reply" },
        parent_header: { msg_id: parent },
        content: { found: false },
      }),
    });

    await expect(promise).resolves.toBeNull();
  });

  it("ignores an inspect_reply whose parent msg_id does not match any pending request", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    const ws = MockWS.last!;
    expect(() =>
      ws.onmessage!({
        data: JSON.stringify({
          header: { msg_type: "inspect_reply" },
          parent_header: { msg_id: "unknown" },
          content: { found: true, data: { "text/plain": "x" } },
        }),
      })
    ).not.toThrow();
  });

  it("resolves null when called before connect()", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await expect(kc.inspect("df.", 3)).resolves.toBeNull();
  });

  it("resolves null when called after disconnect()", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    kc.disconnect();
    await expect(kc.inspect("df.", 3)).resolves.toBeNull();
  });

  it("drains an in-flight inspect() as null when the socket closes unexpectedly", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    const promise = kc.inspect("df.", 3);
    const ws = MockWS.last!;
    ws.close(); // unexpected close, not disconnect()
    await expect(promise).resolves.toBeNull();
  });

  it("times out and resolves null if no inspect_reply arrives", async () => {
    vi.useFakeTimers();
    const kc = new KernelConnection("ws1", "nb1");
    const connectPromise = kc.connect();
    await vi.advanceTimersByTimeAsync(0);
    await connectPromise;

    const promise = kc.inspect("df.", 3);
    await vi.advanceTimersByTimeAsync(3000);
    await expect(promise).resolves.toBeNull();
  });
});
