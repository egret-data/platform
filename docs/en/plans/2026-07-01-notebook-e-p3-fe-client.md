# Notebook E-P3 — Frontend kernel client + outputs renderer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make cells run in the UI: a frontend Jupyter client (WS to the D1c proxy), an outputs renderer, single-cell Run wired into the notebook page, and restore of persisted outputs on load.

**Architecture:** `KernelConnection` (TS) POSTs `…/kernel/connect` (via the CSRF-aware `apiFetch`), then opens a `WebSocket` to `…/kernel/channels` (same-origin, session-cookie auth). It hand-rolls the Jupyter Messaging Protocol (like sparklabx/JupyterLab): sends `execute_request` with `metadata.egret_cell_id`, routes iopub by `parent_header.msg_id`, maps messages to `CellOutput` items (the E-P1 backend contract). `CellOutputRenderer` renders them (DOMPurify html / png / ANSI traceback). `ManagedCell` drives run state + renders outputs; on load it restores from `cell.last_output`.

**Tech Stack:** React 18 + TypeScript, Vitest + RTL (Monaco + WebSocket mocked), TanStack Query, DOMPurify (new dep), ported `ansiToHtml`.

**Scope:** single-cell Run + render + restore-from-`last_output` + a kernel status indicator. Deferred to **E-P4**: run-all/above/below, interrupt/restart buttons (endpoints exist from E-P1), Clear, the SQL-target **dropdown** (E-P3 reads `cell.sql_target` to build the magic prefix but editing it is P4), active-executions restore.

---

## File structure
- `frontend/src/lib/kernel/protocol.ts` (new) — Jupyter message types + `CellOutput` + iopub→CellOutput mapping.
- `frontend/src/lib/kernel/KernelConnection.ts` (new) — the WS client.
- `frontend/src/lib/kernel/__tests__/KernelConnection.test.ts` (new).
- `frontend/src/components/notebook/ansiToHtml.ts` (new, ported) + `CellOutputRenderer.tsx` (new) + tests.
- `frontend/src/api/notebooks.ts` — add `sql_target` to `Cell` + `useUpdateCell`.
- `frontend/src/routes/explore/NotebookPage.tsx` + `components/notebook/CellEditor.tsx` — enable Run + outputs slot + run state.
- `frontend/package.json` — add `dompurify`.

---

## Task 1: `KernelConnection` WS client (TDD)

**Files:** `frontend/src/lib/kernel/protocol.ts`, `KernelConnection.ts`, `__tests__/KernelConnection.test.ts`.

- [ ] **Step 1: Protocol types** `frontend/src/lib/kernel/protocol.ts`:
```ts
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

/** Map an iopub message to a CellOutput, or null if it isn't output. */
export function iopubToOutput(msgType: string, content: Record<string, any>): CellOutput | null {
  switch (msgType) {
    case "stream": return { type: "stream", name: content.name ?? "stdout", text: content.text ?? "" };
    case "execute_result": return { type: "execute_result", data: content.data ?? {} };
    case "display_data": return { type: "display_data", data: content.data ?? {} };
    case "error": return { type: "error", ename: content.ename ?? "", evalue: content.evalue ?? "", traceback: content.traceback ?? [] };
    default: return null;
  }
}
```

- [ ] **Step 2: Failing test** `frontend/src/lib/kernel/__tests__/KernelConnection.test.ts` — a `MockWebSocket` captures sent frames + injects iopub replies:
```ts
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
});
```

- [ ] **Step 3: Run — fails** `cd frontend && npx vitest run src/lib/kernel` → FAIL (module missing).

- [ ] **Step 4: Implement** `frontend/src/lib/kernel/KernelConnection.ts`:
```ts
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
      // ensure the kernel (retries while spawning)
      for (let i = 0; i < 30; i++) {
        const r = await apiFetch(this.path("/connect"), { method: "POST" });
        const body = await r.json().catch(() => ({}));
        if (body.phase === "ready" || body.phase === undefined) break;
        await new Promise((res) => setTimeout(res, 1000));
      }
      await this.openSocket();
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

  disconnect(): void { this.ws?.close(); this.ws = null; this.ready = null; }
}
```

- [ ] **Step 5: Run — passes** `cd frontend && npx vitest run src/lib/kernel`.
- [ ] **Step 6: Commit**
```bash
git add frontend/src/lib/kernel/
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(frontend): KernelConnection Jupyter WS client (connect + execute + iopub routing)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Outputs renderer + ansiToHtml (TDD)

**Files:** `frontend/src/components/notebook/ansiToHtml.ts`, `CellOutputRenderer.tsx`, tests; `frontend/package.json` (add `dompurify`).

- [ ] **Step 1: Add DOMPurify** `cd frontend && npm install dompurify && npm install -D @types/dompurify` (pin the resolved versions in package.json).
- [ ] **Step 2: Port `ansiToHtml`** from `/Users/1pro-l4r8/Documents/github/data-portal/sparklabx/frontend/src/components/Notebooks/ansiToHtml.ts` into `frontend/src/components/notebook/ansiToHtml.ts` (it's self-contained, exports `ansiToHtml(text): string` + `stripAnsi(text): string`). Add a Vitest: `ansiToHtml("\x1b[31mred\x1b[0m")` contains a red-colored span; `stripAnsi` removes escapes.
- [ ] **Step 3: Failing test** `frontend/src/components/notebook/__tests__/CellOutputRenderer.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { CellOutputRenderer } from "../CellOutputRenderer";

