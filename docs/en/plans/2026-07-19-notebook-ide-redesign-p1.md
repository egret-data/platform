# Notebook IDE Redesign — P1 (FE-only) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn egret's single-column notebook page into an IDE-style shell (icon-rail sidebar with TOC + Data Sources, sticky header with a reactive kernel-status badge and run toolbar, richer cells with In[N] gutter + state accent + markdown preview toggle + Shift+Enter, and an upgraded output renderer) — all frontend, no backend or kernel changes.

**Architecture:** Extend `KernelConnection` with an observable status stream consumed by a `useKernelStatus` hook + `KernelStatusBadge`. Add presentational panels (`SidebarIconRail`, `TableOfContentsPanel`, `DataSourcesPanel`) and compose them into a 3-region flex layout inside `NotebookPage`. Upgrade `CellEditor` and `CellOutputRenderer` in place. SQL cell + DataFrame binding is **P2** and out of scope here.

**Tech Stack:** React 18, TypeScript, Tailwind v4 (OKLCH tokens), shadcn/ui, `@monaco-editor/react`, `lucide-react`, Vitest + `@testing-library/react` + `userEvent`, DOMPurify.

**Reference spec:** `docs/en/specs/2026-07-19-notebook-ide-redesign-design.md` (§3, §4.1–4.7).

**Conventions:**
- All commands run from `egret/frontend`.
- Verify commands: `npm run test`, `npm run typecheck`, `npm run lint`, `npm run build`.
- OKLCH: in inline styles use `var(--token)` directly (never `hsl(var(--token))`). Tailwind utilities `text-primary`, `text-destructive`, `text-muted-foreground` exist; `--success` has no utility → use inline `style={{ color: "var(--success)" }}`.
- Do not commit unless the human asks; if asked, branch off `main` first (currently on `main`).

---

## File Structure

| File | New/Mod | Responsibility |
|---|---|---|
| `src/lib/kernel/KernelConnection.ts` | Mod | Add observable status stream + emit transitions |
| `src/lib/kernel/useKernelStatus.ts` | New | React hook subscribing to connection status |
| `src/components/notebook/KernelStatusBadge.tsx` | New | Dot + label + tooltip, status→color mapping |
| `src/components/notebook/CellOutputRenderer.tsx` | Mod | Error boundary + ANSI cache + collapsible stderr + error chrome |
| `src/components/notebook/monaco/registerStatic.ts` | New | Idempotent python + sql static completions |
| `src/components/notebook/panels/tocModel.ts` | New | Pure `deriveToc(cells)` → entries |
| `src/components/notebook/panels/TableOfContentsPanel.tsx` | New | Render TOC, jump-to-cell |
| `src/components/notebook/panels/DataSourcesPanel.tsx` | New | List runtime engines + copy hints |
| `src/components/notebook/SidebarIconRail.tsx` | New | Icon rail, tab switch/collapse |
| `src/components/notebook/NotebookHeader.tsx` | New | Breadcrumb, name, public, badge, run toolbar |
| `src/components/notebook/CellEditor.tsx` | Mod | Gutter In[N], accent bar, md toggle, Shift+Enter, hover toolbar |
| `src/routes/explore/NotebookPage.tsx` | Mod | 3-region layout, sidebar state, TOC jump, wire header/badge |

---

## Task 1: KernelConnection status stream

**Files:**
- Modify: `src/lib/kernel/KernelConnection.ts`
- Test: `src/lib/kernel/__tests__/KernelConnection.status.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/lib/kernel/__tests__/KernelConnection.status.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";

const interruptMock = vi.fn();
const restartMock = vi.fn();
vi.mock("@/api/generated/kernel/kernel", () => ({
  kernelConnect: vi.fn(),
  kernelKernelInterrupt: (...a: unknown[]) => interruptMock(...a),
  kernelKernelRestart: (...a: unknown[]) => restartMock(...a),
}));

import { KernelConnection } from "../KernelConnection";

describe("KernelConnection status stream", () => {
  beforeEach(() => {
    interruptMock.mockReset().mockResolvedValue({});
    restartMock.mockReset().mockResolvedValue({});
  });

  it("emits the current status immediately and stops after unsubscribe", () => {
    const k = new KernelConnection("ws", "nb");
    const seen: string[] = [];
    const off = k.onStatusChange((s) => seen.push(s));
    expect(seen).toEqual(["disconnected"]); // initial value delivered synchronously
    off();
    k.disconnect(); // would emit "disconnected" again, but we're unsubscribed
    expect(seen).toEqual(["disconnected"]);
  });

  it("restart() emits a transient 'restarting' then settles 'disconnected' with no socket", async () => {
    const k = new KernelConnection("ws", "nb");
    const seen: string[] = [];
    k.onStatusChange((s) => seen.push(s));
    await k.restart();
    expect(seen).toContain("restarting");
    expect(k.status).toBe("disconnected");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- KernelConnection.status`
Expected: FAIL — `onStatusChange`/`status` do not exist.

- [ ] **Step 3: Implement the status stream**

Edit `src/lib/kernel/KernelConnection.ts`. Widen the union (line 8):

```ts
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
```

Add fields + emitter to the class (after the `private ready` field, ~line 21):

```ts
  private _status: KernelStatus = "disconnected";
  private statusListeners = new Set<(s: KernelStatus) => void>();

  get status(): KernelStatus {
    return this._status;
  }

  /** Subscribe to connection-status changes. Fires immediately with the
   *  current value; returns an unsubscribe function. */
  onStatusChange(cb: (s: KernelStatus) => void): () => void {
    this.statusListeners.add(cb);
    cb(this._status);
    return () => {
      this.statusListeners.delete(cb);
    };
  }

  private setStatus(s: KernelStatus): void {
    if (this._status === s) return;
    this._status = s;
    this.statusListeners.forEach((cb) => cb(s));
  }
```

Emit transitions. In `connect()`, at the very start of the async IIFE (before the retry loop): `this.setStatus("connecting");`. In the `catch (e)` block add `this.setStatus("error");` before rethrow.

In `openSocket()`: in `ws.onopen` add `this.setStatus("connected");` before `resolve()`; in `ws.onclose` add `this.setStatus("disconnected");` at the top of the handler; in `ws.onerror` add `this.setStatus("error");` before `reject(...)`.

In `onMessage()`, in the `status` branch, after computing `s`:

```ts
if (t === "status") {
  const s = m.content?.execution_state;
  if (s === "busy" || s === "idle") { h.onStatus(s); this.setStatus(s); }
  if (s === "idle") this.pending.delete(parent);
  return;
}
```

Replace `interrupt()` and `restart()` bodies:

