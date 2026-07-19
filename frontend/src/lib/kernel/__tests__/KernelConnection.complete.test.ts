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

describe("KernelConnection.complete()", () => {
  it("sends a complete_request and resolves matches from the correlated complete_reply", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();

    const promise = kc.complete("df.", 3);
    const ws = MockWS.last!;
    expect(ws.sent).toHaveLength(1);
    const sent = JSON.parse(ws.sent[0]);
    expect(sent.header.msg_type).toBe("complete_request");
    expect(sent.content.code).toBe("df.");
    expect(sent.content.cursor_pos).toBe(3);

    const parent = sent.header.msg_id;
    ws.onmessage!({
      data: JSON.stringify({
        header: { msg_type: "complete_reply" },
        parent_header: { msg_id: parent },
        content: { matches: ["df.a", "df.b"], cursor_start: 3, cursor_end: 3 },
      }),
    });

    await expect(promise).resolves.toEqual({ matches: ["df.a", "df.b"], cursorStart: 3, cursorEnd: 3 });
  });

  it("ignores a complete_reply whose parent msg_id does not match any pending request", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    const ws = MockWS.last!;
    // No throw, no crash, and it doesn't fall through to execute-pending routing.
    expect(() =>
      ws.onmessage!({
        data: JSON.stringify({
          header: { msg_type: "complete_reply" },
          parent_header: { msg_id: "unknown" },
          content: { matches: ["x"], cursor_start: 0, cursor_end: 0 },
        }),
      })
    ).not.toThrow();
  });

  it("resolves empty when called before connect()", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await expect(kc.complete("df.", 3)).resolves.toEqual({ matches: [], cursorStart: 3, cursorEnd: 3 });
  });

  it("resolves empty when called after disconnect()", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    kc.disconnect();
    await expect(kc.complete("df.", 3)).resolves.toEqual({ matches: [], cursorStart: 3, cursorEnd: 3 });
  });

  it("drains an in-flight complete() as empty when the socket closes unexpectedly", async () => {
    const kc = new KernelConnection("ws1", "nb1");
    await kc.connect();
    const promise = kc.complete("df.", 3);
    const ws = MockWS.last!;
    ws.close(); // unexpected close, not disconnect()
    await expect(promise).resolves.toEqual({ matches: [], cursorStart: 0, cursorEnd: 0 });
  });

  it("times out and resolves empty if no complete_reply arrives", async () => {
    vi.useFakeTimers();
    const kc = new KernelConnection("ws1", "nb1");
    const connectPromise = kc.connect();
    // Flush the setTimeout(0) inside MockWS that fires onopen.
    await vi.advanceTimersByTimeAsync(0);
    await connectPromise;

    const promise = kc.complete("df.", 3);
    await vi.advanceTimersByTimeAsync(3000);
    await expect(promise).resolves.toEqual({ matches: [], cursorStart: 3, cursorEnd: 3 });
  });
});