describe("CellOutputRenderer", () => {
  it("renders stream text", () => {
    render(<CellOutputRenderer outputs={[{ type: "stream", name: "stdout", text: "hello" }]} />);
    expect(screen.getByText(/hello/)).toBeInTheDocument();
  });
  it("renders an error traceback", () => {
    render(<CellOutputRenderer outputs={[{ type: "error", ename: "ValueError", evalue: "bad", traceback: ["Trace", "line"] }]} />);
    expect(screen.getByText(/ValueError/)).toBeInTheDocument();
  });
  it("renders an execute_result png as an img", () => {
    render(<CellOutputRenderer outputs={[{ type: "execute_result", data: { "image/png": "iVBORw0KGgo=" } }]} />);
    expect(screen.getByRole("img")).toHaveAttribute("src", expect.stringContaining("data:image/png;base64,iVBORw0KGgo="));
  });
  it("sanitizes text/html output", () => {
    render(<CellOutputRenderer outputs={[{ type: "execute_result", data: { "text/html": "<b>ok</b><script>alert(1)</script>" } }]} />);
    expect(screen.getByText("ok")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });
  it("shows a truncated marker", () => {
    render(<CellOutputRenderer outputs={[{ type: "truncated", dropped: 3 }]} />);
    expect(screen.getByText(/truncated/i)).toBeInTheDocument();
  });
});
```
- [ ] **Step 4: Run — fails**; **Step 5: implement** `frontend/src/components/notebook/CellOutputRenderer.tsx`:
```tsx
import DOMPurify from "dompurify";
import { ansiToHtml } from "./ansiToHtml";
import type { CellOutput } from "@/lib/kernel/protocol";

function MimeBundle({ data }: { data: Record<string, unknown> }) {
  const html = data["text/html"] as string | undefined;
  const png = data["image/png"] as string | undefined;
  const jpeg = data["image/jpeg"] as string | undefined;
  const plain = data["text/plain"] as string | undefined;
  if (html) return <div className="text-sm max-h-96 overflow-auto" dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(html) }} />;
  if (png) return <img alt="output" src={`data:image/png;base64,${png}`} />;
  if (jpeg) return <img alt="output" src={`data:image/jpeg;base64,${jpeg}`} />;
  if (plain) return <pre className="text-sm whitespace-pre-wrap" dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(ansiToHtml(plain)) }} />;
  return null;
}

export function CellOutputRenderer({ outputs }: { outputs: CellOutput[] }) {
  if (!outputs?.length) return null;
  return (
    <div className="mt-2 space-y-1 border-t px-3 py-2">
      {outputs.map((o, i) => {
        switch (o.type) {
          case "stream":
            return <pre key={i} className={`text-sm whitespace-pre-wrap ${o.name === "stderr" ? "text-red-600" : ""}`}
              dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(ansiToHtml(o.text)) }} />;
          case "execute_result":
          case "display_data":
            return <MimeBundle key={i} data={o.data} />;
          case "error":
            return <pre key={i} className="text-sm text-red-600 whitespace-pre-wrap"
              dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(`${o.ename}: ${o.evalue}\n` + ansiToHtml(o.traceback.join("\n"))) }} />;
          case "truncated":
            return <div key={i} className="text-xs text-muted-foreground">… output truncated ({o.dropped} dropped)</div>;
        }
      })}
    </div>
  );
}
```
- [ ] **Step 6: Run — passes** `cd frontend && npx vitest run src/components/notebook`.
- [ ] **Step 7: Commit**
```bash
git add frontend/src/components/notebook/ansiToHtml.ts frontend/src/components/notebook/CellOutputRenderer.tsx frontend/src/components/notebook/__tests__/ frontend/package.json frontend/package-lock.json
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(frontend): CellOutputRenderer (html/png/ANSI) + ansiToHtml port

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Wire Run + outputs + restore into the notebook page (TDD)

**Files:** `frontend/src/api/notebooks.ts`, `frontend/src/routes/explore/NotebookPage.tsx`, `frontend/src/components/notebook/CellEditor.tsx`.