```ts
  async interrupt(): Promise<void> {
    this.setStatus("interrupting");
    try {
      await kernelKernelInterrupt(this.ws_slug, this.nb);
    } finally {
      if (this._status === "interrupting") this.setStatus(this.ws ? "idle" : "disconnected");
    }
  }

  async restart(): Promise<void> {
    this.setStatus("restarting");
    try {
      await kernelKernelRestart(this.ws_slug, this.nb);
    } finally {
      if (this._status === "restarting") this.setStatus(this.ws ? "connected" : "disconnected");
    }
  }
```

In `disconnect()` add `this.setStatus("disconnected");` before the field resets.

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- KernelConnection.status`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/lib/kernel/KernelConnection.ts src/lib/kernel/__tests__/KernelConnection.status.test.ts
git commit -m "feat(notebook): observable kernel status stream"
```

---

## Task 2: useKernelStatus hook

**Files:**
- Create: `src/lib/kernel/useKernelStatus.ts`
- Test: `src/lib/kernel/__tests__/useKernelStatus.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/lib/kernel/__tests__/useKernelStatus.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useKernelStatus } from "../useKernelStatus";
import type { KernelStatus } from "../KernelConnection";

// Minimal fake matching the shape useKernelStatus depends on.
function makeFake(initial: KernelStatus) {
  let status = initial;
  const listeners = new Set<(s: KernelStatus) => void>();
  return {
    get status() { return status; },
    onStatusChange(cb: (s: KernelStatus) => void) { listeners.add(cb); cb(status); return () => listeners.delete(cb); },
    emit(s: KernelStatus) { status = s; listeners.forEach((cb) => cb(s)); },
  };
}

describe("useKernelStatus", () => {
  it("returns the current status and updates on change", () => {
    const fake = makeFake("disconnected");
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const { result } = renderHook(() => useKernelStatus(fake as any));
    expect(result.current).toBe("disconnected");
    act(() => fake.emit("connected"));
    expect(result.current).toBe("connected");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- useKernelStatus`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the hook**

Create `src/lib/kernel/useKernelStatus.ts`:

```ts
import { useEffect, useState } from "react";
import type { KernelConnection, KernelStatus } from "./KernelConnection";

/** Subscribe to a KernelConnection's status and re-render on change. */
export function useKernelStatus(kernel: KernelConnection): KernelStatus {
  const [status, setStatus] = useState<KernelStatus>(kernel.status);
  useEffect(() => kernel.onStatusChange(setStatus), [kernel]);
  return status;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- useKernelStatus`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lib/kernel/useKernelStatus.ts src/lib/kernel/__tests__/useKernelStatus.test.tsx
git commit -m "feat(notebook): useKernelStatus hook"
```

---

## Task 3: KernelStatusBadge

**Files:**
- Create: `src/components/notebook/KernelStatusBadge.tsx`
- Test: `src/components/notebook/__tests__/KernelStatusBadge.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/notebook/__tests__/KernelStatusBadge.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { KernelStatusBadge } from "../KernelStatusBadge";

describe("KernelStatusBadge", () => {
  it("labels each status with an accessible text (not color-only)", () => {
    const { rerender } = render(<KernelStatusBadge status="connecting" />);
    expect(screen.getByText("Connecting…")).toBeInTheDocument();
    rerender(<KernelStatusBadge status="error" />);
    expect(screen.getByText("Error")).toBeInTheDocument();
    rerender(<KernelStatusBadge status="idle" />);
    expect(screen.getByText("Ready")).toBeInTheDocument();
  });

  it("exposes a polite live region for screen readers", () => {
    render(<KernelStatusBadge status="busy" />);
    expect(screen.getByText("Running…").closest("[aria-live]")).toHaveAttribute("aria-live", "polite");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- KernelStatusBadge`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the badge**

Create `src/components/notebook/KernelStatusBadge.tsx`:

```tsx
import { Circle } from "lucide-react";
import type { KernelStatus } from "@/lib/kernel/KernelConnection";

interface StatusStyle {
  label: string;
  /** Tailwind text-color utility, or "" when using inlineColor. */
  cls: string;
  /** Inline color for tokens without a utility (e.g. --success). */
  inlineColor?: string;
  pulse?: boolean;
}

const STYLES: Record<KernelStatus, StatusStyle> = {
  disconnected: { label: "Disconnected", cls: "text-muted-foreground" },
  connecting: { label: "Connecting…", cls: "text-primary", pulse: true },
  spawning: { label: "Starting…", cls: "text-primary", pulse: true },
  connected: { label: "Connected", cls: "", inlineColor: "var(--success)" },
  idle: { label: "Ready", cls: "", inlineColor: "var(--success)" },
  busy: { label: "Running…", cls: "text-primary", pulse: true },
  interrupting: { label: "Interrupting…", cls: "text-primary", pulse: true },
  restarting: { label: "Restarting…", cls: "text-primary", pulse: true },
  error: { label: "Error", cls: "text-destructive" },
  dead: { label: "Kernel dead", cls: "text-destructive" },
};

export function KernelStatusBadge({ status }: { status: KernelStatus }) {
  const s = STYLES[status];
  return (
    <div
      className="flex items-center gap-1.5"
      title={s.label}
      aria-live="polite"
    >
      <Circle
        className={`h-2.5 w-2.5 fill-current motion-safe:${s.pulse ? "animate-pulse" : ""} ${s.cls}`}
        style={s.inlineColor ? { color: s.inlineColor } : undefined}
      />
      {/* Fixed width sized to the longest label so the toolbar doesn't shift. */}
      <span className="inline-block w-28 whitespace-nowrap text-sm text-muted-foreground">
        {s.label}
      </span>
    </div>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- KernelStatusBadge`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/components/notebook/KernelStatusBadge.tsx src/components/notebook/__tests__/KernelStatusBadge.test.tsx
git commit -m "feat(notebook): kernel status badge"
```

---

## Task 4: Upgrade CellOutputRenderer

**Files:**
- Modify: `src/components/notebook/CellOutputRenderer.tsx`
- Test: `src/components/notebook/__tests__/CellOutputRenderer.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/notebook/__tests__/CellOutputRenderer.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CellOutputRenderer } from "../CellOutputRenderer";
import type { CellOutput } from "@/lib/kernel/protocol";

describe("CellOutputRenderer", () => {
  it("renders stdout inline and collapses stderr under a Logs disclosure", async () => {
    const outputs: CellOutput[] = [
      { type: "stream", name: "stdout", text: "hello user" },
      { type: "stream", name: "stderr", text: "a warning line" },
    ];
    render(<CellOutputRenderer outputs={outputs} />);
    expect(screen.getByText(/hello user/)).toBeInTheDocument();
    // stderr is hidden until the Logs disclosure is opened
    expect(screen.queryByText(/a warning line/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /logs/i }));
    expect(screen.getByText(/a warning line/)).toBeInTheDocument();
  });

  it("renders an error with its ename and evalue", () => {
    const outputs: CellOutput[] = [
      { type: "error", ename: "ValueError", evalue: "bad", traceback: ["Traceback…"] },
    ];
    render(<CellOutputRenderer outputs={outputs} />);
    expect(screen.getByText(/ValueError: bad/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- CellOutputRenderer`
Expected: FAIL — stderr currently renders inline (no Logs disclosure), so the first test fails.

- [ ] **Step 3: Rewrite the renderer**

Replace the entire contents of `src/components/notebook/CellOutputRenderer.tsx`:

```tsx
import React, { useState } from "react";
import DOMPurify from "dompurify";
import { AlertCircle, ChevronRight } from "lucide-react";
import { ansiToHtml } from "./ansiToHtml";
import type { CellOutput } from "@/lib/kernel/protocol";

// Bounded ANSI→HTML cache. Keys are the exact output strings already held in
// memory by the cell's outputs state; FIFO eviction caps growth. Avoids
// re-converting a long-running cell's whole scrollback on every stream flush.
const ansiCache = new Map<string, string>();
function ansi(text: string): string {
  const hit = ansiCache.get(text);
  if (hit !== undefined) return hit;
  const html = DOMPurify.sanitize(ansiToHtml(text));
  if (ansiCache.size >= 500) {
    const oldest = ansiCache.keys().next().value;
    if (oldest !== undefined) ansiCache.delete(oldest);
  }
  ansiCache.set(text, html);
  return html;
}

// A malformed MIME bundle (corrupt HTML/JSON) can throw during render. Without
// a boundary the whole notebook unmounts; this contains it to one cell.
class OutputErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  render() {
    if (this.state.error) {
      return (
        <div className="border-t px-3 py-2 text-xs text-destructive">
          Failed to render output: {this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}

function MimeBundle({ data }: { data: Record<string, unknown> }) {
  const html = data["text/html"] as string | undefined;
  const png = data["image/png"] as string | undefined;
  const jpeg = data["image/jpeg"] as string | undefined;
  const plain = data["text/plain"] as string | undefined;
  if (html)
    return (
      <div
        className="max-h-96 overflow-auto text-sm"
        dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize(html) }}
      />
    );
  if (png) return <img alt="output" className="max-w-full" src={`data:image/png;base64,${png}`} />;
  if (jpeg) return <img alt="output" className="max-w-full" src={`data:image/jpeg;base64,${jpeg}`} />;
  if (plain)
    return (
      <pre
        className="whitespace-pre-wrap text-sm"
        dangerouslySetInnerHTML={{ __html: ansi(plain) }}
      />
    );
  return null;
}

function InnerRenderer({ outputs }: { outputs: CellOutput[] }) {
  const [showLogs, setShowLogs] = useState(false);
  if (!outputs?.length) return null;

  // stderr streams collapse into a single trailing "Logs" disclosure; stdout,
  // results, and errors render inline in order.
  const stderr = outputs
    .filter((o): o is Extract<CellOutput, { type: "stream" }> => o.type === "stream" && o.name === "stderr")
    .map((o) => o.text)
    .join("");

  return (
    <div className="mt-2 space-y-1 border-t bg-muted/30 px-3 py-2">
      {outputs.map((o, i) => {
        switch (o.type) {
          case "stream":
            if (o.name === "stderr") return null; // shown in Logs
            return (
              <pre
                key={i}
                className="whitespace-pre-wrap text-sm text-foreground"
                dangerouslySetInnerHTML={{ __html: ansi(o.text) }}
              />
            );
          case "execute_result":
          case "display_data":
            return <MimeBundle key={i} data={o.data} />;
          case "error":
            return (
              <div
                key={i}
                className="overflow-x-auto rounded border border-destructive/30 bg-destructive/5 p-2 text-sm text-destructive"
              >
                <div className="mb-1 flex items-center gap-1.5 font-semibold">
                  <AlertCircle className="h-4 w-4" />
                  <span className="whitespace-pre-wrap">
                    {o.ename}: {o.evalue}
                  </span>
                </div>
                {o.traceback.length > 0 && (
                  <pre
                    className="whitespace-pre-wrap break-words border-t border-destructive/20 pt-2 font-mono text-xs opacity-90"
                    dangerouslySetInnerHTML={{ __html: ansi(o.traceback.join("\n")) }}
                  />
                )}
              </div>
            );
          case "truncated":
            return (
              <div key={i} className="text-xs text-muted-foreground">
                … output truncated ({o.dropped} dropped)
              </div>
            );
        }
      })}

      {stderr.trim() && (
        <div className="mt-1">
          <button
            type="button"
            onClick={() => setShowLogs((v) => !v)}
            className="flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
          >
            <ChevronRight
              className={`h-3 w-3 transition-transform ${showLogs ? "rotate-90" : ""}`}
            />
            Logs ({stderr.split("\n").filter(Boolean).length} lines)
          </button>
          {showLogs && (
            <pre
              className="mt-1 max-h-48 overflow-auto rounded border bg-background/50 p-2 text-[11px] text-muted-foreground"
              dangerouslySetInnerHTML={{ __html: ansi(stderr) }}
            />
          )}
        </div>
      )}
    </div>
  );
}

export function CellOutputRenderer({ outputs }: { outputs: CellOutput[] }) {
  return (
    <OutputErrorBoundary>
      <InnerRenderer outputs={outputs} />
    </OutputErrorBoundary>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- CellOutputRenderer`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/components/notebook/CellOutputRenderer.tsx src/components/notebook/__tests__/CellOutputRenderer.test.tsx
git commit -m "feat(notebook): richer output renderer with error boundary and collapsible logs"
```

---

## Task 5: Monaco static completions

**Files:**
- Create: `src/components/notebook/monaco/registerStatic.ts`
- Test: `src/components/notebook/monaco/__tests__/registerStatic.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/components/notebook/monaco/__tests__/registerStatic.test.ts`:

```ts
import { describe, it, expect, vi } from "vitest";
import { registerStaticCompletions } from "../registerStatic";

function fakeMonaco() {
  const registered: string[] = [];
  return {
    registered,
    languages: {
      CompletionItemKind: { Function: 1, Variable: 2, Keyword: 3 },
      CompletionItemInsertTextRule: { InsertAsSnippet: 4 },
      registerCompletionItemProvider: (lang: string) => {
        registered.push(lang);
        return { dispose: vi.fn() };
      },
    },
  };
}

describe("registerStaticCompletions", () => {
  it("registers python and sql once (idempotent)", () => {
    const m = fakeMonaco();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    registerStaticCompletions(m as any);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    registerStaticCompletions(m as any); // second call is a no-op
    expect(m.registered.sort()).toEqual(["python", "sql"]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- registerStatic`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement completions**

Create `src/components/notebook/monaco/registerStatic.ts`:

```ts
import type { Monaco } from "@monaco-editor/react";

let registered = false;

const PYTHON_ITEMS: { label: string; insert: string; detail: string }[] = [
  { label: "trino", insert: 'trino("${1:SELECT 1}")', detail: "egret · governed Trino → Daft DataFrame" },
  { label: "starrocks", insert: 'starrocks("${1:SELECT 1}")', detail: "egret · governed StarRocks → Daft DataFrame" },
  { label: "write_table", insert: 'write_table(${1:df}, "${2:namespace.table}")', detail: "egret · write a DataFrame back" },
  { label: "display", insert: "display(${1:df})", detail: "egret · render a Daft DataFrame" },
  { label: "runtime", insert: "runtime", detail: "egret · parsed notebook runtime manifest" },
  { label: "me", insert: "me", detail: "egret · your username" },
];

const SQL_KEYWORDS = [
  "SELECT", "FROM", "WHERE", "GROUP BY", "ORDER BY", "LIMIT", "JOIN",
  "LEFT JOIN", "INNER JOIN", "ON", "AS", "WITH", "DISTINCT", "COUNT",
  "SUM", "AVG", "MIN", "MAX", "HAVING", "UNION", "INSERT INTO", "VALUES",
];

/** Register python + sql static completions once per page load. Safe to call
 *  from every editor's onMount — subsequent calls are no-ops. */
export function registerStaticCompletions(monaco: Monaco): void {
  if (registered) return;
  registered = true;

  monaco.languages.registerCompletionItemProvider("python", {
    provideCompletionItems() {
      return {
        suggestions: PYTHON_ITEMS.map((it) => ({
          label: it.label,
          kind: monaco.languages.CompletionItemKind.Function,
          insertText: it.insert,
          insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
          detail: it.detail,
          range: undefined as never, // Monaco fills the default replace range
        })),
      };
    },
  });

  monaco.languages.registerCompletionItemProvider("sql", {
    provideCompletionItems() {
      return {
        suggestions: SQL_KEYWORDS.map((kw) => ({
          label: kw,
          kind: monaco.languages.CompletionItemKind.Keyword,
          insertText: kw,
          range: undefined as never,
        })),
      };
    },
  });
}

/** Test-only reset of the module guard. */
export function __resetForTest(): void {
  registered = false;
}
```

Note: the test uses a fresh module import; if Vitest shares module state across the two `registerStaticCompletions` calls within one test that's the intended idempotency check. No `__resetForTest` call is needed in the provided test.

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- registerStatic`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/components/notebook/monaco/registerStatic.ts src/components/notebook/monaco/__tests__/registerStatic.test.ts
git commit -m "feat(notebook): monaco static completions for python + sql"
```

---

## Task 6: Table of Contents model + panel

**Files:**
- Create: `src/components/notebook/panels/tocModel.ts`
- Create: `src/components/notebook/panels/TableOfContentsPanel.tsx`
- Test: `src/components/notebook/panels/__tests__/tocModel.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/components/notebook/panels/__tests__/tocModel.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { deriveToc } from "../tocModel";
import type { Cell } from "@/api/notebooks";

function cell(partial: Partial<Cell> & Pick<Cell, "id" | "cell_type" | "source" | "cell_order">): Cell {
  return { execution_count: null, last_output: null, last_execution_time_ms: null, ...partial };
}

describe("deriveToc", () => {
  it("extracts markdown headings with levels and labels code cells", () => {
    const cells: Cell[] = [
      cell({ id: "a", cell_type: "markdown", source: "# Title\nbody", cell_order: 0 }),
      cell({ id: "b", cell_type: "markdown", source: "## Section", cell_order: 1 }),
      cell({ id: "c", cell_type: "code", source: "df = trino('SELECT 1')\nmore", cell_order: 2, execution_count: 4 }),
    ];
    const toc = deriveToc(cells);
    expect(toc).toEqual([
      { id: "a", label: "Title", level: 1 },
      { id: "b", label: "Section", level: 2 },
      { id: "c", label: "In [4] · df = trino('SELECT 1')", level: 3 },
    ]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- tocModel`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the model + panel**

Create `src/components/notebook/panels/tocModel.ts`:

```ts
import type { Cell } from "@/api/notebooks";

export interface TocEntry {
  id: string;
  label: string;
  level: number;
}

/** Derive a table-of-contents from cells: markdown headings become entries at
 *  their heading level; code/sql cells become level-3 entries labelled with
 *  their execution count + first non-blank source line. */
export function deriveToc(cells: Cell[]): TocEntry[] {
  const sorted = [...cells].sort((a, b) => a.cell_order - b.cell_order);
  const entries: TocEntry[] = [];
  for (const c of sorted) {
    if (c.cell_type === "markdown") {
      const heading = c.source
        .split("\n")
        .map((l) => l.match(/^(#{1,6})\s+(.*)$/))
        .find(Boolean);
      if (heading) {
        entries.push({ id: c.id, label: heading[2].trim(), level: heading[1].length });
      }
      continue;
    }
    const first = c.source.split("\n").find((l) => l.trim()) ?? "(empty)";
    const n = c.execution_count == null ? " " : String(c.execution_count);
    entries.push({ id: c.id, label: `In [${n}] · ${first.trim()}`, level: 3 });
  }
  return entries;
}
```

Create `src/components/notebook/panels/TableOfContentsPanel.tsx`:

```tsx
import type { Cell } from "@/api/notebooks";
import { deriveToc } from "./tocModel";

export function TableOfContentsPanel({
  cells,
  onJump,
}: {
  cells: Cell[];
  onJump: (cellId: string) => void;
}) {
  const toc = deriveToc(cells);
  if (toc.length === 0) {
    return <p className="px-3 py-2 text-xs text-muted-foreground">No headings yet.</p>;
  }
  return (
    <nav aria-label="Table of contents" className="flex flex-col py-1">
      {toc.map((e) => (
        <button
          key={e.id}
          type="button"
          onClick={() => onJump(e.id)}
          className="truncate rounded px-2 py-1 text-left text-xs text-muted-foreground hover:bg-muted hover:text-foreground"
          style={{ paddingLeft: `${0.5 + (e.level - 1) * 0.6}rem` }}
          title={e.label}
        >
          {e.label}
        </button>
      ))}
    </nav>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- tocModel`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/components/notebook/panels/tocModel.ts src/components/notebook/panels/TableOfContentsPanel.tsx src/components/notebook/panels/__tests__/tocModel.test.ts
git commit -m "feat(notebook): table-of-contents panel"
```

---

## Task 7: Data Sources panel

**Files:**
- Create: `src/components/notebook/panels/DataSourcesPanel.tsx`
- Test: `src/components/notebook/panels/__tests__/DataSourcesPanel.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/notebook/panels/__tests__/DataSourcesPanel.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

const runtimeMock = vi.fn();
vi.mock("@/api/notebooks", () => ({
  useNotebookRuntime: () => runtimeMock(),
}));

import { DataSourcesPanel } from "../DataSourcesPanel";

describe("DataSourcesPanel", () => {
  it("lists enabled engines from the runtime", () => {
    runtimeMock.mockReturnValue({
      data: {
        username: "u",
        engines: [
          { id: "e1", name: "Warehouse Trino", engine: "trino", enabled: true },
          { id: "e2", name: "Disabled SR", engine: "starrocks", enabled: false },
        ],
      },
    });
    render(<DataSourcesPanel ws="demo" />);
    expect(screen.getByText("Warehouse Trino")).toBeInTheDocument();
    expect(screen.queryByText("Disabled SR")).not.toBeInTheDocument();
  });

  it("shows an empty hint when there are no engines", () => {
    runtimeMock.mockReturnValue({ data: { username: "u", engines: [] } });
    render(<DataSourcesPanel ws="demo" />);
    expect(screen.getByText(/no data sources/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- DataSourcesPanel`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the panel**

Create `src/components/notebook/panels/DataSourcesPanel.tsx`:

```tsx
import { Database } from "lucide-react";
import { useNotebookRuntime } from "@/api/notebooks";

export function DataSourcesPanel({ ws }: { ws: string }) {
  const runtime = useNotebookRuntime(ws);
  const engines = (runtime.data?.engines ?? []).filter((e) => e.enabled);

  if (engines.length === 0) {
    return (
      <p className="px-3 py-2 text-xs text-muted-foreground">
        No data sources enabled for notebooks. Enable engines on the Notebook
        Settings page.
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-1 py-1">
      {engines.map((e) => (
        <div key={e.id} className="flex items-center gap-2 px-3 py-1.5 text-xs">
          <Database className="h-3.5 w-3.5 text-muted-foreground" />
          <span className="truncate font-medium text-foreground" title={e.name}>
            {e.name}
          </span>
          <span className="ml-auto rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase text-muted-foreground">
            {e.engine}
          </span>
        </div>
      ))}
      <p className="mt-1 border-t px-3 pt-2 text-[11px] leading-relaxed text-muted-foreground">
        Query as a Daft DataFrame:
        <code className="mt-1 block rounded bg-muted px-1.5 py-1 font-mono">
          df = trino("SELECT …")
        </code>
      </p>
    </div>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- DataSourcesPanel`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/components/notebook/panels/DataSourcesPanel.tsx src/components/notebook/panels/__tests__/DataSourcesPanel.test.tsx
git commit -m "feat(notebook): data sources panel"
```

---

## Task 8: SidebarIconRail

**Files:**
- Create: `src/components/notebook/SidebarIconRail.tsx`
- Test: `src/components/notebook/__tests__/SidebarIconRail.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/notebook/__tests__/SidebarIconRail.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SidebarIconRail } from "../SidebarIconRail";

describe("SidebarIconRail", () => {
  it("calls onPick with the tab key", async () => {
    const onPick = vi.fn();
    render(<SidebarIconRail tab="toc" open onPick={onPick} />);
    await userEvent.click(screen.getByRole("button", { name: /data sources/i }));
    expect(onPick).toHaveBeenCalledWith("dataSources");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- SidebarIconRail`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the rail**

Create `src/components/notebook/SidebarIconRail.tsx`:

```tsx
import { List, Database } from "lucide-react";

export type SidebarTab = "toc" | "dataSources";

const TABS: { key: SidebarTab; icon: typeof List; title: string }[] = [
  { key: "toc", icon: List, title: "Table of Contents" },
  { key: "dataSources", icon: Database, title: "Data Sources" },
];

export function SidebarIconRail({
  tab,
  open,
  onPick,
}: {
  tab: SidebarTab;
  open: boolean;
  onPick: (tab: SidebarTab) => void;
}) {
  return (
    <aside className="flex w-12 shrink-0 flex-col items-center border-r bg-muted/50 py-2">
      {TABS.map(({ key, icon: Icon, title }) => {
        const active = tab === key && open;
        return (
          <button
            key={key}
            type="button"
            title={title}
            aria-label={title}
            aria-pressed={active}
            onClick={() => onPick(key)}
            className={`mb-2 rounded p-2 transition-colors ${
              active
                ? "bg-primary/10 text-primary"
                : "text-muted-foreground hover:bg-muted hover:text-foreground"
            }`}
          >
            <Icon className="h-5 w-5" />
          </button>
        );
      })}
    </aside>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- SidebarIconRail`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/components/notebook/SidebarIconRail.tsx src/components/notebook/__tests__/SidebarIconRail.test.tsx
git commit -m "feat(notebook): sidebar icon rail"
```

---

## Task 9: NotebookHeader

**Files:**
- Create: `src/components/notebook/NotebookHeader.tsx`
- Test: `src/components/notebook/__tests__/NotebookHeader.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/notebook/__tests__/NotebookHeader.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { NotebookHeader } from "../NotebookHeader";

function renderHeader(overrides: Partial<React.ComponentProps<typeof NotebookHeader>> = {}) {
  const props: React.ComponentProps<typeof NotebookHeader> = {
    ws: "demo",
    name: "My NB",
    isPublic: false,
    status: "idle",
    runningAll: false,
    onRename: vi.fn(),
    onTogglePublic: vi.fn(),
    onRunAll: vi.fn(),
    onInterrupt: vi.fn(),
    onRestart: vi.fn(),
    onClear: vi.fn(),
    ...overrides,
  };
  return { props, ...render(<MemoryRouter><NotebookHeader {...props} /></MemoryRouter>) };
}

describe("NotebookHeader", () => {
  it("shows the kernel status and fires Run all", async () => {
    const { props } = renderHeader();
    expect(screen.getByText("Ready")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /run all/i }));
    expect(props.onRunAll).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- NotebookHeader`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the header**

Create `src/components/notebook/NotebookHeader.tsx`:

```tsx
import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowLeft, Eraser, Play, RotateCcw, Square } from "lucide-react";
import type { KernelStatus } from "@/lib/kernel/KernelConnection";
import { KernelStatusBadge } from "./KernelStatusBadge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";

export interface NotebookHeaderProps {
  ws: string;
  name: string;
  isPublic: boolean;
  status: KernelStatus;
  runningAll: boolean;
  onRename: (name: string) => void;
  onTogglePublic: (v: boolean) => void;
  onRunAll: () => void;
  onInterrupt: () => void;
  onRestart: () => void;
  onClear: () => void;
}

export function NotebookHeader(props: NotebookHeaderProps) {
  const [localName, setLocalName] = useState(props.name);
  useEffect(() => setLocalName(props.name), [props.name]);

  const commitName = () => {
    const trimmed = localName.trim();
    if (trimmed && trimmed !== props.name) props.onRename(trimmed);
  };

  return (
    <div className="sticky top-0 z-10 flex flex-col gap-3 border-b bg-background/95 px-4 py-3 backdrop-blur">
      <Link
        to={`/explore/${props.ws}/notebooks`}
        className="flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="h-3.5 w-3.5" />
        Notebooks
      </Link>

      <div className="flex flex-wrap items-center gap-4">
        <Input
          value={localName}
          onChange={(e) => setLocalName(e.target.value)}
          onBlur={commitName}
          className="max-w-xs text-base font-semibold"
          aria-label="Notebook name"
        />

        <div className="flex items-center gap-2">
          <Switch
            id="nb-public"
            checked={props.isPublic}
            onCheckedChange={props.onTogglePublic}
            aria-label="Public"
          />
          <Label htmlFor="nb-public" className="text-sm">
            Public
          </Label>
        </div>

        <KernelStatusBadge status={props.status} />

        <div className="ml-auto flex items-center gap-1">
          <Button size="sm" variant="outline" onClick={props.onRunAll} disabled={props.runningAll}>
            <Play className="h-3.5 w-3.5" />
            Run all
          </Button>
          <Button size="sm" variant="outline" onClick={props.onInterrupt}>
            <Square className="h-3.5 w-3.5" />
            Interrupt
          </Button>
          <Button size="sm" variant="outline" onClick={props.onRestart}>
            <RotateCcw className="h-3.5 w-3.5" />
            Restart
          </Button>
          <Button size="sm" variant="ghost" onClick={props.onClear}>
            <Eraser className="h-3.5 w-3.5" />
            Clear outputs
          </Button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- NotebookHeader`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/components/notebook/NotebookHeader.tsx src/components/notebook/__tests__/NotebookHeader.test.tsx
git commit -m "feat(notebook): extracted sticky header with kernel badge"
```

---

## Task 10: Upgrade CellEditor (gutter, accent, markdown toggle, Shift+Enter)

**Files:**
- Modify: `src/components/notebook/CellEditor.tsx`
- Test: `src/components/notebook/__tests__/CellEditor.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/notebook/__tests__/CellEditor.test.tsx`. (Monaco's `@monaco-editor/react` is heavy in jsdom, so mock it with a plain textarea that forwards `onChange`.)

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { CellEditor } from "../CellEditor";
import type { Cell } from "@/api/notebooks";

vi.mock("@monaco-editor/react", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (v?: string) => void }) => (
    <textarea aria-label="code" value={value} onChange={(e) => onChange?.(e.target.value)} />
  ),
}));

function cell(partial: Partial<Cell> & Pick<Cell, "id" | "cell_type" | "source" | "cell_order">): Cell {
  return { execution_count: null, last_output: null, last_execution_time_ms: null, ...partial };
}

const noop = () => {};
const baseProps = {
  onChangeSource: noop,
  onChangeType: noop,
  onDelete: noop,
  onMoveUp: noop,
  onMoveDown: noop,
};

describe("CellEditor", () => {
  it("shows the In [N] execution prompt for code cells", () => {
    render(
      <CellEditor
        {...baseProps}
        cell={cell({ id: "a", cell_type: "code", source: "x = 1", cell_order: 0, execution_count: 7 })}
      />,
    );
    expect(screen.getByText("In [7]:")).toBeInTheDocument();
  });

  it("renders markdown as preview when the source is set and cell is unfocused", () => {
    render(
      <CellEditor
        {...baseProps}
        cell={cell({ id: "b", cell_type: "markdown", source: "# Hello", cell_order: 0 })}
      />,
    );
    expect(screen.getByRole("heading", { name: "Hello" })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- CellEditor`
Expected: FAIL — no `In [7]:` prompt exists yet; markdown currently always shows the textarea + preview together, not a focus-gated preview.

- [ ] **Step 3: Rewrite CellEditor**

Replace the entire contents of `src/components/notebook/CellEditor.tsx`:

```tsx
import { useRef, useState } from "react";
import Editor from "@monaco-editor/react";
import type { Monaco } from "@monaco-editor/react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ArrowDown, ArrowUp, ChevronDown, Loader2, Play, Trash2 } from "lucide-react";

import type { Cell, CellType } from "@/api/notebooks";
import { Button } from "@/components/ui/button";
import { CellOutputRenderer } from "@/components/notebook/CellOutputRenderer";
import { registerStaticCompletions } from "@/components/notebook/monaco/registerStatic";
import type { CellOutput } from "@/lib/kernel/protocol";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

export interface CellEditorProps {
  cell: Cell;
  onChangeSource: (source: string) => void;
  onChangeType: (type: CellType) => void;
  onDelete: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
  onRun?: () => void;
  running?: boolean;
  outputs?: CellOutput[];
  onRunAbove?: () => void;
  onRunBelow?: () => void;
}

export function CellEditor({
  cell,
  onChangeSource,
  onChangeType,
  onDelete,
  onMoveUp,
  onMoveDown,
  onRun,
  running,
  outputs,
  onRunAbove,
  onRunBelow,
}: CellEditorProps) {
  const [mdFocused, setMdFocused] = useState(false);
  // Latest onRun for the Monaco Shift+Enter command (captured once on mount).
  const runRef = useRef<(() => void) | undefined>(onRun);
  runRef.current = onRun;

  const errored = outputs?.some((o) => o.type === "error") ?? false;
  // Left accent bar keyed to state.
  const accent = running
    ? "var(--primary)"
    : errored
      ? "var(--destructive)"
      : outputs && outputs.length > 0
        ? "var(--success)"
        : "var(--border)";

  const execLabel =
    cell.cell_type === "markdown"
      ? ""
      : cell.execution_count == null
        ? "In [ ]:"
        : `In [${cell.execution_count}]:`;

  const handleMount = (
    editor: Parameters<NonNullable<React.ComponentProps<typeof Editor>["onMount"]>>[0],
    monaco: Monaco,
  ) => {
    registerStaticCompletions(monaco);
    editor.addCommand(monaco.KeyMod.Shift | monaco.KeyCode.Enter, () => runRef.current?.());
  };

  return (
    <div
      className="group flex rounded-lg border bg-card shadow-sm"
      style={{ borderLeft: `3px solid ${accent}` }}
    >
      {/* Gutter */}
      <div className="w-16 shrink-0 select-none px-2 py-2 text-right font-mono text-[11px] text-muted-foreground">
        {running ? <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin" /> : execLabel}
      </div>

      <div className="min-w-0 flex-1">
        {/* Toolbar — revealed on hover/focus */}
        <div className="flex items-center gap-1 border-b px-2 py-1 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
          <Select value={cell.cell_type} onValueChange={(v) => onChangeType(v as CellType)}>
            <SelectTrigger className="h-7 w-28 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="code">Code</SelectItem>
              <SelectItem value="markdown">Markdown</SelectItem>
            </SelectContent>
          </Select>

          <div className="ml-auto flex items-center gap-1">
            <Button variant="ghost" size="sm" className="h-7 w-7 p-0" onClick={onMoveUp} aria-label="Move cell up">
              <ArrowUp className="h-3.5 w-3.5" />
            </Button>
            <Button variant="ghost" size="sm" className="h-7 w-7 p-0" onClick={onMoveDown} aria-label="Move cell down">
              <ArrowDown className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 w-7 p-0 text-muted-foreground hover:text-destructive"
              onClick={onDelete}
              aria-label="Delete cell"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1 text-xs"
              disabled={!onRun || running}
              title={running ? "Running…" : "Run (Shift+Enter)"}
              aria-label="Run"
              onClick={onRun}
            >
              {running ? <Loader2 className="h-3 w-3 animate-spin" /> : <Play className="h-3 w-3" />}
              Run
            </Button>
            {cell.cell_type !== "markdown" && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button size="icon" variant="ghost" className="h-7 w-7" aria-label="Run options">
                    <ChevronDown className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem disabled={!onRunAbove} onClick={onRunAbove}>
                    Run all above
                  </DropdownMenuItem>
                  <DropdownMenuItem disabled={!onRunBelow} onClick={onRunBelow}>
                    Run all below
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </div>
        </div>

        {/* Body */}
        {cell.cell_type === "markdown" ? (
          mdFocused || !cell.source.trim() ? (
            <textarea
              autoFocus={mdFocused}
              className="min-h-[80px] w-full resize-y bg-transparent p-3 font-mono text-sm focus:outline-none"
              value={cell.source}
              onChange={(e) => onChangeSource(e.target.value)}
              onBlur={() => setMdFocused(false)}
              aria-label="Markdown source"
              placeholder="Write markdown…"
            />
          ) : (
            <div
              role="button"
              tabIndex={0}
              onClick={() => setMdFocused(true)}
              onKeyDown={(e) => e.key === "Enter" && setMdFocused(true)}
              className="cursor-text px-3 py-2 text-sm [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_h1]:mb-1 [&_h1]:text-lg [&_h1]:font-semibold [&_h2]:mb-1 [&_h2]:text-base [&_h2]:font-semibold [&_ol]:list-decimal [&_ol]:pl-5 [&_ul]:list-disc [&_ul]:pl-5"
            >
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{cell.source}</ReactMarkdown>
            </div>
          )
        ) : (
          <div className="min-h-[120px]">
            <Editor
              height="200px"
              defaultLanguage="python"
              language="python"
              value={cell.source}
              onChange={(v) => onChangeSource(v ?? "")}
              onMount={handleMount}
              options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }}
            />
          </div>
        )}

        {outputs && outputs.length > 0 && <CellOutputRenderer outputs={outputs} />}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm run test -- CellEditor`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/components/notebook/CellEditor.tsx src/components/notebook/__tests__/CellEditor.test.tsx
git commit -m "feat(notebook): richer cell chrome (gutter In[N], accent bar, markdown preview, Shift+Enter)"
```

---

## Task 11: Wire NotebookPage into the IDE shell

**Files:**
- Modify: `src/routes/explore/NotebookPage.tsx`
- Test: `src/routes/explore/__tests__/NotebookPage.test.tsx` (extend existing)

- [ ] **Step 1: Add a test for the shell wiring**

Open `src/routes/explore/__tests__/NotebookPage.test.tsx`. Add a test asserting the sidebar rail and kernel badge render. If the existing file already mocks `@/api/notebooks` and `useKernel`, reuse those mocks; otherwise add this test in a new `describe`:

```tsx
it("renders the sidebar rail and a kernel status badge", async () => {
  // (Uses the file's existing render helper + notebook mock returning a
  // notebook with cells. If the helper is named differently, adapt.)
  renderNotebookPage(); // existing helper
  expect(await screen.findByRole("button", { name: /table of contents/i })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /data sources/i })).toBeInTheDocument();
});
```

If there is no reusable helper, model the render on `NotebookSettings.test.tsx` (QueryClientProvider + MemoryRouter with route `"/explore/:workspace/notebooks/:id"`), mocking `@/api/notebooks` hooks and `@/lib/kernel/useKernel` to return a stub `KernelConnection` (`{ status: "idle", onStatusChange: (cb) => { cb("idle"); return () => {}; }, connect, execute, interrupt, restart, disconnect }`).

- [ ] **Step 2: Run test to verify it fails**

Run: `npm run test -- NotebookPage`
Expected: FAIL — no rail buttons yet.

- [ ] **Step 3: Restructure NotebookPage**

Edit `src/routes/explore/NotebookPage.tsx`:

1. Add imports:

```tsx
import { useKernelStatus } from "@/lib/kernel/useKernelStatus";
import { NotebookHeader } from "@/components/notebook/NotebookHeader";
import { SidebarIconRail, type SidebarTab } from "@/components/notebook/SidebarIconRail";
import { TableOfContentsPanel } from "@/components/notebook/panels/TableOfContentsPanel";
import { DataSourcesPanel } from "@/components/notebook/panels/DataSourcesPanel";
```

2. In `NotebookPage`, after `const active = useActiveExecutions(...)`, add:

```tsx
  const kernelStatus = useKernelStatus(kernel);
  const [sidebarTab, setSidebarTab] = useState<SidebarTab>("toc");
  const [sidebarOpen, setSidebarOpen] = useState(true);

  const pickTab = (t: SidebarTab) => {
    if (t === sidebarTab) setSidebarOpen((v) => !v);
    else {
      setSidebarTab(t);
      setSidebarOpen(true);
    }
  };

  const jumpToCell = (cellId: string) => {
    document.getElementById(`cell-${cellId}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
  };
```

3. Remove the old inline header block (the `<div className="flex flex-col gap-3">` containing the back-link, name `Input`, public `Switch`, and run-controls `<div>`), and remove the now-unused `localName`/`handleNameBlur`/`handlePublicToggle` state — replaced by `NotebookHeader`. Keep `nameDirty`/`useUnsavedChangesWarning` by deriving `nameDirty` from a ref the header reports, OR simplify: keep the beforeunload guard on `pendingCells.size > 0` only (name now commits on blur inside the header). Use the simplified guard:

```tsx
  useUnsavedChangesWarning(pendingCells.size > 0);
```

4. Replace the top-level `return (<section …>)` structure with the 3-region shell:

```tsx
  return (
    <div className="flex h-full min-h-0">
      <SidebarIconRail tab={sidebarTab} open={sidebarOpen} onPick={pickTab} />
      {sidebarOpen && (
        <div className="w-64 shrink-0 overflow-y-auto border-r bg-muted/20">
          {sidebarTab === "toc" ? (
            <TableOfContentsPanel cells={sortedCells} onJump={jumpToCell} />
          ) : (
            <DataSourcesPanel ws={ws!} />
          )}
        </div>
      )}

      <section className="flex min-w-0 flex-1 flex-col overflow-y-auto">
        <h1 className="sr-only">{nb.name}</h1>
        <NotebookHeader
          ws={ws!}
          name={nb.name}
          isPublic={nb.is_public}
          status={kernelStatus}
          runningAll={runningAll}
          onRename={(name) => updateNotebook.mutate({ name })}
          onTogglePublic={(v) => updateNotebook.mutate({ is_public: v })}
          onRunAll={runAll}
          onInterrupt={() =>
            kernel.interrupt().catch((e) => toast.error(String((e as Error)?.message ?? e)))
          }
          onRestart={() =>
            kernel.restart().catch((e) => toast.error(String((e as Error)?.message ?? e)))
          }
          onClear={clearAll}
        />

        <div className="flex flex-col gap-3 p-4">
          {sortedCells.map((cell, index) => (
            <div key={cell.id} id={`cell-${cell.id}`} style={{ scrollMarginTop: "5rem" }}>
              <ManagedCell
                cell={cell}
                kernel={kernel}
                onChangeType={handleChangeType}
                onDelete={handleDeleteCell}
                onMoveUp={handleMoveUp}
                onMoveDown={handleMoveDown}
                updateCellMutate={updateCell.mutate}
                runners={runners}
                onPendingChange={setCellPending}
                initialRunning={runningSet.has(cell.id)}
                onRunAbove={index > 0 ? () => runSlice(0, index) : undefined}
                onRunBelow={
                  index < sortedCells.length - 1
                    ? () => runSlice(index + 1, sortedCells.length)
                    : undefined
                }
              />
            </div>
          ))}

          <div className="flex items-center gap-2">
            <Button variant="outline" size="sm" onClick={() => handleAddCell("code")} aria-label="Add cell">
              <Plus className="h-4 w-4" />
              Add cell
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => handleAddCell("markdown")}
              aria-label="Add markdown cell"
            >
              <Plus className="h-4 w-4" />
              Add markdown
            </Button>
          </div>
        </div>
      </section>
    </div>
  );
```

5. Delete now-unused imports (`ArrowLeft`, `Eraser`, `Play`, `RotateCcw`, `Square`, `Input`, `Label`, `Switch` if no longer referenced in this file — the header owns them now). Keep `Plus` and `Button`. Run typecheck to catch leftovers.

- [ ] **Step 4: Run tests + typecheck**

Run: `npm run test -- NotebookPage`
Expected: PASS.
Run: `npm run typecheck`
Expected: no errors (fix any unused-import errors surfaced).

- [ ] **Step 5: Commit**

```bash
git add src/routes/explore/NotebookPage.tsx src/routes/explore/__tests__/NotebookPage.test.tsx
git commit -m "feat(notebook): compose IDE shell (rail + panels + sticky header)"
```

---

## Task 12: Full frontend verification

**Files:** none (verification only)

- [ ] **Step 1: Lint**

Run: `npm run lint`
Expected: clean (fix any issues in files touched by this plan).

- [ ] **Step 2: Typecheck**

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 3: Test**

Run: `npm run test`
Expected: all pass (new tests + the prior suite green).

- [ ] **Step 4: Build**

Run: `npm run build`
Expected: build succeeds.

- [ ] **Step 5: Live smoke (manual, optional but recommended)**

Start the stack per `egret-frontend-viz-verify` + the notebook live-e2e recipe (kernel venv on the serve PATH + a provisioned warehouse + Vite `ws:true`). Open a notebook and verify: the rail toggles TOC/Data-Sources panels; the kernel badge transitions Disconnected → Connecting… → Ready on first Run; a code cell shows `In [1]:` and its output; a markdown cell renders a preview when unfocused; an error cell shows the destructive error box; stderr collapses under "Logs".

- [ ] **Step 6: Commit (if any lint/type fixups were made)**

```bash
git add -A
git commit -m "chore(notebook): P1 lint/type/test/build green"
```

---

## Self-Review Notes (author)

- **Spec §4.1 shell** → Tasks 8, 11. **§4.2 header** → Task 9. **§4.3 badge + status stream** → Tasks 1–3. **§4.4 cell** → Task 10 (SQL target/into intentionally deferred to P2; the type select stays Code/Markdown). **§4.5 output** → Task 4. **§4.6 panels** → Tasks 6, 7. **§4.7 monaco** → Task 5. All P1 spec items covered.
- **Deviation:** the spec listed a separate `NotebookShell.tsx`; this plan composes the 3-region flex directly in `NotebookPage` (YAGNI — the page is already the composition root) and keeps `SidebarIconRail` as the tested unit.
- **Type consistency:** `KernelStatus` union widened in Task 1 and consumed identically in Tasks 2/3/9/11; `SidebarTab = "toc" | "dataSources"` defined in Task 8 and imported in Task 11; `CellOutput` variants used in Task 4 match `protocol.ts` exactly (`stream|execute_result|display_data|error|truncated`).
- **No placeholders:** every code step contains full source.
- **Out of scope (P2/P3):** SQL cell target dropdown + `sql_into`, `%%egret_sql` magic, migration 0048, OpenAPI regen, Daft-local target.