- [ ] **Step 1: Cell DTO + hook** — in `notebooks.ts`: add `sql_target: string | null` to the `Cell` interface (L6-14). (Leave `useUpdateCell` as-is; editing `sql_target` = E-P4's dropdown. E-P3 only READS it.)
- [ ] **Step 2: `CellEditor` gains Run + outputs** — add `onRun?: () => void`, `running?: boolean`, `outputs?: CellOutput[]` to `CellEditorProps` (import `CellOutput` from `@/lib/kernel/protocol`). Wire the Run `<Button>` (L83-93): `onClick={onRun}`, `disabled={!onRun || running}`, spinner/`title` when running; drop the hardcoded `title="Execution coming soon"`. After the cell body (before the card's closing `</div>` ~L129) render `{outputs && <CellOutputRenderer outputs={outputs} />}`.
- [ ] **Step 3: `NotebookPage`/`ManagedCell` drives execution** — in `NotebookPage`, create ONE `KernelConnection` (memoized per `ws`/`id`) and connect lazily on first Run. `ManagedCell` (L33-79) gains local state: `outputs: CellOutput[]` (initialized from `cell.last_output` cast — **restore-on-load**), `running: boolean`. `handleRun`:
```ts
const kc = useKernel(ws, id); // a small hook returning the memoized KernelConnection
async function handleRun() {
  await kc.connect();
  setOutputs([]); setRunning(true);
  const code = cell.cell_type === "sql"
    ? `%%egret_sql --target ${cell.sql_target || "daft"}\n${source}`
    : source;
  kc.execute(cell.id, code, {
    onOutput: (o) => setOutputs((prev) => [...prev, o]),
    onStatus: (s) => { if (s === "idle") setRunning(false); },
    onExecutionCount: () => {},
  });
}
```
Pass `onRun={cell.cell_type === "markdown" ? undefined : handleRun}` (markdown doesn't run), `running`, `outputs` to `<CellEditor>`. Remove the bare `runDisabled` (or set `runDisabled={false}` and rely on `onRun`). Add a small kernel status indicator in the page header (idle/connecting) — minimal.
- [ ] **Step 4: Failing test** — extend `frontend/src/routes/explore/__tests__/NotebookPage.test.tsx` (mock the `KernelConnection` module + the API hooks): a code cell shows a Run button (enabled), clicking it calls `kc.connect` + `kc.execute` with the cell id; a cell whose `last_output` has a stream renders the text on load (restore). Mock `@/lib/kernel/KernelConnection` with a class whose `execute` immediately calls `onOutput`+`onStatus("idle")`.
- [ ] **Step 5: Run — fails then implement then passes** `cd frontend && npx vitest run src/routes/explore`.
- [ ] **Step 6: Commit**
```bash
git add frontend/src/api/notebooks.ts frontend/src/routes/explore/NotebookPage.tsx frontend/src/components/notebook/CellEditor.tsx frontend/src/routes/explore/__tests__/
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(frontend): run a cell + render outputs + restore last_output on load

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Frontend CI

- [ ] **Step 1:** `cd frontend && npm run lint` → clean (fix any lint from the new files).
- [ ] **Step 2:** `npm run typecheck` → clean.
- [ ] **Step 3:** `npm test` (`vitest run`) → all pass (new kernel + renderer + notebook tests + the existing suite).
- [ ] **Step 4:** `npm run build` → succeeds.
- [ ] **Step 5:** Note in the PR: live-kernel exec is verifiable manually (docker stack + `npm run dev`, run a Python cell); Vitest covers the client/renderer/wiring with mocks. Per [[verification-wording-precision]] state exactly what ran (Vitest, not Playwright).

---

## Self-review
- **Spec coverage (E §3.1 FE client, §3.5 renderer + run-single + restore):** T1 KernelConnection (WS, execute_request+egret_cell_id, iopub routing) ✔; T2 renderer (html/png/ANSI/DOMPurify) ✔; T3 Run wiring + restore-from-last_output ✔. Deferred to E-P4: run-all/above/below, interrupt/restart/clear buttons, SQL-target dropdown, active-executions restore, status polish.
- **Placeholder scan:** T2 says "port ansiToHtml from sparklabx" (a concrete file to copy); T3 says "mock KernelConnection / copy the NotebookPage test setup" (exists in-repo). The KernelConnection, protocol mapping, and renderer are fully specified. No logic placeholders.
- **Type consistency:** `CellOutput` (protocol.ts) = the E-P1 backend shape (stream/execute_result/display_data/error/truncated) — used by the client, the renderer, and restore. `KernelConnection.execute(cellId, code, {onOutput,onStatus,onExecutionCount})`; `newMsg`/`iopubToOutput` shared.
- **Security:** `text/html` + ANSI are DOMPurify-sanitized before `dangerouslySetInnerHTML`; the WS is same-origin session-cookie auth; the connect POST uses the CSRF-aware `apiFetch`; `metadata.egret_cell_id` is what the E-P2 recorder correlates on (contract match).
- **Correctness:** iopub routed by `parent_header.msg_id` (no cross-cell leak — tested); restore reads `cell.last_output` (persisted by E-P2). The recorder (E-P2) owns durable persistence; the FE only renders live + restores.
